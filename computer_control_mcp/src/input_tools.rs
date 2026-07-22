use kage_core::mcp_json_rpc::tool_result_text;
use kage_core::os::input;

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
            result_text(id, input::type_text(text_val))
        }
        "key_press" => {
            let keys = args.get("keys").and_then(|v| v.as_str()).unwrap_or("");
            let dangerous = ["alt+f4", "ctrl+w", "ctrl+q"];
            let normalized = keys.trim().to_lowercase().replace(" ", "");
            if dangerous.iter().any(|&d| normalized == d) {
                tool_result_text(id, &format!("⚠️ DANGEROUS: '{}' — call key_press_confirmed(keys='{}', confirm=true) to proceed.", keys, keys), false)
            } else {
                result_text(id, input::key_press(keys))
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
                match input::key_press(keys) {
                    Ok(_) => tool_result_text(id, &format!("Executed: {}", keys), false),
                    Err(e) => tool_result_text(id, &e, true),
                }
            }
        }
        "click" => {
            let x = args.get("x").and_then(|v| v.as_i64()).map(|v| v as i32);
            let y = args.get("y").and_then(|v| v.as_i64()).map(|v| v as i32);
            let button = args
                .get("button")
                .and_then(|v| v.as_str())
                .unwrap_or("left");
            let count = args.get("count").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
            result_text(id, input::click(x, y, button, count))
        }
        "drag" => {
            let from_x = args.get("from_x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let from_y = args.get("from_y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let to_x = args.get("to_x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let to_y = args.get("to_y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let duration = args.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.5);
            result_text(id, input::drag(from_x, from_y, to_x, to_y, duration))
        }
        "scroll" => {
            let direction = args
                .get("direction")
                .and_then(|v| v.as_str())
                .unwrap_or("down");
            let amount = args.get("amount").and_then(|v| v.as_i64()).unwrap_or(3) as i32;
            let x = args.get("x").and_then(|v| v.as_i64()).map(|v| v as i32);
            let y = args.get("y").and_then(|v| v.as_i64()).map(|v| v as i32);
            result_text(id, input::scroll(direction, amount, x, y))
        }
        "move_mouse" => {
            let x = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let y = args.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            result_text(id, input::move_mouse(x, y))
        }
        "wait" => {
            let ms = args
                .get("milliseconds")
                .and_then(|v| v.as_u64())
                .unwrap_or(500);
            std::thread::sleep(std::time::Duration::from_millis(ms));
            tool_result_text(id, &format!("Waited {}ms", ms), false)
        }
        "get_cursor_position" => match input::get_cursor_position() {
            Ok((cx, cy)) => {
                tool_result_text(id, &format!("{{\"x\": {}, \"y\": {}}}", cx, cy), false)
            }
            Err(e) => tool_result_text(id, &e, true),
        },
        "get_screen_size" => match input::get_screen_size() {
            Ok((w, h)) => tool_result_text(
                id,
                &format!("{{\"width\": {}, \"height\": {}}}", w, h),
                false,
            ),
            Err(e) => tool_result_text(id, &e, true),
        },
        _ => return None,
    })
}

/// Relay an input-synthesis outcome as a tool result: success message on
/// Ok, error text (with the is_error flag) on Err.
fn result_text(id: &serde_json::Value, outcome: Result<String, String>) -> String {
    match outcome {
        Ok(msg) => tool_result_text(id, &msg, false),
        Err(e) => tool_result_text(id, &e, true),
    }
}
