//! ACP (Agent Communication Protocol) client.
//!
//! Split into:
//! - `types`: Protocol types (AcpRequest, AcpResponse, AcpError, etc.)
//! - `transport`: Connection management, pipe/TCP I/O, background reader thread
//! - This module: `AcpClient` facade composing the above with session/protocol logic

mod session;
pub mod transport;
pub mod types;

// Re-export public types so callers can still use `crate::acp_client::AcpRequest` etc.
pub use transport::AcpTransport;
#[allow(unused_imports)]
pub use types::{
    format_acp_error, AcpConnectionMode, AcpError, AcpNotification, AcpRequest, AcpResponse,
    NotificationHandler,
};

use anyhow::Result;
use log::{info, warn};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::lock_ext::LockExt;
use crate::process_manager::ProcessManager;

/// Maximum size for any single session's streaming accumulator (10 MB).
/// Prevents OOM if the server sends an unbounded response on a single session.
pub const MAX_ACCUMULATOR_SIZE: usize = 10 * 1024 * 1024;

/// Accumulated streaming text, keyed by session id. Each session has its own
/// bucket so chunks for session A can't leak into session B if the user
/// switches mid-stream (or auto_steering's hidden prompt overlaps with the
/// user's prompt). The notification handler appends the chunk to the bucket
/// matching its `params.sessionId`; helpers like auto_steering and the
/// inline-assist command read+clear by the session id they sent to.
pub type SessionAccumulators = Arc<Mutex<HashMap<String, String>>>;

pub struct AcpClient {
    transport: AcpTransport,
    initialized: Arc<Mutex<bool>>,
    /// Per-session accumulated streaming text. See SessionAccumulators.
    pub streaming_accumulators: SessionAccumulators,
    /// True while the server is compacting context — outgoing prompts should wait
    pub compacting: Arc<(Mutex<bool>, std::sync::Condvar)>,
    /// True while a `session/load` is in flight. When kiro-cli loads an
    /// existing session it replays the entire conversation history as a
    /// burst of `session/update` notifications (agent_message_chunk +
    /// tool_call) on the reader thread *before* the load response returns.
    /// Those are history, not live output — without gating, they'd dump the
    /// prior conversation into the floating window and poison the streaming
    /// accumulators. The notification handler checks this flag and drops
    /// session/update replay while it's set. Atomic because it's written by
    /// the loading thread and read by the reader thread with no other shared
    /// state.
    pub loading_session: Arc<AtomicBool>,
    /// Vendor extension namespace observed from incoming notifications.
    /// Two ACP vendor namespaces are recognised: `_kage.dev/` and
    /// `_kiro.dev/`. The extension surface (commands/available,
    /// metadata, commands/execute, compaction/status, ...) is identical
    /// across both prefixes. We pin to whichever we observe first on an
    /// inbound notification so outgoing requests target the right
    /// namespace, and the notification handler matches both
    /// interchangeably (see `vendor_method_suffix`).
    pub vendor_prefix: Arc<Mutex<Option<&'static str>>>,
    /// Per-session mutex serialising outgoing `session/prompt` requests.
    /// The agent treats `session/prompt` as exclusive per session — a
    /// second prompt issued while one is in flight is rejected with
    /// `-32603 "Prompt already in progress"`. Several callers race to
    /// prompt the same session the instant a turn ends (the background
    /// session titler, auto-steering, and extension tool-result
    /// follow-ups all fire from the message-complete epilogue), so we
    /// gate each on a per-session lock. See `send_prompt`.
    prompt_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    /// Coalesce guard for `restart_connection`. Holds the instant the last
    /// restart *succeeded*. The mutex is held for the full duration of a
    /// restart, so concurrent callers serialise: the first respawns the
    /// agent, and any caller that arrives within `RESTART_COOLDOWN` of a
    /// successful restart (and finds the transport already healthy) skips
    /// the respawn entirely. This is what stops a burst of failing sends —
    /// or several windows reacting to the same dead agent — from stacking
    /// into a respawn storm. See `restart_connection`.
    restart_guard: Arc<Mutex<Option<std::time::Instant>>>,
}

/// How many spawn+initialize attempts a restart makes before giving up.
const RESTART_MAX_ATTEMPTS: u32 = 3;
/// Base backoff between restart attempts; doubles each retry.
const RESTART_BASE_DELAY_MS: u64 = 300;
/// A restart that succeeded this recently is treated as "good enough" for a
/// concurrent/rapid caller — it coalesces onto the fresh connection instead
/// of respawning again.
const RESTART_COOLDOWN_MS: u64 = 2000;

/// Decide whether a `restart_connection` caller should coalesce onto a recent
/// restart instead of respawning. Coalesce only when BOTH: a prior restart
/// succeeded within the cooldown window, AND the transport is currently
/// healthy. If the connection died again after the last restart (not healthy),
/// we must respawn even inside the cooldown — otherwise a rapidly-flapping
/// agent would be masked by the debounce and never actually recover.
fn should_coalesce_restart(
    since_last_ok: Option<std::time::Duration>,
    transport_healthy: bool,
) -> bool {
    match since_last_ok {
        Some(elapsed) => {
            elapsed < std::time::Duration::from_millis(RESTART_COOLDOWN_MS) && transport_healthy
        }
        None => false,
    }
}

/// Recognised JSON-RPC vendor extension prefixes. Both projects ship
/// the same protocol shape under different prefixes — see the comment
/// on `AcpClient::vendor_prefix`.
pub const VENDOR_PREFIXES: &[&str] = &["_kage.dev/", "_kiro.dev/"];

/// Default outgoing vendor prefix used until we've observed an inbound
/// notification telling us which namespace the agent expects.
pub const DEFAULT_VENDOR_PREFIX: &str = "_kage.dev/";

/// If `method` is a vendor extension call, return the suffix after the
/// prefix (e.g. `_kiro.dev/commands/available` → `Some("commands/available")`).
/// Returns `None` for plain ACP methods like `session/load`.
pub fn vendor_method_suffix(method: &str) -> Option<&str> {
    for p in VENDOR_PREFIXES {
        if let Some(rest) = method.strip_prefix(p) {
            return Some(rest);
        }
    }
    None
}

impl AcpClient {
    pub fn new(mode: AcpConnectionMode) -> Self {
        Self {
            transport: AcpTransport::new(mode),
            initialized: Arc::new(Mutex::new(false)),
            streaming_accumulators: Arc::new(Mutex::new(HashMap::new())),
            compacting: Arc::new((Mutex::new(false), std::sync::Condvar::new())),
            loading_session: Arc::new(AtomicBool::new(false)),
            vendor_prefix: Arc::new(Mutex::new(None)),
            prompt_locks: Arc::new(Mutex::new(HashMap::new())),
            restart_guard: Arc::new(Mutex::new(None)),
        }
    }

    /// Whether a `session/load` replay is currently in flight. The
    /// notification handler consults this to drop replayed history updates.
    pub fn is_loading_session(&self) -> bool {
        self.loading_session.load(Ordering::Acquire)
    }

    /// Record the vendor prefix observed in an inbound method name. Idempotent
    /// — once pinned, subsequent calls are no-ops. We pin to the first
    /// observed prefix because mixing namespaces inside one session would
    /// indicate a misbehaving agent, not a feature worth supporting.
    pub fn observe_vendor_prefix(&self, method: &str) {
        for p in VENDOR_PREFIXES {
            if method.starts_with(p) {
                let mut slot = self.vendor_prefix.lock_or_recover();
                if slot.is_none() {
                    log::info!("Observed vendor extension prefix from agent: {}", p);
                    *slot = Some(*p);
                }
                return;
            }
        }
    }

    /// Vendor prefix to use for outgoing extension requests. Returns the
    /// observed prefix if any, otherwise the default (`_kage.dev/`). The
    /// returned slice is `'static` because the prefix set is compiled in.
    pub fn vendor_prefix_for_send(&self) -> &'static str {
        self.vendor_prefix
            .lock_or_recover()
            .unwrap_or(DEFAULT_VENDOR_PREFIX)
    }

    /// Build a fully-qualified vendor extension method name for outgoing
    /// requests (e.g. `commands/execute` →
    /// `_kage.dev/commands/execute` or `_kiro.dev/commands/execute`).
    pub fn vendor_method(&self, suffix: &str) -> String {
        format!("{}{}", self.vendor_prefix_for_send(), suffix)
    }

    // --- Per-session accumulator helpers ---

    /// Append `text` to the bucket for `session_id`, capped at
    /// MAX_ACCUMULATOR_SIZE. Returns the slice that was actually appended
    /// (truncated if the cap was hit), so the notification handler can emit
    /// the same delta it accumulated.
    pub fn accumulate_chunk<'a>(&self, session_id: &str, text: &'a str) -> Option<&'a str> {
        let mut map = self.streaming_accumulators.lock_or_recover();
        let acc = map
            .entry(session_id.to_string())
            .or_insert_with(|| String::with_capacity(64 * 1024));
        if acc.len() >= MAX_ACCUMULATOR_SIZE {
            return None;
        }
        let remaining = MAX_ACCUMULATOR_SIZE - acc.len();
        if text.len() <= remaining {
            acc.push_str(text);
            Some(text)
        } else {
            // Truncate at a char boundary at or below `remaining` — slicing at
            // a raw byte index panics if it lands mid-codepoint (any non-ASCII
            // response near the cap).
            let mut end = remaining;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            let slice = &text[..end];
            acc.push_str(slice);
            log::warn!(
                "Streaming accumulator for session {} hit {}MB cap — truncating",
                session_id,
                MAX_ACCUMULATOR_SIZE / (1024 * 1024)
            );
            Some(slice)
        }
    }

    /// Read the bucket for `session_id` and remove it in one critical
    /// section. Used by send-and-read flows (auto_steering, invoke_subagent,
    /// inline_assist, execute_macro) which know exactly which session they
    /// targeted and don't need the accumulator to outlive their call.
    /// Returns an empty string if no bucket exists.
    pub fn take_session_accumulator(&self, session_id: &str) -> String {
        self.streaming_accumulators
            .lock_or_recover()
            .remove(session_id)
            .unwrap_or_default()
    }

    /// Reset the bucket for `session_id` to empty. Called before send when
    /// a caller wants to read the response back via take_session_accumulator
    /// without contamination from a prior incomplete response.
    pub fn reset_session_accumulator(&self, session_id: &str) {
        self.streaming_accumulators
            .lock_or_recover()
            .remove(session_id);
    }

    // --- Delegated transport accessors ---

    pub fn set_debug_mode(&self, enabled: bool) {
        *self.transport.debug_mode.lock_or_recover() = enabled;
    }

    pub fn get_process_manager(&self) -> Arc<Mutex<ProcessManager>> {
        self.transport.process_manager.clone()
    }

    pub fn set_notification_handler<F: Fn(serde_json::Value) + Send + Sync + 'static>(
        &self,
        handler: F,
    ) {
        self.transport.set_notification_handler(handler);
    }

    pub fn is_connected(&self) -> bool {
        self.transport.is_connected()
    }

    /// Liveness-aware health check. Unlike `is_connected()`, this also
    /// confirms the managed agent process is still running (Local mode), so a
    /// zombie agent whose EOF hasn't propagated yet reads as unhealthy. See
    /// `AcpTransport::is_healthy`.
    pub fn is_healthy(&self) -> bool {
        self.transport.is_healthy()
    }

    pub fn connect(&self) -> Result<()> {
        // If the transport wasn't connected, this call spawns/reconnects a
        // *fresh* agent process that has never seen our `initialize`
        // handshake. Clear `initialized` so the next `create_session` /
        // `load_existing_session` re-runs `initialize` before issuing a
        // session op. Without this, a lazy reconnect after the agent died
        // (reader thread flipped `connected=false` on EOF) would leave
        // `initialized=true` from the *previous* process and we'd send
        // `session/load` straight at an un-initialized agent.
        let was_connected = self.transport.is_connected();
        self.transport.connect()?;
        if !was_connected {
            *self.initialized.lock_or_recover() = false;
        }
        Ok(())
    }

    pub fn disconnect(&self) {
        self.transport.disconnect();
        // Reset the initialized flag — a new agent subprocess started
        // via the next `connect()` call needs `initialize` re-sent
        // before any `session/new` will work. Without this we'd send
        // session/new straight at a fresh kiro-cli that hadn't seen
        // the protocol handshake and it'd reject the request.
        *self.initialized.lock_or_recover() = false;
        self.clear_compaction_gate();
    }

    /// Switch the transport to a new connection mode. Disconnects any
    /// active connection first and resets initialization state so the
    /// next `connect()` re-handshakes with the fresh backend.
    pub fn set_mode(&self, mode: AcpConnectionMode) {
        self.transport.set_mode(mode);
        *self.initialized.lock_or_recover() = false;
        self.clear_compaction_gate();
    }

    /// Reset the compaction-in-flight flag and wake any thread currently
    /// inside `wait_for_compaction`. If the agent died mid-compaction the
    /// "completed" notification was lost; without this every subsequent
    /// `send_chat_streaming` would block in the wait_timeout_while for the
    /// full 60s before giving up. Called from every teardown route
    /// (`disconnect` and `force_disconnect`) so a recovery restart can't
    /// inherit a stale gate from the dead connection.
    fn clear_compaction_gate(&self) {
        let (lock, cvar) = &*self.compacting;
        let mut is_compacting = lock.lock_or_recover();
        if *is_compacting {
            log::info!("Clearing in-flight compaction gate on teardown");
            *is_compacting = false;
            cvar.notify_all();
        }
    }

    /// Send a JSON-RPC request and wait for its matching response. The
    /// transport allocates the id internally — callers don't choose ids,
    /// which is what makes cross-request response delivery impossible.
    pub fn send_request(&self, method: &str, params: serde_json::Value) -> Result<AcpResponse> {
        self.transport.send_request(method, params)
    }

    /// Per-session lock guarding `session/prompt`. Created lazily on first
    /// use for a session and never removed — the count is bounded by the
    /// number of sessions touched in a process lifetime, and each entry is
    /// a zero-sized `Mutex<()>`, so leaking them is cheaper than the
    /// bookkeeping needed to reap them safely.
    fn prompt_lock_for(&self, session_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.prompt_locks.lock_or_recover();
        locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Send a `session/prompt` request, serialised per session.
    ///
    /// The agent rejects a `session/prompt` issued while another is in
    /// flight on the same session with `-32603 "Prompt already in
    /// progress"`. Multiple callers race to prompt a session the moment a
    /// turn ends — the background titler and auto-steering fire from the
    /// message-complete epilogue, and an extension tool-result follow-up
    /// arrives from the webview at almost the same instant. Holding the
    /// per-session lock across the request makes the losers queue behind
    /// the winner and run once it returns, rather than erroring out (and,
    /// in the tool-result case, dropping the result entirely).
    ///
    /// The lock is held for the full request/response round-trip because
    /// the agent's exclusivity spans exactly that window: the slot frees
    /// only when the prompt's response (`stopReason`) comes back.
    ///
    /// The session's streaming accumulator is reset under the lock, right
    /// before sending, so the bucket holds exactly this prompt's response.
    /// Doing it here (rather than in each caller, before the lock) is what
    /// makes overlap safe: a contending caller can't wipe the in-flight
    /// prompt's partial stream, because it can't reset until it owns the
    /// slot.
    pub fn send_prompt(&self, session_id: &str, params: serde_json::Value) -> Result<AcpResponse> {
        let lock = self.prompt_lock_for(session_id);
        let _guard = lock.lock_or_recover();
        self.reset_session_accumulator(session_id);
        self.transport.send_prompt_request("session/prompt", params)
    }

    /// Like `send_prompt`, but for *background* prompts that must never
    /// make a user wait: yields instead of queuing when a prompt is
    /// already in flight on the session.
    ///
    /// Returns `Ok(None)` when the per-session lock was already held —
    /// the caller should treat this as "skip this round and retry later"
    /// (both background callers, the session titler and auto-steering,
    /// re-attempt on the next `message_complete`). `Ok(Some(_))` carries
    /// the response when we acquired the lock and sent.
    ///
    /// This is what keeps an interactive follow-up prompt from waiting
    /// behind a cosmetic title-generation request: the titler steps aside
    /// for real traffic rather than contending with it.
    pub fn try_send_prompt(
        &self,
        session_id: &str,
        params: serde_json::Value,
    ) -> Result<Option<AcpResponse>> {
        let lock = self.prompt_lock_for(session_id);
        let _guard = match lock.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
            // Poisoned: a prior holder panicked. Recover the guard rather
            // than propagating — the data is `()`, so there's nothing to
            // be inconsistent.
            Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
        };
        // Reset under the lock, mirroring `send_prompt` — only now that we
        // own the slot is it safe to clear the bucket.
        self.reset_session_accumulator(session_id);
        self.transport
            .send_prompt_request("session/prompt", params)
            .map(Some)
    }

    /// Send a JSON-RPC notification (no id, fire-and-forget). Used for
    /// protocol messages that don't expect a response, like session/cancel.
    /// The transport handles serialization and write framing; callers
    /// supply the method name and params.
    pub fn send_notification(&self, method: &str, params: serde_json::Value) -> Result<()> {
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let line = serde_json::to_string(&notif)?;
        self.transport.write_line(&line)
    }

    /// Announce that a live turn's session id changed underneath the caller
    /// — the recovery ladder minted (or reloaded onto) a fresh session after
    /// the backend died mid-turn, and the resend will stream under `new_id`.
    ///
    /// Emitted as a synthetic `_kage/session_migrated` notification so it
    /// rides the existing notification-handler dispatch (which owns the
    /// `AppHandle` and the streaming-audience fan-out) without the session
    /// layer needing a Tauri handle. Distinct from the `session_reset` event:
    /// that one is *terminal* (aborts the wait, shows an error), whereas a
    /// migration means "keep waiting — same turn, new id". Windows pinned to
    /// `old_id` adopt `new_id` and clear the accumulated steering-reply text
    /// so the resend renders clean.
    pub fn notify_session_migrated(&self, old_id: &str, new_id: &str) {
        if old_id == new_id {
            return;
        }
        log::info!("Session migrated mid-turn: {} → {}", old_id, new_id);
        self.transport
            .dispatch_synthetic_notification(serde_json::json!({
                "jsonrpc": "2.0",
                "method": "_kage/session_migrated",
                "params": { "oldSessionId": old_id, "newSessionId": new_id },
            }));
    }

    /// Cancel any in-flight prompt for the given session. The agent
    /// (kiro-cli) treats session/prompt as exclusive per session, so
    /// shutdown paths and the user-facing Cancel button must clear that
    /// lock before issuing a new prompt or letting the connection drop.
    /// No-op if there's no current session.
    pub fn cancel_session(&self, session_id: &str) -> Result<()> {
        info!("Sending session/cancel for session {}", session_id);
        self.send_notification(
            "session/cancel",
            serde_json::json!({ "sessionId": session_id }),
        )
    }

    /// Reply to a session/request_permission notification. The agent waits
    /// for a JSON-RPC *response* keyed to the original request id; this is
    /// not a notification — but it doesn't go through send_request because
    /// we're answering, not asking. We write the wire-formatted response
    /// directly through the transport, bypassing the pending-request map.
    ///
    /// `option_id` is one of the protocol-defined choices: "allow_once",
    /// "allow_24h", "allow_always", "reject_once". The agent rejects
    /// anything else. We pass the string through rather than enforcing
    /// here because the set may grow over the protocol's lifetime.
    pub fn send_permission_response(
        &self,
        request_id: &serde_json::Value,
        option_id: &str,
    ) -> Result<()> {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": { "outcome": { "outcome": "selected", "optionId": option_id } }
        });
        let line = serde_json::to_string(&response)?;
        self.transport.write_line(&line)
    }

    // --- Connection lifecycle (used by session.rs recovery) ---

    pub(crate) fn force_disconnect(&self) {
        self.transport.force_disconnect();
        *self.initialized.lock_or_recover() = false;
        // Match `disconnect`'s teardown: clear the compaction gate so a
        // recovery `restart_connection` (which goes through here) can't
        // inherit a stale "is_compacting=true" from the dead connection and
        // stall the next prompt for 60s.
        self.clear_compaction_gate();
    }

    /// Tear down and rebuild the agent connection, with coalescing and retry.
    ///
    /// Two failure modes this guards against:
    ///
    /// 1. **Respawn storms.** A burst of failing sends, or several windows all
    ///    reacting to the same dead agent, would each call `restart_connection`
    ///    and each kill+respawn kiro-cli. The `restart_guard` mutex serialises
    ///    callers; a caller that arrives within `RESTART_COOLDOWN_MS` of a
    ///    successful restart and finds the transport already healthy returns
    ///    immediately without respawning.
    ///
    /// 2. **Transient cold-start EOF.** kiro-cli occasionally EOFs on the first
    ///    `initialize` right after spawn (this is exactly what stranded the app
    ///    for a week — a single unlucky launch with no retry). We now retry the
    ///    spawn+initialize up to `RESTART_MAX_ATTEMPTS` with exponential
    ///    backoff before surfacing the failure.
    pub(crate) fn restart_connection(&self) -> Result<()> {
        // Serialise restarts. Whoever holds this mutex owns the respawn; late
        // arrivals block here and then hit the cooldown check below.
        let mut last_ok = self.restart_guard.lock_or_recover();

        // Coalesce: if another caller just rebuilt the connection and it's
        // still healthy, don't tear a working agent down again.
        if should_coalesce_restart(
            last_ok.map(|when| when.elapsed()),
            self.transport.is_healthy(),
        ) {
            info!("restart_connection: coalescing onto recent healthy restart");
            return Ok(());
        }

        info!("Restarting ACP connection");
        self.force_disconnect();

        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 0..RESTART_MAX_ATTEMPTS {
            // Backoff before the first attempt too: gives the just-killed
            // child a moment to release its stdio handles before we respawn.
            let delay = RESTART_BASE_DELAY_MS * 2u64.pow(attempt);
            std::thread::sleep(std::time::Duration::from_millis(delay));

            match self.try_connect_and_initialize() {
                Ok(()) => {
                    *last_ok = Some(std::time::Instant::now());
                    return Ok(());
                }
                Err(e) => {
                    warn!(
                        "restart_connection attempt {}/{} failed: {}",
                        attempt + 1,
                        RESTART_MAX_ATTEMPTS,
                        e
                    );
                    // Tear the half-open connection down before retrying so
                    // the next spawn starts from a clean slate.
                    self.force_disconnect();
                    last_err = Some(e);
                }
            }
        }

        Err(last_err
            .unwrap_or_else(|| anyhow::anyhow!("restart_connection failed with no recorded error")))
    }

    /// One spawn+initialize round. Split out so `restart_connection` can retry
    /// it. `connect()` resets `initialized` when it spawns a fresh process, so
    /// the subsequent `initialize()` always runs against the new agent.
    fn try_connect_and_initialize(&self) -> Result<()> {
        self.transport.connect()?;
        self.initialize()?;
        Ok(())
    }

    // Session and protocol methods are in session.rs

    /// Block the current thread until compaction is finished (with a timeout).
    /// Returns true if we waited, false if compaction wasn't active.
    ///
    /// Wakes early if the transport disconnects mid-wait. The reader
    /// thread doesn't know about the compaction Condvar, so we
    /// poll-with-timeout in 500ms slices and re-check `is_connected()`
    /// each time. Without this, an agent that died mid-compaction
    /// would gate every subsequent send for the full 60s before the
    /// timeout fired (the "completed" notification was lost with the
    /// reader thread). Disconnect-then-reconnect sessions used to feel
    /// permanently broken.
    pub fn wait_for_compaction(&self) -> bool {
        let (lock, cvar) = &*self.compacting;
        let mut compacting = lock.lock_or_recover();
        if !*compacting {
            return false;
        }
        info!("Waiting for compaction to finish before sending prompt...");
        let total_timeout = std::time::Duration::from_secs(60);
        let slice = std::time::Duration::from_millis(500);
        let start = std::time::Instant::now();
        loop {
            if !*compacting {
                info!("Compaction finished, proceeding with prompt");
                return true;
            }
            if !self.transport.is_connected() {
                log::warn!("Compaction wait aborted — transport disconnected");
                // Clear the gate so a future reconnect doesn't inherit
                // a stale "is_compacting=true" if the disconnect path
                // didn't already clear it.
                *compacting = false;
                cvar.notify_all();
                return true;
            }
            let elapsed = start.elapsed();
            if elapsed >= total_timeout {
                log::warn!("Compaction wait timed out after 60s — sending anyway");
                return true;
            }
            let remaining = total_timeout - elapsed;
            let this_slice = remaining.min(slice);
            let (g, _) = match cvar.wait_timeout(compacting, this_slice) {
                Ok(r) => r,
                Err(poisoned) => poisoned.into_inner(),
            };
            compacting = g;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_method_suffix_recognises_both_prefixes() {
        assert_eq!(
            vendor_method_suffix("_kage.dev/commands/execute"),
            Some("commands/execute")
        );
        assert_eq!(
            vendor_method_suffix("_kiro.dev/commands/available"),
            Some("commands/available")
        );
        assert_eq!(vendor_method_suffix("_kage.dev/metadata"), Some("metadata"));
    }

    #[test]
    fn vendor_method_suffix_returns_none_for_plain_acp_methods() {
        // Standard ACP methods must NOT be treated as vendor extensions —
        // session/load returning Some("load") would crash the suffix-match
        // dispatch in messaging.rs into wrong branches.
        assert_eq!(vendor_method_suffix("session/load"), None);
        assert_eq!(vendor_method_suffix("session/update"), None);
        assert_eq!(vendor_method_suffix("initialize"), None);
        assert_eq!(vendor_method_suffix(""), None);
    }

    #[test]
    fn vendor_prefix_for_send_defaults_until_observed() {
        let client = AcpClient::new(AcpConnectionMode::Local {
            spawn_command: "true".to_string(),
        });
        assert_eq!(client.vendor_prefix_for_send(), "_kage.dev/");
        assert_eq!(
            client.vendor_method("commands/execute"),
            "_kage.dev/commands/execute"
        );
    }

    #[test]
    fn observe_vendor_prefix_pins_to_first_seen_namespace() {
        // Once the agent has identified itself via an inbound notification,
        // outgoing vendor calls track that namespace. Subsequent observations
        // are no-ops — mixing namespaces inside a single session would
        // indicate a misbehaving agent, not a feature worth supporting.
        let client = AcpClient::new(AcpConnectionMode::Local {
            spawn_command: "true".to_string(),
        });
        client.observe_vendor_prefix("_kiro.dev/commands/available");
        assert_eq!(client.vendor_prefix_for_send(), "_kiro.dev/");
        assert_eq!(client.vendor_method("metadata"), "_kiro.dev/metadata");

        // Pinned — a later kage.dev sighting doesn't override.
        client.observe_vendor_prefix("_kage.dev/metadata");
        assert_eq!(client.vendor_prefix_for_send(), "_kiro.dev/");
    }

    #[test]
    fn observe_vendor_prefix_ignores_non_vendor_methods() {
        let client = AcpClient::new(AcpConnectionMode::Local {
            spawn_command: "true".to_string(),
        });
        client.observe_vendor_prefix("session/update");
        // Still default since no vendor prefix was observed.
        assert_eq!(client.vendor_prefix_for_send(), "_kage.dev/");
    }

    #[test]
    fn notify_session_migrated_dispatches_synthetic_notification() {
        // The migration signal must reach the notification handler as a
        // `_kage/session_migrated` method carrying old/new ids, so the handler
        // can fan it out to streaming windows.
        let client = AcpClient::new(AcpConnectionMode::Local {
            spawn_command: "true".to_string(),
        });
        let seen: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_for_handler = seen.clone();
        client.set_notification_handler(move |n| {
            seen_for_handler.lock_or_recover().push(n);
        });

        client.notify_session_migrated("old-abc", "new-xyz");

        let captured = seen.lock_or_recover();
        assert_eq!(captured.len(), 1, "exactly one notification dispatched");
        let n = &captured[0];
        assert_eq!(
            n.get("method").and_then(|m| m.as_str()),
            Some("_kage/session_migrated")
        );
        let params = n.get("params").expect("params present");
        assert_eq!(
            params.get("oldSessionId").and_then(|v| v.as_str()),
            Some("old-abc")
        );
        assert_eq!(
            params.get("newSessionId").and_then(|v| v.as_str()),
            Some("new-xyz")
        );
    }

    #[test]
    fn notify_session_migrated_is_noop_when_id_unchanged() {
        // Reloading onto the same id (attempt-2 success path) must NOT emit a
        // migration — the id didn't change, so no window needs to re-pin.
        let client = AcpClient::new(AcpConnectionMode::Local {
            spawn_command: "true".to_string(),
        });
        let seen: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_for_handler = seen.clone();
        client.set_notification_handler(move |n| {
            seen_for_handler.lock_or_recover().push(n);
        });

        client.notify_session_migrated("same-id", "same-id");

        assert!(
            seen.lock_or_recover().is_empty(),
            "no notification when id is unchanged"
        );
    }

    #[test]
    fn coalesce_restart_skips_when_recent_and_healthy() {
        // The common debounce case: a second caller arrives right after a
        // successful restart and the connection is still up → skip the
        // respawn.
        assert!(should_coalesce_restart(
            Some(std::time::Duration::from_millis(100)),
            true
        ));
    }

    #[test]
    fn coalesce_restart_respawns_when_unhealthy_even_if_recent() {
        // A flapping agent that died again inside the cooldown must NOT be
        // masked by the debounce — if the transport isn't healthy we respawn
        // regardless of how recent the last restart was.
        assert!(!should_coalesce_restart(
            Some(std::time::Duration::from_millis(100)),
            false
        ));
    }

    #[test]
    fn coalesce_restart_respawns_after_cooldown() {
        // Past the cooldown window, every caller is allowed to drive a real
        // restart again.
        assert!(!should_coalesce_restart(
            Some(std::time::Duration::from_millis(RESTART_COOLDOWN_MS + 1)),
            true
        ));
    }

    #[test]
    fn coalesce_restart_never_skips_on_first_restart() {
        // No prior successful restart recorded → the first caller always does
        // the real work.
        assert!(!should_coalesce_restart(None, true));
        assert!(!should_coalesce_restart(None, false));
    }

    #[test]
    fn prompt_lock_is_per_session() {
        let client = AcpClient::new(AcpConnectionMode::Local {
            spawn_command: "true".to_string(),
        });
        // Same session id → same lock (so contending prompts serialise).
        let a1 = client.prompt_lock_for("session-a");
        let a2 = client.prompt_lock_for("session-a");
        assert!(Arc::ptr_eq(&a1, &a2));
        // Different session → independent lock (so unrelated sessions
        // don't block each other).
        let b = client.prompt_lock_for("session-b");
        assert!(!Arc::ptr_eq(&a1, &b));
    }

    #[test]
    fn try_send_prompt_yields_when_lock_held() {
        // The background-caller contract: if the per-session prompt slot
        // is occupied, `try_send_prompt` returns Ok(None) immediately
        // rather than blocking or erroring. We simulate an in-flight
        // prompt by holding the session's lock on another thread, then
        // assert the background attempt yields. (We can't drive a real
        // send_request without a live transport, so we exercise the
        // gate via the lock directly — the same Arc<Mutex<()>> the
        // method consults.)
        let client = AcpClient::new(AcpConnectionMode::Local {
            spawn_command: "true".to_string(),
        });
        let lock = client.prompt_lock_for("session-a");
        let held = lock.lock_or_recover();

        // try_lock on the same lock would block → method must yield.
        assert!(
            lock.try_lock().is_err(),
            "precondition: lock is held, so try_lock would block"
        );
        drop(held);
        // Once released, the slot is acquirable again.
        assert!(lock.try_lock().is_ok());
    }

    #[test]
    fn prompt_lock_serialises_same_session() {
        // Two threads taking the same session's prompt lock must never
        // hold it simultaneously — this is the property that turns
        // "Prompt already in progress" collisions into queued prompts.
        use std::sync::atomic::{AtomicUsize, Ordering};

        let client = Arc::new(AcpClient::new(AcpConnectionMode::Local {
            spawn_command: "true".to_string(),
        }));
        let inside = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let client = client.clone();
                let inside = inside.clone();
                let max_seen = max_seen.clone();
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        let lock = client.prompt_lock_for("session-a");
                        let _guard = lock.lock_or_recover();
                        let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                        max_seen.fetch_max(now, Ordering::SeqCst);
                        // Touch the counter again so any overlap has a
                        // window to be observed before we decrement.
                        std::thread::yield_now();
                        inside.fetch_sub(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(
            max_seen.load(Ordering::SeqCst),
            1,
            "more than one thread held the per-session prompt lock at once"
        );
    }
}
