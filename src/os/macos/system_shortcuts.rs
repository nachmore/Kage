//! macOS "symbolic hotkeys" — the system-wide shortcuts listed under
//! System Settings → Keyboard → Keyboard Shortcuts (Spotlight ⌘Space,
//! input-source Ctrl+Space, Mission Control, screenshots, …).
//!
//! Read and toggled through three CoreGraphics (SkyLight) SPI functions
//! that have been stable since 10.x and are what window-switcher utilities
//! such as AltTab use to replace ⌘Tab:
//!
//! - `CGSGetSymbolicHotKeyValue(id, &char, &keycode, &modifiers)`
//! - `CGSIsSymbolicHotKeyEnabled(id)`
//! - `CGSSetSymbolicHotKeyEnabled(id, enabled)`
//!
//! Verified on macOS 26.7 (arm64): all three work from an unsigned,
//! untrusted process — no Accessibility or Input Monitoring needed. The
//! enabled flag lives in the WindowServer session: it does NOT touch
//! `com.apple.symbolichotkeys.plist`, survives the calling process exiting,
//! and resets at logout. Callers must therefore restore what they disable
//! (see `crate::system_shortcut_takeover`, which also persists what it owns
//! so a crashed run is undone on the next launch).

use super::keycodes::{fn_flag_is_implied, keycode_to_name};
use crate::os::system_shortcuts::SystemShortcut;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGSGetSymbolicHotKeyValue(
        hot_key: i32,
        out_key_equivalent: *mut u16,
        out_virtual_key_code: *mut u16,
        out_modifiers: *mut u32,
    ) -> i32;
    fn CGSIsSymbolicHotKeyEnabled(hot_key: i32) -> bool;
    fn CGSSetSymbolicHotKeyEnabled(hot_key: i32, is_enabled: bool) -> i32;
}

/// Highest id scanned by `list_impl`. macOS 26 defines ids up to ~260;
/// 511 leaves headroom for future releases at ~40 ms per full scan.
const MAX_SYMBOLIC_HOTKEY_ID: u32 = 511;

/// `CGSGetSymbolicHotKeyValue` reports this keycode for unbound entries.
const UNBOUND_KEYCODE: u16 = 0xFFFF;

// NSEvent / CGEvent modifier-flag bits as stored in symbolic hotkey records.
const FLAG_SHIFT: u32 = 1 << 17;
const FLAG_CONTROL: u32 = 1 << 18;
const FLAG_OPTION: u32 = 1 << 19;
const FLAG_COMMAND: u32 = 1 << 20;
const FLAG_NUMERIC_PAD: u32 = 1 << 21;
const FLAG_FUNCTION: u32 = 1 << 23;

pub fn list_impl() -> Vec<SystemShortcut> {
    (0..=MAX_SYMBOLIC_HOTKEY_ID).filter_map(get_impl).collect()
}

pub fn get_impl(id: u32) -> Option<SystemShortcut> {
    let raw_id = i32::try_from(id).ok()?;
    let mut key_equivalent: u16 = 0;
    let mut keycode: u16 = 0;
    let mut modifiers: u32 = 0;
    // SAFETY: plain out-params; the SPI writes three scalars and returns a
    // CGError. Unknown ids return a non-zero error without touching them.
    let err = unsafe {
        CGSGetSymbolicHotKeyValue(raw_id, &mut key_equivalent, &mut keycode, &mut modifiers)
    };
    if err != 0 {
        return None;
    }
    let (modifiers, key) = decode_binding(keycode, modifiers)?;
    // SAFETY: scalar in, scalar out.
    let enabled = unsafe { CGSIsSymbolicHotKeyEnabled(raw_id) };
    Some(SystemShortcut {
        id,
        enabled,
        modifiers,
        key,
    })
}

pub fn set_enabled_impl(id: u32, enabled: bool) -> bool {
    let Ok(raw_id) = i32::try_from(id) else {
        return false;
    };
    // SAFETY: scalar in, CGError out.
    let err = unsafe { CGSSetSymbolicHotKeyEnabled(raw_id, enabled) };
    if err != 0 {
        log::warn!(
            "[system-shortcuts] CGSSetSymbolicHotKeyEnabled({}, {}) failed: {}",
            id,
            enabled,
            err
        );
        return false;
    }
    true
}

pub fn is_enabled_impl(id: u32) -> Option<bool> {
    let raw_id = i32::try_from(id).ok()?;
    // Existence check first: CGSIsSymbolicHotKeyEnabled returns false for
    // ids that don't exist, which would read as "disabled".
    let (mut a, mut b, mut c) = (0u16, 0u16, 0u32);
    // SAFETY: as in get_impl.
    if unsafe { CGSGetSymbolicHotKeyValue(raw_id, &mut a, &mut b, &mut c) } != 0 {
        return None;
    }
    // SAFETY: scalar in, scalar out.
    Some(unsafe { CGSIsSymbolicHotKeyEnabled(raw_id) })
}

/// Turn a raw (keycode, modifier-flags) binding into Kage's
/// (modifiers, key) vocabulary. `None` when the entry is unbound or uses
/// something Kage can't register (an unknown keycode, or Fn as a genuine
/// modifier such as Fn+Q for Quick Note).
fn decode_binding(keycode: u16, flags: u32) -> Option<(Vec<String>, String)> {
    if keycode == UNBOUND_KEYCODE {
        return None;
    }
    let key = keycode_to_name(keycode);
    if key.starts_with("VK_") {
        return None;
    }
    if flags & FLAG_FUNCTION != 0 && !fn_flag_is_implied(keycode) {
        return None;
    }
    // FLAG_NUMERIC_PAD rides along on arrows / keypad keys and carries no
    // meaning for matching; any other unknown bit makes the binding
    // unrepresentable.
    let known = FLAG_SHIFT | FLAG_CONTROL | FLAG_OPTION | FLAG_COMMAND;
    let ignorable = FLAG_NUMERIC_PAD | FLAG_FUNCTION;
    if flags & !(known | ignorable) & 0xFFFF_0000 != 0 {
        return None;
    }
    // Same order as the capture paths produce: Ctrl, Alt, Shift, Super.
    let mut modifiers = Vec::new();
    if flags & FLAG_CONTROL != 0 {
        modifiers.push("Ctrl".to_string());
    }
    if flags & FLAG_OPTION != 0 {
        modifiers.push("Alt".to_string());
    }
    if flags & FLAG_SHIFT != 0 {
        modifiers.push("Shift".to_string());
    }
    if flags & FLAG_COMMAND != 0 {
        modifiers.push("Super".to_string());
    }
    Some((modifiers, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn decodes_spotlight_binding() {
        // id 64 on a stock install: keycode 49 (Space), flags 0x100000 (⌘).
        assert_eq!(
            decode_binding(49, 0x0010_0000),
            Some((s(&["Super"]), "Space".to_string()))
        );
    }

    #[test]
    fn decodes_finder_search_and_input_source() {
        // id 65: ⌥⌘Space (0x180000); id 61: ⌃⌥Space (0xC0000).
        assert_eq!(
            decode_binding(49, 0x0018_0000),
            Some((s(&["Alt", "Super"]), "Space".to_string()))
        );
        assert_eq!(
            decode_binding(49, 0x000C_0000),
            Some((s(&["Ctrl", "Alt"]), "Space".to_string()))
        );
    }

    #[test]
    fn strips_implied_fn_and_numpad_flags_on_arrows() {
        // Mission Control (id 32): keycode 126 (Up), flags 0x840000 = ⌃ + Fn.
        assert_eq!(
            decode_binding(126, 0x0084_0000),
            Some((s(&["Ctrl"]), "Up".to_string()))
        );
        // Ctrl+Shift+Up with the numeric-pad bit (0x860000 + 0x200000).
        assert_eq!(
            decode_binding(126, 0x00A6_0000),
            Some((s(&["Ctrl", "Shift"]), "Up".to_string()))
        );
    }

    #[test]
    fn rejects_real_fn_combos_and_unbound_entries() {
        // Quick Note: Fn+Q (keycode 12, flags 0x800000).
        assert_eq!(decode_binding(12, 0x0080_0000), None);
        assert_eq!(decode_binding(UNBOUND_KEYCODE, 0), None);
        // Media / brightness keys (keycode > 0x7F) have no Kage name.
        assert_eq!(decode_binding(160, 0x0080_0000), None);
    }

    /// Live, read-only: every binding WindowServer reports must decode into
    /// something global-hotkey can parse, or conflict detection would
    /// silently miss it. Doesn't assert a count — CI sessions may have no
    /// WindowServer connection and return nothing.
    #[test]
    fn live_bindings_parse_as_global_shortcuts() {
        use std::str::FromStr;
        let all = list_impl();
        eprintln!("symbolic hotkeys visible: {}", all.len());
        for sc in &all {
            assert!(
                tauri_plugin_global_shortcut::Shortcut::from_str(&sc.combo_string()).is_ok(),
                "id {} decoded to unparseable {:?}",
                sc.id,
                sc.combo_string()
            );
            assert_eq!(is_enabled_impl(sc.id), Some(sc.enabled));
        }
        if let Some(spotlight) = all.iter().find(|s| s.id == 64) {
            eprintln!(
                "id 64 (Spotlight) = {} enabled={}",
                spotlight.combo_string(),
                spotlight.enabled
            );
        }
    }

    /// Live round-trip through the Rust FFI on a real session. Ignored by
    /// default because it briefly flips the user's Spotlight shortcut;
    /// run with `cargo test --lib live_toggle -- --ignored`.
    #[test]
    #[ignore]
    fn live_toggle_roundtrip_restores_spotlight() {
        let Some(before) = is_enabled_impl(64) else {
            return;
        };
        assert!(set_enabled_impl(64, !before));
        let flipped = is_enabled_impl(64);
        assert!(set_enabled_impl(64, before));
        assert_eq!(flipped, Some(!before));
        assert_eq!(is_enabled_impl(64), Some(before));
    }

    #[test]
    fn combo_string_matches_config_format() {
        let sc = SystemShortcut {
            id: 64,
            enabled: true,
            modifiers: s(&["Super"]),
            key: "Space".into(),
        };
        assert_eq!(sc.combo_string(), "Super+Space");
    }
}
