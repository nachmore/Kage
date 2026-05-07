// macOS calendar — stub.
//
// A real implementation would shell out to a small Swift CLI that reads
// EventKit (Apple's calendar API). Until that exists, return empty
// results and warn once at module-load so a user investigating "why no
// calendar entries" sees a clear answer in the log.

use crate::os::calendar::CalendarEvent;
use std::sync::OnceLock;

static WARNED: OnceLock<()> = OnceLock::new();

fn warn_once() {
    WARNED.get_or_init(|| {
        log::warn!(
            "calendar: macOS implementation not yet available — \
             returning empty results. EventKit integration is a follow-up."
        );
    });
}

pub fn get_upcoming_events_impl(_hours: u32) -> Vec<CalendarEvent> {
    warn_once();
    vec![]
}

pub fn get_events_for_date_impl(_date: &str) -> Vec<CalendarEvent> {
    warn_once();
    vec![]
}
