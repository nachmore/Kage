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
    match window.is_visible() {
        Ok(is_visible) => {
            println!("   Window visible state: {}", is_visible);
            if is_visible {
                println!("  → Hiding floating window");
                match window.hide() {
                    Ok(_) => println!("     ✅ Window hidden successfully"),
                    Err(e) => println!("     ❌ Failed to hide: {}", e),
                }
            } else {
                println!("  → Showing floating window");
                match window.show() {
                    Ok(_) => {
                        println!("     ✅ Window shown successfully");
                        match window.set_focus() {
                            Ok(_) => println!("     ✅ Window focused successfully"),
                            Err(e) => println!("     ⚠️  Failed to focus: {}", e),
                        }
                        if let Some(monitor) = get_active_monitor(window) {
                            let pos = monitor.position();
                            let size = monitor.size();
                            println!(
                                "     Monitor position: ({}, {}), size: {}x{}",
                                pos.x, pos.y, size.width, size.height
                            );

                            let window_size = window.inner_size().unwrap_or(tauri::PhysicalSize {
                                width: 500,
                                height: 60,
                            });
                            let x =
                                pos.x + (size.width as i32 - window_size.width as i32) / 2;
                            let y = pos.y + size.height as i32 / 3;

                            println!(
                                "     Window size: {}x{}",
                                window_size.width, window_size.height
                            );
                            println!("     Positioning at: ({}, {})", x, y);
                            if let Err(e) = window.set_position(tauri::Position::Physical(
                                tauri::PhysicalPosition { x, y },
                            )) {
                                println!("     ⚠️  Failed to position: {}", e);
                            }
                        }
                    }
                    Err(e) => println!("     ❌ Failed to show: {}", e),
                }
            }
        }
        Err(e) => {
            println!("     ❌ Failed to check visibility: {}", e);
        }
    }
}

#[tauri::command]
pub async fn test_floating_window(app: tauri::AppHandle) -> Result<String, String> {
    info!("Testing floating window visibility");
    println!("🧪 Testing floating window...");

    if let Some(window) = app.get_webview_window("floating") {
        let is_visible = window.is_visible().unwrap_or(false);
        println!(
            "   Current state: {}",
            if is_visible { "VISIBLE" } else { "HIDDEN" }
        );

        if is_visible {
            println!("   Action: Hiding window");
            window
                .hide()
                .map_err(|e| format!("Failed to hide: {}", e))?;
            println!("   ✅ Window hidden");
            Ok("Window was visible, now hidden".to_string())
        } else {
            println!("   Action: Showing window");
            window.show().map_err(|e| {
                println!("   ❌ Failed to show: {}", e);
                format!("Failed to show: {}", e)
            })?;
            println!("   ✅ Window shown");

            println!("   Action: Setting focus");
            window.set_focus().map_err(|e| {
                println!("   ⚠️  Failed to focus: {}", e);
                format!("Failed to focus: {}", e)
            })?;
            println!("   ✅ Window focused");

            if let Some(monitor) = get_active_monitor(&window) {
                let pos = monitor.position();
                let size = monitor.size();
                println!(
                    "   Monitor position: ({}, {}), size: {}x{}",
                    pos.x, pos.y, size.width, size.height
                );

                let window_size = window
                    .inner_size()
                    .unwrap_or(tauri::PhysicalSize {
                        width: 500,
                        height: 60,
                    });
                let x = pos.x + (size.width as i32 - window_size.width as i32) / 2;
                let y = pos.y + size.height as i32 / 3;

                println!(
                    "   Window size: {}x{}",
                    window_size.width, window_size.height
                );
                println!("   Positioning at: ({}, {})", x, y);
                window
                    .set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }))
                    .map_err(|e| {
                        println!("   ⚠️  Failed to position: {}", e);
                        format!("Failed to position: {}", e)
                    })?;
                println!("   ✅ Window positioned");
            }

            Ok("Window was hidden, now visible and positioned".to_string())
        }
    } else {
        println!("   ❌ Floating window not found!");
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

#[tauri::command]
pub async fn open_chat_window(app: tauri::AppHandle) -> Result<(), String> {
    info!("Opening chat window");

    if let Some(floating_window) = app.get_webview_window("floating") {
        let _ = floating_window.hide();
    }

    if let Some(window) = app.get_webview_window("main") {
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
    let current_size = window.inner_size().map_err(|e| {
        error!("Failed to get current window size: {}", e);
        e.to_string()
    })?;

    let target_width = width.unwrap_or(current_size.width);
    let target_height = height.unwrap_or(current_size.height);

    info!(
        "Resizing floating window to {}x{}",
        target_width, target_height
    );

    let current_height = current_size.height;

    if (current_height as i32 - target_height as i32).abs() < 20 {
        return window
            .set_size(tauri::Size::Physical(tauri::PhysicalSize {
                width: target_width,
                height: target_height,
            }))
            .map_err(|e| {
                error!("Failed to resize window: {}", e);
                e.to_string()
            });
    }

    let steps = 10;
    let height_diff = target_height as i32 - current_height as i32;
    let step_size = height_diff as f32 / steps as f32;

    for i in 1..=steps {
        let new_height = (current_height as f32 + step_size * i as f32) as u32;

        if let Err(e) = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
            width: target_width,
            height: new_height,
        })) {
            error!("Failed to resize window during animation: {}", e);
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(15)).await;
    }

    window
        .set_size(tauri::Size::Physical(tauri::PhysicalSize {
            width: target_width,
            height: target_height,
        }))
        .map_err(|e| {
            error!("Failed to resize window: {}", e);
            e.to_string()
        })
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
