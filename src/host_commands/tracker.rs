//! The agent's in-flight `run_outside_sandbox` tool calls, as the ACP
//! notification stream reports them, so a bridge request can be matched to
//! the session (and so the window) that made it: same command, call still
//! open, not matched before. Fed by the notification handler
//! (`commands::messaging::notifications`).

use crate::lock_ext::LockExt;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Calls remembered at most (oldest dropped first).
const MAX_CALLS: usize = 64;

/// A call not matched within this long is forgotten.
const MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// How often a waiting match looks again.
const POLL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone)]
struct Tracked {
    session_id: Option<String>,
    tool_call_id: String,
    command: Option<String>,
    seen: Instant,
    claimed: bool,
}

/// The tool call a request was matched to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedCall {
    pub session_id: Option<String>,
    pub tool_call_id: String,
}

#[derive(Default)]
pub struct ToolCallTracker {
    calls: Mutex<VecDeque<Tracked>>,
}

impl ToolCallTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a `tool_call` / `tool_call_update` of our tool. `command` is
    /// its `rawInput.command` when the update carries one; a terminal
    /// `status` (`completed`, `failed`) forgets the call.
    pub fn observe(
        &self,
        session_id: Option<&str>,
        tool_call_id: &str,
        command: Option<&str>,
        status: Option<&str>,
    ) {
        let mut calls = self.calls.lock_or_recover();
        let terminal = matches!(status, Some("completed" | "failed" | "cancelled"));
        if let Some(pos) = calls.iter().position(|c| c.tool_call_id == tool_call_id) {
            if terminal {
                calls.remove(pos);
                return;
            }
            let call = &mut calls[pos];
            if let Some(cmd) = command {
                call.command = Some(cmd.to_string());
            }
            if call.session_id.is_none() {
                call.session_id = session_id.map(str::to_string);
            }
            return;
        }
        if terminal {
            return;
        }
        calls.retain(|c| c.seen.elapsed() < MAX_AGE);
        while calls.len() >= MAX_CALLS {
            calls.pop_front();
        }
        calls.push_back(Tracked {
            session_id: session_id.map(str::to_string),
            tool_call_id: tool_call_id.to_string(),
            command: command.map(str::to_string),
            seen: Instant::now(),
            claimed: false,
        });
    }

    /// Whether `tool_call_id` is one of ours (for updates that carry no
    /// title).
    pub fn knows(&self, tool_call_id: &str) -> bool {
        self.calls
            .lock_or_recover()
            .iter()
            .any(|c| c.tool_call_id == tool_call_id)
    }

    /// Match `command` to the newest open call with exactly that command
    /// not matched before, and mark it matched (so two identical requests
    /// get two different calls).
    pub fn claim(&self, command: &str) -> Option<MatchedCall> {
        let mut calls = self.calls.lock_or_recover();
        let call = calls
            .iter_mut()
            .rev()
            .find(|c| !c.claimed && c.command.as_deref() == Some(command))?;
        call.claimed = true;
        Some(MatchedCall {
            session_id: call.session_id.clone(),
            tool_call_id: call.tool_call_id.clone(),
        })
    }

    /// [`Self::claim`], waiting up to `wait` for the call to show up (its
    /// arguments can arrive on a later update than the request).
    pub fn claim_waiting(&self, command: &str, wait: Duration) -> Option<MatchedCall> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(m) = self.claim(command) {
                return Some(m);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(POLL);
        }
    }
}

static TRACKER: std::sync::LazyLock<ToolCallTracker> =
    std::sync::LazyLock::new(ToolCallTracker::new);

/// The process-wide tracker the notification handler feeds.
pub fn tracker() -> &'static ToolCallTracker {
    &TRACKER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_matches_its_open_call_once() {
        let t = ToolCallTracker::new();
        t.observe(Some("s1"), "c1", Some("brew install a"), Some("pending"));
        assert_eq!(
            t.claim("brew install a"),
            Some(MatchedCall {
                session_id: Some("s1".into()),
                tool_call_id: "c1".into()
            })
        );
        assert_eq!(t.claim("brew install a"), None, "already matched");
    }

    #[test]
    fn the_command_must_be_exactly_the_same() {
        let t = ToolCallTracker::new();
        t.observe(Some("s1"), "c1", Some("brew install a"), None);
        assert_eq!(t.claim("brew install a "), None);
        assert_eq!(t.claim("brew install b"), None);
    }

    #[test]
    fn arguments_can_arrive_on_a_later_update() {
        let t = ToolCallTracker::new();
        t.observe(Some("s1"), "c1", None, Some("pending"));
        assert_eq!(t.claim("ls"), None);
        t.observe(None, "c1", Some("ls"), Some("in_progress"));
        assert_eq!(t.claim("ls").unwrap().tool_call_id, "c1");
    }

    #[test]
    fn finished_calls_are_forgotten() {
        let t = ToolCallTracker::new();
        t.observe(Some("s1"), "c1", Some("ls"), None);
        assert!(t.knows("c1"));
        t.observe(Some("s1"), "c1", None, Some("completed"));
        assert!(!t.knows("c1"));
        assert_eq!(t.claim("ls"), None);
    }

    #[test]
    fn identical_requests_match_different_calls_newest_first() {
        let t = ToolCallTracker::new();
        t.observe(Some("s1"), "c1", Some("make"), None);
        t.observe(Some("s2"), "c2", Some("make"), None);
        assert_eq!(t.claim("make").unwrap().tool_call_id, "c2");
        assert_eq!(t.claim("make").unwrap().tool_call_id, "c1");
    }

    #[test]
    fn memory_is_bounded() {
        let t = ToolCallTracker::new();
        for i in 0..(MAX_CALLS + 10) {
            t.observe(Some("s"), &format!("c{i}"), Some("x"), None);
        }
        assert!(!t.knows("c0"));
        assert!(t.knows(&format!("c{}", MAX_CALLS + 9)));
    }

    #[test]
    fn claim_waiting_sees_a_late_call() {
        let t = std::sync::Arc::new(ToolCallTracker::new());
        let t2 = t.clone();
        let late = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            t2.observe(Some("s1"), "c1", Some("ls"), None);
        });
        assert!(t.claim_waiting("ls", Duration::from_secs(5)).is_some());
        late.join().unwrap();
        assert!(t.claim_waiting("nope", Duration::from_millis(60)).is_none());
    }
}
