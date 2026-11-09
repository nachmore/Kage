//! Cleanup: acks, tombstone and shard retention, retired computers'
//! directories, and the purge of an extension's data (docs/design/SYNC.md
//! §8.7, §5.8).
//!
//! - Own files: the newest 3 heads are kept; older ones go once a newer
//!   one is 10 minutes old. Shards and blobs no kept head names go after
//!   24 hours. Own temps go after 10 minutes.
//! - Tombstones: cells of a deleted record are kept 30 days ("Recently
//!   deleted"), then only the tombstone stays. The tombstone goes once it is
//!   90 days old and every active computer that syncs the namespace has it:
//!   its head says it read this computer's head that carries it, or its
//!   `gc_floor` is past it. This computer then raises its own `gc_floor`.
//! - Coming back after more than 60 days: records the others deleted and
//!   already cleaned up go to the conflicts list as `deleted_while_away`
//!   instead of coming back everywhere.

use super::conflicts;
use super::engine::{SyncEngine, DAY_MS};
use super::folder::{self, DirKind, FileClass};
use super::identity::TEMP_MAX_AGE_MS;
use super::merge::{is_visible, MAX_ANCESTORS};
use super::replica::{DeviceRow, Tx};
use super::types::{ConflictBody, ConflictKind, DeviceId, Side, Stamp, Tomb};
use std::collections::{BTreeMap, BTreeSet};

/// Heads of its own a computer keeps.
pub const KEEP_HEADS: usize = 3;
/// Own files gc at most this often.
const OWN_GC_EVERY_MS: i64 = 10 * 60_000;
/// Unreferenced shards and blobs stay this long.
const UNREF_KEEP_MS: i64 = DAY_MS;
/// A computer is active when its newest head is this recent.
pub const ACTIVE_MS: i64 = 60 * DAY_MS;
/// Deleted records keep their cells this long.
pub const KEEP_CELLS_MS: i64 = 30 * DAY_MS;
/// Tombstones stay at least this long.
pub const KEEP_TOMBS_MS: i64 = 90 * DAY_MS;
/// A removed computer's directory goes once its last head is this old.
const REMOVED_DIR_MS: i64 = 14 * DAY_MS;
/// Archives and trash are kept this long.
const KEEP_ARCHIVE_MS: i64 = 30 * DAY_MS;

/// Whether a device row is active at `now` (§8.7).
pub(crate) fn is_active(d: &DeviceRow, now: i64) -> bool {
    d.state != "retired" && d.state != "removed" && now - d.last_seen_ms < ACTIVE_MS
}

impl SyncEngine {
    /// Own files every 10 minutes; the rest once a day.
    pub(crate) fn gc_maybe(&mut self, now: i64) -> anyhow::Result<()> {
        if self
            .rt
            .last_own_gc_ms
            .is_none_or(|t| now - t >= OWN_GC_EVERY_MS)
        {
            self.rt.last_own_gc_ms = Some(now);
            if let Err(e) = self.gc_own_files(now) {
                log::info!("sync: cleaning this computer's files failed: {e:#}");
            }
        }
        let last_daily = match self.rt.last_daily_ms {
            Some(t) => Some(t),
            None => {
                let core = self.core.as_mut().expect("on");
                core.replica
                    .tx(|tx| tx.meta("last_daily"))?
                    .and_then(|v| v.parse::<i64>().ok())
            }
        };
        if last_daily.is_none_or(|t| now - t >= DAY_MS) {
            self.rt.last_daily_ms = Some(now);
            self.gc_daily(now)?;
            let core = self.core.as_mut().expect("on");
            core.replica
                .tx(|tx| tx.set_meta("last_daily", &now.to_string()))?;
        }
        Ok(())
    }

    /// Delete this computer's old heads, unreferenced files and temps.
    pub(crate) fn gc_own_files(&mut self, now: i64) -> anyhow::Result<()> {
        let fs = self.env.fs.clone();
        let core = self.core.as_mut().expect("on");
        let me = core.me;
        let dev_dir = core.layout.device_dir(me);
        let dirs = [
            (dev_dir.clone(), DirKind::Device),
            (core.layout.shards_dir(me), DirKind::Shards),
            (core.layout.blobs_dir(me), DirKind::Blobs),
        ];
        // Temps.
        for (dir, kind) in &dirs {
            for e in fs.list(dir).unwrap_or_default() {
                if e.is_dir {
                    continue;
                }
                if folder::classify(*kind, &e.name) == FileClass::OwnTemp
                    && e.modified_ms.is_some_and(|m| now - m >= TEMP_MAX_AGE_MS)
                {
                    let _ = fs.remove(&dir.join(&e.name));
                }
            }
        }
        // Heads beyond the newest 3, once the third newest is 10 minutes old.
        let mut heads: Vec<(u64, Option<i64>)> = fs
            .list(&dev_dir)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|e| match folder::classify(DirKind::Device, &e.name) {
                FileClass::Head(s) => Some((s, e.modified_ms)),
                _ => None,
            })
            .collect();
        heads.sort_by_key(|h| std::cmp::Reverse(h.0));
        if heads.len() > KEEP_HEADS {
            let old_enough = heads[KEEP_HEADS - 1]
                .1
                .is_none_or(|m| now - m >= TEMP_MAX_AGE_MS);
            if old_enough {
                for (seq, _) in &heads[KEEP_HEADS..] {
                    let _ = fs.remove(&dev_dir.join(folder::head_name(*seq)));
                }
            }
        }
        // Shards and blobs no kept head names, after 24 hours.
        let refs: BTreeSet<String> = core.replica.tx(|tx| {
            let heads: BTreeMap<u64, Vec<String>> = tx
                .meta("heads_refs")?
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default();
            Ok(heads.into_values().flatten().collect())
        })?;
        if refs.is_empty() {
            return Ok(());
        }
        let mut unref: BTreeMap<String, i64> = core.replica.tx(|tx| {
            Ok(tx
                .meta("unref")?
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default())
        })?;
        let mut present = BTreeSet::new();
        for (dir, kind) in &dirs[1..] {
            for name in super::publish::list_names(&*fs, dir, *kind) {
                present.insert(name.clone());
                if refs.contains(&name) {
                    unref.remove(&name);
                    continue;
                }
                let first = *unref.entry(name.clone()).or_insert(now);
                if now - first >= UNREF_KEEP_MS {
                    let _ = fs.remove(&dir.join(folder::data_name(&name)));
                    unref.remove(&name);
                    // Read again if another computer still names it.
                    core.replica.tx(|tx| tx.remove_applied(&name))?;
                }
            }
        }
        unref.retain(|n, _| present.contains(n));
        core.replica
            .tx(|tx| tx.set_meta("unref", &serde_json::to_string(&unref)?))?;
        Ok(())
    }

    /// Tombstones, conflicts retention, retired directories, archives and
    /// trash.
    pub(crate) fn gc_daily(&mut self, now: i64) -> anyhow::Result<()> {
        let core = self.core.as_mut().expect("on");
        let me = core.me;
        let mut retired: Vec<DeviceRow> = Vec::new();
        let super::engine::Core { replica, hlc, .. } = core;
        replica.tx(|tx| {
            let devices = tx.devices()?;
            let active: Vec<&DeviceRow> = devices.iter().filter(|d| is_active(d, now)).collect();
            let mut floor: Option<Stamp> = tx.meta("gc_floor")?.and_then(|s| s.parse().ok());
            for ns in tx.namespaces()? {
                for mut r in super::ingest::tombstones(tx, &ns)? {
                    let Some(d) = r.del.clone() else { continue };
                    let age = now - d.wall_ms;
                    if !is_visible(&r) && age >= KEEP_CELLS_MS && !r.cells.is_empty() {
                        r.cells.clear();
                        let b = tx.bucket_of(&ns, &r.id)?.unwrap_or(0);
                        tx.put_record(&r, b)?;
                        tx.mark_dirty(&ns, b, false)?;
                    }
                    if age < KEEP_TOMBS_MS || is_visible(&r) {
                        continue;
                    }
                    if acked_by_all(tx, &ns, &r.id, &d, &active)? {
                        // Its shard is written again without it.
                        let b = tx.bucket_of(&ns, &r.id)?.unwrap_or(0);
                        tx.delete_record(&ns, &r.id)?;
                        tx.mark_dirty(&ns, b, false)?;
                        tx.delete_meta(&super::ingest::tseq_key(&ns, &r.id))?;
                        if floor.is_none_or(|f| d.stamp > f) {
                            floor = Some(d.stamp);
                        }
                    }
                }
            }
            if let Some(f) = floor {
                tx.set_meta("gc_floor", &f.to_string())?;
            }
            super::engine::kill::point("mid_gc");
            conflicts::retention_pass(tx, now, me, hlc)?;
            tx.set_meta("hlc_last", &hlc.last().to_string())?;
            for d in devices {
                let read = d.ingested_seq >= d.head_seq;
                let left = d.state == "retired" && read && d.head_seq > 0;
                let removed =
                    d.state == "removed" && read && now - d.last_seen_ms >= REMOVED_DIR_MS;
                if left || removed {
                    retired.push(d);
                }
            }
            Ok(())
        })?;
        // A retired or removed computer's directory, once its last head is
        // fully read (R1's exception).
        for d in retired {
            let core = self.core.as_mut().expect("on");
            let dir = core.layout.device_dir(d.dev);
            match self.env.fs.remove(&dir) {
                Ok(()) => log::info!("sync: removed the folder of a computer that left sync"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    log::info!("sync: couldn't remove a retired computer's folder: {e}");
                    continue;
                }
            }
            let core = self.core.as_mut().expect("on");
            core.replica.tx(|tx| tx.remove_device(d.dev))?;
        }
        self.gc_archives(now);
        self.gc_trash(now);
        Ok(())
    }

    /// Archives of a Start over older than 30 days (any computer may).
    fn gc_archives(&self, now: i64) {
        let Some(core) = self.core.as_ref() else {
            return;
        };
        let dir = core.layout.archive_dir();
        for e in self.env.fs.list(&dir).unwrap_or_default() {
            let Ok(t) = chrono::NaiveDateTime::parse_from_str(&e.name, "%Y%m%d-%H%M%S") else {
                continue;
            };
            if now - t.and_utc().timestamp_millis() >= KEEP_ARCHIVE_MS {
                let _ = self.env.fs.remove(&dir.join(&e.name));
            }
        }
    }

    /// Join backups and bulk-delete copies older than 30 days.
    fn gc_trash(&self, now: i64) {
        let dir = self.env.local_dir.join(super::engine::TRASH_DIR);
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return;
        };
        for e in rd.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .is_some_and(|d| now - d.as_millis() as i64 >= KEEP_ARCHIVE_MS);
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    /// The coming-back rule (§8.7): run once after this computer hadn't
    /// finished an ingest for more than 60 days.
    pub(crate) fn come_back(&mut self, now: i64) -> anyhow::Result<BTreeSet<(String, String)>> {
        let mut changed = BTreeSet::new();
        let core = self.core.as_mut().expect("on");
        let devices = core.replica.tx(|tx| tx.devices())?;
        let active: Vec<&DeviceRow> = devices.iter().filter(|d| is_active(d, now)).collect();
        if active.is_empty() {
            // Nothing recent read yet (the folder may still be catching
            // up): try again next tick.
            return Ok(changed);
        }
        let Some(floor) = active.iter().filter_map(|d| d.gc_floor).max() else {
            self.rt.came_back = false;
            return Ok(changed);
        };
        let ids: Vec<DeviceId> = active.iter().map(|d| d.dev).collect();
        let Some(held) = self.ids_held_elsewhere(&ids, now)? else {
            // Not everything is readable yet: try again next tick.
            return Ok(changed);
        };
        let syncs: BTreeMap<DeviceId, BTreeSet<String>> = {
            let core = self.core.as_mut().expect("on");
            core.replica.tx(|tx| {
                let mut out = BTreeMap::new();
                for d in &ids {
                    let nss: BTreeSet<String> = tx
                        .meta(&format!("nss.{d}"))?
                        .and_then(|v| serde_json::from_str(&v).ok())
                        .unwrap_or_default();
                    out.insert(*d, nss);
                }
                Ok(out)
            })?
        };
        let core = self.core.as_mut().expect("on");
        let keys = core.cur_keys().cloned().expect("not locked");
        let name_key = *keys.name_key();
        let me = core.me;
        let bucket = move |ns: &str, id: &str| keys.bucket(ns, id, 1 << 16);
        let super::engine::Core { replica, hlc, .. } = core;
        replica.tx(|tx| {
            for ns in tx.namespaces()? {
                if ns.starts_with("sync.") || !syncs.values().all(|s| s.contains(&ns)) {
                    continue;
                }
                for mut r in tx.records(&ns)? {
                    if !is_visible(&r) || !tx.is_published(&ns, &r.id)? {
                        continue;
                    }
                    let Some(max) = r.cells.values().map(|c| c.stamp).max() else {
                        continue;
                    };
                    if max > floor || held.contains(&(ns.clone(), r.id.clone())) {
                        continue;
                    }
                    // Deleted while this computer was away.
                    let wall = r.cells.values().map(|c| c.wall_ms).max().unwrap_or(now);
                    let body = ConflictBody {
                        ns: ns.clone(),
                        rid: r.id.clone(),
                        field: None,
                        kind: ConflictKind::DeletedWhileAway,
                        loser: Side {
                            stamp: max,
                            wall_ms: wall,
                            deleted: false,
                            value: Some(r.to_json()),
                        },
                        winner: None,
                    };
                    conflicts::record_action(
                        tx,
                        &name_key,
                        &body,
                        conflicts::OPEN,
                        now,
                        me,
                        hlc,
                        &bucket,
                    )?;
                    let stamp = Stamp::new(hlc.tick(now, max.hlc), me);
                    r.del = Some(Tomb {
                        stamp,
                        wall_ms: now,
                        anc: r.newest_cell_stamps(MAX_ANCESTORS),
                    });
                    let b = tx
                        .bucket_of(&ns, &r.id)?
                        .unwrap_or_else(|| bucket(&ns, &r.id));
                    tx.write(&r, b)?;
                    changed.insert((ns.clone(), r.id.clone()));
                }
            }
            tx.set_meta("hlc_last", &hlc.last().to_string())
        })?;
        if !changed.is_empty() {
            log::info!(
                "sync: {} items deleted on other computers while this one was away moved to the conflicts list",
                changed.len()
            );
        }
        self.rt.came_back = false;
        Ok(changed)
    }
}

/// Whether every active computer that syncs `ns` has the tombstone.
fn acked_by_all(
    tx: &Tx<'_>,
    ns: &str,
    id: &str,
    d: &Tomb,
    active: &[&DeviceRow],
) -> anyhow::Result<bool> {
    let tseq: Option<u64> = tx
        .meta(&super::ingest::tseq_key(ns, id))?
        .and_then(|v| v.parse().ok());
    let acks = tx.acks(ns, id)?;
    for dev in active {
        let nss: BTreeSet<String> = tx
            .meta(&format!("nss.{}", dev.dev))?
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        if !nss.contains(ns) {
            continue;
        }
        let seen: u64 = tx
            .meta(&format!("seen.{}", dev.dev))?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let ok = dev.gc_floor.is_some_and(|f| f >= d.stamp)
            || tseq.is_some_and(|t| seen >= t)
            || acks.contains(&dev.dev);
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_means_recent_and_not_retired() {
        let now = 100 * DAY_MS;
        let mut d = DeviceRow {
            last_seen_ms: now - DAY_MS,
            state: "ok".into(),
            ..Default::default()
        };
        assert!(is_active(&d, now));
        d.last_seen_ms = now - 61 * DAY_MS;
        assert!(!is_active(&d, now));
        d.last_seen_ms = now;
        d.state = "retired".into();
        assert!(!is_active(&d, now));
    }
}
