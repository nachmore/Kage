// Cross-platform clipboard operations

/// Read text from the system clipboard
pub fn read_clipboard() -> Option<String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::clipboard::read_clipboard_impl() }

    #[cfg(target_os = "macos")]
    { crate::os::macos::clipboard::read_clipboard_impl() }

    #[cfg(target_os = "linux")]
    { crate::os::linux::clipboard::read_clipboard_impl() }
}

/// Write text to the system clipboard
pub fn write_clipboard(text: &str) {
    #[cfg(target_os = "windows")]
    { crate::os::windows::clipboard::write_clipboard_impl(text); }

    #[cfg(target_os = "macos")]
    { crate::os::macos::clipboard::write_clipboard_impl(text); }

    #[cfg(target_os = "linux")]
    { crate::os::linux::clipboard::write_clipboard_impl(text); }
}

/// Capture the currently selected text from the active window.
/// This works by simulating Ctrl+C / Cmd+C, reading the clipboard,
/// and restoring the original clipboard content.
/// Returns None if no selection could be captured.
pub fn capture_selection() -> Option<String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::clipboard::capture_selection_impl() }

    #[cfg(target_os = "macos")]
    { crate::os::macos::clipboard::capture_selection_impl() }

    #[cfg(target_os = "linux")]
    { crate::os::linux::clipboard::capture_selection_impl() }
}
