//! Storage change events: tell an extension's instances in other windows
//! that its stored data changed (docs/design/EXTENSION_VIEWS.md §10).
//!
//! Writers call [`record`] after the data is on disk: `save_extension_data`
//! and `delete_extension_data` (source `window`), a committed backup
//! import and uninstall (`bulk`), and later folder sync (`sync`). The
//! coalescer collects one extension's changes for [`COALESCE_WINDOW`],
//! keeps the last change per key, and emits them as one
//! `extension_data_changed` batch through
//! `event_targets::emit_to_extension_hosts`. Each window's host code
//! (`ui/js/shared/extension-data-events.js`) forwards the batch to that
//! extension's sandbox only.
//!
//! Events carry key names only, never data. They are hints, not a log: a
//! window that wasn't listening misses them, so instances still check their
//! own revision markers when they start.

use crate::lock_ext::LockExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Runtime};

/// How long the coalescer collects one extension's changes before it
/// emits them as one batch. The window starts at the batch's first change
/// and doesn't move, so a steady stream of writes still emits every
/// 100 ms.
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

    /// A key that a window's sandbox saved or deleted.
    pub fn from_window(key: impl Into<String>, op: ChangeOp, window: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            op,
            source: ChangeSource::Window,
            window: Some(window.into()),
        }
    }

    /// Whether the key is one a change may name: a valid data key, or
    /// [`BULK_KEY`] for a `bulk` change.
    fn has_valid_key(&self) -> bool {
        match self.op {
            ChangeOp::Bulk => self.key == BULK_KEY,
            ChangeOp::Save | ChangeOp::Delete => {
                crate::extensions::validate_data_key(&self.key).is_ok()
            }
        }
    }
}

/// The `extension_data_changed` payload: one extension's changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    pub extension_id: String,
    pub changes: Vec<Change>,
}

/// One extension's changes waiting for the end of its window.
#[derive(Debug, Default)]
struct Pending {
    /// In write order, one entry per key. After an overflow, the single
    /// bulk change.
    changes: Vec<Change>,
    /// More than [`MAX_BATCH_KEYS`] keys changed: `changes` holds one bulk
    /// change that stands for all of them.
    overflowed: bool,
}

impl Pending {
    fn push(&mut self, change: Change) {
        if self.overflowed {
            merge_into_bulk(&mut self.changes[0], &change);
            return;
        }
        // The last change to a key wins, at the position of that write.
        self.changes.retain(|c| c.key != change.key);
        self.changes.push(change);
        if self.changes.len() > MAX_BATCH_KEYS {
            let mut changes = std::mem::take(&mut self.changes).into_iter();
            let first = changes.next().expect("more than MAX_BATCH_KEYS changes");
            let mut bulk = Change {
                window: first.window.clone(),
                ..Change::bulk(first.source)
            };
            for c in changes {
                merge_into_bulk(&mut bulk, &c);
            }
            self.changes = vec![bulk];
            self.overflowed = true;
        }
    }
}

/// Fold `change` into the bulk change that replaced an overflowing batch.
/// The source is the latest writer's. The window label stays only while
/// every change came from that one window, so a window's sandbox sees
/// `self: true` only for a bulk it wrote all of.
fn merge_into_bulk(bulk: &mut Change, change: &Change) {
    bulk.source = change.source;
    if bulk.window != change.window {
        bulk.window = None;
    }
}

/// Per-extension batches. Pure: the caller owns time (see [`record`]).
#[derive(Debug, Default)]
pub struct Coalescer {
    pending: HashMap<String, Pending>,
}

impl Coalescer {
    /// Add one change to the extension's batch. Returns `true` when it
    /// starts a new batch: the caller then emits [`Self::take`] after
    /// [`COALESCE_WINDOW`].
    pub fn record(&mut self, extension_id: &str, change: Change) -> bool {
        let starts = !self.pending.contains_key(extension_id);
        self.pending
            .entry(extension_id.to_string())
            .or_default()
            .push(change);
        starts
    }

    /// The extension's batch, which ends it. `None` when nothing changed.
    pub fn take(&mut self, extension_id: &str) -> Option<Batch> {
        let pending = self.pending.remove(extension_id)?;
        (!pending.changes.is_empty()).then(|| Batch {
            extension_id: extension_id.to_string(),
            changes: pending.changes,
        })
    }
}

/// The process's coalescer. Kage runs one app, so one is enough.
static COALESCER: LazyLock<Mutex<Coalescer>> = LazyLock::new(Default::default);

/// Record that one of `extension_id`'s keys changed. Call it after the
/// change is on disk (readable by every window). Every window that runs
/// extension instances gets the batch within [`COALESCE_WINDOW`].
pub fn record<R: Runtime>(app: &AppHandle<R>, extension_id: &str, change: Change) {
    if crate::extensions::validate_extension_id(extension_id).is_err() {
        log::warn!("extension_data_changed: dropped a change for an invalid extension id");
        return;
    }
    if !change.has_valid_key() {
        log::warn!(
            "extension_data_changed: dropped a change with an invalid key for '{}'",
            extension_id
        );
        return;
    }
    let starts = COALESCER.lock_or_recover().record(extension_id, change);
    if !starts {
        return;
    }
    let app = app.clone();
    let extension_id = extension_id.to_string();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(COALESCE_WINDOW).await;
        flush(&app, &extension_id);
    });
}

/// Record a `bulk` change: any of the extension's keys may have changed.
pub fn record_bulk<R: Runtime>(app: &AppHandle<R>, extension_id: &str, source: ChangeSource) {
    record(app, extension_id, Change::bulk(source));
}

/// Emit the extension's batch, if it has one.
fn flush<R: Runtime>(app: &AppHandle<R>, extension_id: &str) {
    let Some(batch) = COALESCER.lock_or_recover().take(extension_id) else {
        return;
    };
    log::debug!(
        "extension_data_changed: {} change(s) for '{}'",
        batch.changes.len(),
        extension_id
    );
    crate::event_targets::emit_to_extension_hosts(
        app,
        crate::events::EXTENSION_DATA_CHANGED,
        &batch,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn save(key: &str, window: &str) -> Change {
        Change::from_window(key, ChangeOp::Save, window)
    }

    fn delete(key: &str, window: &str) -> Change {
        Change::from_window(key, ChangeOp::Delete, window)
    }

    fn keys(batch: &Batch) -> Vec<(&str, ChangeOp)> {
        batch
            .changes
            .iter()
            .map(|c| (c.key.as_str(), c.op))
            .collect()
    }

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

    #[test]
    fn changes_within_one_window_make_one_batch() {
        let mut c = Coalescer::default();
        assert!(c.record("mindmap", save("mm-meta", "floating")));
        assert!(!c.record("mindmap", save("mm-0", "floating")));
        assert!(!c.record("mindmap", delete("mm-1", "main")));
        let batch = c.take("mindmap").expect("a batch");
        assert_eq!(batch.extension_id, "mindmap");
        assert_eq!(
            keys(&batch),
            vec![
                ("mm-meta", ChangeOp::Save),
                ("mm-0", ChangeOp::Save),
                ("mm-1", ChangeOp::Delete)
            ]
        );
        // Taken: the next change starts a new batch.
        assert!(c.take("mindmap").is_none());
        assert!(c.record("mindmap", save("mm-meta", "floating")));
    }

    #[test]
    fn the_last_change_to_a_key_wins_at_its_place_in_write_order() {
        let mut c = Coalescer::default();
        c.record("todos", save("a", "floating"));
        c.record("todos", save("b", "floating"));
        c.record("todos", delete("a", "settings"));
        let batch = c.take("todos").unwrap();
        assert_eq!(
            keys(&batch),
            vec![("b", ChangeOp::Save), ("a", ChangeOp::Delete)]
        );
        assert_eq!(batch.changes[1].window.as_deref(), Some("settings"));
    }

    #[test]
    fn the_writing_window_label_is_kept() {
        let mut c = Coalescer::default();
        c.record("todos", save("list", "ext-view:todos:board"));
        c.record("todos", Change::bulk(ChangeSource::Import));
        let batch = c.take("todos").unwrap();
        assert_eq!(batch.changes[0].source, ChangeSource::Window);
        assert_eq!(
            batch.changes[0].window.as_deref(),
            Some("ext-view:todos:board")
        );
        assert_eq!(batch.changes[1], Change::bulk(ChangeSource::Import));
    }

    #[test]
    fn extensions_are_batched_separately() {
        let mut c = Coalescer::default();
        assert!(c.record("a", save("k", "floating")));
        assert!(c.record("b", save("k", "floating")));
        assert!(!c.record("a", save("j", "floating")));
        let a = c.take("a").unwrap();
        assert_eq!(keys(&a), vec![("k", ChangeOp::Save), ("j", ChangeOp::Save)]);
        // Taking one leaves the other waiting.
        let b = c.take("b").unwrap();
        assert_eq!(b.extension_id, "b");
        assert_eq!(keys(&b), vec![("k", ChangeOp::Save)]);
        assert!(c.take("b").is_none());
    }

    #[test]
    fn up_to_max_keys_stay_named() {
        let mut c = Coalescer::default();
        for i in 0..MAX_BATCH_KEYS {
            c.record("mindmap", save(&format!("k{i}"), "floating"));
        }
        // Rewriting a key already in the batch doesn't add one.
        c.record("mindmap", save("k0", "floating"));
        let batch = c.take("mindmap").unwrap();
        assert_eq!(batch.changes.len(), MAX_BATCH_KEYS);
        assert_eq!(batch.changes.last().unwrap().key, "k0");
    }

    #[test]
    fn more_than_max_keys_become_one_bulk_change() {
        let mut c = Coalescer::default();
        for i in 0..=MAX_BATCH_KEYS {
            c.record("mindmap", save(&format!("k{i}"), "floating"));
        }
        // Later changes fold into the bulk change, so memory stays bounded.
        for i in 0..200 {
            assert!(!c.record("mindmap", delete(&format!("x{i}"), "floating")));
        }
        let batch = c.take("mindmap").unwrap();
        assert_eq!(
            batch.changes,
            vec![Change {
                key: BULK_KEY.into(),
                op: ChangeOp::Bulk,
                source: ChangeSource::Window,
                // One window wrote all of it: that window sees `self`.
                window: Some("floating".into()),
            }]
        );
        // The overflow ends with the batch.
        assert!(c.record("mindmap", save("k0", "floating")));
        assert_eq!(c.take("mindmap").unwrap().changes.len(), 1);
    }

    #[test]
    fn a_bulk_from_several_writers_names_no_window() {
        let mut c = Coalescer::default();
        for i in 0..=MAX_BATCH_KEYS {
            c.record("mindmap", save(&format!("k{i}"), "floating"));
        }
        c.record("mindmap", save("late", "ext-view:mindmap:explorer"));
        let batch = c.take("mindmap").unwrap();
        assert_eq!(batch.changes.len(), 1);
        assert_eq!(batch.changes[0].op, ChangeOp::Bulk);
        assert_eq!(batch.changes[0].window, None);

        // Mixed already when it overflows; the latest writer's source.
        for i in 0..MAX_BATCH_KEYS {
            c.record("mindmap", save(&format!("k{i}"), "floating"));
        }
        c.record(
            "mindmap",
            Change {
                key: "synced".into(),
                op: ChangeOp::Save,
                source: ChangeSource::Sync,
                window: None,
            },
        );
        let batch = c.take("mindmap").unwrap();
        assert_eq!(batch.changes[0].window, None);
        assert_eq!(batch.changes[0].source, ChangeSource::Sync);
    }

    #[test]
    fn only_valid_keys_can_be_named() {
        assert!(save("mm-meta", "floating").has_valid_key());
        assert!(Change::bulk(ChangeSource::Uninstall).has_valid_key());
        assert!(!save("*", "floating").has_valid_key());
        assert!(!save("../x", "floating").has_valid_key());
        assert!(!save("", "floating").has_valid_key());
        let bulk_with_a_name = Change {
            key: "mm-meta".into(),
            ..Change::bulk(ChangeSource::Import)
        };
        assert!(!bulk_with_a_name.has_valid_key());
    }
}
