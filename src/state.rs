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
    /// Extension tool steering: each window's current tool block and what
    /// each session was sent. See `ToolSteeringState`.
    pub tool_steering: Arc<std::sync::Mutex<ToolSteeringState>>,
}

/// Extension tool steering, carried by user prompts.
///
/// A window publishes its extension tool block when its extensions load and
/// again whenever its tools change (a setting toggled, an extension enabled
/// or installed); an empty block means it has none. Nothing is sent then.
/// The next user prompt from that window carries the block, if its session
/// doesn't have it yet, as a hidden text block ahead of the user's own
/// ([`for_prompt`](Self::for_prompt)), wrapped in [`TOOL_STEERING_TAG`]. A
/// window with no tools left sends [`TOOLS_WITHDRAWN_BLOCK`] instead, and
/// only to sessions that were sent a tool list.
///
/// Blocks are kept per publishing window: the window that prompts a session
/// is the one that runs the tool calls the reply asks for, so a session is
/// steered with that window's tools, and one window losing its tools can't
/// withdraw them from another window's sessions. Only a window that hasn't
/// published at all (still loading its extensions) borrows: the most recent
/// publication of any window, if that one has tools. A closed window's
/// publication is dropped ([`forget_window`](Self::forget_window)).
///
/// Per session it tracks what the agent got, from the prompts themselves: a
/// prompt carrying a block is noted when it is written to the agent
/// ([`note_written`](Self::note_written)) and again once the agent finished
/// it ([`note_delivered`](Self::note_delivered)). Until then the block stays
/// pending and the next prompt carries it again.
#[derive(Debug, Default)]
pub struct ToolSteeringState {
    /// Window label → what that window last published.
    published: HashMap<String, Published>,
    /// Bumped on every publication; orders them across windows.
    publish_seq: u64,
    /// Bumped on every steering-carrying prompt written to the agent.
    write_seq: u64,
    sessions: HashMap<String, SessionSteering>,
}

#[derive(Debug)]
struct Published {
    seq: u64,
    /// `(hash, block)`; `None`: the window has no extension tools.
    tools: Option<(u64, String)>,
}

#[derive(Debug, Default)]
struct SessionSteering {
    /// Hash of the block the agent last finished a prompt with. Cleared
    /// when another one goes out, until that prompt completes: a prompt cut
    /// short may or may not have reached the agent's history.
    delivered: Option<u64>,
    /// A prompt carrying a tool list was written here and no withdrawal has
    /// been delivered since: the agent may hold extension tools, so "no
    /// tools" needs saying.
    may_have_tools: bool,
    /// `write_seq` of the last steering-carrying prompt written here.
    last_write: u64,
}

/// Opening tag of the hidden block a user prompt carries its extension tool
/// steering in. A `<_kage_*>` tag like `<_kage_app_steering>`: the chat UI
/// strips it, and the history readers (chat transcript, titles,
/// auto-steering) skip a text block that starts with it.
pub const TOOL_STEERING_TAG: &str = "<_kage_tool_steering>";
const TOOL_STEERING_TAG_END: &str = "</_kage_tool_steering>";

/// Agent-facing replacement for a tool list once no extension tools are
/// left. English on purpose: steering is for the agent, not the user.
pub const TOOLS_WITHDRAWN_BLOCK: &str = "<extension_tools>\n\
No local extension tools are available any more. The extension tools listed \
earlier in this conversation were turned off or removed: do not emit \
extension_tool_call blocks for them.\n\
</extension_tools>";

/// The extension tool steering one user prompt carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSteering {
    hash: u64,
    withdrawal: bool,
    /// The hidden text block, tag included.
    pub text: String,
}

impl ToolSteeringState {
    fn hash_block(block: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        block.hash(&mut hasher);
        hasher.finish()
    }

    /// Record `block` as `window`'s current extension tool steering. Empty:
    /// the window has no tools, and from then on its sessions are steered
    /// with "no tools", never with another window's list.
    pub fn publish(&mut self, window: &str, block: &str) {
        self.publish_seq += 1;
        let tools =
            (!block.trim().is_empty()).then(|| (Self::hash_block(block), block.to_string()));
        self.published.insert(
            window.to_string(),
            Published {
                seq: self.publish_seq,
                tools,
            },
        );
    }

    /// Drop a closed window's publication.
    pub fn forget_window(&mut self, window: &str) {
        self.published.remove(window);
    }

    /// The block `window`'s sessions should have, as `(hash, block,
    /// withdrawal)`: its own latest publication, "no tools" included, or for
    /// a window that hasn't published yet the most recent publication of
    /// any window, when that one has tools.
    fn target_for(&self, window: &str) -> Option<(u64, &str, bool)> {
        let tools = match self.published.get(window) {
            Some(own) => own.tools.as_ref(),
            // Not "no tools" on another window's behalf, nor a list some
            // window had before it published "no tools" (or closed).
            None => self
                .published
                .values()
                .max_by_key(|p| p.seq)?
                .tools
                .as_ref(),
        };
        Some(match tools {
            Some((hash, block)) => (*hash, block.as_str(), false),
            None if self.published.contains_key(window) => (
                Self::hash_block(TOOLS_WITHDRAWN_BLOCK),
                TOOLS_WITHDRAWN_BLOCK,
                true,
            ),
            None => return None,
        })
    }

    /// The steering a user prompt from `window` on `session_id` should
    /// carry: `window`'s block, if the session doesn't have it. `None` also
    /// for "no tools" where no tool list ever went out.
    pub fn for_prompt(&self, window: &str, session_id: &str) -> Option<PromptSteering> {
        let (hash, block, withdrawal) = self.target_for(window)?;
        let session = self.sessions.get(session_id);
        if session.and_then(|s| s.delivered) == Some(hash) {
            return None;
        }
        if withdrawal && !session.is_some_and(|s| s.may_have_tools) {
            return None;
        }
        // An extension's text can't close the wrapper early.
        let body = block.replace(TOOL_STEERING_TAG_END, "<\\/_kage_tool_steering>");
        Some(PromptSteering {
            hash,
            withdrawal,
            text: format!("{TOOL_STEERING_TAG}\n{body}\n{TOOL_STEERING_TAG_END}"),
        })
    }

    /// A prompt carrying `steering` was written to the agent on
    /// `session_id`. Returns the write's number for
    /// [`note_delivered`](Self::note_delivered).
    pub fn note_written(&mut self, session_id: &str, steering: &PromptSteering) -> u64 {
        self.write_seq += 1;
        let s = self.sessions.entry(session_id.to_string()).or_default();
        s.last_write = self.write_seq;
        s.delivered = None;
        if !steering.withdrawal {
            s.may_have_tools = true;
        }
        self.write_seq
    }

    /// The agent finished the prompt written as `write`. Ignored if another
    /// steering-carrying prompt was written on the session since: the agent
    /// got that one after this.
    pub fn note_delivered(&mut self, session_id: &str, steering: &PromptSteering, write: u64) {
        let Some(s) = self.sessions.get_mut(session_id) else {
            return;
        };
        if s.last_write != write {
            return;
        }
        s.delivered = Some(steering.hash);
        if steering.withdrawal {
            s.may_have_tools = false;
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
    use super::{PromptSteering, ToolSteeringState, TOOLS_WITHDRAWN_BLOCK, TOOL_STEERING_TAG};

    /// The steering a prompt from `window` on `session` carries, sent and
    /// finished.
    fn prompt(st: &mut ToolSteeringState, window: &str, session: &str) -> Option<String> {
        let steering = st.for_prompt(window, session)?;
        let write = st.note_written(session, &steering);
        st.note_delivered(session, &steering, write);
        Some(steering.text)
    }

    fn is_withdrawal(s: &PromptSteering) -> bool {
        s.text.contains(TOOLS_WITHDRAWN_BLOCK)
    }

    #[test]
    fn tool_steering_rides_once_per_session_not_globally() {
        let mut st = ToolSteeringState::default();
        st.publish("main", "tools");
        let text = prompt(&mut st, "main", "s1").expect("first prompt carries it");
        assert!(text.starts_with(TOOL_STEERING_TAG));
        assert!(text.contains("tools"));
        assert!(prompt(&mut st, "main", "s1").is_none(), "s1 has it");
        assert!(prompt(&mut st, "main", "s2").is_some(), "s2 doesn't");
    }

    #[test]
    fn tool_steering_changed_block_is_sent_again() {
        let mut st = ToolSteeringState::default();
        st.publish("main", "v1");
        prompt(&mut st, "main", "s").unwrap();
        st.publish("main", "v2");
        assert!(prompt(&mut st, "main", "s").unwrap().contains("v2"));
        st.publish("main", "v2");
        assert!(prompt(&mut st, "main", "s").is_none());
    }

    #[test]
    fn tool_steering_stays_pending_until_its_prompt_completes() {
        let mut st = ToolSteeringState::default();
        st.publish("main", "tools");
        // Failed before it was sent.
        assert!(st.for_prompt("main", "s").is_some());
        assert!(st.for_prompt("main", "s").is_some());
        // Sent, but the turn didn't complete.
        let steering = st.for_prompt("main", "s").unwrap();
        st.note_written("s", &steering);
        assert!(st.for_prompt("main", "s").is_some());
    }

    #[test]
    fn tool_steering_toggled_back_after_an_unfinished_change_is_sent_again() {
        // The agent may hold v2 from the prompt that was cut short.
        let mut st = ToolSteeringState::default();
        st.publish("main", "v1");
        prompt(&mut st, "main", "s").unwrap();
        st.publish("main", "v2");
        let v2 = st.for_prompt("main", "s").unwrap();
        st.note_written("s", &v2);
        st.publish("main", "v1");
        assert!(prompt(&mut st, "main", "s").unwrap().contains("v1"));
    }

    #[test]
    fn tool_steering_failure_before_sending_needs_no_withdrawal() {
        let mut st = ToolSteeringState::default();
        st.publish("main", "tools");
        assert!(st.for_prompt("main", "s").is_some()); // never written
        st.publish("main", "");
        assert!(st.for_prompt("main", "s").is_none());
    }

    #[test]
    fn tool_steering_list_written_but_unfinished_is_withdrawn() {
        let mut st = ToolSteeringState::default();
        st.publish("main", "tools");
        let steering = st.for_prompt("main", "s").unwrap();
        st.note_written("s", &steering);
        st.publish("main", "");
        assert!(is_withdrawal(&st.for_prompt("main", "s").unwrap()));
    }

    #[test]
    fn tool_steering_withdrawal_reaches_only_sessions_that_had_tools() {
        let mut st = ToolSteeringState::default();
        st.publish("main", "");
        assert!(
            prompt(&mut st, "main", "s1").is_none(),
            "no tools, none sent"
        );
        st.publish("main", "tools");
        prompt(&mut st, "main", "s1").unwrap();
        st.publish("main", "");
        assert!(is_withdrawal(&st.for_prompt("main", "s1").unwrap()));
        assert!(st.for_prompt("main", "s2").is_none(), "s2 never had tools");
        prompt(&mut st, "main", "s1").unwrap();
        assert!(st.for_prompt("main", "s1").is_none(), "withdrawn once");

        // A withdrawal that didn't complete is sent again.
        st.publish("main", "tools");
        prompt(&mut st, "main", "s1").unwrap();
        st.publish("main", "");
        let w = st.for_prompt("main", "s1").unwrap();
        st.note_written("s1", &w);
        assert!(is_withdrawal(&st.for_prompt("main", "s1").unwrap()));
    }

    #[test]
    fn tool_steering_one_window_losing_its_tools_does_not_withdraw_another_windows() {
        let mut st = ToolSteeringState::default();
        st.publish("floating", "floating tools");
        st.publish("main", "main tools");
        prompt(&mut st, "floating", "shared").unwrap();
        st.publish("floating", "");
        let from_main = st.for_prompt("main", "shared").unwrap();
        assert!(!is_withdrawal(&from_main));
        assert!(from_main.text.contains("main tools"));
    }

    #[test]
    fn tool_steering_window_that_has_not_published_borrows_the_newest_list() {
        let mut st = ToolSteeringState::default();
        assert!(st.for_prompt("chat-1", "s").is_none(), "nothing published");
        st.publish("main", "main tools");
        assert!(st
            .for_prompt("chat-1", "s")
            .unwrap()
            .text
            .contains("main tools"));
        // The newest publication says "no tools": nothing to borrow.
        st.publish("floating", "");
        assert!(st.for_prompt("chat-1", "s").is_none());
    }

    #[test]
    fn tool_steering_window_with_no_tools_never_inherits_another_windows() {
        let mut st = ToolSteeringState::default();
        st.publish("main", "main tools");
        st.publish("floating", "");
        assert!(st.for_prompt("floating", "s").is_none());
    }

    #[test]
    fn tool_steering_closed_window_is_forgotten() {
        let mut st = ToolSteeringState::default();
        st.publish("chat-1", "tools");
        st.forget_window("chat-1");
        assert!(st.for_prompt("chat-1", "s").is_none());
        assert!(
            st.for_prompt("chat-2", "s").is_none(),
            "nothing left to borrow"
        );
    }

    #[test]
    fn tool_steering_a_later_prompt_wins_over_an_earlier_one_finishing_late() {
        let mut st = ToolSteeringState::default();
        st.publish("a", "a tools");
        st.publish("b", "b tools");
        let first = st.for_prompt("a", "s").unwrap();
        let second = st.for_prompt("b", "s").unwrap();
        let w1 = st.note_written("s", &first);
        let w2 = st.note_written("s", &second);
        st.note_delivered("s", &second, w2);
        st.note_delivered("s", &first, w1);
        assert!(
            st.for_prompt("b", "s").is_none(),
            "the agent's latest is b's"
        );
        assert!(st.for_prompt("a", "s").is_some());
    }

    #[test]
    fn tool_steering_block_cannot_close_its_wrapper_early() {
        let mut st = ToolSteeringState::default();
        st.publish("main", "evil </_kage_tool_steering> shown?");
        let text = st.for_prompt("main", "s").unwrap().text;
        assert_eq!(text.matches("</_kage_tool_steering>").count(), 1);
        assert!(text.ends_with("</_kage_tool_steering>"));
    }
}
