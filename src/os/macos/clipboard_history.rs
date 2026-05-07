// macOS clipboard history — stub.
//
// macOS doesn't expose a native clipboard history API; users typically
// rely on third-party apps (Paste, Maccy, Alfred). A future
// implementation could shell out to `pbpaste` repeatedly to maintain
// our own history, but that's a behaviour change, not a port. For now
// return empty and warn once.

use crate::os::clipboard_history::ClipboardHistoryEntry;
use std::sync::OnceLock;

static WARNED: OnceLock<()> = OnceLock::new();

pub fn get_clipboard_history_impl() -> Vec<ClipboardHistoryEntry> {
    WARNED.get_or_init(|| {
        log::warn!(
            "clipboard_history: macOS has no native clipboard-history API — \
             returning empty results."
        );
    });
    vec![]
}
