//! Membership: create, join, leave, remove a computer, start over and undo
//! joining (docs/design/SYNC.md §8.8, §8.9, §13.5–§13.7), and opening this
//! computer's sync state at start.

use super::canonical::{hash, same};
use super::catalog;
use super::conflicts;
use super::crypto::{self, KeyFile, Keys};
use super::engine::{
    effective_record, internal, pins_of, split_fields, Core, JoinPending, SyncEngine, Wake, DAY_MS,
    REPLICA_FILE, TRASH_DIR,
};
use super::folder::{self, Descriptor, DirKind, FileClass, Layout};
use super::hlc::Hlc;
use super::identity::{forget_published, new_instance, remember_instance};
use super::keystore::StoredKey;
use super::merge::is_visible;
use super::prefs::DevicePrefs;
use super::replica::{OpenNote, Replica};
use super::scope;
use super::sources::{LocalRecord, LocalState, LocalWrite, ORDER};
use super::status::{CategoryCounts, JoinComputer, JoinPreview, SyncStatus};
use super::types::{
    Category, ConflictBody, ConflictKind, DeviceId, FieldKey, LocalKey, Origin, Scope, Side, Stamp,
    READER_VERSION, SYNC_FORMAT,
};
use super::{CreateReq, JoinChoice, JoinReq};
use crate::error::{AppError, ErrorKind};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The shortest passphrase a user may choose (§11.3).
pub const MIN_PASSPHRASE: usize = 12;
/// Computers listed in a join preview.
const MAX_PREVIEW_COMPUTERS: usize = 64;
/// How deep Find folder looks in each cloud root.
const FIND_DEPTH: usize = 3;
/// Undo joining is offered this long.
pub const UNDO_JOIN_MS: i64 = 30 * DAY_MS;

fn keyed(kind: ErrorKind, key: &str) -> AppError {
    AppError::keyed(kind, key, &[])
}

fn wrong_passphrase() -> AppError {
    keyed(ErrorKind::InvalidInput, "settings.sync.passphrase.wrong")
}

/// Whether a passphrase is long enough, or reads as a generated one.
pub fn passphrase_ok(p: &str) -> bool {
    let norm = crypto::normalize_passphrase(p);
    norm.chars().count() >= MIN_PASSPHRASE
}

fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Remove `replica.db` and its WAL files.
fn remove_replica(dir: &Path) {
    for name in [REPLICA_FILE, "replica.db-wal", "replica.db-shm"] {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

/// The README written into a new sync folder: in the user's language, then
/// English.
fn readme() -> String {
    let here = crate::i18n::translate("sync.readme", &[]);
    let en = crate::i18n::translate_in("en", "sync.readme", &[]);
    if here == en {
        format!("{en}\n")
    } else {
        format!("{here}\n\n{en}\n")
    }
}

impl SyncEngine {
    /// Open this computer's sync state from `device.json` (sync is on here).
    pub(crate) fn start_core(&mut self) -> anyhow::Result<()> {
        self.check_fingerprint()?;
        let folder = self
            .prefs
            .folder
            .clone()
            .ok_or_else(|| anyhow::anyhow!("device.json names no folder"))?;
        let folder_id = self
            .prefs
            .folder_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("device.json names no folder id"))?;
        let me: DeviceId = self
            .prefs
            .device_id
            .as_deref()
            .and_then(crypto::parse_device_text)
            .ok_or_else(|| anyhow::anyhow!("device.json has no valid device id"))?;
        let layout = Layout::resolve(&*self.env.fs, Path::new(&folder));
        let (mut replica, note) = Replica::open(&self.env.local_dir.join(REPLICA_FILE))?;
        let instance = new_instance();
        let (hlc_last, my_seq, last_ingest, join_pending) = replica.tx(|tx| {
            remember_instance(tx, &instance)?;
            Ok((
                tx.meta("hlc_last")?
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0),
                tx.meta("my_seq")?
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0),
                tx.meta("last_complete_ingest")?
                    .and_then(|v| v.parse::<i64>().ok()),
                tx.meta("join_pending")?.is_some(),
            ))
        })?;
        let mut keys = BTreeMap::new();
        let mut epoch = self.prefs.key_epoch.max(1);
        match self.env.keys.get(&folder_id) {
            Ok(Some(k)) => {
                epoch = k.epoch;
                keys.insert(k.epoch, Keys::derive(&k.dek));
                for e in 1..k.epoch {
                    if let Ok(Some(old)) = self.env.keys.get(&format!("{folder_id}.{e}")) {
                        keys.insert(old.epoch, Keys::derive(&old.dek));
                    }
                }
            }
            Ok(None) => log::warn!("sync: the data key isn't in the key store; sync is locked"),
            Err(e) => log::warn!("sync: the key store can't be read ({e:#}); sync is locked"),
        }
        self.core = Some(Core {
            replica,
            layout,
            folder_id,
            me,
            instance,
            hlc: Hlc::new(hlc_last),
            keys,
            epoch,
            my_seq,
        });
        let now = self.now();
        if last_ingest.is_some_and(|t| now - t > super::gc::ACTIVE_MS) {
            self.rt.came_back = true;
        }
        if note == OpenNote::RebuiltFromCorrupt && my_seq == 0 {
            // A rebuild joins with the other computers' values (§15).
            log::warn!("sync: rebuilding this computer's sync state from the folder");
            self.rt.join = Some(JoinPending {
                started_ms: now,
                auto: true,
            });
            self.rt.adopt_own = true;
        } else if join_pending {
            self.rt.join = Some(JoinPending {
                started_ms: now,
                auto: false,
            });
        }
        Ok(())
    }

    /// Set up sync from this computer: a new sync folder.
    pub(crate) fn create(&mut self, req: CreateReq) -> Result<SyncStatus, AppError> {
        if self.prefs.enabled {
            return Err(keyed(ErrorKind::InvalidInput, "errors.sync.already_on"));
        }
        if !passphrase_ok(&req.passphrase) {
            return Err(keyed(
                ErrorKind::InvalidInput,
                "errors.sync.passphrase_short",
            ));
        }
        let fs = self.env.fs.clone();
        let layout = Layout::in_chosen(&req.path);
        match folder::read_descriptor(&*fs, &Layout::resolve(&*fs, &req.path)) {
            Ok(None) => {}
            _ => return Err(keyed(ErrorKind::InvalidInput, "errors.sync.folder_exists")),
        }
        let now = self.now();
        let me = DeviceId::random();
        let folder_id = crypto::random_id();
        let dek = crypto::new_dek();
        let kf = crypto::wrap_dek_with(&req.passphrase, &dek, &folder_id, 1, self.kdf)
            .map_err(internal)?;
        let d = Descriptor {
            kage_sync: SYNC_FORMAT,
            folder_id: folder_id.clone(),
            created_at: rfc3339(now),
            min_reader: READER_VERSION,
        };
        if let Err(e) = folder::create_folder(&*fs, &layout, &d, &readme(), &kf, me) {
            log::warn!("sync: couldn't create the sync folder: {e:#}");
            return Err(keyed(ErrorKind::InvalidInput, "errors.sync.folder_write"));
        }
        self.env
            .keys
            .set(&folder_id, &StoredKey::new(1, &dek))
            .map_err(internal)?;
        remove_replica(&self.env.local_dir);
        self.prefs = DevicePrefs {
            enabled: true,
            folder: Some(req.path.to_string_lossy().into_owned()),
            folder_id: Some(folder_id),
            device_id: Some(me.to_string()),
            device_name: Some(req.device_name),
            fingerprint: Some(self.env.fingerprint.clone()),
            categories: req.categories,
            key_store: Some(self.env.keys.kind().wire().to_string()),
            key_epoch: 1,
            ..DevicePrefs::default()
        };
        self.save_prefs().map_err(internal)?;
        self.rt = Default::default();
        self.start_core().map_err(internal)?;
        self.rt.descriptor_recheck_at = Some(now + 10_000);
        self.tick(Wake::Manual);
        self.env.events.track(
            "sync_enabled",
            vec![
                ("role", "create".to_string()),
                ("categories", self.prefs.categories_on().to_string()),
            ],
        );
        Ok(self.build_status())
    }

    /// Unlock an existing sync folder and preview what joining changes.
    pub(crate) fn join(&mut self, req: JoinReq) -> Result<JoinPreview, AppError> {
        if self.prefs.enabled && self.rt.join.is_none() {
            return Err(keyed(ErrorKind::InvalidInput, "errors.sync.already_on"));
        }
        let fs = self.env.fs.clone();
        let layout = Layout::resolve(&*fs, &req.path);
        let d = match folder::read_descriptor(&*fs, &layout) {
            Ok(Some(d)) => d,
            Ok(None) => {
                return Err(keyed(
                    ErrorKind::InvalidInput,
                    "errors.sync.not_a_sync_folder",
                ))
            }
            Err(_) => return Err(keyed(ErrorKind::Timeout, "errors.sync.not_ready")),
        };
        if !d.readable_here() {
            return Err(keyed(ErrorKind::InvalidInput, "errors.sync.newer_format"));
        }
        let (epoch, dek) = unlock_from_folder(&*fs, &layout, &d.folder_id, &req.passphrase)?;
        let me = DeviceId::random();
        self.env
            .keys
            .set(&d.folder_id, &StoredKey::new(epoch, &dek))
            .map_err(internal)?;
        remove_replica(&self.env.local_dir);
        let path = if layout.root == req.path {
            req.path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or(req.path.clone())
        } else {
            req.path.clone()
        };
        self.prefs = DevicePrefs {
            enabled: true,
            folder: Some(path.to_string_lossy().into_owned()),
            folder_id: Some(d.folder_id.clone()),
            device_id: Some(me.to_string()),
            device_name: Some(req.device_name),
            fingerprint: Some(self.env.fingerprint.clone()),
            categories: req.categories,
            key_store: Some(self.env.keys.kind().wire().to_string()),
            key_epoch: epoch,
            ..DevicePrefs::default()
        };
        self.save_prefs().map_err(internal)?;
        self.rt = Default::default();
        self.start_core().map_err(internal)?;
        let now = self.now();
        self.rt.join = Some(JoinPending {
            started_ms: now,
            auto: false,
        });
        if let Some(core) = self.core.as_mut() {
            core.replica
                .tx(|tx| tx.set_meta("join_pending", "1"))
                .map_err(internal)?;
        }
        self.tick(Wake::Manual);
        self.join_preview()
    }

    /// What joining would change (§8.8 step 5).
    pub(crate) fn join_preview(&mut self) -> Result<JoinPreview, AppError> {
        let os = self.env.os;
        let me = self.core.as_ref().map(|c| c.me);
        let mut computers = Vec::new();
        let core = self.core.as_mut().ok_or_else(super::unavailable)?;
        let devices = core.replica.tx(|tx| tx.devices()).map_err(internal)?;
        for d in devices
            .iter()
            .filter(|d| Some(d.dev) != me && d.state != "retired")
        {
            computers.push(JoinComputer {
                name: d.name.clone(),
                os: d.os.unwrap_or(os),
                last_seen_at: d.last_seen_ms,
            });
        }
        computers.truncate(MAX_PREVIEW_COMPUTERS);
        let mut p = JoinPreview {
            computers,
            same: CategoryCounts::default(),
            different: CategoryCounts::default(),
            only_here: CategoryCounts::default(),
            only_synced: CategoryCounts::default(),
        };
        let specs = self.specs.clone();
        for si in 0..self.sources.len() {
            let local = self.sources[si].read_local().map_err(internal)?;
            let core = self.core.as_mut().expect("on");
            let ns_specs: Vec<_> = specs
                .values()
                .filter(|(i, _)| *i == si)
                .map(|(_, s)| s.clone())
                .collect();
            core.replica
                .tx(|tx| {
                    for spec in &ns_specs {
                        let mut seen = BTreeSet::new();
                        let pairs = join_matches(tx, spec, &local, os)?;
                        for ((ns, id), l) in &local.records {
                            if *ns != spec.ns {
                                continue;
                            }
                            let Some(l) = l else { continue };
                            let cat = catalog::category_of(ns, id).unwrap_or(spec.category);
                            let rid = pairs.get(id).cloned().unwrap_or_else(|| id.clone());
                            seen.insert(rid.clone());
                            match tx.record(ns, &rid)? {
                                Some(r) if is_visible(&r) && scope::exists_here(&r, os) => {
                                    let pins = pins_of(tx, ns, &rid)?;
                                    let t = effective_record(spec, &r, Some(l), &pins, os);
                                    let mut a = t.fields.clone();
                                    let mut b = l.fields.clone();
                                    a.remove(ORDER);
                                    b.remove(ORDER);
                                    if same(&Value::Object(a), &Value::Object(b)) {
                                        bump(&mut p.same, cat);
                                    } else {
                                        bump(&mut p.different, cat);
                                    }
                                }
                                _ => bump(&mut p.only_here, cat),
                            }
                        }
                        for r in tx.records(&spec.ns)? {
                            if seen.contains(&r.id)
                                || !is_visible(&r)
                                || !scope::exists_here(&r, os)
                            {
                                continue;
                            }
                            let cat =
                                catalog::category_of(&spec.ns, &r.id).unwrap_or(spec.category);
                            bump(&mut p.only_synced, cat);
                        }
                    }
                    Ok(())
                })
                .map_err(internal)?;
        }
        Ok(p)
    }

    /// Join with the user's choice (§8.8).
    pub(crate) fn confirm_join(&mut self, choice: JoinChoice) -> Result<SyncStatus, AppError> {
        if self.rt.join.is_none() {
            return Err(super::unavailable());
        }
        let now = self.now();
        self.write_join_backup(now).map_err(internal)?;
        self.prefs.joined_at = Some(rfc3339(now));
        self.save_prefs().map_err(internal)?;
        let os = self.env.os;
        let specs = self.specs.clone();
        let mut remaps = BTreeSet::new();
        for si in 0..self.sources.len() {
            let local = self.sources[si].read_local().map_err(internal)?;
            let ns_specs: Vec<_> = specs
                .values()
                .filter(|(i, _)| *i == si)
                .map(|(_, s)| s.clone())
                .filter(|s| self.syncs_here(&s.ns, None) || s.ns == catalog::CFG_NS)
                .collect();
            let core = self.core.as_mut().expect("on");
            let keys = core.cur_keys().cloned().ok_or_else(super::unavailable)?;
            let name_key = *keys.name_key();
            let me = core.me;
            let bucket = move |ns: &str, id: &str| keys.bucket(ns, id, 1 << 16);
            let Core { replica, hlc, .. } = core;
            replica
                .tx(|tx| {
                    for spec in &ns_specs {
                        let pairs = join_matches(tx, spec, &local, os)?;
                        for ((ns, id), l) in &local.records {
                            if *ns != spec.ns {
                                continue;
                            }
                            let Some(l) = l else { continue };
                            let rid = pairs.get(id).cloned().unwrap_or_else(|| id.clone());
                            if rid != *id {
                                remaps.insert((ns.clone(), rid.clone()));
                            }
                            let Some(r) = tx.record(ns, &rid)? else {
                                continue;
                            };
                            if !is_visible(&r) || !scope::exists_here(&r, os) {
                                continue;
                            }
                            let cells = split_fields(&l.fields, spec);
                            for (name, lv) in &cells {
                                let Some(c) = super::engine::eff_cell(&r, name, os).cloned() else {
                                    continue;
                                };
                                let (_, sc) = super::engine::eff_source(&r, name, os);
                                let key = LocalKey::new(ns, &rid, name);
                                if same(&c.value, lv) {
                                    tx.set_projected(&key, Some((hash(lv), Some(c.stamp), sc)))?;
                                    continue;
                                }
                                let field =
                                    Some(FieldKey::new(name.clone(), sc.unwrap_or(Scope::All)));
                                let (loser, seen_hash) = match choice {
                                    // The synced value wins; the local one is kept.
                                    JoinChoice::Theirs => (
                                        Side {
                                            stamp: Stamp::new(hlc.tick(now, 0), me),
                                            wall_ms: now,
                                            deleted: false,
                                            value: Some(lv.clone()),
                                        },
                                        hash(lv),
                                    ),
                                    // This computer's wins; the synced one is kept.
                                    JoinChoice::Mine => (
                                        Side {
                                            stamp: c.stamp,
                                            wall_ms: c.wall_ms,
                                            deleted: false,
                                            value: Some(c.value.clone()),
                                        },
                                        hash(&c.value),
                                    ),
                                };
                                let body = ConflictBody {
                                    ns: ns.clone(),
                                    rid: rid.clone(),
                                    field,
                                    kind: ConflictKind::ReplacedOnJoin,
                                    loser,
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
                                tx.set_projected(&key, Some((seen_hash, Some(c.stamp), sc)))?;
                            }
                            if catalog::list_spec(ns).is_some() {
                                if let Some(c) = r.cell(ORDER, Scope::All) {
                                    tx.set_projected(
                                        &LocalKey::new(ns, &rid, ORDER),
                                        Some((hash(&c.value), Some(c.stamp), Some(Scope::All))),
                                    )?;
                                }
                            }
                        }
                    }
                    tx.delete_meta("join_pending")?;
                    tx.set_meta("join_group", &format!("join-{now}"))?;
                    tx.set_meta("hlc_last", &hlc.last().to_string())
                })
                .map_err(internal)?;
        }
        self.rt.join = None;
        self.rt.keep_synced_order = choice == JoinChoice::Theirs;
        // Matched items take the synced id locally; then the normal tick
        // captures what's only here and projects what's only synced.
        let _ = remaps;
        self.rt.first_tick_done = false;
        self.tick(Wake::Manual);
        self.rt.keep_synced_order = false;
        self.env.events.track(
            "sync_enabled",
            vec![
                ("role", "join".to_string()),
                ("categories", self.prefs.categories_on().to_string()),
            ],
        );
        Ok(self.build_status())
    }

    /// Keep this computer's syncable values before a join, for Undo
    /// joining (§8.8 step 2).
    fn write_join_backup(&mut self, now: i64) -> anyhow::Result<()> {
        let mut states = serde_json::Map::new();
        for s in self.sources.iter_mut() {
            let st = s.read_local()?;
            let recs: Vec<Value> = st
                .records
                .iter()
                .filter_map(|((ns, id), r)| {
                    r.as_ref().map(|r| {
                        serde_json::json!({ "ns": ns, "id": id, "fields": r.fields, "avail": r.avail })
                    })
                })
                .collect();
            states.insert(s.id().to_string(), Value::Array(recs));
        }
        let dir = self.env.local_dir.join(TRASH_DIR);
        std::fs::create_dir_all(&dir)?;
        let name = format!("join-{now}.json");
        crate::atomic_write::write(
            &dir.join(&name),
            serde_json::to_vec(&serde_json::json!({ "at": now, "sources": states }))?,
            crate::atomic_write::Perms::Private,
        )?;
        let core = self.core.as_mut().expect("on");
        core.replica.tx(|tx| tx.set_meta("join_backup", &name))
    }

    /// Unlock with the passphrase (a new key epoch, or a lost key).
    pub(crate) fn unlock(&mut self, passphrase: &str) -> Result<SyncStatus, AppError> {
        self.require_on()?;
        let fs = self.env.fs.clone();
        let core = self.core.as_ref().expect("on");
        let (layout, folder_id, old_epoch) =
            (core.layout.clone(), core.folder_id.clone(), core.epoch);
        let (epoch, dek) = unlock_from_folder(&*fs, &layout, &folder_id, passphrase)?;
        let store = self.env.keys.clone();
        if epoch > old_epoch {
            // Keep the old epoch's key: computers that haven't moved still
            // write with it.
            if let Ok(Some(old)) = store.get(&folder_id) {
                let _ = store.set(&format!("{folder_id}.{}", old.epoch), &old);
            }
        }
        store
            .set(&folder_id, &StoredKey::new(epoch, &dek))
            .map_err(internal)?;
        self.prefs.key_epoch = epoch;
        self.save_prefs().map_err(internal)?;
        let core = self.core.as_mut().expect("on");
        core.keys.insert(epoch, Keys::derive(&dek));
        let moved = epoch > core.epoch;
        core.epoch = epoch.max(core.epoch);
        if moved {
            core.replica.tx(forget_published).map_err(internal)?;
            self.rt.first_own_ms.get_or_insert(self.env.clock.now_ms());
            self.rt.last_own_ms = Some(0);
        }
        self.rt.newer_epoch = None;
        self.tick(Wake::Manual);
        Ok(self.build_status())
    }

    /// Turn off sync on this computer (§13.5). Local data stays.
    pub(crate) fn leave(&mut self) -> Result<(), AppError> {
        self.require_on()?;
        let now = self.now();
        if !self.core_locked() {
            if let Err(e) = self.publish_retired(now) {
                log::warn!("sync: couldn't publish the final head: {e:#}");
            }
        }
        self.turn_off("leave");
        Ok(())
    }

    /// Forget this computer's sync state: the key, the replica and
    /// `device.json`'s membership. Local data stays.
    fn turn_off(&mut self, reason: &'static str) {
        if let Some(core) = self.core.take() {
            let _ = self.env.keys.delete(&core.folder_id);
            for e in 1..core.epoch {
                let _ = self.env.keys.delete(&format!("{}.{e}", core.folder_id));
            }
            drop(core);
        }
        remove_replica(&self.env.local_dir);
        let categories = self.prefs.categories;
        let name = self.prefs.device_name.clone();
        self.prefs = DevicePrefs {
            categories,
            device_name: name,
            ..DevicePrefs::default()
        };
        if let Err(e) = self.save_prefs() {
            log::warn!("sync: couldn't save device.json: {e:#}");
        }
        self.rt = Default::default();
        self.env
            .events
            .track("sync_disabled", vec![("reason", reason.to_string())]);
    }

    /// A last head with `retired: true`, so the others clean up (§8.9).
    fn publish_retired(&mut self, now: i64) -> anyhow::Result<()> {
        self.publish(now)?;
        let states: BTreeMap<String, super::publish::NsState> = {
            let core = self.core.as_mut().expect("on");
            core.replica.tx(|tx| {
                let mut out = BTreeMap::new();
                for ns in tx
                    .namespaces()?
                    .into_iter()
                    .chain(super::publish::SYSTEM_NS.iter().map(|s| s.to_string()))
                {
                    if let Some(s) = super::publish::ns_state(tx, &ns)? {
                        out.insert(ns, s);
                    }
                }
                Ok(out)
            })?
        };
        let (head, seq) = self.build_head(now, &states, true)?;
        let plain = super::canonical::canonical(&serde_json::to_value(&head)?);
        let core = self.core.as_ref().expect("on");
        let file = folder::head_name(seq);
        let sealed =
            super::publish::seal_file(core, super::envelope::FileKind::Head, &file, &plain)
                .ok_or_else(|| anyhow::anyhow!("locked"))?;
        super::publish::write_once_retrying(
            &*self.env.fs,
            &core.layout.device_dir(core.me),
            &file,
            &sealed,
        )?;
        Ok(())
    }

    /// Remove another computer from sync (§8.9). "Lost or stolen" also
    /// rotates the key.
    pub(crate) fn remove_device(
        &mut self,
        id: &str,
        lost: bool,
        new_passphrase: Option<String>,
    ) -> Result<(), AppError> {
        self.require_on()?;
        let dev = crypto::parse_device_text(id)
            .ok_or_else(|| keyed(ErrorKind::InvalidInput, "errors.sync.no_such_item"))?;
        let now = self.now();
        let core = self.core.as_mut().expect("on");
        if dev == core.me {
            return Err(keyed(ErrorKind::InvalidInput, "errors.sync.no_such_item"));
        }
        if lost && !new_passphrase.as_deref().is_some_and(passphrase_ok) {
            return Err(keyed(
                ErrorKind::InvalidInput,
                "errors.sync.passphrase_short",
            ));
        }
        let keys = core.cur_keys().cloned().ok_or_else(super::unavailable)?;
        let me = core.me;
        let Core { replica, hlc, .. } = core;
        replica
            .tx(|tx| {
                write_cell(
                    tx,
                    &keys,
                    hlc,
                    me,
                    now,
                    "sync.devices",
                    &dev.to_string(),
                    "revoked",
                    Value::Bool(true),
                )?;
                if let Some(mut row) = tx.device(dev)? {
                    row.state = "removed".into();
                    tx.put_device(&row)?;
                }
                tx.set_meta("hlc_last", &hlc.last().to_string())
            })
            .map_err(internal)?;
        self.rt.first_own_ms.get_or_insert(now);
        self.rt.last_own_ms = Some(0);
        if lost {
            self.rotate_key(new_passphrase.as_deref().unwrap_or_default())
                .map_err(internal)?;
        }
        self.tick(Wake::Manual);
        Ok(())
    }

    /// A new key epoch with a new data key and passphrase (§11.3 "Lost or
    /// stolen computer"). Everything is published again under it.
    fn rotate_key(&mut self, passphrase: &str) -> anyhow::Result<()> {
        let fs = self.env.fs.clone();
        let kdf = self.kdf;
        let store = self.env.keys.clone();
        let core = self.core.as_mut().expect("on");
        let epoch = core.epoch + 1;
        let dek = crypto::new_dek();
        let kf = crypto::wrap_dek_with(passphrase, &dek, &core.folder_id, epoch, kdf)?;
        folder::write_key_file(&*fs, &core.layout, &kf, core.me)?;
        // Keep the old epoch's key: other computers write with it until
        // they move.
        if let Some(old) = self.env.keys.get(&core.folder_id)? {
            store.set(&format!("{}.{}", core.folder_id, old.epoch), &old)?;
        }
        store.set(&core.folder_id, &StoredKey::new(epoch, &dek))?;
        core.keys.insert(epoch, Keys::derive(&dek));
        core.epoch = epoch;
        core.replica.tx(forget_published)?;
        self.prefs.key_epoch = epoch;
        self.save_prefs()?;
        log::info!("sync: the data key was rotated to epoch {epoch}");
        Ok(())
    }

    /// Re-wrap the data key with a new passphrase, from this computer
    /// (§11.3). Other computers notice nothing.
    pub(crate) fn change_passphrase(&mut self, passphrase: &str) -> Result<(), AppError> {
        self.require_on()?;
        if !passphrase_ok(passphrase) {
            return Err(keyed(
                ErrorKind::InvalidInput,
                "errors.sync.passphrase_short",
            ));
        }
        let fs = self.env.fs.clone();
        let stored = {
            let core = self.core.as_ref().expect("on");
            self.env.keys.get(&core.folder_id).map_err(internal)?
        };
        let Some(stored) = stored else {
            return Err(super::unavailable());
        };
        let core = self.core.as_ref().expect("on");
        let kf = crypto::wrap_dek_with(
            passphrase,
            &stored.dek,
            &core.folder_id,
            stored.epoch,
            self.kdf,
        )
        .map_err(internal)?;
        let new_name =
            folder::write_key_file(&*fs, &core.layout, &kf, core.me).map_err(internal)?;
        let mine = core.me.to_string();
        for (class, _) in folder::list_key_files(&*fs, &core.layout).map_err(internal)? {
            if let FileClass::KeyFile { epoch, dev, n } = class {
                let name = folder::key_file_name(epoch, core.me, n);
                if dev == mine && epoch == stored.epoch && name != new_name {
                    let _ = fs.remove(&core.layout.keys_dir().join(name));
                }
            }
        }
        Ok(())
    }

    /// Start over (§13.6): everything in the folder moves to `archive/`,
    /// and this computer starts a new sync folder in its place.
    pub(crate) fn start_over(&mut self, passphrase: &str) -> Result<SyncStatus, AppError> {
        self.require_on()?;
        if !passphrase_ok(passphrase) {
            return Err(keyed(
                ErrorKind::InvalidInput,
                "errors.sync.passphrase_short",
            ));
        }
        let now = self.now();
        let fs = self.env.fs.clone();
        let core = self.core.as_ref().expect("on");
        let layout = core.layout.clone();
        let stamp = chrono::DateTime::from_timestamp_millis(now)
            .unwrap_or_default()
            .format("%Y%m%d-%H%M%S")
            .to_string();
        let to = layout.archive_dir().join(&stamp);
        fs.create_dir_all(&to).map_err(|e| internal(e.into()))?;
        for name in [folder::DEVICES_DIR, folder::KEYS_DIR, folder::DESCRIPTOR] {
            let from = layout.root.join(name);
            match fs.rename(&from, &to.join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(internal(e.into())),
            }
        }
        let path = PathBuf::from(self.prefs.folder.clone().unwrap_or_default());
        let req = CreateReq {
            path,
            passphrase: passphrase.to_string(),
            device_name: self.prefs.device_name.clone().unwrap_or_default(),
            categories: self.prefs.categories,
        };
        self.turn_off("reset");
        self.create(req)
    }

    /// Undo joining (§13.7): put back the values this computer had before
    /// it joined, and turn sync off here. What it sent stays in the folder.
    pub(crate) fn undo_join(&mut self) -> Result<(), AppError> {
        self.require_on()?;
        let core = self.core.as_mut().expect("on");
        let name = core
            .replica
            .tx(|tx| tx.meta("join_backup"))
            .map_err(internal)?
            .ok_or_else(|| keyed(ErrorKind::InvalidInput, "errors.sync.no_join_backup"))?;
        let path = self.env.local_dir.join(TRASH_DIR).join(&name);
        let bytes = std::fs::read(&path).map_err(|e| internal(e.into()))?;
        let backup: Value = serde_json::from_slice(&bytes).map_err(|e| internal(e.into()))?;
        // Dismiss this join's entries, so the conflicts list doesn't offer
        // values that are coming back anyway.
        self.dismiss_all(Some("join")).ok();
        for si in 0..self.sources.len() {
            let sid = self.sources[si].id();
            let Some(recs) = backup["sources"][sid].as_array() else {
                continue;
            };
            let mut before = LocalState::default();
            for r in recs {
                let (Some(ns), Some(id)) = (r["ns"].as_str(), r["id"].as_str()) else {
                    continue;
                };
                let fields = r["fields"].as_object().cloned().unwrap_or_default();
                before.put(
                    ns,
                    id,
                    LocalRecord {
                        fields,
                        avail: r["avail"].as_str().map(str::to_string),
                    },
                );
            }
            let now = self.sources[si].read_local().map_err(internal)?;
            let writes = restore_writes(&before, &now);
            if !writes.is_empty() {
                self.sources[si]
                    .project(writes, Origin::Import)
                    .map_err(internal)?;
            }
        }
        let now = self.now();
        let _ = self.publish(now);
        self.leave()
    }

    /// Remove an extension's data from the folder (§8.7).
    pub(crate) fn purge_extension(&mut self, ext: &str) -> Result<(), AppError> {
        self.require_on()?;
        let now = self.now();
        let core = self.core.as_mut().expect("on");
        let keys = core.cur_keys().cloned().ok_or_else(super::unavailable)?;
        let me = core.me;
        let target = format!("ext.{ext}");
        let Core { replica, hlc, .. } = core;
        let stamp = replica
            .tx(|tx| {
                let s = Stamp::new(hlc.tick(now, 0), me);
                write_cell(
                    tx,
                    &keys,
                    hlc,
                    me,
                    now,
                    "sync.ns_state",
                    &target,
                    "purged",
                    Value::String(s.to_string()),
                )?;
                tx.set_meta("hlc_last", &hlc.last().to_string())?;
                Ok(s)
            })
            .map_err(internal)?;
        self.apply_purge(&target, stamp).map_err(internal)?;
        self.rt.first_own_ms.get_or_insert(now);
        self.rt.last_own_ms = Some(0);
        self.tick(Wake::Manual);
        Ok(())
    }

    /// Look for this sync folder in the detected cloud roots, 3 levels deep
    /// (§13.7 Find folder).
    pub(crate) fn find_folder(&mut self) -> Result<Option<PathBuf>, AppError> {
        self.require_on()?;
        let folder_id = self
            .core
            .as_ref()
            .map(|c| c.folder_id.clone())
            .unwrap_or_default();
        let fs = self.env.fs.clone();
        for root in self.env.cloud.suggest_folders() {
            if let Some(p) = find_in(&*fs, &root.path, &folder_id, FIND_DEPTH) {
                return Ok(Some(p));
            }
        }
        Ok(None)
    }

    /// Use the sync folder at `path` (found again after a move).
    pub(crate) fn apply_folder(&mut self, path: PathBuf) -> Result<(), AppError> {
        self.require_on()?;
        let fs = self.env.fs.clone();
        let layout = Layout::resolve(&*fs, &path);
        let folder_id = self
            .core
            .as_ref()
            .map(|c| c.folder_id.clone())
            .unwrap_or_default();
        match folder::read_descriptor(&*fs, &layout) {
            Ok(Some(d)) if d.folder_id == folder_id => {}
            _ => {
                return Err(keyed(
                    ErrorKind::InvalidInput,
                    "errors.sync.not_this_folder",
                ))
            }
        }
        let chosen = if layout.root == path {
            path.parent().map(Path::to_path_buf).unwrap_or(path)
        } else {
            path
        };
        self.prefs.folder = Some(chosen.to_string_lossy().into_owned());
        self.save_prefs().map_err(internal)?;
        if let Some(core) = self.core.as_mut() {
            core.layout = layout;
        }
        self.rt.blocked = None;
        self.tick(Wake::Manual);
        Ok(())
    }

    /// Rename this computer (shown on the others).
    pub(crate) fn rename_device(&mut self, name: String) -> Result<(), AppError> {
        self.require_on()?;
        let name: String = name.trim().chars().take(64).collect();
        if name.is_empty() {
            return Err(keyed(ErrorKind::InvalidInput, "errors.sync.no_name"));
        }
        self.prefs.device_name = Some(name);
        self.save_prefs().map_err(internal)?;
        let now = self.now();
        self.rt.first_own_ms.get_or_insert(now);
        self.rt.last_own_ms = Some(0);
        Ok(())
    }
}

/// Count one record in its category.
fn bump(c: &mut CategoryCounts, cat: Category) {
    match cat {
        Category::Settings => c.settings += 1,
        Category::Automations => c.automations += 1,
        Category::Extensions => c.extensions += 1,
        Category::Chats => c.chats += 1,
        Category::System => {}
    }
}

/// Local list items that match a synced item by content or name, at a join
/// (§4.4), so items set up by hand on both computers don't appear twice.
/// Returns `local id → synced id`.
fn join_matches(
    tx: &super::replica::Tx<'_>,
    spec: &super::sources::NsSpec,
    local: &LocalState,
    os: super::types::OsKind,
) -> anyhow::Result<BTreeMap<String, String>> {
    if catalog::list_spec(&spec.ns).is_none() {
        return Ok(BTreeMap::new());
    }
    let mut orphans = BTreeMap::new();
    for ((ns, id), l) in &local.records {
        let Some(l) = l else { continue };
        if *ns != spec.ns || tx.record(ns, id)?.is_some() {
            continue;
        }
        let mut f = l.fields.clone();
        f.remove(ORDER);
        orphans.insert(id.clone(), f);
    }
    if orphans.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut synced = BTreeMap::new();
    for r in tx.records(&spec.ns)? {
        if local.get(&spec.ns, &r.id).is_some() || !is_visible(&r) || !scope::exists_here(&r, os) {
            continue;
        }
        let pins = pins_of(tx, &spec.ns, &r.id)?;
        let mut f = effective_record(spec, &r, None, &pins, os).fields;
        f.remove(ORDER);
        synced.insert(r.id.clone(), f);
    }
    let pairs = super::sources::lists::rematch(&orphans, &synced, catalog::label_field(&spec.ns));
    Ok(pairs.into_iter().collect())
}

/// Write one cell of sync's own records (`sync.devices`, `sync.ns_state`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_cell(
    tx: &mut super::replica::Tx<'_>,
    keys: &Keys,
    hlc: &mut Hlc,
    me: DeviceId,
    now: i64,
    ns: &str,
    id: &str,
    field: &str,
    value: Value,
) -> anyhow::Result<()> {
    let mut r = tx
        .record(ns, id)?
        .unwrap_or_else(|| super::types::Record::new(ns, id));
    let key = FieldKey::all(field);
    let cur = r.cells.get(&key).cloned();
    let stamp = Stamp::new(hlc.tick(now, r.max_stamp().map_or(0, |s| s.hlc)), me);
    r.cells.insert(
        key,
        super::types::Cell {
            value,
            stamp,
            wall_ms: now,
            anc: super::merge::ancestors(cur.as_ref(), me),
        },
    );
    let b = tx
        .bucket_of(ns, id)?
        .unwrap_or_else(|| keys.bucket(ns, id, 1 << 16));
    tx.write(&r, b)
}

/// The data key of the newest epoch the passphrase unlocks.
fn unlock_from_folder(
    fs: &dyn super::fsio::SyncFs,
    layout: &Layout,
    folder_id: &str,
    passphrase: &str,
) -> Result<(u32, zeroize::Zeroizing<[u8; 32]>), AppError> {
    let files: Vec<(FileClass, KeyFile)> = folder::list_key_files(fs, layout).map_err(internal)?;
    let mut by_epoch: BTreeMap<u32, Vec<KeyFile>> = BTreeMap::new();
    for (_, kf) in files {
        by_epoch.entry(kf.epoch).or_default().push(kf);
    }
    if by_epoch.is_empty() {
        return Err(keyed(ErrorKind::Timeout, "errors.sync.not_ready"));
    }
    for (epoch, kfs) in by_epoch.iter().rev() {
        for kf in kfs {
            if let Ok(dek) = crypto::unwrap_dek(passphrase, kf, folder_id) {
                return Ok((*epoch, dek));
            }
        }
    }
    Err(wrong_passphrase())
}

/// A folder under `dir` (at most `depth` levels down) whose sync folder has
/// `folder_id`: the folder the user would pick.
fn find_in(
    fs: &dyn super::fsio::SyncFs,
    dir: &Path,
    folder_id: &str,
    depth: usize,
) -> Option<PathBuf> {
    let layout = Layout::resolve(fs, dir);
    if let Ok(Some(d)) = folder::read_descriptor(fs, &layout) {
        if d.folder_id == folder_id {
            return Some(if layout.root == dir {
                dir.parent()
                    .map(Path::to_path_buf)
                    .unwrap_or(dir.to_path_buf())
            } else {
                dir.to_path_buf()
            });
        }
    }
    if depth == 0 {
        return None;
    }
    for e in fs.list(dir).ok()? {
        if e.is_dir
            && !e.name.starts_with('.')
            && folder::classify(DirKind::Root, &e.name) != FileClass::OsJunk
        {
            if let Some(p) = find_in(fs, &dir.join(&e.name), folder_id, depth - 1) {
                return Some(p);
            }
        }
    }
    None
}

/// The writes that put `before` back over `now`: every record it had,
/// lists in their old order, and nothing it didn't have.
fn restore_writes(before: &LocalState, now: &LocalState) -> Vec<LocalWrite> {
    let mut out = Vec::new();
    let mut lists: BTreeMap<&str, Vec<(u64, &str, &LocalRecord)>> = BTreeMap::new();
    for ((ns, id), r) in &before.records {
        let Some(r) = r else { continue };
        if catalog::list_spec(ns).is_some() {
            let pos = r
                .fields
                .get(ORDER)
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX);
            lists
                .entry(ns.as_str())
                .or_default()
                .push((pos, id.as_str(), r));
        } else {
            out.push(LocalWrite {
                ns: ns.clone(),
                id: id.clone(),
                value: Some(r.clone()),
            });
        }
    }
    for items in lists.values_mut() {
        items.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        let mut prev: Option<String> = None;
        for (_, id, r) in items.iter() {
            let key = super::engine::key_between(prev.as_deref(), None);
            let mut r = (*r).clone();
            r.fields
                .insert(ORDER.to_string(), Value::String(key.clone()));
            prev = Some(key);
            let ns = catalog::SYNC_LISTS
                .iter()
                .find(|l| before.get(l.ns, id).is_some())
                .map(|l| l.ns)
                .unwrap_or_default();
            out.push(LocalWrite {
                ns: ns.to_string(),
                id: id.to_string(),
                value: Some(r),
            });
        }
    }
    for (ns, id) in now.records.keys() {
        if before
            .records
            .get(&(ns.clone(), id.clone()))
            .is_none_or(Option::is_none)
        {
            out.push(LocalWrite {
                ns: ns.clone(),
                id: id.clone(),
                value: None,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passphrases_need_twelve_characters_or_the_generated_form() {
        assert!(!passphrase_ok("short"));
        assert!(passphrase_ok("correct horse battery"));
        assert!(passphrase_ok(&crypto::generate_passphrase()));
    }

    #[test]
    fn restoring_puts_lists_back_in_order_and_removes_what_came_later() {
        let mut before = LocalState::default();
        let item = |pos: u64, name: &str| {
            let mut f = serde_json::Map::new();
            f.insert("name".into(), Value::from(name));
            f.insert(ORDER.into(), Value::from(pos));
            LocalRecord {
                fields: f,
                avail: None,
            }
        };
        before.put("list.shortcuts", "s2", item(1, "b"));
        before.put("list.shortcuts", "s1", item(0, "a"));
        let mut now = before.clone();
        now.put("list.shortcuts", "s9", item(2, "synced"));
        let w = restore_writes(&before, &now);
        let ids: Vec<(&str, bool)> = w
            .iter()
            .map(|w| (w.id.as_str(), w.value.is_some()))
            .collect();
        assert_eq!(ids, vec![("s1", true), ("s2", true), ("s9", false)]);
        let k1 = w[0].value.as_ref().unwrap().fields[ORDER]
            .as_str()
            .unwrap()
            .to_string();
        let k2 = w[1].value.as_ref().unwrap().fields[ORDER]
            .as_str()
            .unwrap()
            .to_string();
        assert!(k1 < k2);
    }
}
