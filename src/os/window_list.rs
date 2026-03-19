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
    list_windows_impl()
}

/// Bring a window to the foreground by its handle.
pub fn focus_window(handle: u64) -> Result<(), String> {
    focus_window_impl(handle)
}

/// Look up a cached app icon by process name (e.g. "winword", "chrome").
/// Returns the base64 data URI if found. The cache is populated by list_windows().
pub fn get_app_icon(process_name: &str) -> Option<String> {
    get_app_icon_impl(process_name)
}

/// Get the foreground window's title and process name.
/// Returns None if no foreground window or it's our own window.
pub fn get_foreground_window_info() -> Option<(String, String)> {
    get_foreground_window_info_impl()
}

#[cfg(target_os = "windows")]
fn list_windows_impl() -> Vec<WindowInfo> {
    crate::os::windows::window_list::list_windows_impl()
}

#[cfg(target_os = "macos")]
fn list_windows_impl() -> Vec<WindowInfo> {
    crate::os::macos::window_list::list_windows_impl()
}

#[cfg(target_os = "linux")]
fn list_windows_impl() -> Vec<WindowInfo> {
    crate::os::linux::window_list::list_windows_impl()
}

#[cfg(target_os = "windows")]
fn focus_window_impl(handle: u64) -> Result<(), String> {
    crate::os::windows::window_list::focus_window_impl(handle)
}

#[cfg(target_os = "macos")]
fn focus_window_impl(handle: u64) -> Result<(), String> {
    crate::os::macos::window_list::focus_window_impl(handle)
}

#[cfg(target_os = "linux")]
fn focus_window_impl(handle: u64) -> Result<(), String> {
    crate::os::linux::window_list::focus_window_impl(handle)
}

#[cfg(target_os = "windows")]
fn get_app_icon_impl(name: &str) -> Option<String> {
    crate::os::windows::window_list::get_icon_by_process_name(name)
}

#[cfg(target_os = "macos")]
fn get_app_icon_impl(_name: &str) -> Option<String> {
    None // TODO: implement icon lookup on macOS
}

#[cfg(target_os = "linux")]
fn get_app_icon_impl(_name: &str) -> Option<String> {
    None // TODO: implement icon lookup on Linux
}

#[cfg(target_os = "windows")]
fn get_foreground_window_info_impl() -> Option<(String, String)> {
    crate::os::windows::window_list::get_foreground_window_info()
}

#[cfg(target_os = "macos")]
fn get_foreground_window_info_impl() -> Option<(String, String)> {
    None // TODO: implement on macOS
}

#[cfg(target_os = "linux")]
fn get_foreground_window_info_impl() -> Option<(String, String)> {
    None // TODO: implement on Linux
}
