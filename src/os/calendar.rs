// Cross-platform calendar integration.
//
// Uses OS-native calendar APIs:
// - Windows: Windows.ApplicationModel.Appointments
// - macOS: EventKit via the `kage-calendar-helper` sidecar (a Rust
//          workspace member provisioned by build.rs), with icalBuddy
//          as a fallback
// - Linux: stub (no standard API)
//
// The pure types live in kage-core so the kage-calendar-helper sidecar
// shares the exact wire struct + meeting-URL sniffing; re-exported here
// so app code keeps using `crate::os::calendar::CalendarEvent`.

use chrono::{DateTime, Duration, NaiveDate, TimeZone};
use std::collections::BTreeMap;

pub use kage_core::calendar::CalendarEvent;
// Only the Windows and macOS backends sniff meeting URLs; Linux is a stub.
#[cfg(not(target_os = "linux"))]
pub use kage_core::calendar::extract_meeting_url;

/// Get upcoming calendar events within the next `hours` hours.
///
/// Returns `Err` when the calendar backend reports a hard failure (e.g.
/// permission denied on macOS). An empty `Ok(vec![])` means "no events
/// found" — callers should distinguish the two in the UI.
pub fn get_upcoming_events(hours: u32) -> Result<Vec<CalendarEvent>, String> {
    crate::os::platform::calendar::get_upcoming_events_impl(hours)
}

/// Get calendar events for a specific date (YYYY-MM-DD).
pub fn get_events_for_date(date: &str) -> Result<Vec<CalendarEvent>, String> {
    crate::os::platform::calendar::get_events_for_date_impl(date)
}

/// Why a range query failed. Typed (rather than the free-form `String` the
/// older entry points return) so the command layer can tell "the user never
/// granted Calendar access" — which the date picker turns into a hint — apart
/// from a backend that is merely broken or absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarQueryError {
    /// The OS refused calendar access (macOS TCC denial, or the prompt
    /// timed out unanswered).
    PermissionDenied,
    /// Any other backend failure. English, for logs only. (macOS and Linux
    /// degrade to "no events" instead — only the Outlook path reports this.)
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Failed(String),
}

/// Events overlapping the local-time day range `[start, end)` (both local
/// midnights). Backs `get_calendar_busy_days`; the events never leave the
/// Rust side — the command reduces them to per-day counts first.
pub fn get_events_in_range(
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<CalendarEvent>, CalendarQueryError> {
    crate::os::platform::calendar::get_events_in_range_impl(start, end)
}

// ---------------------------------------------------------------------------
// Day-range math for the month view (pure; unit-tested below)
// ---------------------------------------------------------------------------

/// Longest `[start, end)` span `get_calendar_busy_days` accepts. A month grid
/// shows at most 6 weeks (42 days); the slack covers a caller that pads the
/// range, while still bounding how much calendar one call can scan.
pub const MAX_RANGE_DAYS: i64 = 62;

/// Why a day range was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayRangeError {
    /// Not a strict, real `YYYY-MM-DD` date (e.g. `2026-02-30`, `2026/02/01`).
    Malformed,
    /// `end` is not after `start`.
    Empty,
    /// Wider than [`MAX_RANGE_DAYS`].
    TooLong,
}

/// Parse and validate a `[start, end)` day range given as `YYYY-MM-DD`
/// strings. The shape check is the same strict one `get_calendar_events_for_date`
/// uses (the Windows backend interpolates these into PowerShell), followed by
/// a real calendar-date parse so `2026-02-30` is rejected rather than rolled.
pub fn parse_day_range(start: &str, end: &str) -> Result<(NaiveDate, NaiveDate), DayRangeError> {
    let parse = |s: &str| -> Result<NaiveDate, DayRangeError> {
        if !is_strict_iso_date_shape(s) {
            return Err(DayRangeError::Malformed);
        }
        NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| DayRangeError::Malformed)
    };
    let (start, end) = (parse(start)?, parse(end)?);
    let span = (end - start).num_days();
    if span <= 0 {
        return Err(DayRangeError::Empty);
    }
    if span > MAX_RANGE_DAYS {
        return Err(DayRangeError::TooLong);
    }
    Ok((start, end))
}

/// Exactly `DDDD-DD-DD`, ASCII digits only.
fn is_strict_iso_date_shape(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// Count, for every local day in `[start, end)`, how many events touch it.
///
/// Semantics match what a click on that day shows (`get_events_for_date`
/// returns every event overlapping the day), so a dot never promises a
/// meeting the list then doesn't have:
///
/// - **Timed events** cover each local day their `[start, start + duration)`
///   interval overlaps. The interval is measured in absolute time and only
///   then mapped into `tz`, so an event spanning a DST change lands on the
///   right days. A zero-length event counts on its start day.
/// - **All-day events** are counted by calendar date, not by elapsed time:
///   `round(duration / 24h)` days from the start date (at least one). An
///   all-day event on a DST-change day lasts 23h or 25h of wall time, which
///   interval maths would round onto a second day.
/// - Events whose `start_time` doesn't parse are skipped — there's no day to
///   put them on.
///
/// Days with no events are absent from the map. Iteration per event is
/// clamped to the requested range, so a years-long recurring block can't
/// make this loop long.
pub fn count_events_per_day<Tz: TimeZone>(
    events: &[CalendarEvent],
    start: NaiveDate,
    end: NaiveDate,
    tz: &Tz,
) -> BTreeMap<NaiveDate, u32> {
    let mut counts = BTreeMap::new();
    for event in events {
        let Some((first, last)) = event_day_span(event, tz) else {
            continue;
        };
        let mut day = first.max(start);
        let stop = last.min(end.pred_opt().unwrap_or(end));
        while day <= stop {
            *counts.entry(day).or_insert(0) += 1;
            let Some(next) = day.succ_opt() else { break };
            day = next;
        }
    }
    counts
}

/// The first and last local day (inclusive) an event occupies in `tz`.
fn event_day_span<Tz: TimeZone>(event: &CalendarEvent, tz: &Tz) -> Option<(NaiveDate, NaiveDate)> {
    let start = DateTime::parse_from_rfc3339(event.start_time.trim()).ok()?;
    let first = start.with_timezone(tz).date_naive();
    if event.all_day {
        // Whole days by date (see the doc comment above for why not elapsed
        // time). 1439 min (EventKit's 23:59:59 end) and 1440 both round to 1.
        let days = ((f64::from(event.duration_minutes) / 1440.0).round() as i64).max(1);
        return Some((first, first + Duration::days(days - 1)));
    }
    if event.duration_minutes == 0 {
        return Some((first, first));
    }
    // The interval is half-open: an event ending exactly at midnight does
    // not spill onto the next day.
    let end = start + Duration::minutes(i64::from(event.duration_minutes));
    let last = (end - Duration::seconds(1)).with_timezone(tz).date_naive();
    Some((first, last.max(first)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn ev(start_time: &str, duration_minutes: u32, all_day: bool) -> CalendarEvent {
        CalendarEvent {
            id: format!("fake:{start_time}"),
            subject: "Fake event".into(),
            location: String::new(),
            organizer: String::new(),
            start_time: start_time.into(),
            duration_minutes,
            all_day,
            online_url: None,
        }
    }

    fn counts(
        events: &[CalendarEvent],
        start: &str,
        end: &str,
        tz: &FixedOffset,
    ) -> Vec<(String, u32)> {
        count_events_per_day(events, d(start), d(end), tz)
            .into_iter()
            .map(|(k, v)| (k.format("%Y-%m-%d").to_string(), v))
            .collect()
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    #[test]
    fn parse_day_range_accepts_a_month_grid() {
        assert_eq!(
            parse_day_range("2026-10-26", "2026-12-07"),
            Ok((d("2026-10-26"), d("2026-12-07")))
        );
        // Exactly the cap is fine; one more is not.
        assert!(parse_day_range("2026-01-01", "2026-03-04").is_ok());
        assert_eq!(
            parse_day_range("2026-01-01", "2026-03-05"),
            Err(DayRangeError::TooLong)
        );
    }

    #[test]
    fn parse_day_range_rejects_bad_shapes_and_impossible_dates() {
        for bad in ["2026-2-01", "2026/02/01", "2026-02-01'", "20260201", "", "２０２６-02-01"] {
            assert_eq!(
                parse_day_range(bad, "2026-03-01"),
                Err(DayRangeError::Malformed),
                "{bad:?}"
            );
        }
        assert_eq!(
            parse_day_range("2026-02-30", "2026-03-10"),
            Err(DayRangeError::Malformed)
        );
        // Leap day only exists in leap years.
        assert!(parse_day_range("2028-02-29", "2028-03-01").is_ok());
        assert_eq!(
            parse_day_range("2027-02-29", "2027-03-01"),
            Err(DayRangeError::Malformed)
        );
    }

    #[test]
    fn parse_day_range_rejects_empty_and_reversed_ranges() {
        assert_eq!(
            parse_day_range("2026-05-01", "2026-05-01"),
            Err(DayRangeError::Empty)
        );
        assert_eq!(
            parse_day_range("2026-05-02", "2026-05-01"),
            Err(DayRangeError::Empty)
        );
    }

    #[test]
    fn counts_timed_events_on_their_local_day() {
        let events = [
            ev("2026-11-05T09:00:00Z", 30, false),
            ev("2026-11-05T15:00:00Z", 60, false),
            ev("2026-11-07T23:30:00Z", 15, false),
        ];
        assert_eq!(
            counts(&events, "2026-11-01", "2026-12-01", &utc()),
            vec![("2026-11-05".into(), 2), ("2026-11-07".into(), 1)]
        );
    }

    #[test]
    fn local_timezone_moves_events_across_midnight() {
        // 23:30 UTC is already the next day in UTC+2 and still the same day
        // in UTC-7 — the dot has to follow the user's wall clock.
        let events = [ev("2026-11-07T23:30:00Z", 15, false)];
        let east = FixedOffset::east_opt(2 * 3600).unwrap();
        let west = FixedOffset::west_opt(7 * 3600).unwrap();
        assert_eq!(
            counts(&events, "2026-11-01", "2026-12-01", &east),
            vec![("2026-11-08".into(), 1)]
        );
        assert_eq!(
            counts(&events, "2026-11-01", "2026-12-01", &west),
            vec![("2026-11-07".into(), 1)]
        );
    }

    #[test]
    fn overnight_event_counts_on_both_days_but_midnight_end_does_not_spill() {
        let events = [
            // 22:00 → 02:00 next day: touches two days.
            ev("2026-11-10T22:00:00Z", 240, false),
            // 23:00 → 00:00 exactly: half-open, one day only.
            ev("2026-11-12T23:00:00Z", 60, false),
        ];
        assert_eq!(
            counts(&events, "2026-11-01", "2026-12-01", &utc()),
            vec![
                ("2026-11-10".into(), 1),
                ("2026-11-11".into(), 1),
                ("2026-11-12".into(), 1)
            ]
        );
    }

    #[test]
    fn zero_length_and_unparseable_events() {
        let events = [
            ev("2026-11-03T12:00:00Z", 0, false),
            ev("not a date", 30, false),
            ev("", 30, false),
        ];
        assert_eq!(
            counts(&events, "2026-11-01", "2026-12-01", &utc()),
            vec![("2026-11-03".into(), 1)]
        );
    }

    #[test]
    fn all_day_events_count_by_date_not_elapsed_time() {
        let tz = FixedOffset::east_opt(3600).unwrap();
        let events = [
            // EventKit-style: local midnight start, 23:59:59 end.
            ev("2026-03-28T23:00:00Z", 1439, true),
            // A DST spring-forward day is 23h of wall time…
            ev("2026-03-29T23:00:00Z", 1380, true),
            // …and fall-back is 25h. Neither may spill onto the next day.
            ev("2026-10-24T23:00:00Z", 1500, true),
        ];
        assert_eq!(
            counts(&events, "2026-03-01", "2026-04-01", &tz),
            vec![("2026-03-29".into(), 1), ("2026-03-30".into(), 1)]
        );
        assert_eq!(
            counts(&events, "2026-10-01", "2026-11-01", &tz),
            vec![("2026-10-25".into(), 1)]
        );
    }

    #[test]
    fn timed_event_across_a_dst_shift_uses_absolute_time() {
        // A 3h event starting 00:30 local on the night clocks go back.
        // Fixed offsets can't model the shift itself, but the maths is done
        // on absolute instants, so the event stays on the one day it covers.
        let tz = FixedOffset::east_opt(3600).unwrap();
        let events = [ev("2026-10-24T23:30:00Z", 180, false)];
        assert_eq!(
            counts(&events, "2026-10-01", "2026-11-01", &tz),
            vec![("2026-10-25".into(), 1)]
        );
    }

    #[test]
    fn multi_day_events_are_clamped_to_the_range() {
        let events = [
            // A week-long all-day block starting before the visible grid.
            ev("2026-10-29T00:00:00Z", 7 * 1440, true),
            // A long timed block running past the end of the range.
            ev("2026-11-29T10:00:00Z", 5 * 1440, false),
        ];
        assert_eq!(
            counts(&events, "2026-11-01", "2026-12-01", &utc()),
            vec![
                ("2026-11-01".into(), 1),
                ("2026-11-02".into(), 1),
                ("2026-11-03".into(), 1),
                ("2026-11-04".into(), 1),
                ("2026-11-29".into(), 1),
                ("2026-11-30".into(), 1),
            ]
        );
    }

    #[test]
    fn events_wholly_outside_the_range_are_ignored() {
        let events = [
            ev("2026-10-31T10:00:00Z", 30, false),
            ev("2026-12-01T00:00:00Z", 30, false),
        ];
        assert!(counts(&events, "2026-11-01", "2026-12-01", &utc()).is_empty());
    }

    #[test]
    fn leap_day_is_a_real_day() {
        let events = [ev("2028-02-29T09:00:00Z", 30, false)];
        assert_eq!(
            counts(&events, "2028-02-01", "2028-03-01", &utc()),
            vec![("2028-02-29".into(), 1)]
        );
    }
}
