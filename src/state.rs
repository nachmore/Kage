use crate::acp_client::AcpClient;
use crate::app_launcher::AppLauncher;
use crate::config::Config;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::Mutex;

/// State that lives or dies with the agent connection — the ACP client itself
/// and the per-session bookkeeping that tracks one in-flight conversation
/// (pending permission prompt, slash commands the agent advertised, available
/// models). When we tear down the ACP connection on reconnect we should
/// conceptually reset everything in this bucket together.
pub struct AcpHandles {
    /// AcpClient is internally synchronized — every method takes `&self` and
    /// the type owns its own fine-grained locks (transport pending map,
    /// session id, streaming accumulator, compacting condvar). Wrapping it
    /// in an outer mutex would just serialize callers behind whatever
    /// long-running prompt happened to hold the guard, which is what the
    /// pre-2026-05 codebase did.
    pub client: Arc<AcpClient>,
    /// Pending permission requests, keyed by the serialized JSON-RPC
    /// request id. Inserted when a permission_request notification
    /// arrives, removed when responded to / dismissed. A map (not a
    /// single slot) because multiple chat windows on different sessions
    /// can each have a prompt blocked on a permission at the same time —
    /// a single slot lost every request but the latest.
    pub pending_permissions: Arc<std::sync::Mutex<HashMap<String, PendingPermission>>>,
    /// Slash commands received from the ACP server via the
    /// `commands/available` vendor extension notification (under either
    /// `_kage.dev/` or `_kiro.dev/` — see acp_client::vendor_method_suffix).
    pub slash_commands: Arc<std::sync::Mutex<Vec<SlashCommand>>>,
    /// Available models from the ACP session/new response
    pub available_models: Arc<std::sync::Mutex<Vec<AcpModel>>>,
    /// Which extension tool steering block each session has been sent (to
    /// avoid duplicates, per session). See `ToolSteeringState`.
    pub tool_steering: Arc<std::sync::Mutex<ToolSteeringState>>,
}

/// Per-session delivery bookkeeping for extension tool steering.
///
/// Keyed by session because every window sends the same block on its own
/// session: a single process-wide hash let whichever window sent first
/// starve every other session of the tool definitions.
///
/// Windows re-publish whenever their extension tools change (a setting
/// toggled, an extension enabled or installed), so a session that already
/// has a block is sent the replacement once, and a session that already
/// has exactly the current block is sent nothing. When a window's last tool
/// goes away it publishes an empty block; that becomes
/// [`TOOLS_WITHDRAWN_BLOCK`], delivered only to sessions that were told
/// about tools before.
///
/// Blocks are kept per publishing window: the window that prompts a session
/// is the one that runs the tool calls the reply asks for, so a session is
/// steered with that window's tools, and one window losing its tools can't
/// withdraw them from another window's sessions.
#[derive(Debug, Default)]
pub struct ToolSteeringState {
    /// Window label → latest block that window published.
    latest: HashMap<String, LatestSteering>,
    /// The window that published most recently. Its block stands in for a
    /// window that hasn't published (yet), e.g. one still loading its
    /// extensions.
    last_publisher: Option<String>,
    /// Session id → hash of the block the agent was sent on it.
    delivered: HashMap<String, u64>,
    /// Session id → hash of a claimed block not yet delivered or released.
    in_flight: HashMap<String, u64>,
    /// Session id → lock held while a block is delivered on it, so blocks
    /// reach the agent in claim order and a user prompt goes out only after
    /// a delivery already under way.
    delivery_locks: HashMap<String, Arc<std::sync::Mutex<()>>>,
}

#[derive(Debug, Clone)]
struct LatestSteering {
    hash: u64,
    body: String,
    /// The tools-withdrawn notice rather than a tool list.
    withdrawal: bool,
}

/// Agent-facing replacement for a tool list once no extension tools are
/// left. English on purpose: steering is for the agent, not the user.
pub const TOOLS_WITHDRAWN_BLOCK: &str = "<extension_tools>\n\
No local extension tools are available any more. The extension tools listed \
earlier in this conversation were turned off or removed: do not emit \
extension_tool_call blocks for them.\n\
</extension_tools>";

/// One session claimed for one steering delivery. Hand it to
/// [`ToolSteeringState::confirm`] once sent, or to
/// [`ToolSteeringState::release`] if the send didn't happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteeringClaim {
    pub hash: u64,
    /// The steering text to send.
    pub body: String,
}

/// `delivered` value for a session whose last block may or may not have
/// landed (its prompt was cancelled): the agent has *some* steering, so a
/// withdrawal still applies, and any published block differs from it, so
/// the next prompt re-sends.
const UNKNOWN_DELIVERY: u64 = 0;

impl ToolSteeringState {
    fn hash_block(block: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        block.hash(&mut hasher);
        hasher.finish()
    }

    /// Publish `block` as `window`'s current extension tool steering and
    /// claim `session_id` for it. An empty block means "no tools now".
    /// Returns `None` if this session already has (or is receiving) exactly
    /// this block, or if it's a withdrawal for a session that never had
    /// tools. Claiming before the send is what stops two windows sharing a
    /// session from both sending it.
    pub fn claim(&mut self, window: &str, session_id: &str, block: &str) -> Option<SteeringClaim> {
        self.publish(window, block);
        self.claim_latest(window, session_id)
    }

    /// Record `block` as `window`'s current steering without claiming a
    /// session (the window has none right now). Its sessions get it replayed
    /// before their next prompt.
    pub fn publish(&mut self, window: &str, block: &str) {
        let latest = if !block.trim().is_empty() {
            LatestSteering {
                hash: Self::hash_block(block),
                body: block.to_string(),
                withdrawal: false,
            }
        } else if self.latest.contains_key(window) {
            // No tools now, but this window's sessions may have been told
            // about some.
            LatestSteering {
                hash: Self::hash_block(TOOLS_WITHDRAWN_BLOCK),
                body: TOOLS_WITHDRAWN_BLOCK.to_string(),
                withdrawal: true,
            }
        } else {
            // Nothing published by this window yet: nothing to withdraw.
            return;
        };
        self.latest.insert(window.to_string(), latest);
        self.last_publisher = Some(window.to_string());
    }

    /// Claim `session_id` for the latest block `window` published (or, if
    /// it hasn't published, the most recent one any window did) if the
    /// session doesn't have it.
    pub fn claim_latest(&mut self, window: &str, session_id: &str) -> Option<SteeringClaim> {
        let latest = self
            .latest
            .get(window)
            .or_else(|| self.latest.get(self.last_publisher.as_deref()?))?;
        let (hash, withdrawal) = (latest.hash, latest.withdrawal);
        let current = self
            .in_flight
            .get(session_id)
            .or_else(|| self.delivered.get(session_id))
            .copied();
        if current == Some(hash) || (withdrawal && current.is_none()) {
            return None;
        }
        let body = latest.body.clone();
        self.in_flight.insert(session_id.to_string(), hash);
        Some(SteeringClaim { hash, body })
    }

    /// Whether `claim` is still the newest claim on `session_id`. A newer
    /// one carries the current tools, so a superseded claim isn't sent.
    pub fn is_current(&self, session_id: &str, claim: &SteeringClaim) -> bool {
        self.in_flight.get(session_id) == Some(&claim.hash)
    }

    /// Whether a claimed block is still on its way on `session_id`.
    pub fn has_in_flight(&self, session_id: &str) -> bool {
        self.in_flight.contains_key(session_id)
    }

    /// Record that `claim` reached the agent.
    pub fn confirm(&mut self, session_id: &str, claim: &SteeringClaim) {
        if self.in_flight.get(session_id) == Some(&claim.hash) {
            self.in_flight.remove(session_id);
        }
        self.delivered.insert(session_id.to_string(), claim.hash);
    }

    /// Undo a claim whose send failed or may not have landed (cancelled),
    /// so a later call retries. Leaves a newer claim alone. A block already
    /// recorded as delivered becomes "unknown": the agent may have it.
    pub fn release(&mut self, session_id: &str, claim: &SteeringClaim) {
        if self.in_flight.get(session_id) == Some(&claim.hash) {
            self.in_flight.remove(session_id);
        }
        if self.delivered.get(session_id) == Some(&claim.hash) {
            self.delivered
                .insert(session_id.to_string(), UNKNOWN_DELIVERY);
        }
    }

    /// The lock serialising steering deliveries on `session_id`. Lock it
    /// after dropping the state guard: it's held across a whole agent turn.
    pub fn delivery_lock(&mut self, session_id: &str) -> Arc<std::sync::Mutex<()>> {
        self.delivery_locks
            .entry(session_id.to_string())
            .or_default()
            .clone()
    }
}

/// Frontend-driven UI state — typically set when the floating window's
/// global hotkey fires, then read when the user sends a message. None of
/// these survive a restart.
pub struct UiState {
    pub dev_mode: bool,
    /// Per-window pinned session ids. Keyed by Tauri webview label
    /// (`main`, `floating`, future `chat-<uuid>`). The frontend writes
    /// to this via `set_window_session` whenever a window adopts a
    /// session (boot, switch, new); the backend reads it where it
    /// needs to know "which session does X belong to" — quit-time
    /// auto-steering, the updater's resume marker, the floating
    /// expand-to-chat handoff. No entry means the window has no
    /// pinned session yet.
    pub window_sessions: Arc<std::sync::Mutex<HashMap<String, String>>>,
    /// Maps an in-flight session id to the window label that issued
    /// the prompt. Written by `send_message_streaming` before the ACP
    /// call, read by the permission handler to route the modal back
    /// to the originating window, cleared on prompt complete/error.
    /// A miss falls back to "floating" — the historical default for
    /// hotkey-driven prompts.
    pub pending_prompt_originators: Arc<std::sync::Mutex<HashMap<String, String>>>,
    /// Label of the most recently focused chat window (`main` or
    /// `chat-<uuid>`). Written by the global `WindowEvent::Focused`
    /// listener installed in setup; read by the single-instance handler
    /// and any "bring chat to front" affordance to decide which window
    /// to surface. None means no chat window has been focused this
    /// session — fall back to `main`.
    pub last_focused_chat: Arc<std::sync::Mutex<Option<String>>>,
    /// Generation counter for the chat-window shutdown timer. When the
    /// last chat window closes we schedule a "disconnect the agent in
    /// 30s" task; if a chat window opens before the task fires it
    /// bumps this counter so the pending task observes the change and
    /// exits without disconnecting. Avoids needing a JoinHandle that
    /// can be aborted (which Tauri's runtime makes awkward).
    pub chat_shutdown_generation: Arc<std::sync::atomic::AtomicU64>,
    /// Text that was selected in the previously active window when the hotkey was pressed
    pub last_selection: Arc<std::sync::Mutex<Option<String>>>,
    /// Info about the foreground window when the hotkey was pressed (title, process_name)
    pub source_window: Arc<std::sync::Mutex<Option<(String, String)>>>,
    /// Whether the floating window frontend's `init()` has completed.
    /// Diagnostic-only — written by `notify_frontend_ready`, never read
    /// as a gate (see comment on that command). The "Frontend signaled
    /// ready" log line it emits is what we actually rely on.
    pub frontend_ready: Arc<AtomicBool>,
    /// Last set of global-hotkey registration failures, as `(slot, hotkey)`
    /// pairs. `register_all_hotkeys` overwrites this each run and emits
    /// `HOTKEY_REGISTRATION_FAILED`. The Settings → Hotkeys window reads it
    /// via `get_hotkey_registration_failures` on open, so a failure that
    /// happened at startup (before any window could listen) is still
    /// discoverable. Empty means the last registration pass was fully clean.
    pub hotkey_registration_failures: Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

/// Child processes we spawn and need to clean up. Held as `Option<Child>`
/// so the slot is reusable: starting again replaces; stopping clears. The
/// Job Object on Windows kills these on parent exit even if we crash.
pub struct ChildProcesses {
    /// Pocket TTS server child process
    pub pocket_tts: Arc<std::sync::Mutex<Option<std::process::Child>>>,
    /// Pocket TTS pip install child process (for cancellation)
    pub pocket_tts_install: Arc<std::sync::Mutex<Option<std::process::Child>>>,
}

/// Long-lived feature singletons and caches — services that exist for the
/// process lifetime. Each is independent of the others; this is the
/// "everything else that's process-scoped" bucket.
pub struct FeatureServices {
    pub config: Arc<std::sync::Mutex<Config>>,
    pub app_launcher: Arc<Mutex<AppLauncher>>,
    pub updater: Arc<crate::updater::UpdaterState>,
    /// Cached user info (expensive to compute — involves subprocess on Windows)
    pub user_info_cache: Arc<std::sync::Mutex<Option<crate::commands::system::UserInfo>>>,
    /// Cached session list (avoids re-scanning directory on every call)
    pub session_cache: Arc<std::sync::Mutex<Option<crate::commands::sessions::SessionCache>>>,
    /// Cancellation flag for automation plan execution
    pub automation_plan_cancelled: Arc<AtomicBool>,
    /// Activity tracker for focus/screen time reports
    pub activity_tracker: Arc<crate::activity_tracker::ActivityTrackerState>,
    /// Runtime registry of agent session providers (kiro-cli sqlite, kage
    /// desktop json/.chat, future Claude Code/Codex/Ollama). Owns each
    /// provider's per-instance cache. See `agent_sessions::AgentSessionRegistry`.
    pub agent_session_registry: Arc<crate::agent_sessions::AgentSessionRegistry>,
    /// Automation signal sender (for extensions to emit signals)
    pub automation_signal_tx: Arc<
        std::sync::Mutex<Option<tokio::sync::mpsc::Sender<crate::automation::AutomationSignal>>>,
    >,
}

/// The full set of Tauri-managed state, built in one place so
/// production (`main.rs::run`) and the mock-app harness
/// (`tests/mock_app_test.rs`) manage EXACTLY the same types. If you add
/// a `.manage()`-ed type, add it here — the harness then covers it
/// automatically, and `tests/state_manage_parity_test.rs` enforces that
/// no code requests state outside this set.
pub struct ManagedState {
    pub acp_handles: AcpHandles,
    pub ui_state: UiState,
    pub child_processes: ChildProcesses,
    pub feature_services: FeatureServices,
}

/// Construct every managed-state value from a loaded `Config` and an
/// `AcpClient`. Pure construction — no I/O, no spawns, no Tauri types —
/// so it is callable from integration tests without a runtime.
///
/// `main.rs` layers side-effectful wiring (signal handlers, child-killer
/// hooks) around this after calling it; those belong at the call site,
/// not here, precisely so tests can build state without them.
pub fn build_managed_state(
    config: Arc<std::sync::Mutex<Config>>,
    acp_client: Arc<AcpClient>,
    dev_mode: bool,
) -> ManagedState {
    ManagedState {
        acp_handles: AcpHandles {
            client: acp_client,
            pending_permissions: Arc::new(std::sync::Mutex::new(HashMap::new())),
            slash_commands: Arc::new(std::sync::Mutex::new(Vec::new())),
            available_models: Arc::new(std::sync::Mutex::new(Vec::new())),
            tool_steering: Arc::new(std::sync::Mutex::new(ToolSteeringState::default())),
        },
        ui_state: UiState {
            dev_mode,
            window_sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
            pending_prompt_originators: Arc::new(std::sync::Mutex::new(HashMap::new())),
            last_focused_chat: Arc::new(std::sync::Mutex::new(None)),
            chat_shutdown_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            last_selection: Arc::new(std::sync::Mutex::new(None)),
            source_window: Arc::new(std::sync::Mutex::new(None)),
            frontend_ready: Arc::new(AtomicBool::new(false)),
            hotkey_registration_failures: Arc::new(std::sync::Mutex::new(Vec::new())),
        },
        child_processes: ChildProcesses {
            pocket_tts: Arc::new(std::sync::Mutex::new(None)),
            pocket_tts_install: Arc::new(std::sync::Mutex::new(None)),
        },
        feature_services: FeatureServices {
            config,
            app_launcher: Arc::new(Mutex::new(AppLauncher::new())),
            updater: Arc::new(crate::updater::UpdaterState::new()),
            user_info_cache: Arc::new(std::sync::Mutex::new(None)),
            session_cache: Arc::new(std::sync::Mutex::new(None)),
            automation_plan_cancelled: Arc::new(AtomicBool::new(false)),
            activity_tracker: Arc::new(crate::activity_tracker::ActivityTrackerState::new()),
            agent_session_registry: Arc::new(crate::agent_sessions::AgentSessionRegistry::new()),
            automation_signal_tx: Arc::new(std::sync::Mutex::new(None)),
        },
    }
}

#[derive(Debug, Clone, Default)]
pub struct PendingPermission {
    pub request_id: serde_json::Value,
    /// Session the permission belongs to, when the notification carried
    /// one. Lets dismissal target only the caller's session.
    pub session_id: Option<String>,
    /// What the policy needs to decide the request again later
    /// (`approve_permissions_if_allowed`, after the user trusts a category).
    pub request: crate::config::ToolRequest,
    /// The audit log's `args_preview` should the request be approved that
    /// way (see `notifications::auto_allow_args_preview`).
    pub args_preview: Option<String>,
}

/// Canonical map key for a pending permission: the compact JSON encoding
/// of its JSON-RPC request id (ids can be numbers or strings on the wire).
pub fn permission_key(request_id: &serde_json::Value) -> String {
    request_id.to_string()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub meta: Option<SlashCommandMeta>,
    /// How this command is executed, set by the agent layer at discovery
    /// time. `"vendor"` (default) → call the `commands/execute` vendor RPC
    /// and render the structured reply (Kiro). `"prompt"` → send the slash
    /// text as a normal `session/prompt` and let the answer stream back as
    /// an assistant message (Claude / standard ACP). The frontend routes on
    /// this so it sets up streaming UI for `prompt` commands. Defaults to
    /// `"vendor"` so existing Kiro configs and any caller that omits it keep
    /// working.
    #[serde(default = "default_dispatch")]
    pub dispatch: String,
}

fn default_dispatch() -> String {
    "vendor".to_string()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SlashCommandMeta {
    #[serde(rename = "optionsMethod")]
    pub options_method: Option<String>,
    #[serde(rename = "inputType")]
    pub input_type: Option<String>,
    pub hint: Option<String>,
    pub local: Option<bool>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AcpModel {
    #[serde(rename = "modelId")]
    pub model_id: String,
    pub name: String,
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::{ToolSteeringState, TOOLS_WITHDRAWN_BLOCK};

    // Window labels.
    const FL: &str = "floating";
    const MAIN: &str = "main";

    /// Claim and deliver successfully, returning the body sent.
    fn send(
        st: &mut ToolSteeringState,
        window: &str,
        session: &str,
        block: &str,
    ) -> Option<String> {
        let c = st.claim(window, session, block)?;
        st.confirm(session, &c);
        Some(c.body)
    }

    fn replay(st: &mut ToolSteeringState, window: &str, session: &str) -> Option<String> {
        let c = st.claim_latest(window, session)?;
        st.confirm(session, &c);
        Some(c.body)
    }

    #[test]
    fn tool_steering_is_deduplicated_per_session_not_globally() {
        // The regression: floating sent on F first, and the main window's
        // identical block for M was swallowed by a process-wide hash.
        let mut st = ToolSteeringState::default();
        assert!(send(&mut st, FL, "F", "tools").is_some());
        assert!(send(&mut st, MAIN, "M", "tools").is_some());
        assert!(st.claim(FL, "F", "tools").is_none());
        assert!(st.claim(MAIN, "M", "tools").is_none());
    }

    #[test]
    fn changed_block_is_resent() {
        let mut st = ToolSteeringState::default();
        send(&mut st, FL, "F", "v1").unwrap();
        assert_eq!(send(&mut st, FL, "F", "v2").as_deref(), Some("v2"));
    }

    #[test]
    fn released_claim_is_retried() {
        // A failed or disconnected send must not mark the session done.
        let mut st = ToolSteeringState::default();
        let c = st.claim(FL, "F", "tools").unwrap();
        st.release("F", &c);
        assert!(st.claim(FL, "F", "tools").is_some());
    }

    #[test]
    fn in_flight_claim_is_not_claimed_twice() {
        let mut st = ToolSteeringState::default();
        let _c = st.claim(FL, "F", "tools").unwrap();
        assert!(st.claim(MAIN, "F", "tools").is_none(), "already on its way");
        assert!(st.claim_latest(FL, "F").is_none());
    }

    #[test]
    fn release_leaves_a_newer_claim_alone() {
        let mut st = ToolSteeringState::default();
        let old = st.claim(FL, "F", "v1").unwrap();
        st.claim(FL, "F", "v2").unwrap();
        st.release("F", &old);
        assert!(st.claim(FL, "F", "v2").is_none());
    }

    #[test]
    fn a_newer_claim_supersedes_one_still_waiting() {
        // Two changes while the session is busy: only the newest list may
        // reach the agent, whichever delivery thread wakes first.
        let mut st = ToolSteeringState::default();
        let t1 = st.claim(FL, "F", "v1").unwrap();
        let t2 = st.claim(FL, "F", "v2").unwrap();
        assert!(!st.is_current("F", &t1), "t1 is skipped");
        assert!(st.is_current("F", &t2));
        st.confirm("F", &t2);
        assert!(st.claim_latest(FL, "F").is_none());
    }

    #[test]
    fn failed_claims_fall_back_to_what_was_delivered() {
        // Both in-flight claims fail: the session must end up with the
        // block it actually got, not an undelivered one.
        let mut st = ToolSteeringState::default();
        send(&mut st, FL, "F", "v0").unwrap();
        let t1 = st.claim(FL, "F", "v1").unwrap();
        let t2 = st.claim(FL, "F", "v2").unwrap();
        st.release("F", &t1);
        st.release("F", &t2);
        // Toggling back to v1 must still send it: F only ever had v0.
        assert_eq!(send(&mut st, FL, "F", "v1").as_deref(), Some("v1"));
    }

    #[test]
    fn cancelled_delivery_is_replayed() {
        // Stop pressed during a replayed steering prompt: it may not have
        // landed, so the next send replays it.
        let mut st = ToolSteeringState::default();
        send(&mut st, FL, "F", "v0").unwrap();
        st.claim(FL, "X", "v1").unwrap(); // F's window now publishes v1
        let c = st.claim_latest(FL, "F").unwrap();
        st.confirm("F", &c);
        st.release("F", &c);
        assert_eq!(replay(&mut st, FL, "F").as_deref(), Some("v1"));
    }

    #[test]
    fn cancelled_block_still_counts_as_having_tools() {
        // Another claim confirmed while this one waited; then Stop cancels
        // this one. The agent was told about tools either way.
        let mut st = ToolSteeringState::default();
        let old = st.claim(FL, "S", "v1").unwrap();
        let replayed = st.claim(MAIN, "S", "v2").unwrap();
        st.confirm("S", &old);
        st.confirm("S", &replayed);
        st.release("S", &replayed);
        // A later withdrawal must still go out...
        assert_eq!(
            send(&mut st, MAIN, "S", "").as_deref(),
            Some(TOOLS_WITHDRAWN_BLOCK)
        );
        // ...and with the tools back, the next prompt re-sends them.
        let mut st2 = ToolSteeringState::default();
        let c = st2.claim(MAIN, "S", "v2").unwrap();
        st2.confirm("S", &c);
        st2.release("S", &c);
        assert_eq!(replay(&mut st2, MAIN, "S").as_deref(), Some("v2"));
    }

    #[test]
    fn publishing_without_a_session_updates_that_windows_replay() {
        let mut st = ToolSteeringState::default();
        send(&mut st, MAIN, "S1", "v1").unwrap();
        st.publish(MAIN, "v2"); // main had no session when its tools changed
        assert_eq!(replay(&mut st, MAIN, "S_new").as_deref(), Some("v2"));
        assert_eq!(replay(&mut st, MAIN, "S1").as_deref(), Some("v2"));
    }

    #[test]
    fn claim_latest_replays_to_sessions_that_never_got_it() {
        let mut st = ToolSteeringState::default();
        assert!(st.claim_latest(FL, "new").is_none(), "nothing sent yet");
        send(&mut st, FL, "F", "tools").unwrap();
        assert_eq!(replay(&mut st, FL, "new").as_deref(), Some("tools"));
        assert!(st.claim_latest(FL, "new").is_none(), "claimed once");
        assert!(st.claim_latest(FL, "F").is_none(), "F already has it");
    }

    #[test]
    fn a_window_that_has_not_published_gets_the_most_recent_block() {
        let mut st = ToolSteeringState::default();
        send(&mut st, MAIN, "M", "tools").unwrap();
        assert_eq!(replay(&mut st, FL, "F").as_deref(), Some("tools"));
    }

    #[test]
    fn setting_toggled_on_resends_once_and_unchanged_does_not() {
        // Start with no tools (the setting is off): nothing to send.
        let mut st = ToolSteeringState::default();
        assert!(st.claim(FL, "F", "").is_none());
        // Setting on: the tools appear and go out once.
        assert!(send(&mut st, FL, "F", "with-stocks").is_some());
        // A later config save that leaves the tools as they were.
        assert!(st.claim(FL, "F", "with-stocks").is_none());
        // Another session picks up the current block before its next prompt.
        assert_eq!(replay(&mut st, FL, "S2").as_deref(), Some("with-stocks"));
        assert!(st.claim_latest(FL, "S2").is_none());
    }

    #[test]
    fn withdrawal_reaches_only_sessions_that_had_tools() {
        let mut st = ToolSteeringState::default();
        send(&mut st, FL, "F", "tools").unwrap();
        replay(&mut st, FL, "S2").unwrap();
        assert_eq!(
            send(&mut st, FL, "F", "").as_deref(),
            Some(TOOLS_WITHDRAWN_BLOCK)
        );
        assert!(st.claim(FL, "F", "").is_none(), "withdrawn once");
        assert_eq!(
            replay(&mut st, FL, "S2").as_deref(),
            Some(TOOLS_WITHDRAWN_BLOCK)
        );
        assert!(
            st.claim_latest(FL, "fresh").is_none(),
            "a session that never had tools needs no withdrawal"
        );
        // Tools come back: everyone who had the withdrawal gets the list.
        assert_eq!(send(&mut st, FL, "F", "tools").as_deref(), Some("tools"));
        assert_eq!(replay(&mut st, FL, "S2").as_deref(), Some("tools"));
        assert_eq!(replay(&mut st, FL, "fresh").as_deref(), Some("tools"));
    }

    #[test]
    fn one_window_losing_its_tools_does_not_withdraw_another_windows() {
        let mut st = ToolSteeringState::default();
        send(&mut st, FL, "F", "tools").unwrap();
        send(&mut st, MAIN, "M", "tools").unwrap();
        // The floating window alone ends up with no tools.
        assert_eq!(
            send(&mut st, FL, "F", "").as_deref(),
            Some(TOOLS_WITHDRAWN_BLOCK)
        );
        assert!(
            st.claim_latest(MAIN, "M").is_none(),
            "main's session keeps its tools"
        );
        // A session the main window prompts is steered with main's tools.
        assert!(st.claim_latest(MAIN, "F").is_some());
    }

    #[test]
    fn failed_withdrawal_is_retried() {
        let mut st = ToolSteeringState::default();
        send(&mut st, FL, "F", "tools").unwrap();
        let w = st.claim(FL, "F", "").unwrap();
        st.release("F", &w);
        assert_eq!(
            replay(&mut st, FL, "F").as_deref(),
            Some(TOOLS_WITHDRAWN_BLOCK)
        );
    }

    #[test]
    fn empty_block_before_any_tools_is_ignored() {
        let mut st = ToolSteeringState::default();
        assert!(st.claim(FL, "F", "  ").is_none());
        assert!(st.claim_latest(MAIN, "M").is_none());
    }

    #[test]
    fn delivery_lock_is_per_session() {
        let mut st = ToolSteeringState::default();
        let a = st.delivery_lock("A");
        assert!(std::sync::Arc::ptr_eq(&a, &st.delivery_lock("A")));
        assert!(!std::sync::Arc::ptr_eq(&a, &st.delivery_lock("B")));
    }
}
