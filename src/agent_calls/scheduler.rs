//! The queue of reserved calls: one queue per extension, round-robin
//! between extensions, one turn in total, and the per-extension and global
//! cooldowns and hourly counts (docs/design/EXTENSION_AGENT_CALLS.md §5.6,
//! §5.7).
//!
//! Pure bookkeeping: the service decides whether a head may start (idle,
//! allowance, gates) and calls [`Scheduler::grant`].

use chrono::{DateTime, Duration, Utc};
use std::collections::{BTreeMap, HashMap, VecDeque};

use super::limits;

/// The call holding the one turn (given a turn, or running).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Active {
    call_id: String,
    extension_id: String,
}

#[derive(Debug, Default)]
pub struct Scheduler {
    /// Waiting reservations per extension, head first.
    queues: BTreeMap<String, VecDeque<String>>,
    active: Option<Active>,
    /// The extension that got the last turn (round-robin cursor).
    last_extension: Option<String>,
    last_end_by_extension: HashMap<String, DateTime<Utc>>,
    last_end: Option<DateTime<Utc>>,
    /// When each extension's calls were sent, within the last hour.
    starts: HashMap<String, VecDeque<DateTime<Utc>>>,
}

impl Scheduler {
    /// Add a reservation at the back of its extension's queue. Returns its
    /// 1-based position in that queue.
    pub fn enqueue(&mut self, extension_id: &str, call_id: &str) -> usize {
        let q = self.queues.entry(extension_id.to_string()).or_default();
        q.push_back(call_id.to_string());
        q.len()
    }

    /// Whether the extension has room for another waiting reservation.
    pub fn has_room(&self, extension_id: &str) -> bool {
        self.waiting(extension_id) < limits::MAX_WAITING_PER_EXTENSION
    }

    /// Remove a waiting reservation. `false` if it wasn't waiting.
    pub fn remove_waiting(&mut self, call_id: &str) -> bool {
        let mut found = false;
        for q in self.queues.values_mut() {
            if let Some(i) = q.iter().position(|c| c == call_id) {
                q.remove(i);
                found = true;
                break;
            }
        }
        self.queues.retain(|_, q| !q.is_empty());
        found
    }

    pub fn waiting(&self, extension_id: &str) -> usize {
        self.queues.get(extension_id).map_or(0, VecDeque::len)
    }

    pub fn waiting_total(&self) -> usize {
        self.queues.values().map(VecDeque::len).sum()
    }

    /// The call holding the turn, if any.
    pub fn active(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.call_id.as_str())
    }

    /// Each extension's head, in round-robin order: starting with the
    /// extension after the one that got the last turn.
    pub fn heads_in_turn_order(&self) -> Vec<(String, String)> {
        let heads: Vec<(String, String)> = self
            .queues
            .iter()
            .filter_map(|(ext, q)| q.front().map(|c| (ext.clone(), c.clone())))
            .collect();
        let Some(last) = &self.last_extension else {
            return heads;
        };
        let split = heads
            .iter()
            .position(|(ext, _)| ext > last)
            .unwrap_or(heads.len());
        let mut ordered = heads[split..].to_vec();
        ordered.extend_from_slice(&heads[..split]);
        ordered
    }

    /// Give the turn to `call_id`, which must be its extension's head and
    /// nothing may hold the turn. `false` otherwise.
    pub fn grant(&mut self, extension_id: &str, call_id: &str) -> bool {
        if self.active.is_some() {
            return false;
        }
        let Some(q) = self.queues.get_mut(extension_id) else {
            return false;
        };
        if q.front().map(String::as_str) != Some(call_id) {
            return false;
        }
        q.pop_front();
        self.queues.retain(|_, q| !q.is_empty());
        self.active = Some(Active {
            call_id: call_id.to_string(),
            extension_id: extension_id.to_string(),
        });
        self.last_extension = Some(extension_id.to_string());
        true
    }

    /// Note that the turn holder was sent at `now` (it counts toward the
    /// hourly cap).
    pub fn record_start(&mut self, extension_id: &str, now: DateTime<Utc>) {
        let s = self.starts.entry(extension_id.to_string()).or_default();
        s.push_back(now);
        let hour_ago = now - Duration::hours(1);
        while s.front().is_some_and(|t| *t <= hour_ago) {
            s.pop_front();
        }
    }

    /// Free the turn if `call_id` holds it. `ended` is when a call that
    /// was sent ended: the cooldowns run from then. A turn given back
    /// unused (never sent) starts no cooldown.
    pub fn release(&mut self, call_id: &str, ended: Option<DateTime<Utc>>) -> bool {
        let Some(active) = self.active.take_if(|a| a.call_id == call_id) else {
            return false;
        };
        if let Some(t) = ended {
            self.last_end = Some(t);
            self.last_end_by_extension.insert(active.extension_id, t);
        }
        true
    }

    /// When the cooldowns let `extension_id` start next, if that is later
    /// than `now`.
    pub fn cooldown_until(&self, extension_id: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let own = self
            .last_end_by_extension
            .get(extension_id)
            .map(|t| *t + Duration::seconds(limits::EXTENSION_COOLDOWN_SECS));
        let global = self
            .last_end
            .map(|t| t + Duration::seconds(limits::GLOBAL_COOLDOWN_SECS));
        own.into_iter().chain(global).max().filter(|t| *t > now)
    }

    /// When `extension_id` drops back under the hourly cap, if it is at it.
    pub fn hourly_free_at(&self, extension_id: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let s = self.starts.get(extension_id)?;
        let hour_ago = now - Duration::hours(1);
        let recent: Vec<&DateTime<Utc>> = s.iter().filter(|t| **t > hour_ago).collect();
        if recent.len() < limits::CALLS_PER_HOUR {
            return None;
        }
        // The oldest of the newest CALLS_PER_HOUR leaves the window first.
        let oldest = recent[recent.len() - limits::CALLS_PER_HOUR];
        Some(*oldest + Duration::hours(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-09T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn one_turn_in_total() {
        let mut s = Scheduler::default();
        s.enqueue("a", "a1");
        s.enqueue("b", "b1");
        assert!(s.grant("a", "a1"));
        assert_eq!(s.active(), Some("a1"));
        // Nothing else while a turn is held, for any extension.
        assert!(!s.grant("b", "b1"));
        assert!(s.release("a1", None));
        assert!(s.grant("b", "b1"));
    }

    #[test]
    fn only_the_head_may_start() {
        let mut s = Scheduler::default();
        assert_eq!(s.enqueue("a", "a1"), 1);
        assert_eq!(s.enqueue("a", "a2"), 2);
        assert!(!s.grant("a", "a2"));
        assert!(s.grant("a", "a1"));
        assert_eq!(s.waiting("a"), 1);
    }

    #[test]
    fn round_robin_between_extensions() {
        let mut s = Scheduler::default();
        for (ext, id) in [
            ("a", "a1"),
            ("a", "a2"),
            ("b", "b1"),
            ("c", "c1"),
            ("c", "c2"),
        ] {
            s.enqueue(ext, id);
        }
        let mut order = Vec::new();
        while let Some((ext, id)) = s.heads_in_turn_order().into_iter().next() {
            assert!(s.grant(&ext, &id));
            order.push(id.clone());
            s.release(&id, None);
        }
        assert_eq!(order, vec!["a1", "b1", "c1", "a2", "c2"]);
    }

    #[test]
    fn round_robin_order_wraps_after_the_last_extension() {
        let mut s = Scheduler::default();
        s.enqueue("a", "a1");
        s.enqueue("b", "b1");
        s.enqueue("c", "c1");
        assert!(s.grant("b", "b1"));
        s.release("b1", None);
        let order: Vec<String> = s
            .heads_in_turn_order()
            .into_iter()
            .map(|(e, _)| e)
            .collect();
        assert_eq!(order, vec!["c", "a"]);
    }

    #[test]
    fn queue_room_and_removal() {
        let mut s = Scheduler::default();
        for i in 0..4 {
            assert!(s.has_room("a"));
            s.enqueue("a", &format!("a{i}"));
        }
        assert!(!s.has_room("a"));
        assert!(s.has_room("b"));
        assert!(s.remove_waiting("a2"));
        assert!(!s.remove_waiting("a2"));
        assert_eq!(s.waiting("a"), 3);
        assert_eq!(s.waiting_total(), 3);
    }

    #[test]
    fn cooldowns_run_from_the_end_of_a_sent_call() {
        let mut s = Scheduler::default();
        let t = t0();
        s.enqueue("a", "a1");
        s.grant("a", "a1");
        s.record_start("a", t);
        // Ends a minute after it started: the cooldown counts from the end.
        let end = t + Duration::seconds(60);
        s.release("a1", Some(end));
        assert_eq!(
            s.cooldown_until("a", end + Duration::seconds(1)),
            Some(end + Duration::seconds(15))
        );
        assert_eq!(s.cooldown_until("a", end + Duration::seconds(15)), None);
        // Another extension waits only for the global cooldown.
        assert_eq!(
            s.cooldown_until("b", end + Duration::seconds(1)),
            Some(end + Duration::seconds(5))
        );
        assert_eq!(s.cooldown_until("b", end + Duration::seconds(5)), None);
        // A turn given back unused starts no cooldown.
        s.enqueue("b", "b1");
        s.grant("b", "b1");
        s.release("b1", None);
        assert_eq!(s.cooldown_until("b", end + Duration::seconds(6)), None);
    }

    #[test]
    fn hourly_cap() {
        let mut s = Scheduler::default();
        let t = t0();
        for i in 0..20 {
            s.record_start("a", t + Duration::minutes(i));
        }
        let now = t + Duration::minutes(30);
        assert_eq!(s.hourly_free_at("a", now), Some(t + Duration::hours(1)));
        assert_eq!(s.hourly_free_at("b", now), None);
        assert_eq!(s.hourly_free_at("a", t + Duration::hours(1)), None);
    }
}
