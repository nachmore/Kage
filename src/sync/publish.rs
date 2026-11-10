//! Publish: write this computer's dirty buckets as content-named shards,
//! then a new head (docs/design/SYNC.md §8.3), and the file formats of
//! §5.5–§5.7.
//!
//! Order of writes: blobs, then shards, then the head. Every file is
//! written once (`SyncFs::write_once`); a reader that sees the head can
//! find everything it names, or waits for it (§8.4).

use super::canonical::canonical;
use super::catalog::{self, CFG_NS};
use super::crypto::Keys;
use super::engine::{Core, SyncEngine};
use super::envelope::{self, Aad, FileKind, MAX_SHARD_PLAIN};
use super::folder::{data_name, head_name, DirKind, FileClass};
use super::replica::Tx;
use super::types::{DeviceId, Record, Stamp, READER_VERSION, SYNC_FORMAT};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io;

/// Values above this go in a blob (§5.6).
pub const BLOB_OVER: usize = 64 * 1024;
/// The average shard a namespace aims for (§5.8).
pub const TARGET_SHARD: usize = 128 * 1024;
/// Most buckets a namespace uses.
pub const MAX_BUCKETS: u32 = 256;
/// The namespaces sync keeps for itself, always published.
pub const SYSTEM_NS: [&str; 3] = ["sync.devices", "sync.conflicts", "sync.ns_state"];

/// One namespace in a head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NsEntry {
    pub schema: u32,
    pub buckets: u32,
    pub shards: Vec<String>,
}

/// The plaintext of a head (§5.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeadFile {
    pub format: u32,
    pub min_reader: u32,
    pub device: String,
    pub instance: String,
    pub seq: u64,
    pub written_at: i64,
    pub max_stamp: Stamp,
    #[serde(default)]
    pub kage_version: String,
    #[serde(default)]
    pub catalog: u32,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub retired: bool,
    #[serde(default)]
    pub gc_floor: Option<Stamp>,
    #[serde(default)]
    pub seen: BTreeMap<String, u64>,
    #[serde(default)]
    pub namespaces: BTreeMap<String, NsEntry>,
}

/// The plaintext of a shard (§5.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShardFile {
    pub ns: String,
    pub schema: u32,
    pub bucket: u32,
    pub of: u32,
    pub records: Vec<Value>,
}

/// The plaintext of a blob (§5.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlobFile {
    pub len: u64,
    pub data: Value,
}

/// A cell value that points at a blob: `{"$blob": "<32 hex>", "len": n}`.
pub fn blob_ref(v: &Value) -> Option<&str> {
    let o = v.as_object()?;
    if o.len() != 2 || !o.contains_key("len") {
        return None;
    }
    let name = o.get("$blob")?.as_str()?;
    (name.len() == 32 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .then_some(name)
}

/// What this computer has published for one namespace (meta `pub.<ns>`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct NsState {
    pub schema: u32,
    pub buckets: u32,
    pub shards: Vec<String>,
    pub sizes: Vec<u64>,
}

pub(crate) fn ns_state(tx: &Tx<'_>, ns: &str) -> anyhow::Result<Option<NsState>> {
    Ok(tx
        .meta(&format!("pub.{ns}"))?
        .and_then(|v| serde_json::from_str(&v).ok()))
}

fn set_ns_state(tx: &mut Tx<'_>, ns: &str, st: &NsState) -> anyhow::Result<()> {
    tx.set_meta(&format!("pub.{ns}"), &serde_json::to_string(st)?)
}

/// A shard written this publish, and the blobs it needs.
struct Pending {
    name: String,
    bytes: Vec<u8>,
}

/// Seal a plaintext as a file of this computer.
pub(crate) fn seal_file(core: &Core, kind: FileKind, name: &str, plain: &[u8]) -> Option<Vec<u8>> {
    let keys = core.cur_keys()?;
    let aad = Aad {
        folder_id: &core.folder_id,
        device: core.me,
        kind,
        name,
    };
    Some(envelope::seal(keys, core.epoch, &aad, plain))
}

/// The shard form of a record, with values over 64 KiB moved to blobs.
fn record_json(
    r: &Record,
    keys: &Keys,
    blobs: &mut BTreeMap<String, Vec<u8>>,
    write_blobs: bool,
) -> Value {
    let mut j = r.to_json();
    if let Some(Value::Object(cells)) = j.get_mut("c") {
        for cell in cells.values_mut() {
            let Some(arr) = cell.as_array_mut() else {
                continue;
            };
            let Some(v) = arr.first() else { continue };
            let c = canonical(v);
            if c.len() <= BLOB_OVER {
                continue;
            }
            let blob = serde_json::json!({ "len": c.len(), "data": v });
            let plain = canonical(&blob);
            let name = keys.content_name(&plain);
            if write_blobs {
                blobs.insert(name.clone(), plain);
            }
            arr[0] = serde_json::json!({ "$blob": name, "len": c.len() });
        }
    }
    j
}

impl SyncEngine {
    /// The namespaces this computer's head lists: every one that syncs
    /// here, sync's own, and opaque ones from a newer Kage.
    pub(crate) fn head_namespaces(&mut self) -> anyhow::Result<BTreeSet<String>> {
        let mut out: BTreeSet<String> = SYSTEM_NS.iter().map(|s| s.to_string()).collect();
        let specs: Vec<String> = self.specs.keys().cloned().collect();
        for ns in specs {
            let on = if ns == CFG_NS {
                self.prefs.categories.settings
                    || self.prefs.categories.automations
                    || self.prefs.categories.extensions
            } else {
                self.syncs_here(&ns, None)
            };
            if on {
                out.insert(ns);
            }
        }
        let Some(core) = self.core.as_mut() else {
            return Ok(out);
        };
        let in_replica = core.replica.tx(|tx| tx.namespaces())?;
        for ns in in_replica {
            if !self.specs.contains_key(&ns) && self.syncs_here(&ns, None) {
                out.insert(ns);
            }
        }
        Ok(out)
    }

    /// Whether a record of `ns` goes into this computer's shards.
    fn publishes(&self, ns: &str, r: &Record, me: DeviceId) -> bool {
        if ns == CFG_NS && !self.syncs_here(ns, Some(&r.id)) {
            return false;
        }
        let echo = self.specs.get(ns).is_none_or(|(_, s)| s.echo);
        if echo {
            return true;
        }
        // Only the origin publishes a chat; another computer publishes only
        // its own changes of it (a rename, a hide).
        let mine = format!("{}:", me);
        r.id.starts_with(&mine)
            || r.cells.values().any(|c| c.stamp.dev == me)
            || r.del.as_ref().is_some_and(|d| d.stamp.dev == me)
    }

    /// Write the dirty buckets and a new head. Returns whether a head was
    /// written.
    pub(crate) fn publish(&mut self, now: i64) -> anyhow::Result<bool> {
        if self.core_locked() {
            return Ok(false);
        }
        let namespaces = self.head_namespaces()?;
        let held = self.outgoing_holds()?;
        let core = self.core.as_ref().expect("checked above");
        let fs = self.env.fs.clone();
        for d in [
            core.layout.shards_dir(core.me),
            core.layout.blobs_dir(core.me),
        ] {
            if let Err(e) = fs.create_dir_all(&d) {
                return self.write_failed(now, e);
            }
        }
        self.publish_now(now, &namespaces, &held)
    }

    /// The publish itself: build every changed shard, write blobs, shards
    /// and the head, then commit what was published.
    fn publish_now(
        &mut self,
        now: i64,
        namespaces: &BTreeSet<String>,
        held: &BTreeSet<String>,
    ) -> anyhow::Result<bool> {
        let schemas: BTreeMap<String, u32> = self
            .specs
            .iter()
            .map(|(ns, (_, s))| (ns.clone(), s.schema))
            .collect();
        let fs = self.env.fs.clone();
        let (me, keys, shards_dir, blobs_dir, dev_dir) = {
            let core = self.core.as_ref().expect("on");
            (
                core.me,
                core.cur_keys().cloned().expect("not locked"),
                core.layout.shards_dir(core.me),
                core.layout.blobs_dir(core.me),
                core.layout.device_dir(core.me),
            )
        };
        // Evicted ones count: the cloud still has them (§8.12).
        let existing_shards: BTreeSet<String> = own_names(&*fs, &shards_dir, DirKind::Shards)
            .into_keys()
            .collect();
        let existing_blobs: BTreeSet<String> = own_names(&*fs, &blobs_dir, DirKind::Blobs)
            .into_keys()
            .collect();
        // Read what to write.
        let mut dirty: Vec<(String, u32, bool)> = Vec::new();
        let mut states: BTreeMap<String, NsState> = BTreeMap::new();
        let mut shards: Vec<Pending> = Vec::new();
        let mut blobs: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut records_by_ns: BTreeMap<String, BTreeMap<u32, Vec<Record>>> = BTreeMap::new();
        {
            let core = self.core.as_mut().expect("on");
            let res = core.replica.tx(|tx| {
                dirty = tx.take_dirty()?;
                let mut by_ns: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
                for (ns, b, _) in &dirty {
                    by_ns.entry(ns.clone()).or_default().insert(*b);
                }
                for ns in namespaces {
                    if held.contains(ns) {
                        if let Some(prev) = ns_state(tx, ns)? {
                            states.insert(ns.clone(), prev);
                        }
                        continue;
                    }
                    let prev = ns_state(tx, ns)?;
                    let hashes = by_ns.get(ns).cloned().unwrap_or_default();
                    let schema = schemas
                        .get(ns)
                        .copied()
                        .or_else(|| prev.as_ref().map(|p| p.schema))
                        .unwrap_or(1)
                        .max(1);
                    let mut st = prev.clone().unwrap_or(NsState {
                        schema,
                        buckets: 1,
                        shards: vec![String::new()],
                        sizes: vec![0],
                    });
                    st.schema = st.schema.max(schema);
                    let mut all = prev.is_none() || st.shards.iter().any(String::is_empty);
                    if !all && hashes.is_empty() {
                        states.insert(ns.clone(), st);
                        continue;
                    }
                    // Grow or shrink the bucket count (§5.6).
                    let total: u64 = st.sizes.iter().sum();
                    let mut b = st.buckets.max(1);
                    let avg = total / u64::from(b);
                    if avg > (TARGET_SHARD as u64 * 3) / 2 && b < MAX_BUCKETS {
                        while b < MAX_BUCKETS && total / u64::from(b) > TARGET_SHARD as u64 {
                            b *= 2;
                        }
                    } else if avg < TARGET_SHARD as u64 / 4 && b > 1 {
                        while b > 1 && total / u64::from(b / 2).max(1) < TARGET_SHARD as u64 {
                            b /= 2;
                        }
                    }
                    if b != st.buckets {
                        st.buckets = b;
                        st.shards = vec![String::new(); b as usize];
                        st.sizes = vec![0; b as usize];
                        all = true;
                    }
                    let todo: BTreeSet<u32> = if all {
                        (0..b).collect()
                    } else {
                        hashes.iter().map(|h| h % b).collect()
                    };
                    let mut per_bucket = BTreeMap::new();
                    for bucket in todo {
                        per_bucket.insert(bucket, tx.records_in_bucket(ns, bucket, b)?);
                    }
                    records_by_ns.insert(ns.clone(), per_bucket);
                    states.insert(ns.clone(), st);
                }
                // Read only: the dirty marks come back unless the publish
                // succeeds.
                anyhow::bail!(PlanOnly)
            });
            match res {
                Err(e) if e.is::<PlanOnly>() => {}
                Err(e) => return Err(e),
                Ok(()) => {}
            }
        }
        // Build the shards.
        let mut grow: BTreeSet<String> = BTreeSet::new();
        for (ns, buckets) in &records_by_ns {
            let st = states.get_mut(ns).expect("planned");
            for (bucket, recs) in buckets {
                let records: Vec<Value> = recs
                    .iter()
                    .filter(|r| self.publishes(ns, r, me))
                    .map(|r| {
                        let origin_here = r.id.starts_with(&format!("{me}:"))
                            || self.specs.get(ns.as_str()).is_none_or(|(_, s)| s.echo);
                        record_json(r, &keys, &mut blobs, origin_here)
                    })
                    .collect();
                let shard = ShardFile {
                    ns: ns.clone(),
                    schema: st.schema,
                    bucket: *bucket,
                    of: st.buckets,
                    records,
                };
                let plain = canonical(&serde_json::to_value(&shard)?);
                if plain.len() > MAX_SHARD_PLAIN && st.buckets < MAX_BUCKETS {
                    grow.insert(ns.clone());
                }
                let name = keys.content_name(&plain);
                st.shards[*bucket as usize] = name.clone();
                st.sizes[*bucket as usize] = plain.len() as u64;
                if !existing_shards.contains(&name) {
                    let core = self.core.as_ref().expect("on");
                    let file = data_name(&name);
                    let sealed = super::publish::seal_file(core, FileKind::Shard, &file, &plain)
                        .ok_or_else(|| anyhow::anyhow!("locked"))?;
                    shards.push(Pending {
                        name: file,
                        bytes: sealed,
                    });
                }
            }
        }
        if !grow.is_empty() {
            // A shard over its limit: publish that namespace again with
            // twice the buckets.
            let core = self.core.as_mut().expect("on");
            core.replica.tx(|tx| {
                for ns in &grow {
                    if let Some(mut st) = ns_state(tx, ns)? {
                        let b = (st.buckets * 2).min(MAX_BUCKETS);
                        st.buckets = b;
                        st.shards = vec![String::new(); b as usize];
                        st.sizes = vec![MAX_SHARD_PLAIN as u64; b as usize];
                        set_ns_state(tx, ns, &st)?;
                    } else {
                        set_ns_state(
                            tx,
                            ns,
                            &NsState {
                                schema: 1,
                                buckets: 2,
                                shards: vec![String::new(); 2],
                                sizes: vec![MAX_SHARD_PLAIN as u64; 2],
                            },
                        )?;
                    }
                }
                Ok(())
            })?;
            return self.publish_now(now, namespaces, held);
        }
        // Write blobs, then shards.
        let core = self.core.as_ref().expect("on");
        for (name, plain) in &blobs {
            if existing_blobs.contains(name) {
                continue;
            }
            let file = data_name(name);
            let sealed = seal_file(core, FileKind::Blob, &file, plain)
                .ok_or_else(|| anyhow::anyhow!("locked"))?;
            if let Err(e) = write_once_retrying(&*fs, &blobs_dir, &file, &sealed) {
                return self.write_failed(now, e);
            }
            super::engine::kill::point("after_blob_write");
        }
        for p in &shards {
            if let Err(e) = write_once_retrying(&*fs, &shards_dir, &p.name, &p.bytes) {
                return self.write_failed(now, e);
            }
            super::engine::kill::point("after_shard_write");
        }
        super::engine::kill::point("before_head_write");
        // The head. One that would say nothing new isn't written.
        let (head, seq) = self.build_head(now, &states, false)?;
        let sig = head_signature(&head);
        let first_of_run = self.rt.last_head_sig.is_none();
        if records_by_ns.is_empty()
            && !first_of_run
            && self.rt.last_head_sig.as_deref() == Some(sig.as_str())
            && self.core.as_ref().is_some_and(|c| c.my_seq > 0)
        {
            self.rt.last_publish_ms = Some(now);
            self.rt.first_own_ms = None;
            self.rt.last_own_ms = None;
            return Ok(false);
        }
        let plain = canonical(&serde_json::to_value(&head)?);
        let core = self.core.as_ref().expect("on");
        let mut seq = seq;
        loop {
            let file = head_name(seq);
            let mut h = head.clone();
            h.seq = seq;
            let plain_seq = if seq == head.seq {
                plain.clone()
            } else {
                canonical(&serde_json::to_value(&h)?)
            };
            let sealed = seal_file(core, FileKind::Head, &file, &plain_seq)
                .ok_or_else(|| anyhow::anyhow!("locked"))?;
            match write_once_retrying(&*fs, &dev_dir, &file, &sealed) {
                Ok(()) => break,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && seq < head.seq + 8 => {
                    // A head with this number is there already (a restore
                    // or a clone wrote it): take the next.
                    seq += 1;
                }
                Err(e) => return self.write_failed(now, e),
            }
        }
        // Commit what was published.
        let refs: Vec<String> = states
            .values()
            .flat_map(|s| s.shards.clone())
            .chain(blobs.keys().cloned())
            .collect();
        let written: BTreeSet<String> = records_by_ns.keys().cloned().collect();
        let core = self.core.as_mut().expect("on");
        core.replica.tx(|tx| {
            // What wasn't published (held, or not synced here now) stays
            // dirty for a later publish.
            for (ns, b, own) in tx.take_dirty()? {
                if !written.contains(&ns) {
                    tx.mark_dirty(&ns, b, own)?;
                }
            }
            for (ns, st) in &states {
                set_ns_state(tx, ns, st)?;
                if written.contains(ns) {
                    tx.mark_published(ns)?;
                }
            }
            tx.set_meta("my_seq", &seq.to_string())?;
            let mut heads: BTreeMap<u64, Vec<String>> = tx
                .meta("heads_refs")?
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default();
            heads.insert(seq, refs.clone());
            while heads.len() > 3 {
                let first = *heads.keys().next().expect("non-empty");
                heads.remove(&first);
            }
            tx.set_meta("heads_refs", &serde_json::to_string(&heads)?)?;
            Ok(())
        })?;
        core.my_seq = seq;
        let _ = dirty;
        self.rt.last_head_sig = Some(sig);
        self.rt.last_publish_ms = Some(now);
        self.rt.first_own_ms = None;
        self.rt.last_own_ms = None;
        self.rt.write_backoff = None;
        self.rt.write_failed = false;
        self.rt.disk_full = false;
        log::info!("sync: published head {seq}");
        Ok(true)
    }

    /// This computer's head for the published `states`.
    pub(crate) fn build_head(
        &mut self,
        now: i64,
        states: &BTreeMap<String, NsState>,
        retired: bool,
    ) -> anyhow::Result<(HeadFile, u64)> {
        let name = self.prefs.device_name.clone().unwrap_or_default();
        let os = self.env.os.as_str().to_string();
        let version = self.env.kage_version.clone();
        let core = self.core.as_mut().expect("on");
        let me = core.me;
        let (seen, gc_floor) = core.replica.tx(|tx| {
            let seen: BTreeMap<String, u64> = tx
                .devices()?
                .into_iter()
                .filter(|d| d.ingested_seq > 0)
                .map(|d| (d.dev.to_string(), d.ingested_seq))
                .collect();
            let floor = tx.meta("gc_floor")?.and_then(|s| s.parse::<Stamp>().ok());
            Ok((seen, floor))
        })?;
        let seq = core.my_seq + 1;
        let head = HeadFile {
            format: SYNC_FORMAT,
            min_reader: READER_VERSION,
            device: me.to_string(),
            instance: core.instance.clone(),
            seq,
            written_at: now,
            max_stamp: Stamp::new(core.hlc.last(), me),
            kage_version: version,
            catalog: catalog::CATALOG_VERSION,
            os,
            name,
            retired,
            gc_floor,
            seen,
            namespaces: states
                .iter()
                .filter(|(_, s)| !s.shards.iter().any(String::is_empty))
                .map(|(ns, s)| {
                    (
                        ns.clone(),
                        NsEntry {
                            schema: s.schema,
                            buckets: s.buckets,
                            shards: s.shards.clone(),
                        },
                    )
                })
                .collect(),
        };
        Ok((head, seq))
    }

    /// A write to the folder failed: keep everything for later, back off
    /// (5 s, 30 s, 5 min, then hourly) and say so.
    fn write_failed(&mut self, now: i64, e: io::Error) -> anyhow::Result<bool> {
        log::warn!("sync: couldn't write to the sync folder: {e}");
        if super::fsio::is_unresponsive(&e) {
            self.rt.io_paused_until = Some(now + 10 * 60_000);
        }
        let n = self.rt.write_backoff.map_or(0, |(_, n)| n + 1);
        let wait = match n {
            0 => 5_000,
            1 => 30_000,
            2 => 300_000,
            _ => 3_600_000,
        };
        self.rt.write_backoff = Some((now + wait, n));
        self.rt.write_failed = true;
        if e.kind() == io::ErrorKind::StorageFull {
            self.rt.disk_full = true;
        }
        Ok(false)
    }
}

/// What a head says, apart from its number, time, clock and instance: two
/// heads with the same signature carry the same state.
fn head_signature(h: &HeadFile) -> String {
    let v = serde_json::json!({
        "seen": h.seen,
        "namespaces": h.namespaces,
        "gc_floor": h.gc_floor,
        "name": h.name,
        "os": h.os,
        "kage_version": h.kage_version,
        "catalog": h.catalog,
        "retired": h.retired,
    });
    String::from_utf8(canonical(&v)).unwrap_or_default()
}

/// The namespaces held back by an outgoing mass-change question.
impl SyncEngine {
    fn outgoing_holds(&mut self) -> anyhow::Result<BTreeSet<String>> {
        let Some(core) = self.core.as_mut() else {
            return Ok(BTreeSet::new());
        };
        core.replica.tx(|tx| {
            Ok(super::guards::holds(tx)?
                .into_iter()
                .filter(|(_, h)| h.outgoing && !h.accepted)
                .map(|(ns, _)| ns)
                .collect())
        })
    }
}

/// A transaction run only to read: its error rolls it back.
#[derive(Debug)]
struct PlanOnly;

impl std::fmt::Display for PlanOnly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("plan only")
    }
}

impl std::error::Error for PlanOnly {}

fn classify_is_data(kind: DirKind, name: &str) -> bool {
    matches!(super::folder::classify(kind, name), FileClass::Data)
}

/// This computer's own data files in `dir`: content name → the file's name
/// on disk. An older macOS evicts iCloud files to `.<name>.icloud` stubs;
/// Kage never needs its own files back (§8.12), so an evicted file still
/// counts as written, and its stub is what gc removes.
pub(crate) fn own_names(
    fs: &dyn super::fsio::SyncFs,
    dir: &std::path::Path,
    kind: DirKind,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for e in fs.list(dir).unwrap_or_default() {
        if e.is_dir {
            continue;
        }
        let data = match super::folder::classify(kind, &e.name) {
            FileClass::Data => e.name.clone(),
            FileClass::ICloudStub(inner) if classify_is_data(kind, &inner) => inner,
            _ => continue,
        };
        if let Some(stem) = data.strip_suffix(".kage") {
            // A real file wins over a stub of the same name.
            if data == e.name || !out.contains_key(stem) {
                out.insert(stem.to_string(), e.name.clone());
            }
        }
    }
    out
}

/// This computer's own heads in its directory: `seq` → the file's name on
/// disk, evicted ones included (see [`own_names`]).
pub(crate) fn own_heads(
    fs: &dyn super::fsio::SyncFs,
    dir: &std::path::Path,
) -> Vec<(u64, String, Option<i64>)> {
    let mut out: BTreeMap<u64, (String, Option<i64>)> = BTreeMap::new();
    for e in fs.list(dir).unwrap_or_default() {
        if e.is_dir {
            continue;
        }
        let (seq, real) = match super::folder::classify(DirKind::Device, &e.name) {
            FileClass::Head(s) => (s, true),
            FileClass::ICloudStub(inner) => {
                match super::folder::classify(DirKind::Device, &inner) {
                    FileClass::Head(s) => (s, false),
                    _ => continue,
                }
            }
            _ => continue,
        };
        if real || !out.contains_key(&seq) {
            out.insert(seq, (e.name.clone(), e.modified_ms));
        }
    }
    out.into_iter().map(|(s, (n, m))| (s, n, m)).collect()
}

/// `write_once`, retrying a Windows sharing violation (OneDrive,
/// antivirus) for a moment. A file that is there already counts as
/// written for content-named files; the caller decides for heads.
pub(crate) fn write_once_retrying(
    fs: &dyn super::fsio::SyncFs,
    dir: &std::path::Path,
    name: &str,
    bytes: &[u8],
) -> io::Result<()> {
    let mut wait = std::time::Duration::from_millis(50);
    let mut total = std::time::Duration::ZERO;
    loop {
        match fs.write_once(dir, name, bytes) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && !name.starts_with("head-") => {
                return Ok(())
            }
            Err(e)
                if super::fsio::is_sharing_violation(&e)
                    && total < std::time::Duration::from_secs(5) =>
            {
                std::thread::sleep(wait);
                total += wait;
                wait = (wait * 2).min(std::time::Duration::from_secs(1));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Read a record's blob references back into values, from `find`.
pub(crate) fn resolve_blobs(rec: &mut Value, find: &mut dyn FnMut(&str) -> Option<Value>) -> bool {
    let Some(Value::Object(cells)) = rec.get_mut("c") else {
        return true;
    };
    for cell in cells.values_mut() {
        let Some(arr) = cell.as_array_mut() else {
            continue;
        };
        let Some(name) = arr.first().and_then(blob_ref).map(str::to_string) else {
            continue;
        };
        match find(&name) {
            Some(v) => arr[0] = v,
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn blob_refs_are_strict() {
        let name = "0123456789abcdef0123456789abcdef";
        assert_eq!(blob_ref(&json!({ "$blob": name, "len": 5 })), Some(name));
        assert!(blob_ref(&json!({ "$blob": name })).is_none());
        assert!(blob_ref(&json!({ "$blob": "XYZ", "len": 5 })).is_none());
        assert!(blob_ref(&json!({ "$blob": name, "len": 5, "x": 1 })).is_none());
    }

    #[test]
    fn large_values_move_to_blobs_and_come_back() {
        let keys = Keys::derive(&[7; 32]);
        let d = DeviceId([1; 10]);
        let mut r = Record::new("doc.steering", "auto");
        let big = "x".repeat(BLOB_OVER + 10);
        r.cells.insert(
            super::super::types::FieldKey::all("text"),
            super::super::types::Cell {
                value: Value::String(big.clone()),
                stamp: Stamp::new(1, d),
                wall_ms: 0,
                anc: vec![],
            },
        );
        let mut blobs = BTreeMap::new();
        let mut j = record_json(&r, &keys, &mut blobs, true);
        assert_eq!(blobs.len(), 1);
        let stored: BTreeMap<String, Value> = blobs
            .iter()
            .map(|(k, v)| {
                let b: BlobFile = serde_json::from_slice(v).unwrap();
                (k.clone(), b.data)
            })
            .collect();
        assert!(resolve_blobs(&mut j, &mut |n| stored.get(n).cloned()));
        assert_eq!(Record::from_json("doc.steering", &j).unwrap(), r);
        let mut j2 = record_json(&r, &keys, &mut BTreeMap::new(), false);
        assert!(!resolve_blobs(&mut j2, &mut |_| None));
    }
}
