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
