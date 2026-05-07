// Cross-platform clipboard history

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ClipboardHistoryEntry {
    pub id: String,
    pub text: String,
    pub timestamp: String,
    pub content_type: String,
}

/// Get clipboard history items from the OS.
/// Returns an empty vec on platforms without clipboard history support
/// (the platform stub logs a once-per-process warn explaining why).
pub fn get_clipboard_history() -> Vec<ClipboardHistoryEntry> {
    crate::os::platform::clipboard_history::get_clipboard_history_impl()
}
