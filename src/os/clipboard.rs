// Cross-platform clipboard operations.
//
// Each platform's `clipboard` submodule defines its own opaque
// `SelectionCaptureToken` type — the data needed to complete a
// two-phase capture is genuinely different per OS (Windows uses the
// clipboard sequence number; macOS/Linux just capture synchronously
// and stash the result). We re-export the platform's token here so
// callers see a uniform name.

pub use crate::os::platform::clipboard::SelectionCaptureToken;

/// Read text from the system clipboard.
pub fn read_clipboard() -> Option<String> {
    crate::os::platform::clipboard::read_clipboard_impl()
}

/// Write text to the system clipboard.
pub fn write_clipboard(text: &str) {
    crate::os::platform::clipboard::write_clipboard_impl(text);
}

/// Capture the currently selected text from the active window.
#[allow(dead_code)]
pub fn capture_selection() -> Option<String> {
    crate::os::platform::clipboard::capture_selection_impl()
}

/// Phase 1 of two-phase selection capture: send the copy keystroke to
/// the foreground window. Must be called while the source window is
/// still focused.
pub fn begin_selection_capture() -> SelectionCaptureToken {
    crate::os::platform::clipboard::begin_selection_capture_impl()
}

/// Phase 2: poll the clipboard and return the captured text.
pub fn finish_selection_capture(token: SelectionCaptureToken) -> Option<String> {
    crate::os::platform::clipboard::finish_selection_capture_impl(token)
}
