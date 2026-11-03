//! When to (re-)probe the login shell and how long a spawn may wait for
//! it. Pure: the caller passes `now`, so the timing rules are unit-tested
//! without sleeping.
//!
//! * The **first** probe (startup prewarm, or the first spawn after the
//!   user turns the setting on) is the only one a spawn may wait for, and
//!   only until [`SPAWN_WAIT`](super::super::SPAWN_WAIT) after it started —
//!   a shared deadline, so a caller that waits before taking a lock and
//!   then spawns can't wait twice.
//! * After a **failed** probe (timed out, no environment, …), a use at
//!   least [`RETRY_AFTER_FAILURE`] later starts one background re-probe.
//!   The delay doubles with each consecutive failure, capped at
//!   [`REFRESH_AFTER`], so a broken rc file costs a probe every half hour
//!   at worst rather than every minute.
//! * A successful result older than [`REFRESH_AFTER`] is refreshed in the
//!   background on the next **agent** (re)spawn — the spawn that matters
//!   for picking up a newly installed tool or an edited rc file. Lookups
//!   and version probes don't trigger it.
//!
//! Nobody ever waits for a re-probe or refresh: they keep serving the
//! cached result (or the fallback PATH) until the new one lands, and a
//! failed refresh keeps the previous good result.

use std::sync::Arc;
use std::time::{Duration, Instant};

/// First retry after a failed probe.
pub(super) const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(60);
/// Age at which an agent (re)spawn refreshes a good result; also the cap
/// on the failure backoff.
pub(super) const REFRESH_AFTER: Duration = Duration::from_secs(30 * 60);

/// What a use of the login environment is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Purpose {
    /// Any user-tool spawn (`which`, version probes, npm, python, …).
    Tool,
    /// The agent (re)spawn: may also refresh a stale result.
    Agent,
}

/// The probe lifecycle. `E` is the resolved environment (generic so the
/// tests don't need a real one).
#[derive(Debug)]
pub(super) struct Schedule<E> {
    env: Option<Arc<E>>,
    resolved_at: Option<Instant>,
    /// When the in-flight probe started, if one is running.
    probing_since: Option<Instant>,
    /// Whether any probe has finished, successfully or not.
    finished_once: bool,
    /// When the last probe failed, if the last probe failed.
    failed_at: Option<Instant>,
    consecutive_failures: u32,
}

impl<E> Schedule<E> {
    pub(super) const fn new() -> Self {
        Self {
            env: None,
            resolved_at: None,
            probing_since: None,
            finished_once: false,
            failed_at: None,
            consecutive_failures: 0,
        }
    }

    /// Record one use at `now`. Returns `true` when the caller must start
    /// a probe; the schedule then already counts it as in flight, so
    /// concurrent callers never start a second one. The caller must call
    /// [`finish`](Self::finish) when it ends (also if it couldn't start).
    pub(super) fn on_use(&mut self, now: Instant, purpose: Purpose) -> bool {
        if self.probing_since.is_some() {
            return false;
        }
        let start = if !self.finished_once {
            true
        } else if let Some(failed_at) = self.failed_at {
            now.saturating_duration_since(failed_at) >= retry_delay(self.consecutive_failures)
        } else {
            purpose == Purpose::Agent
                && self
                    .resolved_at
                    .is_some_and(|at| now.saturating_duration_since(at) >= REFRESH_AFTER)
        };
        if start {
            self.probing_since = Some(now);
        }
        start
    }

    /// Until when a caller that may block should wait: only while the very
    /// first probe is in flight, and only until `wait` after it started.
    pub(super) fn wait_deadline(&self, wait: Duration) -> Option<Instant> {
        if self.finished_once {
            return None;
        }
        self.probing_since.map(|since| since + wait)
    }

    /// Whether the first probe is still running (what waiters wait on).
    pub(super) fn first_probe_in_flight(&self) -> bool {
        !self.finished_once && self.probing_since.is_some()
    }

    /// Whether an in-flight probe is a retry/refresh rather than the first.
    pub(super) fn is_reprobe(&self) -> bool {
        self.finished_once
    }

    /// The in-flight probe ended at `now` with `result` (`None` = failed).
    pub(super) fn finish(&mut self, now: Instant, result: Option<E>) {
        self.probing_since = None;
        self.finished_once = true;
        match result {
            Some(env) => {
                self.env = Some(Arc::new(env));
                self.resolved_at = Some(now);
                self.failed_at = None;
                self.consecutive_failures = 0;
            }
            None => {
                // A failed refresh keeps the previous good result.
                self.failed_at = Some(now);
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            }
        }
    }

    /// The latest good result, if any.
    pub(super) fn current(&self) -> Option<Arc<E>> {
        self.env.clone()
    }

    /// Whether the most recent finished probe failed.
    pub(super) fn last_probe_failed(&self) -> bool {
        self.failed_at.is_some()
    }

    /// When the latest good result was resolved.
    pub(super) fn resolved_at(&self) -> Option<Instant> {
        self.resolved_at
    }
}

/// Backoff before re-probing after `failures` consecutive failures:
/// 1 min, 2 min, 4 min, … capped at [`REFRESH_AFTER`].
pub(super) fn retry_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    RETRY_AFTER_FAILURE
        .saturating_mul(1u32 << doublings)
        .min(REFRESH_AFTER)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn first_use_starts_exactly_one_probe() {
        let t0 = Instant::now();
        let mut s = Schedule::<u32>::new();
        assert!(s.wait_deadline(WAIT).is_none());
        assert!(s.on_use(t0, Purpose::Tool));
        // Concurrent users while it runs: no second probe.
        assert!(!s.on_use(t0 + secs(1), Purpose::Agent));
        assert!(!s.on_use(t0 + secs(2), Purpose::Tool));
        assert!(s.first_probe_in_flight());
        assert!(!s.is_reprobe());
    }

    #[test]
    fn only_the_first_probe_can_be_waited_for_and_the_deadline_is_shared() {
        let t0 = Instant::now();
        let mut s = Schedule::<u32>::new();
        s.on_use(t0, Purpose::Tool);
        // Every waiter gets the same deadline, however late it arrives.
        assert_eq!(s.wait_deadline(WAIT), Some(t0 + WAIT));
        s.on_use(t0 + secs(3), Purpose::Agent);
        assert_eq!(s.wait_deadline(WAIT), Some(t0 + WAIT));

        s.finish(t0 + secs(1), None);
        assert!(!s.first_probe_in_flight());
        assert!(s.wait_deadline(WAIT).is_none());
        // The retry is in flight, but nobody waits for it.
        assert!(s.on_use(t0 + secs(1) + RETRY_AFTER_FAILURE, Purpose::Tool));
        assert!(s.is_reprobe());
        assert!(s.wait_deadline(WAIT).is_none());
        assert!(!s.first_probe_in_flight());
    }

    #[test]
    fn failure_retries_once_after_a_minute() {
        let t0 = Instant::now();
        let mut s = Schedule::<u32>::new();
        s.on_use(t0, Purpose::Tool);
        let failed = t0 + secs(10);
        s.finish(failed, None);
        assert!(s.current().is_none());

        // Too soon, for any purpose.
        assert!(!s.on_use(failed + secs(59), Purpose::Tool));
        assert!(!s.on_use(failed + secs(59), Purpose::Agent));
        // A minute after the failure: one background re-probe…
        assert!(s.on_use(failed + secs(60), Purpose::Tool));
        // …and only one, however many spawns follow.
        assert!(!s.on_use(failed + secs(61), Purpose::Tool));
        assert!(!s.on_use(failed + secs(600), Purpose::Agent));

        s.finish(failed + secs(62), Some(7));
        assert!(!s.last_probe_failed());
        assert_eq!(s.current().as_deref(), Some(&7));
        // Success clears the failure: no more re-probes until stale.
        assert!(!s.on_use(failed + secs(200), Purpose::Tool));
    }

    #[test]
    fn consecutive_failures_back_off_up_to_the_refresh_interval() {
        assert_eq!(retry_delay(1), secs(60));
        assert_eq!(retry_delay(2), secs(120));
        assert_eq!(retry_delay(3), secs(240));
        assert_eq!(retry_delay(6), REFRESH_AFTER);
        assert_eq!(retry_delay(u32::MAX), REFRESH_AFTER);

        let mut now = Instant::now();
        let mut s = Schedule::<u32>::new();
        s.on_use(now, Purpose::Tool);
        s.finish(now, None);
        for failures in 1..=4u32 {
            let delay = retry_delay(failures);
            assert!(!s.on_use(now + delay - secs(1), Purpose::Tool));
            now += delay;
            assert!(s.on_use(now, Purpose::Tool), "retry #{failures}");
            s.finish(now, None);
        }
    }

    #[test]
    fn stale_result_refreshes_on_agent_spawn_only() {
        let t0 = Instant::now();
        let mut s = Schedule::<u32>::new();
        s.on_use(t0, Purpose::Tool);
        s.finish(t0, Some(1));
        assert_eq!(s.resolved_at(), Some(t0));

        let fresh = t0 + REFRESH_AFTER - secs(1);
        assert!(!s.on_use(fresh, Purpose::Agent));
        let stale = t0 + REFRESH_AFTER;
        assert!(!s.on_use(stale, Purpose::Tool), "lookups don't refresh");
        assert!(s.on_use(stale, Purpose::Agent));
        // Still served while the refresh runs; nobody waits.
        assert_eq!(s.current().as_deref(), Some(&1));
        assert!(s.wait_deadline(WAIT).is_none());
        assert!(!s.on_use(stale + secs(1), Purpose::Agent));

        s.finish(stale + secs(2), Some(2));
        assert_eq!(s.current().as_deref(), Some(&2));
        assert_eq!(s.resolved_at(), Some(stale + secs(2)));
    }

    #[test]
    fn failed_refresh_keeps_the_previous_result_and_retries() {
        let t0 = Instant::now();
        let mut s = Schedule::<u32>::new();
        s.on_use(t0, Purpose::Tool);
        s.finish(t0, Some(1));
        let stale = t0 + REFRESH_AFTER;
        assert!(s.on_use(stale, Purpose::Agent));
        s.finish(stale, None);
        assert!(s.last_probe_failed());
        assert_eq!(s.current().as_deref(), Some(&1), "stale beats nothing");
        assert!(!s.on_use(stale + secs(30), Purpose::Agent));
        assert!(s.on_use(stale + RETRY_AFTER_FAILURE, Purpose::Tool));
    }

    #[test]
    fn time_going_backwards_never_panics_or_starts_probes() {
        let t0 = Instant::now() + secs(1000);
        let mut s = Schedule::<u32>::new();
        s.on_use(t0, Purpose::Tool);
        s.finish(t0, None);
        assert!(!s.on_use(t0 - secs(500), Purpose::Agent));
    }
}
