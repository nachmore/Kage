//! The agent's in-flight `run_outside_sandbox` tool calls, as the ACP
//! notification stream reports them, so a bridge request can be matched to
//! the session (and so the window) that made it: same command, same
//! working directory, call still open, not matched before. Fed by the
//! notification handler (`commands::messaging::notifications`).
//!
//! The working directory is part of the match because a match is what
//! lets the user's rules run a command without asking: a request from
//! anywhere else that names a visible call's command must not get that
//! call's standing in a folder the call never asked for.

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
    /// The call's arguments, once an update carried them.
    input: Option<CallInput>,
    seen: Instant,
    claimed: bool,
}

/// What a call asked to run: its `rawInput.command` and `rawInput.cwd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallInput {
    pub command: String,
    /// `None` for an absent or blank `cwd` (the sidecar sends none then).
    pub cwd: Option<String>,
}

impl CallInput {
    pub fn new(command: &str, cwd: Option<&str>) -> Self {
        Self {
            command: command.to_string(),
            cwd: normalize_cwd(cwd).map(str::to_string),
        }
    }
}

/// A `cwd` as both sides of the match read it: blank is none.
pub fn normalize_cwd(cwd: Option<&str>) -> Option<&str> {
    cwd.filter(|c| !c.trim().is_empty())
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

    /// Record a `tool_call` / `tool_call_update` of our tool. `input` is
    /// its `rawInput` (command and cwd, read together) when the update
    /// carries one; a terminal `status` (`completed`, `failed`) forgets the
    /// call.
    pub fn observe(
        &self,
        session_id: Option<&str>,
        tool_call_id: &str,
        input: Option<CallInput>,
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
            if input.is_some() {
                call.input = input;
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
            input,
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

    /// Stop tracking `tool_call_id` (it turned out not to be our tool).
    pub fn forget(&self, tool_call_id: &str) {
        self.calls
            .lock_or_recover()
            .retain(|c| c.tool_call_id != tool_call_id);
    }

    /// Match a request for `command` in `cwd` to the newest open call that
    /// asked for exactly that command in exactly that directory and wasn't
    /// matched before, and mark it matched (so two identical requests get
    /// two different calls).
    pub fn claim(&self, command: &str, cwd: Option<&str>) -> Option<MatchedCall> {
        let want = CallInput::new(command, cwd);
        let mut calls = self.calls.lock_or_recover();
        let call = calls
            .iter_mut()
            .rev()
            .find(|c| !c.claimed && c.input.as_ref() == Some(&want))?;
        call.claimed = true;
        Some(MatchedCall {
            session_id: call.session_id.clone(),
            tool_call_id: call.tool_call_id.clone(),
        })
    }

    /// [`Self::claim`], waiting up to `wait` for the call to show up (its
    /// arguments can arrive on a later update than the request).
    pub fn claim_waiting(
        &self,
        command: &str,
        cwd: Option<&str>,
        wait: Duration,
    ) -> Option<MatchedCall> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(m) = self.claim(command, cwd) {
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

    fn input(command: &str) -> Option<CallInput> {
        Some(CallInput::new(command, None))
    }

    #[test]
    fn a_request_matches_its_open_call_once() {
        let t = ToolCallTracker::new();
        t.observe(Some("s1"), "c1", input("brew install a"), Some("pending"));
        assert_eq!(
            t.claim("brew install a", None),
            Some(MatchedCall {
                session_id: Some("s1".into()),
                tool_call_id: "c1".into()
            })
        );
        assert_eq!(t.claim("brew install a", None), None, "already matched");
    }

    #[test]
    fn the_command_must_be_exactly_the_same() {
        let t = ToolCallTracker::new();
        t.observe(Some("s1"), "c1", input("brew install a"), None);
        assert_eq!(t.claim("brew install a ", None), None);
        assert_eq!(t.claim("brew install b", None), None);
    }

    #[test]
    fn the_working_directory_must_be_the_one_the_call_asked_for() {
        let t = ToolCallTracker::new();
        t.observe(
            Some("s1"),
            "c1",
            Some(CallInput::new("make", Some("/Users/me/project"))),
            None,
        );
        // Same command elsewhere (a folder another process filled), or with
        // no folder at all (the home folder): not this call.
        assert_eq!(t.claim("make", Some("/tmp/evil")), None);
        assert_eq!(t.claim("make", None), None);
        assert_eq!(
            t.claim("make", Some("/Users/me/project"))
                .unwrap()
                .tool_call_id,
            "c1"
        );

        // A blank cwd is none, on both sides.
        t.observe(
            Some("s1"),
            "c2",
            Some(CallInput::new("ls", Some("  "))),
            None,
        );
        assert_eq!(t.claim("ls", Some("")).unwrap().tool_call_id, "c2");
    }

    #[test]
    fn arguments_can_arrive_on_a_later_update() {
        let t = ToolCallTracker::new();
        t.observe(Some("s1"), "c1", None, Some("pending"));
        assert_eq!(t.claim("ls", None), None);
        t.observe(None, "c1", input("ls"), Some("in_progress"));
        assert_eq!(t.claim("ls", None).unwrap().tool_call_id, "c1");
    }

    #[test]
    fn finished_or_forgotten_calls_are_gone() {
        let t = ToolCallTracker::new();
        t.observe(Some("s1"), "c1", input("ls"), None);
        assert!(t.knows("c1"));
        t.observe(Some("s1"), "c1", None, Some("completed"));
        assert!(!t.knows("c1"));
        assert_eq!(t.claim("ls", None), None);

        t.observe(Some("s1"), "c2", input("ls"), None);
        t.forget("c2");
        assert_eq!(t.claim("ls", None), None);
    }

    #[test]
    fn identical_requests_match_different_calls_newest_first() {
        let t = ToolCallTracker::new();
        t.observe(Some("s1"), "c1", input("make"), None);
        t.observe(Some("s2"), "c2", input("make"), None);
        assert_eq!(t.claim("make", None).unwrap().tool_call_id, "c2");
        assert_eq!(t.claim("make", None).unwrap().tool_call_id, "c1");
    }

    #[test]
    fn memory_is_bounded() {
        let t = ToolCallTracker::new();
        for i in 0..(MAX_CALLS + 10) {
            t.observe(Some("s"), &format!("c{i}"), input("x"), None);
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
            t2.observe(Some("s1"), "c1", input("ls"), None);
        });
        assert!(t
            .claim_waiting("ls", None, Duration::from_secs(5))
            .is_some());
        late.join().unwrap();
        assert!(t
            .claim_waiting("nope", None, Duration::from_millis(60))
            .is_none());
    }
}
