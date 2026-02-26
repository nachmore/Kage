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
#[allow(dead_code)]
pub fn capture_selection() -> Option<String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::clipboard::capture_selection_impl() }

    #[cfg(target_os = "macos")]
    { crate::os::macos::clipboard::capture_selection_impl() }

    #[cfg(target_os = "linux")]
    { crate::os::linux::clipboard::capture_selection_impl() }
}

/// Phase 1 of two-phase selection capture: send the copy keystroke to the
/// foreground window. Must be called while the source window is still focused.
/// Returns an opaque token to pass to `finish_selection_capture`.
pub fn begin_selection_capture() -> SelectionCaptureToken {
    #[cfg(target_os = "windows")]
    {
        let (orig, seq) = crate::os::windows::clipboard::begin_selection_capture();
        SelectionCaptureToken { original_clipboard: orig, seq_before: seq }
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Non-Windows: no two-phase support, capture synchronously in begin
        let selection = capture_selection();
        SelectionCaptureToken { selection }
    }
}

/// Phase 2: poll the clipboard and return the captured text.
/// Can be called after the floating window is shown.
pub fn finish_selection_capture(token: SelectionCaptureToken) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::clipboard::finish_selection_capture(token.original_clipboard, token.seq_before)
    }

    #[cfg(not(target_os = "windows"))]
    {
        token.selection
    }
}

/// Opaque token carrying state between begin/finish selection capture.
pub struct SelectionCaptureToken {
    #[cfg(target_os = "windows")]
    original_clipboard: Option<String>,
    #[cfg(target_os = "windows")]
    seq_before: u32,
    #[cfg(not(target_os = "windows"))]
    selection: Option<String>,
}
