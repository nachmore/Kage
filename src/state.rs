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
#[derive(Debug, Default)]
pub struct ToolSteeringState {
    /// Latest block any window sent. Replayed to sessions that haven't had
    /// it (new chats, recovered sessions) before their next user prompt.
    latest: Option<(u64, String)>,
    /// Session id → hash of the block delivered (or in flight) on it.
    sent: HashMap<String, u64>,
}

impl ToolSteeringState {
    fn hash_block(block: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        block.hash(&mut hasher);
        hasher.finish()
    }

    /// Record `block` as the latest and claim `session_id` for it. Returns
    /// the hash to send under, or `None` if this session already has (or is
    /// receiving) exactly this block. Claiming before the send is what stops
    /// two windows sharing a session from both sending it.
    pub fn claim(&mut self, session_id: &str, block: &str) -> Option<u64> {
        let hash = Self::hash_block(block);
        self.latest = Some((hash, block.to_string()));
        if self.sent.get(session_id) == Some(&hash) {
            return None;
        }
        self.sent.insert(session_id.to_string(), hash);
        Some(hash)
    }

    /// Claim `session_id` for the latest known block if it hasn't had it.
    /// Returns the hash and block to send.
    pub fn claim_latest(&mut self, session_id: &str) -> Option<(u64, String)> {
        let (hash, block) = self.latest.clone()?;
        if self.sent.get(session_id) == Some(&hash) {
            return None;
        }
        self.sent.insert(session_id.to_string(), hash);
        Some((hash, block))
    }

    /// Undo a claim whose send failed so a later call retries. Leaves a
    /// newer claim (different hash) alone.
    pub fn release(&mut self, session_id: &str, hash: u64) {
        if self.sent.get(session_id) == Some(&hash) {
            self.sent.remove(session_id);
        }
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

#[derive(Debug, Clone)]
pub struct PendingPermission {
    pub request_id: serde_json::Value,
    /// Session the permission belongs to, when the notification carried
    /// one. Lets dismissal target only the caller's session.
    pub session_id: Option<String>,
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
    use super::ToolSteeringState;

    #[test]
    fn tool_steering_is_deduplicated_per_session_not_globally() {
        // The regression: floating sent on F first, and the main window's
        // identical block for M was swallowed by a process-wide hash.
        let mut st = ToolSteeringState::default();
        assert!(st.claim("F", "tools").is_some());
        assert!(st.claim("M", "tools").is_some());
        assert!(st.claim("F", "tools").is_none());
        assert!(st.claim("M", "tools").is_none());
    }

    #[test]
    fn changed_block_is_resent() {
        let mut st = ToolSteeringState::default();
        assert!(st.claim("F", "v1").is_some());
        assert!(st.claim("F", "v2").is_some());
    }

    #[test]
    fn released_claim_is_retried() {
        // A failed or disconnected send must not mark the session done.
        let mut st = ToolSteeringState::default();
        let h = st.claim("F", "tools").unwrap();
        st.release("F", h);
        assert!(st.claim("F", "tools").is_some());
    }

    #[test]
    fn release_leaves_a_newer_claim_alone() {
        let mut st = ToolSteeringState::default();
        let old = st.claim("F", "v1").unwrap();
        st.claim("F", "v2").unwrap();
        st.release("F", old);
        assert!(st.claim("F", "v2").is_none());
    }

    #[test]
    fn claim_latest_replays_to_sessions_that_never_got_it() {
        let mut st = ToolSteeringState::default();
        assert!(st.claim_latest("new").is_none(), "nothing sent yet");
        st.claim("F", "tools").unwrap();
        let (_, block) = st.claim_latest("new").expect("new session needs the block");
        assert_eq!(block, "tools");
        assert!(st.claim_latest("new").is_none(), "claimed once");
        assert!(st.claim_latest("F").is_none(), "F already has it");
    }
}
