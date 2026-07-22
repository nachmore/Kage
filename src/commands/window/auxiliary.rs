//! Settings, context-menu, and window-state commands.

use super::floating::{clamp_into_monitor, find_monitor_at_position};
use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use crate::window_labels;
use log::info;
use tauri::{Manager, WebviewWindow};

pub async fn resize_floating_window<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<(), AppError> {
    let current_size = window.inner_size().map_err(|e| e.to_string())?;

    let target_width = width.unwrap_or(current_size.width);
    let target_height = height.unwrap_or(current_size.height);

    window
        .set_size(tauri::Size::Physical(tauri::PhysicalSize {
            width: target_width,
            height: target_height,
        }))
        .map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.window.show_failed",
                &[("reason", &e.to_string())],
            )
        })
}

pub async fn open_settings_window<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    section: Option<String>,
    sub_section: Option<String>,
) -> Result<(), AppError> {
    use tauri::WebviewWindowBuilder;
    info!(
        "Opening settings window (section: {:?}, sub: {:?})",
        section, sub_section
    );
    // Reuse an existing settings window if it's already up (user
    // already opened it, then dismissed without closing). Otherwise
    // build a fresh one — settings is excluded from the initial
    // tauri.conf.json windows array so we don't pay for the WebView2
    // process + its JS init at every app launch (it calls
    // detect_agents on startup, which used to flash DOS windows for
    // each preset's `where`/`--version` probe).
    let already_open = app.get_webview_window(window_labels::SETTINGS).is_some();
    let window = if let Some(w) = app.get_webview_window(window_labels::SETTINGS) {
        w
    } else {
        // Encode section/subsection in the URL so the fresh window's
        // boot path can apply them synchronously. Going through the
        // event channel races: we'd `emit()` immediately after `show()`
        // but the new webview's listener isn't registered until its
        // JS finishes booting, so the event is silently lost. Query
        // params survive that race because they're attached to the
        // initial document URL, available to JS the moment it runs.
        // Section + subsection IDs are caller-supplied but always
        // simple ASCII identifiers (e.g. 'updates', 'changelog') —
        // they're code-defined, not user-input. Filter to that shape
        // defensively so a future caller passing user content can't
        // smuggle URL syntax. Anything outside [A-Za-z0-9_-] is
        // dropped silently.
        fn safe_id(s: &str) -> String {
            s.chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect()
        }
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(s) = section.as_deref() {
            let safe = safe_id(s);
            if !safe.is_empty() {
                params.push(("section", safe));
            }
        }
        if let Some(s) = sub_section.as_deref() {
            let safe = safe_id(s);
            if !safe.is_empty() {
                params.push(("subsection", safe));
            }
        }
        let mut url = String::from("settings.html");
        for (i, (k, v)) in params.iter().enumerate() {
            url.push(if i == 0 { '?' } else { '&' });
            url.push_str(k);
            url.push('=');
            url.push_str(v);
        }
        WebviewWindowBuilder::new(
            &app,
            window_labels::SETTINGS,
            tauri::WebviewUrl::App(url.into()),
        )
        .title("Settings - Kage")
        .inner_size(800.0, 700.0)
        .min_inner_size(600.0, 450.0)
        .resizable(true)
        .center()
        .visible(false) // shown below after we know it built
        .build()
        .map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.window.create_failed",
                &[("reason", &e.to_string())],
            )
        })?
    };

    let _ = window.show();
    let _ = window.set_focus();
    // Only emit nav events when the window already existed — the
    // freshly-built case picked them up via URL params above. Doing
    // both would re-invoke switchSection redundantly (harmless but
    // noisy) and could trigger a flash of a different section.
    if already_open {
        // The settings window is the only listener — `WebviewWindow::emit`
        // would fan out to every webview, but the cost adds up if a user
        // re-opens settings repeatedly. Scope it.
        if let Some(ref s) = section {
            crate::event_targets::emit_to_settings(&app, "navigate_settings_section", s);
        }
        if let Some(ref sub) = sub_section {
            crate::event_targets::emit_to_settings(&app, "navigate_settings_subsection", sub);
        }
    }
    crate::setup::update_activation_policy(&app);
    crate::telemetry::track(
        &app,
        "settings_opened",
        section
            .as_deref()
            .map(|s| serde_json::json!({ "section": s })),
    );
    Ok(())
}

/// Logical (DIP) size of the context-menu window. Kept here as a single
/// source of truth — used both for the initial WebviewWindowBuilder and
/// for clamping math when the window's outer_size() isn't available
/// yet (the very first show, before the WebView2 process has reported
/// its rendered size).
const CONTEXT_MENU_LOGICAL_W: f64 = 160.0;
const CONTEXT_MENU_LOGICAL_H: f64 = 220.0;

pub async fn show_context_menu<R: tauri::Runtime>(
    x: i32,
    y: i32,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    use tauri::WebviewWindowBuilder;
    crate::telemetry::track(&app, "context_menu_shown", None);

    // Build the window on first show. Excluded from tauri.conf.json's
    // initial windows so we don't pay for a WebView2 process at every
    // launch — the menu is opened rarely and via explicit user action.
    let window = if let Some(w) = app.get_webview_window(window_labels::CONTEXT_MENU) {
        w
    } else {
        let w = WebviewWindowBuilder::new(
            &app,
            window_labels::CONTEXT_MENU,
            tauri::WebviewUrl::App("context-menu.html".into()),
        )
        .title("")
        .inner_size(CONTEXT_MENU_LOGICAL_W, CONTEXT_MENU_LOGICAL_H)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .build()
        .map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.window.create_failed",
                &[("reason", &e.to_string())],
            )
        })?;
        // Match what configure_transparent_windows did for the
        // preloaded version. set_shadow is Windows-only on the trait.
        let _ = w.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        #[cfg(target_os = "windows")]
        let _ = w.set_shadow(false);
        w
    };

    let mut final_x = x;
    let mut final_y = y;

    // Find which monitor the click is on and clamp to its bounds
    if let Some(monitor) = find_monitor_at_position(&window, x, y) {
        let scale = monitor.scale_factor();

        // outer_size() can be 0×0 on a freshly-built window that hasn't
        // rendered yet. Fall back to the configured logical size in that
        // case so first-show clamping still works.
        let outer = window.outer_size().unwrap_or(tauri::PhysicalSize {
            width: 0,
            height: 0,
        });
        let menu_w = if outer.width > 0 {
            (outer.width as f64 / scale) as i32
        } else {
            CONTEXT_MENU_LOGICAL_W as i32
        };
        let menu_h = if outer.height > 0 {
            (outer.height as f64 / scale) as i32
        } else {
            CONTEXT_MENU_LOGICAL_H as i32
        };

        // Flip the menu left/up if it would overflow the right/bottom edge.
        (final_x, final_y) = clamp_into_monitor(&monitor, final_x, final_y, menu_w, menu_h);
    }

    window
        .set_position(tauri::Position::Physical(tauri::PhysicalPosition {
            x: final_x,
            y: final_y,
        }))
        .map_err(|e| format!("Failed to position context menu: {}", e))?;
    window
        .show()
        .map_err(|e| format!("Failed to show context menu: {}", e))?;
    window
        .set_focus()
        .map_err(|e| format!("Failed to focus context menu: {}", e))?;
    Ok(())
}

pub async fn set_floating_opacity<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    opacity: f64,
) -> Result<(), AppError> {
    // Opacity is applied via CSS in the frontend (body opacity).
    // This command exists so the frontend can trigger it via config_updated.
    // The actual application happens in floating-theme.js loadAndApplyTheme().
    let _ = app; // Acknowledge the handle
    let _ = opacity;
    Ok(())
}

pub async fn apply_chat_window_size<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    width: u32,
    height: u32,
) -> Result<(), AppError> {
    if let Some(window) = app.get_webview_window(window_labels::MAIN) {
        let scale = window.scale_factor().unwrap_or(1.0);
        let phys_width = (width as f64 * scale) as u32;
        let phys_height = (height as f64 * scale) as u32;
        window
            .set_size(tauri::Size::Physical(tauri::PhysicalSize {
                width: phys_width,
                height: phys_height,
            }))
            .map_err(|e| format!("Failed to resize chat window: {}", e))?;
    }
    Ok(())
}

pub async fn save_window_position(
    features: tauri::State<'_, crate::state::FeatureServices>,
    x: i32,
    y: i32,
) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();
    config.ui.last_window_x = Some(x);
    config.ui.last_window_y = Some(y);
    config
        .save()
        .map_err(|e| format!("Failed to save window position: {}", e))?;
    Ok(())
}

pub async fn save_chat_window_geometry(
    features: tauri::State<'_, crate::state::FeatureServices>,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();
    config.ui.chat_window_width = width;
    config.ui.chat_window_height = height;
    config.ui.chat_window_x = Some(x);
    config.ui.chat_window_y = Some(y);
    config
        .save()
        .map_err(|e| format!("Failed to save chat window geometry: {}", e))?;
    Ok(())
}

pub async fn get_last_selection(
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<Option<String>, AppError> {
    let sel = ui.last_selection.lock().map_err(|e| e.to_string())?;
    Ok(sel.clone())
}

// `set_notification_source` and `show_notification_source_window`
// existed under the single-session model where one global string told
// the backend which window owned the in-flight prompt. Permission
// routing is now done per-session via
// `UiState.pending_prompt_originators` — see
// `commands::messaging::handle_permission_notification`.
/// Called by the floating window frontend once `init()` completes.
/// Used purely as a diagnostic / telemetry beacon — the backend log line
/// "Frontend signaled ready" is the canonical signal that the floating
/// JS bundle loaded and ran end-to-end. If you ever see hotkey presses
/// without this line preceding them, the JS module graph aborted (e.g.
/// a top-level `window.__TAURI__` access racing the global injection).
///
/// We used to gate `show_floating_at_mouse` / `toggle_floating_window`
/// on the boolean this command sets, to suppress a brief flash of an
/// unthemed window during the first ~200ms of startup. That gate was
/// removed because Tauri doesn't load the floating window's HTML+JS
/// until the window first shows (the window is configured
/// `visible: false`), which made the gate a permanent deadlock if the
/// JS init ever failed: window never shows → JS never runs →
/// frontend_ready never flips → window never shows. A one-frame flash
/// is fine; a permanently dead app is not.
pub fn notify_frontend_ready(ui: tauri::State<'_, crate::state::UiState>) {
    info!("Frontend signaled ready");
    ui.frontend_ready
        .store(true, std::sync::atomic::Ordering::Release);
}

/// Get the source window info (title, process_name) captured when the hotkey was pressed.
pub async fn get_source_window(
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<Option<serde_json::Value>, AppError> {
    let sw = ui.source_window.lock().map_err(|e| e.to_string())?;
    match sw.as_ref() {
        Some((title, process_name)) => Ok(Some(serde_json::json!({
            "title": title,
            "processName": process_name,
        }))),
        None => Ok(None),
    }
}

/// Get a shallow accessibility tree snapshot of the source window for screen context.
/// Uses depth 2 to keep it fast and small. Returns a compact text representation.
pub async fn get_screen_context(
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<Option<String>, AppError> {
    let title = {
        let sw = ui.source_window.lock().map_err(|e| e.to_string())?;
        match sw.as_ref() {
            Some((title, _)) => title.clone(),
            None => return Ok(None),
        }
    }; // MutexGuard dropped here

    // Spawn on a blocking thread since UI Automation is COM-based
    let result = tokio::task::spawn_blocking(move || {
        crate::os::accessibility::get_ui_tree(Some(&title), 2, false)
    })
    .await
    .map_err(|e| format!("Join error: {}", e))?;

    match result {
        Ok(tree) => {
            let text = tree.to_text(0, 2);
            // Cap at 4KB to keep token usage reasonable
            if text.len() > 4096 {
                Ok(Some(text[..4096].to_string() + "\n... (truncated)"))
            } else {
                Ok(Some(text))
            }
        }
        Err(e) => {
            log::warn!("Screen context capture failed: {}", e);
            Ok(None)
        }
    }
}
