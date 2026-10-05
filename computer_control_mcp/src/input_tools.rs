use kage_core::mcp_json_rpc::tool_result_text;
use kage_core::os::input;

/// Upper bound for model-supplied sleeps (`wait`, `launch_and_get_tree`'s
/// `wait_ms`). The stdio loop is single-threaded, so an unbounded sleep
/// freezes every later request — ping included — until it ends.
pub(crate) const MAX_WAIT_MS: u64 = 60_000;

pub(crate) fn clamp_wait_ms(ms: u64) -> u64 {
    ms.min(MAX_WAIT_MS)
}

/// Narrow a model-supplied click count to `u32` without wrapping (a bare
/// `as u32` turns 2^32 + 1 into 1) and cap it at `MAX_CLICK_COUNT`. 0
/// passes through so the platform layer can reject it.
pub(crate) fn clamp_click_count(count: u64) -> u32 {
    u32::try_from(count)
        .unwrap_or(u32::MAX)
        .min(input::MAX_CLICK_COUNT)
}

pub(crate) fn dispatch(
    id: &serde_json::Value,
    tool_name: &str,
    args: &serde_json::Value,
) -> Option<String> {
    Some(match tool_name {
        "type_text" => {
            let text_val = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            // Count only: typed text can be a password, and the log is plaintext.
            log::info!("[type_text] Typing {} chars", text_val.chars().count());
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
            let count = clamp_click_count(args.get("count").and_then(|v| v.as_u64()).unwrap_or(1));
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
            let requested = args
                .get("milliseconds")
                .and_then(|v| v.as_u64())
                .unwrap_or(500);
            let ms = clamp_wait_ms(requested);
            std::thread::sleep(std::time::Duration::from_millis(ms));
            let suffix = if ms < requested { " (capped)" } else { "" };
            tool_result_text(id, &format!("Waited {}ms{}", ms, suffix), false)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_wait_ms_passes_small_values_through() {
        assert_eq!(clamp_wait_ms(0), 0);
        assert_eq!(clamp_wait_ms(500), 500);
        assert_eq!(clamp_wait_ms(MAX_WAIT_MS), MAX_WAIT_MS);
    }

    #[test]
    fn clamp_wait_ms_caps_huge_values() {
        assert_eq!(clamp_wait_ms(MAX_WAIT_MS + 1), MAX_WAIT_MS);
        assert_eq!(clamp_wait_ms(u64::MAX), MAX_WAIT_MS);
    }

    #[test]
    fn clamp_click_count_passes_valid_counts_through() {
        assert_eq!(clamp_click_count(0), 0);
        assert_eq!(clamp_click_count(1), 1);
        assert_eq!(clamp_click_count(2), 2);
        assert_eq!(
            clamp_click_count(u64::from(input::MAX_CLICK_COUNT)),
            input::MAX_CLICK_COUNT
        );
    }

    #[test]
    fn clamp_click_count_caps_without_wrapping() {
        assert_eq!(clamp_click_count(11), input::MAX_CLICK_COUNT);
        // 2^32 + 1 would wrap to 1 under a bare `as u32`.
        assert_eq!(clamp_click_count((1u64 << 32) + 1), input::MAX_CLICK_COUNT);
        assert_eq!(clamp_click_count(u64::MAX), input::MAX_CLICK_COUNT);
    }
}
