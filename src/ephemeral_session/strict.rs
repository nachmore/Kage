//! Strict ephemeral sessions: throwaway sessions on which no tool may run.
//!
//! ACP sessions can't be created without tools, so a strict session is
//! guarded where the agent's requests arrive, in the notification handler
//! (`commands::messaging::notifications`):
//!
//! - every permission request on a strict session is refused before any
//!   policy runs, so Allow rules, trusted categories and terminator mode
//!   never approve a tool there;
//! - the first sign of a tool (a permission request, or a `tool_call` /
//!   `tool_call_update` the agent started without asking) goes through
//!   [`note_tool_attempt`], which cancels the turn and tells the owner of
//!   the session (the background agent-call service) through its
//!   [`StrictHooks`].
//!
//! A strict id stays known for a while after its session ends
//! ([`ENDED_KEPT`]), so a late request for it is still refused rather than
//! falling through to the user's policy.

use log::{info, warn};
use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex, RwLock};

use crate::acp_client::AcpClient;
use crate::lock_ext::{LockExt, RwLockExt};

/// Ended strict sessions remembered, newest last.
pub const ENDED_KEPT: usize = 256;

/// What the owner of strict sessions hears from the notification handler.
/// Plain function pointers, so the low-level ACP code never depends on
/// the service that owns the sessions. Until [`set_strict_hooks`] installs
/// them, both events are only logged.
#[derive(Debug, Clone, Copy)]
pub struct StrictHooks {
    /// The agent tried to use a tool on this strict session. Called once
    /// per session, after the turn was cancelled.
    pub tool_attempt: fn(&str),
    /// The agent's stream closed (`_kage/agent_disconnected`).
    pub disconnected: fn(),
}

#[derive(Default)]
struct StrictRegistry {
    /// Live strict sessions, and whether a tool attempt was seen.
    live: HashMap<String, bool>,
    /// Ended strict sessions and their tool-attempt flag, newest last.
    ended: VecDeque<(String, bool)>,
}

static STRICT: LazyLock<Mutex<StrictRegistry>> =
    LazyLock::new(|| Mutex::new(StrictRegistry::default()));

static HOOKS: RwLock<Option<StrictHooks>> = RwLock::new(None);

/// Install (or, with `None`, remove) the hooks. The background agent-call
/// service calls this when it starts.
#[allow(dead_code)] // Called by the background agent-call engine.
pub fn set_strict_hooks(hooks: Option<StrictHooks>) {
    *HOOKS.write_or_recover() = hooks;
}

fn hooks() -> Option<StrictHooks> {
    *HOOKS.read_or_recover()
}

/// Mark `session_id` strict for its lifetime.
pub(super) fn begin(session_id: &str) {
    STRICT
        .lock_or_recover()
        .live
        .insert(session_id.to_string(), false);
}

/// The session is gone. Its id is still refused for a while.
pub(super) fn end(session_id: &str) {
    let mut reg = STRICT.lock_or_recover();
    if let Some(attempted) = reg.live.remove(session_id) {
        reg.ended.push_back((session_id.to_string(), attempted));
        while reg.ended.len() > ENDED_KEPT {
            reg.ended.pop_front();
        }
    }
}

/// True for a live strict session, and for one that ended recently.
/// Requests and updates for either are refused and never shown.
pub fn is_strict(session_id: &str) -> bool {
    let reg = STRICT.lock_or_recover();
    reg.live.contains_key(session_id) || reg.ended.iter().any(|(id, _)| id == session_id)
}

/// Whether the agent tried to use a tool on this strict session.
#[allow(dead_code)] // Read by the background agent-call engine.
pub fn tool_attempted(session_id: &str) -> bool {
    let reg = STRICT.lock_or_recover();
    reg.live.get(session_id).copied().unwrap_or_else(|| {
        reg.ended
            .iter()
            .any(|(id, attempted)| id == session_id && *attempted)
    })
}

/// Mark the first tool attempt on a live strict session: returns `true`
/// the first time, `false` after that, for ended sessions and for
/// sessions that aren't strict.
fn mark_attempt(session_id: &str) -> bool {
    let mut reg = STRICT.lock_or_recover();
    match reg.live.get_mut(session_id) {
        Some(attempted) if !*attempted => {
            *attempted = true;
            true
        }
        _ => false,
    }
}

/// The agent tried to use a tool on `session_id`. On the first attempt
/// for a live strict session: send `session/cancel`, then call the
/// `tool_attempt` hook. Returns whether this was that first attempt.
///
/// Runs on the ACP reader thread. The cancel is a notification (nothing
/// waits for a reply), and the hook must not block either.
pub fn note_tool_attempt(client: &AcpClient, session_id: &str) -> bool {
    if !mark_attempt(session_id) {
        return false;
    }
    info!(
        "Agent tried to use a tool on strict session {}; cancelling the turn",
        session_id
    );
    if let Err(e) = client.cancel_session(session_id) {
        warn!(
            "Failed to cancel strict session {} after a tool attempt: {}",
            session_id, e
        );
    }
    if let Some(hooks) = hooks() {
        (hooks.tool_attempt)(session_id);
    }
    true
}

/// The agent's stream closed. Tells the owner of strict sessions.
pub fn notify_disconnected() {
    if let Some(hooks) = hooks() {
        (hooks.disconnected)();
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Serialises the tests that install hooks (they are process-global).
    pub(crate) static HOOK_TEST_LOCK: Mutex<()> = Mutex::new(());

    static TOOL_ATTEMPTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static DISCONNECTS: AtomicUsize = AtomicUsize::new(0);

    fn record_tool_attempt(id: &str) {
        TOOL_ATTEMPTS.lock_or_recover().push(id.to_string());
    }

    fn record_disconnect() {
        DISCONNECTS.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn recording_hooks() -> StrictHooks {
        StrictHooks {
            tool_attempt: record_tool_attempt,
            disconnected: record_disconnect,
        }
    }

    pub(crate) fn tool_attempts_for(id: &str) -> usize {
        TOOL_ATTEMPTS
            .lock_or_recover()
            .iter()
            .filter(|s| s.as_str() == id)
            .count()
    }

    fn offline_client() -> AcpClient {
        AcpClient::new(crate::acp_client::AcpConnectionMode::Local {
            spawn_command: "true".to_string(),
        })
    }

    #[test]
    fn a_session_is_strict_from_begin_and_stays_known_after_end() {
        assert!(!is_strict("strict-lifecycle"));
        begin("strict-lifecycle");
        assert!(is_strict("strict-lifecycle"));
        end("strict-lifecycle");
        assert!(
            is_strict("strict-lifecycle"),
            "a late request for an ended strict session is still refused"
        );
        assert!(!is_strict("never-strict"));
    }

    #[test]
    fn only_the_newest_ended_sessions_are_remembered() {
        let ids: Vec<String> = (0..ENDED_KEPT + 3)
            .map(|i| format!("strict-ended-{i}"))
            .collect();
        for id in &ids {
            begin(id);
            end(id);
        }
        // Other tests end sessions concurrently, so only the bound and the
        // newest are certain.
        assert!(STRICT.lock_or_recover().ended.len() <= ENDED_KEPT);
        assert!(is_strict(ids.last().unwrap()));
    }

    #[test]
    fn ending_a_session_that_was_never_strict_changes_nothing() {
        end("not-a-strict-session");
        assert!(!is_strict("not-a-strict-session"));
    }

    #[test]
    fn the_first_tool_attempt_cancels_and_calls_the_hook_once() {
        let _serial = HOOK_TEST_LOCK.lock_or_recover();
        set_strict_hooks(Some(recording_hooks()));
        let client = offline_client();
        begin("strict-attempt");
        assert!(!tool_attempted("strict-attempt"));

        // No transport: the cancel fails and is logged, the hook still runs.
        assert!(note_tool_attempt(&client, "strict-attempt"));
        assert!(tool_attempted("strict-attempt"));
        assert!(
            !note_tool_attempt(&client, "strict-attempt"),
            "the permission request that follows a tool_call is the same attempt"
        );
        assert_eq!(tool_attempts_for("strict-attempt"), 1);

        end("strict-attempt");
        assert!(
            tool_attempted("strict-attempt"),
            "kept after the session ends"
        );
        assert!(!note_tool_attempt(&client, "strict-attempt"));
        assert_eq!(tool_attempts_for("strict-attempt"), 1);
        set_strict_hooks(None);
    }

    #[test]
    fn a_tool_on_a_session_that_isnt_strict_is_not_an_attempt() {
        let _serial = HOOK_TEST_LOCK.lock_or_recover();
        set_strict_hooks(Some(recording_hooks()));
        let client = offline_client();
        assert!(!note_tool_attempt(&client, "user-chat-session"));
        assert_eq!(tool_attempts_for("user-chat-session"), 0);
        assert!(!tool_attempted("user-chat-session"));
        set_strict_hooks(None);
    }

    #[test]
    fn hooks_are_no_ops_until_installed() {
        let _serial = HOOK_TEST_LOCK.lock_or_recover();
        set_strict_hooks(None);
        let before = DISCONNECTS.load(Ordering::SeqCst);
        notify_disconnected();
        assert_eq!(DISCONNECTS.load(Ordering::SeqCst), before);

        let client = offline_client();
        begin("strict-no-hooks");
        assert!(note_tool_attempt(&client, "strict-no-hooks"));
        assert_eq!(tool_attempts_for("strict-no-hooks"), 0);
        end("strict-no-hooks");

        set_strict_hooks(Some(recording_hooks()));
        notify_disconnected();
        assert_eq!(DISCONNECTS.load(Ordering::SeqCst), before + 1);
        set_strict_hooks(None);
    }
}
