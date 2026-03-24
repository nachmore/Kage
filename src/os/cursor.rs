// Cross-platform cursor position detection

/// Get the current cursor position in screen coordinates.
/// Returns None if the cursor position cannot be determined.
pub fn get_cursor_position() -> Option<(i32, i32)> {
    crate::os::platform::cursor::get_cursor_position_impl()
}
