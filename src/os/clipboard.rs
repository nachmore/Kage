// Cross-platform clipboard operations

/// Read text from the system clipboard
pub fn read_clipboard() -> Option<String> {
    crate::os::platform::clipboard::read_clipboard_impl()
}

/// Write text to the system clipboard
pub fn write_clipboard(text: &str) {
    crate::os::platform::clipboard::write_clipboard_impl(text);
}

/// Capture the currently selected text from the active window.
#[allow(dead_code)]
pub fn capture_selection() -> Option<String> {
    crate::os::platform::clipboard::capture_selection_impl()
}

/// Phase 1 of two-phase selection capture: send the copy keystroke to the
/// foreground window. Must be called while the source window is still focused.
pub fn begin_selection_capture() -> SelectionCaptureToken {
    #[cfg(target_os = "windows")]
    {
        let (orig, seq) = crate::os::windows::clipboard::begin_selection_capture();
        SelectionCaptureToken { original_clipboard: orig, seq_before: seq }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let selection = capture_selection();
        SelectionCaptureToken { selection }
    }
}

/// Phase 2: poll the clipboard and return the captured text.
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
