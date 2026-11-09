//! Extension view windows: on-demand Kage windows whose content an
//! extension draws (docs/design/EXTENSION_VIEWS.md §6, §7, §16).
//!
//! This is the skeleton: the wire types the view commands return and the
//! registry type `UiState` will hold. The window lifecycle (build, place,
//! cache, tear down, persist geometry) comes with WP-V2. Until then the
//! commands in `commands::extensions::views` refuse or return nothing.

use serde::Serialize;

/// What opened a view (§9.1). Informational: it reaches the view as
/// `source`. Anything else is read as `launcher`.
pub const VIEW_SOURCES: &[&str] = &[
    "launcher",
    "chat",
    "formatter",
    "widget",
    "toolbar",
    "settings",
    "view",
];

/// The `source` an `ext_view_open` caller passed, checked against
/// [`VIEW_SOURCES`].
pub fn normalize_source(source: Option<&str>) -> &'static str {
    source
        .and_then(|s| VIEW_SOURCES.iter().copied().find(|known| *known == s))
        .unwrap_or("launcher")
}

/// `ext_view_open`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtViewOpened {
    /// The view window's label (`ext-view:<extension>:<view>`).
    pub label: String,
    /// The window already existed (visible or cached hidden).
    pub reused: bool,
}

/// `ext_view_boot`'s answer: what the calling view window shows. Derived
/// from the window's label, never from an argument.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtViewBoot {
    pub extension_id: String,
    pub view_id: String,
    pub params: Option<serde_json::Value>,
    /// 1 on the first open, one more on every open after it.
    pub open_seq: u64,
    /// One of [`VIEW_SOURCES`].
    pub source: String,
    /// The page booted before in this window (a reload in place).
    pub reloaded: bool,
}

/// `ext_view_focus_state`'s answer, for the launcher's blur handler.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ExtViewFocusState {
    pub view_focused: bool,
}

/// One entry of `ext_view_list`, for Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtViewInfo {
    pub extension_id: String,
    pub view_id: String,
    pub visible: bool,
    pub focused: bool,
}

/// Open view windows by label (§16.2): their boot records, hide timers,
/// pending geometry and rate limits. WP-V2 fills it in and adds it to
/// `UiState` as `ext_views`, so no new managed type is needed.
#[derive(Debug, Default)]
pub struct Registry {
    _records: std::sync::Mutex<()>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_is_one_of_the_closed_list() {
        for source in VIEW_SOURCES {
            assert_eq!(normalize_source(Some(source)), *source);
        }
        assert_eq!(normalize_source(None), "launcher");
        assert_eq!(normalize_source(Some("")), "launcher");
        assert_eq!(normalize_source(Some("Chat")), "launcher");
        assert_eq!(normalize_source(Some("deep-link")), "launcher");
    }

    #[test]
    fn wire_shapes() {
        let opened = ExtViewOpened {
            label: "ext-view:mindmap:explorer".into(),
            reused: false,
        };
        assert_eq!(
            serde_json::to_value(&opened).unwrap(),
            serde_json::json!({ "label": "ext-view:mindmap:explorer", "reused": false })
        );
        let boot = ExtViewBoot {
            extension_id: "mindmap".into(),
            view_id: "explorer".into(),
            params: None,
            open_seq: 1,
            source: "launcher".into(),
            reloaded: false,
        };
        assert_eq!(
            serde_json::to_value(&boot).unwrap(),
            serde_json::json!({
                "extension_id": "mindmap", "view_id": "explorer", "params": null,
                "open_seq": 1, "source": "launcher", "reloaded": false
            })
        );
        assert_eq!(
            serde_json::to_value(ExtViewFocusState::default()).unwrap(),
            serde_json::json!({ "view_focused": false })
        );
        let info = ExtViewInfo {
            extension_id: "mindmap".into(),
            view_id: "explorer".into(),
            visible: true,
            focused: false,
        };
        assert_eq!(
            serde_json::to_value(&info).unwrap(),
            serde_json::json!({
                "extension_id": "mindmap", "view_id": "explorer",
                "visible": true, "focused": false
            })
        );
    }
}
