// Cross-platform hotkey capture

/// Result of a captured hotkey combination
#[derive(Debug, Clone)]
pub struct CapturedHotkey {
    pub modifiers: Vec<String>,
    pub key: String,
    pub display: String,
}

/// Capture a hotkey combination by listening for key presses.
/// Blocks until a hotkey is captured or the timeout (ms) expires.
/// Returns None if cancelled or timed out.
pub fn capture_hotkey(timeout_ms: u64) -> Option<CapturedHotkey> {
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::hotkey_capture::capture_hotkey_via_helper(timeout_ms)
            .map(|h| CapturedHotkey { modifiers: h.modifiers, key: h.key, display: h.display })
    }

    #[cfg(target_os = "macos")]
    { crate::os::macos::hotkey::capture_hotkey_impl(timeout_ms) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::hotkey::capture_hotkey_impl(timeout_ms) }
}

/// Cancel an in-progress hotkey capture
pub fn cancel_hotkey_capture() {
    #[cfg(target_os = "windows")]
    { crate::os::windows::hotkey_capture::cancel_capture(); }

    #[cfg(target_os = "macos")]
    { crate::os::macos::hotkey::cancel_capture_impl(); }

    #[cfg(target_os = "linux")]
    { crate::os::linux::hotkey::cancel_capture_impl(); }
}
