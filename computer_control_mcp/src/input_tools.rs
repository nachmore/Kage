use kage_core::mcp_json_rpc::tool_result_text;
use kage_core::os::input;
use kage_core::os::kage_target;

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

/// Where an input tool's input would land: the OS by default, a fake in
/// tests.
pub(crate) trait TargetProbe {
    /// Keyboard input sent now would reach a Kage window.
    fn keyboard_goes_to_kage(&self) -> bool;
    /// A click at this screen point would land on a Kage window.
    fn point_is_on_kage(&self, x: i32, y: i32) -> bool;
    /// The mouse cursor's position, when known.
    fn cursor(&self) -> Option<(i32, i32)>;
}

pub(crate) struct OsProbe;

impl TargetProbe for OsProbe {
    fn keyboard_goes_to_kage(&self) -> bool {
        kage_target::keyboard_goes_to_kage()
    }
    fn point_is_on_kage(&self, x: i32, y: i32) -> bool {
        kage_target::point_is_on_kage(x, y)
    }
    fn cursor(&self) -> Option<(i32, i32)> {
        input::get_cursor_position().ok()
    }
}

fn arg_point(args: &serde_json::Value, x: &str, y: &str) -> Option<(i32, i32)> {
    let coord = |k: &str| args.get(k).and_then(|v| v.as_i64()).map(|v| v as i32);
    Some((coord(x)?, coord(y)?))
}

/// Whether `tool` would inject input into one of Kage's own windows,
/// whose approval buttons only the user may press (`os::kage_target`).
/// Keyboard tools go wherever keyboard focus is; mouse tools to their
/// point (the cursor when none is given; for a drag, both ends). Tools
/// that don't inject input are never refused.
pub(crate) fn targets_kage(tool: &str, args: &serde_json::Value, probe: &dyn TargetProbe) -> bool {
    match tool {
        "type_text" | "key_press" | "key_press_confirmed" => probe.keyboard_goes_to_kage(),
        "click" | "scroll" => arg_point(args, "x", "y")
            .or_else(|| probe.cursor())
            .is_some_and(|(x, y)| probe.point_is_on_kage(x, y)),
        "move_mouse" => {
            let (x, y) = arg_point(args, "x", "y").unwrap_or((0, 0));
            probe.point_is_on_kage(x, y)
        }
        "drag" => {
            let from = arg_point(args, "from_x", "from_y").unwrap_or((0, 0));
            let to = arg_point(args, "to_x", "to_y").unwrap_or((0, 0));
            probe.point_is_on_kage(from.0, from.1) || probe.point_is_on_kage(to.0, to.1)
        }
        _ => false,
    }
}

pub(crate) fn dispatch(
    id: &serde_json::Value,
    tool_name: &str,
    args: &serde_json::Value,
) -> Option<String> {
    if targets_kage(tool_name, args, &OsProbe) {
        log::warn!("[{}] refused: it would act on Kage's own window", tool_name);
        return Some(tool_result_text(id, kage_target::REFUSAL, true));
    }
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
    use serde_json::json;

    /// Kage's window covers x < 100; it holds keyboard focus when told.
    struct FakeProbe {
        keyboard: bool,
        cursor: Option<(i32, i32)>,
    }

    impl TargetProbe for FakeProbe {
        fn keyboard_goes_to_kage(&self) -> bool {
            self.keyboard
        }
        fn point_is_on_kage(&self, x: i32, _y: i32) -> bool {
            x < 100
        }
        fn cursor(&self) -> Option<(i32, i32)> {
            self.cursor
        }
    }

    const ELSEWHERE: FakeProbe = FakeProbe {
        keyboard: false,
        cursor: Some((500, 500)),
    };

    #[test]
    fn keyboard_tools_refuse_while_kage_has_focus() {
        let focused = FakeProbe {
            keyboard: true,
            cursor: None,
        };
        for tool in ["type_text", "key_press", "key_press_confirmed"] {
            assert!(
                targets_kage(tool, &json!({"keys": "enter"}), &focused),
                "{tool}"
            );
            assert!(
                !targets_kage(tool, &json!({"keys": "enter"}), &ELSEWHERE),
                "{tool}"
            );
        }
    }

    #[test]
    fn mouse_tools_refuse_points_on_kage() {
        assert!(targets_kage(
            "click",
            &json!({"x": 10, "y": 10}),
            &ELSEWHERE
        ));
        assert!(!targets_kage(
            "click",
            &json!({"x": 300, "y": 10}),
            &ELSEWHERE
        ));
        assert!(targets_kage(
            "scroll",
            &json!({"x": 10, "y": 10}),
            &ELSEWHERE
        ));
        assert!(targets_kage(
            "move_mouse",
            &json!({"x": 10, "y": 10}),
            &ELSEWHERE
        ));
        // A drag that ends on Kage, or starts there.
        let drag = |fx: i32, tx: i32| json!({"from_x": fx, "from_y": 0, "to_x": tx, "to_y": 0});
        assert!(targets_kage("drag", &drag(500, 50), &ELSEWHERE));
        assert!(targets_kage("drag", &drag(50, 500), &ELSEWHERE));
        assert!(!targets_kage("drag", &drag(500, 600), &ELSEWHERE));
    }

    #[test]
    fn a_click_without_coordinates_goes_where_the_cursor_is() {
        let over_kage = FakeProbe {
            keyboard: false,
            cursor: Some((20, 20)),
        };
        assert!(targets_kage("click", &json!({}), &over_kage));
        assert!(!targets_kage("click", &json!({}), &ELSEWHERE));
    }

    #[test]
    fn read_only_tools_are_never_refused() {
        let everywhere = FakeProbe {
            keyboard: true,
            cursor: Some((0, 0)),
        };
        for tool in [
            "get_cursor_position",
            "get_screen_size",
            "wait",
            "get_ui_tree",
        ] {
            assert!(
                !targets_kage(tool, &json!({"x": 0, "y": 0}), &everywhere),
                "{tool}"
            );
        }
    }

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
