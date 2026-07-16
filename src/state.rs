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
    /// Hash of the last sent extension tool steering (to avoid sending duplicates)
    pub last_tool_steering_hash: Arc<std::sync::Mutex<u64>>,
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
