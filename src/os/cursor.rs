// Cross-platform cursor position detection

/// Get the current cursor position in screen coordinates
/// Returns None if the cursor position cannot be determined
pub fn get_cursor_position() -> Option<(i32, i32)> {
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::cursor::get_cursor_position_impl()
    }
    
    #[cfg(target_os = "macos")]
    {
        crate::os::macos::cursor::get_cursor_position_impl()
    }
    
    #[cfg(target_os = "linux")]
    {
        crate::os::linux::cursor::get_cursor_position_impl()
    }
}
