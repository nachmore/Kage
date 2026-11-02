//! macOS virtual keycode (`kVK_*` from `<HIToolbox/Events.h>`) → Kage key
//! name mapping, shared by the hotkey-capture tap (`hotkey.rs`) and the
//! system-shortcut reader (`system_shortcuts.rs`).
//!
//! The names produced here are the ones Kage stores in config and hands to
//! `tauri-plugin-global-shortcut` (which parses them with
//! `global_hotkey::hotkey::parse_key`), so every non-`VK_` name below must
//! be accepted by that parser — `keycode_names_parse_as_global_shortcuts`
//! enforces it.

pub const KEYCODE_ESCAPE: u16 = 0x35;

/// Map a macOS virtual keycode to Kage's key name. Unknown codes come back
/// as `VK_XX` so callers can surface (or skip) them instead of dropping
/// them silently.
pub fn keycode_to_name(code: u16) -> String {
    // The layout groups non-contiguously (letters aren't alphabetic in
    // keycode order) so a flat match is the clearest way to express it.
    match code {
        0x00 => "A".into(),
        0x01 => "S".into(),
        0x02 => "D".into(),
        0x03 => "F".into(),
        0x04 => "H".into(),
        0x05 => "G".into(),
        0x06 => "Z".into(),
        0x07 => "X".into(),
        0x08 => "C".into(),
        0x09 => "V".into(),
        0x0B => "B".into(),
        0x0C => "Q".into(),
        0x0D => "W".into(),
        0x0E => "E".into(),
        0x0F => "R".into(),
        0x10 => "Y".into(),
        0x11 => "T".into(),
        0x1F => "O".into(),
        0x20 => "U".into(),
        0x22 => "I".into(),
        0x23 => "P".into(),
        0x25 => "L".into(),
        0x26 => "J".into(),
        0x28 => "K".into(),
        0x2D => "N".into(),
        0x2E => "M".into(),

        // Digits (0x12-0x1D, non-alphabetic order too)
        0x12 => "1".into(),
        0x13 => "2".into(),
        0x14 => "3".into(),
        0x15 => "4".into(),
        0x16 => "6".into(),
        0x17 => "5".into(),
        0x19 => "9".into(),
        0x1A => "7".into(),
        0x1C => "8".into(),
        0x1D => "0".into(),

        // Editing / layout
        0x18 => "=".into(),
        0x1B => "-".into(),
        0x1E => "]".into(),
        0x21 => "[".into(),
        0x24 => "Enter".into(),
        0x27 => "'".into(),
        0x29 => ";".into(),
        0x2A => "\\".into(),
        0x2B => ",".into(),
        0x2C => "/".into(),
        0x2F => ".".into(),
        0x30 => "Tab".into(),
        0x31 => "Space".into(),
        0x32 => "`".into(),
        0x33 => "Backspace".into(),
        0x35 => "Escape".into(),

        // Arrows / navigation
        0x73 => "Home".into(),
        0x74 => "PageUp".into(),
        0x75 => "Delete".into(),
        0x77 => "End".into(),
        0x79 => "PageDown".into(),
        0x7B => "Left".into(),
        0x7C => "Right".into(),
        0x7D => "Down".into(),
        0x7E => "Up".into(),

        // Function keys (non-contiguous — painful but stable)
        0x7A => "F1".into(),
        0x78 => "F2".into(),
        0x63 => "F3".into(),
        0x76 => "F4".into(),
        0x60 => "F5".into(),
        0x61 => "F6".into(),
        0x62 => "F7".into(),
        0x64 => "F8".into(),
        0x65 => "F9".into(),
        0x6D => "F10".into(),
        0x67 => "F11".into(),
        0x6F => "F12".into(),
        0x69 => "F13".into(),
        0x6B => "F14".into(),
        0x71 => "F15".into(),
        0x6A => "F16".into(),
        0x40 => "F17".into(),
        0x4F => "F18".into(),
        0x50 => "F19".into(),
        0x5A => "F20".into(),

        // Numpad
        0x41 => "NumDecimal".into(),
        0x43 => "NumMultiply".into(),
        0x45 => "NumAdd".into(),
        0x4B => "NumDivide".into(),
        0x4C => "NumEnter".into(),
        0x4E => "NumSubtract".into(),
        // `NumEqual`, not `NumEquals` — the latter isn't accepted by
        // global-hotkey's parser, so a captured keypad-= could never register.
        0x51 => "NumEqual".into(),
        0x52 => "Num0".into(),
        0x53 => "Num1".into(),
        0x54 => "Num2".into(),
        0x55 => "Num3".into(),
        0x56 => "Num4".into(),
        0x57 => "Num5".into(),
        0x58 => "Num6".into(),
        0x59 => "Num7".into(),
        0x5B => "Num8".into(),
        0x5C => "Num9".into(),

        // Unknown — surface the raw code so the user sees something meaningful
        // rather than a silent drop.
        other => format!("VK_{:02X}", other),
    }
}

/// Keys whose system-shortcut records carry the Fn (and often numeric-pad)
/// flag implicitly: on Apple keyboards the F-row, arrows and the
/// navigation cluster are reached through Fn, so macOS stores e.g.
/// Mission Control as `Ctrl+Fn+Up`. For these keys the Fn flag doesn't
/// change which hotkey a user means; for every other key Fn is a real
/// modifier that Kage can't register.
pub fn fn_flag_is_implied(code: u16) -> bool {
    matches!(
        code,
        // F1–F20
        0x7A | 0x78 | 0x63 | 0x76 | 0x60 | 0x61 | 0x62 | 0x64 | 0x65 | 0x6D | 0x67 | 0x6F
            | 0x69 | 0x6B | 0x71 | 0x6A | 0x40 | 0x4F | 0x50 | 0x5A
            // Arrows
            | 0x7B | 0x7C | 0x7D | 0x7E
            // Help/Insert, Home, PageUp, Forward Delete, End, PageDown
            | 0x72 | 0x73 | 0x74 | 0x75 | 0x77 | 0x79
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keycode_letters_map_correctly() {
        assert_eq!(keycode_to_name(0x00), "A");
        assert_eq!(keycode_to_name(0x09), "V");
        assert_eq!(keycode_to_name(0x2E), "M");
    }

    #[test]
    fn keycode_digits_map_correctly() {
        assert_eq!(keycode_to_name(0x12), "1");
        assert_eq!(keycode_to_name(0x1D), "0");
    }

    #[test]
    fn keycode_arrows_map_correctly() {
        assert_eq!(keycode_to_name(0x7B), "Left");
        assert_eq!(keycode_to_name(0x7E), "Up");
    }

    #[test]
    fn keycode_function_keys_map_correctly() {
        assert_eq!(keycode_to_name(0x7A), "F1");
        assert_eq!(keycode_to_name(0x6F), "F12");
        assert_eq!(keycode_to_name(0x69), "F13");
        assert_eq!(keycode_to_name(0x5A), "F20");
    }

    #[test]
    fn unknown_keycode_falls_back_to_vk_hex() {
        // 0xFF isn't a real kVK_* — the fallback should expose it verbatim
        // so the user sees something rather than a silent drop.
        assert_eq!(keycode_to_name(0xFF), "VK_FF");
    }

    #[test]
    fn keycode_names_parse_as_global_shortcuts() {
        use std::str::FromStr;
        for code in 0u16..=0x7F {
            let name = keycode_to_name(code);
            if name.starts_with("VK_") {
                continue;
            }
            let combo = format!("Super+{}", name);
            assert!(
                tauri_plugin_global_shortcut::Shortcut::from_str(&combo).is_ok(),
                "keycode {:#04x} maps to {:?}, which global-hotkey can't parse",
                code,
                name
            );
        }
    }

    #[test]
    fn fn_flag_implied_only_for_fn_row_and_navigation() {
        assert!(fn_flag_is_implied(0x7E)); // Up
        assert!(fn_flag_is_implied(0x67)); // F11
        assert!(fn_flag_is_implied(0x73)); // Home
        assert!(!fn_flag_is_implied(0x0C)); // Q — fn+Q (Quick Note) is a real Fn combo
        assert!(!fn_flag_is_implied(0x31)); // Space
    }
}
