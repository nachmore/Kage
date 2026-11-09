//! This computer's replica (`<data_local>/kage/sync/replica.db`,
//! docs/design/SYNC.md §6.2): every record it has merged, what each local
//! file shows, pins, devices, and the extension tables. The source of truth
//! for sync; `config.json` and extension data are projections of it.
//!
//! C0 skeleton: nothing opens yet, and a `Tx` answers "empty". WP-S1
//! writes the schema, its `user_version` migrations and the bodies.

use super::types::{DeviceId, LocalKey, OsKind, PinReason, Record, Scope, Stamp};
use super::NOT_BUILT;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// How [`Replica::open`] found the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenNote {
    /// There was no file; a new one was created.
    Fresh,
    /// An existing replica opened normally.
    Existing,
    /// The file was corrupt. It was moved aside and a new one created; the
    /// engine rebuilds it from the folder (§15).
    RebuiltFromCorrupt,
}

/// What a local file shows for one field, and which cell it came from:
/// the value's hash, the cell's stamp and scope (`None` for a local value).
pub type Projected = ([u8; 32], Option<Stamp>, Option<Scope>);

/// One row of the `devices` table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceRow {
    pub dev: DeviceId,
    pub name: String,
    pub os: Option<OsKind>,
    pub kage_version: String,
    pub head_seq: u64,
    pub ingested_seq: u64,
    pub last_seen_ms: i64,
    pub instance: String,
    pub min_reader: u32,
    pub gc_floor: Option<Stamp>,
    /// `ok`, `stale`, `inactive`, `needs_update` or `retired`.
    pub state: String,
}

/// The replica database. Only the engine thread holds it.
pub struct Replica {
    _conn: rusqlite::Connection,
}

impl Replica {
    /// Open (or create) the replica at `path`.
    pub fn open(path: &Path) -> anyhow::Result<(Self, OpenNote)> {
        let _ = path;
        anyhow::bail!(NOT_BUILT)
    }

    /// A replica that lives in memory, for tests.
    pub fn open_in_memory() -> anyhow::Result<Self> {
        anyhow::bail!(NOT_BUILT)
    }

    /// Run `f` in one transaction. Every multi-row change goes through here.
    pub fn tx<T>(&mut self, f: impl FnOnce(&mut Tx<'_>) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let _ = f;
        anyhow::bail!(NOT_BUILT)
    }
}

/// One transaction on the replica.
pub struct Tx<'a> {
    _tx: rusqlite::Transaction<'a>,
}

impl Tx<'_> {
    pub fn record(&self, ns: &str, id: &str) -> anyhow::Result<Option<Record>> {
        let _ = (ns, id);
        Ok(None)
    }

    /// Store a record in `bucket` (§5.6).
    pub fn put_record(&mut self, r: &Record, bucket: u32) -> anyhow::Result<()> {
        let _ = (r, bucket);
        Ok(())
    }

    /// The records of `ns` in `bucket` out of `of`.
    pub fn records_in_bucket(&self, ns: &str, bucket: u32, of: u32) -> anyhow::Result<Vec<Record>> {
        let _ = (ns, bucket, of);
        Ok(Vec::new())
    }

    /// Mark a bucket for the next publish. `own` is a change made here, as
    /// against one only echoed.
    pub fn mark_dirty(&mut self, ns: &str, bucket: u32, own: bool) -> anyhow::Result<()> {
        let _ = (ns, bucket, own);
        Ok(())
    }

    /// Take every dirty `(ns, bucket, own)` and clear the table.
    pub fn take_dirty(&mut self) -> anyhow::Result<Vec<(String, u32, bool)>> {
        Ok(Vec::new())
    }

    pub fn projected(&self, k: &LocalKey) -> anyhow::Result<Option<Projected>> {
        let _ = k;
        Ok(None)
    }

    /// `None` removes the row.
    pub fn set_projected(&mut self, k: &LocalKey, v: Option<Projected>) -> anyhow::Result<()> {
        let _ = (k, v);
        Ok(())
    }

    pub fn pin(&self, k: &LocalKey) -> anyhow::Result<Option<(serde_json::Value, PinReason)>> {
        let _ = k;
        Ok(None)
    }

    /// `None` removes the pin. Field `*` pins a whole item.
    pub fn set_pin(
        &mut self,
        k: &LocalKey,
        v: Option<(serde_json::Value, PinReason)>,
    ) -> anyhow::Result<()> {
        let _ = (k, v);
        Ok(())
    }

    /// Whether the shard named `shard` (from any device) is merged already.
    pub fn applied(&self, shard: &str) -> anyhow::Result<bool> {
        let _ = shard;
        Ok(false)
    }

    pub fn add_applied(&mut self, shard: &str) -> anyhow::Result<()> {
        let _ = shard;
        Ok(())
    }

    /// `dev` has seen the tombstone of `ns`/`id` (§8.7).
    pub fn ack(&mut self, ns: &str, id: &str, dev: DeviceId) -> anyhow::Result<()> {
        let _ = (ns, id, dev);
        Ok(())
    }

    pub fn meta(&self, k: &str) -> anyhow::Result<Option<String>> {
        let _ = k;
        Ok(None)
    }

    pub fn set_meta(&mut self, k: &str, v: &str) -> anyhow::Result<()> {
        let _ = (k, v);
        Ok(())
    }

    pub fn device(&self, dev: DeviceId) -> anyhow::Result<Option<DeviceRow>> {
        let _ = dev;
        Ok(None)
    }

    pub fn put_device(&mut self, row: &DeviceRow) -> anyhow::Result<()> {
        let _ = row;
        Ok(())
    }

    pub fn devices(&self) -> anyhow::Result<Vec<DeviceRow>> {
        Ok(Vec::new())
    }
}

/// The extension tables (§6.2, §12.4–§12.6), as a trait so the extension
/// source needs no `Tx` lifetimes. `Tx` implements it.
pub trait ExtStore {
    /// Append a change to an extension's feed. Returns its sequence number.
    fn feed_push(&mut self, ext: &str, coll: &str, id: &str) -> anyhow::Result<u64>;
    /// Up to `max` feed rows after `after`: `(seq, collection, id)`.
    fn feed_page(
        &self,
        ext: &str,
        after: u64,
        max: usize,
    ) -> anyhow::Result<Vec<(u64, String, String)>>;
    fn feed_ack(&mut self, ext: &str, upto: u64) -> anyhow::Result<()>;
    /// Per field, the cell the extension last saw: its stamp and value hash.
    fn ext_base(
        &self,
        ext: &str,
        coll: &str,
        id: &str,
    ) -> anyhow::Result<BTreeMap<String, (Stamp, [u8; 32])>>;
    fn set_ext_base(
        &mut self,
        ext: &str,
        coll: &str,
        id: &str,
        f: &BTreeMap<String, (Stamp, [u8; 32])>,
    ) -> anyhow::Result<()>;
    /// Lists mode: the element ids a window last saw for a key.
    fn ext_view(&self, window: &str, ext: &str, key: &str) -> anyhow::Result<BTreeSet<String>>;
    fn set_ext_view(
        &mut self,
        window: &str,
        ext: &str,
        key: &str,
        ids: &BTreeSet<String>,
    ) -> anyhow::Result<()>;
    /// Drop a destroyed window's `reads` and `ext_view` rows.
    fn forget_window(&mut self, window: &str) -> anyhow::Result<()>;
    /// Keys mode: the stamp of the value a window last loaded or saved.
    fn read_stamp(&self, window: &str, ext: &str, key: &str) -> anyhow::Result<Option<Stamp>>;
    fn set_read_stamp(
        &mut self,
        window: &str,
        ext: &str,
        key: &str,
        s: Option<Stamp>,
    ) -> anyhow::Result<()>;
}

impl ExtStore for Tx<'_> {
    fn feed_push(&mut self, ext: &str, coll: &str, id: &str) -> anyhow::Result<u64> {
        let _ = (ext, coll, id);
        Ok(0)
    }

    fn feed_page(
        &self,
        ext: &str,
        after: u64,
        max: usize,
    ) -> anyhow::Result<Vec<(u64, String, String)>> {
        let _ = (ext, after, max);
        Ok(Vec::new())
    }

    fn feed_ack(&mut self, ext: &str, upto: u64) -> anyhow::Result<()> {
        let _ = (ext, upto);
        Ok(())
    }

    fn ext_base(
        &self,
        ext: &str,
        coll: &str,
        id: &str,
    ) -> anyhow::Result<BTreeMap<String, (Stamp, [u8; 32])>> {
        let _ = (ext, coll, id);
        Ok(BTreeMap::new())
    }

    fn set_ext_base(
        &mut self,
        ext: &str,
        coll: &str,
        id: &str,
        f: &BTreeMap<String, (Stamp, [u8; 32])>,
    ) -> anyhow::Result<()> {
        let _ = (ext, coll, id, f);
        Ok(())
    }

    fn ext_view(&self, window: &str, ext: &str, key: &str) -> anyhow::Result<BTreeSet<String>> {
        let _ = (window, ext, key);
        Ok(BTreeSet::new())
    }

    fn set_ext_view(
        &mut self,
        window: &str,
        ext: &str,
        key: &str,
        ids: &BTreeSet<String>,
    ) -> anyhow::Result<()> {
        let _ = (window, ext, key, ids);
        Ok(())
    }

    fn forget_window(&mut self, window: &str) -> anyhow::Result<()> {
        let _ = window;
        Ok(())
    }

    fn read_stamp(&self, window: &str, ext: &str, key: &str) -> anyhow::Result<Option<Stamp>> {
        let _ = (window, ext, key);
        Ok(None)
    }

    fn set_read_stamp(
        &mut self,
        window: &str,
        ext: &str,
        key: &str,
        s: Option<Stamp>,
    ) -> anyhow::Result<()> {
        let _ = (window, ext, key, s);
        Ok(())
    }
}
