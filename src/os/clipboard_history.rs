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
/// Returns an empty vec on platforms without clipboard history support.
pub fn get_clipboard_history() -> Vec<ClipboardHistoryEntry> {
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::clipboard_history::get_clipboard_history_impl()
            .into_iter()
            .map(|e| ClipboardHistoryEntry {
                id: e.id,
                text: e.text,
                timestamp: e.timestamp,
                content_type: e.content_type,
            })
            .collect()
    }

    #[cfg(not(target_os = "windows"))]
    {
        vec![]
    }
}
