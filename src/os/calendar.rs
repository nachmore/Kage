// Cross-platform calendar integration.
//
// Uses OS-native calendar APIs:
// - Windows: Windows.ApplicationModel.Appointments
// - macOS: EventKit (via swift CLI) — TODO
// - Linux: stub (no standard API)

use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEvent {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub organizer: String,
    #[serde(default)]
    pub start_time: String, // ISO 8601
    #[serde(default)]
    pub duration_minutes: u32,
    #[serde(default)]
    pub all_day: bool,
    #[serde(default)]
    pub online_url: Option<String>,
}

/// Get upcoming calendar events within the next `hours` hours.
pub fn get_upcoming_events(hours: u32) -> Vec<CalendarEvent> {
    crate::os::platform::calendar::get_upcoming_events_impl(hours)
}

/// Get calendar events for a specific date (YYYY-MM-DD).
pub fn get_events_for_date(date: &str) -> Vec<CalendarEvent> {
    crate::os::platform::calendar::get_events_for_date_impl(date)
}

/// Extract a meeting/join URL from event location and body text.
/// Checks location first (Teams/Zoom often put the URL there), then body.
/// This is cross-platform — used by all OS calendar implementations.
pub fn extract_meeting_url(location: &str, body: &str) -> Option<String> {
    // Known meeting URL domain suffixes (matched against the URL after "https://")
    let domain_patterns = [
        "teams.microsoft.com",
        ".zoom.us",
        "zoom.us",
        "meet.google.com",
        "chime.aws",
        ".webex.com",
    ];

    // Check location first, then body
    for text in [location, body] {
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(pos) = trimmed.find("https://") {
                let url = &trimmed[pos..];
                let end = url.find(|c: char| {
                    c.is_whitespace() || c == '"' || c == '\'' || c == '<' || c == '>' || c == ')'
                }).unwrap_or(url.len());
                let candidate = &url[..end];
                let after_scheme = &candidate[8..]; // skip "https://"
                if domain_patterns.iter().any(|p| {
                    after_scheme.starts_with(p) || after_scheme.contains(&format!("{}/", p))
                        || (p.starts_with('.') && after_scheme.find('/').is_some_and(|slash| after_scheme[..slash].ends_with(p)))
                })
                    && candidate.len() > 15 {
                        return Some(candidate.to_string());
                    }
            }
        }
    }
    None
}
