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

pub use kage_core::calendar::{extract_meeting_url, CalendarEvent};

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
