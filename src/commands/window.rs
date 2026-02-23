use crate::os;
use log::{error, info, warn};
use tauri::{Manager, WebviewWindow};

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
        println!("     Cursor position: ({}, {})", cursor_x, cursor_y);

        if let Some(monitor) = find_monitor_at_position(window, cursor_x, cursor_y) {
            println!("     Found active monitor at cursor position");
            return Some(monitor);
        }
    }

    println!("     Falling back to primary monitor");
    window.primary_monitor().ok().flatten()
}

/// Toggle the floating window visibility and position it
pub fn toggle_floating_window(window: &WebviewWindow) {
    // Read config for positioning preference
    let app = window.app_handle();
    let state: tauri::State<'_, crate::state::AppState> = app.state();
    let config = tauri::async_runtime::block_on(state.config.lock());
    let start_pos = config.ui.window_start_position.clone();
    let last_x = config.ui.last_window_x;
    let last_y = config.ui.last_window_y;
    drop(config);

    match window.is_visible() {
        Ok(is_visible) => {
            if is_visible {
                // Save position before hiding if "remember" mode
                if start_pos == "remember" {
                    if let Ok(pos) = window.outer_position() {
                        let state: tauri::State<'_, crate::state::AppState> = app.state();
                        let mut config = tauri::async_runtime::block_on(state.config.lock());
                        config.ui.last_window_x = Some(pos.x);
                        config.ui.last_window_y = Some(pos.y);
                        let _ = config.save();
                    }
                }
                let _ = window.hide();
            } else {
                let _ = window.show();
                let _ = window.set_focus();
                position_floating_window(window, &start_pos, last_x, last_y);
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
    match strategy {
        "mouse" => {
            // Position near cursor, but ensure fully on-screen
            if let Some((cursor_x, cursor_y)) = get_cursor_position() {
                if let Some(monitor) = find_monitor_at_position(window, cursor_x, cursor_y) {
                    let mon_pos = monitor.position();
                    let mon_size = monitor.size();
                    let win_size = window.inner_size().unwrap_or(tauri::PhysicalSize { width: 500, height: 60 });

                    // Start at cursor, offset slightly down-right
                    let mut x = cursor_x;
                    let mut y = cursor_y + 20;

                    // Clamp to monitor bounds
                    let max_x = mon_pos.x + mon_size.width as i32 - win_size.width as i32;
                    let max_y = mon_pos.y + mon_size.height as i32 - win_size.height as i32;
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
        // Apply configured chat window size
        let state: tauri::State<'_, crate::state::AppState> = app.state();
        let config = state.config.lock().await;
        let width = config.ui.chat_window_width;
        let height = config.ui.chat_window_height;
        drop(config);

        let scale = window.scale_factor().unwrap_or(1.0);
        let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
            width: (width as f64 * scale) as u32,
            height: (height as f64 * scale) as u32,
        }));

        center_window_on_active_monitor(&window);
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    } else {
        warn!("Main window not found, this shouldn't happen");
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
pub async fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    info!("Opening settings window");
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.set_focus();
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
        window
            .set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }))
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
    let mut config = state.config.lock().await;
    config.ui.last_window_x = Some(x);
    config.ui.last_window_y = Some(y);
    config.save().map_err(|e| format!("Failed to save window position: {}", e))?;
    Ok(())
}
