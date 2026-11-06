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
    /// The window is minimized (Windows: iconic; macOS: in the Dock).
    /// Minimized windows are listed, not filtered, so callers decide.
    /// `false` when the platform can't tell: Linux, and macOS while Kage
    /// lacks Accessibility (minimized windows aren't listed there at all).
    #[serde(default)]
    pub minimized: bool,
}

/// List visible top-level windows with titles. Cheap and never messages
/// other apps, so it's fine on an async worker. Minimized windows are
/// listed where that's free (Windows: flagged; Linux: unflagged); on macOS
/// they're left out — use [`list_windows_with_minimized`] for those.
pub fn list_windows() -> Vec<WindowInfo> {
    crate::os::platform::window_list::list_windows_impl()
}

/// [`list_windows`] with minimized windows included and flagged (see
/// [`WindowInfo::minimized`]). On macOS that takes an Accessibility probe
/// of other apps (bounded, but up to a few hundred ms), so call it from a
/// blocking thread.
pub fn list_windows_with_minimized() -> Vec<WindowInfo> {
    crate::os::platform::window_list::list_windows_with_minimized_impl()
}

/// Bring a window to the foreground by its handle.
pub fn focus_window(handle: u64) -> Result<(), String> {
    crate::os::platform::window_list::focus_window_impl(handle)
}

/// Look up a cached app icon by process name (e.g. "winword", "chrome").
/// Returns the base64 data URI if found in the cache populated by an
/// earlier `list_windows()` (Windows) or `get_window_icons()` call. On
/// Windows, an empty cache is primed by enumerating windows once — covers
/// the activity tracker's first-call path before any UI has rendered.
/// Elsewhere `list_windows()` doesn't register icons, so priming would
/// only cost an enumeration per lookup.
pub fn get_app_icon(process_name: &str) -> Option<String> {
    if let Some(icon) = crate::os::icon::get_icon_by_process_name(process_name) {
        return Some(icon);
    }
    if cfg!(target_os = "windows") && crate::os::icon::process_name_cache_is_empty() {
        let _ = list_windows();
        return crate::os::icon::get_icon_by_process_name(process_name);
    }
    None
}

/// Get the foreground window's title and process name.
/// Returns None if no foreground window or it's our own window.
pub fn get_foreground_window_info() -> Option<(String, String)> {
    crate::os::platform::window_list::get_foreground_window_info()
}

/// True when the foreground window belongs to this process.
/// [`get_foreground_window_info`] returns `None` both for our own windows
/// and for no window at all; this tells the two apart. Always false on
/// Linux, where foreground lookup isn't implemented.
pub fn foreground_is_own_process() -> bool {
    crate::os::platform::window_list::foreground_is_own_process()
}

/// Fetch app icons for a set of window handles (PIDs on macOS, HWNDs on Windows).
/// Returns a map of handle → base64 data URI.
/// This is designed to be called asynchronously after `list_windows()` so the
/// window list can render immediately while icons load in the background.
pub fn get_window_icons(handles: &[u64]) -> std::collections::HashMap<u64, String> {
    crate::os::platform::window_list::get_window_icons(handles)
}

#[cfg(test)]
mod tests {
    use super::WindowInfo;

    #[test]
    fn minimized_is_always_serialized() {
        let win = WindowInfo {
            title: "Notes".into(),
            process_name: "Notes".into(),
            handle: 42,
            icon_base64: None,
            minimized: true,
        };
        assert_eq!(
            serde_json::to_value(&win).unwrap(),
            serde_json::json!({
                "title": "Notes",
                "process_name": "Notes",
                "handle": 42,
                "minimized": true,
            })
        );
        let shown = WindowInfo {
            minimized: false,
            ..win
        };
        assert_eq!(serde_json::to_value(&shown).unwrap()["minimized"], false);
    }

    #[test]
    fn payload_without_minimized_reads_as_not_minimized() {
        let win: WindowInfo =
            serde_json::from_str(r#"{"title":"a","process_name":"b","handle":1}"#).unwrap();
        assert!(!win.minimized);
        assert!(win.icon_base64.is_none());
    }
}
