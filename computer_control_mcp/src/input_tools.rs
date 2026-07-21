#[cfg(target_os = "windows")]
use super::handlers::convert_key_combo;
#[cfg(target_os = "macos")]
use super::macos_input;
use super::tool_result_text;
#[cfg(target_os = "windows")]
use super::win32_mouse_event;
#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_WHEEL,
};

pub(crate) fn dispatch(
    id: &serde_json::Value,
    tool_name: &str,
    args: &serde_json::Value,
) -> Option<String> {
    Some(match tool_name {
        "type_text" => {
            let text_val = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            log::info!(
                "[type_text] Typing {} chars: {:?}",
                text_val.len(),
                text_val
            );
            #[cfg(target_os = "windows")]
            {
                let kb = uiautomation::inputs::Keyboard::new();
                // Handle newlines: split text on \n and send Enter between lines
                let lines: Vec<&str> = text_val.split('\n').collect();
                for (i, line) in lines.iter().enumerate() {
                    if !line.is_empty() {
                        if let Err(e) = kb.send_text(line) {
                            return Some(tool_result_text(
                                id,
                                &format!("Failed to type line {}: {}", i + 1, e),
                                true,
                            ));
                        }
                    }
                    // Send Enter between lines (not after the last one)
                    if i < lines.len() - 1 {
                        if let Err(e) = kb.send_keys("{Enter}") {
                            return Some(tool_result_text(
                                id,
                                &format!("Failed to send Enter: {}", e),
                                true,
                            ));
                        }
                    }
                }
                tool_result_text(
                    id,
                    &format!(
                        "Typed {} characters ({} lines)",
                        text_val.len(),
                        lines.len()
                    ),
                    false,
                )
            }
            #[cfg(target_os = "macos")]
            {
                match macos_input::type_text(text_val) {
                    Ok(msg) => tool_result_text(id, &msg, false),
                    Err(e) => tool_result_text(id, &e, true),
                }
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                tool_result_text(id, "type_text not available on this platform", true)
            }
        }
        "key_press" => {
            let keys = args.get("keys").and_then(|v| v.as_str()).unwrap_or("");
            let dangerous = ["alt+f4", "ctrl+w", "ctrl+q", "alt+f4"];
            let normalized = keys.trim().to_lowercase().replace(" ", "");
            if dangerous.iter().any(|&d| normalized == d) {
                tool_result_text(id, &format!("⚠️ DANGEROUS: '{}' — call key_press_confirmed(keys='{}', confirm=true) to proceed.", keys, keys), false)
            } else {
                #[cfg(target_os = "windows")]
                {
                    let kb = uiautomation::inputs::Keyboard::new();
                    // Convert "ctrl+s" format to uiautomation "{Ctrl}s" format
                    let uia_keys = convert_key_combo(keys);
                    match kb.send_keys(&uia_keys) {
                        Ok(_) => tool_result_text(id, &format!("Pressed: {}", keys), false),
                        Err(e) => tool_result_text(
                            id,
                            &format!("Failed to press '{}': {}", keys, e),
                            true,
                        ),
                    }
                }
                #[cfg(target_os = "macos")]
                {
                    match macos_input::key_press(keys) {
                        Ok(msg) => tool_result_text(id, &msg, false),
                        Err(e) => tool_result_text(id, &e, true),
                    }
                }
                #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                {
                    tool_result_text(id, "key_press not available on this platform", true)
                }
            }
        }
        "key_press_confirmed" => {
            let keys = args.get("keys").and_then(|v| v.as_str()).unwrap_or("");
            let confirm = args
                .get("confirm")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !confirm {
                tool_result_text(id, "Cancelled — confirm must be true.", false)
            } else {
                #[cfg(target_os = "windows")]
                {
                    let kb = uiautomation::inputs::Keyboard::new();
                    let uia_keys = convert_key_combo(keys);
                    match kb.send_keys(&uia_keys) {
                        Ok(_) => tool_result_text(id, &format!("Executed: {}", keys), false),
                        Err(e) => tool_result_text(id, &format!("Failed: {}", e), true),
                    }
                }
                #[cfg(target_os = "macos")]
                {
                    match macos_input::key_press(keys) {
                        Ok(_) => tool_result_text(id, &format!("Executed: {}", keys), false),
                        Err(e) => tool_result_text(id, &e, true),
                    }
                }
                #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                {
                    tool_result_text(id, "key_press not available on this platform", true)
                }
            }
        }
        "click" => {
            let x = args.get("x").and_then(|v| v.as_i64());
            let y = args.get("y").and_then(|v| v.as_i64());
            let button = args
                .get("button")
                .and_then(|v| v.as_str())
                .unwrap_or("left");
            let count = args.get("count").and_then(|v| v.as_u64()).unwrap_or(1);
            #[cfg(target_os = "windows")]
            {
                let mouse = uiautomation::inputs::Mouse::new()
                    .auto_move(true)
                    .move_time(50);
                if let (Some(px), Some(py)) = (x, y) {
                    let pt = uiautomation::types::Point::new(px as i32, py as i32);
                    let result = match (button, count) {
                        ("right", _) => mouse.right_click(&pt),
                        (_, 2) => mouse.double_click(&pt),
                        _ => mouse.click(&pt),
                    };
                    match result {
                        Ok(_) => tool_result_text(
                            id,
                            &format!("Clicked {} at ({}, {})", button, px, py),
                            false,
                        ),
                        Err(e) => tool_result_text(id, &format!("Click failed: {}", e), true),
                    }
                } else {
                    let pos = uiautomation::inputs::Mouse::get_cursor_pos()
                        .unwrap_or(uiautomation::types::Point::new(0, 0));
                    tool_result_text(
                        id,
                        &format!("Clicked {} at ({}, {})", button, pos.get_x(), pos.get_y()),
                        false,
                    )
                }
            }
            #[cfg(target_os = "macos")]
            {
                match macos_input::click(
                    x.map(|v| v as i32),
                    y.map(|v| v as i32),
                    button,
                    count as u32,
                ) {
                    Ok(msg) => tool_result_text(id, &msg, false),
                    Err(e) => tool_result_text(id, &e, true),
                }
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                tool_result_text(id, "click not available on this platform", true)
            }
        }
        "drag" => {
            let from_x = args.get("from_x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let from_y = args.get("from_y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let to_x = args.get("to_x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let to_y = args.get("to_y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let duration = args.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.5);
            #[cfg(target_os = "windows")]
            {
                let _ = uiautomation::inputs::Mouse::set_cursor_pos(
                    &uiautomation::types::Point::new(from_x, from_y),
                );
                std::thread::sleep(std::time::Duration::from_millis(50));
                // Press, move in steps, release
                win32_mouse_event(MOUSEEVENTF_LEFTDOWN, 0);
                let steps = (duration * 60.0).max(10.0) as i32;
                let dx = (to_x - from_x) as f64 / steps as f64;
                let dy = (to_y - from_y) as f64 / steps as f64;
                for i in 1..=steps {
                    let _ = uiautomation::inputs::Mouse::set_cursor_pos(
                        &uiautomation::types::Point::new(
                            from_x + (dx * i as f64) as i32,
                            from_y + (dy * i as f64) as i32,
                        ),
                    );
                    std::thread::sleep(std::time::Duration::from_secs_f64(duration / steps as f64));
                }
                win32_mouse_event(MOUSEEVENTF_LEFTUP, 0);
                tool_result_text(
                    id,
                    &format!(
                        "Dragged from ({},{}) to ({},{})",
                        from_x, from_y, to_x, to_y
                    ),
                    false,
                )
            }
            #[cfg(target_os = "macos")]
            {
                match macos_input::drag(from_x, from_y, to_x, to_y, duration) {
                    Ok(msg) => tool_result_text(id, &msg, false),
                    Err(e) => tool_result_text(id, &e, true),
                }
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                tool_result_text(id, "drag not available on this platform", true)
            }
        }
        "scroll" => {
            let direction = args
                .get("direction")
                .and_then(|v| v.as_str())
                .unwrap_or("down");
            let amount = args.get("amount").and_then(|v| v.as_i64()).unwrap_or(3) as i32;
            let x = args.get("x").and_then(|v| v.as_i64());
            let y = args.get("y").and_then(|v| v.as_i64());
            #[cfg(target_os = "windows")]
            {
                if let (Some(px), Some(py)) = (x, y) {
                    let _ = uiautomation::inputs::Mouse::set_cursor_pos(
                        &uiautomation::types::Point::new(px as i32, py as i32),
                    );
                }
                let wheel_delta = if direction == "up" {
                    amount * 120
                } else {
                    -amount * 120
                };
                win32_mouse_event(MOUSEEVENTF_WHEEL, wheel_delta);
                tool_result_text(id, &format!("Scrolled {} by {}", direction, amount), false)
            }
            #[cfg(target_os = "macos")]
            {
                match macos_input::scroll(
                    direction,
                    amount,
                    x.map(|v| v as i32),
                    y.map(|v| v as i32),
                ) {
                    Ok(msg) => tool_result_text(id, &msg, false),
                    Err(e) => tool_result_text(id, &e, true),
                }
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                tool_result_text(id, "scroll not available on this platform", true)
            }
        }
        "move_mouse" => {
            let x = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let y = args.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            #[cfg(target_os = "windows")]
            {
                match uiautomation::inputs::Mouse::set_cursor_pos(&uiautomation::types::Point::new(
                    x, y,
                )) {
                    Ok(_) => tool_result_text(id, &format!("Mouse moved to ({}, {})", x, y), false),
                    Err(e) => tool_result_text(id, &format!("Failed to move mouse: {}", e), true),
                }
            }
            #[cfg(target_os = "macos")]
            {
                match macos_input::move_mouse(x, y) {
                    Ok(msg) => tool_result_text(id, &msg, false),
                    Err(e) => tool_result_text(id, &e, true),
                }
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                tool_result_text(id, "move_mouse not available on this platform", true)
            }
        }
        "wait" => {
            let ms = args
                .get("milliseconds")
                .and_then(|v| v.as_u64())
                .unwrap_or(500);
            std::thread::sleep(std::time::Duration::from_millis(ms));
            tool_result_text(id, &format!("Waited {}ms", ms), false)
        }
        "get_cursor_position" => {
            #[cfg(target_os = "windows")]
            {
                match uiautomation::inputs::Mouse::get_cursor_pos() {
                    Ok(pos) => tool_result_text(
                        id,
                        &format!("{{\"x\": {}, \"y\": {}}}", pos.get_x(), pos.get_y()),
                        false,
                    ),
                    Err(e) => tool_result_text(id, &format!("Failed: {}", e), true),
                }
            }
            #[cfg(target_os = "macos")]
            {
                match macos_input::get_cursor_position() {
                    Ok((cx, cy)) => {
                        tool_result_text(id, &format!("{{\"x\": {}, \"y\": {}}}", cx, cy), false)
                    }
                    Err(e) => tool_result_text(id, &e, true),
                }
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                tool_result_text(id, "Not available on this platform", true)
            }
        }
        "get_screen_size" => {
            #[cfg(target_os = "windows")]
            {
                match uiautomation::inputs::get_screen_size() {
                    Ok((w, h)) => tool_result_text(
                        id,
                        &format!("{{\"width\": {}, \"height\": {}}}", w, h),
                        false,
                    ),
                    Err(e) => tool_result_text(id, &format!("Failed: {}", e), true),
                }
            }
            #[cfg(target_os = "macos")]
            {
                match macos_input::get_screen_size() {
                    Ok((w, h)) => tool_result_text(
                        id,
                        &format!("{{\"width\": {}, \"height\": {}}}", w, h),
                        false,
                    ),
                    Err(e) => tool_result_text(id, &e, true),
                }
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                tool_result_text(id, "Not available on this platform", true)
            }
        }
        // Folder tools
        _ => return None,
    })
}
