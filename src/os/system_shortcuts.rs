//! OS-reserved keyboard shortcuts ("system shortcuts") — e.g. macOS's
//! ⌘Space for Spotlight.
//!
//! A global hotkey that collides with an enabled system shortcut registers
//! fine but never fires: the OS consumes the keystroke first. This module
//! lets Kage *see* those shortcuts (to warn about collisions) and toggle
//! them (so `crate::system_shortcut_takeover` can hand ⌘Space to Kage while
//! it runs).
//!
//! Only macOS has an implementation. Windows/Linux return no shortcuts and
//! refuse to toggle anything, which makes every caller a no-op there.

/// One system shortcut as the OS currently has it bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemShortcut {
    /// Platform identifier (on macOS: the CGS symbolic-hotkey id, e.g. 64
    /// for "Show Spotlight search").
    pub id: u32,
    /// Whether the OS currently acts on it.
    pub enabled: bool,
    /// Kage modifier names (`Ctrl`, `Alt`, `Shift`, `Super`) in that order.
    pub modifiers: Vec<String>,
    /// Kage key name (same vocabulary as config / global-shortcut strings).
    pub key: String,
}

impl SystemShortcut {
    /// `Mods+Key` string in the format Kage stores hotkeys in.
    pub fn combo_string(&self) -> String {
        if self.modifiers.is_empty() {
            self.key.clone()
        } else {
            format!("{}+{}", self.modifiers.join("+"), self.key)
        }
    }
}

/// Every system shortcut that is bound to a key Kage could register.
/// Unbound entries and ones using modifiers Kage can't express (e.g. Fn+Q)
/// are omitted. Cost on macOS: one WindowServer round-trip per id
/// (~40 ms for the full scan) — call off the main thread.
pub fn list() -> Vec<SystemShortcut> {
    crate::os::platform::system_shortcuts::list_impl()
}

/// Look up one shortcut by id. `None` if it doesn't exist, is unbound, or
/// the platform has no system shortcuts.
pub fn get(id: u32) -> Option<SystemShortcut> {
    crate::os::platform::system_shortcuts::get_impl(id)
}

/// Enable or disable a system shortcut for the current login session.
/// Returns `true` on success. Not persisted to the user's preferences —
/// on macOS the change lasts until it's flipped back or the user logs out.
pub fn set_enabled(id: u32, enabled: bool) -> bool {
    crate::os::platform::system_shortcuts::set_enabled_impl(id, enabled)
}

/// Current enabled state, or `None` if unknown / unsupported.
pub fn is_enabled(id: u32) -> Option<bool> {
    crate::os::platform::system_shortcuts::is_enabled_impl(id)
}

/// The user's *saved* preference for a shortcut — what System Settings
/// shows — independent of the live session state Kage toggles. `Some(false)`
/// means they switched it off themselves; `None` means no explicit setting
/// (the OS default applies) or the platform has no such store.
pub fn user_preference(id: u32) -> Option<bool> {
    crate::os::platform::system_shortcuts::user_preference_impl(id)
}
