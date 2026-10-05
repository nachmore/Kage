// Windows input synthesis — UIA (`uiautomation`) for keyboard/typed text
// and cursor APIs, raw `SendInput` for button/wheel events.
//
// Mouse events use the windows crate's INPUT/MOUSEINPUT — these types
// have correct layout on every supported architecture, unlike a hand-
// rolled MouseInput struct which would only work on x64 by accident of
// padding.

use uiautomation::inputs::MouseButton;
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

/// Map the tool's button name to a UIA button. Unknown names are rejected
/// rather than silently becoming a left click.
fn parse_button(button: &str) -> Option<MouseButton> {
    match button {
        "left" => Some(MouseButton::LEFT),
        "right" => Some(MouseButton::RIGHT),
        "middle" => Some(MouseButton::MIDDLE),
        _ => None,
    }
}

/// Highest click count we synthesise; anything larger is a model mistake.
const MAX_CLICK_COUNT: u32 = 10;

/// How to deliver `count` clicks: whether to start with a double-click, and
/// how many single clicks follow it. Repeats run back-to-back so the OS
/// still counts them as one multi-click (e.g. 3 = triple-click).
fn click_plan(count: u32) -> Result<(bool, u32), String> {
    match count {
        0 => Err("Click count must be at least 1".to_string()),
        1 => Ok((false, 0)),
        n if n <= MAX_CLICK_COUNT => Ok((true, n - 2)),
        n => Err(format!(
            "Click count {} exceeds the maximum of {}",
            n, MAX_CLICK_COUNT
        )),
    }
}

pub fn click_impl(
    x: Option<i32>,
    y: Option<i32>,
    button: &str,
    count: u32,
) -> Result<String, String> {
    let btn = parse_button(button).ok_or_else(|| {
        format!(
            "Unsupported mouse button '{}' (expected left, right or middle)",
            button
        )
    })?;
    let (double_first, extra_singles) = click_plan(count)?;
    let mouse = uiautomation::inputs::Mouse::new()
        .auto_move(true)
        .move_time(50);
    // No coordinates means "click where the cursor is" — that path used to
    // report success without clicking at all.
    let pt = match (x, y) {
        (Some(px), Some(py)) => {
            let pt = uiautomation::types::Point::new(px, py);
            mouse
                .move_to(&pt)
                .map_err(|e| format!("Failed to move mouse: {}", e))?;
            pt
        }
        _ => uiautomation::inputs::Mouse::get_cursor_pos()
            .map_err(|e| format!("Failed to read cursor position: {}", e))?,
    };
    let first = if double_first {
        mouse.double_click_button(btn)
    } else {
        mouse.click_button(btn)
    };
    first.map_err(|e| format!("Click failed: {}", e))?;
    for _ in 0..extra_singles {
        mouse
            .click_button(btn)
            .map_err(|e| format!("Click failed: {}", e))?;
    }
    Ok(format!(
        "Clicked {} x{} at ({}, {})",
        button,
        count,
        pt.get_x(),
        pt.get_y()
    ))
}

/// Upper bound on a drag's duration. `duration` comes straight from the
/// model; a huge value would hold the button down and block the sidecar.
const MAX_DRAG_SECS: f64 = 10.0;

/// Clamp a model-supplied drag duration into a sane range. Negative/NaN
/// values used to panic in `Duration::from_secs_f64` mid-drag.
fn sanitize_drag_duration(duration: f64) -> f64 {
    if duration.is_finite() {
        duration.clamp(0.0, MAX_DRAG_SECS)
    } else {
        0.5
    }
}

/// Sends LEFTUP when dropped, so every exit path from a drag — including a
/// panic unwind — releases the button instead of leaving it held system-wide.
struct LeftButtonRelease;

impl Drop for LeftButtonRelease {
    fn drop(&mut self) {
        win32_mouse_event(MOUSEEVENTF_LEFTUP, 0);
    }
}

pub fn drag_impl(
    from_x: i32,
    from_y: i32,
    to_x: i32,
    to_y: i32,
    duration: f64,
) -> Result<String, String> {
    let duration = sanitize_drag_duration(duration);
    let _ = uiautomation::inputs::Mouse::set_cursor_pos(&uiautomation::types::Point::new(
        from_x, from_y,
    ));
    std::thread::sleep(std::time::Duration::from_millis(50));
    // Press, move in steps, release
    win32_mouse_event(MOUSEEVENTF_LEFTDOWN, 0);
    let release = LeftButtonRelease;
    let steps = (duration * 60.0).max(10.0) as i32;
    let dx = (to_x - from_x) as f64 / steps as f64;
    let dy = (to_y - from_y) as f64 / steps as f64;
    let step_sleep = std::time::Duration::from_secs_f64(duration / steps as f64);
    for i in 1..=steps {
        let _ = uiautomation::inputs::Mouse::set_cursor_pos(&uiautomation::types::Point::new(
            from_x + (dx * i as f64) as i32,
            from_y + (dy * i as f64) as i32,
        ));
        std::thread::sleep(step_sleep);
    }
    drop(release);
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
    use super::{click_plan, convert_key_combo, parse_button, sanitize_drag_duration};
    use uiautomation::inputs::MouseButton;

    #[test]
    fn converts_modifiers_and_named_keys() {
        assert_eq!(convert_key_combo("ctrl+shift+s"), "{Ctrl}{Shift}s");
        assert_eq!(convert_key_combo("alt+F4"), "{Alt}{F4}");
        assert_eq!(convert_key_combo("win+e"), "{Win}e");
        assert_eq!(convert_key_combo("enter"), "{Enter}");
    }

    #[test]
    fn parses_supported_buttons_and_rejects_others() {
        assert_eq!(parse_button("left"), Some(MouseButton::LEFT));
        assert_eq!(parse_button("right"), Some(MouseButton::RIGHT));
        assert_eq!(parse_button("middle"), Some(MouseButton::MIDDLE));
        assert_eq!(parse_button("back"), None);
        assert_eq!(parse_button(""), None);
    }

    #[test]
    fn click_plan_covers_multi_clicks() {
        assert_eq!(click_plan(1), Ok((false, 0)));
        assert_eq!(click_plan(2), Ok((true, 0)));
        assert_eq!(click_plan(3), Ok((true, 1)));
        assert!(click_plan(0).is_err());
        assert!(click_plan(11).is_err());
    }

    #[test]
    fn drag_duration_is_clamped() {
        assert_eq!(sanitize_drag_duration(-1.0), 0.0);
        assert_eq!(sanitize_drag_duration(1e7), 10.0);
        assert_eq!(sanitize_drag_duration(f64::NAN), 0.5);
        assert_eq!(sanitize_drag_duration(f64::INFINITY), 0.5);
        assert_eq!(sanitize_drag_duration(1.5), 1.5);
    }
}
