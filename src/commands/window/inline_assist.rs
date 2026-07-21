//! Inline-assist window commands and clipboard application.

use super::floating::{clamp_into_monitor, find_monitor_at_position, get_cursor_position};
use crate::error::{AppError, ErrorKind};
use crate::events;
use crate::lock_ext::LockExt;
use crate::window_labels;
use log::{info, warn};
use tauri::Manager;

// ---------------------------------------------------------------------------
// Inline Assist — context-aware AI popup at cursor position
// ---------------------------------------------------------------------------

/// Show the inline assist popup at the cursor position.
/// Captures the selected text and foreground window info first.
pub async fn show_inline_assist(app: tauri::AppHandle) -> Result<(), AppError> {
    // When called as a Tauri command (not from hotkey), capture here
    let source_info = crate::os::window_list::get_foreground_window_info();
    let features: tauri::State<'_, crate::state::FeatureServices> = app.state();
    let blocklist = features
        .config
        .lock_or_recover()
        .system
        .capture_selection_blocklist
        .clone();
    let fg_process = source_info
        .as_ref()
        .map(|(_, proc)| proc.as_str())
        .unwrap_or("");
    let selection = if crate::os::clipboard::is_process_blocklisted(fg_process, &blocklist) {
        info!("[inline-assist] Skipping capture — foreground app '{fg_process}' is blocklisted");
        None
    } else {
        let capture_token = crate::os::clipboard::begin_selection_capture();
        crate::os::clipboard::finish_selection_capture(capture_token)
    };
    let cursor_pos = get_cursor_position().unwrap_or((500, 500));

    show_inline_assist_with_context(app, source_info, selection, cursor_pos).await
}

/// Show the inline assist popup with pre-captured context.
/// Called from the hotkey handler where capture happens synchronously.
pub async fn show_inline_assist_with_context(
    app: tauri::AppHandle,
    source_info: Option<(String, String)>,
    selection: Option<String>,
    cursor_pos: (i32, i32),
) -> Result<(), AppError> {
    info!(
        "show_inline_assist_with_context: source={:?}, selection_len={}, cursor=({},{})",
        source_info,
        selection.as_ref().map(|s| s.len()).unwrap_or(0),
        cursor_pos.0,
        cursor_pos.1
    );

    // Single firing point for both the command-driven and hotkey-driven
    // entry. has_selection lets us tell whether inline-assist is being
    // used to operate on highlighted text vs as a freeform prompt.
    crate::telemetry::track(
        &app,
        "inline_assist_shown",
        Some(serde_json::json!({
            "has_selection": selection.is_some(),
        })),
    );

    let ui: tauri::State<'_, crate::state::UiState> = app.state();

    // Store source window info for paste-back
    if let Ok(mut sw) = ui.source_window.lock() {
        *sw = source_info.clone();
    }

    // Show the inline assist window at cursor
    if let Some(window) = app.get_webview_window(window_labels::INLINE_ASSIST) {
        info!(
            "Found inline-assist window, positioning at ({}, {})",
            cursor_pos.0, cursor_pos.1
        );
        // Position near cursor, with screen edge clamping
        let mut x = cursor_pos.0;
        let mut y = cursor_pos.1;

        if let Some(monitor) = find_monitor_at_position(&window, x, y) {
            let scale = monitor.scale_factor();
            let win_w = (300.0 * scale) as i32;
            let win_h = (320.0 * scale) as i32;
            (x, y) = clamp_into_monitor(&monitor, x, y, win_w, win_h);
        }

        let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }));
        let _ = window.show();
        let _ = window.set_focus();

        // Send the context to the frontend (delay to ensure JS module is loaded)
        let (app_name, title) = source_info.unwrap_or_default();
        let sel = selection.unwrap_or_default();
        let window_clone = window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let payload = serde_json::json!({
                "selection": sel,
                "app": app_name,
                "title": title,
            });
            crate::event_targets::emit_to_self(&window_clone, events::INLINE_ASSIST_SHOW, &payload);
        });
    } else {
        warn!("inline-assist window not found — is it defined in tauri.conf.json?");
    }

    Ok(())
}

/// Apply inline assist result: hide popup, focus source window, write to clipboard, paste.
pub async fn inline_assist_apply(
    text: String,
    app: tauri::AppHandle,
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<(), AppError> {
    // Hide the inline assist window FIRST so it doesn't receive the paste
    if let Some(window) = app.get_webview_window(window_labels::INLINE_ASSIST) {
        let _ = window.hide();
    }

    // Small delay to let the window fully hide
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Focus the source window
    let source_title = {
        let sw = ui.source_window.lock().map_err(|e| e.to_string())?;
        sw.as_ref().map(|(title, _)| title.clone())
    };

    if let Some(title) = source_title {
        let windows = crate::os::list_windows();
        if let Some(win) = windows.iter().find(|w| w.title.contains(&title)) {
            let _ = crate::os::window_list::focus_window(win.handle);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    // Write result to clipboard and paste. If the clipboard write didn't
    // land (another app held the clipboard), DON'T paste — the clipboard
    // still holds its previous contents and we'd paste stale text over the
    // user's selection. Surface it as an error instead.
    if !crate::os::clipboard::write_clipboard(&text) {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.clipboard.write_failed",
            &[],
        ));
    }
    std::thread::sleep(std::time::Duration::from_millis(30));
    crate::os::simulate_paste();

    Ok(())
}
