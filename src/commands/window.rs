use crate::os;
use log::{error, info, warn};
use tauri::{Emitter, Manager, WebviewWindow};

/// Get cursor position via OS abstraction
fn get_cursor_position() -> Option<(i32, i32)> {
    os::get_cursor_position()
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
    let state: tauri::State<'_, crate::state::AppState> = app.state();

    // Don't show if frontend hasn't finished initializing yet
    if !state.frontend_ready.load(std::sync::atomic::Ordering::Acquire) {
        info!("Ignoring show_floating_at_mouse — frontend not ready");
        return;
    }

    // If already visible, hide it (toggle behavior)
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }

    position_floating_window_with_height(window, "mouse", None, None, Some(500));
    let _ = window.show();
    let _ = window.set_focus();
    state.updater.touch_activity();

    // Clear selection — clipboard mode doesn't use it
    if let Ok(mut sel) = state.last_selection.lock() {
        *sel = None;
    }
    let _ = app.emit("selection_captured", false);
}

/// Toggle the floating window visibility and position it
pub fn toggle_floating_window(window: &WebviewWindow) {
    let app = window.app_handle();
    let state: tauri::State<'_, crate::state::AppState> = app.state();

    // Don't show if frontend hasn't finished initializing yet
    if !state.frontend_ready.load(std::sync::atomic::Ordering::Acquire) {
        info!("Ignoring hotkey — frontend not ready");
        return;
    }

    let is_showing = !window.is_visible().unwrap_or(true);

    let config = state.config.lock().unwrap();
    let capture_enabled = config.system.capture_selection;
    let start_pos = config.ui.window_start_position.clone();
    let last_x = config.ui.last_window_x;
    let last_y = config.ui.last_window_y;
    drop(config);

    // Phase 1: send Ctrl+C while the source window is still focused (~20ms).
    // This must happen before we show our window.
    let capture_token = if is_showing && capture_enabled {
        Some(crate::os::clipboard::begin_selection_capture())
    } else {
        None
    };

    match window.is_visible() {
        Ok(is_visible) => {
            if is_visible {
                // Save position before hiding if "remember" mode
                if start_pos == "remember" {
                    if let Ok(pos) = window.outer_position() {
                        let state: tauri::State<'_, crate::state::AppState> = app.state();
                        let mut config = state.config.lock().unwrap();
                        config.ui.last_window_x = Some(pos.x);
                        config.ui.last_window_y = Some(pos.y);
                        let _ = config.save();
                    }
                }
                let _ = window.hide();
            } else {
                // Show window immediately — don't wait for clipboard poll
                position_floating_window(window, &start_pos, last_x, last_y);
                let _ = window.show();
                let _ = window.set_focus();

                // Record floating window activity for the updater idle check
                state.updater.touch_activity();

                // Phase 2: poll clipboard in background and deliver result via event.
                // The Ctrl+C was already sent, so this just waits for the clipboard change.
                if let Some(token) = capture_token {
                    let last_selection = state.last_selection.clone();
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
                    if let Ok(mut sel) = state.last_selection.lock() {
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
                    let win_size = window.inner_size().unwrap_or(tauri::PhysicalSize { width: 500, height: 60 });
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

                    let _ = window.set_position(tauri::Position::Physical(
                        tauri::PhysicalPosition { x, y },
                    ));
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
                    let win_size = window.inner_size().unwrap_or(tauri::PhysicalSize { width: 500, height: 60 });
                    if find_monitor_at_position(window, x + win_size.width as i32 / 2, y + 30).is_some() {
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

/// Center the floating window horizontally on the active monitor, 1/3 down
fn center_floating_on_active_monitor(window: &WebviewWindow) {
    if let Some(monitor) = get_active_monitor(window) {
        let pos = monitor.position();
        let size = monitor.size();
        let window_size = window.inner_size().unwrap_or(tauri::PhysicalSize { width: 500, height: 60 });
        let x = pos.x + (size.width as i32 - window_size.width as i32) / 2;
        let y = pos.y + size.height as i32 / 3;
        let _ = window.set_position(tauri::Position::Physical(
            tauri::PhysicalPosition { x, y },
        ));
    }
}

#[tauri::command]
pub async fn test_floating_window(app: tauri::AppHandle) -> Result<String, String> {
    info!("Testing floating window visibility");

    if let Some(window) = app.get_webview_window("floating") {
        let is_visible = window.is_visible().unwrap_or(false);

        if is_visible {
            window.hide().map_err(|e| format!("Failed to hide: {}", e))?;
            Ok("Window was visible, now hidden".to_string())
        } else {
            window.show().map_err(|e| format!("Failed to show: {}", e))?;
            window.set_focus().map_err(|e| format!("Failed to focus: {}", e))?;
            center_floating_on_active_monitor(&window);
            Ok("Window was hidden, now visible and positioned".to_string())
        }
    } else {
        Err("Floating window not found".to_string())
    }
}

#[tauri::command]
pub async fn start_drag_window(window: WebviewWindow) -> Result<(), String> {
    info!("Starting window drag");
    window.start_dragging().map_err(|e| {
        error!("Failed to start dragging: {}", e);
        e.to_string()
    })
}

/// Center a window on the active monitor (where the cursor is)
pub fn center_window_on_active_monitor(window: &WebviewWindow) {
    if let Some(monitor) = get_active_monitor(window) {
        let mon_pos = monitor.position();
        let mon_size = monitor.size();
        let win_size = window.inner_size().unwrap_or(tauri::PhysicalSize {
            width: 800,
            height: 600,
        });
        let x = mon_pos.x + (mon_size.width as i32 - win_size.width as i32) / 2;
        let y = mon_pos.y + (mon_size.height as i32 - win_size.height as i32) / 2;
        let _ = window.set_position(tauri::Position::Physical(
            tauri::PhysicalPosition { x, y },
        ));
    }
}

#[tauri::command]
pub async fn open_chat_window(app: tauri::AppHandle) -> Result<(), String> {
    info!("Opening chat window");

    if let Some(floating_window) = app.get_webview_window("floating") {
        let _ = floating_window.hide();
    }

    if let Some(window) = app.get_webview_window("main") {
        let state: tauri::State<'_, crate::state::AppState> = app.state();
        let config = state.config.lock().unwrap();
        let saved_w = config.ui.chat_window_width;
        let saved_h = config.ui.chat_window_height;
        let saved_x = config.ui.chat_window_x;
        let saved_y = config.ui.chat_window_y;
        drop(config);

        let scale = window.scale_factor().unwrap_or(1.0);

        // Get active monitor bounds
        let monitor = get_active_monitor(&window);
        let (mon_x, mon_y, mon_w, mon_h) = monitor.as_ref().map(|m| {
            let p = m.position();
            let s = m.size();
            (p.x, p.y, s.width as i32, s.height as i32)
        }).unwrap_or((0, 0, 1920, 1080));

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
                let _ = window.set_position(tauri::Position::Physical(
                    tauri::PhysicalPosition { x: cx, y: cy },
                ));
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
) -> Result<(), String> {
    let current_size = window.inner_size().map_err(|e| e.to_string())?;

    let target_width = width.unwrap_or(current_size.width);
    let target_height = height.unwrap_or(current_size.height);

    window
        .set_size(tauri::Size::Physical(tauri::PhysicalSize {
            width: target_width,
            height: target_height,
        }))
        .map_err(|e| e.to_string())
}


#[tauri::command]
pub async fn open_settings_window(app: tauri::AppHandle, section: Option<String>) -> Result<(), String> {
    info!("Opening settings window (section: {:?})", section);
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.set_focus();
        if let Some(ref s) = section {
            let _ = window.emit("navigate_settings_section", s);
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn show_context_menu(
    x: i32,
    y: i32,
    app: tauri::AppHandle,
) -> Result<(), String> {
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
            .set_position(tauri::Position::Physical(tauri::PhysicalPosition { x: final_x, y: final_y }))
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
pub async fn set_floating_opacity(
    app: tauri::AppHandle,
    opacity: f64,
) -> Result<(), String> {
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
) -> Result<(), String> {
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
    state: tauri::State<'_, crate::state::AppState>,
    x: i32,
    y: i32,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.ui.last_window_x = Some(x);
    config.ui.last_window_y = Some(y);
    config.save().map_err(|e| format!("Failed to save window position: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn save_chat_window_geometry(
    state: tauri::State<'_, crate::state::AppState>,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.ui.chat_window_width = width;
    config.ui.chat_window_height = height;
    config.ui.chat_window_x = Some(x);
    config.ui.chat_window_y = Some(y);
    config.save().map_err(|e| format!("Failed to save chat window geometry: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn get_last_selection(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<Option<String>, String> {
    let sel = state.last_selection.lock().map_err(|e| e.to_string())?;
    Ok(sel.clone())
}

#[tauri::command]
pub async fn set_notification_source(
    state: tauri::State<'_, crate::state::AppState>,
    source: String,
) -> Result<(), String> {
    if let Ok(mut s) = state.notification_source.lock() {
        *s = source;
    }
    Ok(())
}

#[tauri::command]
pub async fn show_notification_source_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    let source = state.notification_source.lock()
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
/// Called by the floating window frontend once initialization is complete.
/// Until this is called, hotkey presses are silently ignored to prevent
/// showing a half-initialized window.
#[tauri::command]
pub fn notify_frontend_ready(state: tauri::State<'_, crate::state::AppState>) {
    info!("Frontend signaled ready");
    state.frontend_ready.store(true, std::sync::atomic::Ordering::Release);
}
