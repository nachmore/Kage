//! Cross-platform input synthesis (keyboard, mouse, screen metrics).
//!
//! Same Pattern-A dispatch as the rest of `os/`: each function forwards
//! to `crate::os::platform::input::*_impl`. Windows synthesises via UIA
//! (`uiautomation`) + `SendInput`; macOS via CGEvent; Linux is
//! unsupported and returns a uniform error.
//!
//! All functions return `Result<String, String>` (success message /
//! error message) because the sole consumer is the MCP sidecar's tool
//! layer, which relays the string verbatim to the model. Policy checks
//! (e.g. the dangerous-key-combo confirmation) live in the tool layer,
//! not here.

/// Message returned by the Linux stubs (and any future unsupported path).
pub const UNSUPPORTED: &str = "not available on this platform";

/// Type literal text into the focused element. Newlines are sent as
/// Enter presses between lines.
pub fn type_text(text: &str) -> Result<String, String> {
    crate::os::platform::input::type_text_impl(text)
}

/// Press a key combo given as "ctrl+shift+s"-style text.
pub fn key_press(keys: &str) -> Result<String, String> {
    crate::os::platform::input::key_press_impl(keys)
}

/// Click at (x, y) — or at the current cursor position when None —
/// with the given button ("left"/"right") and count (2 = double-click).
pub fn click(x: Option<i32>, y: Option<i32>, button: &str, count: u32) -> Result<String, String> {
    crate::os::platform::input::click_impl(x, y, button, count)
}

/// Drag from (from_x, from_y) to (to_x, to_y) over `duration` seconds.
pub fn drag(
    from_x: i32,
    from_y: i32,
    to_x: i32,
    to_y: i32,
    duration: f64,
) -> Result<String, String> {
    crate::os::platform::input::drag_impl(from_x, from_y, to_x, to_y, duration)
}

/// Scroll "up"/"down" by `amount` notches, optionally moving the cursor
/// to (x, y) first.
pub fn scroll(
    direction: &str,
    amount: i32,
    x: Option<i32>,
    y: Option<i32>,
) -> Result<String, String> {
    crate::os::platform::input::scroll_impl(direction, amount, x, y)
}

/// Move the mouse cursor to (x, y).
pub fn move_mouse(x: i32, y: i32) -> Result<String, String> {
    crate::os::platform::input::move_mouse_impl(x, y)
}

/// Current cursor position.
pub fn get_cursor_position() -> Result<(i32, i32), String> {
    crate::os::platform::input::get_cursor_position_impl()
}

/// Primary display size in pixels.
pub fn get_screen_size() -> Result<(u32, u32), String> {
    crate::os::platform::input::get_screen_size_impl()
}
