//! Ephemeral ACP sessions: create a throwaway session, do one self-
//! contained piece of work on it, and tear it down so nothing leaks into
//! the user's chat UI or the on-disk session list.
//!
//! # Why this exists
//!
//! Several features need the agent for a one-shot task that isn't part of
//! any conversation: generating a script from a prompt (Settings → script
//! editor), and — candidates for migration — AI session titling and
//! quit-time steering. The wrong way to do this is to borrow a real
//! window's session: it may not exist (Settings is often open with no
//! chat window, so `get_window_session` returns `null`), and even when it
//! does, the hidden prompt and its reply pollute the user's actual
//! conversation history.
//!
//! [`run`] is the single primitive: it ensures the connection is up,
//! creates a fresh session, hands the id to a caller-supplied closure,
//! and guarantees the session files are removed afterwards (via an RAII
//! guard, so an early `?` or a panic in the closure still cleans up). The
//! teardown deletes the session files directly rather than routing through
//! the `delete_session` command, because we don't want its chat-host
//! `session_changed` broadcast — no window ever knew about this session.
//!
//! Run it from a blocking context (`spawn_blocking`): the ACP client's
//! `create_session` / `send_chat_streaming` are synchronous and block the
//! calling thread until the agent replies.
//!
//! [`run_with`] adds options for background agent calls
//! (docs/design/EXTENSION_AGENT_CALLS.md §5.5): a **strict** session that
//! no tool may run on ([`strict`]), never starting the agent, and an empty
//! Kage-owned working folder ([`scratch`]). Every throwaway session id is
//! also recorded so no Kage session list shows it ([`hidden`]).

mod hidden;
mod scratch;
mod strict;

use anyhow::{Context, Result};
use log::warn;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use crate::acp_client::AcpClient;
use crate::config::Config;
use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;

pub use scratch::{prepare_scratch_dir, scratch_dir};
pub use strict::{is_strict, note_tool_attempt, notify_disconnected};
#[allow(unused_imports)] // Used by the background agent-call engine.
pub use strict::{set_strict_hooks, tool_attempted, StrictHooks};

/// Ids of ephemeral sessions currently alive. The session scanner skips
/// these, so a throwaway session never flashes into the sidebar between
/// the agent writing its files and the cleanup guard deleting them.
static ACTIVE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// True while `session_id` is an ephemeral session owned by [`run`].
pub fn is_active(session_id: &str) -> bool {
    ACTIVE.lock_or_recover().contains(session_id)
}

/// The recorded throwaway session ids, loaded from disk on first use.
static HIDDEN: LazyLock<Mutex<hidden::HiddenSessions>> = LazyLock::new(|| {
    Mutex::new(hidden::HiddenSessions::load(
        hidden::default_path(),
        chrono::Utc::now(),
    ))
});

/// True when `session_id` is one of Kage's throwaway sessions, from this
/// run or an earlier one (kept 30 days). Session lists skip these.
pub fn is_hidden(session_id: &str) -> bool {
    HIDDEN.lock_or_recover().contains(session_id)
}

/// Every recorded throwaway session id (see [`is_hidden`]).
pub fn hidden_ids() -> HashSet<String> {
    HIDDEN.lock_or_recover().ids()
}

fn record_hidden(session_id: &str) {
    HIDDEN
        .lock_or_recover()
        .record(session_id, chrono::Utc::now());
}

/// How [`run_with`] sets up a throwaway session. The default is what
/// [`run`] has always done.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EphemeralOptions {
    /// No tool may run: every permission request is refused before any
    /// policy, and the turn is cancelled at the first tool attempt
    /// ([`strict`]).
    pub strict: bool,
    /// Never start or reconnect the agent. When it isn't connected, fail
    /// with `errors.agent_call.agent_unavailable` (an [`AppError`] inside
    /// the `anyhow::Error`).
    pub require_connected: bool,
    /// The session's working folder. `None` uses the configured one. The
    /// scratch folder ([`scratch_dir`]) is checked and emptied around a
    /// strict session; no other folder is ever touched.
    pub cwd: Option<PathBuf>,
}

impl EphemeralOptions {
    /// The options a background agent call uses: strict, never start the
    /// agent, and run in the empty scratch folder (created if needed).
    #[allow(dead_code)] // Used by the background agent-call engine.
    pub fn background() -> std::io::Result<Self> {
        Ok(Self {
            strict: true,
            require_connected: true,
            cwd: Some(prepare_scratch_dir()?),
        })
    }
}

/// Deletes an ephemeral session's on-disk files when dropped. Best-effort:
/// a failure leaves a stray file the user can delete manually, so we log
/// rather than propagate. Holding this as a `let _guard = …` binding means
/// cleanup runs on every exit path out of [`run`] — normal return, `?`
/// early-return, or a panic unwinding through the closure.
struct SessionCleanup {
    config: Arc<Mutex<Config>>,
    session_id: String,
    /// Strict sessions leave the strict set after their files are gone.
    strict: bool,
    /// The scratch folder to check and empty, for a strict session run in it.
    scratch: Option<PathBuf>,
}

#[cfg(test)]
impl SessionCleanup {
    fn new(config: Arc<Mutex<Config>>, session_id: String) -> Self {
        Self {
            config,
            session_id,
            strict: false,
            scratch: None,
        }
    }
}

impl Drop for SessionCleanup {
    fn drop(&mut self) {
        // Unregister only after the files are gone (end of this fn), so the
        // scanner can't list them in between.
        struct Unregister<'a> {
            session_id: &'a str,
            strict: bool,
        }
        impl Drop for Unregister<'_> {
            fn drop(&mut self) {
                if self.strict {
                    strict::end(self.session_id);
                }
                ACTIVE.lock_or_recover().remove(self.session_id);
            }
        }
        let _unregister = Unregister {
            session_id: &self.session_id,
            strict: self.strict,
        };

        if let Some(dir) = self.scratch.as_deref() {
            scratch::check_and_empty(dir, &self.session_id, "after");
        }

        let sessions_dir = {
            let cfg = self.config.lock_or_recover();
            crate::agent_presets::resolve_sessions_dir(&cfg)
        };
        let Some(dir) = sessions_dir else {
            warn!(
                "Could not resolve sessions dir to clean up ephemeral session {}",
                self.session_id
            );
            return;
        };
        for ext in &["json", "jsonl", "lock"] {
            let path = dir.join(format!("{}.{}", self.session_id, ext));
            if path.exists() {
                if let Err(e) = std::fs::remove_file(&path) {
                    warn!("Failed to delete ephemeral session file {:?}: {}", path, e);
                }
            }
        }
    }
}

/// Run `work` against a fresh, throwaway ACP session, then delete it.
///
/// Connects the client if needed, creates a new session (no steering
/// primer — ephemeral work is self-contained, not a conversation), invokes
/// `work` with the new session id, and tears the session down regardless of
/// how `work` returns. The closure's value is propagated to the caller.
///
/// Call from a blocking context — the ACP calls inside `work` (and the
/// `create_session` here) are synchronous.
///
/// ```ignore
/// let reply = ephemeral_session::run(&client, &config, |sid| {
///     client.send_chat_streaming_background(sid, &prompt, true)?;
///     Ok(client.take_session_accumulator(sid))
/// })?;
/// ```
pub fn run<T, F>(client: &Arc<AcpClient>, config: &Arc<Mutex<Config>>, work: F) -> Result<T>
where
    F: FnOnce(&str) -> Result<T>,
{
    run_with(client, config, EphemeralOptions::default(), work)
}

/// [`run`], set up by `options`. With the defaults it is exactly [`run`].
///
/// - `require_connected`: fails with `errors.agent_call.agent_unavailable`
///   instead of connecting; nothing is spawned or dialled.
/// - `cwd`: the session's working folder instead of the configured one.
/// - `strict`: the session is strict ([`is_strict`]) from creation until
///   its files are gone, and for a while after. Run in the scratch folder,
///   that folder is emptied before the session starts and checked again
///   when it ends (a warning is logged if anything appeared).
///
/// Every session is recorded as hidden ([`is_hidden`]) as soon as it
/// exists.
pub fn run_with<T, F>(
    client: &Arc<AcpClient>,
    config: &Arc<Mutex<Config>>,
    options: EphemeralOptions,
    work: F,
) -> Result<T>
where
    F: FnOnce(&str) -> Result<T>,
{
    if options.require_connected {
        if !client.is_healthy() {
            return Err(AppError::keyed(
                ErrorKind::ConnectionLost,
                "errors.agent_call.agent_unavailable",
                &[],
            )
            .into());
        }
    } else if !client.is_connected() {
        client
            .connect()
            .context("Failed to connect for ephemeral session")?;
    }

    let cwd = match options.cwd.as_deref() {
        Some(dir) => Some(dir.to_string_lossy().into_owned()),
        None => {
            let cfg = config.lock_or_recover();
            cfg.acp.agent.working_directory.clone()
        }
    };

    // Only Kage's own scratch folder is ever emptied, never a folder a
    // caller passed in.
    let scratch = options
        .cwd
        .clone()
        .filter(|dir| options.strict && Some(dir) == scratch_dir().as_ref());
    if let Some(dir) = scratch.as_deref() {
        scratch::ensure_private_dir(dir).context("Failed to prepare the agent scratch folder")?;
        scratch::check_and_empty(dir, "(new)", "before");
    }

    let (session_id, _) = client
        .create_session(cwd)
        .context("Failed to create ephemeral session")?;

    // Arm cleanup before running the work so an error or panic in `work`
    // still removes the session files. Strict before anything can prompt
    // the session: the agent can't ask for a tool before its first prompt.
    ACTIVE.lock_or_recover().insert(session_id.clone());
    if options.strict {
        strict::begin(&session_id);
    }
    let _cleanup = SessionCleanup {
        config: config.clone(),
        session_id: session_id.clone(),
        strict: options.strict,
        scratch,
    };
    record_hidden(&session_id);

    work(&session_id)
}

/// Convenience wrapper for the common "send one prompt, get the full
/// reply" shape. Runs `prompt` on a fresh ephemeral session and returns
/// the agent's accumulated response text.
pub fn prompt_once(
    client: &Arc<AcpClient>,
    config: &Arc<Mutex<Config>>,
    prompt: &str,
) -> Result<String> {
    run(client, config, |session_id| {
        // The background sender keeps the reply out of every window's
        // stream (no chunks or tool-call updates are emitted) while still
        // accumulating it; it blocks until the prompt completes, leaving
        // the full reply in the accumulator for us to take.
        client.send_chat_streaming_background(session_id, prompt, true)?;
        Ok(client.take_session_accumulator(session_id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// A Config whose active connection points its sessions dir at `dir`.
    /// That's the override `resolve_sessions_dir` honours first, so the
    /// cleanup guard deletes from `dir`.
    fn config_with_sessions_dir(dir: &std::path::Path) -> Arc<Mutex<Config>> {
        let mut cfg = Config::default();
        cfg.acp
            .active_connection_mut()
            .expect("default config seeds a connection")
            .sessions_directory = Some(dir.to_string_lossy().to_string());
        Arc::new(Mutex::new(cfg))
    }

    /// Create the on-disk files the agent would leave for a session:
    /// `<id>.json`, `<id>.jsonl`, `<id>.lock`.
    fn touch_session_files(dir: &std::path::Path, id: &str) {
        for ext in &["json", "jsonl", "lock"] {
            std::fs::write(dir.join(format!("{id}.{ext}")), b"x").unwrap();
        }
    }

    fn session_files_exist(dir: &std::path::Path, id: &str) -> bool {
        ["json", "jsonl", "lock"]
            .iter()
            .any(|ext| dir.join(format!("{id}.{ext}")).exists())
    }

    #[test]
    fn active_registration_ends_when_the_guard_drops() {
        let tmp = tempfile::tempdir().unwrap();
        let config = config_with_sessions_dir(tmp.path());
        ACTIVE
            .lock_or_recover()
            .insert("ephemeral-live".to_string());
        assert!(is_active("ephemeral-live"));
        {
            let _guard = SessionCleanup::new(config, "ephemeral-live".to_string());
        }
        assert!(!is_active("ephemeral-live"));
    }

    #[test]
    fn cleanup_guard_deletes_session_files_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let id = "ephemeral-abc";
        touch_session_files(tmp.path(), id);
        let config = config_with_sessions_dir(tmp.path());

        assert!(session_files_exist(tmp.path(), id));
        {
            let _guard = SessionCleanup::new(config.clone(), id.to_string());
        } // guard drops here
        assert!(
            !session_files_exist(tmp.path(), id),
            "all session files removed when the guard drops"
        );
    }

    #[test]
    fn cleanup_guard_leaves_other_sessions_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        touch_session_files(tmp.path(), "ephemeral-mine");
        touch_session_files(tmp.path(), "real-user-session");
        let config = config_with_sessions_dir(tmp.path());

        {
            let _guard = SessionCleanup::new(config.clone(), "ephemeral-mine".to_string());
        }
        assert!(
            !session_files_exist(tmp.path(), "ephemeral-mine"),
            "the ephemeral session is deleted"
        );
        assert!(
            session_files_exist(tmp.path(), "real-user-session"),
            "an unrelated session is left alone"
        );
    }

    #[test]
    fn cleanup_runs_even_when_closure_errors() {
        // `run` must tear the session down on the error path too, not just
        // on success — otherwise a failed generation would leak a session
        // file. We can't stand up a live ACP agent here, so we exercise
        // the guard placement directly: a guard armed before fallible work
        // still fires when that work returns early.
        let tmp = tempfile::tempdir().unwrap();
        let id = "ephemeral-erroring";
        touch_session_files(tmp.path(), id);
        let config = config_with_sessions_dir(tmp.path());

        let result: Result<()> = (|| {
            let _guard = SessionCleanup::new(config.clone(), id.to_string());
            anyhow::bail!("simulated send failure");
        })();

        assert!(result.is_err());
        assert!(
            !session_files_exist(tmp.path(), id),
            "session files cleaned up despite the error"
        );
    }

    // --- run / run_with against a fake agent ---

    /// What the fake agent received: `(method, params)` for every request
    /// and notification, in order.
    type Seen = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

    /// A fake ACP agent on loopback (ephemeral port, test-only). Answers
    /// `session/new` with a fresh id (`<tag>-<n>`) and every other request
    /// with `end_turn`, and records everything it reads. Returns the port,
    /// what it saw, and whether a client ever connected.
    fn fake_agent(tag: &'static str) -> (u16, Seen, Arc<std::sync::atomic::AtomicBool>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let dialled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (seen_in, dialled_in) = (seen.clone(), dialled.clone());
        std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            dialled_in.store(true, std::sync::atomic::Ordering::SeqCst);
            let mut writer = stream.try_clone().unwrap();
            let mut sessions = 0;
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { return };
                let msg: serde_json::Value = serde_json::from_str(&line).unwrap();
                let method = msg["method"].as_str().unwrap_or("").to_string();
                seen_in
                    .lock_or_recover()
                    .push((method.clone(), msg["params"].clone()));
                if msg.get("id").is_none() || method.is_empty() {
                    continue;
                }
                let result = match method.as_str() {
                    "session/new" => {
                        sessions += 1;
                        serde_json::json!({ "sessionId": format!("{tag}-{sessions}") })
                    }
                    "session/prompt" => serde_json::json!({ "stopReason": "end_turn" }),
                    _ => serde_json::json!({}),
                };
                let reply =
                    serde_json::json!({ "jsonrpc": "2.0", "id": msg["id"], "result": result });
                if writeln!(writer, "{reply}").is_err() {
                    return;
                }
            }
        });
        (port, seen, dialled)
    }

    fn remote_client(port: u16) -> Arc<AcpClient> {
        Arc::new(AcpClient::new(
            crate::acp_client::AcpConnectionMode::Remote {
                host: "127.0.0.1".to_string(),
                port,
            },
        ))
    }

    fn new_session_cwd(seen: &Seen) -> Option<String> {
        seen.lock_or_recover()
            .iter()
            .find(|(m, _)| m == "session/new")
            .and_then(|(_, p)| p["cwd"].as_str().map(str::to_string))
    }

    #[test]
    fn the_default_options_are_what_run_has_always_done() {
        assert_eq!(
            EphemeralOptions::default(),
            EphemeralOptions {
                strict: false,
                require_connected: false,
                cwd: None,
            }
        );
    }

    #[test]
    fn run_connects_uses_the_configured_folder_and_hides_the_id() {
        let (port, seen, _) = fake_agent("eph-default");
        let client = remote_client(port);
        let tmp = tempfile::tempdir().unwrap();
        let config = config_with_sessions_dir(tmp.path());
        let workdir = tmp.path().join("project");
        config.lock_or_recover().acp.agent.working_directory =
            Some(workdir.to_string_lossy().to_string());
        assert!(!client.is_connected());

        let id = run(&client, &config, |sid| {
            assert!(is_active(sid));
            assert!(!is_strict(sid), "the default session is not strict");
            assert!(is_hidden(sid), "recorded before any prompt");
            touch_session_files(tmp.path(), sid);
            Ok(sid.to_string())
        })
        .unwrap();

        assert!(client.is_connected(), "run connects when needed");
        assert_eq!(
            new_session_cwd(&seen).as_deref(),
            Some(workdir.to_string_lossy().as_ref())
        );
        assert!(!is_active(&id));
        assert!(!is_strict(&id));
        assert!(is_hidden(&id), "still hidden after the files are gone");
        assert!(hidden_ids().contains(&id));
        assert!(!session_files_exist(tmp.path(), &id));
    }

    #[test]
    fn require_connected_never_dials_the_agent() {
        let (port, _seen, dialled) = fake_agent("eph-offline");
        let client = remote_client(port);
        let tmp = tempfile::tempdir().unwrap();
        let config = config_with_sessions_dir(tmp.path());
        let options = EphemeralOptions {
            require_connected: true,
            ..Default::default()
        };

        let mut ran = false;
        let err = run_with(&client, &config, options, |_| {
            ran = true;
            Ok(())
        })
        .unwrap_err();

        assert!(!ran);
        let app_error = err.downcast_ref::<AppError>().expect("an AppError");
        assert_eq!(app_error.key, "errors.agent_call.agent_unavailable");
        assert!(matches!(app_error.kind, ErrorKind::ConnectionLost));
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!dialled.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!client.is_connected());
    }

    #[test]
    fn a_background_session_is_strict_and_runs_in_the_emptied_scratch_folder() {
        let (port, seen, _) = fake_agent("eph-strict");
        let client = remote_client(port);
        client.connect().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let config = config_with_sessions_dir(tmp.path());
        let options = EphemeralOptions::background().unwrap();
        let scratch = options.cwd.clone().unwrap();
        assert_eq!(Some(&scratch), scratch_dir().as_ref());
        assert!(options.strict && options.require_connected);

        // Left behind by an earlier crash: gone before the agent starts.
        std::fs::write(scratch.join("left-over"), b"x").unwrap();

        let id = run_with(&client, &config, options, |sid| {
            assert!(is_strict(sid));
            assert!(is_active(sid));
            assert!(!scratch.join("left-over").exists());
            // A tool wrote here.
            std::fs::write(scratch.join("written-by-a-tool"), b"x").unwrap();
            Ok(sid.to_string())
        })
        .unwrap();

        assert_eq!(
            new_session_cwd(&seen).as_deref(),
            Some(scratch.to_string_lossy().as_ref())
        );
        assert!(scratch.is_dir());
        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
        assert!(!is_active(&id));
        assert!(is_strict(&id), "a late request is still refused");
        assert!(is_hidden(&id));
    }

    #[test]
    fn a_strict_prompt_goes_out_exactly_as_framed() {
        let (port, seen, _) = fake_agent("eph-framed");
        let client = remote_client(port);
        client.connect().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let config = config_with_sessions_dir(tmp.path());
        let options = EphemeralOptions {
            strict: true,
            require_connected: true,
            cwd: None,
        };

        run_with(&client, &config, options, |sid| {
            client.send_chat_streaming_background(sid, "framed prompt", true)?;
            Ok(())
        })
        .unwrap();

        let prompts: Vec<serde_json::Value> = seen
            .lock_or_recover()
            .iter()
            .filter(|(m, _)| m == "session/prompt")
            .map(|(_, p)| p["prompt"].clone())
            .collect();
        assert_eq!(
            prompts,
            vec![serde_json::json!([{ "type": "text", "text": "framed prompt" }])],
            "no time stamp or any other block"
        );
    }

    #[test]
    fn a_strict_session_never_empties_a_folder_the_caller_chose() {
        let (port, seen, _) = fake_agent("eph-own-folder");
        let client = remote_client(port);
        client.connect().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let config = config_with_sessions_dir(tmp.path());
        let folder = tmp.path().join("users-folder");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("keep.txt"), b"keep").unwrap();
        let options = EphemeralOptions {
            strict: true,
            require_connected: true,
            cwd: Some(folder.clone()),
        };

        run_with(&client, &config, options, |_| Ok(())).unwrap();

        assert_eq!(
            new_session_cwd(&seen).as_deref(),
            Some(folder.to_string_lossy().as_ref())
        );
        assert!(folder.join("keep.txt").exists());
    }

    #[test]
    fn a_tool_attempt_on_a_strict_session_cancels_its_turn() {
        let _serial = strict::tests::HOOK_TEST_LOCK.lock_or_recover();
        set_strict_hooks(Some(strict::tests::recording_hooks()));
        let (port, seen, _) = fake_agent("eph-cancel");
        let client = remote_client(port);
        client.connect().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let config = config_with_sessions_dir(tmp.path());
        let options = EphemeralOptions {
            strict: true,
            require_connected: true,
            cwd: None,
        };

        let id = run_with(&client, &config, options, |sid| {
            assert!(note_tool_attempt(&client, sid));
            assert!(!note_tool_attempt(&client, sid));
            assert!(tool_attempted(sid));
            Ok(sid.to_string())
        })
        .unwrap();
        set_strict_hooks(None);

        assert_eq!(strict::tests::tool_attempts_for(&id), 1);
        assert!(tool_attempted(&id));
        // The cancel is a notification; give the fake agent a moment to read it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let cancels = loop {
            let cancels: Vec<serde_json::Value> = seen
                .lock_or_recover()
                .iter()
                .filter(|(m, _)| m == "session/cancel")
                .map(|(_, p)| p.clone())
                .collect();
            if !cancels.is_empty() || std::time::Instant::now() > deadline {
                break cancels;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(cancels, vec![serde_json::json!({ "sessionId": id })]);
    }
}
