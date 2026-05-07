// Linux clipboard history — stub.
//
// Most Linux desktops don't expose clipboard history natively (KDE
// Klipper is one exception via D-Bus). A future implementation could
// target Klipper or maintain history via xclip/wl-clipboard polling.
// For now return empty and warn once.

use crate::os::clipboard_history::ClipboardHistoryEntry;
use std::sync::OnceLock;

static WARNED: OnceLock<()> = OnceLock::new();

pub fn get_clipboard_history_impl() -> Vec<ClipboardHistoryEntry> {
    WARNED.get_or_init(|| {
        log::warn!(
            "clipboard_history: Linux implementation not yet available — \
             returning empty results."
        );
    });
    vec![]
}
