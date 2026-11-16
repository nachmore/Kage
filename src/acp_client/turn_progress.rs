//! What a user turn did before it ended: did it stream anything, run a
//! tool, ask the user something, or get rate limited or stopped?
//!
//! The recovery ladder (`session.rs`) restarts the agent and resends a
//! failed prompt only when the turn did none of these, so a resend can never
//! repeat work the agent already started or answer a prompt twice.
//!
//! A turn is "open" on a session from the moment its `session/prompt` takes
//! the session's prompt lock until its reply is in (`AcpClient::begin_turn_progress`
//! / `take_turn_progress`). The reader thread counts every message for an
//! open turn through [`observe`], before any UI handler runs and before the
//! reply is handed to the caller, so the count is complete when the caller
//! reads it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::lock_ext::LockExt;

/// Counters for one open turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnProgress {
    /// Streamed text, thoughts, plans, and any update kind we don't know.
    pub updates: u32,
    /// `tool_call` and `tool_call_update`.
    pub tool_calls: u32,
    /// Requests the agent made of the client: permission, `fs/*`,
    /// `terminal/*`.
    pub permission_requests: u32,
    /// The agent sent its `error/rate_limit` notification.
    pub rate_limited: bool,
    /// The user stopped the turn (`session/cancel` was sent).
    pub cancel_requested: bool,
}

impl TurnProgress {
    /// Whether the turn did anything a resend could repeat or the user
    /// could have seen.
    pub fn made_progress(&self) -> bool {
        self.updates > 0 || self.tool_calls > 0 || self.permission_requests > 0
    }

    /// Add what another attempt of the same user turn did (the recovery
    /// ladder resends it).
    pub fn absorb(&mut self, other: TurnProgress) {
        self.updates = self.updates.saturating_add(other.updates);
        self.tool_calls = self.tool_calls.saturating_add(other.tool_calls);
        self.permission_requests = self
            .permission_requests
            .saturating_add(other.permission_requests);
        self.rate_limited |= other.rate_limited;
        self.cancel_requested |= other.cancel_requested;
    }

    /// Whether the turn could be sent again without repeating anything or
    /// overriding the user: it did nothing, and was neither rate limited
    /// nor stopped.
    pub fn resendable(&self) -> bool {
        !self.made_progress() && !self.rate_limited && !self.cancel_requested
    }
}

/// How one `session/prompt` attempt ended, as far as Kage can know.
///
/// Turn progress only says what Kage saw; it can't tell "the agent did
/// nothing" from "the agent is still at it, or did things Kage never heard
/// of". So a prompt is resent as it is only when how it ended rules that
/// out (see [`TurnTally::resendable`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptOutcome {
    /// The agent answered the request: a result, or a JSON-RPC error reply
    /// to it. Either way it is done with the turn.
    Answered,
    /// The request provably never reached the agent: no byte of it was
    /// written (`RequestNotSent`).
    NotSent,
    /// Anything else: it timed out, the connection was lost after it went
    /// out, the reader went away. The agent may have run it, or may still
    /// be running it.
    Unknown,
}

impl PromptOutcome {
    /// How a turn whose attempts ended as `self` and `other` stands: unknown
    /// if either is, answered if either was.
    pub fn and(self, other: PromptOutcome) -> PromptOutcome {
        use PromptOutcome::*;
        match (self, other) {
            (Unknown, _) | (_, Unknown) => Unknown,
            (Answered, _) | (_, Answered) => Answered,
            (NotSent, NotSent) => NotSent,
        }
    }

    /// Whether the agent is known to be done with it, or never had it.
    pub fn is_settled(self) -> bool {
        self != PromptOutcome::Unknown
    }
}

/// What every attempt of one user turn did and how each ended, added up
/// as the recovery ladder goes (it may resend the turn). Read once the
/// ladder is done, so nothing depends on a progress record that is gone by
/// then.
#[derive(Debug, Default)]
pub(crate) struct TurnTally {
    progress: std::cell::Cell<TurnProgress>,
    /// `None` until an attempt has ended.
    outcome: std::cell::Cell<Option<PromptOutcome>>,
}

impl TurnTally {
    /// One attempt ended as `outcome`, having done `progress`.
    pub(crate) fn add_attempt(&self, progress: TurnProgress, outcome: PromptOutcome) {
        let mut total = self.progress.get();
        total.absorb(progress);
        self.progress.set(total);
        let outcome = match self.outcome.get() {
            Some(before) => before.and(outcome),
            None => outcome,
        };
        self.outcome.set(Some(outcome));
    }

    pub(crate) fn progress(&self) -> TurnProgress {
        self.progress.get()
    }

    /// How the attempts ended, together. No attempt ended at all: unknown.
    pub(crate) fn outcome(&self) -> PromptOutcome {
        self.outcome.get().unwrap_or(PromptOutcome::Unknown)
    }

    /// Whether the turn may be sent again as it is: every attempt is known
    /// to have left the agent untouched (answered with an error, or never
    /// sent), and none of them did anything (`TurnProgress::resendable`).
    pub(crate) fn resendable(&self) -> bool {
        self.outcome().is_settled() && self.progress().resendable()
    }
}

/// Open turns, keyed by session id.
pub(crate) type TurnProgressMap = Arc<Mutex<HashMap<String, TurnProgress>>>;

/// Open a turn on `session_id`, replacing whatever was recorded for it.
pub(crate) fn begin(map: &TurnProgressMap, session_id: &str) {
    map.lock_or_recover()
        .insert(session_id.to_string(), TurnProgress::default());
}

/// Close the turn on `session_id` and return what it did.
pub(crate) fn take(map: &TurnProgressMap, session_id: &str) -> TurnProgress {
    map.lock_or_recover().remove(session_id).unwrap_or_default()
}

/// Note that the user stopped the open turn on `session_id`, if any.
pub(crate) fn mark_cancel_requested(map: &TurnProgressMap, session_id: &str) {
    if let Some(p) = map.lock_or_recover().get_mut(session_id) {
        p.cancel_requested = true;
    }
}

/// `session/update` kinds that say nothing about the turn's work.
const IGNORED_UPDATE_KINDS: &[&str] = &[
    "available_commands_update",
    "current_mode_update",
    "user_message_chunk",
    "session_info_update",
    "usage_update",
];

/// Count one inbound message (a notification, or a request the agent makes
/// of the client) towards its session's open turn.
///
/// Ignored: messages without a session id (except a rate limit, below),
/// sessions whose `session/load` replay is running (that's history), and
/// sessions with no open turn.
pub(crate) fn observe(
    map: &TurnProgressMap,
    loading: &Mutex<HashSet<String>>,
    msg: &serde_json::Value,
) {
    let Some(method) = msg.get("method").and_then(|m| m.as_str()) else {
        return;
    };
    let params = msg.get("params");
    let session_id = params
        .and_then(|p| p.get("sessionId"))
        .and_then(|s| s.as_str());

    if super::vendor_method_suffix(method) == Some("error/rate_limit") {
        let mut open = map.lock_or_recover();
        match session_id {
            Some(sid) => {
                if let Some(p) = open.get_mut(sid) {
                    p.rate_limited = true;
                }
            }
            // A rate limit that names no session is broadcast to every
            // window, so it may be about any open turn: none of them
            // restarts.
            None => {
                for p in open.values_mut() {
                    p.rate_limited = true;
                }
            }
        }
        return;
    }

    let Some(sid) = session_id else {
        return;
    };
    if loading.lock_or_recover().contains(sid) {
        return;
    }
    let mut open = map.lock_or_recover();
    let Some(progress) = open.get_mut(sid) else {
        return;
    };

    if method == "session/update" {
        let update = params.and_then(|p| p.get("update"));
        let kind = update
            .and_then(|u| u.get("sessionUpdate"))
            .and_then(|k| k.as_str())
            .unwrap_or("");
        match kind {
            "agent_message_chunk" => {
                let text = update
                    .and_then(|u| u.get("content"))
                    .and_then(|c| c.get("text"))
                    .and_then(|t| t.as_str());
                // A non-text block (an image, a resource) is output too.
                if text.is_none_or(|t| !t.is_empty()) {
                    progress.updates += 1;
                }
            }
            "tool_call" | "tool_call_update" => progress.tool_calls += 1,
            k if IGNORED_UPDATE_KINDS.contains(&k) => {}
            // agent_thought_chunk, plan, and any kind we don't know: count
            // it, so a new kind can only ever stop a resend, never cause one.
            _ => progress.updates += 1,
        }
    } else if method == "session/request_permission"
        || method.starts_with("fs/")
        || method.starts_with("terminal/")
    {
        progress.permission_requests += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn setup(open: &[&str]) -> (TurnProgressMap, Mutex<HashSet<String>>) {
        let map: TurnProgressMap = Arc::new(Mutex::new(HashMap::new()));
        for sid in open {
            begin(&map, sid);
        }
        (map, Mutex::new(HashSet::new()))
    }

    fn update(sid: &str, update: serde_json::Value) -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": { "sessionId": sid, "update": update }
        })
    }

    fn chunk(sid: &str, text: &str) -> serde_json::Value {
        update(
            sid,
            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } }),
        )
    }

    fn get(map: &TurnProgressMap, sid: &str) -> TurnProgress {
        map.lock().unwrap().get(sid).copied().unwrap()
    }

    #[test]
    fn counts_output_kinds_as_updates() {
        let (map, loading) = setup(&["s1"]);
        observe(&map, &loading, &chunk("s1", "hello"));
        observe(
            &map,
            &loading,
            &update("s1", json!({ "sessionUpdate": "agent_thought_chunk" })),
        );
        observe(
            &map,
            &loading,
            &update("s1", json!({ "sessionUpdate": "plan", "entries": [] })),
        );
        observe(
            &map,
            &loading,
            &update("s1", json!({ "sessionUpdate": "some_future_kind" })),
        );
        let p = get(&map, "s1");
        assert_eq!(p.updates, 4);
        assert!(p.made_progress());
    }

    #[test]
    fn counts_tool_calls() {
        let (map, loading) = setup(&["s1"]);
        observe(
            &map,
            &loading,
            &update(
                "s1",
                json!({ "sessionUpdate": "tool_call", "toolCallId": "t" }),
            ),
        );
        observe(
            &map,
            &loading,
            &update(
                "s1",
                json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t" }),
            ),
        );
        let p = get(&map, "s1");
        assert_eq!(p.tool_calls, 2);
        assert_eq!(p.updates, 0);
        assert!(p.made_progress());
    }

    #[test]
    fn counts_client_requests_as_permission_requests() {
        let (map, loading) = setup(&["s1"]);
        for method in [
            "session/request_permission",
            "fs/read_text_file",
            "terminal/create",
        ] {
            observe(
                &map,
                &loading,
                &json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": { "sessionId": "s1" } }),
            );
        }
        let p = get(&map, "s1");
        assert_eq!(p.permission_requests, 3);
        assert!(p.made_progress());
    }

    #[test]
    fn ignores_empty_chunks_and_bookkeeping_updates() {
        let (map, loading) = setup(&["s1"]);
        observe(&map, &loading, &chunk("s1", ""));
        for kind in [
            "available_commands_update",
            "current_mode_update",
            "user_message_chunk",
            "session_info_update",
            "usage_update",
        ] {
            observe(
                &map,
                &loading,
                &update("s1", json!({ "sessionUpdate": kind })),
            );
        }
        assert_eq!(get(&map, "s1"), TurnProgress::default());
        assert!(!get(&map, "s1").made_progress());
    }

    #[test]
    fn rate_limit_sets_the_flag_under_both_prefixes() {
        for method in ["_kiro.dev/error/rate_limit", "_kage.dev/error/rate_limit"] {
            let (map, loading) = setup(&["s1", "s2"]);
            observe(
                &map,
                &loading,
                &json!({ "jsonrpc": "2.0", "method": method, "params": { "sessionId": "s1", "message": "slow down" } }),
            );
            assert!(get(&map, "s1").rate_limited, "{method}");
            assert!(!get(&map, "s1").made_progress());
            assert!(!get(&map, "s2").rate_limited, "{method}: other session");
        }
    }

    #[test]
    fn rate_limit_without_a_session_marks_every_open_turn() {
        let (map, loading) = setup(&["s1", "s2"]);
        observe(
            &map,
            &loading,
            &json!({ "jsonrpc": "2.0", "method": "_kiro.dev/error/rate_limit", "params": {} }),
        );
        assert!(get(&map, "s1").rate_limited);
        assert!(get(&map, "s2").rate_limited);
    }

    #[test]
    fn ignores_loading_other_and_closed_sessions() {
        let (map, loading) = setup(&["s1", "s2"]);
        loading.lock().unwrap().insert("s1".to_string());
        // s1's session/load replay is history, not this turn.
        observe(&map, &loading, &chunk("s1", "old turn"));
        assert_eq!(get(&map, "s1"), TurnProgress::default());
        // A chunk for s3 (no open turn) opens nothing and touches nothing.
        observe(&map, &loading, &chunk("s3", "background"));
        assert!(!map.lock().unwrap().contains_key("s3"));
        assert_eq!(get(&map, "s2"), TurnProgress::default());
        // No session id at all.
        observe(
            &map,
            &loading,
            &json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "update": { "sessionUpdate": "plan" } } }),
        );
        assert_eq!(get(&map, "s2"), TurnProgress::default());
    }

    #[test]
    fn only_a_turn_known_to_have_left_the_agent_untouched_is_resendable() {
        use PromptOutcome::*;
        let nothing = TurnProgress::default();
        let tally = |attempts: &[PromptOutcome]| {
            let t = TurnTally::default();
            for o in attempts {
                t.add_attempt(nothing, *o);
            }
            t
        };
        // No attempt ended: nothing is known.
        assert!(!tally(&[]).resendable());
        assert!(tally(&[Answered]).resendable());
        assert!(tally(&[NotSent]).resendable());
        assert!(tally(&[NotSent, Answered]).resendable());
        assert_eq!(tally(&[NotSent, Answered]).outcome(), Answered);
        assert_eq!(tally(&[NotSent, NotSent]).outcome(), NotSent);
        // A timeout or a connection lost after the write, at any attempt.
        assert!(!tally(&[Unknown]).resendable());
        assert!(!tally(&[Unknown, Answered]).resendable());
        assert!(!tally(&[Answered, Unknown]).resendable());
        assert_eq!(tally(&[Unknown, NotSent]).outcome(), Unknown);

        // A settled turn that did something is not resendable either.
        let t = TurnTally::default();
        t.add_attempt(
            TurnProgress {
                tool_calls: 1,
                ..Default::default()
            },
            Answered,
        );
        assert!(!t.resendable());
        assert_eq!(t.progress().tool_calls, 1);
    }

    #[test]
    fn begin_resets_and_take_closes() {
        let (map, loading) = setup(&["s1"]);
        observe(&map, &loading, &chunk("s1", "hello"));
        mark_cancel_requested(&map, "s1");
        assert!(get(&map, "s1").cancel_requested);
        begin(&map, "s1");
        assert_eq!(get(&map, "s1"), TurnProgress::default());
        observe(&map, &loading, &chunk("s1", "again"));
        assert_eq!(take(&map, "s1").updates, 1);
        assert!(!map.lock().unwrap().contains_key("s1"));
        assert_eq!(take(&map, "s1"), TurnProgress::default());
        // Cancelling with no open turn opens nothing.
        mark_cancel_requested(&map, "s1");
        assert!(!map.lock().unwrap().contains_key("s1"));
    }
}
