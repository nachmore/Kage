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

/// Extension view windows are labelled `ext-view:<extensionId>:<viewId>`
/// (docs/design/EXTENSION_VIEWS.md §7.1). Use `ext_view_label` to build
/// one and `parse_ext_view_label` / `is_ext_view_label` to match — never
/// inline the prefix.
pub const EXT_VIEW_PREFIX: &str = "ext-view:";

/// Build the label for an extension's view window. `None` when either id
/// is invalid, so a label can never carry a second `:` separator.
pub fn ext_view_label(extension_id: &str, view_id: &str) -> Option<String> {
    valid_ext_view_parts(extension_id, view_id)
        .then(|| format!("{}{}:{}", EXT_VIEW_PREFIX, extension_id, view_id))
}

/// Split a view window label into `(extension_id, view_id)`. `None` for
/// any other label, including a prefixed one with invalid ids.
pub fn parse_ext_view_label(label: &str) -> Option<(&str, &str)> {
    let (extension_id, view_id) = label.strip_prefix(EXT_VIEW_PREFIX)?.split_once(':')?;
    valid_ext_view_parts(extension_id, view_id).then_some((extension_id, view_id))
}

/// Whether a label refers to an extension view window.
pub fn is_ext_view_label(label: &str) -> bool {
    parse_ext_view_label(label).is_some()
}

fn valid_ext_view_parts(extension_id: &str, view_id: &str) -> bool {
    crate::extensions::validate_extension_id(extension_id).is_ok()
        && crate::extensions::views::is_valid_view_id(view_id)
}

/// Whether a label refers to a window that runs extension instances:
/// the launcher, chat hosts, settings and view windows. The audience of
/// `event_targets::emit_to_extension_hosts`.
pub fn is_extension_host_label(label: &str) -> bool {
    label == FLOATING
        || label == SETTINGS
        || is_session_host_label(label)
        || is_ext_view_label(label)
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
            "ext-view:mindmap:explorer",
        ] {
            assert!(
                !is_chat_label(label),
                "{} should not be a chat label",
                label
            );
        }
    }

    #[test]
    fn ext_view_label_round_trips() {
        let label = ext_view_label("mindmap", "explorer").expect("valid ids");
        assert_eq!(label, "ext-view:mindmap:explorer");
        assert!(label.starts_with(EXT_VIEW_PREFIX));
        assert_eq!(parse_ext_view_label(&label), Some(("mindmap", "explorer")));
        assert!(is_ext_view_label(&label));
        assert!(!is_session_host_label(&label));
        let label = ext_view_label("my_ext-2", "map-2").expect("valid ids");
        assert_eq!(parse_ext_view_label(&label), Some(("my_ext-2", "map-2")));
    }

    #[test]
    fn ext_view_label_refuses_ids_that_would_inject_a_separator() {
        assert_eq!(ext_view_label("mind:map", "explorer"), None);
        assert_eq!(ext_view_label("mindmap", "explorer:x"), None);
        assert_eq!(ext_view_label("", "explorer"), None);
        assert_eq!(ext_view_label("mindmap", ""), None);
        assert_eq!(ext_view_label("MindMap", "explorer"), None);
        assert_eq!(ext_view_label("mindmap", "Explorer"), None);
        assert_eq!(ext_view_label("../x", "explorer"), None);
        for label in [
            "ext-view:",
            "ext-view:mindmap",
            "ext-view:mindmap:",
            "ext-view::explorer",
            "ext-view:mind:map:explorer",
            "ext-view:mindmap:explorer:",
            "ext-view:mindmap:Explorer",
            "ext-view:mindmap:2d",
            "ext-view:-x:explorer",
            "ext-view-mindmap:explorer",
            "EXT-VIEW:mindmap:explorer",
            MAIN,
            FLOATING,
            SETTINGS,
            &chat_label("x"),
        ] {
            assert_eq!(parse_ext_view_label(label), None, "{label:?}");
            assert!(!is_ext_view_label(label), "{label:?}");
        }
    }

    #[test]
    fn extension_hosts_are_launcher_chat_settings_and_views() {
        for label in [
            FLOATING,
            MAIN,
            SETTINGS,
            &chat_label("11111111-2222-3333-4444-555555555555"),
            "ext-view:mindmap:explorer",
        ] {
            assert!(is_extension_host_label(label), "{label:?}");
        }
        for label in [
            CONTEXT_MENU,
            WELCOME,
            STORE,
            INLINE_ASSIST,
            "ext-view:bad:Label",
            "",
        ] {
            assert!(!is_extension_host_label(label), "{label:?}");
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
