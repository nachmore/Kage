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
        println!("     Cursor position: ({}, {})", cursor_x, cursor_y);

        if let Some(monitor) = find_monitor_at_position(window, cursor_x, cursor_y) {
            println!("     Found active monitor at cursor position");
            return Some(monitor);
        }
    }

    println!("     Falling back to primary monitor");
    window.primary_monitor().ok().flatten()
}

/// Capture the currently selected text from the active window.
fn capture_selection() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        extern "system" {
            fn GetClipboardSequenceNumber() -> u32;
            fn SendInput(count: u32, inputs: *const WinInput, size: i32) -> u32;
            fn MapVirtualKeyW(code: u32, map_type: u32) -> u32;
        }
        const MAPVK_VK_TO_VSC: u32 = 0;

        #[repr(C)]
        struct KbdInput { vk: u16, scan: u16, flags: u32, time: u32, extra: usize }
        #[repr(C)]
        struct WinInput { input_type: u32, ki: KbdInput, _pad: [u8; 8] }
        const INPUT_KEYBOARD: u32 = 1;
        const KEYEVENTF_KEYUP: u32 = 0x0002;

        let make = |vk: u16, up: bool| -> WinInput {
            let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
            WinInput {
                input_type: INPUT_KEYBOARD,
                ki: KbdInput { vk, scan, flags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, extra: 0 },
                _pad: [0u8; 8],
            }
        };
        let size = std::mem::size_of::<WinInput>() as i32;

        unsafe {
            // Save clipboard state
            let original_clipboard = read_clipboard_raw();
            let seq_before = GetClipboardSequenceNumber();

            // Step 1: Force-release ALL modifiers (even if not pressed — harmless)
            let releases = [
                make(0x10, true), // VK_SHIFT
                make(0x11, true), // VK_CONTROL
                make(0x12, true), // VK_MENU (Alt)
                make(0x5B, true), // VK_LWIN
                make(0x5C, true), // VK_RWIN
            ];
            SendInput(releases.len() as u32, releases.as_ptr(), size);

            // Step 2: Small delay for target app to process releases
            std::thread::sleep(std::time::Duration::from_millis(10));

            // Step 3: Send clean Ctrl+C using SCANCODE flag for fresh synthetic input
            const KEYEVENTF_SCANCODE: u32 = 0x0008;
            let sc_ctrl: u16 = 0x1D; // scan code for Ctrl
            let sc_c: u16 = 0x2E;    // scan code for C
            let scan_input = |scan: u16, up: bool| -> WinInput {
                WinInput {
                    input_type: INPUT_KEYBOARD,
                    ki: KbdInput {
                        vk: 0,
                        scan,
                        flags: KEYEVENTF_SCANCODE | if up { KEYEVENTF_KEYUP } else { 0 },
                        time: 0,
                        extra: 0,
                    },
                    _pad: [0u8; 8],
                }
            };
            let copy_keys = [
                scan_input(sc_ctrl, false), // Ctrl down
                scan_input(sc_c, false),    // C down
                scan_input(sc_c, true),     // C up
                scan_input(sc_ctrl, true),  // Ctrl up
            ];
            let sent = SendInput(4, copy_keys.as_ptr(), size);
            if sent != 4 {
                info!("[selection] SendInput failed: returned {}", sent);
            }

            // Step 4: Poll for clipboard change
            let changed = wait_for_clipboard_change(seq_before, 300);

            if changed {
                // Extra delay for apps like Word that may still be writing clipboard formats
                std::thread::sleep(std::time::Duration::from_millis(50));
                let new_text = read_clipboard_raw();
                // Restore original clipboard
                if let Some(ref orig) = original_clipboard {
                    write_clipboard_raw(orig);
                } else {
                    write_clipboard_raw("");
                }
                if let Some(ref text) = new_text {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() && new_text != original_clipboard {
                        info!("[selection] Captured {} chars", trimmed.len());
                        return Some(trimmed.to_string());
                    }
                }
            }

            return None;
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let original_clipboard = read_clipboard_raw();
        simulate_copy();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let new_clipboard = read_clipboard_raw();
        match (&original_clipboard, &new_clipboard) {
            (Some(orig), Some(new)) if orig != new && !new.is_empty() => {
                write_clipboard_raw(orig);
                Some(new.clone())
            }
            (None, Some(new)) if !new.is_empty() => {
                write_clipboard_raw("");
                Some(new.clone())
            }
            _ => None,
        }
    }
}

/// Poll for clipboard sequence number change with timeout (ms)
#[cfg(target_os = "windows")]
fn wait_for_clipboard_change(seq_before: u32, timeout_ms: u32) -> bool {
    extern "system" {
        fn GetClipboardSequenceNumber() -> u32;
    }
    let steps = (timeout_ms / 10).max(1);
    for _ in 0..steps {
        std::thread::sleep(std::time::Duration::from_millis(10));
        if unsafe { GetClipboardSequenceNumber() } != seq_before {
            return true;
        }
    }
    false
}

#[cfg(target_os = "macos")]
fn simulate_copy() {
    use std::process::Command;
    let _ = Command::new("osascript")
        .args(["-e", "tell application \"System Events\" to keystroke \"c\" using command down"])
        .output();
}

#[cfg(target_os = "linux")]
fn simulate_copy() {
    use std::process::Command;
    let _ = Command::new("xdotool")
        .args(["key", "ctrl+c"])
        .output();
}

fn read_clipboard_raw() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        // Use Win32 clipboard API directly
        use std::ptr;
        extern "system" {
            fn OpenClipboard(hwnd: *mut std::ffi::c_void) -> i32;
            fn CloseClipboard() -> i32;
            fn GetClipboardData(format: u32) -> *mut std::ffi::c_void;
            fn GlobalLock(hmem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
            fn GlobalUnlock(hmem: *mut std::ffi::c_void) -> i32;
        }
        const CF_UNICODETEXT: u32 = 13;
        unsafe {
            if OpenClipboard(ptr::null_mut()) == 0 { return None; }
            let handle = GetClipboardData(CF_UNICODETEXT);
            if handle.is_null() { CloseClipboard(); return None; }
            let ptr = GlobalLock(handle) as *const u16;
            if ptr.is_null() { CloseClipboard(); return None; }
            let mut len = 0;
            while *ptr.add(len) != 0 { len += 1; }
            let slice = std::slice::from_raw_parts(ptr, len);
            let text = String::from_utf16_lossy(slice);
            GlobalUnlock(handle);
            CloseClipboard();
            Some(text)
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        Command::new("pbpaste").output().ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    }
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        Command::new("xclip").args(["-selection", "clipboard", "-o"]).output().ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    }
}

fn write_clipboard_raw(text: &str) {
    #[cfg(target_os = "windows")]
    {
        use std::ptr;
        extern "system" {
            fn OpenClipboard(hwnd: *mut std::ffi::c_void) -> i32;
            fn CloseClipboard() -> i32;
            fn EmptyClipboard() -> i32;
            fn SetClipboardData(format: u32, hmem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
            fn GlobalAlloc(flags: u32, bytes: usize) -> *mut std::ffi::c_void;
            fn GlobalLock(hmem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
            fn GlobalUnlock(hmem: *mut std::ffi::c_void) -> i32;
        }
        const CF_UNICODETEXT: u32 = 13;
        const GMEM_MOVEABLE: u32 = 0x0002;
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = wide.len() * 2;
        unsafe {
            if OpenClipboard(ptr::null_mut()) == 0 { return; }
            EmptyClipboard();
            let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if !hmem.is_null() {
                let dest = GlobalLock(hmem) as *mut u16;
                if !dest.is_null() {
                    ptr::copy_nonoverlapping(wide.as_ptr(), dest, wide.len());
                    GlobalUnlock(hmem);
                    SetClipboardData(CF_UNICODETEXT, hmem);
                }
            }
            CloseClipboard();
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        use std::io::Write;
        if let Ok(mut child) = Command::new("pbcopy").stdin(std::process::Stdio::piped()).spawn() {
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        use std::io::Write;
        if let Ok(mut child) = Command::new("xclip").args(["-selection", "clipboard"]).stdin(std::process::Stdio::piped()).spawn() {
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }
}

/// Toggle the floating window visibility and position it
pub fn toggle_floating_window(window: &WebviewWindow) {
    // Read config for positioning preference
    let app = window.app_handle();
    let state: tauri::State<'_, crate::state::AppState> = app.state();

    // Capture selection FIRST, before any config reads or window ops
    // (the target app may lose focus during config lock)
    let is_showing = !window.is_visible().unwrap_or(true);
    let selection = if is_showing { capture_selection() } else { None };

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
                let has_sel = selection.as_ref().map_or(false, |s| !s.is_empty());
                if let Ok(mut sel) = state.last_selection.lock() {
                    *sel = selection;
                }

                // Position before showing to avoid visual jump
                position_floating_window(window, &start_pos, last_x, last_y);
                let _ = window.show();
                let _ = window.set_focus();

                let _ = app.emit("selection_captured", has_sel);
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
        let state: tauri::State<'_, crate::state::AppState> = app.state();
        let config = state.config.lock().await;
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
    let mut config = state.config.lock().await;
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
    let mut config = state.config.lock().await;
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
