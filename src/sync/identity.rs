//! Identity: this computer's device id, and telling clones, restores and
//! rewinds apart (docs/design/SYNC.md §8.9), plus the root checks every
//! tick starts with (§8.1 `check_root`).
//!
//! - A profile copied to another computer (a migration tool, a disk
//!   image, a VM clone) keeps `device.json`, but its fingerprint differs:
//!   it takes a new device id and publishes everything again under it.
//! - A head of this computer's id with a higher `seq` than it wrote, from
//!   an `instance` it doesn't know, means another computer has the same id:
//!   this one takes a new id. From an instance it knows, this computer was
//!   restored from a backup: it folds its own newer state back in.
//! - A head that went missing, or a shard its newest head names, means the
//!   sync app rewound this computer's directory: publish everything again.
//!   Three times in an hour is `folder_reverting` and a 1-hour pause.

use super::engine::SyncEngine;
use super::envelope::FileKind;
use super::folder::{self, DirKind, FileClass};
use super::publish::HeadFile;
use super::status::{Problem, SyncState};
use super::types::DeviceId;
use std::collections::{BTreeMap, BTreeSet};
use std::io;

/// Instances remembered as this computer's own.
const MAX_INSTANCES: usize = 16;
/// Own temp files older than this are deleted.
pub const TEMP_MAX_AGE_MS: i64 = 10 * 60_000;

/// A new random instance id: 16 lowercase hex characters.
pub fn new_instance() -> String {
    use rand::RngCore;
    let mut b = [0u8; 8];
    rand::rng().fill_bytes(&mut b);
    hex::encode(b)
}

impl SyncEngine {
    /// §8.9 at start: a fingerprint that isn't this computer's means a
    /// copied profile. Returns whether the device id changed.
    pub(crate) fn check_fingerprint(&mut self) -> anyhow::Result<bool> {
        let fp = self.env.fingerprint.clone();
        match self.prefs.fingerprint.as_deref() {
            Some(f) if f == fp => Ok(false),
            None => {
                self.prefs.fingerprint = Some(fp);
                self.save_prefs()?;
                Ok(false)
            }
            Some(_) => {
                log::warn!(
                    "sync: this profile was copied from another computer; taking a new device id"
                );
                self.prefs.fingerprint = Some(fp);
                self.rotate_prefs_id();
                self.save_prefs()?;
                Ok(true)
            }
        }
    }

    /// Give `device.json` a new device id, keeping the old one.
    fn rotate_prefs_id(&mut self) -> DeviceId {
        if let Some(old) = self.prefs.device_id.take() {
            self.prefs.previous_device_ids.push(old);
        }
        let id = DeviceId::random();
        self.prefs.device_id = Some(id.to_string());
        id
    }

    /// Take a new device id now and publish everything again under it.
    pub(crate) fn rotate_device_id(&mut self) -> anyhow::Result<()> {
        let id = self.rotate_prefs_id();
        self.save_prefs()?;
        let Some(core) = self.core.as_mut() else {
            return Ok(());
        };
        core.me = id;
        core.my_seq = 0;
        core.instance = new_instance();
        let instance = core.instance.clone();
        core.replica.tx(|tx| {
            tx.set_meta("my_seq", "0")?;
            forget_published(tx)?;
            remember_instance(tx, &instance)?;
            Ok(())
        })?;
        self.rt.first_own_ms.get_or_insert(self.env.clock.now_ms());
        self.rt.last_own_ms = Some(0);
        Ok(())
    }

    /// The descriptor is there, names this folder and is readable by this
    /// build. Returns `false` (and sets the blocking state) when not.
    pub(crate) fn check_root(&mut self, now: i64) -> anyhow::Result<bool> {
        let Some(core) = self.core.as_ref() else {
            return Ok(false);
        };
        let fs = self.env.fs.clone();
        let layout = core.layout.clone();
        let folder_id = core.folder_id.clone();
        let blocked = match folder::read_descriptor(&*fs, &layout) {
            Ok(Some(d)) if d.folder_id == folder_id => {
                if d.readable_here() {
                    None
                } else {
                    Some(SyncState::UpdateRequired)
                }
            }
            // Started over on another computer, or another computer won
            // the race to create the folder (§5.3).
            Ok(Some(_)) => Some(SyncState::Reset),
            Ok(None) => match fs.list(&layout.root) {
                Ok(_) => Some(SyncState::FolderEmpty),
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    Some(SyncState::FolderDenied)
                }
                Err(_) => Some(SyncState::FolderMissing),
            },
            // Not downloaded yet: files are bound to the folder anyway.
            Err(e) if folder::is_not_ready(&e) => None,
            Err(e) => {
                let denied = e
                    .downcast_ref::<io::Error>()
                    .is_some_and(|io| io.kind() == io::ErrorKind::PermissionDenied);
                Some(if denied {
                    SyncState::FolderDenied
                } else {
                    SyncState::FolderMissing
                })
            }
        };
        // A removal stays until the user joins again.
        if self.rt.blocked == Some(SyncState::Removed) {
            return Ok(false);
        }
        if blocked != self.rt.blocked {
            if let Some(b) = blocked {
                log::warn!("sync: the sync folder can't be used now ({b:?})");
            }
            self.rt.blocked = blocked;
        }
        if blocked.is_none() {
            self.check_key_files(now)?;
        }
        Ok(blocked.is_none())
    }

    /// A key file of a newer epoch means the key changed on another
    /// computer (§11.3): this one is locked until the new passphrase.
    fn check_key_files(&mut self, _now: i64) -> anyhow::Result<()> {
        let Some(core) = self.core.as_ref() else {
            return Ok(());
        };
        let epoch = core.epoch;
        let names = self
            .env
            .fs
            .list(&core.layout.keys_dir())
            .unwrap_or_default();
        let newest = names
            .iter()
            .filter_map(|e| match folder::classify(DirKind::Keys, &e.name) {
                FileClass::KeyFile { epoch, .. } => Some(epoch),
                _ => None,
            })
            .max();
        self.rt.newer_epoch = newest.filter(|e| *e > epoch);
        Ok(())
    }

    /// This computer's own directory: clones, restores and rewinds.
    pub(crate) fn check_own_dir(&mut self, now: i64) -> anyhow::Result<()> {
        let Some(core) = self.core.as_ref() else {
            return Ok(());
        };
        let me = core.me;
        let my_seq = core.my_seq;
        let dir = core.layout.device_dir(me);
        let fs = self.env.fs.clone();
        let entries = match fs.list(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(_) => return Ok(()),
        };
        let mut top = 0u64;
        let mut dups = Vec::new();
        for e in entries.iter().filter(|e| !e.is_dir) {
            match folder::classify(DirKind::Device, &e.name) {
                FileClass::Head(s) => top = top.max(s),
                FileClass::Duplicate { of } => dups.push((e.name.clone(), of)),
                _ => {}
            }
        }
        let instances = self.instances()?;
        // Provider duplicates of my heads: a different instance is a clone.
        for (name, of) in &dups {
            if let Ok(plain) = self.read_open(&dir.join(name), FileKind::Head, me, None, now) {
                if let Ok(h) = serde_json::from_slice::<HeadFile>(&plain) {
                    let _ = of;
                    if !instances.contains(&h.instance) {
                        return self.collision();
                    }
                }
            }
            if let Err(e) = fs.remove(&dir.join(name)) {
                log::info!(
                    "sync: couldn't remove a sync app's copy in this computer's folder: {e}"
                );
            }
            self.rt.duplicates += 1;
        }
        if top > my_seq {
            let path = dir.join(folder::head_name(top));
            match self.read_open(&path, FileKind::Head, me, None, now) {
                Ok(plain) => {
                    let head: HeadFile = serde_json::from_slice(&plain)?;
                    if !instances.contains(&head.instance) && !self.rt.adopt_own {
                        return self.collision();
                    }
                    self.rt.adopt_own = false;
                    // Restored from a backup of this computer: take its
                    // newer state back in and continue after it.
                    log::info!("sync: this computer's folder has a newer head of its own; folding it back in");
                    self.fold_own_head(&head, now)?;
                    if let Some(core) = self.core.as_mut() {
                        core.my_seq = top;
                        core.replica
                            .tx(|tx| tx.set_meta("my_seq", &top.to_string()))?;
                    }
                }
                Err(_) => {}
            }
            return Ok(());
        }
        let rewound = (my_seq > 0 && top < my_seq) || self.own_shard_missing()?;
        if rewound {
            log::warn!(
                "sync: the sync app rewound or removed this computer's files; publishing again"
            );
            if let Some(core) = self.core.as_mut() {
                core.replica.tx(forget_published)?;
            }
            self.rt.first_own_ms.get_or_insert(now);
            self.rt.last_own_ms = Some(0);
            self.rt.rewinds.retain(|t| now - t < 3_600_000);
            self.rt.rewinds.push(now);
            if self.rt.rewinds.len() >= 3 {
                self.rt.publish_paused_until = Some(now + 3_600_000);
                self.rt.blocked = Some(SyncState::FolderReverting);
            }
        }
        Ok(())
    }

    /// Another computer uses this device id: take a new one.
    fn collision(&mut self) -> anyhow::Result<()> {
        log::warn!("sync: another computer uses this computer's device id; taking a new one");
        self.rt.collision = true;
        if !self
            .rt
            .problems
            .iter()
            .any(|p| p.kind == "device_collision")
        {
            self.rt.problems.push(Problem {
                kind: "device_collision".to_string(),
                key: "sync.problem.device_collision".to_string(),
                params: BTreeMap::new(),
            });
        }
        self.rotate_device_id()
    }

    /// Merge this computer's own head (a restore brought back a newer one).
    fn fold_own_head(&mut self, head: &HeadFile, now: i64) -> anyhow::Result<()> {
        let mut changed = BTreeSet::new();
        let core = self.core.as_ref().expect("on");
        let shards_dir = core.layout.shards_dir(core.me);
        let dev = core.me;
        for (ns, entry) in &head.namespaces {
            for name in &entry.shards {
                let path = shards_dir.join(folder::data_name(name));
                let Ok(plain) = self.read_open(&path, FileKind::Shard, dev, None, now) else {
                    continue;
                };
                let Ok(shard) = serde_json::from_slice::<super::publish::ShardFile>(&plain) else {
                    continue;
                };
                self.merge_own_shard(ns, name, &shard.records, &mut changed)?;
            }
        }
        if !changed.is_empty() {
            self.project(&changed, false)?;
        }
        Ok(())
    }

    fn merge_own_shard(
        &mut self,
        ns: &str,
        name: &str,
        records: &[serde_json::Value],
        changed: &mut BTreeSet<(String, String)>,
    ) -> anyhow::Result<()> {
        let core = self.core.as_mut().expect("on");
        let Some(keys) = core.cur_keys().cloned() else {
            return Ok(());
        };
        core.replica.tx(|tx| {
            for v in records {
                let Ok(rec) = super::types::Record::from_json(ns, v) else {
                    continue;
                };
                let b = tx
                    .bucket_of(&rec.ns, &rec.id)?
                    .unwrap_or_else(|| keys.bucket(&rec.ns, &rec.id, 1 << 16));
                if tx.merge(&rec, b)?.changed {
                    changed.insert((rec.ns.clone(), rec.id.clone()));
                }
            }
            tx.add_applied(name)?;
            Ok(())
        })
    }

    /// Whether a shard this computer's newest head names is gone.
    fn own_shard_missing(&mut self) -> anyhow::Result<bool> {
        let Some(core) = self.core.as_mut() else {
            return Ok(false);
        };
        if core.my_seq == 0 {
            return Ok(false);
        }
        let dir = core.layout.shards_dir(core.me);
        let present = super::publish::list_names(&*self.env.fs, &dir, DirKind::Shards);
        let listed = match self.env.fs.list(&dir) {
            Ok(_) => true,
            Err(e) => e.kind() != io::ErrorKind::NotFound,
        };
        let names: Vec<String> = core.replica.tx(|tx| {
            let mut out = Vec::new();
            for ns in tx
                .namespaces()?
                .into_iter()
                .chain(super::publish::SYSTEM_NS.iter().map(|s| s.to_string()))
            {
                if let Some(st) = super::publish::ns_state(tx, &ns)? {
                    out.extend(st.shards);
                }
            }
            Ok(out)
        })?;
        if !listed {
            return Ok(!names.is_empty());
        }
        Ok(names.iter().any(|n| !n.is_empty() && !present.contains(n)))
    }

    /// The instances this computer's sync state has had.
    fn instances(&mut self) -> anyhow::Result<Vec<String>> {
        let Some(core) = self.core.as_mut() else {
            return Ok(Vec::new());
        };
        core.replica.tx(|tx| {
            Ok(tx
                .meta("instances")?
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default())
        })
    }
}

/// Remember an instance as this computer's own.
pub(crate) fn remember_instance(
    tx: &mut super::replica::Tx<'_>,
    instance: &str,
) -> anyhow::Result<()> {
    let mut list: Vec<String> = tx
        .meta("instances")?
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    if !list.iter().any(|i| i == instance) {
        list.push(instance.to_string());
    }
    while list.len() > MAX_INSTANCES {
        list.remove(0);
    }
    tx.set_meta("instances", &serde_json::to_string(&list)?)
}

/// Forget what was published, so the next publish writes every namespace
/// again (a new device id, a rewound directory).
pub(crate) fn forget_published(tx: &mut super::replica::Tx<'_>) -> anyhow::Result<()> {
    for ns in tx
        .namespaces()?
        .into_iter()
        .chain(super::publish::SYSTEM_NS.iter().map(|s| s.to_string()))
    {
        tx.delete_meta(&format!("pub.{ns}"))?;
    }
    // Namespaces with no records yet are published empty again too.
    for ns in [
        "cfg",
        "doc.steering",
        "chat.sessions",
        "installs",
        "cfg.ext",
    ] {
        tx.delete_meta(&format!("pub.{ns}"))?;
    }
    for l in super::catalog::SYNC_LISTS {
        tx.delete_meta(&format!("pub.{}", l.ns))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instances_are_hex_and_random() {
        let a = new_instance();
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, new_instance());
    }
}
