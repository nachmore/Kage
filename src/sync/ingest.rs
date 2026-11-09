//! Ingest: read other computers' heads, shards and blobs from the folder
//! and merge them, one shard per transaction (docs/design/SYNC.md §8.4).
//!
//! - Only the newest readable head of each computer counts, and only when
//!   its `seq` is above what was fully ingested before (a replayed old head
//!   is ignored).
//! - A file that is a placeholder is asked to download and skipped; one
//!   that doesn't open is "waiting" for 10 minutes, then "unreadable". A
//!   head whose shards haven't all arrived is merged as far as it goes and
//!   finished later.
//! - A shard is merged in one transaction together with its `applied`
//!   row, so a crash never leaves half a shard merged.

use super::catalog::{self, CFG_NS};
use super::conflicts;
use super::engine::{SyncEngine, TickReport, DAY_MS, NOT_READY_MS};
use super::envelope::{self, Aad, FileKind, OpenError};
use super::folder::{data_name, DirKind, FileClass};
use super::fsio::{Entry, READ_TIMEOUT};
use super::publish::{resolve_blobs, BlobFile, HeadFile, ShardFile};
use super::replica::{DeviceRow, Tx};
use super::status::{Problem, SyncState};
use super::types::{DeviceId, OsKind, Record, Stamp, READER_VERSION};
use crate::os::cloud_files::Residency;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

/// A remote stamp this far ahead of the wall clock is worth a warning.
const CLOCK_WARN_MS: i64 = 5 * 60_000;

/// Why a file couldn't be used this time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotYet {
    /// Downloading, being written, or a placeholder: try again later.
    Waiting,
    /// Not there.
    Missing,
    /// Still not readable after 10 minutes.
    Unreadable,
    /// Sealed with a key epoch this computer doesn't have yet.
    NewerKey(u32),
}

/// What one ingest pass counted.
#[derive(Default)]
pub(crate) struct IngestStats {
    pub waiting: u32,
    pub placeholders: bool,
    pub unreadable: BTreeMap<DeviceId, u32>,
    pub used_bytes: u64,
    pub newer_key: Option<u32>,
}

impl SyncEngine {
    /// Read every other computer's newest head and merge what it names.
    /// `force` reads every head and shard again, ignoring what was applied
    /// (a rebuild). Returns the records that changed.
    pub(crate) fn ingest(
        &mut self,
        now: i64,
        rep: &mut TickReport,
        force: bool,
    ) -> anyhow::Result<BTreeSet<(String, String)>> {
        let mut changed = BTreeSet::new();
        let mut stats = IngestStats::default();
        let Some(core) = self.core.as_ref() else {
            return Ok(changed);
        };
        let devices_dir = core.layout.devices_dir();
        let me = core.me;
        let fs = self.env.fs.clone();
        let entries = match fs.list(&devices_dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                log::info!("sync: couldn't list the sync folder's computers: {e}");
                return Ok(changed);
            }
        };
        for e in entries.iter().filter(|e| e.is_dir) {
            let Some(dev) = super::crypto::parse_device_text(&e.name) else {
                continue;
            };
            if dev == me {
                continue;
            }
            self.ingest_device(dev, now, force, rep, &mut changed, &mut stats)?;
        }
        self.rt.waiting_files = stats.waiting;
        if stats.placeholders {
            self.rt.placeholder_ticks += 1;
        } else {
            self.rt.placeholder_ticks = 0;
        }
        self.rt.unreadable = stats.unreadable;
        self.rt.used_bytes = stats.used_bytes;
        if let Some(e) = stats.newer_key {
            if self.core.as_ref().is_some_and(|c| e > c.epoch) {
                self.rt.newer_epoch = Some(e);
            }
        }
        self.rt.last_ingest_ms = Some(now);
        self.after_ingest(&changed)?;
        if stats.waiting == 0 {
            if let Some(core) = self.core.as_mut() {
                core.replica
                    .tx(|tx| tx.set_meta("last_complete_ingest", &now.to_string()))?;
            }
        }
        Ok(changed)
    }

    /// One computer's newest readable head.
    fn ingest_device(
        &mut self,
        dev: DeviceId,
        now: i64,
        force: bool,
        rep: &mut TickReport,
        changed: &mut BTreeSet<(String, String)>,
        stats: &mut IngestStats,
    ) -> anyhow::Result<()> {
        let core = self.core.as_ref().expect("on");
        let dir = core.layout.device_dir(dev);
        let fs = self.env.fs.clone();
        let entries = match fs.list(&dir) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };
        stats.used_bytes += entries.iter().map(|e| e.len).sum::<u64>();
        let mut heads: Vec<(u64, &Entry, bool)> = Vec::new();
        for e in entries.iter().filter(|e| !e.is_dir) {
            match super::folder::classify(DirKind::Device, &e.name) {
                FileClass::Head(seq) => heads.push((seq, e, false)),
                FileClass::ICloudStub(inner) => {
                    if let FileClass::Head(seq) = super::folder::classify(DirKind::Device, &inner) {
                        heads.push((seq, e, true));
                    }
                }
                _ => {}
            }
        }
        heads.sort_by(|a, b| b.0.cmp(&a.0));
        let row = {
            let core = self.core.as_mut().expect("on");
            core.replica.tx(|tx| tx.device(dev))?
        };
        let done = row.as_ref().map_or(0, |r| r.ingested_seq);
        for (seq, entry, stub) in heads {
            if seq <= done && !force {
                break;
            }
            let path = dir.join(super::folder::head_name(seq));
            if stub {
                self.env.cloud.request_download(&path);
                stats.waiting += 1;
                stats.placeholders = true;
                continue;
            }
            let plain = match self.read_open(&path, FileKind::Head, dev, entry.modified_ms, now) {
                Ok(p) => p,
                Err(NotYet::NewerKey(e)) => {
                    stats.newer_key = Some(stats.newer_key.map_or(e, |x| x.max(e)));
                    continue;
                }
                Err(NotYet::Waiting) => {
                    stats.waiting += 1;
                    if self.env.cloud.residency(&path) == Residency::Placeholder {
                        stats.placeholders = true;
                    }
                    continue;
                }
                Err(NotYet::Unreadable) => {
                    *stats.unreadable.entry(dev).or_default() += 1;
                    continue;
                }
                Err(NotYet::Missing) => continue,
            };
            let Ok(head) = serde_json::from_slice::<HeadFile>(&plain) else {
                *stats.unreadable.entry(dev).or_default() += 1;
                continue;
            };
            if head.device != dev.to_string() || head.seq != seq {
                // Authentic, but not this file: ignore it.
                continue;
            }
            if head.min_reader > READER_VERSION {
                self.note_device(dev, &head, row.as_ref(), false, "needs_update")?;
                break;
            }
            let complete = self.ingest_head(dev, &head, now, force, rep, changed, stats)?;
            let floor = super::hlc::physical(now);
            let _ = floor;
            if let Some(core) = self.core.as_mut() {
                core.hlc.observe(head.max_stamp.hlc, now);
            }
            self.clock_check(dev, &head, now);
            let state = if head.retired { "retired" } else { "ok" };
            self.note_device(dev, &head, row.as_ref(), complete, state)?;
            break;
        }
        Ok(())
    }

    /// Merge every shard a head names that isn't merged yet. Returns
    /// whether the head is complete here.
    #[allow(clippy::too_many_arguments)]
    fn ingest_head(
        &mut self,
        dev: DeviceId,
        head: &HeadFile,
        now: i64,
        force: bool,
        rep: &mut TickReport,
        changed: &mut BTreeSet<(String, String)>,
        stats: &mut IngestStats,
    ) -> anyhow::Result<bool> {
        let mut complete = true;
        let core = self.core.as_ref().expect("on");
        let shards_dir = core.layout.shards_dir(dev);
        let fs = self.env.fs.clone();
        let listing: BTreeMap<String, Entry> = fs
            .list(&shards_dir)
            .map(|v| v.into_iter().map(|e| (e.name.clone(), e)).collect())
            .unwrap_or_default();
        stats.used_bytes += listing.values().map(|e| e.len).sum::<u64>();
        for (ns, entry) in &head.namespaces {
            if !self.syncs_here(ns, None) && ns != CFG_NS {
                continue;
            }
            if ns == CFG_NS
                && !(self.prefs.categories.settings
                    || self.prefs.categories.automations
                    || self.prefs.categories.extensions)
            {
                continue;
            }
            let known = self.specs.get(ns).map(|(_, s)| s.schema);
            if let Some(mine) = known {
                if entry.schema > mine {
                    let core = self.core.as_mut().expect("on");
                    core.replica.tx(|tx| {
                        let seen = tx
                            .meta(&format!("schema.{ns}"))?
                            .and_then(|v| v.parse::<u32>().ok())
                            .unwrap_or(0);
                        if entry.schema > seen {
                            tx.set_meta(&format!("schema.{ns}"), &entry.schema.to_string())?;
                        }
                        Ok(())
                    })?;
                }
            }
            for name in &entry.shards {
                let applied = {
                    let core = self.core.as_mut().expect("on");
                    core.replica.tx(|tx| tx.applied(name))?
                };
                if applied && !force {
                    continue;
                }
                let file = data_name(name);
                let path = shards_dir.join(&file);
                let modified = listing.get(&file).and_then(|e| e.modified_ms);
                if !listing.contains_key(&file) {
                    // A placeholder stub, or not here yet.
                    let stub = format!(".{file}.icloud");
                    if listing.contains_key(&stub) {
                        self.env.cloud.request_download(&path);
                        stats.placeholders = true;
                    }
                    stats.waiting += 1;
                    complete = false;
                    continue;
                }
                let plain = match self.read_open(&path, FileKind::Shard, dev, modified, now) {
                    Ok(p) => p,
                    Err(NotYet::Unreadable) => {
                        *stats.unreadable.entry(dev).or_default() += 1;
                        complete = false;
                        continue;
                    }
                    Err(NotYet::NewerKey(e)) => {
                        stats.newer_key = Some(e);
                        complete = false;
                        continue;
                    }
                    Err(_) => {
                        stats.waiting += 1;
                        complete = false;
                        continue;
                    }
                };
                let Ok(shard) = serde_json::from_slice::<ShardFile>(&plain) else {
                    *stats.unreadable.entry(dev).or_default() += 1;
                    complete = false;
                    continue;
                };
                if shard.ns != *ns {
                    complete = false;
                    continue;
                }
                let mut records = shard.records;
                let mut all_blobs = true;
                for r in records.iter_mut() {
                    let ok = resolve_blobs(r, &mut |blob| self.find_blob(dev, blob, now));
                    all_blobs &= ok;
                }
                if !all_blobs {
                    stats.waiting += 1;
                    complete = false;
                    continue;
                }
                let n = self.merge_shard(ns, name, &records, changed, dev)?;
                rep.new_conflicts += n;
                rep.ingested_shards += 1;
            }
        }
        Ok(complete)
    }

    /// Merge one shard's records and mark it applied, in one transaction.
    fn merge_shard(
        &mut self,
        ns: &str,
        name: &str,
        records: &[serde_json::Value],
        changed: &mut BTreeSet<(String, String)>,
        from: DeviceId,
    ) -> anyhow::Result<usize> {
        let core = self.core.as_mut().expect("on");
        let keys = core.cur_keys().cloned();
        let Some(keys) = keys else {
            return Ok(0);
        };
        let name_key = *keys.name_key();
        let my_seq = core.my_seq;
        let bucket = move |ns: &str, id: &str| keys.bucket(ns, id, 1 << 16);
        let mut found = 0;
        let mut touched = Vec::new();
        core.replica.tx(|tx| {
            for v in records {
                let rec = match Record::from_json(ns, v) {
                    Ok(r) => r,
                    Err(e) => {
                        log::warn!("sync: skipped a record of {ns} that doesn't read: {e}");
                        continue;
                    }
                };
                for rec in renamed(rec) {
                    let b = tx
                        .bucket_of(&rec.ns, &rec.id)?
                        .unwrap_or_else(|| bucket(&rec.ns, &rec.id));
                    let had_del = tx.record(&rec.ns, &rec.id)?.and_then(|r| r.del);
                    let out = tx.merge(&rec, b)?;
                    if out.changed {
                        touched.push((rec.ns.clone(), rec.id.clone()));
                        let now_del = tx.record(&rec.ns, &rec.id)?.and_then(|r| r.del);
                        if now_del.is_some() && now_del != had_del {
                            tx.set_meta(&tseq_key(&rec.ns, &rec.id), &(my_seq + 1).to_string())?;
                            tx.clear_acks(&rec.ns, &rec.id)?;
                        }
                    }
                    if rec.del.is_some() {
                        tx.ack(&rec.ns, &rec.id, from)?;
                    }
                    found += conflicts::record_detected(tx, &name_key, &out.conflicts, &bucket)?;
                }
                super::engine::kill::point("mid_merge_txn");
            }
            tx.add_applied(name)?;
            Ok(())
        })?;
        changed.extend(touched);
        Ok(found)
    }

    /// A blob, from the shard's computer first, then any other.
    fn find_blob(&self, first: DeviceId, name: &str, now: i64) -> Option<serde_json::Value> {
        let core = self.core.as_ref()?;
        let mut devs = vec![first];
        if let Ok(entries) = self.env.fs.list(&core.layout.devices_dir()) {
            devs.extend(
                entries
                    .iter()
                    .filter(|e| e.is_dir)
                    .filter_map(|e| super::crypto::parse_device_text(&e.name))
                    .filter(|d| *d != first),
            );
        }
        for dev in devs {
            let core = self.core.as_ref()?;
            let path = core.layout.blobs_dir(dev).join(data_name(name));
            if let Ok(plain) = self.read_open(&path, FileKind::Blob, dev, None, now) {
                if let Ok(b) = serde_json::from_slice::<BlobFile>(&plain) {
                    return Some(b.data);
                }
            }
        }
        None
    }

    /// Read and open one sealed file of computer `dev`.
    pub(crate) fn read_open(
        &self,
        path: &Path,
        kind: FileKind,
        dev: DeviceId,
        modified_ms: Option<i64>,
        now: i64,
    ) -> Result<Vec<u8>, NotYet> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        self.read_open_as(path, &name, kind, dev, modified_ms, now)
    }

    /// [`Self::read_open`] for a file a sync app copied under another
    /// name: it opens under the name it was sealed with.
    pub(crate) fn read_open_as(
        &self,
        path: &Path,
        name: &str,
        kind: FileKind,
        dev: DeviceId,
        modified_ms: Option<i64>,
        now: i64,
    ) -> Result<Vec<u8>, NotYet> {
        let core = self.core.as_ref().ok_or(NotYet::Waiting)?;
        if self.env.cloud.residency(path) == Residency::Placeholder {
            self.env.cloud.request_download(path);
            return Err(NotYet::Waiting);
        }
        let bytes = match self
            .env
            .fs
            .read(path, envelope::max_sealed(kind), READ_TIMEOUT)
        {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(NotYet::Missing),
            Err(_) => return Err(NotYet::Waiting),
        };
        let aad = Aad {
            folder_id: &core.folder_id,
            device: dev,
            kind,
            name,
        };
        let keys = |e: u32| core.keys.get(&e).cloned();
        match envelope::open(&keys, &aad, &bytes) {
            Ok(p) => Ok(p),
            Err(OpenError::UnknownEpoch(e)) => Err(NotYet::NewerKey(e)),
            Err(err) => {
                let age = modified_ms.map_or(0, |m| now - m);
                if age >= NOT_READY_MS {
                    log::warn!("sync: a file of computer {dev} can't be used: {err}");
                    Err(NotYet::Unreadable)
                } else {
                    Err(NotYet::Waiting)
                }
            }
        }
    }

    /// Remember what a computer's head said.
    fn note_device(
        &mut self,
        dev: DeviceId,
        head: &HeadFile,
        prev: Option<&DeviceRow>,
        complete: bool,
        state: &str,
    ) -> anyhow::Result<()> {
        let me = self.core.as_ref().expect("on").me;
        let mut row = prev.cloned().unwrap_or(DeviceRow {
            dev,
            ..DeviceRow::default()
        });
        row.name = head.name.clone();
        row.os = OsKind::from_name(&head.os);
        row.kage_version = head.kage_version.clone();
        row.head_seq = row.head_seq.max(head.seq);
        if complete {
            row.ingested_seq = head.seq;
        }
        row.last_seen_ms = head.written_at;
        row.instance = head.instance.clone();
        row.min_reader = head.min_reader;
        row.gc_floor = head.gc_floor;
        row.state = state.to_string();
        let seen_me = head.seen.get(&me.to_string()).copied().unwrap_or(0);
        let nss: Vec<&String> = head.namespaces.keys().collect();
        let nss = serde_json::to_string(&nss)?;
        let catalog = head.catalog;
        let core = self.core.as_mut().expect("on");
        core.replica.tx(|tx| {
            tx.put_device(&row)?;
            tx.set_meta(&format!("seen.{dev}"), &seen_me.to_string())?;
            tx.set_meta(&format!("nss.{dev}"), &nss)?;
            tx.set_meta(&format!("catalog.{dev}"), &catalog.to_string())?;
            Ok(())
        })
    }

    /// Warn once a day when a computer's clock runs ahead (§8.12).
    fn clock_check(&mut self, dev: DeviceId, head: &HeadFile, now: i64) {
        let ahead = super::hlc::wall_ms_of(head.max_stamp.hlc) - now;
        if ahead <= CLOCK_WARN_MS {
            return;
        }
        let last = self.rt.clock_warned.get(&dev).copied();
        if last.is_some_and(|t| now - t < DAY_MS) {
            return;
        }
        self.rt.clock_warned.insert(dev, now);
        let mut params = BTreeMap::new();
        params.insert("computer".to_string(), head.name.clone());
        params.insert("minutes".to_string(), (ahead / 60_000).to_string());
        self.rt.problems.retain(|p| {
            !(p.kind == "clock_warning" && p.params.get("computer") == Some(&head.name))
        });
        self.rt.problems.push(Problem {
            kind: "clock_warning".to_string(),
            key: "sync.problem.clock_warning".to_string(),
            params,
        });
        log::warn!(
            "sync: computer {dev}'s clock is about {} minutes ahead",
            ahead / 60_000
        );
    }

    /// After merging: purges, a removal of this computer, and records that
    /// changed in sync's own bookkeeping.
    fn after_ingest(&mut self, changed: &BTreeSet<(String, String)>) -> anyhow::Result<()> {
        let me = self.core.as_ref().map(|c| c.me);
        let Some(me) = me else { return Ok(()) };
        let mut purges: Vec<(String, Stamp)> = Vec::new();
        let mut removed = false;
        {
            let core = self.core.as_mut().expect("on");
            core.replica.tx(|tx| {
                for (ns, id) in changed {
                    if ns == "sync.devices" && *id == me.to_string() {
                        if let Some(r) = tx.record(ns, id)? {
                            removed |= r
                                .cell("revoked", super::types::Scope::All)
                                .is_some_and(|c| c.value == serde_json::Value::Bool(true));
                        }
                    }
                    if ns == "sync.ns_state" {
                        if let Some(r) = tx.record(ns, id)? {
                            if let Some(c) = r.cell("purged", super::types::Scope::All) {
                                if let Some(s) =
                                    c.value.as_str().and_then(|s| s.parse::<Stamp>().ok())
                                {
                                    purges.push((id.clone(), s));
                                }
                            }
                        }
                    }
                }
                Ok(())
            })?;
        }
        if removed {
            log::warn!("sync: this computer was removed from sync on another computer");
            self.rt.blocked = Some(SyncState::Removed);
            self.env
                .events
                .track("sync_disabled", vec![("reason", "removed".to_string())]);
        }
        for (target, stamp) in purges {
            self.apply_purge(&target, stamp)?;
        }
        Ok(())
    }

    /// Drop the cells of a purged extension's data at or below `stamp`,
    /// and turn its data sync off here (§8.7). Local files are untouched.
    pub(crate) fn apply_purge(&mut self, target: &str, stamp: Stamp) -> anyhow::Result<()> {
        let ext = target.strip_prefix("ext.").unwrap_or(target).to_string();
        let prefix = format!("ext.{ext}.");
        let core = self.core.as_mut().expect("on");
        core.replica.tx(|tx| {
            for ns in tx.namespaces()? {
                if !ns.starts_with(&prefix) {
                    continue;
                }
                for mut r in tx.records(&ns)? {
                    let before = r.cells.len();
                    r.cells.retain(|_, c| c.stamp > stamp);
                    if r.del.as_ref().is_some_and(|d| d.stamp <= stamp) {
                        r.del = None;
                    }
                    if r.cells.is_empty() && r.del.is_none() {
                        let b = tx.bucket_of(&ns, &r.id)?.unwrap_or(0);
                        tx.delete_record(&ns, &r.id)?;
                        tx.mark_dirty(&ns, b, false)?;
                    } else if r.cells.len() != before {
                        let b = tx.bucket_of(&ns, &r.id)?.unwrap_or(0);
                        tx.put_record(&r, b)?;
                        tx.mark_dirty(&ns, b, false)?;
                    }
                }
                tx.feed_clear(&ext)?;
            }
            Ok(())
        })?;
        if !self.prefs.ext_data_is_off(&ext) {
            self.prefs.ext_data_off.push(ext.clone());
            self.save_prefs()?;
            self.env.events.extension_sync_changes(&ext, &[], "purged");
        }
        Ok(())
    }

    /// The ids every active computer's newest head holds, for the
    /// coming-back rule (§8.7). Reads every shard again.
    pub(crate) fn ids_held_elsewhere(
        &mut self,
        active: &[DeviceId],
        now: i64,
    ) -> anyhow::Result<Option<BTreeSet<(String, String)>>> {
        let mut out = BTreeSet::new();
        for dev in active {
            let core = self.core.as_ref().expect("on");
            let dir = core.layout.device_dir(*dev);
            let Ok(entries) = self.env.fs.list(&dir) else {
                return Ok(None);
            };
            let Some(seq) = entries
                .iter()
                .filter_map(
                    |e| match super::folder::classify(DirKind::Device, &e.name) {
                        FileClass::Head(s) => Some(s),
                        _ => None,
                    },
                )
                .max()
            else {
                continue;
            };
            let path = dir.join(super::folder::head_name(seq));
            let Ok(plain) = self.read_open(&path, FileKind::Head, *dev, None, now) else {
                return Ok(None);
            };
            let Ok(head) = serde_json::from_slice::<HeadFile>(&plain) else {
                return Ok(None);
            };
            for (ns, entry) in &head.namespaces {
                for name in &entry.shards {
                    let core = self.core.as_ref().expect("on");
                    let p: PathBuf = core.layout.shards_dir(*dev).join(data_name(name));
                    let Ok(plain) = self.read_open(&p, FileKind::Shard, *dev, None, now) else {
                        return Ok(None);
                    };
                    let Ok(shard) = serde_json::from_slice::<ShardFile>(&plain) else {
                        return Ok(None);
                    };
                    for r in shard.records {
                        if let Some(id) = r.get("id").and_then(|v| v.as_str()) {
                            out.insert((ns.clone(), id.to_string()));
                        }
                    }
                }
            }
        }
        Ok(Some(out))
    }
}

/// The meta key holding the `seq` of this computer's first head that
/// carries a record's tombstone (§8.7 acks).
pub(crate) fn tseq_key(ns: &str, id: &str) -> String {
    format!("tseq|{ns}|{id}")
}

/// A record from an older Kage under a renamed config path counts for the
/// new path too (§8.11).
fn renamed(rec: Record) -> Vec<Record> {
    if rec.ns != CFG_NS {
        return vec![rec];
    }
    renamed_with(rec, catalog::RENAMED)
}

/// [`renamed`] against a given table, so tests can use one.
pub(crate) fn renamed_with(rec: Record, table: &[(&str, &str)]) -> Vec<Record> {
    match table.iter().find(|(old, _)| *old == rec.id) {
        Some((_, new)) => {
            let mut moved = rec.clone();
            moved.id = new.to_string();
            vec![rec, moved]
        }
        None => vec![rec],
    }
}

/// Every tombstone stamp of `ns` in the replica, for gc.
pub(crate) fn tombstones(tx: &Tx<'_>, ns: &str) -> anyhow::Result<Vec<Record>> {
    Ok(tx
        .records(ns)?
        .into_iter()
        .filter(|r| r.del.is_some())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_renamed_path_counts_for_both_names() {
        let r = Record::new(CFG_NS, "ui.old_theme");
        let out = renamed_with(r, &[("ui.old_theme", "ui.theme")]);
        let ids: Vec<&str> = out.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["ui.old_theme", "ui.theme"]);
        let r = Record::new(CFG_NS, "ui.font_size");
        assert_eq!(renamed_with(r, &[("ui.old_theme", "ui.theme")]).len(), 1);
    }
}
