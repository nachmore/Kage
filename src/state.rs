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
    /// avoid duplicates, per session), and the per-session delivery gate.
    /// See `ToolSteering`.
    pub tool_steering: Arc<ToolSteering>,
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
/// has exactly the current block is sent nothing. A window publishes an
/// empty block when it has no tools, from its first publication on; for
/// that window's sessions it becomes [`TOOLS_WITHDRAWN_BLOCK`], sent only
/// where a tool list was actually transmitted before.
///
/// Blocks are kept per publishing window: the window that prompts a session
/// is the one that runs the tool calls the reply asks for, so a session is
/// steered with that window's tools, and one window losing its tools can't
/// withdraw them from another window's sessions. Only a window that hasn't
/// published at all (still loading its extensions) borrows: the most recent
/// publication of any window, if that one has tools.
///
/// What a session was sent is tracked from transmissions, not claims: a
/// claim only queues a block, [`begin_transmit`](Self::begin_transmit)
/// records that it is going to the agent, and [`settle`](Self::settle)
/// records how that ended ([`SteeringOutcome`]).
///
/// A user prompt about to go out on a session claims through
/// [`claim_for_prompt`](Self::claim_for_prompt) instead: it takes over a
/// block that is only queued and sends it itself, ahead of the prompt, and
/// while it does ([`ToolSteering::begin_prompt`]) other deliveries on the
/// session step aside.
#[derive(Debug, Default)]
pub struct ToolSteeringState {
    /// Window label → what that window last published.
    published: HashMap<String, Published>,
    /// Bumped on every publication; orders them across windows.
    publish_seq: u64,
    /// Bumped on every claim; tells claims of the same block apart.
    claim_seq: u64,
    sessions: HashMap<String, SessionSteering>,
}

#[derive(Debug, Clone)]
struct Published {
    seq: u64,
    /// `None`: the window has no extension tools.
    tools: Option<SteeringBlock>,
}

#[derive(Debug, Clone)]
struct SteeringBlock {
    hash: u64,
    body: String,
}

/// What the agent is known to hold on a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delivered {
    /// This block: its steering turn completed.
    Known(u64),
    /// Some block whose steering turn didn't complete after it was written
    /// (Stop, its deadline, a lost connection): the agent may or may not
    /// have kept it. Matches no block, so it is re-sent.
    Unknown,
}

#[derive(Debug, Default)]
struct SessionSteering {
    delivered: Option<Delivered>,
    /// The agent may believe it has extension tools: a tool list was
    /// transmitted here (completed or not) and no withdrawal has completed
    /// since. Decides whether "no tools" needs saying at all.
    may_have_tools: bool,
    /// Newest claim not yet settled: `(hash, claim id)`.
    in_flight: Option<(u64, u64)>,
    /// Claim id being transmitted right now, with `may_have_tools` from
    /// before it (restored if the write never happened).
    transmitting: Option<(u64, bool)>,
    /// Blocks whose full-length steering turn ran out of time here since
    /// the last one that completed. The agent got them but didn't finish
    /// in time, so a user prompt isn't made to wait on another try of the
    /// same block (a change, or a new background delivery, still sends
    /// one). A set, so windows taking turns with different blocks on one
    /// session don't each retry theirs.
    timed_out: std::collections::HashSet<u64>,
    /// A delivery holds the session (see [`ToolSteering::begin_delivery`]).
    delivering: bool,
    /// User prompts steering this session right now (see
    /// [`ToolSteering::begin_prompt`]).
    prompting: u32,
}

/// Agent-facing replacement for a tool list once no extension tools are
/// left. English on purpose: steering is for the agent, not the user.
pub const TOOLS_WITHDRAWN_BLOCK: &str = "<extension_tools>\n\
No local extension tools are available any more. The extension tools listed \
earlier in this conversation were turned off or removed: do not emit \
extension_tool_call blocks for them.\n\
</extension_tools>";

/// One session claimed for one steering delivery. Hand it to
/// [`ToolSteeringState::begin_transmit`] right before it is written to the
/// agent and to [`ToolSteeringState::settle`] once that's over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteeringClaim {
    pub hash: u64,
    /// Tells this claim apart from other claims of the same block.
    pub id: u64,
    /// The steering text to send.
    pub body: String,
    /// The tools-withdrawn notice rather than a tool list.
    pub withdrawal: bool,
    /// Claimed by a user prompt for itself ([`ToolSteeringState::claim_for_prompt`]).
    pub for_prompt: bool,
}

/// How a claimed delivery ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteeringOutcome {
    /// Never reached the agent, or the agent turned it down: superseded,
    /// stepped aside for a user prompt, no connection, the write failed, or
    /// an error reply.
    NotSent,
    /// The agent got it and finished the steering turn.
    Delivered,
    /// Written to the agent, but the turn was cut short (Stop, a dropped
    /// connection): whether the agent kept it is unknown, so it counts as
    /// sent (a later withdrawal still applies) and is re-sent.
    Unknown,
    /// Written, then cancelled at its deadline: like [`Unknown`](Self::Unknown),
    /// except that a user prompt doesn't wait on a retry of the same block.
    TimedOut,
}

/// What a user prompt should do about steering before it goes out (see
/// [`ToolSteeringState::claim_for_prompt`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptSteering {
    /// Nothing to send first.
    Ready,
    /// A steering turn is on the wire: wait for it to end.
    Transmitting,
    /// Send this first.
    Send(SteeringClaim),
}

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
    /// this block, or if it's a withdrawal for a session that was never
    /// sent tools. Claiming before the send is what stops two windows
    /// sharing a session from both sending it.
    pub fn claim(&mut self, window: &str, session_id: &str, block: &str) -> Option<SteeringClaim> {
        self.publish(window, block);
        self.claim_latest(window, session_id)
    }

    /// Record `block` as `window`'s current steering without claiming a
    /// session (the window has none right now). Its sessions get it replayed
    /// before their next prompt. An empty block is recorded too: from then
    /// on the window's sessions are steered with "no tools", never with
    /// another window's list.
    pub fn publish(&mut self, window: &str, block: &str) {
        self.publish_seq += 1;
        let tools = (!block.trim().is_empty()).then(|| SteeringBlock {
            hash: Self::hash_block(block),
            body: block.to_string(),
        });
        self.published.insert(
            window.to_string(),
            Published {
                seq: self.publish_seq,
                tools,
            },
        );
    }

    /// The block `window`'s sessions should be steered with, as `(hash,
    /// body, withdrawal)`: its own latest publication, "no tools" included,
    /// or for a window that hasn't published yet the most recent
    /// publication of any window, when that one has tools. `None`: nothing
    /// to send.
    fn target_for(&self, window: &str) -> Option<(u64, String, bool)> {
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
            Some(block) => (block.hash, block.body.clone(), false),
            None if self.published.contains_key(window) => (
                Self::hash_block(TOOLS_WITHDRAWN_BLOCK),
                TOOLS_WITHDRAWN_BLOCK.to_string(),
                true,
            ),
            None => return None,
        })
    }

    fn next_claim(
        &mut self,
        hash: u64,
        body: String,
        withdrawal: bool,
        for_prompt: bool,
    ) -> SteeringClaim {
        self.claim_seq += 1;
        SteeringClaim {
            hash,
            id: self.claim_seq,
            body,
            withdrawal,
            for_prompt,
        }
    }

    /// Claim `session_id` for the steering `window` should give it (see
    /// [`target_for`](Self::target_for)) if the session doesn't have it.
    pub fn claim_latest(&mut self, window: &str, session_id: &str) -> Option<SteeringClaim> {
        let (hash, body, withdrawal) = self.target_for(window)?;
        let s = self.sessions.entry(session_id.to_string()).or_default();
        if withdrawal && !s.may_have_tools {
            // No tool list ever went out here (at most one is queued, and
            // nothing is being transmitted): nothing to withdraw, and the
            // queued list must not go out either.
            s.in_flight = None;
            return None;
        }
        if s.delivered == Some(Delivered::Known(hash)) && s.transmitting.is_none() {
            // The agent has exactly this; a queued replacement is moot.
            s.in_flight = None;
            return None;
        }
        if s.in_flight.is_some_and(|(h, _)| h == hash) {
            return None;
        }
        let claim = self.next_claim(hash, body, withdrawal, false);
        let s = self.sessions.entry(session_id.to_string()).or_default();
        s.in_flight = Some((hash, claim.id));
        Some(claim)
    }

    /// What a user prompt from `window` should do about steering on
    /// `session_id` before it goes out. A block only queued (not yet on the
    /// wire) is taken over: the prompt sends it itself, so it never waits
    /// on a delivery that hasn't started. A block whose last turn here ran
    /// out of time isn't sent again for a prompt (see
    /// [`SteeringOutcome::TimedOut`]).
    pub fn claim_for_prompt(&mut self, window: &str, session_id: &str) -> PromptSteering {
        let target = self.target_for(window);
        let s = self.sessions.entry(session_id.to_string()).or_default();
        if s.transmitting.is_some() {
            return PromptSteering::Transmitting;
        }
        let Some((hash, body, withdrawal)) = target else {
            return PromptSteering::Ready;
        };
        let has_it = s.delivered == Some(Delivered::Known(hash));
        let timed_out = s.delivered == Some(Delivered::Unknown) && s.timed_out.contains(&hash);
        if has_it || timed_out || (withdrawal && !s.may_have_tools) {
            // Whatever is queued is moot, or would only hold the prompt.
            s.in_flight = None;
            return PromptSteering::Ready;
        }
        let claim = self.next_claim(hash, body, withdrawal, true);
        let s = self.sessions.entry(session_id.to_string()).or_default();
        s.in_flight = Some((hash, claim.id));
        PromptSteering::Send(claim)
    }

    /// Drop `claim` if it is still queued: the prompt that claimed it goes
    /// out without it (it is claimed again before the next one).
    pub fn abandon(&mut self, session_id: &str, claim: &SteeringClaim) {
        if let Some(s) = self.sessions.get_mut(session_id) {
            if s.in_flight == Some((claim.hash, claim.id)) {
                s.in_flight = None;
            }
        }
    }

    /// Whether `claim` should still go out: it is the session's newest
    /// claim, and no user prompt is steering the session (unless it is that
    /// prompt's own).
    pub fn is_current(&self, session_id: &str, claim: &SteeringClaim) -> bool {
        self.sessions.get(session_id).is_some_and(|s| {
            s.in_flight == Some((claim.hash, claim.id)) && (claim.for_prompt || s.prompting == 0)
        })
    }

    /// Whether a claimed block is still on its way on `session_id`.
    #[cfg(test)]
    pub fn has_in_flight(&self, session_id: &str) -> bool {
        self.sessions
            .get(session_id)
            .is_some_and(|s| s.in_flight.is_some())
    }

    /// `claim` is about to be written to the agent (the session's prompt
    /// slot is held). Returns false if it shouldn't go out after all
    /// ([`is_current`](Self::is_current), or a withdrawal for a session
    /// whose only tool list never reached the agent). From here on the
    /// agent may have it: a tool list makes the session one a withdrawal
    /// applies to.
    pub fn begin_transmit(&mut self, session_id: &str, claim: &SteeringClaim) -> bool {
        if !self.is_current(session_id, claim) {
            return false;
        }
        let s = self.sessions.entry(session_id.to_string()).or_default();
        if claim.withdrawal && !s.may_have_tools {
            s.in_flight = None;
            return false;
        }
        s.transmitting = Some((claim.id, s.may_have_tools));
        if !claim.withdrawal {
            s.may_have_tools = true;
        }
        true
    }

    /// Record how `claim`'s delivery ended. Leaves a newer claim queued.
    pub fn settle(&mut self, session_id: &str, claim: &SteeringClaim, outcome: SteeringOutcome) {
        let s = self.sessions.entry(session_id.to_string()).or_default();
        let before_transmit = match s.transmitting {
            Some((id, before)) if id == claim.id => {
                s.transmitting = None;
                Some(before)
            }
            _ => None,
        };
        if s.in_flight == Some((claim.hash, claim.id)) {
            s.in_flight = None;
        }
        match outcome {
            SteeringOutcome::NotSent => {
                if let Some(before) = before_transmit {
                    s.may_have_tools = before;
                }
            }
            SteeringOutcome::Delivered => {
                s.delivered = Some(Delivered::Known(claim.hash));
                s.may_have_tools = !claim.withdrawal;
                s.timed_out.clear();
            }
            SteeringOutcome::Unknown | SteeringOutcome::TimedOut => {
                s.delivered = Some(Delivered::Unknown);
                if !claim.withdrawal {
                    s.may_have_tools = true;
                }
                if outcome == SteeringOutcome::TimedOut {
                    s.timed_out.insert(claim.hash);
                } else {
                    s.timed_out.remove(&claim.hash);
                }
            }
        }
    }

    fn is_transmitting(&self, session_id: &str) -> bool {
        self.sessions
            .get(session_id)
            .is_some_and(|s| s.transmitting.is_some())
    }
}

/// [`ToolSteeringState`] plus the waits around it: one delivery at a time
/// per session, in claim order, and the bookkeeping that lets a user prompt
/// take steering over.
#[derive(Debug, Default)]
pub struct ToolSteering {
    state: std::sync::Mutex<ToolSteeringState>,
    /// Signalled when a delivery ends or a claim settles.
    changed: std::sync::Condvar,
}

/// Longest single wait on [`ToolSteering::changed`], so a caller's `stopped`
/// check runs that often.
const STEERING_WAIT_SLICE: std::time::Duration = std::time::Duration::from_millis(25);

impl ToolSteering {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, ToolSteeringState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// [`ToolSteeringState::settle`], waking anyone waiting on the session.
    pub fn settle(&self, session_id: &str, claim: &SteeringClaim, outcome: SteeringOutcome) {
        self.lock().settle(session_id, claim, outcome);
        self.changed.notify_all();
    }

    /// Take `session_id`'s delivery slot, waiting for a delivery already
    /// holding it. Every delivery is bounded (a steering turn has an
    /// absolute deadline; a queued one gives up as soon as it is superseded
    /// or a user prompt takes over), so this wait is too. Blocking.
    pub fn begin_delivery(&self, session_id: &str) -> DeliveryGuard<'_> {
        let mut st = self.lock();
        loop {
            let s = st.sessions.entry(session_id.to_string()).or_default();
            if !s.delivering {
                s.delivering = true;
                return DeliveryGuard {
                    owner: self,
                    session_id: session_id.to_string(),
                };
            }
            st = self.wait(st, STEERING_WAIT_SLICE);
        }
    }

    /// Mark a user prompt as steering `session_id` until the guard drops:
    /// other deliveries there step aside (unsent) rather than start a turn,
    /// so everything the prompt waits on stays within its own deadline.
    pub fn begin_prompt(&self, session_id: &str) -> PromptGuard<'_> {
        self.lock()
            .sessions
            .entry(session_id.to_string())
            .or_default()
            .prompting += 1;
        PromptGuard {
            owner: self,
            session_id: session_id.to_string(),
        }
    }

    /// Wait while a steering turn is on the wire on `session_id`, until
    /// `deadline` or until `stopped` returns true. Returns whether it ended
    /// (false: gave up). Blocking.
    pub fn wait_while_transmitting(
        &self,
        session_id: &str,
        deadline: std::time::Instant,
        stopped: &dyn Fn() -> bool,
    ) -> bool {
        let mut st = self.lock();
        while st.is_transmitting(session_id) {
            let now = std::time::Instant::now();
            if now >= deadline || stopped() {
                return false;
            }
            st = self.wait(st, (deadline - now).min(STEERING_WAIT_SLICE));
        }
        true
    }

    fn wait<'a>(
        &self,
        guard: std::sync::MutexGuard<'a, ToolSteeringState>,
        timeout: std::time::Duration,
    ) -> std::sync::MutexGuard<'a, ToolSteeringState> {
        match self.changed.wait_timeout(guard, timeout) {
            Ok((guard, _)) => guard,
            Err(poisoned) => poisoned.into_inner().0,
        }
    }
}

/// A session's delivery slot, held for one delivery. Released on drop.
pub struct DeliveryGuard<'a> {
    owner: &'a ToolSteering,
    session_id: String,
}

impl Drop for DeliveryGuard<'_> {
    fn drop(&mut self) {
        if let Some(s) = self.owner.lock().sessions.get_mut(&self.session_id) {
            s.delivering = false;
        }
        self.owner.changed.notify_all();
    }
}

/// See [`ToolSteering::begin_prompt`].
pub struct PromptGuard<'a> {
    owner: &'a ToolSteering,
    session_id: String,
}

impl Drop for PromptGuard<'_> {
    fn drop(&mut self) {
        if let Some(s) = self.owner.lock().sessions.get_mut(&self.session_id) {
            s.prompting = s.prompting.saturating_sub(1);
        }
        self.owner.changed.notify_all();
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
            tool_steering: Arc::new(ToolSteering::default()),
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
    use super::{
        PromptSteering, SteeringClaim, SteeringOutcome, ToolSteering, ToolSteeringState,
        TOOLS_WITHDRAWN_BLOCK,
    };
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    // Window labels.
    const FL: &str = "floating";
    const MAIN: &str = "main";

    /// Transmit a claim and settle it with `outcome`.
    fn finish(
        st: &mut ToolSteeringState,
        session: &str,
        c: &SteeringClaim,
        outcome: SteeringOutcome,
    ) {
        assert!(st.begin_transmit(session, c), "claim was superseded");
        st.settle(session, c, outcome);
    }

    /// Claim and deliver successfully, returning the body sent.
    fn send(
        st: &mut ToolSteeringState,
        window: &str,
        session: &str,
        block: &str,
    ) -> Option<String> {
        let c = st.claim(window, session, block)?;
        finish(st, session, &c, SteeringOutcome::Delivered);
        Some(c.body)
    }

    fn replay(st: &mut ToolSteeringState, window: &str, session: &str) -> Option<String> {
        let c = st.claim_latest(window, session)?;
        finish(st, session, &c, SteeringOutcome::Delivered);
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
    fn tool_steering_changed_block_is_resent() {
        let mut st = ToolSteeringState::default();
        send(&mut st, FL, "F", "v1").unwrap();
        assert_eq!(send(&mut st, FL, "F", "v2").as_deref(), Some("v2"));
    }

    #[test]
    fn tool_steering_unsent_claim_is_retried() {
        // A failed or disconnected send must not mark the session done.
        let mut st = ToolSteeringState::default();
        let c = st.claim(FL, "F", "tools").unwrap();
        st.settle("F", &c, SteeringOutcome::NotSent);
        assert!(st.claim(FL, "F", "tools").is_some());
    }

    #[test]
    fn tool_steering_in_flight_claim_is_not_claimed_twice() {
        let mut st = ToolSteeringState::default();
        let _c = st.claim(FL, "F", "tools").unwrap();
        st.publish(MAIN, "tools");
        assert!(st.claim(MAIN, "F", "tools").is_none(), "already on its way");
        assert!(st.claim_latest(FL, "F").is_none());
    }

    #[test]
    fn tool_steering_settle_leaves_a_newer_claim_alone() {
        let mut st = ToolSteeringState::default();
        let old = st.claim(FL, "F", "v1").unwrap();
        st.claim(FL, "F", "v2").unwrap();
        st.settle("F", &old, SteeringOutcome::NotSent);
        assert!(st.claim(FL, "F", "v2").is_none());
        assert!(st.has_in_flight("F"));
    }

    #[test]
    fn tool_steering_newer_claim_supersedes_one_still_waiting() {
        // Two changes while the session is busy: only the newest list may
        // reach the agent, whichever delivery thread wakes first.
        let mut st = ToolSteeringState::default();
        let t1 = st.claim(FL, "F", "v1").unwrap();
        let t2 = st.claim(FL, "F", "v2").unwrap();
        assert!(!st.is_current("F", &t1), "t1 is skipped");
        assert!(
            !st.begin_transmit("F", &t1),
            "nor sent once it has the slot"
        );
        finish(&mut st, "F", &t2, SteeringOutcome::Delivered);
        assert!(st.claim_latest(FL, "F").is_none());
    }

    #[test]
    fn tool_steering_failed_claims_fall_back_to_what_was_delivered() {
        // Both in-flight claims fail: the session must end up with the
        // block it actually got, not an undelivered one.
        let mut st = ToolSteeringState::default();
        send(&mut st, FL, "F", "v0").unwrap();
        let t1 = st.claim(FL, "F", "v1").unwrap();
        let t2 = st.claim(FL, "F", "v2").unwrap();
        st.settle("F", &t1, SteeringOutcome::NotSent);
        finish(&mut st, "F", &t2, SteeringOutcome::NotSent);
        // Toggling back to v1 must still send it: F only ever had v0.
        assert_eq!(send(&mut st, FL, "F", "v1").as_deref(), Some("v1"));
    }

    #[test]
    fn tool_steering_toggled_back_to_what_was_delivered_drops_the_queued_change() {
        let mut st = ToolSteeringState::default();
        send(&mut st, FL, "F", "v1").unwrap();
        let queued = st.claim(FL, "F", "v2").unwrap();
        assert!(st.claim(FL, "F", "v1").is_none(), "the agent has v1");
        assert!(!st.begin_transmit("F", &queued), "v2 must not go out");
        assert!(!st.has_in_flight("F"));
    }

    #[test]
    fn tool_steering_toggled_back_while_the_change_is_sending_resends() {
        let mut st = ToolSteeringState::default();
        send(&mut st, FL, "F", "v1").unwrap();
        let sending = st.claim(FL, "F", "v2").unwrap();
        assert!(st.begin_transmit("F", &sending));
        // The agent will hold v2 once that lands, so v1 has to follow it.
        let back = st.claim(FL, "F", "v1").expect("v1 re-sent after v2");
        st.settle("F", &sending, SteeringOutcome::Delivered);
        finish(&mut st, "F", &back, SteeringOutcome::Delivered);
        assert!(st.claim_latest(FL, "F").is_none());
    }

    #[test]
    fn tool_steering_cut_short_delivery_is_replayed() {
        // Stop pressed during a steering turn, or its deadline passed: it
        // may not have landed, so the next prompt replays it.
        let mut st = ToolSteeringState::default();
        let c = st.claim(FL, "F", "v1").unwrap();
        finish(&mut st, "F", &c, SteeringOutcome::Unknown);
        assert_eq!(replay(&mut st, FL, "F").as_deref(), Some("v1"));
        assert!(st.claim_latest(FL, "F").is_none());
    }

    #[test]
    fn tool_steering_cut_short_first_delivery_still_gets_a_withdrawal() {
        // The regression: cancelling the session's first delivery dropped
        // its claim without recording that the agent may have it, so
        // turning the tools off later withdrew nothing.
        let mut st = ToolSteeringState::default();
        let c = st.claim(FL, "S", "tools").unwrap();
        finish(&mut st, "S", &c, SteeringOutcome::Unknown);
        assert_eq!(
            send(&mut st, FL, "S", "").as_deref(),
            Some(TOOLS_WITHDRAWN_BLOCK)
        );
    }

    #[test]
    fn tool_steering_failure_before_sending_needs_no_withdrawal() {
        let mut st = ToolSteeringState::default();
        // Never got as far as the write (no connection, superseded).
        let c = st.claim(FL, "A", "tools").unwrap();
        st.settle("A", &c, SteeringOutcome::NotSent);
        assert!(st.claim(FL, "A", "").is_none());
        // The write itself failed: the transmission didn't start either.
        let c = st.claim(FL, "B", "tools").unwrap();
        finish(&mut st, "B", &c, SteeringOutcome::NotSent);
        assert!(st.claim(FL, "B", "").is_none());
    }

    #[test]
    fn tool_steering_tools_withdrawn_before_they_were_sent_go_nowhere() {
        // The regression: tools claimed, then an empty block before the
        // delivery started. The withdrawal superseded the queued list and
        // became the session's first steering turn.
        let mut st = ToolSteeringState::default();
        let queued = st.claim(FL, "S", "tools").unwrap();
        assert!(st.claim(FL, "S", "").is_none(), "nothing to withdraw");
        assert!(!st.begin_transmit("S", &queued), "the list is dropped too");
        assert!(!st.has_in_flight("S"));
        assert!(st.claim_latest(FL, "S").is_none());
    }

    #[test]
    fn tool_steering_tools_withdrawn_while_being_sent_are_withdrawn() {
        let mut st = ToolSteeringState::default();
        let sending = st.claim(FL, "S", "tools").unwrap();
        assert!(st.begin_transmit("S", &sending));
        let w = st.claim(FL, "S", "").expect("the agent may get the list");
        assert!(w.withdrawal);
        st.settle("S", &sending, SteeringOutcome::Delivered);
        finish(&mut st, "S", &w, SteeringOutcome::Delivered);
        assert!(st.claim_latest(FL, "S").is_none());
    }

    #[test]
    fn tool_steering_publishing_without_a_session_updates_that_windows_replay() {
        let mut st = ToolSteeringState::default();
        send(&mut st, MAIN, "S1", "v1").unwrap();
        st.publish(MAIN, "v2"); // main had no session when its tools changed
        assert_eq!(replay(&mut st, MAIN, "S_new").as_deref(), Some("v2"));
        assert_eq!(replay(&mut st, MAIN, "S1").as_deref(), Some("v2"));
    }

    #[test]
    fn tool_steering_replays_to_sessions_that_never_got_it() {
        let mut st = ToolSteeringState::default();
        assert!(st.claim_latest(FL, "new").is_none(), "nothing sent yet");
        send(&mut st, FL, "F", "tools").unwrap();
        assert_eq!(replay(&mut st, FL, "new").as_deref(), Some("tools"));
        assert!(st.claim_latest(FL, "new").is_none(), "claimed once");
        assert!(st.claim_latest(FL, "F").is_none(), "F already has it");
    }

    #[test]
    fn tool_steering_window_that_has_not_published_borrows_the_newest_list() {
        let mut st = ToolSteeringState::default();
        send(&mut st, MAIN, "M", "old").unwrap();
        send(&mut st, FL, "F", "new").unwrap();
        assert_eq!(replay(&mut st, "inline", "I").as_deref(), Some("new"));
        // The newest publication says "no tools": nothing is borrowed, not
        // even an older window's list (it may be stale, its window closed),
        // and no withdrawal goes out on another window's behalf.
        st.publish(FL, "");
        assert!(st.claim_latest("inline", "J").is_none());
        assert!(st.claim_latest("inline", "I").is_none());
    }

    #[test]
    fn tool_steering_window_with_no_tools_never_inherits_another_windows() {
        // The regression: main had no tools, so its first publication was
        // dropped, and claim_latest(main, …) then served floating's tools.
        let mut st = ToolSteeringState::default();
        send(&mut st, FL, "F", "tools").unwrap();
        assert!(st.claim(MAIN, "M", "").is_none(), "M was never sent tools");
        assert!(st.claim_latest(MAIN, "M").is_none());
        assert!(st.claim_latest(MAIN, "M2").is_none());
        // A session that has floating's tools and is now prompted from
        // main (which can't run them) is told there are none.
        assert_eq!(
            replay(&mut st, MAIN, "F").as_deref(),
            Some(TOOLS_WITHDRAWN_BLOCK)
        );
        // Floating's own sessions still get its list.
        assert_eq!(replay(&mut st, FL, "F").as_deref(), Some("tools"));
    }

    #[test]
    fn tool_steering_setting_toggled_on_resends_once_and_unchanged_does_not() {
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
    fn tool_steering_withdrawal_reaches_only_sessions_that_had_tools() {
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
    fn tool_steering_one_window_losing_its_tools_does_not_withdraw_another_windows() {
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
    fn tool_steering_failed_withdrawal_is_retried() {
        let mut st = ToolSteeringState::default();
        send(&mut st, FL, "F", "tools").unwrap();
        let w = st.claim(FL, "F", "").unwrap();
        finish(&mut st, "F", &w, SteeringOutcome::NotSent);
        assert_eq!(
            replay(&mut st, FL, "F").as_deref(),
            Some(TOOLS_WITHDRAWN_BLOCK)
        );
        // A withdrawal cut short is re-sent too, and the session still
        // counts as one that may have tools until one completes.
        send(&mut st, FL, "F", "tools").unwrap();
        let w = st.claim(FL, "F", "").unwrap();
        finish(&mut st, "F", &w, SteeringOutcome::Unknown);
        assert_eq!(
            replay(&mut st, FL, "F").as_deref(),
            Some(TOOLS_WITHDRAWN_BLOCK)
        );
    }

    #[test]
    fn tool_steering_delivery_slot_is_per_session_and_serialised() {
        let steering = Arc::new(ToolSteering::default());
        let a = steering.begin_delivery("A");
        let _b = steering.begin_delivery("B"); // another session: no wait
        let (st, order) = (
            steering.clone(),
            Arc::new(std::sync::Mutex::new(Vec::new())),
        );
        let o = order.clone();
        let second = std::thread::spawn(move || {
            let _a2 = st.begin_delivery("A");
            o.lock().unwrap().push("second");
        });
        std::thread::sleep(Duration::from_millis(50));
        order.lock().unwrap().push("first done");
        drop(a);
        second.join().unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["first done", "second"]);
    }

    #[test]
    fn tool_steering_withdrawal_isnt_sent_if_the_only_list_never_went_out() {
        let mut st = ToolSteeringState::default();
        let list = st.claim(FL, "S", "tools").unwrap();
        assert!(st.begin_transmit("S", &list));
        let w = st.claim(FL, "S", "").expect("the list may reach the agent");
        // ...but its write fails.
        st.settle("S", &list, SteeringOutcome::NotSent);
        assert!(!st.begin_transmit("S", &w), "nothing to withdraw after all");
        assert!(!st.has_in_flight("S"));
    }

    #[test]
    fn tool_steering_prompt_takes_over_a_queued_claim() {
        let mut st = ToolSteeringState::default();
        let queued = st.claim(FL, "S", "tools").unwrap();
        let PromptSteering::Send(own) = st.claim_for_prompt(FL, "S") else {
            panic!("the prompt sends it itself");
        };
        assert_eq!((own.hash, own.for_prompt), (queued.hash, true));
        assert!(
            !st.begin_transmit("S", &queued),
            "the queued delivery skips"
        );
        finish(&mut st, "S", &own, SteeringOutcome::Delivered);
        assert_eq!(st.claim_for_prompt(FL, "S"), PromptSteering::Ready);
        assert!(st.claim_latest(FL, "S").is_none());
    }

    #[test]
    fn tool_steering_prompt_waits_for_a_turn_on_the_wire_only() {
        let mut st = ToolSteeringState::default();
        let c = st.claim(FL, "S", "v1").unwrap();
        assert!(st.begin_transmit("S", &c));
        assert_eq!(st.claim_for_prompt(FL, "S"), PromptSteering::Transmitting);
        st.settle("S", &c, SteeringOutcome::Delivered);
        assert_eq!(st.claim_for_prompt(FL, "S"), PromptSteering::Ready);
        // A session that never had tools needs nothing for a "no tools"
        // window; one that has the list needs nothing either.
        st.publish(MAIN, "");
        assert_eq!(st.claim_for_prompt(MAIN, "fresh"), PromptSteering::Ready);
        assert_eq!(
            st.claim_for_prompt("inline", "fresh"),
            PromptSteering::Ready
        );
    }

    #[test]
    fn tool_steering_timed_out_block_is_not_retried_for_prompts() {
        // The review's case: an agent too slow to ack inside the deadline.
        // Retrying the same block before every prompt would hold each one
        // for a whole deadline and never finish.
        let mut st = ToolSteeringState::default();
        let c = st.claim(FL, "S", "v1").unwrap();
        finish(&mut st, "S", &c, SteeringOutcome::TimedOut);
        assert_eq!(st.claim_for_prompt(FL, "S"), PromptSteering::Ready);
        assert_eq!(st.claim_for_prompt(FL, "S"), PromptSteering::Ready);
        // A changed block is still sent, and a withdrawal still applies.
        st.publish(FL, "v2");
        let PromptSteering::Send(v2) = st.claim_for_prompt(FL, "S") else {
            panic!("a new block goes out");
        };
        st.abandon("S", &v2);
        assert!(st.claim(FL, "S", "").is_some());
        // A Stop, unlike a timeout, is retried.
        let mut st = ToolSteeringState::default();
        let c = st.claim(FL, "S", "v1").unwrap();
        finish(&mut st, "S", &c, SteeringOutcome::Unknown);
        assert!(matches!(
            st.claim_for_prompt(FL, "S"),
            PromptSteering::Send(_)
        ));
    }

    #[test]
    fn tool_steering_windows_taking_turns_dont_each_retry_a_timed_out_block() {
        // Floating has a list, main has none (so a withdrawal), the agent is
        // too slow for either: after one timeout each, neither is retried.
        let mut st = ToolSteeringState::default();
        let f = st.claim(FL, "S", "f-tools").unwrap();
        finish(&mut st, "S", &f, SteeringOutcome::TimedOut);
        st.publish(MAIN, "");
        let PromptSteering::Send(w) = st.claim_for_prompt(MAIN, "S") else {
            panic!("main's withdrawal goes out once");
        };
        finish(&mut st, "S", &w, SteeringOutcome::TimedOut);
        assert_eq!(st.claim_for_prompt(FL, "S"), PromptSteering::Ready);
        assert_eq!(st.claim_for_prompt(MAIN, "S"), PromptSteering::Ready);
    }

    #[test]
    fn tool_steering_other_deliveries_step_aside_while_a_prompt_steers() {
        let steering = ToolSteering::default();
        let background = steering.lock().claim(FL, "S", "v1").unwrap();
        let guard = steering.begin_prompt("S");
        assert!(!steering.lock().is_current("S", &background));
        assert!(!steering.lock().begin_transmit("S", &background));
        drop(guard);
        assert!(steering.lock().is_current("S", &background));
        // Another session is unaffected.
        let other = steering.lock().claim(FL, "O", "v1").unwrap();
        let _guard = steering.begin_prompt("S");
        assert!(steering.lock().is_current("O", &other));
    }

    #[test]
    fn tool_steering_wait_while_transmitting_ends_on_settle_deadline_or_stop() {
        let steering = Arc::new(ToolSteering::default());
        let claim = steering.lock().claim(FL, "S", "v1").unwrap();
        assert!(steering.lock().begin_transmit("S", &claim));
        let long = Instant::now() + Duration::from_secs(60);

        // Deadline.
        let start = Instant::now();
        let soon = start + Duration::from_millis(100);
        assert!(!steering.wait_while_transmitting("S", soon, &|| false));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        // Stop.
        let start = Instant::now();
        assert!(!steering.wait_while_transmitting("S", long, &|| true));
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "{:?}",
            start.elapsed()
        );
        // Settled.
        let (st, c) = (steering.clone(), claim.clone());
        let settler = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            st.settle("S", &c, SteeringOutcome::Delivered);
        });
        assert!(steering.wait_while_transmitting("S", long, &|| false));
        settler.join().unwrap();
        // Nothing on the wire: no wait.
        assert!(steering.wait_while_transmitting("other", long, &|| false));
    }
}
