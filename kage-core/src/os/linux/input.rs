// Linux input synthesis — not implemented. Every entry point returns a
// uniform "unsupported" error so the MCP tool layer can relay it to the
// model. (An AT-SPI/xdotool-based implementation is possible if Linux
// computer-control ever becomes a priority.)

use crate::os::input::UNSUPPORTED;

fn unsupported<T>() -> Result<T, String> {
    Err(UNSUPPORTED.to_string())
}

pub fn type_text_impl(_text: &str) -> Result<String, String> {
    unsupported()
}

pub fn key_press_impl(_keys: &str) -> Result<String, String> {
    unsupported()
}

pub fn click_impl(
    _x: Option<i32>,
    _y: Option<i32>,
    _button: &str,
    _count: u32,
) -> Result<String, String> {
    unsupported()
}

pub fn drag_impl(
    _from_x: i32,
    _from_y: i32,
    _to_x: i32,
    _to_y: i32,
    _duration: f64,
) -> Result<String, String> {
    unsupported()
}

pub fn scroll_impl(
    _direction: &str,
    _amount: i32,
    _x: Option<i32>,
    _y: Option<i32>,
) -> Result<String, String> {
    unsupported()
}

pub fn move_mouse_impl(_x: i32, _y: i32) -> Result<String, String> {
    unsupported()
}

pub fn get_cursor_position_impl() -> Result<(i32, i32), String> {
    unsupported()
}

pub fn get_screen_size_impl() -> Result<(u32, u32), String> {
    unsupported()
}
