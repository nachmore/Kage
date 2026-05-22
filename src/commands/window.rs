use crate::error::AppError;
use crate::lock_ext::LockExt;
use crate::os;
use log::{error, info, warn};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager, WebviewWindow};
use tauri_plugin_notification::NotificationExt;

/// Get cursor position via OS abstraction
fn get_cursor_position() -> Option<(i32, i32)> {
    os::get_cursor_position()
}

/// Last time we surfaced a "floating window won't show" notification.
/// Used to throttle the toast — without this, hotkey mashing into a
/// stuck WebView2 would queue dozens of notifications that the user
/// would only see when the system finally caught up.
static LAST_SHOW_FAILURE_NOTIFY: LazyLock<Mutex<Option<Instant>>> =
    LazyLock::new(|| Mutex::new(None));

/// Minimum gap between consecutive "show failed" toasts. Long enough
/// that a frustrated user mashing the hotkey only sees one toast per
/// burst, short enough that if the situation goes south again later
/// they get told.
const SHOW_FAILURE_NOTIFY_THROTTLE: Duration = Duration::from_secs(60);

/// Surface a system notification when a floating-window `show()` call
/// returns Err. The HRESULT 0x8007139F family ("Class not registered" /
/// "group or resource is not in the correct state") usually means the
/// WebView2 controller is wedged and the only fix the user has is to
/// restart kage. Without this, the failure is silent: the hotkey logs
/// say "show() result: Err(...)" but nothing surfaces to the human.
///
/// Rate-limited so a user mashing Alt+Space at a stuck window doesn't
/// queue up a stack of toasts; the throttle is per-process state so
/// every fresh launch starts unmuted.
fn notify_show_failed(app: &tauri::AppHandle, err: &tauri::Error) {
    {
        let mut guard = LAST_SHOW_FAILURE_NOTIFY.lock_or_recover();
        if let Some(prev) = *guard {
            if prev.elapsed() < SHOW_FAILURE_NOTIFY_THROTTLE {
                return;
            }
        }
        *guard = Some(Instant::now());
    }

    let body = format!(
        "Something's gone wrong with the webview ({}). Restart Kage from the tray to recover.",
        err
    );
    if let Err(e) = app
        .notification()
        .builder()
        .title("Kage's floating window won't show")
        .body(body)
        .show()
    {
        warn!("Failed to send recovery notification: {}", e);
    }
}

/// Find which monitor contains the given point
fn find_monitor_at_position(window: &WebviewWindow, x: i32, y: i32) -> Option<tauri::Monitor> {
    if let Ok(monitors) = window.available_monitors() {
        for monitor in monitors {
            let pos = monitor.position();
            let size = monitor.size();

            if x >= pos.x
                && x < pos.x + size.width as i32
                && y >= pos.y
                && y < pos.y + size.height as i32
            {
                return Some(monitor);
            }
        }
    }
    None
}

/// Get the active monitor (where cursor is) or fall back to primary
pub fn get_active_monitor(window: &WebviewWindow) -> Option<tauri::Monitor> {
    if let Some((cursor_x, cursor_y)) = get_cursor_position() {
        info!("Cursor position: ({}, {})", cursor_x, cursor_y);

        if let Some(monitor) = find_monitor_at_position(window, cursor_x, cursor_y) {
            info!("Found active monitor at cursor position");
            return Some(monitor);
        }
    }

    info!("Falling back to primary monitor");
    window.primary_monitor().ok().flatten()
}

/// Position the floating window at the mouse cursor, regardless of user config.
/// Show the floating window at the mouse cursor without selection capture.
/// Used by the clipboard hotkey — we don't want to send Ctrl+C when the user
/// just wants to browse their clipboard history.
pub fn show_floating_at_mouse(window: &WebviewWindow) {
    let app = window.app_handle();
    let ui: tauri::State<'_, crate::state::UiState> = app.state();
    let features: tauri::State<'_, crate::state::FeatureServices> = app.state();

    // Don't show during first-run wizard
    if !features.config.lock_or_recover().first_run_completed {
        info!("Ignoring show_floating_at_mouse — first run not completed");
        return;
    }

    // If already visible, hide it (toggle behavior)
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }

    // Position before showing so the window appears in its final spot —
    // showing first and snapping creates a visible jump.
    position_floating_window_with_height(window, "mouse", None, None, Some(500));
    if let Err(e) = window.show() {
        error!("[show_at_mouse] show() failed: {}", e);
        notify_show_failed(app, &e);
        return;
    }
    let _ = window.set_focus();
    features.updater.touch_activity();

    // Clear selection — clipboard mode doesn't use it
    if let Ok(mut sel) = ui.last_selection.lock() {
        *sel = None;
    }
    let _ = app.emit("selection_captured", false);
}

/// Toggle the floating window visibility and position it
pub fn toggle_floating_window(window: &WebviewWindow) {
    let app = window.app_handle();
    let ui_state: tauri::State<'_, crate::state::UiState> = app.state();
    let features: tauri::State<'_, crate::state::FeatureServices> = app.state();

    // Don't show during first-run wizard — user should complete onboarding first
    if !features.config.lock_or_recover().first_run_completed {
        info!("Ignoring hotkey — first run not completed");
        return;
    }

    let is_showing = !window.is_visible().unwrap_or(true);
    info!(
        "[toggle] entry: is_visible={:?}, taking={} branch",
        window.is_visible(),
        if is_showing { "show" } else { "hide" }
    );

    let config = features.config.lock_or_recover();
    let capture_enabled = config.system.capture_selection;
    let capture_blocklist = config.system.capture_selection_blocklist.clone();
    let start_pos = config.ui.window_start_position.clone();
    let last_x = config.ui.last_window_x;
    let last_y = config.ui.last_window_y;
    drop(config);

    // Capture the foreground window info before we steal focus (~1ms).
    // Grab it ahead of the Ctrl+C injection so we can also consult the
    // blocklist without a second foreground lookup.
    let source_window_info = if is_showing {
        crate::os::window_list::get_foreground_window_info()
    } else {
        None
    };

    // Phase 1: send Ctrl+C while the source window is still focused (~20ms).
    // This must happen before we show our window. Skip the injection when
    // the foreground app is blocklisted (terminals etc.) — sending Ctrl+C
    // there interrupts commands instead of copying text.
    let capture_token = if is_showing && capture_enabled {
        let fg_process = source_window_info
            .as_ref()
            .map(|(_, proc)| proc.as_str())
            .unwrap_or("");
        if crate::os::clipboard::is_process_blocklisted(fg_process, &capture_blocklist) {
            info!("[selection] Skipping capture — foreground app '{fg_process}' is blocklisted");
            None
        } else {
            Some(crate::os::clipboard::begin_selection_capture())
        }
    } else {
        None
    };

    match window.is_visible() {
        Ok(is_visible) => {
            if is_visible {
                // Save position before hiding if "remember" mode
                if start_pos == "remember" {
                    if let Ok(pos) = window.outer_position() {
                        let mut config = features.config.lock_or_recover();
                        config.ui.last_window_x = Some(pos.x);
                        config.ui.last_window_y = Some(pos.y);
                        let _ = config.save();
                    }
                }
                let hide_res = window.hide();
                info!("[toggle] hide() result: {:?}", hide_res);
                // Clear source window info when hiding
                if let Ok(mut sw) = ui_state.source_window.lock() {
                    *sw = None;
                }
            } else {
                // Restore saved launcher size if enabled
                {
                    let config = features.config.lock_or_recover();
                    if config.ui.remember_launcher_size {
                        if let (Some(w), Some(h)) =
                            (config.ui.launcher_width, config.ui.launcher_height)
                        {
                            let scale = window.scale_factor().unwrap_or(1.0);
                            let phys_w = (w as f64 * scale) as u32;
                            let phys_h = (h as f64 * scale) as u32;
                            let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
                                width: phys_w,
                                height: phys_h,
                            }));
                        }
                    }
                }
                // Position the window before showing so it appears at
                // its final location — showing first and then moving
                // produces a visible jump.
                position_floating_window(window, &start_pos, last_x, last_y);
                let show_res = window.show();
                let focus_res = window.set_focus();
                info!(
                    "[toggle] show() result: {:?}, set_focus() result: {:?}",
                    show_res, focus_res
                );
                if let Err(e) = show_res {
                    notify_show_failed(app, &e);
                    return;
                }

                // Record floating window activity for the updater idle check
                features.updater.touch_activity();

                // Store the source window info for screen context
                if let Ok(mut sw) = ui_state.source_window.lock() {
                    *sw = source_window_info;
                }

                // Phase 2: poll clipboard in background and deliver result via event.
                // The Ctrl+C was already sent, so this just waits for the clipboard change.
                if let Some(token) = capture_token {
                    let last_selection = ui_state.last_selection.clone();
                    let app_handle = app.clone();
                    std::thread::spawn(move || {
                        let selection = crate::os::clipboard::finish_selection_capture(token);
                        let has_sel = selection.as_ref().is_some_and(|s| !s.is_empty());
                        if let Ok(mut sel) = last_selection.lock() {
                            *sel = selection;
                        }
                        let _ = app_handle.emit("selection_captured", has_sel);
                    });
                } else {
                    if let Ok(mut sel) = ui_state.last_selection.lock() {
                        *sel = None;
                    }
                    let _ = app.emit("selection_captured", false);
                }
            }
        }
        Err(e) => {
            error!("Failed to check visibility: {}", e);
        }
    }
}

/// Position the floating window based on the configured strategy
fn position_floating_window(
    window: &WebviewWindow,
    strategy: &str,
    last_x: Option<i32>,
    last_y: Option<i32>,
) {
    position_floating_window_with_height(window, strategy, last_x, last_y, None);
}

fn position_floating_window_with_height(
    window: &WebviewWindow,
    strategy: &str,
    last_x: Option<i32>,
    last_y: Option<i32>,
    estimated_height: Option<u32>,
) {
    match strategy {
        "mouse" => {
            // Position near cursor, but ensure fully on-screen
            if let Some((cursor_x, cursor_y)) = get_cursor_position() {
                if let Some(monitor) = find_monitor_at_position(window, cursor_x, cursor_y) {
                    let mon_pos = monitor.position();
                    let mon_size = monitor.size();
                    let win_size = window.inner_size().unwrap_or(tauri::PhysicalSize {
                        width: 500,
                        height: 60,
                    });
                    // Use estimated height if provided (e.g. clipboard dropdown)
                    let effective_height = estimated_height.unwrap_or(win_size.height) as i32;

                    // Start at cursor, offset slightly down-right
                    let mut x = cursor_x;
                    let mut y = cursor_y + 20;

                    // Clamp to monitor bounds using the effective height
                    let max_x = mon_pos.x + mon_size.width as i32 - win_size.width as i32;
                    let max_y = mon_pos.y + mon_size.height as i32 - effective_height;
                    x = x.max(mon_pos.x).min(max_x);
                    y = y.max(mon_pos.y).min(max_y);

                    let _ = window
                        .set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }));
                    return;
                }
            }
            // Fallback to center
            center_floating_on_active_monitor(window);
        }
        "remember" => {
            // Use saved position if it's still on a valid monitor
            if let (Some(x), Some(y)) = (last_x, last_y) {
                if let Some(_monitor) = find_monitor_at_position(window, x, y) {
                    // Verify the window would be mostly visible
                    let win_size = window.inner_size().unwrap_or(tauri::PhysicalSize {
                        width: 500,
                        height: 60,
                    });
                    if find_monitor_at_position(window, x + win_size.width as i32 / 2, y + 30)
                        .is_some()
                    {
                        let _ = window.set_position(tauri::Position::Physical(
                            tauri::PhysicalPosition { x, y },
                        ));
                        return;
                    }
                }
            }
            // Fallback to center if saved position is off-screen
            center_floating_on_active_monitor(window);
        }
        _ => {
            // "center" — center on active monitor at 1/3 height
            center_floating_on_active_monitor(window);
        }
    }
}

/// Center a window on the active monitor.
/// `vertical_fraction` controls vertical placement (0.33 = 1/3 down, 0.5 = centered).
/// `default_size` is used if the window's inner size can't be determined.
pub fn center_on_active_monitor(
    window: &WebviewWindow,
    vertical_fraction: f64,
    default_size: tauri::PhysicalSize<u32>,
) {
    if let Some(monitor) = get_active_monitor(window) {
        let mon_pos = monitor.position();
        let mon_size = monitor.size();
        let win_size = window.inner_size().unwrap_or(default_size);
        let x = mon_pos.x + (mon_size.width as i32 - win_size.width as i32) / 2;
        let y = mon_pos.y
            + ((mon_size.height as f64 - win_size.height as f64) * vertical_fraction) as i32;
        let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }));
    }
}

/// Center the floating window horizontally on the active monitor, 1/3 down
pub fn center_floating_on_active_monitor(window: &WebviewWindow) {
    center_on_active_monitor(
        window,
        0.33,
        tauri::PhysicalSize {
            width: 500,
            height: 60,
        },
    );
}

#[tauri::command]
pub async fn test_floating_window(app: tauri::AppHandle) -> Result<String, AppError> {
    info!("Testing floating window visibility");

    if let Some(window) = app.get_webview_window("floating") {
        let is_visible = window.is_visible().unwrap_or(false);

        if is_visible {
            window
                .hide()
                .map_err(|e| format!("Failed to hide: {}", e))?;
            Ok("Window was visible, now hidden".to_string())
        } else {
            window
                .show()
                .map_err(|e| format!("Failed to show: {}", e))?;
            window
                .set_focus()
                .map_err(|e| format!("Failed to focus: {}", e))?;
            center_floating_on_active_monitor(&window);
            Ok("Window was hidden, now visible and positioned".to_string())
        }
    } else {
        Err(AppError::internal("Floating window not found"))
    }
}

#[tauri::command]
pub async fn start_drag_window(window: WebviewWindow) -> Result<(), AppError> {
    info!("Starting window drag");
    window.start_dragging().map_err(|e| {
        error!("Failed to start dragging: {}", e);
        AppError::internal(e.to_string())
    })
}

/// Center a window on the active monitor (where the cursor is)
pub fn center_window_on_active_monitor(window: &WebviewWindow) {
    center_on_active_monitor(
        window,
        0.5,
        tauri::PhysicalSize {
            width: 800,
            height: 600,
        },
    );
}

#[tauri::command]
pub async fn open_chat_window(app: tauri::AppHandle) -> Result<(), AppError> {
    info!("Opening chat window");

    if let Some(floating_window) = app.get_webview_window("floating") {
        let _ = floating_window.hide();
    }

    if let Some(window) = app.get_webview_window("main") {
        let features: tauri::State<'_, crate::state::FeatureServices> = app.state();
        let config = features.config.lock_or_recover();
        let saved_w = config.ui.chat_window_width;
        let saved_h = config.ui.chat_window_height;
        let saved_x = config.ui.chat_window_x;
        let saved_y = config.ui.chat_window_y;
        drop(config);

        let scale = window.scale_factor().unwrap_or(1.0);

        // Get active monitor bounds
        let monitor = get_active_monitor(&window);
        let (mon_x, mon_y, mon_w, mon_h) = monitor
            .as_ref()
            .map(|m| {
                let p = m.position();
                let s = m.size();
                (p.x, p.y, s.width as i32, s.height as i32)
            })
            .unwrap_or((0, 0, 1920, 1080));

        if saved_w > 0 && saved_h > 0 {
            // Clamp size to monitor dimensions
            let phys_w = ((saved_w as f64 * scale) as i32).min(mon_w);
            let phys_h = ((saved_h as f64 * scale) as i32).min(mon_h);
            let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
                width: phys_w as u32,
                height: phys_h as u32,
            }));

            // Restore position if saved, clamped to screen
            if let (Some(x), Some(y)) = (saved_x, saved_y) {
                let win_w = phys_w;
                let win_h = phys_h;
                let cx = x.max(mon_x).min(mon_x + mon_w - win_w);
                let cy = y.max(mon_y).min(mon_y + mon_h - win_h);
                let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
                    x: cx,
                    y: cy,
                }));
            } else {
                center_window_on_active_monitor(&window);
            }
        } else {
            // Default: 800x600 centered
            let def_w = (800.0 * scale) as u32;
            let def_h = (600.0 * scale) as u32;
            let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
                width: def_w.min(mon_w as u32),
                height: def_h.min(mon_h as u32),
            }));
            center_window_on_active_monitor(&window);
        }

        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
        crate::setup::update_activation_policy(&app);
    } else {
        warn!("Main window not found");
    }

    Ok(())
}

#[tauri::command]
pub async fn resize_floating_window(
    window: WebviewWindow,
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
        .map_err(|e| AppError::internal(e.to_string()))
}

#[tauri::command]
pub async fn open_settings_window(
    app: tauri::AppHandle,
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
    let window = if let Some(w) = app.get_webview_window("settings") {
        w
    } else {
        WebviewWindowBuilder::new(
            &app,
            "settings",
            tauri::WebviewUrl::App("settings.html".into()),
        )
        .title("Settings - Kage")
        .inner_size(800.0, 700.0)
        .min_inner_size(600.0, 450.0)
        .resizable(true)
        .center()
        .visible(false) // shown below after we know it built
        .build()
        .map_err(|e| AppError::internal(format!("Failed to build settings window: {}", e)))?
    };

    let _ = window.show();
    let _ = window.set_focus();
    if let Some(ref s) = section {
        let _ = window.emit("navigate_settings_section", s);
    }
    if let Some(ref sub) = sub_section {
        let _ = window.emit("navigate_settings_subsection", sub);
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

#[tauri::command]
pub async fn show_context_menu(x: i32, y: i32, app: tauri::AppHandle) -> Result<(), AppError> {
    crate::telemetry::track(&app, "context_menu_shown", None);
    if let Some(window) = app.get_webview_window("context-menu") {
        // Get the menu window size
        let win_size = window.outer_size().unwrap_or(tauri::PhysicalSize {
            width: 160,
            height: 220,
        });

        let mut final_x = x;
        let mut final_y = y;

        // Find which monitor the click is on and clamp to its bounds
        if let Some(monitor) = find_monitor_at_position(&window, x, y) {
            let mon_pos = monitor.position();
            let mon_size = monitor.size();
            let scale = monitor.scale_factor();

            let mon_right = mon_pos.x + mon_size.width as i32;
            let mon_bottom = mon_pos.y + mon_size.height as i32;
            let menu_w = (win_size.width as f64 / scale) as i32;
            let menu_h = (win_size.height as f64 / scale) as i32;

            // If menu would overflow right edge, flip to left of cursor
            if final_x + menu_w > mon_right {
                final_x = (mon_right - menu_w).max(mon_pos.x);
            }
            // If menu would overflow bottom edge, flip upward
            if final_y + menu_h > mon_bottom {
                final_y = (mon_bottom - menu_h).max(mon_pos.y);
            }
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
    }
    Ok(())
}

#[tauri::command]
pub async fn set_floating_opacity(app: tauri::AppHandle, opacity: f64) -> Result<(), AppError> {
    // Opacity is applied via CSS in the frontend (body opacity).
    // This command exists so the frontend can trigger it via config_updated.
    // The actual application happens in floating-theme.js loadAndApplyTheme().
    let _ = app; // Acknowledge the handle
    let _ = opacity;
    Ok(())
}

#[tauri::command]
pub async fn apply_chat_window_size(
    app: tauri::AppHandle,
    width: u32,
    height: u32,
) -> Result<(), AppError> {
    if let Some(window) = app.get_webview_window("main") {
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

#[tauri::command]
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

#[tauri::command]
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

#[tauri::command]
pub async fn get_last_selection(
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<Option<String>, AppError> {
    let sel = ui.last_selection.lock().map_err(|e| e.to_string())?;
    Ok(sel.clone())
}

#[tauri::command]
pub async fn set_notification_source(
    ui: tauri::State<'_, crate::state::UiState>,
    source: String,
) -> Result<(), AppError> {
    if let Ok(mut s) = ui.notification_source.lock() {
        *s = source;
    }
    Ok(())
}

#[tauri::command]
pub async fn show_notification_source_window(
    app: tauri::AppHandle,
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<(), AppError> {
    let source = ui
        .notification_source
        .lock()
        .map(|s| s.clone())
        .unwrap_or_else(|_| "floating".to_string());

    if source == "main" {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.set_focus();
        }
    } else if let Some(window) = app.get_webview_window("floating") {
        let _ = window.show();
        let _ = window.set_focus();
    }
    Ok(())
}
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
#[tauri::command]
pub fn notify_frontend_ready(ui: tauri::State<'_, crate::state::UiState>) {
    info!("Frontend signaled ready");
    ui.frontend_ready
        .store(true, std::sync::atomic::Ordering::Release);
}

/// Get the source window info (title, process_name) captured when the hotkey was pressed.
#[tauri::command]
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
#[tauri::command]
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

// ---------------------------------------------------------------------------
// Inline Assist — context-aware AI popup at cursor position
// ---------------------------------------------------------------------------

/// Show the inline assist popup at the cursor position.
/// Captures the selected text and foreground window info first.
#[tauri::command]
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
    if let Some(window) = app.get_webview_window("inline-assist") {
        info!(
            "Found inline-assist window, positioning at ({}, {})",
            cursor_pos.0, cursor_pos.1
        );
        // Position near cursor, with screen edge clamping
        let mut x = cursor_pos.0;
        let mut y = cursor_pos.1;

        if let Some(monitor) = find_monitor_at_position(&window, x, y) {
            let mon_pos = monitor.position();
            let mon_size = monitor.size();
            let scale = monitor.scale_factor();
            let win_w = (300.0 * scale) as i32;
            let win_h = (320.0 * scale) as i32;
            let mon_right = mon_pos.x + mon_size.width as i32;
            let mon_bottom = mon_pos.y + mon_size.height as i32;

            if x + win_w > mon_right {
                x = (mon_right - win_w).max(mon_pos.x);
            }
            if y + win_h > mon_bottom {
                y = (mon_bottom - win_h).max(mon_pos.y);
            }
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
            let _ = window_clone.emit("inline-assist-show", &payload);
        });
    } else {
        warn!("inline-assist window not found — is it defined in tauri.conf.json?");
    }

    Ok(())
}

/// Apply inline assist result: hide popup, focus source window, write to clipboard, paste.
#[tauri::command]
pub async fn inline_assist_apply(
    text: String,
    app: tauri::AppHandle,
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<(), AppError> {
    // Hide the inline assist window FIRST so it doesn't receive the paste
    if let Some(window) = app.get_webview_window("inline-assist") {
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

    // Write result to clipboard and paste
    crate::os::clipboard::write_clipboard(&text);
    std::thread::sleep(std::time::Duration::from_millis(30));
    crate::os::simulate_paste();

    Ok(())
}
