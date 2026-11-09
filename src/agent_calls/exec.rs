//! Running one framed prompt on the agent
//! (docs/design/EXTENSION_AGENT_CALLS.md §5.4, §5.5).
//!
//! [`AgentBackend`] is everything the engine needs from the agent: the
//! turn conditions, one blocking run, the reply's size while it streams,
//! and `session/cancel`. [`AcpBackend`] is the real one. Tests use a fake.
//!
//! A real run goes through a strict ephemeral session: tool requests are
//! refused, the agent is never started or reconnected, and its working
//! folder is an empty Kage-owned scratch folder. That session primitive
//! is `ephemeral_session::run_with` (WP-AC2); the backend reaches it
//! through a [`SessionOpener`] it is given, so this module builds on its
//! own and the wiring passes `run_with` in.

use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::acp_client::AcpClient;
use crate::config::Config;
use crate::lock_ext::LockExt;

/// Why a running call was stopped before it finished on its own. The
/// first stop wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The request's timeout passed (seconds).
    Timeout(u32),
    /// The reply grew past the cap (characters).
    ResponseTooLarge(u32),
    /// The agent asked for a tool, or started one.
    ToolAttempted,
    /// A user prompt started.
    UserActivity,
    /// **Stop now**.
    ByUser,
    /// The extension cancelled, or its sandbox went away.
    ByExtension,
    /// Kage is quitting.
    Shutdown,
    /// Background AI was turned off, or the extension was disabled,
    /// uninstalled or lost its grant.
    AccessOff,
    /// The active agent connection changed.
    ConsentStale,
    /// The agent disconnected.
    Disconnected,
}

/// What [`ephemeral_session::run_with`](crate::ephemeral_session) is asked
/// for. Background calls always use `strict`, `require_connected` and the
/// scratch folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpec {
    /// Refuse every tool request on the session, before any policy.
    pub strict: bool,
    /// Never spawn or reconnect the agent.
    pub require_connected: bool,
    /// The session's working folder.
    pub cwd: Option<PathBuf>,
}

/// Opens a throwaway session per the spec, runs the work on its id, and
/// deletes the session whatever the work returns. Blocking.
pub type SessionOpener = Arc<
    dyn Fn(SessionSpec, &mut dyn FnMut(&str) -> anyhow::Result<String>) -> anyhow::Result<String>
        + Send
        + Sync,
>;

/// Why a run ended without a reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// The agent wasn't connected; nothing was sent.
    Unavailable,
    /// A stop came before the prompt was sent.
    Stopped,
    /// The agent (or the session around it) failed. English text for the
    /// extension; never logged.
    Agent(String),
}

/// Everything the engine needs from the agent.
pub trait AgentBackend: Send + Sync {
    fn connected(&self) -> bool;
    /// The agent is compacting its context.
    fn compacting(&self) -> bool;
    /// A user prompt is in flight in some window.
    fn user_prompt_in_flight(&self) -> bool;
    /// Low battery, or the power mode is "saving".
    fn low_power(&self) -> bool;
    /// Run `prompt` and return the whole reply. Blocking: call it from a
    /// blocking thread. Must call [`RunControl::attach`] with the session
    /// id before sending, and not send when it returns `false`.
    fn run(&self, prompt: &str, ctl: &RunControl) -> Result<String, RunError>;
    /// Bytes of the reply received so far.
    fn reply_len(&self, session_id: &str) -> usize;
    /// Send `session/cancel`.
    fn cancel(&self, session_id: &str);
}

#[derive(Debug, Default)]
struct RunInner {
    session: Option<String>,
    sent: bool,
    stop: Option<Stop>,
    last_cancel: Option<DateTime<Utc>>,
}

/// Shared between a run, its watchdog and the hooks that stop it.
#[derive(Debug, Default)]
pub struct RunControl {
    inner: Mutex<RunInner>,
    /// Woken when the run is stopped or finishes.
    pub(super) changed: tokio::sync::Notify,
}

impl RunControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the session the prompt goes to. `false` means a stop came
    /// first: don't send.
    pub fn attach(&self, session_id: &str) -> bool {
        let mut i = self.inner.lock_or_recover();
        i.session = Some(session_id.to_string());
        if i.stop.is_some() {
            return false;
        }
        i.sent = true;
        true
    }

    pub fn session(&self) -> Option<String> {
        self.inner.lock_or_recover().session.clone()
    }

    /// Whether the prompt went out (attach returned `true`).
    pub fn was_sent(&self) -> bool {
        self.inner.lock_or_recover().sent
    }

    pub fn stopped(&self) -> Option<Stop> {
        self.inner.lock_or_recover().stop
    }

    /// Record `stop` unless another came first. Returns whether this one
    /// won, and the session to send `session/cancel` to.
    pub(super) fn request_stop(&self, stop: Stop, now: DateTime<Utc>) -> (bool, Option<String>) {
        let mut i = self.inner.lock_or_recover();
        let first = i.stop.is_none();
        if first {
            i.stop = Some(stop);
        }
        let session = i.session.clone();
        if session.is_some() {
            i.last_cancel = Some(now);
        }
        drop(i);
        self.changed.notify_one();
        (first, session)
    }

    /// After a stop, the session to cancel again, at most every
    /// [`CANCEL_RESEND_MS`](super::limits::CANCEL_RESEND_MS).
    pub(super) fn cancel_due(&self, now: DateTime<Utc>) -> Option<String> {
        let mut i = self.inner.lock_or_recover();
        i.stop?;
        let session = i.session.clone()?;
        let due = i.last_cancel.is_none_or(|t| {
            now - t >= chrono::Duration::milliseconds(super::limits::CANCEL_RESEND_MS)
        });
        if !due {
            return None;
        }
        i.last_cancel = Some(now);
        Some(session)
    }
}

/// The empty working folder strict sessions run in:
/// `<data_local_dir>/kage/agent-scratch/`, owner-only on Unix.
pub fn scratch_dir() -> std::io::Result<PathBuf> {
    let dir = crate::paths::data_local_dir()
        .ok_or_else(|| std::io::Error::other("no local data folder"))?
        .join("kage")
        .join("agent-scratch");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// Whether `e` is `run_with`'s `agent_unavailable` refusal (an `AppError`
/// inside the `anyhow::Error`).
fn is_unavailable(e: &anyhow::Error) -> bool {
    e.downcast_ref::<crate::error::AppError>()
        .is_some_and(|a| a.key == "errors.agent_call.agent_unavailable")
}

/// The agent behind Kage's ACP client.
pub struct AcpBackend {
    client: Arc<AcpClient>,
    config: Arc<Mutex<Config>>,
    open: SessionOpener,
}

impl AcpBackend {
    /// `open` must be `ephemeral_session::run_with` (or an equivalent that
    /// honours every field of [`SessionSpec`]).
    pub fn new(client: Arc<AcpClient>, config: Arc<Mutex<Config>>, open: SessionOpener) -> Self {
        Self {
            client,
            config,
            open,
        }
    }
}

impl AgentBackend for AcpBackend {
    fn connected(&self) -> bool {
        self.client.is_connected()
    }

    fn compacting(&self) -> bool {
        *self.client.compacting.0.lock_or_recover()
    }

    fn user_prompt_in_flight(&self) -> bool {
        !self.client.in_flight_prompts.lock_or_recover().is_empty()
    }

    fn low_power(&self) -> bool {
        let saving = self.config.lock_or_recover().automation_power.mode == "saving";
        saving || crate::os::power::get_power_state() == crate::os::power::PowerState::LowBattery
    }

    fn run(&self, prompt: &str, ctl: &RunControl) -> Result<String, RunError> {
        if !self.client.is_connected() {
            return Err(RunError::Unavailable);
        }
        // Never fall back to the user's working folder.
        let cwd = scratch_dir().map_err(|e| {
            RunError::Agent(format!("Kage couldn't prepare its scratch folder: {e}"))
        })?;
        let spec = SessionSpec {
            strict: true,
            require_connected: true,
            cwd: Some(cwd),
        };
        let client = &self.client;
        let mut work = |session_id: &str| -> anyhow::Result<String> {
            if !ctl.attach(session_id) {
                anyhow::bail!("stopped before the prompt was sent");
            }
            let sent = client.send_chat_streaming_background(session_id, prompt, true);
            // Take the bucket on every path, so nothing is left behind.
            let reply = client.take_session_accumulator(session_id);
            sent.map(|()| reply)
        };
        match (self.open)(spec, &mut work) {
            Ok(reply) => Ok(reply),
            Err(_) if ctl.stopped().is_some() && !ctl.was_sent() => Err(RunError::Stopped),
            Err(_) if !self.client.is_connected() && !ctl.was_sent() => Err(RunError::Unavailable),
            // `run_with` refused before anything was spawned or sent: the
            // agent is connected but not healthy (§5.5).
            Err(e) if !ctl.was_sent() && is_unavailable(&e) => Err(RunError::Unavailable),
            Err(e) => Err(RunError::Agent(e.to_string())),
        }
    }

    fn reply_len(&self, session_id: &str) -> usize {
        self.client
            .streaming_accumulators
            .lock_or_recover()
            .get(session_id)
            .map_or(0, String::len)
    }

    fn cancel(&self, session_id: &str) {
        let _ = self.client.cancel_session(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn a_stop_before_attach_prevents_the_send() {
        let ctl = RunControl::new();
        let (first, session) = ctl.request_stop(Stop::UserActivity, t("2026-11-09T10:00:00Z"));
        assert!(first);
        assert_eq!(session, None);
        assert!(!ctl.attach("s1"));
        assert!(!ctl.was_sent());
        assert_eq!(ctl.session().as_deref(), Some("s1"));
    }

    #[test]
    fn the_first_stop_wins_and_cancels_are_resent() {
        let ctl = RunControl::new();
        assert!(ctl.attach("s1"));
        let now = t("2026-11-09T10:00:00Z");
        assert_eq!(ctl.cancel_due(now), None, "nothing to cancel before a stop");
        let (first, session) = ctl.request_stop(Stop::Timeout(60), now);
        assert!(first);
        assert_eq!(session.as_deref(), Some("s1"));
        let (first, _) = ctl.request_stop(Stop::ToolAttempted, now);
        assert!(!first);
        assert_eq!(ctl.stopped(), Some(Stop::Timeout(60)));
        assert_eq!(
            ctl.cancel_due(now + chrono::Duration::milliseconds(500)),
            None
        );
        assert_eq!(
            ctl.cancel_due(now + chrono::Duration::milliseconds(2_000))
                .as_deref(),
            Some("s1")
        );
        assert_eq!(
            ctl.cancel_due(now + chrono::Duration::milliseconds(2_500)),
            None
        );
    }

    #[test]
    fn run_withs_unavailable_refusal_is_recognised() {
        let refused: anyhow::Error = crate::error::AppError::keyed(
            crate::error::ErrorKind::ConnectionLost,
            "errors.agent_call.agent_unavailable",
            &[],
        )
        .into();
        assert!(is_unavailable(&refused));
        assert!(!is_unavailable(&anyhow::anyhow!("Session creation failed")));
        let other: anyhow::Error = crate::error::AppError::keyed(
            crate::error::ErrorKind::ConnectionLost,
            "errors.agent_call.agent_disconnected",
            &[],
        )
        .into();
        assert!(!is_unavailable(&other));
    }

    #[test]
    fn scratch_folder_is_created_owner_only() {
        let dir = scratch_dir().unwrap();
        assert!(dir.is_dir());
        assert!(crate::paths::is_under_isolation_root(&dir));
        assert!(dir.ends_with("kage/agent-scratch"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
    }

    #[test]
    fn acp_backend_never_runs_without_a_connection() {
        let client = Arc::new(AcpClient::new(
            crate::acp_client::AcpConnectionMode::Local {
                spawn_command: "true".to_string(),
            },
        ));
        let opened = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = opened.clone();
        let open: SessionOpener = Arc::new(move |_spec, _work| {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(String::new())
        });
        let backend = AcpBackend::new(client, Arc::new(Mutex::new(Config::default())), open);
        assert!(!backend.connected());
        assert!(!backend.compacting());
        assert!(!backend.user_prompt_in_flight());
        assert_eq!(
            backend.run("p", &RunControl::new()),
            Err(RunError::Unavailable)
        );
        assert!(
            !opened.load(std::sync::atomic::Ordering::SeqCst),
            "no session opened"
        );
        assert_eq!(backend.reply_len("nope"), 0);
    }
}
