//! Window labels — string identifiers Tauri uses to find a webview by name.
//!
//! These are referenced from many places (`get_webview_window`, the
//! pre-declared windows in `tauri.conf.json`, `WindowEvent` matchers,
//! `setup.rs`'s blur-handler installer, the chat-window broadcast loop).
//! Centralising them here means a typo in one place won't silently
//! create a "second main window" or fail a label-equality check.
//!
//! `tauri.conf.json` is JSON config and can't reference Rust consts —
//! the labels declared there for `main`, `floating`, and `inline-assist`
//! must match the values below by hand. The `cfg_label_matches` test
//! enforces that link.
//!
//! Mirror constants live in `ui/js/shared/window-labels.js` for the
//! frontend; the same alignment test verifies they agree.

/// Primary chat-sessions window. Pre-declared in `tauri.conf.json`.
pub const MAIN: &str = "main";

/// Always-on-top floating launcher / quick-prompt window.
/// Pre-declared in `tauri.conf.json`.
pub const FLOATING: &str = "floating";

/// On-demand settings panel. Created by `WebviewWindowBuilder` in
/// `commands::window::open_settings_window`.
pub const SETTINGS: &str = "settings";

/// On-demand right-click menu surface for floating/main.
pub const CONTEXT_MENU: &str = "context-menu";

/// First-run onboarding window. Created and destroyed lazily.
pub const WELCOME: &str = "welcome";

/// Extension store browser. Created on demand.
pub const STORE: &str = "store";

/// Inline-assist popup, summoned by the inline-assist hotkey.
/// Pre-declared in `tauri.conf.json`.
pub const INLINE_ASSIST: &str = "inline-assist";

/// Per-conversation chat windows are labelled `chat-<uuid>`. Use
/// `chat_label(uuid)` to construct one and `is_chat_label(label)` to
/// match — never inline the prefix.
pub const CHAT_PREFIX: &str = "chat-";

/// Build the label for a chat window from its session UUID.
pub fn chat_label(session_uuid: &str) -> String {
    format!("{}{}", CHAT_PREFIX, session_uuid)
}

/// Whether a label refers to a per-conversation chat window.
pub fn is_chat_label(label: &str) -> bool {
    label.starts_with(CHAT_PREFIX)
}

/// Whether a label refers to a window that hosts a chat session
/// (the primary `main` window or any `chat-<uuid>` peer). Several
/// broadcast / fan-out paths target exactly this set.
pub fn is_session_host_label(label: &str) -> bool {
    label == MAIN || is_chat_label(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_label_prefixes_uuid() {
        assert_eq!(chat_label("abc-123"), "chat-abc-123");
    }

    #[test]
    fn chat_label_matcher_round_trips() {
        let l = chat_label("11111111-2222-3333-4444-555555555555");
        assert!(is_chat_label(&l));
        assert!(is_session_host_label(&l));
    }

    #[test]
    fn non_chat_labels_are_not_chat() {
        for label in [
            MAIN,
            FLOATING,
            SETTINGS,
            CONTEXT_MENU,
            WELCOME,
            STORE,
            INLINE_ASSIST,
        ] {
            assert!(
                !is_chat_label(label),
                "{} should not be a chat label",
                label
            );
        }
    }

    #[test]
    fn session_host_set_is_main_plus_chats() {
        assert!(is_session_host_label(MAIN));
        assert!(is_session_host_label(&chat_label("x")));
        // Nothing else qualifies — floating windows have their own pinned
        // session in UiState but aren't part of the broadcast set used
        // by setup.rs / commands::window.
        assert!(!is_session_host_label(FLOATING));
        assert!(!is_session_host_label(SETTINGS));
        assert!(!is_session_host_label(INLINE_ASSIST));
    }

    /// Verify the labels declared in `tauri.conf.json` agree with the
    /// constants here. Tauri's `windows[]` array is JSON config; if a
    /// dev reads `MAIN` to look up the pre-declared window but
    /// `tauri.conf.json` has been hand-edited to "Main" or "kage-main",
    /// `get_webview_window(MAIN)` returns None and the app silently
    /// loses its main window.
    #[test]
    fn tauri_conf_json_labels_match_constants() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read tauri.conf.json at {:?}: {}", path, e));
        let conf: serde_json::Value = serde_json::from_str(&text).expect("parse tauri.conf.json");
        let windows = conf
            .pointer("/app/windows")
            .and_then(|v| v.as_array())
            .expect("app.windows[] missing or not an array");
        let labels: Vec<String> = windows
            .iter()
            .filter_map(|w| w.get("label").and_then(|l| l.as_str()).map(String::from))
            .collect();
        assert!(
            labels.contains(&MAIN.to_string()),
            "tauri.conf.json must declare a window with label {:?} (found {:?})",
            MAIN,
            labels
        );
        assert!(
            labels.contains(&FLOATING.to_string()),
            "tauri.conf.json must declare a window with label {:?} (found {:?})",
            FLOATING,
            labels
        );
        assert!(
            labels.contains(&INLINE_ASSIST.to_string()),
            "tauri.conf.json must declare a window with label {:?} (found {:?})",
            INLINE_ASSIST,
            labels
        );
    }
}
