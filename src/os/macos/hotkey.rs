// macOS hotkey capture (stub — not yet implemented)

use log::warn;

pub fn capture_hotkey_impl(_timeout_ms: u64) -> Option<crate::os::hotkey::CapturedHotkey> {
    warn!("Hotkey capture not yet implemented on macOS");
    None
}

pub fn cancel_capture_impl() {
    // No-op
}
