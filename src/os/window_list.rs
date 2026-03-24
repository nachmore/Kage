// Cross-platform window enumeration and focus

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Window title
    pub title: String,
    /// Process name (e.g. "chrome", "Code")
    pub process_name: String,
    /// Platform-specific window handle (as u64 for serialization)
    pub handle: u64,
    /// Optional base64-encoded icon
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_base64: Option<String>,
}

/// List all visible top-level windows with titles.
pub fn list_windows() -> Vec<WindowInfo> {
    crate::os::platform::window_list::list_windows_impl()
}

/// Bring a window to the foreground by its handle.
pub fn focus_window(handle: u64) -> Result<(), String> {
    crate::os::platform::window_list::focus_window_impl(handle)
}

/// Look up a cached app icon by process name (e.g. "winword", "chrome").
/// Returns the base64 data URI if found.
pub fn get_app_icon(process_name: &str) -> Option<String> {
    crate::os::platform::window_list::get_icon_by_process_name(process_name)
}

/// Get the foreground window's title and process name.
/// Returns None if no foreground window or it's our own window.
pub fn get_foreground_window_info() -> Option<(String, String)> {
    crate::os::platform::window_list::get_foreground_window_info()
}
