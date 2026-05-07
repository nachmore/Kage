// Windows clipboard history using WinRT Clipboard API
//
// Requires the user to have clipboard history enabled (Win+V or Settings > System > Clipboard).
// Uses Windows.ApplicationModel.DataTransfer.Clipboard.GetHistoryItemsAsync().

use crate::os::clipboard_history::ClipboardHistoryEntry;
use log::{info, warn};

pub fn get_clipboard_history_impl() -> Vec<ClipboardHistoryEntry> {
    // The WinRT Clipboard API requires STA (single-threaded apartment).
    // Tauri's async runtime uses MTA threads, so we spawn a dedicated STA thread.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        unsafe {
            // Initialize COM as STA on this thread
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );
        }
        let result = get_clipboard_history_sta();
        let _ = tx.send(result);
    });

    match rx.recv() {
        Ok(entries) => entries,
        Err(e) => {
            warn!("Clipboard history thread failed: {}", e);
            vec![]
        }
    }
}

fn get_clipboard_history_sta() -> Vec<ClipboardHistoryEntry> {
    use windows::ApplicationModel::DataTransfer::{
        Clipboard, ClipboardHistoryItem, ClipboardHistoryItemsResult, DataPackageView,
    };
    use windows_collections::IVectorView;

    // Get history items (async WinRT call, block on it).
    // Note: IsHistoryEnabled() is unreliable for desktop (non-UWP) apps,
    // so we skip it and just try to fetch — empty result means disabled.
    let result: ClipboardHistoryItemsResult = match Clipboard::GetHistoryItemsAsync() {
        Ok(op) => match op.join() {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to get clipboard history: {}", e);
                return vec![];
            }
        },
        Err(e) => {
            warn!("Failed to request clipboard history: {}", e);
            return vec![];
        }
    };

    let items: IVectorView<ClipboardHistoryItem> = match result.Items() {
        Ok(items) => items,
        Err(e) => {
            warn!("Failed to read clipboard history items: {}", e);
            return vec![];
        }
    };

    // Check the status — AccessDenied means clipboard history is disabled
    if let Ok(status) = result.Status() {
        use windows::ApplicationModel::DataTransfer::ClipboardHistoryItemsResultStatus;
        match status {
            ClipboardHistoryItemsResultStatus::Success => {}
            ClipboardHistoryItemsResultStatus::AccessDenied => {
                info!("Clipboard history access denied — feature may be disabled");
                return vec![];
            }
            ClipboardHistoryItemsResultStatus::ClipboardHistoryDisabled => {
                info!("Clipboard history is disabled in Windows settings");
                return vec![];
            }
            _ => {
                warn!("Clipboard history returned unexpected status: {:?}", status);
                return vec![];
            }
        }
    }

    let mut entries = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        let content: DataPackageView = match item.Content() {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Try to get text content
        let has_text = content
            .Contains(&windows::core::HSTRING::from("Text"))
            .unwrap_or(false);

        if has_text {
            if let Ok(op) = content.GetTextAsync() {
                if let Ok(text) = op.join() {
                    let text_str: String = text.to_string();
                    if !text_str.is_empty() {
                        // Get timestamp
                        let timestamp = item
                            .Timestamp()
                            .map(|ts| {
                                // Windows DateTime is 100-nanosecond intervals since 1601-01-01
                                let unix_nanos = ts.UniversalTime - 116_444_736_000_000_000;
                                let secs = unix_nanos / 10_000_000;
                                let nanos = ((unix_nanos % 10_000_000) * 100) as u32;
                                chrono::DateTime::from_timestamp(secs, nanos)
                                    .map(|dt| dt.to_rfc3339())
                                    .unwrap_or_default()
                            })
                            .unwrap_or_default();

                        entries.push(ClipboardHistoryEntry {
                            id: format!("clip_{}", i),
                            text: text_str,
                            timestamp,
                            content_type: "text".to_string(),
                        });
                    }
                }
            }
        } else {
            // Non-text content (images, etc.) — note it but skip for now
            let has_bitmap = content
                .Contains(&windows::core::HSTRING::from("Bitmap"))
                .unwrap_or(false);
            if has_bitmap {
                let timestamp = item
                    .Timestamp()
                    .map(|ts| {
                        let unix_nanos = ts.UniversalTime - 116_444_736_000_000_000;
                        let secs = unix_nanos / 10_000_000;
                        let nanos = ((unix_nanos % 10_000_000) * 100) as u32;
                        chrono::DateTime::from_timestamp(secs, nanos)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();

                entries.push(ClipboardHistoryEntry {
                    id: format!("clip_{}", i),
                    text: "[Image]".to_string(),
                    timestamp,
                    content_type: "image".to_string(),
                });
            }
        }
    }

    info!("Retrieved {} clipboard history entries", entries.len());
    entries
}
