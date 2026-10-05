// macOS calendar integration.
//
// Three backends in preference order:
//
// 1. **EventKit helper** — `kage-calendar-helper`, a tiny Rust sidecar
//    (workspace member, provisioned by build.rs into src-tauri/binaries/
//    and copied next to the app binary by tauri-build). Uses Apple's
//    EventKit API via objc2-event-kit, returns JSON. Requires Calendar
//    TCC permission (surfaced via `NSCalendarsFullAccessUsageDescription`
//    / `NSCalendarsUsageDescription` in Info.plist and prompted on first
//    call). This is the canonical path.
//
// 2. **icalBuddy** — third-party Homebrew tool (`brew install ical-buddy`).
//    Zero permission dance but only helps if the user installed it.
//
// 3. **Empty** — warn once and return no events. Happens when neither
//    backend is available; the cross-platform layer treats this as "no
//    calendar integration" rather than an error.
//
// Each call tries (1), and on any failure (missing binary, permission
// denied, JSON parse error, stderr noise) falls through to (2), and
// finally (3).

use crate::os::calendar::CalendarEvent;
use chrono::{Duration, Local, NaiveDate};
use log::{debug, warn};
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Public API — what the cross-platform dispatch in src/os/calendar.rs calls
// ---------------------------------------------------------------------------

pub fn get_upcoming_events_impl(hours: u32) -> Result<Vec<CalendarEvent>, String> {
    match run_eventkit_helper(&["upcoming", &hours.to_string()]) {
        HelperResult::Ok(events) => return Ok(events),
        HelperResult::PermissionDenied(msg) => return Err(msg),
        HelperResult::OtherFailure => {} // fall through to icalBuddy
    }
    if icalbuddy_is_available() {
        return Ok(get_upcoming_events_via_icalbuddy(hours));
    }
    warn_unavailable_once();
    Ok(vec![])
}

pub fn get_events_for_date_impl(date: &str) -> Result<Vec<CalendarEvent>, String> {
    if NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        debug!("calendar: ignoring malformed date '{date}'");
        return Ok(vec![]);
    }
    match run_eventkit_helper(&["date", date]) {
        HelperResult::Ok(events) => return Ok(events),
        HelperResult::PermissionDenied(msg) => return Err(msg),
        HelperResult::OtherFailure => {} // fall through to icalBuddy
    }
    if icalbuddy_is_available() {
        return Ok(get_events_for_date_via_icalbuddy(date));
    }
    warn_unavailable_once();
    Ok(vec![])
}

// ---------------------------------------------------------------------------
// EventKit helper (preferred backend)
// ---------------------------------------------------------------------------

/// Locate the `kage-calendar-helper` binary. Looks next to the current
/// executable first (tauri-build copies every externalBin there, triple
/// suffix stripped — covers both bundled apps and target/<profile>/ dev
/// runs), then PATH.
fn helper_path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let sibling = dir.join("kage-calendar-helper");
                if sibling.exists() {
                    return Some(sibling);
                }
            }
        }
        // Last-ditch: someone dropped it on PATH.
        if let Ok(out) = Command::new("which").arg("kage-calendar-helper").output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
        }
        None
    })
    .clone()
}

#[derive(Debug, Deserialize)]
struct HelperError {
    error: String,
}

/// Tri-state result from the EventKit helper binary.
enum HelperResult {
    /// Successfully parsed events (may be empty — that's fine).
    Ok(Vec<CalendarEvent>),
    /// Calendar access was denied — this should propagate to the user.
    PermissionDenied(String),
    /// Any other failure (missing binary, parse error, etc.) — fall through
    /// to the next backend.
    OtherFailure,
}

fn run_eventkit_helper(args: &[&str]) -> HelperResult {
    let path = match helper_path() {
        Some(p) => p,
        None => return HelperResult::OtherFailure,
    };
    let output = match Command::new(&path).args(args).output() {
        Ok(o) => o,
        Err(e) => {
            debug!("calendar-helper spawn failed: {e}");
            return HelperResult::OtherFailure;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout_trim = stdout.trim();
    if stdout_trim.is_empty() {
        debug!("calendar-helper produced empty stdout");
        return HelperResult::OtherFailure;
    }

    // Try parsing as the success shape (Vec<CalendarEvent>) first. If that
    // fails, see if the helper emitted a structured error payload so we
    // can log something useful before the fallback.
    if let Ok(events) = serde_json::from_str::<Vec<CalendarEvent>>(stdout_trim) {
        debug!(
            "calendar-helper returned {} events for {:?}",
            events.len(),
            args
        );
        return HelperResult::Ok(events);
    }
    if let Ok(err) = serde_json::from_str::<HelperError>(stdout_trim) {
        warn!("calendar-helper error: {}", err.error);
        // Detect permission-denied errors so the UI can prompt the user.
        let lower = err.error.to_lowercase();
        if lower.contains("denied") || lower.contains("permission") || lower.contains("timed out") {
            return HelperResult::PermissionDenied(
                "Calendar access denied — grant Calendar permission in System Settings".to_string(),
            );
        }
        return HelperResult::OtherFailure;
    }
    warn!(
        "calendar-helper returned unexpected output ({} bytes); \
         falling back",
        stdout_trim.len()
    );
    HelperResult::OtherFailure
}

// ---------------------------------------------------------------------------
// icalBuddy backend (legacy fallback)
// ---------------------------------------------------------------------------

/// Shared icalBuddy flags. `-nrd` plus fixed `-df`/`-tf` pin the datetime
/// line to `2026-01-15 at 10:00 - 10:30` regardless of the user's locale —
/// the defaults print "today"/"tomorrow" and locale-dependent 12h/24h
/// times, which the parser can't handle.
const ICALBUDDY_ARGS: &[&str] = &[
    "-nc",
    "-nrd",
    "-df",
    "%Y-%m-%d",
    "-tf",
    "%H:%M",
    "-iep",
    "title,datetime,location,notes,url,attendees",
    "-po",
    "title,datetime,location,notes,url,attendees",
    "-b",
    "* ",
    "--separateByCalendar",
];

fn icalbuddy_args(range: &str) -> Vec<&str> {
    let mut args: Vec<&str> = ICALBUDDY_ARGS.to_vec();
    args.push(range);
    args
}

fn get_upcoming_events_via_icalbuddy(hours: u32) -> Vec<CalendarEvent> {
    let now = Local::now();
    let end = now + Duration::hours(hours as i64);
    let start_str = now.format("%Y-%m-%d").to_string();
    let end_str = end.format("%Y-%m-%d").to_string();

    // icalBuddy only takes whole days, so trim to [now, end) ourselves —
    // same overlap semantics as the EventKit helper's predicate.
    let range = format!("eventsFrom:{start_str} to:{end_str}");
    let events: Vec<CalendarEvent> = run_icalbuddy(&icalbuddy_args(&range))
        .into_iter()
        .filter(|e| overlaps_window(e, now, end))
        .collect();

    debug!("icalBuddy returned {} upcoming events", events.len());
    events
}

fn get_events_for_date_via_icalbuddy(date: &str) -> Vec<CalendarEvent> {
    let range = format!("eventsFrom:{date} to:{date}");
    run_icalbuddy(&icalbuddy_args(&range))
}

/// True when the event's [start, start + duration) overlaps
/// [window_start, window_end). All-day events carry local-midnight starts
/// and whole-day durations, so today's all-day events overlap any window
/// that starts today. Unparseable starts are kept rather than silently
/// dropped.
fn overlaps_window(
    event: &CalendarEvent,
    window_start: chrono::DateTime<Local>,
    window_end: chrono::DateTime<Local>,
) -> bool {
    let Ok(start) = chrono::DateTime::parse_from_rfc3339(&event.start_time) else {
        return true;
    };
    let end = start + Duration::minutes(i64::from(event.duration_minutes));
    end > window_start && start < window_end
}

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
            "calendar: no backend available on macOS — make sure the \
             kage-calendar-helper binary ships alongside the app and \
             Calendar permission is granted, or install icalBuddy \
             (`brew install ical-buddy`) as a fallback."
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
///     2026-01-15 at 10:00 - 10:30
///     location: Zoom
///     url: https://zoom.us/j/12345
///
/// * Lunch
///     2026-01-15 at 12:00 - 13:00
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

/// Parse icalBuddy's datetime line (as pinned by `ICALBUDDY_ARGS`) into
/// (iso8601_start, duration_minutes, all_day). Examples:
///
/// - `2026-01-15 at 10:00 - 10:30` → (ISO, 30, false)
/// - `2026-01-15 at 10:00 - 2026-01-16 at 14:00` → multi-day
/// - `2026-01-15` → (ISO with midnight, 1440, true)
/// - `2026-01-15 - 2026-01-17` → multi-day all-day (end date inclusive)
///
/// Returns None on truly unparseable input — callers filter out events
/// that would render as "sometime this year" with no useful time info.
fn parse_datetime_range(line: &str) -> Option<(String, u32, bool)> {
    // Split on " - " (spaces required — ISO dates contain bare "-").
    let (start_part, end_part) = match line.split_once(" - ") {
        Some((s, e)) => (s.trim(), Some(e.trim())),
        None => (line.trim(), None),
    };
    let (start_date, start_time) = parse_date_time_tokens(start_part);
    let start_date = start_date?;
    let (end_date, end_time) = end_part.map(parse_date_time_tokens).unwrap_or((None, None));

    let Some(start_time) = start_time else {
        // Date-only → all-day, spanning through the (inclusive) end date.
        let days = end_date
            .map(|ed| (ed - start_date).num_days() + 1)
            .unwrap_or(1)
            .max(1);
        let midnight = chrono::NaiveTime::from_hms_opt(0, 0, 0)?;
        let start = to_local_rfc3339(start_date.and_time(midnight))?;
        return Some((start, (days * 24 * 60) as u32, true));
    };

    // icalBuddy elides the end date when it matches the start date
    // ("10:00 - 10:30"), so default it to the start's. A start time with
    // no end time (zero-length event, or a `date at time - date` line)
    // becomes a zero-duration event rather than being dropped.
    let start_dt = start_date.and_time(start_time);
    let end_dt = match end_time {
        Some(t) => end_date.unwrap_or(start_date).and_time(t),
        None => start_dt,
    };
    let duration_minutes = (end_dt - start_dt).num_minutes().max(0) as u32;
    Some((to_local_rfc3339(start_dt)?, duration_minutes, false))
}

/// Pull the first `%Y-%m-%d` date and `%H:%M` time tokens out of one side
/// of the range. Any other words (icalBuddy's localisable " at "
/// separator) are ignored, so we don't depend on their wording.
fn parse_date_time_tokens(s: &str) -> (Option<NaiveDate>, Option<chrono::NaiveTime>) {
    let mut date = None;
    let mut time = None;
    for tok in s.split_whitespace() {
        if date.is_none() {
            if let Ok(d) = NaiveDate::parse_from_str(tok, "%Y-%m-%d") {
                date = Some(d);
                continue;
            }
        }
        if time.is_none() {
            if let Ok(t) = chrono::NaiveTime::parse_from_str(tok, "%H:%M") {
                time = Some(t);
            }
        }
    }
    (date, time)
}

fn to_local_rfc3339(dt: chrono::NaiveDateTime) -> Option<String> {
    // `earliest` rather than `single` so the repeated hour on a DST
    // fall-back day still resolves instead of dropping the event.
    Some(dt.and_local_timezone(Local).earliest()?.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_datetime_range_handles_same_day_event() {
        let r = parse_datetime_range("2026-01-15 at 10:00 - 10:30").unwrap();
        assert_eq!(r.1, 30);
        assert!(!r.2, "30-min event shouldn't be marked all-day");
        assert!(r.0.starts_with("2026-01-15T10:00:00"));
    }

    #[test]
    fn parse_datetime_range_handles_all_day_event() {
        let r = parse_datetime_range("2026-01-15").unwrap();
        assert_eq!(r.1, 24 * 60);
        assert!(r.2);
    }

    #[test]
    fn parse_datetime_range_handles_multi_day_event() {
        let r = parse_datetime_range("2026-01-15 at 10:00 - 2026-01-16 at 14:00").unwrap();
        assert_eq!(r.1, 28 * 60); // 28 hours
        assert!(!r.2);
    }

    #[test]
    fn parse_datetime_range_handles_multi_day_all_day_event() {
        let r = parse_datetime_range("2026-01-15 - 2026-01-17").unwrap();
        assert_eq!(r.1, 3 * 24 * 60);
        assert!(r.2);
    }

    #[test]
    fn parse_datetime_range_ignores_localised_separator_word() {
        // " at " is localisable in icalBuddy; only the tokens matter.
        let r = parse_datetime_range("2026-01-15 um 09:05 - 09:50").unwrap();
        assert_eq!(r.1, 45);
        assert!(r.0.starts_with("2026-01-15T09:05:00"));
    }

    #[test]
    fn parse_datetime_range_rejects_relative_dates() {
        assert!(parse_datetime_range("today at 10:00 - 10:30").is_none());
    }

    fn event_at(start: chrono::DateTime<Local>, minutes: u32) -> CalendarEvent {
        CalendarEvent {
            id: String::new(),
            subject: String::new(),
            location: String::new(),
            organizer: String::new(),
            start_time: start.to_rfc3339(),
            duration_minutes: minutes,
            all_day: false,
            online_url: None,
        }
    }

    #[test]
    fn overlaps_window_keeps_only_events_in_range() {
        let now = Local::now();
        let end = now + Duration::hours(2);
        // Already ended.
        assert!(!overlaps_window(
            &event_at(now - Duration::hours(2), 30),
            now,
            end
        ));
        // In progress.
        assert!(overlaps_window(
            &event_at(now - Duration::minutes(10), 30),
            now,
            end
        ));
        // Upcoming within the window.
        assert!(overlaps_window(
            &event_at(now + Duration::hours(1), 30),
            now,
            end
        ));
        // Starts after the window.
        assert!(!overlaps_window(
            &event_at(now + Duration::hours(3), 30),
            now,
            end
        ));
    }

    #[test]
    fn parse_icalbuddy_output_extracts_event_with_all_fields() {
        let input = r#"Work
----
* Team standup
    2026-01-15 at 10:00 - 10:30
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
    2026-01-15 at 10:00 - 10:30

* Lunch
    2026-01-15 at 12:00 - 13:00
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
    2026-01-15 at 10:00 - 10:30
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
