// Linux calendar — stub.
//
// There is no standard Linux calendar API; a real implementation would
// likely target Evolution Data Server (libecal) or shell out to
// `khal`/`gcalcli`. Until that exists, return empty results and warn
// once so users understand why nothing comes back.

use crate::os::calendar::CalendarEvent;
use std::sync::OnceLock;

static WARNED: OnceLock<()> = OnceLock::new();

fn warn_once() {
    WARNED.get_or_init(|| {
        log::warn!(
            "calendar: Linux implementation not yet available — \
             returning empty results. Evolution/khal integration is a follow-up."
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
