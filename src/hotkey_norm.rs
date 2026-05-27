//! Pure (no-Tauri) helpers for hotkey-string handling. Lives outside the
//! `commands` module so unit tests can exercise it without dragging in
//! Tauri / global-shortcut state setup.

/// Normalize a hotkey string for equality comparison: lowercase + sort
/// modifiers (the trailing key is kept in last position).
///
/// `"Ctrl+Shift+A"` and `"Shift+Ctrl+A"` are the same hotkey to the OS;
/// without this normalisation, `try_register_hotkey`'s conflict check
/// would let the user re-bind a slot to a combo already used by another
/// slot just by reordering modifiers.
pub fn normalize_hotkey(s: &str) -> String {
    let mut parts: Vec<String> = s.split('+').map(|p| p.trim().to_lowercase()).collect();
    if parts.len() > 1 {
        let key = parts.pop().expect("len > 1 so pop is Some");
        parts.sort();
        parts.push(key);
    }
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_and_sorts_modifiers() {
        assert_eq!(normalize_hotkey("Ctrl+Shift+A"), "ctrl+shift+a");
        assert_eq!(
            normalize_hotkey("Shift+Ctrl+A"),
            normalize_hotkey("Ctrl+Shift+A")
        );
    }

    #[test]
    fn keeps_key_in_last_position() {
        // Even though "z" sorts after "shift", the trailing key must stay last
        // so we don't swap modifier and key when modifiers happen to sort
        // before the key alphabetically.
        let n = normalize_hotkey("Shift+Ctrl+Z");
        assert!(n.ends_with("+z"), "key must be last: got {}", n);
    }

    #[test]
    fn handles_single_key_no_modifiers() {
        // F1 with no modifiers — len == 1, so the .pop()/sort branch is skipped.
        assert_eq!(normalize_hotkey("F1"), "f1");
        assert_eq!(normalize_hotkey(""), "");
    }

    #[test]
    fn trims_whitespace_around_parts() {
        // Hand-edited config might surface "Ctrl + Shift+A" — handle gracefully.
        assert_eq!(normalize_hotkey("Ctrl + Shift+A"), "ctrl+shift+a");
    }

    #[test]
    fn case_insensitive_matches_across_orderings() {
        // The whole point: two strings that mean the same combo must produce
        // the same key.
        assert_eq!(
            normalize_hotkey("CTRL+ALT+space"),
            normalize_hotkey("alt+ctrl+SPACE")
        );
    }
}
