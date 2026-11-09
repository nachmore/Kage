//! Host limits for background agent calls
//! (docs/design/EXTENSION_AGENT_CALLS.md §5.6), the clock they are
//! measured with, and the local-day helpers.
//!
//! Everything here is pure: callers pass `now`, so tests drive time with a
//! fake clock.

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Offset, TimeZone, Utc};

use crate::config::AgentAllowance;

/// Reservations an extension may have waiting (not running, not holding a
/// turn).
pub const MAX_WAITING_PER_EXTENSION: usize = 4;
/// How long an extension has to send after its turn starts.
pub const SEND_WINDOW_SECS: i64 = 10;
/// Pause between the end of one extension's call and its next start.
pub const EXTENSION_COOLDOWN_SECS: i64 = 15;
/// Pause between the end of any call and the next start.
pub const GLOBAL_COOLDOWN_SECS: i64 = 5;
/// Calls one extension may send in any hour.
pub const CALLS_PER_HOUR: usize = 20;
/// Calls all extensions together may send in one local day.
pub const GLOBAL_DAILY_CALLS: u32 = 200;

/// `instructions`, in characters.
pub const MAX_INSTRUCTIONS_CHARS: usize = 4_000;
/// `input`, serialized, in characters.
pub const MAX_INPUT_CHARS: usize = 24_000;
/// `schema`, serialized, in bytes.
pub const MAX_SCHEMA_BYTES: usize = 8 * 1024;
/// Deepest schema: the root is depth 1.
pub const MAX_SCHEMA_DEPTH: usize = 8;
/// Most schema objects in one schema.
pub const MAX_SCHEMA_NODES: usize = 200;
/// The framed prompt, in characters (a manifest may lower it).
pub const MAX_PROMPT_CHARS: usize = 32_000;
/// `label`, in characters.
pub const MAX_LABEL_CHARS: usize = 80;
/// `dedupe_key`, in characters.
pub const MAX_DEDUPE_KEY_CHARS: usize = 64;
/// The reply, when the request doesn't say.
pub const DEFAULT_RESPONSE_CHARS: u32 = 8_000;
/// The longest reply a request may ask for.
pub const MAX_RESPONSE_CHARS: u32 = 32_000;
/// The streaming reply may grow to this many bytes per allowed character
/// before the call is stopped. The exact character check runs at the end.
pub const STREAM_BYTES_PER_CHAR: usize = 4;
/// Timeout when the request doesn't say.
pub const DEFAULT_TIMEOUT_MS: u32 = 60_000;
pub const MIN_TIMEOUT_MS: u32 = 5_000;
pub const MAX_TIMEOUT_MS: u32 = 180_000;

/// Failures in a row that open the failure breaker.
pub const FAILURE_BREAKER_THRESHOLD: u32 = 3;
/// How long the failure breaker stays open.
pub const FAILURE_PAUSE_MINUTES: i64 = 30;
/// Tool attempts in one local day that open the tool breaker (until the
/// next local midnight).
pub const TOOL_BREAKER_THRESHOLD: u32 = 2;
/// How long **Stop now** pauses.
pub const STOP_PAUSE_MINUTES: i64 = 60;

/// The user counts as active for this long after their last activity.
pub const IDLE_AFTER_SECS: i64 = 90;
/// The scheduler wakes at least this often.
pub const TICK_MS: u64 = 5_000;
/// How often the watchdog checks a running call.
pub const WATCHDOG_POLL_MS: u64 = 250;
/// After a stop, `session/cancel` is sent again this often until the
/// session ends (a cancel can arrive before the prompt it is meant for).
pub const CANCEL_RESEND_MS: i64 = 2_000;
/// How long shutdown waits for a running call's session to be deleted.
pub const SHUTDOWN_WAIT_MS: u64 = 2_000;
/// Calls whose prompt and reply stay in memory for "Show what was sent".
pub const RECENT_CALLS: usize = 5;
/// Finished call ids remembered, so a late `send` gets the real reason.
pub const FINISHED_CALLS: usize = 64;
/// Longest agent error text passed back to the extension, in characters.
pub const MAX_AGENT_ERROR_CHARS: usize = 300;

/// Daily allowance for one level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Allowance {
    pub calls: u32,
    pub chars: u64,
}

pub const ALLOWANCE_LOW: Allowance = Allowance {
    calls: 10,
    chars: 100_000,
};
pub const ALLOWANCE_NORMAL: Allowance = Allowance {
    calls: 40,
    chars: 400_000,
};
pub const ALLOWANCE_HIGH: Allowance = Allowance {
    calls: 150,
    chars: 1_500_000,
};

/// The allowance for `level`. A manifest's `dailyCalls` only lowers
/// "Normal", and never below one call.
pub fn allowance(level: AgentAllowance, manifest_daily_calls: Option<u32>) -> Allowance {
    match level {
        AgentAllowance::Low => ALLOWANCE_LOW,
        AgentAllowance::High => ALLOWANCE_HIGH,
        AgentAllowance::Normal => Allowance {
            calls: manifest_daily_calls.map_or(ALLOWANCE_NORMAL.calls, |n| {
                n.clamp(1, ALLOWANCE_NORMAL.calls)
            }),
            chars: ALLOWANCE_NORMAL.chars,
        },
    }
}

/// The framed-prompt cap for an extension: the host's, or the manifest's
/// lower `maxPromptChars`.
pub fn prompt_cap(manifest_max_prompt_chars: Option<u32>) -> usize {
    manifest_max_prompt_chars.map_or(MAX_PROMPT_CHARS, |n| (n as usize).min(MAX_PROMPT_CHARS))
}

/// `max_response_chars` from a request, clamped to 1..=32,000.
pub fn response_cap(requested: Option<u32>) -> u32 {
    requested.map_or(DEFAULT_RESPONSE_CHARS, |n| n.clamp(1, MAX_RESPONSE_CHARS))
}

/// `timeout_ms` from a request, clamped to 5–180 s.
pub fn timeout_ms(requested: Option<u32>) -> u32 {
    requested.map_or(DEFAULT_TIMEOUT_MS, |n| {
        n.clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS)
    })
}

/// Bucket for the daily telemetry event: thousands of characters.
pub fn kchars_bucket(chars: u64) -> &'static str {
    match chars / 1000 {
        0..=9 => "<10",
        10..=49 => "<50",
        50..=199 => "<200",
        200..=999 => "<1000",
        _ => "1000+",
    }
}

// --- Time ----------------------------------------------------------------------

/// Where the engine gets the time. The system clock in the app; a fake in
/// tests, so cooldowns, local days and DST can be checked exactly.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
    /// The local UTC offset in effect at `t`.
    fn offset_at(&self, t: DateTime<Utc>) -> FixedOffset;
}

/// The real clock: `Utc::now()` and the OS time zone.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn offset_at(&self, t: DateTime<Utc>) -> FixedOffset {
        chrono::Local.offset_from_utc_datetime(&t.naive_utc()).fix()
    }
}

/// The local calendar date at `t`.
pub fn local_date(clock: &dyn Clock, t: DateTime<Utc>) -> NaiveDate {
    t.with_timezone(&clock.offset_at(t)).date_naive()
}

/// The first instant of the local day after `t`'s: when the daily
/// allowance resets. Works across DST changes, including a midnight the
/// change skips (the day then starts at the first instant that exists).
pub fn next_local_midnight(clock: &dyn Clock, t: DateTime<Utc>) -> DateTime<Utc> {
    let date = local_date(clock, t)
        .succ_opt()
        .unwrap_or_else(|| local_date(clock, t));
    let midnight = date.and_hms_opt(0, 0, 0).unwrap_or_default();
    let to_utc = |offset: FixedOffset| {
        let secs = i64::from(offset.local_minus_utc());
        DateTime::<Utc>::from_naive_utc_and_offset(midnight - Duration::seconds(secs), Utc)
    };
    // Each guess uses the offset in effect at the previous guess; two or
    // three rounds settle on either side of a DST change.
    let mut candidates = Vec::with_capacity(4);
    let mut guess = to_utc(clock.offset_at(t));
    for _ in 0..4 {
        candidates.push(guess);
        let next = to_utc(clock.offset_at(guess));
        if next == guess {
            break;
        }
        guess = next;
    }
    candidates
        .iter()
        .copied()
        .filter(|c| *c > t && local_date(clock, *c) == date)
        .min()
        .or_else(|| candidates.iter().copied().filter(|c| *c > t).max())
        .unwrap_or(t + Duration::days(1))
}

/// `t` as a local time for a message: `HH:MM` today, `YYYY-MM-DD HH:MM`
/// on another day.
pub fn format_local_time(clock: &dyn Clock, now: DateTime<Utc>, t: DateTime<Utc>) -> String {
    let local = t.with_timezone(&clock.offset_at(t));
    if local.date_naive() == local_date(clock, now) {
        local.format("%H:%M").to_string()
    } else {
        local.format("%Y-%m-%d %H:%M").to_string()
    }
}

/// RFC 3339 with the local offset, as `resets_at` and friends use.
pub fn rfc3339_local(clock: &dyn Clock, t: DateTime<Utc>) -> String {
    t.with_timezone(&clock.offset_at(t))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Mutex;

    /// A settable clock with an optional DST change: `before` applies to
    /// instants before `switch_at`, `after` from it on.
    pub struct FakeClock {
        now: Mutex<DateTime<Utc>>,
        before: FixedOffset,
        after: FixedOffset,
        switch_at: Option<DateTime<Utc>>,
    }

    impl FakeClock {
        pub fn new(now: DateTime<Utc>) -> Self {
            Self::with_offsets(now, FixedOffset::east_opt(0).unwrap(), None)
        }

        pub fn with_offsets(
            now: DateTime<Utc>,
            offset: FixedOffset,
            dst: Option<(DateTime<Utc>, FixedOffset)>,
        ) -> Self {
            Self {
                now: Mutex::new(now),
                before: offset,
                after: dst.map_or(offset, |(_, o)| o),
                switch_at: dst.map(|(t, _)| t),
            }
        }

        pub fn set(&self, t: DateTime<Utc>) {
            *self.now.lock().unwrap() = t;
        }

        pub fn advance_secs(&self, secs: i64) {
            let mut now = self.now.lock().unwrap();
            *now += Duration::seconds(secs);
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> DateTime<Utc> {
            *self.now.lock().unwrap()
        }

        fn offset_at(&self, t: DateTime<Utc>) -> FixedOffset {
            match self.switch_at {
                Some(s) if t >= s => self.after,
                _ => self.before,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeClock;
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn allowance_levels_and_manifest_clamp() {
        assert_eq!(allowance(AgentAllowance::Low, None), ALLOWANCE_LOW);
        assert_eq!(allowance(AgentAllowance::Normal, None), ALLOWANCE_NORMAL);
        assert_eq!(allowance(AgentAllowance::High, None), ALLOWANCE_HIGH);
        // dailyCalls lowers Normal only, never raises it.
        assert_eq!(allowance(AgentAllowance::Normal, Some(25)).calls, 25);
        assert_eq!(allowance(AgentAllowance::Normal, Some(500)).calls, 40);
        assert_eq!(allowance(AgentAllowance::Normal, Some(0)).calls, 1);
        assert_eq!(allowance(AgentAllowance::High, Some(5)), ALLOWANCE_HIGH);
        assert_eq!(allowance(AgentAllowance::Low, Some(500)), ALLOWANCE_LOW);
    }

    #[test]
    fn caps_and_clamps() {
        assert_eq!(prompt_cap(None), 32_000);
        assert_eq!(prompt_cap(Some(16_000)), 16_000);
        assert_eq!(prompt_cap(Some(99_000)), 32_000);
        assert_eq!(response_cap(None), 8_000);
        assert_eq!(response_cap(Some(50_000)), 32_000);
        assert_eq!(response_cap(Some(0)), 1);
        assert_eq!(timeout_ms(None), 60_000);
        assert_eq!(timeout_ms(Some(10)), 5_000);
        assert_eq!(timeout_ms(Some(999_999)), 180_000);
    }

    #[test]
    fn kchars_buckets() {
        assert_eq!(kchars_bucket(0), "<10");
        assert_eq!(kchars_bucket(9_999), "<10");
        assert_eq!(kchars_bucket(10_000), "<50");
        assert_eq!(kchars_bucket(199_999), "<200");
        assert_eq!(kchars_bucket(999_999), "<1000");
        assert_eq!(kchars_bucket(1_000_000), "1000+");
    }

    #[test]
    fn local_day_follows_the_offset() {
        // 23:30 UTC is already the next day at UTC+2.
        let plus2 = FixedOffset::east_opt(2 * 3600).unwrap();
        let clock = FakeClock::with_offsets(utc("2026-03-10T23:30:00Z"), plus2, None);
        let now = clock.now();
        assert_eq!(local_date(&clock, now).to_string(), "2026-03-11");
        assert_eq!(
            next_local_midnight(&clock, now),
            utc("2026-03-11T22:00:00Z")
        );
        assert_eq!(format_local_time(&clock, now, now), "01:30");
        assert_eq!(
            format_local_time(&clock, now, next_local_midnight(&clock, now)),
            "2026-03-12 00:00"
        );
        assert_eq!(rfc3339_local(&clock, now), "2026-03-11T01:30:00+02:00");
    }

    #[test]
    fn next_midnight_across_a_spring_forward() {
        // UTC-5 → UTC-4 at 07:00Z on 2026-03-08 (02:00 local). Midnight
        // of the 9th is at UTC-4: 04:00Z.
        let est = FixedOffset::west_opt(5 * 3600).unwrap();
        let edt = FixedOffset::west_opt(4 * 3600).unwrap();
        let clock = FakeClock::with_offsets(
            utc("2026-03-08T04:30:00Z"),
            est,
            Some((utc("2026-03-08T07:00:00Z"), edt)),
        );
        let now = clock.now();
        assert_eq!(local_date(&clock, now).to_string(), "2026-03-07");
        // Midnight of the 8th is still before the change: UTC-5.
        assert_eq!(
            next_local_midnight(&clock, now),
            utc("2026-03-08T05:00:00Z")
        );
        clock.set(utc("2026-03-08T12:00:00Z"));
        assert_eq!(
            next_local_midnight(&clock, clock.now()),
            utc("2026-03-09T04:00:00Z")
        );
    }

    #[test]
    fn next_midnight_when_the_change_skips_midnight() {
        // UTC-3 → UTC-2 at 03:00Z: local 00:00 jumps to 01:00, so the day
        // starts at 01:00 local, 03:00Z.
        let before = FixedOffset::west_opt(3 * 3600).unwrap();
        let after = FixedOffset::west_opt(2 * 3600).unwrap();
        let clock = FakeClock::with_offsets(
            utc("2026-10-31T20:00:00Z"),
            before,
            Some((utc("2026-11-01T03:00:00Z"), after)),
        );
        let reset = next_local_midnight(&clock, clock.now());
        assert_eq!(reset, utc("2026-11-01T03:00:00Z"));
        assert_eq!(local_date(&clock, reset).to_string(), "2026-11-01");
    }

    #[test]
    fn next_midnight_across_a_fall_back() {
        // UTC-4 → UTC-5 at 06:00Z on 2026-11-01 (02:00 local back to
        // 01:00). Midnight of the 2nd is at UTC-5: 05:00Z.
        let edt = FixedOffset::west_opt(4 * 3600).unwrap();
        let est = FixedOffset::west_opt(5 * 3600).unwrap();
        let clock = FakeClock::with_offsets(
            utc("2026-11-01T12:00:00Z"),
            edt,
            Some((utc("2026-11-01T06:00:00Z"), est)),
        );
        assert_eq!(
            next_local_midnight(&clock, clock.now()),
            utc("2026-11-02T05:00:00Z")
        );
    }
}
