// Cross-platform hotkey capture

/// Result of a captured hotkey combination.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CapturedHotkey {
    pub modifiers: Vec<String>,
    pub key: String,
    pub display: String,
}

/// Capture a hotkey combination by listening for key presses.
/// Blocks until a hotkey is captured or the timeout (ms) expires.
/// Returns None if cancelled or timed out.
pub fn capture_hotkey(timeout_ms: u64) -> Option<CapturedHotkey> {
    crate::os::platform::hotkey::capture_hotkey_impl(timeout_ms)
}

/// Cancel an in-progress hotkey capture.
pub fn cancel_hotkey_capture() {
    crate::os::platform::hotkey::cancel_capture_impl();
}
