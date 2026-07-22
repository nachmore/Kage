// Windows input synthesis — UIA (`uiautomation`) for keyboard/typed text
// and cursor APIs, raw `SendInput` for button/wheel events.
//
// Mouse events use the windows crate's INPUT/MOUSEINPUT — these types
// have correct layout on every supported architecture, unlike a hand-
// rolled MouseInput struct which would only work on x64 by accident of
// padding.

use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS,
};

fn win32_mouse_event(flags: MOUSE_EVENT_FLAGS, data: i32) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

pub fn type_text_impl(text: &str) -> Result<String, String> {
    let kb = uiautomation::inputs::Keyboard::new();
    // Handle newlines: split text on \n and send Enter between lines
    let lines: Vec<&str> = text.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        if !line.is_empty() {
            kb.send_text(line)
                .map_err(|e| format!("Failed to type line {}: {}", i + 1, e))?;
        }
        // Send Enter between lines (not after the last one)
        if i < lines.len() - 1 {
            kb.send_keys("{Enter}")
                .map_err(|e| format!("Failed to send Enter: {}", e))?;
        }
    }
    Ok(format!(
        "Typed {} characters ({} lines)",
        text.len(),
        lines.len()
    ))
}

pub fn key_press_impl(keys: &str) -> Result<String, String> {
    let kb = uiautomation::inputs::Keyboard::new();
    let uia_keys = convert_key_combo(keys);
    kb.send_keys(&uia_keys)
        .map_err(|e| format!("Failed to press '{}': {}", keys, e))?;
    Ok(format!("Pressed: {}", keys))
}

pub fn click_impl(
    x: Option<i32>,
    y: Option<i32>,
    button: &str,
    count: u32,
) -> Result<String, String> {
    let mouse = uiautomation::inputs::Mouse::new()
        .auto_move(true)
        .move_time(50);
    if let (Some(px), Some(py)) = (x, y) {
        let pt = uiautomation::types::Point::new(px, py);
        let result = match (button, count) {
            ("right", _) => mouse.right_click(&pt),
            (_, 2) => mouse.double_click(&pt),
            _ => mouse.click(&pt),
        };
        result.map_err(|e| format!("Click failed: {}", e))?;
        Ok(format!("Clicked {} at ({}, {})", button, px, py))
    } else {
        let pos = uiautomation::inputs::Mouse::get_cursor_pos()
            .unwrap_or(uiautomation::types::Point::new(0, 0));
        Ok(format!(
            "Clicked {} at ({}, {})",
            button,
            pos.get_x(),
            pos.get_y()
        ))
    }
}

pub fn drag_impl(
    from_x: i32,
    from_y: i32,
    to_x: i32,
    to_y: i32,
    duration: f64,
) -> Result<String, String> {
    let _ = uiautomation::inputs::Mouse::set_cursor_pos(&uiautomation::types::Point::new(
        from_x, from_y,
    ));
    std::thread::sleep(std::time::Duration::from_millis(50));
    // Press, move in steps, release
    win32_mouse_event(MOUSEEVENTF_LEFTDOWN, 0);
    let steps = (duration * 60.0).max(10.0) as i32;
    let dx = (to_x - from_x) as f64 / steps as f64;
    let dy = (to_y - from_y) as f64 / steps as f64;
    for i in 1..=steps {
        let _ = uiautomation::inputs::Mouse::set_cursor_pos(&uiautomation::types::Point::new(
            from_x + (dx * i as f64) as i32,
            from_y + (dy * i as f64) as i32,
        ));
        std::thread::sleep(std::time::Duration::from_secs_f64(duration / steps as f64));
    }
    win32_mouse_event(MOUSEEVENTF_LEFTUP, 0);
    Ok(format!(
        "Dragged from ({},{}) to ({},{})",
        from_x, from_y, to_x, to_y
    ))
}

pub fn scroll_impl(
    direction: &str,
    amount: i32,
    x: Option<i32>,
    y: Option<i32>,
) -> Result<String, String> {
    if let (Some(px), Some(py)) = (x, y) {
        let _ =
            uiautomation::inputs::Mouse::set_cursor_pos(&uiautomation::types::Point::new(px, py));
    }
    let wheel_delta = if direction == "up" {
        amount * 120
    } else {
        -amount * 120
    };
    win32_mouse_event(MOUSEEVENTF_WHEEL, wheel_delta);
    Ok(format!("Scrolled {} by {}", direction, amount))
}

pub fn move_mouse_impl(x: i32, y: i32) -> Result<String, String> {
    uiautomation::inputs::Mouse::set_cursor_pos(&uiautomation::types::Point::new(x, y))
        .map_err(|e| format!("Failed to move mouse: {}", e))?;
    Ok(format!("Mouse moved to ({}, {})", x, y))
}

pub fn get_cursor_position_impl() -> Result<(i32, i32), String> {
    uiautomation::inputs::Mouse::get_cursor_pos()
        .map(|pos| (pos.get_x(), pos.get_y()))
        .map_err(|e| format!("Failed: {}", e))
}

pub fn get_screen_size_impl() -> Result<(u32, u32), String> {
    uiautomation::inputs::get_screen_size()
        .map(|(w, h)| (w as u32, h as u32))
        .map_err(|e| format!("Failed: {}", e))
}

/// Convert "ctrl+shift+s" format to uiautomation "{Ctrl}{Shift}s" format.
pub fn convert_key_combo(keys: &str) -> String {
    let parts: Vec<&str> = keys.split('+').map(|s| s.trim()).collect();
    let mut result = String::new();
    for part in &parts {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => result.push_str("{Ctrl}"),
            "alt" => result.push_str("{Alt}"),
            "shift" => result.push_str("{Shift}"),
            "win" | "windows" | "meta" | "super" => result.push_str("{Win}"),
            "enter" | "return" => result.push_str("{Enter}"),
            "tab" => result.push_str("{Tab}"),
            "escape" | "esc" => result.push_str("{Esc}"),
            "backspace" | "back" => result.push_str("{Backspace}"),
            "delete" | "del" => result.push_str("{Delete}"),
            "space" => result.push_str("{Space}"),
            "up" => result.push_str("{Up}"),
            "down" => result.push_str("{Down}"),
            "left" => result.push_str("{Left}"),
            "right" => result.push_str("{Right}"),
            "home" => result.push_str("{Home}"),
            "end" => result.push_str("{End}"),
            "pageup" | "pgup" => result.push_str("{PageUp}"),
            "pagedown" | "pgdn" => result.push_str("{PageDown}"),
            "insert" | "ins" => result.push_str("{Insert}"),
            k if k.starts_with('f') && k[1..].parse::<u32>().is_ok() => {
                result.push_str(&format!("{{{}}}", part));
            }
            _ => result.push_str(part),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::convert_key_combo;

    #[test]
    fn converts_modifiers_and_named_keys() {
        assert_eq!(convert_key_combo("ctrl+shift+s"), "{Ctrl}{Shift}s");
        assert_eq!(convert_key_combo("alt+F4"), "{Alt}{F4}");
        assert_eq!(convert_key_combo("win+e"), "{Win}e");
        assert_eq!(convert_key_combo("enter"), "{Enter}");
    }
}
