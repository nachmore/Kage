// Kage calendar helper — reads EventKit events and outputs JSON.
//
// Usage:
//   kage-calendar-helper upcoming <hours>
//   kage-calendar-helper date <yyyy-MM-dd>
//
// Output (stdout): JSON array of CalendarEvent (see kage-core/src/calendar.rs
// — { id, subject, location, organizer, start_time (ISO8601),
// duration_minutes, all_day, online_url }).
//
// Error handling: writes a JSON object { "error": "<message>" } to stdout
// and exits with a non-zero code. The Rust caller in the app
// (src/os/macos/calendar.rs) treats any non-JSON stdout as fatal and
// falls back to icalBuddy (or empty).
//
// Permission: uses EKEventStore requestFullAccessToEvents on macOS 14+,
// falls back to the deprecated requestAccess(to:) on older versions —
// selected at runtime via respondsToSelector so one binary covers both.
// If the user denies access we exit with an "access denied" payload;
// the caller surfaces this verbatim. TCC attributes the permission to
// this executable (bundled inside Kage.app), same as the Swift
// original it replaces.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = match parse_mode(&args) {
        Some(m) => m,
        None => emit_error("usage: kage-calendar-helper {upcoming <hours> | date <yyyy-MM-dd>}"),
    };
    imp::run(mode)
}

// The non-macOS stub never reads the parsed values — parsing still runs
// so the usage error is identical on every platform.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum Mode {
    Upcoming { hours: u32 },
    Date { iso: String },
}

fn parse_mode(args: &[String]) -> Option<Mode> {
    if args.len() < 3 {
        return None;
    }
    match args[1].as_str() {
        "upcoming" => match args[2].parse::<u32>() {
            Ok(h) if h > 0 => Some(Mode::Upcoming { hours: h }),
            _ => None,
        },
        "date" => Some(Mode::Date {
            iso: args[2].clone(),
        }),
        _ => None,
    }
}

fn emit_error(message: &str) -> ! {
    println!("{}", serde_json::json!({ "error": message }));
    std::process::exit(1);
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn run(_mode: super::Mode) -> ! {
        super::emit_error("kage-calendar-helper is macOS-only")
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{emit_error, Mode};
    use block2::RcBlock;
    use chrono::{DateTime, NaiveDate, TimeZone, Utc};
    use kage_core::calendar::{extract_meeting_url, CalendarEvent};
    use objc2::rc::Retained;
    use objc2::runtime::Bool;
    use objc2::sel;
    use objc2_event_kit::{EKEntityType, EKEvent, EKEventStore};
    use objc2_foundation::{NSDate, NSError, NSObjectProtocol};
    use std::sync::mpsc;
    use std::time::Duration;

    pub fn run(mode: Mode) -> ! {
        let store = unsafe { EKEventStore::new() };
        if !request_access(&store) {
            emit_error(
                "calendar access denied or timed out — grant Calendar permission in System Settings",
            );
        }

        let now = Utc::now();
        let (start, end): (DateTime<Utc>, DateTime<Utc>) = match mode {
            Mode::Upcoming { hours } => (now, now + chrono::Duration::hours(i64::from(hours))),
            Mode::Date { ref iso } => {
                let Ok(day) = NaiveDate::parse_from_str(iso, "%Y-%m-%d") else {
                    emit_error(&format!("invalid date '{iso}' — expected yyyy-MM-dd"));
                };
                // Local midnight, matching the Swift original's DateFormatter
                // with TimeZone.current.
                let local_start = chrono::Local
                    .from_local_datetime(&day.and_hms_opt(0, 0, 0).expect("00:00:00 is valid"))
                    .earliest()
                    .unwrap_or_else(|| {
                        emit_error(&format!("date '{iso}' has no valid local midnight"))
                    });
                let start = local_start.with_timezone(&Utc);
                (start, start + chrono::Duration::hours(24))
            }
        };

        let events = fetch_events(&store, start, end);
        println!(
            "{}",
            serde_json::to_string(&events).unwrap_or_else(|_| "[]".into())
        );
        std::process::exit(0);
    }

    /// Request calendar access, blocking until the user answers the TCC
    /// prompt (or 15s, matching the Swift original's semaphore timeout).
    fn request_access(store: &EKEventStore) -> bool {
        let (tx, rx) = mpsc::channel::<bool>();
        let completion = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
            let _ = tx.send(granted.as_bool());
        });

        unsafe {
            // macOS 14+ granular API when available, legacy single-tier
            // requestAccess otherwise. Runtime-detected so the same binary
            // works across OS versions without an availability shim.
            if store.respondsToSelector(sel!(requestFullAccessToEventsWithCompletion:)) {
                store.requestFullAccessToEventsWithCompletion(&*completion as *const _ as *mut _);
            } else {
                #[allow(deprecated)]
                store.requestAccessToEntityType_completion(
                    EKEntityType::Event,
                    &*completion as *const _ as *mut _,
                );
            }
        }

        rx.recv_timeout(Duration::from_secs(15)).unwrap_or(false)
    }

    fn fetch_events(
        store: &EKEventStore,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Vec<CalendarEvent> {
        let ns_start = NSDate::dateWithTimeIntervalSince1970(start.timestamp() as f64);
        let ns_end = NSDate::dateWithTimeIntervalSince1970(end.timestamp() as f64);

        let mut events: Vec<(f64, CalendarEvent)> = unsafe {
            let calendars = store.calendarsForEntityType(EKEntityType::Event);
            let predicate = store.predicateForEventsWithStartDate_endDate_calendars(
                &ns_start,
                &ns_end,
                Some(&calendars),
            );
            store
                .eventsMatchingPredicate(&predicate)
                .iter()
                .map(|e| serialize(&e))
                .collect()
        };
        // No guaranteed order from EventKit — sort by start time ascending.
        events.sort_by(|a, b| a.0.total_cmp(&b.0));
        events.into_iter().map(|(_, e)| e).collect()
    }

    /// EKEvent → (epoch start for sorting, wire struct). Field mapping is
    /// 1:1 with the Swift original's serialize().
    unsafe fn serialize(event: &Retained<EKEvent>) -> (f64, CalendarEvent) {
        let start_epoch = event.startDate().timeIntervalSince1970();
        let end_epoch = event.endDate().timeIntervalSince1970();
        let start_time = epoch_to_iso8601(start_epoch);

        let subject = event.title().to_string();
        let location = event.location().map(|s| s.to_string()).unwrap_or_default();
        let notes = event.notes().map(|s| s.to_string()).unwrap_or_default();

        let id = event
            .eventIdentifier()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("eventkit:{start_time}:{subject}"));

        let online_url = event
            .URL()
            .and_then(|u| u.absoluteString())
            .map(|s| s.to_string())
            .or_else(|| extract_meeting_url(&location, &notes));

        let calendar_event = CalendarEvent {
            id,
            subject,
            location,
            organizer: event
                .organizer()
                .and_then(|o| o.name())
                .map(|s| s.to_string())
                .unwrap_or_default(),
            start_time,
            duration_minutes: ((end_epoch - start_epoch).max(0.0) / 60.0) as u32,
            all_day: event.isAllDay(),
            online_url,
        };
        (start_epoch, calendar_event)
    }

    /// UTC ISO8601 with fractional seconds — same shape as the Swift
    /// original's ISO8601DateFormatter with .withFractionalSeconds.
    fn epoch_to_iso8601(epoch: f64) -> String {
        let secs = epoch.floor() as i64;
        let nanos = ((epoch - epoch.floor()) * 1e9) as u32;
        Utc.timestamp_opt(secs, nanos)
            .single()
            .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .unwrap_or_default()
    }
}
