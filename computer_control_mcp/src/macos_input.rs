use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

/// Create a CGEventSource for synthetic input.
fn source() -> Result<CGEventSource, String> {
    CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|()| "Failed to create CGEventSource".to_string())
}

/// Get the current cursor position.
pub fn get_cursor_position() -> Result<(i32, i32), String> {
    let src = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| "Failed to create CGEventSource".to_string())?;
    let event =
        CGEvent::new(src).map_err(|()| "Failed to create CGEvent for position".to_string())?;
    let point = event.location();
    Ok((point.x as i32, point.y as i32))
}

/// Get the main display size.
pub fn get_screen_size() -> Result<(u32, u32), String> {
    let display = CGDisplay::main();
    let w = display.pixels_wide();
    let h = display.pixels_high();
    if w == 0 || h == 0 {
        return Err("Failed to get screen size".to_string());
    }
    Ok((w as u32, h as u32))
}

/// Move the mouse cursor to (x, y).
pub fn move_mouse(x: i32, y: i32) -> Result<String, String> {
    let src = source()?;
    let point = CGPoint::new(x as f64, y as f64);
    let event = CGEvent::new_mouse_event(
        src,
        CGEventType::MouseMoved,
        point,
        CGMouseButton::Left, // ignored for move events
    )
    .map_err(|()| "Failed to create mouse move event".to_string())?;
    event.post(CGEventTapLocation::HID);
    Ok(format!("Mouse moved to ({}, {})", x, y))
}

/// Click at optional (x, y) with the given button and count.
pub fn click(x: Option<i32>, y: Option<i32>, button: &str, count: u32) -> Result<String, String> {
    let src = source()?;

    // Determine click position — use provided coords or current cursor
    let point = if let (Some(px), Some(py)) = (x, y) {
        CGPoint::new(px as f64, py as f64)
    } else {
        let (cx, cy) = get_cursor_position()?;
        CGPoint::new(cx as f64, cy as f64)
    };

    let (down_type, up_type, cg_button) = match button {
        "right" => (
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGMouseButton::Right,
        ),
        "middle" => (
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGMouseButton::Center,
        ),
        _ => (
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGMouseButton::Left,
        ),
    };

    // For double/triple-click, set the click count field
    let click_count = count.max(1) as i64;

    let down = CGEvent::new_mouse_event(src.clone(), down_type, point, cg_button)
        .map_err(|()| "Failed to create mouse down event".to_string())?;
    down.set_integer_value_field(
        core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE,
        click_count,
    );

    let up = CGEvent::new_mouse_event(src, up_type, point, cg_button)
        .map_err(|()| "Failed to create mouse up event".to_string())?;
    up.set_integer_value_field(
        core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE,
        click_count,
    );

    // For multi-click (double, triple, etc.), send preceding clicks with
    // incrementing click state so the OS recognizes the sequence.
    if click_count > 1 {
        for n in 1..click_count {
            let src_n = source()?;
            let dn = CGEvent::new_mouse_event(src_n.clone(), down_type, point, cg_button)
                .map_err(|()| "Failed to create mouse event".to_string())?;
            dn.set_integer_value_field(
                core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE,
                n,
            );
            let un = CGEvent::new_mouse_event(src_n, up_type, point, cg_button)
                .map_err(|()| "Failed to create mouse event".to_string())?;
            un.set_integer_value_field(
                core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE,
                n,
            );
            dn.post(CGEventTapLocation::HID);
            un.post(CGEventTapLocation::HID);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }

    down.post(CGEventTapLocation::HID);
    up.post(CGEventTapLocation::HID);

    let px = point.x as i32;
    let py = point.y as i32;
    Ok(format!("Clicked {} at ({}, {})", button, px, py))
}

/// Drag from (from_x, from_y) to (to_x, to_y) over the given duration.
pub fn drag(
    from_x: i32,
    from_y: i32,
    to_x: i32,
    to_y: i32,
    duration: f64,
) -> Result<String, String> {
    let src = source()?;
    let from = CGPoint::new(from_x as f64, from_y as f64);

    // Mouse down at start position
    let down = CGEvent::new_mouse_event(
        src.clone(),
        CGEventType::LeftMouseDown,
        from,
        CGMouseButton::Left,
    )
    .map_err(|()| "Failed to create mouse down event".to_string())?;
    down.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Move in steps
    let steps = (duration * 60.0).max(10.0) as i32;
    let dx = (to_x - from_x) as f64 / steps as f64;
    let dy = (to_y - from_y) as f64 / steps as f64;
    let step_duration = std::time::Duration::from_secs_f64(duration / steps as f64);

    for i in 1..=steps {
        let pt = CGPoint::new(from_x as f64 + dx * i as f64, from_y as f64 + dy * i as f64);
        let drag_src = source()?;
        let drag_event = CGEvent::new_mouse_event(
            drag_src,
            CGEventType::LeftMouseDragged,
            pt,
            CGMouseButton::Left,
        )
        .map_err(|()| "Failed to create drag event".to_string())?;
        drag_event.post(CGEventTapLocation::HID);
        std::thread::sleep(step_duration);
    }

    // Mouse up at end position
    let to = CGPoint::new(to_x as f64, to_y as f64);
    let up_src = source()?;
    let up = CGEvent::new_mouse_event(up_src, CGEventType::LeftMouseUp, to, CGMouseButton::Left)
        .map_err(|()| "Failed to create mouse up event".to_string())?;
    up.post(CGEventTapLocation::HID);

    Ok(format!(
        "Dragged from ({},{}) to ({},{})",
        from_x, from_y, to_x, to_y
    ))
}

/// Scroll in the given direction by the given amount.
/// Optionally moves the cursor to (x, y) first.
pub fn scroll(
    direction: &str,
    amount: i32,
    x: Option<i32>,
    y: Option<i32>,
) -> Result<String, String> {
    // Move cursor to position if specified
    if let (Some(px), Some(py)) = (x, y) {
        move_mouse(px, py)?;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let src = source()?;

    // wheel1 = vertical (positive = up), wheel2 = horizontal (positive = left)
    let (wheel1, wheel2) = match direction {
        "up" => (amount, 0),
        "down" => (-amount, 0),
        "left" => (0, amount),
        "right" => (0, -amount),
        _ => return Err(format!("Unknown scroll direction: '{}'", direction)),
    };

    let event = CGEvent::new_scroll_event(src, ScrollEventUnit::LINE, 2, wheel1, wheel2, 0)
        .map_err(|()| "Failed to create scroll event".to_string())?;
    event.post(CGEventTapLocation::HID);

    Ok(format!("Scrolled {} by {}", direction, amount))
}

/// Type text by posting keyboard events for each character.
/// Uses CGEventKeyboardSetUnicodeString for proper Unicode support.
pub fn type_text(text: &str) -> Result<String, String> {
    let src = source()?;
    let lines: Vec<&str> = text.split('\n').collect();

    for (i, line) in lines.iter().enumerate() {
        // Type each character in the line
        for ch in line.chars() {
            type_character(&src, ch)?;
            // Small delay between characters for reliability
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Send Enter between lines (not after the last one)
        if i < lines.len() - 1 {
            send_key(&src, KEYCODE_RETURN, CGEventFlags::empty())?;
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    Ok(format!(
        "Typed {} characters ({} lines)",
        text.chars().count(),
        lines.len()
    ))
}

/// Press a key combination like "ctrl+s", "cmd+shift+n", etc.
pub fn key_press(keys: &str) -> Result<String, String> {
    let src = source()?;
    let parts: Vec<&str> = keys.split('+').map(|s| s.trim()).collect();

    let mut flags = CGEventFlags::empty();
    let mut key_part: Option<&str> = None;

    for part in &parts {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
            "alt" | "option" => flags |= CGEventFlags::CGEventFlagAlternate,
            "shift" => flags |= CGEventFlags::CGEventFlagShift,
            "cmd" | "command" | "meta" | "super" | "win" | "windows" => {
                flags |= CGEventFlags::CGEventFlagCommand
            }
            _ => key_part = Some(part),
        }
    }

    let key_name = match key_part {
        Some(k) => k,
        None => return Err("No key specified in combo".to_string()),
    };

    let keycode = match name_to_keycode(key_name) {
        Some(kc) => kc,
        None => return Err(format!("Unknown key: '{}'", key_name)),
    };

    send_key(&src, keycode, flags)?;
    Ok(format!("Pressed: {}", keys))
}

/// Send a single key down + up with the given flags.
fn send_key(src: &CGEventSource, keycode: u16, flags: CGEventFlags) -> Result<(), String> {
    let down = CGEvent::new_keyboard_event(src.clone(), keycode, true)
        .map_err(|()| "Failed to create key down event".to_string())?;
    if !flags.is_empty() {
        down.set_flags(flags);
    }

    let up = CGEvent::new_keyboard_event(src.clone(), keycode, false)
        .map_err(|()| "Failed to create key up event".to_string())?;
    if !flags.is_empty() {
        up.set_flags(flags);
    }

    down.post(CGEventTapLocation::HID);
    up.post(CGEventTapLocation::HID);
    Ok(())
}

/// Type a single Unicode character using CGEventKeyboardSetUnicodeString.
fn type_character(src: &CGEventSource, ch: char) -> Result<(), String> {
    let down = CGEvent::new_keyboard_event(src.clone(), 0, true)
        .map_err(|()| "Failed to create key event".to_string())?;
    let up = CGEvent::new_keyboard_event(src.clone(), 0, false)
        .map_err(|()| "Failed to create key event".to_string())?;

    // Encode the character as UTF-16 for the CGEvent unicode string API
    let mut buf = [0u16; 2];
    let encoded = ch.encode_utf16(&mut buf);
    down.set_string_from_utf16_unchecked(encoded);
    up.set_string_from_utf16_unchecked(encoded);

    down.post(CGEventTapLocation::HID);
    up.post(CGEventTapLocation::HID);
    Ok(())
}

// -----------------------------------------------------------------------
// macOS virtual keycodes (from HIToolbox/Events.h)
// -----------------------------------------------------------------------
const KEYCODE_RETURN: u16 = 0x24;
const KEYCODE_TAB: u16 = 0x30;
const KEYCODE_SPACE: u16 = 0x31;
const KEYCODE_DELETE: u16 = 0x33; // Backspace
const KEYCODE_ESCAPE: u16 = 0x35;
const KEYCODE_FORWARD_DELETE: u16 = 0x75;
const KEYCODE_HOME: u16 = 0x73;
const KEYCODE_END: u16 = 0x77;
const KEYCODE_PAGE_UP: u16 = 0x74;
const KEYCODE_PAGE_DOWN: u16 = 0x79;
const KEYCODE_LEFT_ARROW: u16 = 0x7B;
const KEYCODE_RIGHT_ARROW: u16 = 0x7C;
const KEYCODE_DOWN_ARROW: u16 = 0x7D;
const KEYCODE_UP_ARROW: u16 = 0x7E;
const KEYCODE_F1: u16 = 0x7A;
const KEYCODE_F2: u16 = 0x78;
const KEYCODE_F3: u16 = 0x63;
const KEYCODE_F4: u16 = 0x76;
const KEYCODE_F5: u16 = 0x60;
const KEYCODE_F6: u16 = 0x61;
const KEYCODE_F7: u16 = 0x62;
const KEYCODE_F8: u16 = 0x64;
const KEYCODE_F9: u16 = 0x65;
const KEYCODE_F10: u16 = 0x6D;
const KEYCODE_F11: u16 = 0x67;
const KEYCODE_F12: u16 = 0x6F;

/// Map a key name to a macOS virtual keycode.
fn name_to_keycode(name: &str) -> Option<u16> {
    match name.to_lowercase().as_str() {
        "enter" | "return" => Some(KEYCODE_RETURN),
        "tab" => Some(KEYCODE_TAB),
        "space" => Some(KEYCODE_SPACE),
        "backspace" | "back" | "delete" => Some(KEYCODE_DELETE),
        "del" | "forwarddelete" => Some(KEYCODE_FORWARD_DELETE),
        "escape" | "esc" => Some(KEYCODE_ESCAPE),
        "up" => Some(KEYCODE_UP_ARROW),
        "down" => Some(KEYCODE_DOWN_ARROW),
        "left" => Some(KEYCODE_LEFT_ARROW),
        "right" => Some(KEYCODE_RIGHT_ARROW),
        "home" => Some(KEYCODE_HOME),
        "end" => Some(KEYCODE_END),
        "pageup" | "pgup" => Some(KEYCODE_PAGE_UP),
        "pagedown" | "pgdn" => Some(KEYCODE_PAGE_DOWN),
        "f1" => Some(KEYCODE_F1),
        "f2" => Some(KEYCODE_F2),
        "f3" => Some(KEYCODE_F3),
        "f4" => Some(KEYCODE_F4),
        "f5" => Some(KEYCODE_F5),
        "f6" => Some(KEYCODE_F6),
        "f7" => Some(KEYCODE_F7),
        "f8" => Some(KEYCODE_F8),
        "f9" => Some(KEYCODE_F9),
        "f10" => Some(KEYCODE_F10),
        "f11" => Some(KEYCODE_F11),
        "f12" => Some(KEYCODE_F12),
        // Single character — map to ANSI keycode
        s if s.len() == 1 => char_to_keycode(s.chars().next().unwrap()),
        _ => None,
    }
}

/// Map a single ASCII character to its macOS virtual keycode.
/// Based on the US ANSI keyboard layout.
fn char_to_keycode(ch: char) -> Option<u16> {
    let kc = match ch.to_ascii_lowercase() {
        'a' => 0x00,
        's' => 0x01,
        'd' => 0x02,
        'f' => 0x03,
        'h' => 0x04,
        'g' => 0x05,
        'z' => 0x06,
        'x' => 0x07,
        'c' => 0x08,
        'v' => 0x09,
        'b' => 0x0B,
        'q' => 0x0C,
        'w' => 0x0D,
        'e' => 0x0E,
        'r' => 0x0F,
        'y' => 0x10,
        't' => 0x11,
        '1' => 0x12,
        '2' => 0x13,
        '3' => 0x14,
        '4' => 0x15,
        '6' => 0x16,
        '5' => 0x17,
        '=' => 0x18,
        '9' => 0x19,
        '7' => 0x1A,
        '-' => 0x1B,
        '8' => 0x1C,
        '0' => 0x1D,
        ']' => 0x1E,
        'o' => 0x1F,
        'u' => 0x20,
        '[' => 0x21,
        'i' => 0x22,
        'p' => 0x23,
        'l' => 0x25,
        'j' => 0x26,
        '\'' => 0x27,
        'k' => 0x28,
        ';' => 0x29,
        '\\' => 0x2A,
        ',' => 0x2B,
        '/' => 0x2C,
        'n' => 0x2D,
        'm' => 0x2E,
        '.' => 0x2F,
        '`' => 0x32,
        _ => return None,
    };
    Some(kc)
}
