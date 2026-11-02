// System-shortcut takeover is macOS-only (see `src/os/system_shortcuts.rs`).
// This platform reports no system shortcuts and refuses to toggle any, so
// `crate::system_shortcut_takeover` is a no-op here.

use crate::os::system_shortcuts::SystemShortcut;

pub fn list_impl() -> Vec<SystemShortcut> {
    Vec::new()
}

pub fn get_impl(_id: u32) -> Option<SystemShortcut> {
    None
}

pub fn set_enabled_impl(_id: u32, _enabled: bool) -> bool {
    false
}

pub fn is_enabled_impl(_id: u32) -> Option<bool> {
    None
}
