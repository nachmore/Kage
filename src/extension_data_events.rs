//! Storage change events: tell an extension's instances in other windows
//! that its stored data changed (docs/design/EXTENSION_VIEWS.md §10).
//!
//! This is the skeleton: the wire types of `extension_data_changed` and
//! the coalescer's limits. WP-V3 adds the coalescer, which batches writes
//! per extension and emits through
//! `event_targets::emit_to_extension_hosts`. Events carry key names only,
//! never data.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// How long the coalescer collects one extension's changes before it
/// emits them as one batch.
pub const COALESCE_WINDOW: Duration = Duration::from_millis(100);

/// Most keys in one batch. A batch with more becomes a single
/// `{ key: BULK_KEY, op: Bulk }` change.
pub const MAX_BATCH_KEYS: usize = 64;

/// The key of a `bulk` change: any of the extension's keys may have
/// changed.
pub const BULK_KEY: &str = "*";

/// What happened to a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeOp {
    Save,
    Delete,
    /// Many keys, or keys we can't name: re-read everything.
    Bulk,
}

/// What wrote the change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSource {
    /// `save_extension_data` / `delete_extension_data` from a window.
    Window,
    /// A committed backup import.
    Import,
    /// Folder sync.
    Sync,
    /// The extension was uninstalled.
    Uninstall,
}

/// One changed key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub key: String,
    pub op: ChangeOp,
    pub source: ChangeSource,
    /// Label of the window whose sandbox wrote it (`source: window` only).
    /// The host code turns it into `self` and drops it before the sandbox
    /// sees the change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
}

impl Change {
    /// A `bulk` change for every key of an extension.
    pub fn bulk(source: ChangeSource) -> Self {
        Self {
            key: BULK_KEY.to_string(),
            op: ChangeOp::Bulk,
            source,
            window: None,
        }
    }
}

/// The `extension_data_changed` payload: one extension's changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    pub extension_id: String,
    pub changes: Vec<Change>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn batch_wire_shape() {
        let batch = Batch {
            extension_id: "mindmap".into(),
            changes: vec![
                Change {
                    key: "mm-meta".into(),
                    op: ChangeOp::Save,
                    source: ChangeSource::Window,
                    window: Some("floating".into()),
                },
                Change::bulk(ChangeSource::Import),
            ],
        };
        let wire = serde_json::to_value(&batch).unwrap();
        assert_eq!(
            wire,
            json!({
                "extension_id": "mindmap",
                "changes": [
                    { "key": "mm-meta", "op": "save", "source": "window", "window": "floating" },
                    { "key": "*", "op": "bulk", "source": "import" }
                ]
            })
        );
        let back: Batch = serde_json::from_value(wire).unwrap();
        assert_eq!(back, batch);
    }

    #[test]
    fn op_and_source_names() {
        let names = |v: serde_json::Value| v.as_str().unwrap().to_string();
        assert_eq!(names(json!(ChangeOp::Delete)), "delete");
        assert_eq!(names(json!(ChangeSource::Sync)), "sync");
        assert_eq!(names(json!(ChangeSource::Uninstall)), "uninstall");
    }
}
