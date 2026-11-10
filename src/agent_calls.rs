//! Background agent calls for extensions: the engine
//! (docs/design/EXTENSION_AGENT_CALLS.md, WP-AC1).
//!
//! An extension that holds `agent_background` asks Kage to run one prompt
//! on the user's agent, out of sight. [`AgentCallService`] owns the whole
//! life of such a call:
//!
//! 1. **Reserve** (`agent_call_reserve`): check the four gates (§2), the
//!    purpose, the queue, the dedupe key, the pauses and the allowance.
//! 2. **Ready** (`agent_call_ready`): wait for a turn. The scheduler gives
//!    one turn at a time, round-robin between extensions, while the user
//!    isn't using Kage (unless `when: now`), the agent is connected, and
//!    the cooldowns, hourly cap, allowance and breakers allow it.
//! 3. **Send** (`agent_call_send`), within 10 s of the turn: re-check the
//!    gates, validate, refuse the extension's own credentials, redact
//!    secret-shaped text, frame the prompt, check its size and the
//!    allowance, then run it on a strict ephemeral session with a watchdog
//!    for the timeout and the reply cap.
//! 4. **Finish**: validate the reply, charge the allowance, update the
//!    breakers, keep the prompt and reply in memory (last 5), write one
//!    audit entry, log one line, and tell first-party windows.
//!
//! This module has no Tauri `AppHandle` dependency. What needs one (audit
//! entries, telemetry, the activity event, the panel's log rows) goes out
//! through [`AgentCallHooks`], which the wiring supplies.
//!
//! **Logging.** One line per call: id, extension, purpose, outcome, sizes,
//! duration. Never prompt or reply text, labels or agent error text, in
//! any mode. Every log line in `src/agent_calls/*` goes through
//! [`info_line`] / [`warn_line`]; a test checks there is no other.

pub mod consent;
pub mod exec;
pub mod limits;
pub mod prompt;
pub mod recent;
pub mod redact;
pub mod request;
pub mod scheduler;
pub mod schema;
pub mod usage;

#[cfg(test)]
mod tests;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};

use crate::config::{AgentAllowance, AgentConsent, Config, ExtensionAgentAccess};
use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;

pub use consent::{Access, Location, LocationKind};
pub use exec::{AcpBackend, AgentBackend, RunControl, RunError, SessionOpener, SessionSpec, Stop};
pub use limits::{Allowance, Clock, SystemClock};
pub use recent::RequestDetail;
pub use request::{AgentBackgroundDecl, AgentCallRequest, CallIdArgs, ReserveArgs, SendArgs, When};
pub use usage::PauseReason;

use request::error as call_error;

// --- Logging -------------------------------------------------------------------

#[cfg(test)]
pub(crate) static LOG_CAPTURE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The one way this module logs at info level. English, never content.
pub(crate) fn info_line(msg: String) {
    #[cfg(test)]
    LOG_CAPTURE.lock_or_recover().push(msg.clone());
    log::info!("{msg}");
}

/// The one way this module logs a warning. English, never content.
pub(crate) fn warn_line(msg: String) {
    #[cfg(test)]
    LOG_CAPTURE.lock_or_recover().push(msg.clone());
    log::warn!("{msg}");
}

// --- Public types --------------------------------------------------------------

/// How a call that was sent ended (the audit entry's `outcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCallOutcome {
    Ok,
    InvalidResponse,
    NotJson,
    ResponseTooLarge,
    Timeout,
    Cancelled,
    ToolAttempted,
    AgentError,
    ConnectionLost,
}

/// Who or what cancelled a call (the audit entry's `cancel_reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelReason {
    User,
    Activity,
    Shutdown,
    AccessOff,
    Extension,
}

/// `agent_call_reserve`'s result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reservation {
    pub call_id: String,
    /// 1-based place in this extension's own queue.
    pub position: u32,
}

/// `agent_call_ready`'s result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TurnInfo {
    pub call_id: String,
    /// RFC 3339: send before this, or the turn is given up.
    pub send_by: String,
}

/// `agent_call_send`'s result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentCallResult {
    pub call_id: String,
    /// The raw reply, at most `max_response_chars`.
    pub text: String,
    /// Set when a schema was given and the reply matched it.
    pub json: Option<serde_json::Value>,
    pub prompt_chars: u32,
    pub response_chars: u32,
    pub duration_ms: u64,
    /// Secret-shaped strings replaced before sending.
    pub redactions: u32,
}

/// Today's usage on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub calls_today: u32,
    pub calls_limit: u32,
    pub chars_today: u64,
    pub chars_limit: u64,
    /// RFC 3339 with the local offset: the next local midnight.
    pub resets_at: String,
}

/// `agent_call_status`'s result. Never the agent's name, the user's idle
/// state, or another extension's numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentCallStatus {
    /// Gates 1 and 2.
    pub available: bool,
    pub access: Access,
    pub connected: bool,
    /// This extension's reserved calls not yet running.
    pub waiting: u32,
    pub running: bool,
    pub usage: Usage,
    /// When rate limits or a pause end, if one applies.
    pub next_start_at: Option<String>,
    /// While a breaker or **Stop now** pause is in effect.
    pub paused_until: Option<String>,
    /// Stable per machine and extension; see [`usage::machine_tag`].
    pub machine_tag: String,
}

/// One call's audit entry (`AuditEvent::ExtensionAgentCall`). Counts and
/// ids only, never text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallRecord {
    pub extension_id: String,
    pub call_id: String,
    pub purpose: String,
    /// Dropped when it looks like a secret.
    pub label: Option<String>,
    /// The connection's display name at the time.
    pub agent: String,
    pub outcome: AgentCallOutcome,
    pub cancel_reason: Option<CancelReason>,
    pub prompt_chars: u32,
    pub response_chars: u32,
    pub duration_ms: u64,
    pub redactions: u32,
}

/// The host switch was flipped (`AuditEvent::ExtensionAgentAccessChanged`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccessRecord {
    pub extension_id: String,
    pub enabled: bool,
    pub agent: Option<String>,
}

/// A running call, for the activity event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunningCall {
    pub extension_id: String,
    pub name: String,
    pub purpose: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub started_at: String,
}

/// The `agent_calls_activity` payload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ActivitySnapshot {
    pub running: Vec<RunningCall>,
    /// Reservations not running yet, across all extensions.
    pub waiting: u32,
}

/// The active agent, as the panel and consent dialog show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentInfo {
    pub label: String,
    pub location: Location,
}

/// A pause in effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PauseInfo {
    pub until: String,
    pub reason: PauseReason,
}

/// The three allowance levels for one extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Allowances {
    pub low: Allowance,
    pub normal: Allowance,
    pub high: Allowance,
}

/// `get_extension_agent_access`'s result: everything the host panel and
/// the consent dialog need. First-party windows only.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccessPanel {
    pub extension_id: String,
    pub name: String,
    /// Gates 1 and 2.
    pub available: bool,
    pub granted: bool,
    pub declared: bool,
    pub extension_enabled: bool,
    /// The host switch.
    pub enabled: bool,
    pub access: Access,
    pub connected: bool,
    /// The active connection, if one is configured.
    pub agent: Option<AgentInfo>,
    /// The active connection's fingerprint, to pass back to
    /// `set_extension_agent_access`. Never sent to extensions.
    pub consent_token: Option<String>,
    /// The agent the recorded consent named.
    pub consent_agent: Option<String>,
    pub allowance: AgentAllowance,
    pub allowances: Allowances,
    pub usage: Usage,
    pub pause: Option<PauseInfo>,
    pub next_start_at: Option<String>,
    pub running: bool,
    pub waiting: u32,
    /// The manifest's `reason` (text or a `__MSG_key__` token).
    pub reason: Option<String>,
    pub purposes: Vec<String>,
    /// Data capabilities the extension also holds, for the dialog.
    pub reads: Vec<String>,
    /// Ids of this session's calls whose prompt and reply can be shown.
    pub viewable: Vec<String>,
    /// The last audit rows for this extension, newest first (from
    /// [`AgentCallHooks::log_rows`]).
    pub log: Vec<serde_json::Value>,
}

/// What the engine needs from an extension's manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFacts {
    /// The display name (`__MSG_` resolved).
    pub name: String,
    /// The `agentBackground` block, if valid.
    pub decl: Option<AgentBackgroundDecl>,
    /// Settings the manifest marks `sensitive`.
    pub sensitive_fields: Vec<String>,
}

/// Looks up installed extensions' manifests.
pub trait ExtensionDirectory: Send + Sync {
    /// `None` when no extension with this id is installed.
    fn manifest(&self, extension_id: &str) -> Option<ManifestFacts>;
}

/// The extensions installed on this machine.
pub struct InstalledExtensions;

impl ExtensionDirectory for InstalledExtensions {
    fn manifest(&self, extension_id: &str) -> Option<ManifestFacts> {
        let item = crate::extensions::discover_items("extension", &HashMap::new())
            .into_iter()
            .find(|i| i.manifest.id == extension_id)?;
        let decl = item.manifest.agent_background_decl();
        Some(ManifestFacts {
            name: crate::config_export::sensitive::display_name(&item),
            decl,
            sensitive_fields: item
                .manifest
                .config_field_decls()
                .into_iter()
                .filter(|d| d.sensitive)
                .map(|d| d.field)
                .collect(),
        })
    }
}

/// Where the engine's results go. Every method has a no-op default.
pub trait AgentCallHooks: Send + Sync {
    /// One entry per call that was sent, when it ends.
    fn audit_call(&self, _record: &CallRecord) {}
    /// The host switch was turned on or off.
    fn audit_access(&self, _record: &AccessRecord) {}
    /// A telemetry event (counts only). Goes through `telemetry::track`.
    fn telemetry(&self, _event: &str, _props: serde_json::Value) {}
    /// A call started or ended, or the queue changed
    /// (`agent_calls_activity`).
    fn activity(&self, _snapshot: &ActivitySnapshot) {}
    /// The last `limit` audit rows for this extension, newest first.
    fn log_rows(&self, _extension_id: &str, _limit: usize) -> Vec<serde_json::Value> {
        Vec::new()
    }
}

/// Hooks that do nothing (before the wiring supplies real ones).
pub struct NoHooks;
impl AgentCallHooks for NoHooks {}

/// Capabilities that let an extension read user data, listed in the
/// consent dialog when the extension holds any.
pub const DATA_CAPABILITIES: &[&str] = &[
    "clipboard",
    "calendar",
    "session",
    "activity",
    "windows",
    "filesystem",
];

/// The capability this engine is gated on.
pub const CAPABILITY: &str = "agent_background";

/// Rows the panel shows.
pub const PANEL_LOG_ROWS: usize = 20;

// --- The process-wide service, for hooks without state access --------------------

static GLOBAL: Mutex<Option<Weak<AgentCallService>>> = Mutex::new(None);

fn global() -> Option<Arc<AgentCallService>> {
    GLOBAL.lock_or_recover().as_ref().and_then(Weak::upgrade)
}

/// The agent asked for a tool, or started one, on `session_id`. Stops the
/// background call on that session, if there is one. A no-op before the
/// service starts.
pub fn note_tool_attempt(session_id: &str) {
    if let Some(svc) = global() {
        svc.note_tool_attempt(session_id);
    }
}

/// The agent disconnected. A no-op before the service starts.
pub fn on_disconnect() {
    if let Some(svc) = global() {
        svc.on_disconnect();
    }
}

// --- Internal state ------------------------------------------------------------

/// What a call's watchers see.
#[derive(Debug, Clone)]
enum Signal {
    Waiting,
    Turn {
        send_by: String,
    },
    /// Finished: the error, or `None` when it ended well.
    Done(Option<AppError>),
}

enum Phase {
    Waiting,
    Turn { send_by: DateTime<Utc> },
    Running(Arc<Running>),
}

struct Call {
    extension_id: String,
    purpose: String,
    when: When,
    dedupe_key: Option<String>,
    phase: Phase,
    signal: tokio::sync::watch::Sender<Signal>,
}

/// A call that was handed to the agent.
struct Running {
    call_id: String,
    extension_id: String,
    name: String,
    purpose: String,
    label: Option<String>,
    /// The agent the prompt went to (its display name then).
    agent: String,
    started: DateTime<Utc>,
    prompt: String,
    prompt_chars: u32,
    redactions: u32,
    schema: Option<serde_json::Value>,
    max_response_chars: u32,
    timeout_ms: u32,
    ctl: RunControl,
    result_tx: Mutex<Option<tokio::sync::oneshot::Sender<Result<AgentCallResult, AppError>>>>,
    /// The runner returned.
    done: AtomicBool,
    /// Usage, audit and the memory ring were updated.
    accounted: AtomicBool,
}

impl Running {
    fn resolve(&self, result: Result<AgentCallResult, AppError>) {
        if let Some(tx) = self.result_tx.lock_or_recover().take() {
            let _ = tx.send(result);
        }
    }
}

struct Finished {
    call_id: String,
    extension_id: String,
    error: Option<AppError>,
}

#[derive(Default)]
struct State {
    calls: HashMap<String, Call>,
    sched: scheduler::Scheduler,
    usage: Option<usage::UsageBook>,
    usage_dirty: bool,
    recent: recent::Recent,
    finished: VecDeque<Finished>,
    last_activity: Option<DateTime<Utc>>,
    shutting_down: bool,
    machine_id: Option<String>,
    reported_on: Option<chrono::NaiveDate>,
}

/// Gates 1–4 and what goes with them, read from the config and manifest.
struct Gates {
    name: String,
    installed: bool,
    granted: bool,
    decl: Option<AgentBackgroundDecl>,
    extension_enabled: bool,
    entry: Option<ExtensionAgentAccess>,
    access: Access,
    fingerprint: Option<String>,
    agent: Option<AgentInfo>,
    secrets: Vec<String>,
    reads: Vec<String>,
}

impl Gates {
    fn available(&self) -> bool {
        self.installed && self.granted && self.decl.is_some()
    }

    fn check(&self) -> Result<(), AppError> {
        let ext = [("extension", self.name.as_str())];
        if !self.available() {
            return Err(call_error(ErrorKind::NotPermitted, "not_available", &ext));
        }
        if !self.extension_enabled {
            return Err(call_error(
                ErrorKind::NotPermitted,
                "extension_disabled",
                &ext,
            ));
        }
        match self.access {
            Access::Off => Err(call_error(ErrorKind::NotPermitted, "access_off", &ext)),
            Access::NeedsConsent => Err(call_error(
                ErrorKind::NotPermitted,
                "consent_required",
                &ext,
            )),
            Access::ConsentStale => Err(call_error(ErrorKind::NotPermitted, "consent_stale", &ext)),
            Access::On => Ok(()),
        }
    }

    fn level(&self) -> AgentAllowance {
        self.entry.as_ref().map(|e| e.allowance).unwrap_or_default()
    }

    fn allowance(&self) -> Allowance {
        limits::allowance(self.level(), self.decl.as_ref().and_then(|d| d.daily_calls))
    }

    fn prompt_cap(&self) -> usize {
        limits::prompt_cap(self.decl.as_ref().and_then(|d| d.max_prompt_chars))
    }

    /// The error that ends a waiting or running call when a gate closes.
    fn closing_error(&self, gate: &AppError) -> (AppError, Stop) {
        if gate.key == "errors.agent_call.consent_stale" {
            (gate.clone(), Stop::ConsentStale)
        } else {
            (
                call_error(
                    ErrorKind::Cancelled,
                    "cancelled_access_off",
                    &[("extension", &self.name)],
                ),
                Stop::AccessOff,
            )
        }
    }
}

/// The parts a service is built from. [`AgentCallService::new`] fills in
/// the real ones; tests pass fakes.
pub struct ServiceParts {
    pub config: Arc<Mutex<Config>>,
    pub backend: Arc<dyn AgentBackend>,
    pub directory: Arc<dyn ExtensionDirectory>,
    pub clock: Arc<dyn Clock>,
    /// `None`: usage lives in memory only.
    pub usage_path: Option<PathBuf>,
    /// `None`: a new random machine id each run.
    pub machine_id_path: Option<PathBuf>,
    /// Writes the config after a host switch change: `Config::save`.
    pub save_config: fn(&Config) -> anyhow::Result<()>,
}

/// The background agent call engine. One per process, in
/// `FeatureServices.agent_calls`.
pub struct AgentCallService {
    config: Arc<Mutex<Config>>,
    backend: Arc<dyn AgentBackend>,
    directory: Arc<dyn ExtensionDirectory>,
    clock: Arc<dyn Clock>,
    usage_path: Option<PathBuf>,
    machine_id_path: Option<PathBuf>,
    save_config: fn(&Config) -> anyhow::Result<()>,
    hooks: Mutex<Arc<dyn AgentCallHooks>>,
    state: Mutex<State>,
    /// Wakes the scheduler task.
    wake: tokio::sync::Notify,
    /// Runners still in their session (for shutdown).
    runners: (Mutex<usize>, Condvar),
    /// Serialises usage-file writes, oldest snapshot first.
    persist_lock: Mutex<()>,
    /// Bumped for each scheduler task started; an older task stops.
    scheduler_generation: AtomicU64,
}

fn ms_between(a: DateTime<Utc>, b: DateTime<Utc>) -> u64 {
    u64::try_from((b - a).num_milliseconds()).unwrap_or(0)
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl AgentCallService {
    /// The real service: the installed extensions, the system clock, and
    /// the usage and machine-id files under `<data_local_dir>/kage/`. No
    /// I/O here; files are read on first use.
    pub fn new(config: Arc<Mutex<Config>>, backend: Arc<dyn AgentBackend>) -> Self {
        Self::with_parts(ServiceParts {
            config,
            backend,
            directory: Arc::new(InstalledExtensions),
            clock: Arc::new(SystemClock),
            usage_path: usage::default_path(),
            machine_id_path: usage::default_machine_id_path(),
            save_config: Config::save,
        })
    }

    pub fn with_parts(parts: ServiceParts) -> Self {
        Self {
            config: parts.config,
            backend: parts.backend,
            directory: parts.directory,
            clock: parts.clock,
            usage_path: parts.usage_path,
            machine_id_path: parts.machine_id_path,
            save_config: parts.save_config,
            hooks: Mutex::new(Arc::new(NoHooks)),
            state: Mutex::new(State {
                recent: recent::Recent::new(limits::RECENT_CALLS),
                ..Default::default()
            }),
            wake: tokio::sync::Notify::new(),
            runners: (Mutex::new(0), Condvar::new()),
            persist_lock: Mutex::new(()),
            scheduler_generation: AtomicU64::new(0),
        }
    }

    /// Set the hooks, make the service reachable from
    /// [`note_tool_attempt`] and [`on_disconnect`], and start the
    /// scheduler task. Call once, from `setup()`.
    pub fn start(self: &Arc<Self>, hooks: Arc<dyn AgentCallHooks>) {
        self.set_hooks(hooks);
        *GLOBAL.lock_or_recover() = Some(Arc::downgrade(self));
        self.spawn_scheduler();
    }

    /// Kage didn't quit after [`shutdown`](Self::shutdown) after all (an
    /// update that failed to install): take calls again and restart the
    /// scheduler task. A no-op unless the service was shut down.
    pub fn resume(self: &Arc<Self>) {
        {
            let mut st = self.state.lock_or_recover();
            if !st.shutting_down {
                return;
            }
            st.shutting_down = false;
        }
        info_line("Background AI resumed: Kage is not quitting after all".to_string());
        self.spawn_scheduler();
    }

    /// Start the scheduler task. A task started earlier stops at its next
    /// wake, so only the newest one runs.
    fn spawn_scheduler(self: &Arc<Self>) {
        let generation = self.scheduler_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let weak = Arc::downgrade(self);
        tauri::async_runtime::spawn(async move {
            loop {
                let Some(svc) = weak.upgrade() else {
                    break;
                };
                if svc.scheduler_generation.load(Ordering::SeqCst) != generation {
                    break;
                }
                if svc.state.lock_or_recover().shutting_down {
                    break;
                }
                let pump = svc.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || pump.pump()).await;
                let wait = svc.next_wake();
                tokio::select! {
                    _ = svc.wake.notified() => {}
                    _ = tokio::time::sleep(wait) => {}
                }
            }
        });
    }

    pub fn set_hooks(&self, hooks: Arc<dyn AgentCallHooks>) {
        *self.hooks.lock_or_recover() = hooks;
    }

    fn hooks(&self) -> Arc<dyn AgentCallHooks> {
        self.hooks.lock_or_recover().clone()
    }

    // --- Gates ------------------------------------------------------------------

    fn gates(&self, extension_id: &str) -> Gates {
        let (granted_caps, enabled, entry, active, settings) = {
            let cfg = self.config.lock_or_recover();
            (
                cfg.extension_grants
                    .get(extension_id)
                    .map(|g| g.granted.clone())
                    .unwrap_or_default(),
                cfg.extension_states
                    .get(extension_id)
                    .copied()
                    .unwrap_or(true),
                cfg.extension_agent_access.get(extension_id).cloned(),
                cfg.acp.active_connection().cloned(),
                cfg.extensions.get(extension_id).cloned(),
            )
        };
        let manifest = self.directory.manifest(extension_id);
        let fingerprint = active.as_ref().map(consent::fingerprint);
        let access = consent::access_state(entry.as_ref(), fingerprint.as_deref());
        let secrets = match (&settings, &manifest) {
            (Some(s), m) => redact::collect_secrets(
                s,
                m.as_ref()
                    .map_or(&[][..], |m| m.sensitive_fields.as_slice()),
            ),
            (None, _) => Vec::new(),
        };
        Gates {
            name: manifest
                .as_ref()
                .map(|m| m.name.clone())
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| extension_id.to_string()),
            installed: manifest.is_some(),
            granted: granted_caps.iter().any(|c| c == CAPABILITY),
            decl: manifest.and_then(|m| m.decl),
            extension_enabled: enabled,
            entry,
            access,
            fingerprint,
            agent: active.as_ref().map(|c| AgentInfo {
                label: consent::agent_label(c),
                location: consent::location(c),
            }),
            secrets,
            reads: DATA_CAPABILITIES
                .iter()
                .filter(|c| granted_caps.iter().any(|g| g == *c))
                .map(|c| c.to_string())
                .collect(),
        }
    }

    // --- Usage --------------------------------------------------------------------

    fn ensure_usage(&self, st: &mut State) {
        if st.usage.is_none() {
            st.usage = Some(
                self.usage_path
                    .as_deref()
                    .map(usage::UsageBook::load)
                    .unwrap_or_default(),
            );
        }
    }

    /// Write the usage file if it changed. Never call with the state lock
    /// held.
    fn persist(&self) {
        let _p = self.persist_lock.lock_or_recover();
        let book = {
            let mut st = self.state.lock_or_recover();
            if !st.usage_dirty {
                return;
            }
            st.usage_dirty = false;
            st.usage.clone()
        };
        let (Some(book), Some(path)) = (book, self.usage_path.as_deref()) else {
            return;
        };
        if let Err(e) = book.save(path) {
            warn_line(format!("Could not save background AI usage: {e}"));
        }
    }

    fn time_text(&self, now: DateTime<Utc>, t: DateTime<Utc>) -> String {
        limits::format_local_time(self.clock.as_ref(), now, t)
    }

    /// Refuse a call the pauses, allowance or caps don't allow now.
    /// `extra_chars` is what this call would add (its prompt).
    fn check_budget(
        &self,
        st: &State,
        extension_id: &str,
        g: &Gates,
        now: DateTime<Utc>,
        extra_chars: u64,
    ) -> Result<(), AppError> {
        let clock = self.clock.as_ref();
        let book = st.usage.as_ref().expect("usage loaded");
        let ext = g.name.as_str();
        if let Some((until, _)) = book.pause(extension_id, now) {
            return Err(call_error(
                ErrorKind::RateLimited,
                "paused",
                &[("extension", ext), ("time", &self.time_text(now, until))],
            ));
        }
        let today = limits::local_date(clock, now);
        let reset = limits::next_local_midnight(clock, now);
        let day = book.day(extension_id, today);
        let allow = g.allowance();
        if day.calls >= allow.calls
            || day.chars >= allow.chars
            || day.chars + extra_chars > allow.chars
        {
            return Err(call_error(
                ErrorKind::RateLimited,
                "budget_exhausted",
                &[("extension", ext), ("time", &self.time_text(now, reset))],
            ));
        }
        if book.global_calls(today) >= limits::GLOBAL_DAILY_CALLS {
            return Err(call_error(
                ErrorKind::RateLimited,
                "rate_limited",
                &[("time", &self.time_text(now, reset))],
            ));
        }
        if let Some(free) = st.sched.hourly_free_at(extension_id, now) {
            return Err(call_error(
                ErrorKind::RateLimited,
                "rate_limited",
                &[("time", &self.time_text(now, free))],
            ));
        }
        Ok(())
    }

    fn user_active(&self, st: &State, now: DateTime<Utc>) -> bool {
        st.last_activity
            .is_some_and(|t| now - t < Duration::seconds(limits::IDLE_AFTER_SECS))
    }

    // --- Finishing ------------------------------------------------------------------

    /// End a call that isn't running (waiting or holding its turn).
    fn finish_locked(&self, st: &mut State, call_id: &str, error: Option<AppError>) -> bool {
        let Some(call) = st.calls.get(call_id) else {
            return false;
        };
        if matches!(call.phase, Phase::Running(_)) {
            return false;
        }
        let call = st.calls.remove(call_id).expect("present");
        st.sched.remove_waiting(call_id);
        st.sched.release(call_id, None);
        call.signal.send_replace(Signal::Done(error.clone()));
        Self::remember_finished(st, call_id, &call.extension_id, error);
        true
    }

    fn remember_finished(
        st: &mut State,
        call_id: &str,
        extension_id: &str,
        error: Option<AppError>,
    ) {
        while st.finished.len() >= limits::FINISHED_CALLS {
            st.finished.pop_front();
        }
        st.finished.push_back(Finished {
            call_id: call_id.to_string(),
            extension_id: extension_id.to_string(),
            error,
        });
    }

    /// The error for an id that isn't live for this extension: how it
    /// ended, if it ended badly and recently, else `unknown_call`.
    fn gone_error(st: &State, extension_id: &str, call_id: &str) -> AppError {
        st.finished
            .iter()
            .rev()
            .find(|f| f.call_id == call_id && f.extension_id == extension_id)
            .and_then(|f| f.error.clone())
            .unwrap_or_else(|| call_error(ErrorKind::Internal, "unknown_call", &[]))
    }

    fn snapshot_locked(&self, st: &State) -> ActivitySnapshot {
        let clock = self.clock.as_ref();
        let mut running = Vec::new();
        let mut waiting = 0u32;
        for call in st.calls.values() {
            match &call.phase {
                Phase::Running(r) => running.push(RunningCall {
                    extension_id: r.extension_id.clone(),
                    name: r.name.clone(),
                    purpose: r.purpose.clone(),
                    label: r.label.clone(),
                    started_at: limits::rfc3339_local(clock, r.started),
                }),
                _ => waiting += 1,
            }
        }
        ActivitySnapshot { running, waiting }
    }

    /// The `agent_calls_activity` payload now.
    pub fn activity(&self) -> ActivitySnapshot {
        let st = self.state.lock_or_recover();
        self.snapshot_locked(&st)
    }

    fn emit_activity(&self) {
        let snapshot = self.activity();
        self.hooks().activity(&snapshot);
    }

    /// Stop a running call: record why (the first stop wins), send
    /// `session/cancel`, and answer the extension now. The runner finishes
    /// the bookkeeping when the session ends.
    fn stop_running(&self, run: &Running, stop: Stop) {
        let (first, session) = run.ctl.request_stop(stop, self.clock.now());
        if let Some(sid) = session {
            self.backend.cancel(&sid);
        }
        if first {
            run.resolve(Err(stop_error(stop, &run.name)));
        }
    }

    // --- Extension commands -------------------------------------------------------------

    /// `agent_call_reserve`.
    pub fn reserve(&self, extension_id: &str, args: ReserveArgs) -> Result<Reservation, AppError> {
        let g = self.gates(extension_id);
        g.check()?;
        let decl = g.decl.as_ref().expect("checked");
        if !decl.declares(&args.purpose) {
            return Err(call_error(
                ErrorKind::InvalidInput,
                "purpose_not_declared",
                &[("extension", &g.name), ("purpose", &args.purpose)],
            ));
        }
        if let Some(k) = &args.dedupe_key {
            request::validate_dedupe_key(k)?;
        }
        let now = self.clock.now();
        let connected = self.backend.connected();
        let reservation = {
            let mut st = self.state.lock_or_recover();
            if st.shutting_down {
                return Err(call_error(ErrorKind::Cancelled, "cancelled_shutdown", &[]));
            }
            self.ensure_usage(&mut st);
            self.check_budget(&st, extension_id, &g, now, 0)?;
            if !connected {
                return Err(call_error(
                    ErrorKind::ConnectionLost,
                    "agent_unavailable",
                    &[],
                ));
            }
            if let Some(k) = &args.dedupe_key {
                // A stopped call that is still winding down no longer holds
                // its key: the extension already has its answer and may
                // retry.
                let taken = st.calls.values().any(|c| {
                    c.extension_id == extension_id
                        && c.dedupe_key.as_ref() == Some(k)
                        && !matches!(&c.phase, Phase::Running(r) if r.ctl.stopped().is_some())
                });
                if taken {
                    return Err(call_error(ErrorKind::RateLimited, "duplicate", &[]));
                }
            }
            if !st.sched.has_room(extension_id) {
                return Err(call_error(
                    ErrorKind::RateLimited,
                    "queue_full",
                    &[
                        ("extension", &g.name),
                        ("limit", &limits::MAX_WAITING_PER_EXTENSION.to_string()),
                    ],
                ));
            }
            let call_id = uuid::Uuid::new_v4().to_string();
            let (signal, _) = tokio::sync::watch::channel(Signal::Waiting);
            st.calls.insert(
                call_id.clone(),
                Call {
                    extension_id: extension_id.to_string(),
                    purpose: args.purpose.clone(),
                    when: args.when.unwrap_or_default(),
                    dedupe_key: args.dedupe_key.clone(),
                    phase: Phase::Waiting,
                    signal,
                },
            );
            let position = st.sched.enqueue(extension_id, &call_id);
            Reservation {
                call_id,
                position: to_u32(position),
            }
        };
        self.emit_activity();
        self.wake.notify_one();
        Ok(reservation)
    }

    /// `agent_call_ready`: resolves when the call's turn starts.
    pub async fn ready(&self, extension_id: &str, args: CallIdArgs) -> Result<TurnInfo, AppError> {
        let call_id = args.call_id;
        let mut rx = {
            let st = self.state.lock_or_recover();
            match st.calls.get(&call_id) {
                Some(c) if c.extension_id == extension_id => c.signal.subscribe(),
                _ => return Err(Self::gone_error(&st, extension_id, &call_id)),
            }
        };
        loop {
            let signal = rx.borrow_and_update().clone();
            match signal {
                Signal::Turn { send_by } => return Ok(TurnInfo { call_id, send_by }),
                Signal::Done(Some(e)) => return Err(e),
                Signal::Done(None) => {
                    return Err(call_error(ErrorKind::Internal, "unknown_call", &[]))
                }
                Signal::Waiting => {}
            }
            if rx.changed().await.is_err() {
                let st = self.state.lock_or_recover();
                return Err(Self::gone_error(&st, extension_id, &call_id));
            }
        }
    }

    /// `agent_call_cancel`. Idempotent; unknown ids and other extensions'
    /// ids are fine.
    pub fn cancel(&self, extension_id: &str, args: CallIdArgs) -> Result<(), AppError> {
        let running = {
            let mut st = self.state.lock_or_recover();
            let Some(call) = st.calls.get(&args.call_id) else {
                return Ok(());
            };
            if call.extension_id != extension_id {
                return Ok(());
            }
            match &call.phase {
                Phase::Running(r) => Some(r.clone()),
                _ => {
                    let e = call_error(ErrorKind::Cancelled, "cancelled_by_extension", &[]);
                    self.finish_locked(&mut st, &args.call_id, Some(e));
                    None
                }
            }
        };
        match running {
            Some(r) => self.stop_running(&r, Stop::ByExtension),
            None => {
                self.emit_activity();
                self.wake.notify_one();
            }
        }
        Ok(())
    }

    /// `agent_call_send`: run the request on the agent and return the
    /// reply.
    pub async fn send(
        self: &Arc<Self>,
        extension_id: &str,
        args: SendArgs,
    ) -> Result<AgentCallResult, AppError> {
        let call_id = args.call_id;
        self.expect_turn(extension_id, &call_id)?;

        // Ends the turn with `e` and returns it.
        let fail = |e: AppError| -> Result<AgentCallResult, AppError> {
            {
                let mut st = self.state.lock_or_recover();
                self.finish_locked(&mut st, &call_id, Some(e.clone()));
            }
            self.emit_activity();
            self.wake.notify_one();
            Err(e)
        };

        let g = self.gates(extension_id);
        if let Err(e) = g.check() {
            return fail(e);
        }
        let mut req = match request::validate(args.request) {
            Ok(r) => r,
            Err(e) => return fail(e),
        };
        if redact::contains_credential(&req, &g.secrets) {
            return fail(call_error(
                ErrorKind::InvalidInput,
                "contains_credential",
                &[("extension", &g.name)],
            ));
        }
        let redactions = redact::redact_request(&mut req);
        let prompt = prompt::frame(&g.name, &req.instructions, &req.input, req.schema.as_ref());
        let prompt_chars = prompt.chars().count();
        let cap = g.prompt_cap();
        if prompt_chars > cap {
            return fail(call_error(
                ErrorKind::InvalidInput,
                "prompt_too_large",
                &[
                    ("size", &prompt_chars.to_string()),
                    ("limit", &cap.to_string()),
                ],
            ));
        }

        let now = self.clock.now();
        let prompt_in_flight = self.backend.user_prompt_in_flight();
        let connected = self.backend.connected();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let run = {
            let mut st = self.state.lock_or_recover();
            // The turn may have expired or been cancelled meanwhile.
            let when = match st.calls.get(&call_id) {
                Some(c)
                    if c.extension_id == extension_id && matches!(c.phase, Phase::Turn { .. }) =>
                {
                    c.when
                }
                _ => return Err(Self::gone_error(&st, extension_id, &call_id)),
            };
            self.ensure_usage(&mut st);
            let refusal = if st.shutting_down {
                Some(call_error(ErrorKind::Cancelled, "cancelled_shutdown", &[]))
            } else if let Err(e) =
                self.check_budget(&st, extension_id, &g, now, prompt_chars as u64)
            {
                Some(e)
            } else if when == When::Idle && (prompt_in_flight || self.user_active(&st, now)) {
                Some(call_error(
                    ErrorKind::Cancelled,
                    "cancelled_user_activity",
                    &[],
                ))
            } else if !connected {
                Some(call_error(
                    ErrorKind::ConnectionLost,
                    "agent_unavailable",
                    &[],
                ))
            } else {
                None
            };
            if let Some(e) = refusal {
                drop(st);
                return fail(e);
            }
            let call = st.calls.get_mut(&call_id).expect("checked");
            let run = Arc::new(Running {
                call_id: call_id.clone(),
                extension_id: extension_id.to_string(),
                name: g.name.clone(),
                purpose: call.purpose.clone(),
                label: req.label.clone(),
                agent: g
                    .agent
                    .as_ref()
                    .map(|a| a.label.clone())
                    .unwrap_or_default(),
                started: now,
                prompt,
                prompt_chars: to_u32(prompt_chars),
                redactions,
                schema: req.schema.clone(),
                max_response_chars: req.max_response_chars,
                timeout_ms: req.timeout_ms,
                ctl: RunControl::new(),
                result_tx: Mutex::new(Some(tx)),
                done: AtomicBool::new(false),
                accounted: AtomicBool::new(false),
            });
            call.phase = Phase::Running(run.clone());
            st.sched.record_start(extension_id, now);
            *self.runners.0.lock_or_recover() += 1;
            run
        };
        self.emit_activity();

        let this = self.clone();
        let runner = run.clone();
        tokio::task::spawn_blocking(move || {
            let out = this.backend.run(&runner.prompt, &runner.ctl);
            this.finish_run(&runner, out);
        });
        tokio::spawn(self.clone().watchdog(run));

        rx.await
            .unwrap_or_else(|_| Err(call_error(ErrorKind::Internal, "unknown_call", &[])))
    }

    /// `Ok` when the call is this extension's and holds its turn.
    fn expect_turn(&self, extension_id: &str, call_id: &str) -> Result<(), AppError> {
        let st = self.state.lock_or_recover();
        match st.calls.get(call_id) {
            Some(c) if c.extension_id == extension_id => match c.phase {
                Phase::Turn { .. } => Ok(()),
                Phase::Waiting => Err(call_error(ErrorKind::Internal, "not_ready", &[])),
                Phase::Running(_) => Err(call_error(ErrorKind::Internal, "unknown_call", &[])),
            },
            _ => Err(Self::gone_error(&st, extension_id, call_id)),
        }
    }

    /// Enforce the timeout and the reply cap while the call runs, and
    /// resend `session/cancel` after a stop until the session ends.
    async fn watchdog(self: Arc<Self>, run: Arc<Running>) {
        let poll = std::time::Duration::from_millis(limits::WATCHDOG_POLL_MS);
        let byte_cap = run.max_response_chars as usize * limits::STREAM_BYTES_PER_CHAR;
        let timeout = Duration::milliseconds(i64::from(run.timeout_ms));
        while !run.done.load(Ordering::SeqCst) {
            tokio::select! {
                _ = run.ctl.changed.notified() => {}
                _ = tokio::time::sleep(poll) => {}
            }
            if run.done.load(Ordering::SeqCst) {
                break;
            }
            let now = self.clock.now();
            if run.ctl.stopped().is_some() {
                if let Some(sid) = run.ctl.cancel_due(now) {
                    self.backend.cancel(&sid);
                }
                continue;
            }
            let too_large = run
                .ctl
                .session()
                .is_some_and(|sid| self.backend.reply_len(&sid) > byte_cap);
            if too_large {
                self.stop_running(&run, Stop::ResponseTooLarge(run.max_response_chars));
            } else if now - run.started >= timeout {
                self.stop_running(&run, Stop::Timeout(run.timeout_ms / 1000));
            }
        }
    }

    /// The runner returned: decide the outcome, account for it, answer the
    /// extension (unless a stop already did), and free the turn.
    fn finish_run(&self, run: &Running, out: Result<String, RunError>) {
        run.done.store(true, Ordering::SeqCst);
        run.ctl.changed.notify_one();
        let now = self.clock.now();
        let reply = out.as_ref().cloned().unwrap_or_default();
        let response_chars = reply.chars().count();
        let (result, outcome, reason) = match run.ctl.stopped() {
            Some(stop) => {
                let (outcome, reason) = stop_outcome(stop);
                (Err(stop_error(stop, &run.name)), outcome, reason)
            }
            None => match out {
                Err(RunError::Unavailable | RunError::Stopped) => (
                    Err(call_error(
                        ErrorKind::ConnectionLost,
                        "agent_unavailable",
                        &[],
                    )),
                    AgentCallOutcome::ConnectionLost,
                    None,
                ),
                Err(RunError::Agent(_)) if !self.backend.connected() => (
                    Err(call_error(
                        ErrorKind::ConnectionLost,
                        "agent_disconnected",
                        &[],
                    )),
                    AgentCallOutcome::ConnectionLost,
                    None,
                ),
                Err(RunError::Agent(msg)) => {
                    let reason: String = msg.chars().take(limits::MAX_AGENT_ERROR_CHARS).collect();
                    (
                        Err(call_error(
                            ErrorKind::Internal,
                            "agent_error",
                            &[("reason", &reason)],
                        )),
                        AgentCallOutcome::AgentError,
                        None,
                    )
                }
                Ok(text) => {
                    let (r, o) = judge(run, text, response_chars, ms_between(run.started, now));
                    (r, o, None)
                }
            },
        };
        let error = result.as_ref().err().cloned();
        if run.ctl.was_sent() && !run.accounted.swap(true, Ordering::SeqCst) {
            self.account(run, outcome, reason, response_chars, reply, now);
        }
        // Free the turn (and the dedupe key) before answering, so the
        // extension can reserve again as soon as it has the result.
        {
            let mut st = self.state.lock_or_recover();
            if let Some(call) = st.calls.remove(&run.call_id) {
                call.signal.send_replace(Signal::Done(error.clone()));
            }
            st.sched
                .release(&run.call_id, run.ctl.was_sent().then_some(now));
            Self::remember_finished(&mut st, &run.call_id, &run.extension_id, error);
        }
        run.resolve(result);
        {
            let (count, cvar) = &self.runners;
            let mut n = count.lock_or_recover();
            *n = n.saturating_sub(1);
            cvar.notify_all();
        }
        self.emit_activity();
        self.wake.notify_one();
    }

    /// Charge the allowance, update the breakers, keep the prompt and
    /// reply in memory, write the audit entry and the log line.
    fn account(
        &self,
        run: &Running,
        outcome: AgentCallOutcome,
        reason: Option<CancelReason>,
        response_chars: usize,
        reply: String,
        now: DateTime<Utc>,
    ) {
        let clock = self.clock.as_ref();
        let today = limits::local_date(clock, now);
        let midnight = limits::next_local_midnight(clock, now);
        let chars = u64::from(run.prompt_chars) + response_chars as u64;
        let duration_ms = ms_between(run.started, now);
        let (change, closed) = {
            let mut st = self.state.lock_or_recover();
            self.ensure_usage(&mut st);
            let change = st.usage.as_mut().expect("loaded").charge(
                &run.extension_id,
                today,
                outcome,
                chars,
                now,
                midnight,
            );
            st.usage_dirty = true;
            st.recent.push(RequestDetail {
                call_id: run.call_id.clone(),
                extension_id: run.extension_id.clone(),
                purpose: run.purpose.clone(),
                label: run.label.clone(),
                at: limits::rfc3339_local(clock, now),
                prompt: run.prompt.clone(),
                // An agent that ignores the cap can't fill memory.
                reply: reply
                    .chars()
                    .take(run.max_response_chars as usize * limits::STREAM_BYTES_PER_CHAR)
                    .collect(),
            });
            // A breaker that opens rejects this extension's waiting calls.
            let mut closed = false;
            if let usage::BreakerChange::Opened(_) = change {
                let until = st
                    .usage
                    .as_ref()
                    .and_then(|b| b.pause(&run.extension_id, now))
                    .map_or(midnight, |(t, _)| t);
                let e = call_error(
                    ErrorKind::RateLimited,
                    "paused",
                    &[
                        ("extension", &run.name),
                        ("time", &self.time_text(now, until)),
                    ],
                );
                let ids: Vec<String> = st
                    .calls
                    .iter()
                    .filter(|(_, c)| {
                        c.extension_id == run.extension_id && !matches!(c.phase, Phase::Running(_))
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in ids {
                    closed |= self.finish_locked(&mut st, &id, Some(e.clone()));
                }
            }
            (change, closed)
        };
        self.persist();
        if closed {
            self.emit_activity();
        }
        let hooks = self.hooks();
        hooks.audit_call(&CallRecord {
            extension_id: run.extension_id.clone(),
            call_id: run.call_id.clone(),
            purpose: run.purpose.clone(),
            label: run
                .label
                .clone()
                .filter(|l| !crate::config_export::sensitive::looks_like_secret(l)),
            agent: run.agent.clone(),
            outcome,
            cancel_reason: reason,
            prompt_chars: run.prompt_chars,
            response_chars: to_u32(response_chars),
            duration_ms,
            redactions: run.redactions,
        });
        if let usage::BreakerChange::Opened(why) = change {
            hooks.telemetry(
                "extension_agent_calls_paused",
                serde_json::json!({
                    "extension_id": run.extension_id,
                    "reason": match why {
                        PauseReason::ToolAttempt => "tool_attempt",
                        _ => "failures",
                    },
                }),
            );
        }
        info_line(format!(
            "Background AI call {}: extension={} purpose={} outcome={} prompt_chars={} response_chars={} duration_ms={} redactions={}",
            run.call_id,
            run.extension_id,
            run.purpose,
            outcome_name(outcome),
            run.prompt_chars,
            response_chars,
            duration_ms,
            run.redactions,
        ));
    }

    /// `agent_call_status`.
    pub fn status(&self, extension_id: &str) -> AgentCallStatus {
        let g = self.gates(extension_id);
        let now = self.clock.now();
        let connected = self.backend.connected();
        let machine_id = self.machine_id();
        let mut st = self.state.lock_or_recover();
        self.ensure_usage(&mut st);
        let (waiting, running) = Self::counts(&st, extension_id);
        let book = st.usage.as_ref().expect("loaded");
        let clock = self.clock.as_ref();
        AgentCallStatus {
            available: g.available(),
            access: g.access,
            connected,
            waiting,
            running,
            usage: self.usage_of(&st, extension_id, &g, now),
            next_start_at: self
                .next_start(&st, extension_id, &g, now)
                .map(|t| limits::rfc3339_local(clock, t)),
            paused_until: book
                .pause(extension_id, now)
                .map(|(t, _)| limits::rfc3339_local(clock, t)),
            machine_tag: usage::machine_tag(&machine_id, extension_id),
        }
    }

    /// This machine's id, read (or created) on first use.
    fn machine_id(&self) -> String {
        if let Some(id) = self.state.lock_or_recover().machine_id.clone() {
            return id;
        }
        let id = usage::machine_id(self.machine_id_path.as_deref());
        self.state
            .lock_or_recover()
            .machine_id
            .get_or_insert(id)
            .clone()
    }

    fn counts(st: &State, extension_id: &str) -> (u32, bool) {
        let mut waiting = 0;
        let mut running = false;
        for c in st.calls.values().filter(|c| c.extension_id == extension_id) {
            match c.phase {
                Phase::Running(_) => running = true,
                _ => waiting += 1,
            }
        }
        (waiting, running)
    }

    fn usage_of(&self, st: &State, extension_id: &str, g: &Gates, now: DateTime<Utc>) -> Usage {
        let clock = self.clock.as_ref();
        let day = st
            .usage
            .as_ref()
            .expect("loaded")
            .day(extension_id, limits::local_date(clock, now));
        let allow = g.allowance();
        Usage {
            calls_today: day.calls,
            calls_limit: allow.calls,
            chars_today: day.chars,
            chars_limit: allow.chars,
            resets_at: limits::rfc3339_local(clock, limits::next_local_midnight(clock, now)),
        }
    }

    /// The earliest a call could start, when something holds it back.
    fn next_start(
        &self,
        st: &State,
        extension_id: &str,
        g: &Gates,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        let clock = self.clock.as_ref();
        let book = st.usage.as_ref()?;
        let today = limits::local_date(clock, now);
        let day = book.day(extension_id, today);
        let allow = g.allowance();
        let mut at: Option<DateTime<Utc>> = None;
        let mut later = |t: DateTime<Utc>| at = Some(at.map_or(t, |a| a.max(t)));
        if let Some((until, _)) = book.pause(extension_id, now) {
            later(until);
        }
        if day.calls >= allow.calls
            || day.chars >= allow.chars
            || book.global_calls(today) >= limits::GLOBAL_DAILY_CALLS
        {
            later(limits::next_local_midnight(clock, now));
        }
        if let Some(t) = st.sched.hourly_free_at(extension_id, now) {
            later(t);
        }
        if let Some(t) = st.sched.cooldown_until(extension_id, now) {
            later(t);
        }
        at
    }

    // --- Host-only commands ---------------------------------------------------------------

    /// `get_extension_agent_access`.
    pub fn access_panel(&self, extension_id: &str) -> AccessPanel {
        let g = self.gates(extension_id);
        let now = self.clock.now();
        let connected = self.backend.connected();
        let clock = self.clock.as_ref();
        let mut panel = {
            let mut st = self.state.lock_or_recover();
            self.ensure_usage(&mut st);
            let (waiting, running) = Self::counts(&st, extension_id);
            let book = st.usage.as_ref().expect("loaded");
            let daily = g.decl.as_ref().and_then(|d| d.daily_calls);
            AccessPanel {
                extension_id: extension_id.to_string(),
                name: g.name.clone(),
                available: g.available(),
                granted: g.granted,
                declared: g.decl.is_some(),
                extension_enabled: g.extension_enabled,
                enabled: g.entry.as_ref().is_some_and(|e| e.enabled),
                access: g.access,
                connected,
                agent: g.agent.clone(),
                consent_token: g.fingerprint.clone(),
                consent_agent: g
                    .entry
                    .as_ref()
                    .and_then(|e| e.consent.as_ref())
                    .map(|c| c.agent_label.clone()),
                allowance: g.level(),
                allowances: Allowances {
                    low: limits::allowance(AgentAllowance::Low, daily),
                    normal: limits::allowance(AgentAllowance::Normal, daily),
                    high: limits::allowance(AgentAllowance::High, daily),
                },
                usage: self.usage_of(&st, extension_id, &g, now),
                pause: book.pause(extension_id, now).map(|(t, reason)| PauseInfo {
                    until: limits::rfc3339_local(clock, t),
                    reason,
                }),
                next_start_at: self
                    .next_start(&st, extension_id, &g, now)
                    .map(|t| limits::rfc3339_local(clock, t)),
                running,
                waiting,
                reason: g.decl.as_ref().map(|d| d.reason.clone()),
                purposes: g
                    .decl
                    .as_ref()
                    .map(|d| d.purposes.clone())
                    .unwrap_or_default(),
                reads: g.reads.clone(),
                viewable: st.recent.ids_for(extension_id),
                log: Vec::new(),
            }
        };
        panel.log = self.hooks().log_rows(extension_id, PANEL_LOG_ROWS);
        panel
    }

    /// `set_extension_agent_access`: turn on (with the token from
    /// [`access_panel`](Self::access_panel)), turn off, or change the
    /// allowance. Saves the config; the caller broadcasts
    /// `config_updated`.
    pub fn set_access(
        &self,
        extension_id: &str,
        enabled: bool,
        consent_token: Option<String>,
        allowance: Option<AgentAllowance>,
    ) -> Result<(), AppError> {
        let g = self.gates(extension_id);
        // Turning off always works; turning on needs gates 1 and 2.
        if enabled && !g.available() {
            return Err(call_error(
                ErrorKind::NotPermitted,
                "not_available",
                &[("extension", &g.name)],
            ));
        }
        let (was_on, was_allowed, label) = {
            let mut cfg = self.config.lock_or_recover();
            let active = cfg.acp.active_connection().cloned();
            let before = cfg.extension_agent_access.get(extension_id).cloned();
            let was_on = before.as_ref().is_some_and(|e| e.enabled);
            // Calls could run: on, with an OK for the agent active now.
            let was_allowed = consent::access_state(
                before.as_ref(),
                active.as_ref().map(consent::fingerprint).as_deref(),
            ) == Access::On;
            let mut entry = before.clone().unwrap_or_default();
            let label = active.as_ref().map(consent::agent_label);
            if enabled {
                let Some(conn) = active.as_ref() else {
                    return Err(call_error(
                        ErrorKind::ConnectionLost,
                        "agent_unavailable",
                        &[],
                    ));
                };
                let fp = consent::fingerprint(conn);
                match consent_token {
                    Some(t) if t == fp => {
                        entry.consent = Some(AgentConsent {
                            agent_fingerprint: fp,
                            agent_label: consent::agent_label(conn),
                            granted_at: usage::format_time(self.clock.now()),
                        });
                    }
                    Some(_) => {
                        return Err(call_error(ErrorKind::NotPermitted, "agent_changed", &[]));
                    }
                    None if consent::access_state(Some(&entry), Some(&fp)) == Access::On => {}
                    None => {
                        return Err(call_error(
                            ErrorKind::NotPermitted,
                            "consent_required",
                            &[("extension", &g.name)],
                        ));
                    }
                }
                entry.enabled = true;
            } else {
                entry.enabled = false;
                entry.consent = None;
            }
            if let Some(a) = allowance {
                entry.allowance = a;
            }
            if before.as_ref() != Some(&entry) {
                if !enabled && entry == ExtensionAgentAccess::default() {
                    cfg.extension_agent_access.remove(extension_id);
                } else {
                    cfg.extension_agent_access
                        .insert(extension_id.to_string(), entry);
                }
                if let Err(e) = (self.save_config)(&cfg) {
                    // Keep memory and disk the same.
                    match before {
                        Some(b) => {
                            cfg.extension_agent_access
                                .insert(extension_id.to_string(), b);
                        }
                        None => {
                            cfg.extension_agent_access.remove(extension_id);
                        }
                    }
                    return Err(AppError::internal(format!("Failed to save config: {e}")));
                }
            }
            (was_on, was_allowed, label)
        };
        // The audit log keeps every OK the user gives, with the agent it
        // names: also one given while the switch was already on (the
        // agent changed, or no OK was recorded). And every switch-off.
        // Telemetry counts flips of the switch only.
        let audited = if enabled { !was_allowed } else { was_on };
        let hooks = self.hooks();
        if audited {
            hooks.audit_access(&AccessRecord {
                extension_id: extension_id.to_string(),
                enabled,
                agent: label,
            });
            info_line(format!(
                "Background AI turned {} for extension {}",
                if enabled { "on" } else { "off" },
                extension_id
            ));
        }
        if was_on != enabled {
            hooks.telemetry(
                "extension_agent_access_changed",
                serde_json::json!({ "extension_id": extension_id, "enabled": u8::from(enabled) }),
            );
        }
        if !enabled {
            self.on_extension_changed(extension_id);
        }
        self.wake.notify_one();
        Ok(())
    }

    /// `stop_extension_agent_calls`: cancel the running call and every
    /// waiting one, and pause that extension (or every extension) for an
    /// hour.
    pub fn stop(&self, extension_id: Option<&str>) {
        let now = self.clock.now();
        let until = now + Duration::minutes(limits::STOP_PAUSE_MINUTES);
        let running = {
            let mut st = self.state.lock_or_recover();
            self.ensure_usage(&mut st);
            let book = st.usage.as_mut().expect("loaded");
            match extension_id {
                Some(id) => book.set_pause(id, until, PauseReason::Stopped),
                None => book.set_pause_all(until),
            }
            st.usage_dirty = true;
            let e = call_error(ErrorKind::Cancelled, "cancelled_by_user", &[]);
            self.end_matching(
                &mut st,
                |c| extension_id.is_none_or(|id| c.extension_id == id),
                &e,
            )
        };
        for r in running {
            self.stop_running(&r, Stop::ByUser);
        }
        self.persist();
        self.emit_activity();
        info_line(format!(
            "Background AI stopped by the user for {}",
            extension_id.unwrap_or("every extension")
        ));
    }

    /// End every non-running call that matches with `error`; return the
    /// running ones that match, for the caller to stop outside the lock.
    fn end_matching(
        &self,
        st: &mut State,
        matches: impl Fn(&Call) -> bool,
        error: &AppError,
    ) -> Vec<Arc<Running>> {
        let mut running = Vec::new();
        let mut ids = Vec::new();
        for (id, c) in st.calls.iter().filter(|(_, c)| matches(c)) {
            match &c.phase {
                Phase::Running(r) => running.push(r.clone()),
                _ => ids.push(id.clone()),
            }
        }
        for id in ids {
            self.finish_locked(st, &id, Some(error.clone()));
        }
        running
    }

    /// `get_extension_agent_request_detail`: one of this session's last
    /// calls, prompt and reply exactly. Memory only.
    pub fn request_detail(
        &self,
        extension_id: &str,
        call_id: &str,
    ) -> Result<RequestDetail, AppError> {
        self.state
            .lock_or_recover()
            .recent
            .get(extension_id, call_id)
            .cloned()
            .ok_or_else(|| call_error(ErrorKind::Internal, "unknown_call", &[]))
    }

    // --- Hooks from the rest of Kage -------------------------------------------------------

    /// The user did something in Kage (floating input, a window shown or
    /// focused). Holds back new turns of `idle` calls for 90 s.
    pub fn note_user_activity(&self) {
        let now = self.clock.now();
        self.state.lock_or_recover().last_activity = Some(now);
    }

    /// A user prompt started: stop the running call
    /// (`cancelled_user_activity`). Waiting calls stay, held back.
    pub fn on_user_prompt_started(&self) {
        self.note_user_activity();
        for r in self.running_calls() {
            self.stop_running(&r, Stop::UserActivity);
        }
    }

    /// The agent disconnected: the running call ends `agent_disconnected`.
    /// Waiting calls stay and wait for a reconnect.
    pub fn on_disconnect(&self) {
        for r in self.running_calls() {
            self.stop_running(&r, Stop::Disconnected);
        }
    }

    /// The agent asked for a tool, or started one, on `session_id`.
    pub fn note_tool_attempt(&self, session_id: &str) {
        for r in self.running_calls() {
            if r.ctl.session().as_deref() == Some(session_id) {
                info_line(format!(
                    "Background AI call {} tried to use a tool; stopping it",
                    r.call_id
                ));
                self.stop_running(&r, Stop::ToolAttempted);
            }
        }
    }

    /// The active agent connection changed or was edited: calls whose
    /// consent no longer matches end `consent_stale`.
    pub fn on_agent_changed(&self) {
        self.recheck(None);
    }

    /// An extension was disabled, uninstalled, lost its grant, or its host
    /// switch changed: its calls end `cancelled_access_off` when a gate
    /// now fails.
    pub fn on_extension_changed(&self, extension_id: &str) {
        self.recheck(Some(extension_id));
    }

    fn running_calls(&self) -> Vec<Arc<Running>> {
        self.state
            .lock_or_recover()
            .calls
            .values()
            .filter_map(|c| match &c.phase {
                Phase::Running(r) => Some(r.clone()),
                _ => None,
            })
            .collect()
    }

    fn recheck(&self, only: Option<&str>) {
        let mut extensions: Vec<String> = self
            .state
            .lock_or_recover()
            .calls
            .values()
            .map(|c| c.extension_id.clone())
            .filter(|e| only.is_none_or(|o| o == e))
            .collect();
        extensions.sort();
        extensions.dedup();
        let mut changed = false;
        for ext in extensions {
            let g = self.gates(&ext);
            let Err(gate) = g.check() else {
                continue;
            };
            let (error, stop) = g.closing_error(&gate);
            let running = {
                let mut st = self.state.lock_or_recover();
                let before = st.calls.len();
                let running = self.end_matching(&mut st, |c| c.extension_id == ext, &error);
                changed |= st.calls.len() != before;
                running
            };
            for r in running {
                self.stop_running(&r, stop);
            }
        }
        if changed {
            self.emit_activity();
        }
    }

    /// Kage is quitting: no new turns; the running call is cancelled and
    /// gets up to 2 s to delete its session; its audit entry is written and
    /// the usage file saved. Call before disconnecting the agent. Every
    /// exit path calls it, so a second call returns at once.
    pub fn shutdown(&self) {
        let running = {
            let mut st = self.state.lock_or_recover();
            if st.shutting_down {
                return;
            }
            st.shutting_down = true;
            let e = call_error(ErrorKind::Cancelled, "cancelled_shutdown", &[]);
            self.end_matching(&mut st, |_| true, &e)
        };
        for r in &running {
            self.stop_running(r, Stop::Shutdown);
        }
        self.wake.notify_one();
        {
            let (count, cvar) = &self.runners;
            let n = count.lock_or_recover();
            let wait = std::time::Duration::from_millis(limits::SHUTDOWN_WAIT_MS);
            let _ = cvar.wait_timeout_while(n, wait, |n| *n > 0);
        }
        // A session that didn't end in time: account for it now.
        let now = self.clock.now();
        for r in &running {
            if r.ctl.was_sent() && !r.accounted.swap(true, Ordering::SeqCst) {
                let (outcome, reason) = stop_outcome(Stop::Shutdown);
                self.account(r, outcome, reason, 0, String::new(), now);
            }
        }
        {
            let mut st = self.state.lock_or_recover();
            if st.usage.is_some() {
                st.usage_dirty = true;
            }
        }
        self.persist();
    }

    // --- Scheduling ----------------------------------------------------------------------

    /// How long the scheduler task may sleep: until the turn holder's
    /// deadline, at most [`limits::TICK_MS`].
    fn next_wake(&self) -> std::time::Duration {
        let tick = std::time::Duration::from_millis(limits::TICK_MS);
        let now = self.clock.now();
        let st = self.state.lock_or_recover();
        let deadline = st
            .sched
            .active()
            .and_then(|id| match st.calls.get(id)?.phase {
                Phase::Turn { send_by } => Some(send_by),
                _ => None,
            });
        deadline
            .and_then(|d| (d - now).to_std().ok())
            .map_or(tick, |d| d.min(tick))
            .max(std::time::Duration::from_millis(50))
    }

    /// One scheduler pass: report finished days, give up an expired turn,
    /// and start the next turn if everything allows it. The scheduler task
    /// runs this on a blocking thread.
    pub fn pump(&self) {
        let now = self.clock.now();
        self.report_days(now);
        let mut changed = false;
        {
            let mut st = self.state.lock_or_recover();
            if let Some(id) = st.sched.active().map(str::to_string) {
                let expired = matches!(
                    st.calls.get(&id).map(|c| &c.phase),
                    Some(Phase::Turn { send_by }) if *send_by <= now
                );
                if expired {
                    let e = call_error(
                        ErrorKind::Timeout,
                        "turn_expired",
                        &[("seconds", &limits::SEND_WINDOW_SECS.to_string())],
                    );
                    changed |= self.finish_locked(&mut st, &id, Some(e));
                }
            }
        }
        // Each pass of this loop either starts a turn, ends a call, or
        // stops; it can't run more often than there are waiting calls.
        let rounds = self.state.lock_or_recover().sched.waiting_total() + 1;
        for _ in 0..rounds {
            match self.try_start_turn(now) {
                TurnAttempt::Started | TurnAttempt::Ended => changed = true,
                TurnAttempt::Nothing => break,
            }
            if self.state.lock_or_recover().sched.active().is_some() {
                break;
            }
        }
        if changed {
            self.emit_activity();
        }
    }

    fn report_days(&self, now: DateTime<Utc>) {
        let today = limits::local_date(self.clock.as_ref(), now);
        let taken = {
            let mut st = self.state.lock_or_recover();
            if st.reported_on == Some(today) {
                return;
            }
            self.ensure_usage(&mut st);
            st.reported_on = Some(today);
            let taken = st.usage.as_mut().expect("loaded").take_unreported(today);
            st.usage_dirty = true;
            taken
        };
        self.persist();
        let hooks = self.hooks();
        for (ext, day) in taken {
            hooks.telemetry(
                "extension_agent_calls_daily",
                serde_json::json!({
                    "extension_id": ext,
                    "calls": day.calls,
                    "ok": day.ok,
                    "failed": day.failed,
                    "cancelled": day.cancelled,
                    "tool_attempts": day.tool_attempts,
                    "kchars_bucket": limits::kchars_bucket(day.chars),
                }),
            );
        }
    }

    fn try_start_turn(&self, now: DateTime<Utc>) -> TurnAttempt {
        // Cheap, global conditions first.
        let candidate = {
            let mut st = self.state.lock_or_recover();
            if st.shutting_down || st.sched.active().is_some() || st.sched.waiting_total() == 0 {
                return TurnAttempt::Nothing;
            }
            self.ensure_usage(&mut st);
            let today = limits::local_date(self.clock.as_ref(), now);
            if st.usage.as_ref().expect("loaded").global_calls(today) >= limits::GLOBAL_DAILY_CALLS
            {
                return TurnAttempt::Nothing;
            }
            drop(st);
            if !self.backend.connected() || self.backend.compacting() || self.backend.low_power() {
                return TurnAttempt::Nothing;
            }
            let busy = self.backend.user_prompt_in_flight();
            let st = self.state.lock_or_recover();
            let user_active = busy || self.user_active(&st, now);
            st.sched
                .heads_in_turn_order()
                .into_iter()
                .find(|(ext, id)| {
                    let Some(call) = st.calls.get(id) else {
                        return false;
                    };
                    !(call.when == When::Idle && user_active)
                        && st.sched.cooldown_until(ext, now).is_none()
                        && st.sched.hourly_free_at(ext, now).is_none()
                })
        };
        let Some((ext, id)) = candidate else {
            return TurnAttempt::Nothing;
        };
        // Gates and allowance (reads the config and the manifest).
        let g = self.gates(&ext);
        let mut st = self.state.lock_or_recover();
        let still_head = st
            .calls
            .get(&id)
            .is_some_and(|c| matches!(c.phase, Phase::Waiting))
            && st.sched.active().is_none();
        if !still_head {
            return TurnAttempt::Nothing;
        }
        if let Err(gate) = g.check() {
            let (error, _) = g.closing_error(&gate);
            self.finish_locked(&mut st, &id, Some(error));
            return TurnAttempt::Ended;
        }
        if let Err(e) = self.check_budget(&st, &ext, &g, now, 0) {
            // A pause or an empty allowance won't clear soon: give up the
            // waiting calls of this extension rather than hold them for
            // hours.
            let ids: Vec<String> = st
                .calls
                .iter()
                .filter(|(_, c)| c.extension_id == ext && matches!(c.phase, Phase::Waiting))
                .map(|(i, _)| i.clone())
                .collect();
            for i in ids {
                self.finish_locked(&mut st, &i, Some(e.clone()));
            }
            return TurnAttempt::Ended;
        }
        if !st.sched.grant(&ext, &id) {
            return TurnAttempt::Nothing;
        }
        let send_by = now + Duration::seconds(limits::SEND_WINDOW_SECS);
        let send_by_text = limits::rfc3339_local(self.clock.as_ref(), send_by);
        let call = st.calls.get_mut(&id).expect("checked");
        call.phase = Phase::Turn { send_by };
        call.signal.send_replace(Signal::Turn {
            send_by: send_by_text,
        });
        TurnAttempt::Started
    }
}

enum TurnAttempt {
    Started,
    Ended,
    Nothing,
}

/// The outcome of a reply that came back whole.
fn judge(
    run: &Running,
    text: String,
    response_chars: usize,
    duration_ms: u64,
) -> (Result<AgentCallResult, AppError>, AgentCallOutcome) {
    if response_chars > run.max_response_chars as usize {
        return (
            Err(call_error(
                ErrorKind::Internal,
                "response_too_large",
                &[("limit", &run.max_response_chars.to_string())],
            )),
            AgentCallOutcome::ResponseTooLarge,
        );
    }
    let json = match &run.schema {
        None => None,
        Some(schema) => {
            let Some(value) = schema::extract_json(&text) else {
                return (
                    Err(call_error(ErrorKind::Internal, "not_json", &[])),
                    AgentCallOutcome::NotJson,
                );
            };
            if let Err(path) = schema::validate(schema, &value) {
                return (
                    Err(call_error(
                        ErrorKind::Internal,
                        "invalid_response",
                        &[("path", &path)],
                    )),
                    AgentCallOutcome::InvalidResponse,
                );
            }
            Some(value)
        }
    };
    (
        Ok(AgentCallResult {
            call_id: run.call_id.clone(),
            text,
            json,
            prompt_chars: run.prompt_chars,
            response_chars: to_u32(response_chars),
            duration_ms,
            redactions: run.redactions,
        }),
        AgentCallOutcome::Ok,
    )
}

/// The error the extension gets for a stop.
fn stop_error(stop: Stop, name: &str) -> AppError {
    let ext = [("extension", name)];
    match stop {
        Stop::Timeout(secs) => call_error(
            ErrorKind::Timeout,
            "timeout",
            &[("seconds", &secs.to_string())],
        ),
        Stop::ResponseTooLarge(limit) => call_error(
            ErrorKind::Internal,
            "response_too_large",
            &[("limit", &limit.to_string())],
        ),
        Stop::ToolAttempted => call_error(ErrorKind::Internal, "tool_attempted", &[]),
        Stop::UserActivity => call_error(ErrorKind::Cancelled, "cancelled_user_activity", &[]),
        Stop::ByUser => call_error(ErrorKind::Cancelled, "cancelled_by_user", &[]),
        Stop::ByExtension => call_error(ErrorKind::Cancelled, "cancelled_by_extension", &[]),
        Stop::Shutdown => call_error(ErrorKind::Cancelled, "cancelled_shutdown", &[]),
        Stop::AccessOff => call_error(ErrorKind::Cancelled, "cancelled_access_off", &ext),
        Stop::ConsentStale => call_error(ErrorKind::NotPermitted, "consent_stale", &ext),
        Stop::Disconnected => call_error(ErrorKind::ConnectionLost, "agent_disconnected", &[]),
    }
}

/// The audit outcome and cancel reason for a stop.
fn stop_outcome(stop: Stop) -> (AgentCallOutcome, Option<CancelReason>) {
    match stop {
        Stop::Timeout(_) => (AgentCallOutcome::Timeout, None),
        Stop::ResponseTooLarge(_) => (AgentCallOutcome::ResponseTooLarge, None),
        Stop::ToolAttempted => (AgentCallOutcome::ToolAttempted, None),
        Stop::Disconnected => (AgentCallOutcome::ConnectionLost, None),
        Stop::UserActivity => (AgentCallOutcome::Cancelled, Some(CancelReason::Activity)),
        Stop::ByUser => (AgentCallOutcome::Cancelled, Some(CancelReason::User)),
        Stop::ByExtension => (AgentCallOutcome::Cancelled, Some(CancelReason::Extension)),
        Stop::Shutdown => (AgentCallOutcome::Cancelled, Some(CancelReason::Shutdown)),
        Stop::AccessOff | Stop::ConsentStale => {
            (AgentCallOutcome::Cancelled, Some(CancelReason::AccessOff))
        }
    }
}

fn outcome_name(o: AgentCallOutcome) -> &'static str {
    match o {
        AgentCallOutcome::Ok => "ok",
        AgentCallOutcome::InvalidResponse => "invalid_response",
        AgentCallOutcome::NotJson => "not_json",
        AgentCallOutcome::ResponseTooLarge => "response_too_large",
        AgentCallOutcome::Timeout => "timeout",
        AgentCallOutcome::Cancelled => "cancelled",
        AgentCallOutcome::ToolAttempted => "tool_attempted",
        AgentCallOutcome::AgentError => "agent_error",
        AgentCallOutcome::ConnectionLost => "connection_lost",
    }
}
