// macOS calendar integration.
//
// macOS has two canonical paths to read calendar events:
//
// 1. **EventKit** (Apple's official API) — proper but requires a Swift
//    CLI helper bundled alongside kage-computer-control-mcp plus
//    `NSCalendarsUsageDescription` in Info.plist + runtime permission
//    prompt via TCC. That's a full feature with its own design space;
//    tracked in MACMIGRATION.md as a follow-up.
//
// 2. **icalBuddy** (third-party Homebrew tool) — if the user already
//    has it installed (`brew install ical-buddy`) we can shell out and
//    parse its output. No permissions dance, no bundled helper, but
//    zero coverage for users who don't have the tool.
//
// This file implements path 2 as a best-effort today and falls back to
// the warn-once empty result when icalBuddy is absent. When the
// EventKit path lands it replaces this file entirely.

use crate::os::calendar::CalendarEvent;
use chrono::{Duration, Local, NaiveDate};
use log::{debug, warn};
use std::process::Command;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Public API — what the cross-platform dispatch in src/os/calendar.rs calls
// ---------------------------------------------------------------------------

pub fn get_upcoming_events_impl(hours: u32) -> Vec<CalendarEvent> {
    if !icalbuddy_is_available() {
        warn_unavailable_once();
        return vec![];
    }

    let now = Local::now();
    let end = now + Duration::hours(hours as i64);
    let start_str = now.format("%Y-%m-%d").to_string();
    let end_str = end.format("%Y-%m-%d").to_string();

    // `eventsFrom:to:` accepts inclusive YYYY-MM-DD dates. Using the
    // date granularity is coarser than the requested hour window but
    // avoids parsing + normalizing icalBuddy's human-readable times;
    // callers already filter by exact start_time downstream.
    let range = format!("eventsFrom:{start_str} to:{end_str}");
    let events = run_icalbuddy(&[
        "-nc",
        "-iep",
        "title,datetime,location,notes,url,attendees",
        "-po",
        "title,datetime,location,notes,url,attendees",
        "-b",
        "* ",
        "--separateByCalendar",
        &range,
    ]);

    debug!("icalBuddy returned {} upcoming events", events.len());
    events
}

pub fn get_events_for_date_impl(date: &str) -> Vec<CalendarEvent> {
    if !icalbuddy_is_available() {
        warn_unavailable_once();
        return vec![];
    }

    // Validate date format — icalBuddy fails with an unhelpful error on
    // bad input, and we'd rather return empty than surface that.
    if NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        debug!("calendar: ignoring malformed date '{date}'");
        return vec![];
    }

    let range = format!("eventsFrom:{date} to:{date}");
    run_icalbuddy(&[
        "-nc",
        "-iep",
        "title,datetime,location,notes,url,attendees",
        "-po",
        "title,datetime,location,notes,url,attendees",
        "-b",
        "* ",
        "--separateByCalendar",
        &range,
    ])
}

// ---------------------------------------------------------------------------
// icalBuddy backend
// ---------------------------------------------------------------------------

/// Check whether `icalBuddy` is on PATH. Cached for the process lifetime —
/// installing it after Kage is running is vanishingly rare, and the
/// per-call `which` spawn would dominate the cost of a no-result path.
fn icalbuddy_is_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new("which")
            .arg("icalBuddy")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

fn warn_unavailable_once() {
    static WARNED: OnceLock<()> = OnceLock::new();
    WARNED.get_or_init(|| {
        log::warn!(
            "calendar: no backend available on macOS — install icalBuddy \
             (`brew install ical-buddy`) for a lightweight option, or wait \
             for the EventKit integration (tracked in MACMIGRATION.md)."
        );
    });
}

fn run_icalbuddy(args: &[&str]) -> Vec<CalendarEvent> {
    let output = match Command::new("icalBuddy").args(args).output() {
        Ok(o) => o,
        Err(e) => {
            warn!("icalBuddy failed to launch: {e}");
            return vec![];
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!("icalBuddy exited non-zero: {}", stderr.trim());
        return vec![];
    }

    let text = String::from_utf8_lossy(&output.stdout);
    parse_icalbuddy_output(&text)
}

/// Parse icalBuddy's text output into CalendarEvent structs. Output
/// format (with `-b "* "` bullet) looks like:
///
/// ```text
/// Work
/// ----
/// * Team standup
///     Jan 15, 2026 at 10:00 AM - 10:30 AM
///     location: Zoom
///     url: https://zoom.us/j/12345
///
/// * Lunch
///     Jan 15, 2026 at 12:00 PM - 1:00 PM
/// ```
///
/// We tolerate missing fields — only the title and datetime lines are
/// strictly required for a usable event.
fn parse_icalbuddy_output(text: &str) -> Vec<CalendarEvent> {
    let mut events: Vec<CalendarEvent> = Vec::new();
    let mut current: Option<EventBuilder> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim_end();
        if line.is_empty() {
            continue;
        }

        // New bullet → flush previous, start fresh.
        if let Some(title) = line.strip_prefix("* ") {
            if let Some(b) = current.take() {
                if let Some(evt) = b.into_event() {
                    events.push(evt);
                }
            }
            current = Some(EventBuilder::new(title.trim().to_string()));
            continue;
        }

        // Section header ("Work", "----") — skip.
        if line.trim().chars().all(|c| c == '-') {
            continue;
        }

        // Indented field line for the current event.
        if let Some(builder) = current.as_mut() {
            let field = line.trim();
            if let Some(v) = field.strip_prefix("location: ") {
                builder.location = v.to_string();
            } else if let Some(v) = field.strip_prefix("url: ") {
                builder.online_url = Some(v.to_string());
            } else if let Some(v) = field.strip_prefix("notes: ") {
                builder.notes = v.to_string();
            } else if let Some(v) = field.strip_prefix("attendees: ") {
                // Only store the first attendee as organizer — closest
                // thing icalBuddy gives us without switching formats.
                builder.organizer = v.split(',').next().unwrap_or("").trim().to_string();
            } else if builder.datetime_line.is_empty()
                && !field.starts_with("location:")
                && !field.starts_with("url:")
                && !field.starts_with("notes:")
            {
                // First unlabelled indented line is the datetime.
                builder.datetime_line = field.to_string();
            }
        }
    }

    if let Some(b) = current.take() {
        if let Some(evt) = b.into_event() {
            events.push(evt);
        }
    }

    events
}

struct EventBuilder {
    title: String,
    datetime_line: String,
    location: String,
    notes: String,
    online_url: Option<String>,
    organizer: String,
}

impl EventBuilder {
    fn new(title: String) -> Self {
        Self {
            title,
            datetime_line: String::new(),
            location: String::new(),
            notes: String::new(),
            online_url: None,
            organizer: String::new(),
        }
    }

    fn into_event(self) -> Option<CalendarEvent> {
        // An event with a title but no parseable datetime is still
        // marginally useful (the launcher can show it as "scheduled"
        // without a time), but it's also likely a malformed line —
        // filter to keep only events we can actually render.
        let (start_time, duration_minutes, all_day) = parse_datetime_range(&self.datetime_line)?;

        // Fall back to the built-in URL extraction for the common case
        // where the URL lives in location or notes rather than icalBuddy's
        // dedicated url field.
        let online_url = self
            .online_url
            .clone()
            .or_else(|| crate::os::calendar::extract_meeting_url(&self.location, &self.notes));

        Some(CalendarEvent {
            id: format!("icalbuddy:{}:{}", start_time, self.title),
            subject: self.title,
            location: self.location,
            organizer: self.organizer,
            start_time,
            duration_minutes,
            all_day,
            online_url,
        })
    }
}

/// Parse icalBuddy's datetime line into (iso8601_start, duration_minutes,
/// all_day). Examples:
///
/// - `Jan 15, 2026 at 10:00 AM - 10:30 AM` → (ISO, 30, false)
/// - `Jan 15, 2026 at 10:00 AM - Jan 16, 2026 at 2:00 PM` → multi-day
/// - `Jan 15, 2026` → (ISO with midnight, 1440, true)
///
/// Returns None on truly unparseable input — callers filter out events
/// that would render as "sometime this year" with no useful time info.
fn parse_datetime_range(line: &str) -> Option<(String, u32, bool)> {
    // Split on " - " (spaces required — times contain ":" and dates contain
    // "-" inside abbreviations that we must not split on).
    let (start_part, end_part) = match line.split_once(" - ") {
        Some((s, e)) => (s.trim(), e.trim()),
        // No range separator → all-day event.
        None => {
            let start = parse_single_datetime(line, false)?;
            return Some((start, 24 * 60, true));
        }
    };

    let has_time_separator = start_part.contains(" at ");
    let start = parse_single_datetime(start_part, has_time_separator)?;

    // For the end, icalBuddy elides the date when it matches the start date
    // ("10:00 AM - 10:30 AM"). Infer by detecting a " at " in the end part.
    let end_has_date = end_part.contains(" at ");
    let end_full = if end_has_date {
        end_part.to_string()
    } else {
        // Reuse the start's date — pull everything before " at ".
        let date_prefix = start_part
            .split_once(" at ")
            .map(|(d, _)| d)
            .unwrap_or(start_part);
        format!("{date_prefix} at {end_part}")
    };
    let end = parse_single_datetime(&end_full, true)?;

    let duration_minutes = minutes_between(&start, &end).unwrap_or(0) as u32;
    Some((start, duration_minutes, false))
}

fn parse_single_datetime(s: &str, has_time: bool) -> Option<String> {
    use chrono::{NaiveDateTime, NaiveTime};

    // icalBuddy uses the user's locale date format. The common US default is
    // "%b %e, %Y at %l:%M %p" — match that first, fall back to a handful
    // of variants we've seen in the wild.
    let formats = if has_time {
        &["%b %e, %Y at %l:%M %p", "%B %e, %Y at %l:%M %p"][..]
    } else {
        &["%b %e, %Y", "%B %e, %Y"][..]
    };

    for f in formats {
        if has_time {
            if let Ok(dt) = NaiveDateTime::parse_from_str(s, f) {
                return Some(dt.and_local_timezone(Local).single()?.to_rfc3339());
            }
        } else if let Ok(d) = NaiveDate::parse_from_str(s, f) {
            let midnight = NaiveTime::from_hms_opt(0, 0, 0)?;
            let dt = d.and_time(midnight);
            return Some(dt.and_local_timezone(Local).single()?.to_rfc3339());
        }
    }
    None
}

fn minutes_between(start_iso: &str, end_iso: &str) -> Option<i64> {
    use chrono::DateTime;
    let start = DateTime::parse_from_rfc3339(start_iso).ok()?;
    let end = DateTime::parse_from_rfc3339(end_iso).ok()?;
    Some((end - start).num_minutes().max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_datetime_range_handles_same_day_event() {
        let r = parse_datetime_range("Jan 15, 2026 at 10:00 AM - 10:30 AM").unwrap();
        assert_eq!(r.1, 30);
        assert!(!r.2, "30-min event shouldn't be marked all-day");
        assert!(r.0.starts_with("2026-01-15T10:00:00"));
    }

    #[test]
    fn parse_datetime_range_handles_all_day_event() {
        let r = parse_datetime_range("Jan 15, 2026").unwrap();
        assert_eq!(r.1, 24 * 60);
        assert!(r.2);
    }

    #[test]
    fn parse_datetime_range_handles_multi_day_event() {
        let r = parse_datetime_range("Jan 15, 2026 at 10:00 AM - Jan 16, 2026 at 2:00 PM").unwrap();
        assert_eq!(r.1, 28 * 60); // 28 hours
        assert!(!r.2);
    }

    #[test]
    fn parse_icalbuddy_output_extracts_event_with_all_fields() {
        let input = r#"Work
----
* Team standup
    Jan 15, 2026 at 10:00 AM - 10:30 AM
    location: Zoom
    url: https://zoom.us/j/12345
"#;
        let events = parse_icalbuddy_output(input);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].subject, "Team standup");
        assert_eq!(events[0].location, "Zoom");
        assert_eq!(
            events[0].online_url.as_deref(),
            Some("https://zoom.us/j/12345")
        );
        assert_eq!(events[0].duration_minutes, 30);
    }

    #[test]
    fn parse_icalbuddy_output_extracts_multiple_events() {
        let input = r#"Work
----
* Standup
    Jan 15, 2026 at 10:00 AM - 10:30 AM

* Lunch
    Jan 15, 2026 at 12:00 PM - 1:00 PM
    location: Kitchen
"#;
        let events = parse_icalbuddy_output(input);
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].subject, "Lunch");
        assert_eq!(events[1].location, "Kitchen");
        assert_eq!(events[1].duration_minutes, 60);
    }

    #[test]
    fn parse_icalbuddy_output_drops_unparseable_events() {
        // An event with no datetime at all should be filtered — it's
        // almost always malformed output or an unsupported line format.
        let input = "* Mystery event\n    some garbled line\n";
        let events = parse_icalbuddy_output(input);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_icalbuddy_output_infers_meeting_url_from_location() {
        // If icalBuddy doesn't expose a separate url field but the
        // location string contains a Zoom/Teams link, the cross-platform
        // extractor should still pick it up.
        let input = r#"* Sync
    Jan 15, 2026 at 10:00 AM - 10:30 AM
    location: https://zoom.us/j/99999
"#;
        let events = parse_icalbuddy_output(input);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].online_url.as_deref(),
            Some("https://zoom.us/j/99999")
        );
    }
}
