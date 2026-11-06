//! Context switches, sessions and focus streaks, derived from the ordered
//! samples.
//!
//! The poll loop stores one `activity_log` row per tick (every poll
//! interval, ~5 s) naming the app in front. Those rows are samples, not
//! switches: counting them is counting ticks. The rules here:
//!
//! - A **streak** is an unbroken run of one app. Its length is the sum of its
//!   rows' credited seconds, the same seconds screen time adds up, so a
//!   streak can't exceed its app's total.
//! - A **switch** is a move from one app to a *different* app with no break
//!   between the two samples. The first app of a pass, and the first app
//!   after a break, start a streak without being a switch, because nothing
//!   was switched away from. The report's `context_switches` counts these.
//! - A **session** is one streak, however it started. Per app,
//!   `switches_to` counts sessions: how many separate times the app was in
//!   front. That's what the Focus Tracker shows as "N sessions". It includes
//!   the fresh starts, so a day spent in one app is one session and no
//!   switches, and the sum of every app's `switches_to` is
//!   `context_switches` plus the number of fresh starts.
//! - **Transparent** samples (see [`sample_kind`]) are seen through:
//!   A → Kage → A is one run of A and no switch; A → Kage → B is a single
//!   switch, into B. They add nothing to any streak. While Kage's own
//!   windows or transient system UI are in front, the poll loop stores a
//!   marker row each tick (empty process name, 0 s; see
//!   `activity_tracker::row_for_tick`), so the counter can see that the
//!   machine was awake and in use the whole time.
//! - A **break** ends the current streak, and the next app starts a new
//!   one without a switch. A break is a lock screen or screen saver
//!   sample, a gap between consecutive samples of more than [`GAP_FACTOR`]
//!   poll intervals (sleep, Kage not running, tracker stopped, or no
//!   window in front, which on Windows includes the locked desktop), or a
//!   row credited more than [`MAX_ROW_SECS`] (an older build's resume
//!   from sleep).

use chrono::{DateTime, FixedOffset};
use rusqlite::Connection;
use std::collections::HashMap;

/// Consecutive samples further apart than this many poll intervals have a
/// break between them. Every awake tick stores a row, apart from ticks
/// with no window in front, so rows are normally one interval apart. A
/// larger gap means ticks went missing. It's the same threshold
/// `credited_secs` uses to treat a single tick as a wake from sleep.
pub(super) const GAP_FACTOR: u64 = 3;

/// The most seconds a current build credits one row: `credited_secs` caps
/// a tick at two poll intervals, and `start_tracker` caps the interval.
/// Builds before that cap credited the whole time since the previous tick,
/// and on Windows `Instant` keeps counting through sleep, so the first row
/// after a night's sleep can hold ~54,000 s. A row over this limit is read
/// the way `credited_secs` now treats a resume: a break before it, and
/// one poll interval of credit.
pub(super) const MAX_ROW_SECS: u64 = 2 * super::MAX_POLL_INTERVAL_SECS;

/// What a stored sample means for switch / streak counting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SampleKind {
    /// An app the user was working in.
    App,
    /// Seen through: Kage's own windows, an empty process name (the marker
    /// rows current builds store for Kage and system UI), transient system
    /// UI. Older rows may also name Kage or the system UI process.
    Transparent,
    /// The user stepped away: lock screen or screen saver. Ends the streak
    /// without being a switch. Its time still counts in the totals (as
    /// "Screen Locked" on Windows), as it always has.
    Away,
}

pub(super) fn sample_kind(process_name: &str) -> SampleKind {
    let name = process_name.trim();
    if name.is_empty() || is_own_process_name(name) || super::is_system_noise(name) {
        SampleKind::Transparent
    } else if is_away_screen(name) {
        SampleKind::Away
    } else {
        SampleKind::App
    }
}

/// True for Kage's own process, by the name each platform reports for it.
/// Exact match, ignoring case, on `kage`:
///
/// - **Windows**: the executable's file stem from
///   `QueryFullProcessImageNameW`, which is `kage` for `kage.exe`. The
///   platform layer finds our windows by PID instead, and the poll loop
///   stores a marker for them. This name check catches rows that older
///   builds stored when they matched by window title.
/// - **macOS**: `NSRunningApplication.localizedName`. That's `Kage` (the
///   `productName`) for the app bundle, and the executable name `kage` for
///   an unbundled `cargo tauri dev` binary. The platform layer's
///   case-sensitive `contains("Kage")` misses the second.
/// - **Linux**: `/proc/<pid>/comm`, which is `kage` (foreground lookup isn't
///   implemented there yet).
///
/// It doesn't use `contains`, so an unrelated app whose name merely
/// includes the word isn't treated as Kage. The MCP sidecar
/// (`kage-computer-control-mcp`) never owns a foreground window.
pub(super) fn is_own_process_name(process_name: &str) -> bool {
    process_name.trim().eq_ignore_ascii_case("kage")
}

/// Lock screen / screen saver hosts: `LockApp` (Windows lock screen),
/// `loginwindow` (macOS lock screen and fast user switching),
/// `ScreenSaverEngine` (macOS screen saver).
fn is_away_screen(process_name: &str) -> bool {
    matches!(
        process_name.to_lowercase().as_str(),
        "lockapp" | "loginwindow" | "screensaverengine"
    )
}

/// One unbroken run of a single app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Streak {
    /// The process name exactly as stored, so it keys the same way as the
    /// per-app totals.
    pub app: String,
    /// `YYYY-MM-DD` prefix of the first sample's timestamp. This is the
    /// same day key the daily rollup uses.
    pub day: String,
    /// Sum of the run's credited seconds.
    pub secs: u64,
    /// Reached by switching from a different app. False for the first app
    /// of a pass and for the first app after a break.
    pub switched_in: bool,
}

/// Turns samples (in insertion order) into [`Streak`]s.
pub(super) struct StreakCounter {
    poll_interval: u64,
    /// Timestamp of the previous sample of any kind. Transparent samples
    /// count too, as evidence that the machine was awake.
    last_seen: Option<DateTime<FixedOffset>>,
    current: Option<Streak>,
}

impl StreakCounter {
    pub fn new(poll_interval: u64) -> Self {
        Self {
            poll_interval: poll_interval.max(1),
            last_seen: None,
            current: None,
        }
    }

    /// Feed the next sample. Returns the streak this sample ended, if any.
    /// A sample ends at most one streak.
    pub fn push(&mut self, timestamp: &str, process_name: &str, secs: u64) -> Option<Streak> {
        let at = DateTime::parse_from_rfc3339(timestamp).ok();
        // An older build's resume row: the credit is the sleep, not time in
        // the app. See MAX_ROW_SECS.
        let resumed = secs > MAX_ROW_SECS;
        let secs = if resumed {
            secs.min(self.poll_interval)
        } else {
            secs
        };
        let mut ended = if resumed || self.is_gap(at, secs) {
            self.current.take()
        } else {
            None
        };
        // An unparseable timestamp leaves the next gap unknown rather than
        // measured against an older sample: no timestamp, no break.
        self.last_seen = at;

        match sample_kind(process_name) {
            SampleKind::Transparent => {}
            SampleKind::Away => ended = ended.or_else(|| self.current.take()),
            SampleKind::App => {
                if let Some(current) = self.current.as_mut().filter(|c| c.app == process_name) {
                    current.secs += secs;
                } else {
                    // A different app while one is open is a switch. With
                    // none open (first sample, or after a break) it isn't.
                    let previous = self.current.take();
                    self.current = Some(Streak {
                        app: process_name.to_string(),
                        day: day_key(timestamp).to_string(),
                        secs,
                        switched_in: previous.is_some(),
                    });
                    ended = ended.or(previous);
                }
            }
        }
        ended
    }

    /// The streak still open after the last sample.
    pub fn finish(mut self) -> Option<Streak> {
        self.current.take()
    }

    /// Whether the time since the previous sample is long enough to call a
    /// break. The limit uses the larger of the current poll interval and the
    /// seconds this row was credited with (at most [`MAX_ROW_SECS`]),
    /// because each row's credit is roughly the interval in force when it
    /// was written. Rows from a slower interval than the current one aren't
    /// each read as a break. A clock
    /// that went backwards also counts as a break, since the samples on
    /// either side can't be shown to be continuous.
    fn is_gap(&self, at: Option<DateTime<FixedOffset>>, secs: u64) -> bool {
        let (Some(prev), Some(at)) = (self.last_seen, at) else {
            return false;
        };
        let limit = GAP_FACTOR * self.poll_interval.max(secs);
        let gap = at.signed_duration_since(prev).num_seconds();
        gap < 0 || gap as u64 > limit
    }
}

/// `YYYY-MM-DD` of an RFC 3339 timestamp, matching `substr(timestamp, 1, 10)`.
fn day_key(timestamp: &str) -> &str {
    timestamp.get(..10).unwrap_or(timestamp)
}

/// Switch, session and streak figures for one pass over the samples.
#[derive(Debug, Default)]
pub(super) struct SwitchSummary {
    /// Streaks reached by switching from a different app.
    pub context_switches: u64,
    /// Streaks per app (sessions), keyed by stored process name.
    pub sessions: HashMap<String, u64>,
    pub longest_streak_secs: u64,
    pub longest_streak_app: String,
}

impl SwitchSummary {
    pub fn add(&mut self, streak: Streak) {
        if streak.switched_in {
            self.context_switches += 1;
        }
        *self.sessions.entry(streak.app.clone()).or_default() += 1;
        if streak.secs > self.longest_streak_secs {
            self.longest_streak_secs = streak.secs;
            self.longest_streak_app = streak.app;
        }
    }
}

/// Stream `activity_log` rows through a [`StreakCounter`] and hand every
/// streak to `on_streak`. `filter` is the `WHERE` condition, with `?1`
/// bound to `param`. Rows are read in insertion (`id`) order. That is the
/// order they were sampled in, and unlike timestamp strings it survives
/// a DST offset change or a clock adjustment.
pub(super) fn for_each_streak(
    conn: &Connection,
    filter: &str,
    param: &str,
    poll_interval: u64,
    mut on_streak: impl FnMut(Streak),
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!(
        "SELECT timestamp, process_name, duration_secs FROM activity_log
         WHERE {filter} ORDER BY id"
    ))?;
    let rows = stmt.query_map(rusqlite::params![param], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, u64>(2)?,
        ))
    })?;
    let mut counter = StreakCounter::new(poll_interval);
    for row in rows {
        let (timestamp, process_name, secs) = row?;
        if let Some(streak) = counter.push(&timestamp, &process_name, secs) {
            on_streak(streak);
        }
    }
    if let Some(streak) = counter.finish() {
        on_streak(streak);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples at `t` seconds past a fixed instant, credited `secs` each.
    fn summarize(samples: &[(i64, &str, u64)], poll_interval: u64) -> SwitchSummary {
        let mut summary = SwitchSummary::default();
        for s in streaks(samples, poll_interval) {
            summary.add(s);
        }
        summary
    }

    fn streaks(samples: &[(i64, &str, u64)], poll_interval: u64) -> Vec<Streak> {
        let mut counter = StreakCounter::new(poll_interval);
        let mut out: Vec<Streak> = samples
            .iter()
            .filter_map(|(t, p, s)| counter.push(&ts(*t), p, *s))
            .collect();
        out.extend(counter.finish());
        out
    }

    fn ts(offset_secs: i64) -> String {
        let base = DateTime::parse_from_rfc3339("2026-03-02T09:00:00+01:00").unwrap();
        (base + chrono::Duration::seconds(offset_secs)).to_rfc3339()
    }

    /// Evenly spaced 5 s samples of the given processes.
    fn every_5s<'a>(processes: &[&'a str]) -> Vec<(i64, &'a str, u64)> {
        processes
            .iter()
            .enumerate()
            .map(|(i, p)| (i as i64 * 5, *p, 5))
            .collect()
    }

    /// Switches into `app`: its streaks that were reached from another app.
    fn switches_into(streaks: &[Streak], app: &str) -> usize {
        streaks
            .iter()
            .filter(|s| s.app == app && s.switched_in)
            .count()
    }

    fn sessions(s: &SwitchSummary, app: &str) -> u64 {
        s.sessions.get(app).copied().unwrap_or(0)
    }

    #[test]
    fn first_app_is_not_a_switch_but_is_a_session() {
        let s = summarize(&every_5s(&["code", "code", "code"]), 5);
        assert_eq!(s.context_switches, 0);
        assert_eq!(sessions(&s, "code"), 1);
        assert_eq!(s.longest_streak_secs, 15);
        assert_eq!(s.longest_streak_app, "code");
    }

    #[test]
    fn samples_are_not_switches() {
        // Ten ticks in one app plus a single move to another and back:
        // two switches, not twelve.
        let mut seq = vec!["code"; 10];
        seq.extend(["slack", "code"]);
        let st = streaks(&every_5s(&seq), 5);
        assert_eq!(switches_into(&st, "slack"), 1);
        assert_eq!(switches_into(&st, "code"), 1);
        let s = summarize(&every_5s(&seq), 5);
        assert_eq!(s.context_switches, 2);
        assert_eq!(sessions(&s, "code"), 2);
        assert_eq!(sessions(&s, "slack"), 1);
        assert_eq!(s.longest_streak_secs, 50);
    }

    #[test]
    fn sessions_are_switches_plus_fresh_starts() {
        // a b a c c b a (kage) a, then a break, then b.
        let mut samples = every_5s(&["a", "b", "a", "c", "c", "b", "a", "kage", "a"]);
        samples.push((600, "b", 5));
        let s = summarize(&samples, 5);
        assert_eq!(s.context_switches, 5);
        // Two fresh starts: the first sample, and b after the gap.
        assert_eq!(s.sessions.values().sum::<u64>(), s.context_switches + 2);
        assert_eq!(sessions(&s, "a"), 3);
        assert_eq!(sessions(&s, "b"), 3);
        assert_eq!(sessions(&s, "c"), 1);
    }

    #[test]
    fn same_app_after_a_lock_is_a_second_session() {
        let s = summarize(&every_5s(&["code", "code", "lockapp", "code"]), 5);
        assert_eq!(s.context_switches, 0);
        assert_eq!(sessions(&s, "code"), 2);
    }

    #[test]
    fn kage_in_between_is_transparent() {
        for kage in ["Kage", "kage", "KAGE", " kage ", ""] {
            let s = summarize(&every_5s(&["code", kage, kage, "code"]), 5);
            assert_eq!(s.context_switches, 0, "A→{kage:?}→A must not switch");
            assert_eq!(sessions(&s, "code"), 1, "{kage:?}");
            // One streak of code; Kage's own seconds aren't credited to it.
            assert_eq!(s.longest_streak_secs, 10);
            assert_eq!(sessions(&s, kage), 0);
        }
    }

    #[test]
    fn kage_between_two_apps_is_one_switch() {
        let st = streaks(&every_5s(&["code", "kage", "chrome"]), 5);
        assert_eq!(st.len(), 2);
        assert_eq!(switches_into(&st, "chrome"), 1);
        assert_eq!(switches_into(&st, "code"), 0);
    }

    /// A minute in Kage's chat, as current builds store it: one 0 s marker
    /// row (empty process name) per tick.
    fn with_a_minute_of_kage_markers<'a>(
        before: &'a str,
        after: &'a str,
    ) -> Vec<(i64, &'a str, u64)> {
        let mut samples = vec![(0, before, 5), (5, before, 5)];
        samples.extend((2..=13).map(|i| (i * 5, "", 0)));
        samples.extend([(70, after, 5), (75, after, 5)]);
        samples
    }

    #[test]
    fn a_minute_in_kage_between_two_apps_is_one_switch() {
        let st = streaks(&with_a_minute_of_kage_markers("code", "chrome"), 5);
        assert_eq!(st.len(), 2);
        assert_eq!(switches_into(&st, "chrome"), 1);
        assert_eq!(st[0].secs, 10);
        assert_eq!(st[1].secs, 10);
    }

    #[test]
    fn a_minute_in_kage_keeps_the_streak_whole() {
        let st = streaks(&with_a_minute_of_kage_markers("code", "code"), 5);
        assert_eq!(st.len(), 1);
        // The markers add no seconds: 2 rows of code either side.
        assert_eq!(st[0].secs, 20);
    }

    #[test]
    fn a_minute_with_nothing_stored_is_still_a_break() {
        // No markers: no window in front (on Windows, the locked desktop).
        let s = summarize(&[(0, "code", 5), (5, "code", 5), (65, "chrome", 5)], 5);
        assert_eq!(s.context_switches, 0);
    }

    #[test]
    fn empty_and_system_ui_samples_are_transparent() {
        let s = summarize(
            &every_5s(&[
                "code",
                "",
                "  ",
                "SearchHost",
                "ShellExperienceHost",
                "code",
            ]),
            5,
        );
        assert_eq!(s.context_switches, 0);
        assert_eq!(s.longest_streak_secs, 10);
    }

    #[test]
    fn own_process_name_is_an_exact_match() {
        assert!(is_own_process_name("Kage"));
        assert!(is_own_process_name("kage"));
        assert!(!is_own_process_name("kagetool"));
        assert!(!is_own_process_name("kage-computer-control-mcp"));
        assert_eq!(sample_kind("kagetool"), SampleKind::App);
    }

    #[test]
    fn long_gap_ends_the_streak_without_a_switch() {
        // Same app either side of a 10-minute sleep.
        let s = streaks(&[(0, "code", 5), (5, "code", 5), (605, "code", 5)], 5);
        assert_eq!(s.len(), 2);
        assert!(s.iter().all(|st| !st.switched_in));
        assert_eq!(s[0].secs, 10);
        assert_eq!(s[1].secs, 5);
    }

    #[test]
    fn different_app_after_a_gap_starts_fresh() {
        let st = streaks(&[(0, "code", 5), (5, "code", 5), (605, "slack", 5)], 5);
        assert_eq!(switches_into(&st, "slack"), 0);
        let s = summarize(&[(0, "code", 5), (5, "code", 5), (605, "slack", 5)], 5);
        assert_eq!(s.context_switches, 0);
        assert_eq!(sessions(&s, "slack"), 1);
    }

    #[test]
    fn gap_limit_is_three_poll_intervals() {
        // 15 s apart (two ticks with no window in front) is bridged.
        // 20 s apart is a break.
        let bridged = summarize(&[(0, "code", 5), (15, "slack", 5)], 5);
        assert_eq!(bridged.context_switches, 1);
        let broken = summarize(&[(0, "code", 5), (20, "slack", 5)], 5);
        assert_eq!(broken.context_switches, 0);
    }

    #[test]
    fn stored_transparent_rows_keep_the_timeline_continuous() {
        // A minute of Kage rows from an older build: no gap between samples,
        // so code's streak isn't broken and coming back isn't a switch.
        let mut samples = vec![(0, "code", 5)];
        samples.extend((1..=12).map(|i| (i * 5, "kage", 5)));
        samples.push((65, "code", 5));
        let s = streaks(&samples, 5);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].secs, 10);
    }

    #[test]
    fn gap_limit_follows_the_rows_own_interval() {
        // Rows written at a 30 s interval, read with the default 5 s one:
        // 30 s spacing is normal for them, not a break.
        let s = summarize(&[(0, "code", 30), (30, "slack", 30), (60, "code", 30)], 5);
        assert_eq!(s.context_switches, 2);
        // The slowest interval current builds write: 60 s, credited up to
        // 120 s.
        let s = summarize(
            &[
                (0, "code", 60),
                (120, "slack", MAX_ROW_SECS),
                (180, "code", 60),
            ],
            5,
        );
        assert_eq!(s.context_switches, 2);
    }

    #[test]
    fn overnight_credit_from_an_older_build_is_a_break_not_a_streak() {
        // Before the credit cap, the first row after a night's sleep on
        // Windows held the whole night. Its own credit used to stretch the
        // gap limit far enough to bridge the sleep.
        let st = streaks(
            &[
                (0, "code", 5),
                (5, "code", 5),
                (54_005, "code", 54_000),
                (54_010, "code", 5),
            ],
            5,
        );
        assert_eq!(st.len(), 2);
        assert!(st.iter().all(|s| !s.switched_in));
        assert_eq!(st[0].secs, 10);
        // One poll interval for the resume row, plus the next row.
        assert_eq!(st[1].secs, 10);
    }

    #[test]
    fn overnight_credit_ends_the_streak_even_without_a_timestamp_gap() {
        // The break comes from the credit alone, the way credited_secs
        // reads a resume.
        let s = summarize(&[(0, "code", 5), (5, "slack", 54_000)], 5);
        assert_eq!(s.context_switches, 0);
        assert_eq!(s.longest_streak_secs, 5);
        assert_eq!(sessions(&s, "slack"), 1);
    }

    #[test]
    fn lock_screen_ends_the_streak_without_switches() {
        for lock in ["LockApp", "lockapp", "loginwindow", "ScreenSaverEngine"] {
            let s = streaks(&every_5s(&["code", "code", lock, lock, "code"]), 5);
            assert_eq!(s.len(), 2, "{lock}");
            assert!(s.iter().all(|st| !st.switched_in && st.app == "code"));
            let s = summarize(&every_5s(&["code", lock, "slack"]), 5);
            assert_eq!(s.context_switches, 0, "{lock}");
            assert_eq!(sessions(&s, lock), 0);
        }
    }

    #[test]
    fn lock_screen_is_never_the_longest_streak() {
        let s = summarize(&every_5s(&["code", "lockapp", "lockapp", "lockapp"]), 5);
        assert_eq!(s.longest_streak_app, "code");
        assert_eq!(s.longest_streak_secs, 5);
    }

    #[test]
    fn streak_length_is_credited_seconds() {
        let s = summarize(&[(0, "code", 5), (5, "code", 7), (12, "code", 3)], 5);
        assert_eq!(s.longest_streak_secs, 15);
    }

    #[test]
    fn clock_going_backwards_is_a_break() {
        let s = summarize(&[(100, "code", 5), (0, "slack", 5)], 5);
        assert_eq!(s.context_switches, 0);
    }

    #[test]
    fn unparseable_timestamps_fall_back_to_order_only() {
        let mut counter = StreakCounter::new(5);
        let mut summary = SwitchSummary::default();
        for p in ["code", "slack", "code"] {
            if let Some(s) = counter.push("not a timestamp", p, 5) {
                summary.add(s);
            }
        }
        summary.add(counter.finish().unwrap());
        assert_eq!(summary.context_switches, 2);
    }

    #[test]
    fn streak_is_dated_by_its_first_sample() {
        let base = DateTime::parse_from_rfc3339("2026-03-02T23:59:55+00:00").unwrap();
        let mut counter = StreakCounter::new(5);
        assert!(counter.push(&base.to_rfc3339(), "code", 5).is_none());
        let next = base + chrono::Duration::seconds(5);
        assert!(counter.push(&next.to_rfc3339(), "code", 5).is_none());
        let streak = counter.finish().unwrap();
        assert_eq!(streak.day, "2026-03-02");
        assert_eq!(streak.secs, 10);
    }
}
