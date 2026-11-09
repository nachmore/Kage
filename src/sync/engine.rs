//! The sync engine (docs/design/SYNC.md §8, §19.4): one tick captures,
//! ingests, merges, projects and publishes. It runs only on the `kage-sync`
//! thread and reaches the app only through the ports below.
//!
//! The work is split by step: capture and projection here, then
//! [`super::ingest`], [`super::publish`], [`super::gc`],
//! [`super::identity`], [`super::membership`], [`super::guards`] and
//! [`super::status`], each an `impl SyncEngine` block over the same state.

use super::canonical::{hash, same};
use super::catalog::{self, CFG_NS};
use super::conflicts;
use super::crypto::{KdfCost, Keys};
use super::folder::Layout;
use super::fsio::SyncFs;
use super::hlc::Hlc;
use super::keystore::KeyStore;
use super::merge::{ancestors, is_visible, MAX_ANCESTORS};
use super::prefs::DevicePrefs;
use super::replica::{Projected, Replica, Tx};
use super::scope;
use super::sources::{
    is_unchanged, LocalRecord, LocalState, LocalWrite, NsSpec, ProjectResult, Source, ORDER,
};
use super::split::{self, absent_value, AVAIL};
use super::status::{Problem, SyncState, SyncStatus};
use super::types::{
    Cell, ConflictBody, ConflictKind, DefaultScope, DeviceId, FieldKey, LocalKey, Origin, OsKind,
    PinReason, Record, Scope, Side, Stamp, Tomb,
};
use super::{EngineReply, EngineRequest};
use crate::error::AppError;
use crate::os::cloud_files::CloudFiles;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// `config.json` as the engine sees it.
pub trait ConfigPort: Send + Sync {
    /// The current config and its revision.
    fn snapshot(&self) -> (u64, crate::config::Config);
    /// Save `next` if nothing else saved since `base_revision`. Returns the
    /// new revision.
    fn commit(
        &self,
        base_revision: u64,
        next: crate::config::Config,
        origin: Origin,
    ) -> Result<u64, CommitError>;
    /// Whether `Config::load` reset the file or some sections (§8.10).
    fn load_was_reset(&self) -> LoadReset;
    /// A newer build wrote `config.json`; saves are suspended.
    fn saves_suspended(&self) -> bool;
    fn installed(&self) -> Vec<crate::extensions::InstalledItem>;
}

/// Why a commit didn't save.
#[derive(Debug)]
pub enum CommitError {
    /// Another save came first. Capture, then try again.
    Stale,
    Suspended,
    Io(anyhow::Error),
}

/// What `Config::load` had to reset this run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LoadReset {
    #[default]
    None,
    All,
    Sections(Vec<String>),
}

/// What the engine tells the app.
pub trait HostEvents: Send + Sync {
    fn status_changed(&self, s: &SyncStatus);
    fn conflicts_changed(&self, open: usize);
    /// Emitted before `config_updated`, with catalog paths.
    fn applied(&self, keys: &[String]);
    /// Keys and lists modes: `extension_data_changed` with source `sync`.
    fn extension_data_changed(&self, ext: &str, keys: &[String]);
    /// Collections mode: `extension_sync_changes`.
    fn extension_sync_changes(&self, ext: &str, collections: &[String], reason: &str);
    /// An OS notification for a state the user must act on (§13.8).
    fn notify_user(&self, state: &str);
    fn track(&self, event: &'static str, props: Vec<(&'static str, String)>);
}

/// Time and power, so tests can drive them.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
    fn power(&self) -> crate::os::power::PowerState;
}

/// Everything the engine needs. It never reads `crate::paths` globals, so
/// one test process can run several computers.
pub struct EngineEnv {
    /// `<data_local>/kage/sync/`.
    pub local_dir: PathBuf,
    pub os: OsKind,
    pub kage_version: String,
    pub fingerprint: String,
    pub fs: Arc<dyn SyncFs>,
    pub cloud: Arc<dyn CloudFiles>,
    pub keys: Arc<dyn KeyStore>,
    pub clock: Arc<dyn Clock>,
    pub events: Arc<dyn HostEvents>,
    pub config: Arc<dyn ConfigPort>,
}

/// Why a tick runs (§14.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    Startup,
    Local {
        source: &'static str,
        origin: Origin,
    },
    Folder,
    Poll,
    Focus,
    Resume,
    Manual,
    Exit,
}

/// What one tick did, for logs and tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    pub captured: usize,
    pub ingested_shards: usize,
    pub projected: usize,
    pub published: bool,
    pub new_conflicts: usize,
}

/// Debounce of own edits before a publish (§8.3).
pub const PUBLISH_QUIET_MS: i64 = 5_000;
/// The longest own edits wait for a publish.
pub const PUBLISH_MAX_WAIT_MS: i64 = 30_000;
/// Echo publishes wait this long when nothing of this computer's changed.
pub const ECHO_EVERY_MS: i64 = 30 * 60_000;
/// A file that fails to open is "waiting" this long, then "unreadable".
pub const NOT_READY_MS: i64 = 10 * 60_000;
pub(crate) const DAY_MS: i64 = 86_400_000;

/// A file name in `<data_local>/kage/sync/`.
pub const REPLICA_FILE: &str = "replica.db";
pub const LOCK_FILE: &str = "engine.lock";
pub const TRASH_DIR: &str = "trash";

/// This computer's live sync state: everything that exists only while
/// sync is set up here.
pub(crate) struct Core {
    pub replica: Replica,
    pub layout: Layout,
    pub folder_id: String,
    pub me: DeviceId,
    /// Random per process start (§5.5), to tell a clone from a restore.
    pub instance: String,
    pub hlc: Hlc,
    /// The keys of every epoch this computer can read. Empty while locked.
    pub keys: BTreeMap<u32, Keys>,
    pub epoch: u32,
    /// The `seq` of this computer's newest head.
    pub my_seq: u64,
}

impl Core {
    pub fn cur_keys(&self) -> Option<&Keys> {
        self.keys.get(&self.epoch)
    }

    pub fn locked(&self) -> bool {
        self.cur_keys().is_none()
    }
}

/// A join waiting for the user's choice (§8.8 step 5).
#[derive(Debug, Clone, Default)]
pub(crate) struct JoinPending {
    pub started_ms: i64,
    /// A rebuild of a lost replica: join with the other computers' values
    /// by itself once everything is read (§15).
    pub auto: bool,
}

/// How long a join waits for files to download before it goes on.
pub const JOIN_WAIT_MS: i64 = 2 * 60_000;

/// What the engine remembers between ticks, but not across restarts.
#[derive(Default)]
pub(crate) struct Runtime {
    pub first_tick_done: bool,
    pub dirty_sources: BTreeSet<&'static str>,
    pub pending_origin: Option<Origin>,
    pub first_own_ms: Option<i64>,
    pub last_own_ms: Option<i64>,
    pub last_publish_ms: Option<i64>,
    pub last_ingest_ms: Option<i64>,
    pub last_own_gc_ms: Option<i64>,
    pub last_daily_ms: Option<i64>,
    /// A problem that stops folder I/O (folder missing, reset, removed…).
    pub blocked: Option<SyncState>,
    pub busy: bool,
    pub waiting_files: u32,
    pub placeholder_ticks: u32,

    pub unreadable: BTreeMap<DeviceId, u32>,
    pub problems: Vec<Problem>,
    pub last_status: Option<SyncStatus>,
    pub last_conflicts_open: Option<usize>,
    pub join: Option<JoinPending>,
    /// `(ns, id, cell)` → change times and devices, for the churn guard.
    pub churn: HashMap<(String, String, String), Vec<(i64, DeviceId)>>,
    pub frozen: HashSet<(String, String, String)>,
    pub write_backoff: Option<(i64, u32)>,
    pub rewinds: Vec<i64>,
    pub publish_paused_until: Option<i64>,
    pub used_bytes: u64,
    pub clock_warned: HashMap<DeviceId, i64>,
    pub problem_days: HashMap<String, i64>,
    pub reset_handled: bool,
    pub descriptor_recheck_at: Option<i64>,
    pub io_paused_until: Option<i64>,
    pub came_back: bool,
    pub write_failed: bool,
    pub disk_full: bool,
    /// A key file or file of a newer key epoch is in the folder (§11.3).
    pub newer_epoch: Option<u32>,
    pub duplicates: u32,
    pub collision: bool,
    /// Right after joining with the other computers' values: their list
    /// order wins too.
    pub keep_synced_order: bool,
    /// `config.json` was reset at load and the synced values were put
    /// back (§8.10): Settings → Sync says so.
    pub reset_notice: bool,
    pub cache: super::status::StatusCache,
    /// The state the user must act on, and since when.
    pub state_since: Option<(SyncState, i64)>,
    /// The replica was rebuilt: this computer's own newer heads are its own
    /// state to fold back in, not a clone's (§15).
    pub adopt_own: bool,
}

/// The engine. Owns the replica; lives on the `kage-sync` thread.
pub struct SyncEngine {
    pub(crate) env: EngineEnv,
    pub(crate) sources: Vec<Box<dyn Source>>,
    /// Namespace → (source index, spec).
    pub(crate) specs: BTreeMap<String, (usize, NsSpec)>,
    pub(crate) prefs: DevicePrefs,
    pub(crate) core: Option<Core>,
    /// Held for its OS lock on `engine.lock`: one engine per profile.
    pub(crate) _lock: Option<std::fs::File>,
    pub(crate) other_engine: bool,
    pub(crate) rt: Runtime,
    pub(crate) kdf: KdfCost,
}

/// Named places a test can make the engine crash at (§18.3), to check that
/// a restart from the same directories loses nothing. Inert unless armed,
/// which only tests do.
#[doc(hidden)]
pub mod kill {
    use std::cell::Cell;

    thread_local! {
        static ARMED: Cell<Option<&'static str>> = const { Cell::new(None) };
    }

    /// Crash at `name` the next time the engine passes it on this thread.
    pub fn arm(name: &'static str) {
        ARMED.with(|a| a.set(Some(name)));
    }

    pub fn disarm() {
        ARMED.with(|a| a.set(None));
    }

    /// A kill point: panics if armed for `name`.
    pub(crate) fn point(name: &'static str) {
        if ARMED.with(|a| a.get()) == Some(name) {
            disarm();
            panic!("kill point {name}");
        }
    }
}

/// The engine's own error, as the frontend sees it.
pub(crate) fn internal(e: anyhow::Error) -> AppError {
    log::warn!("sync: {e:#}");
    AppError::keyed(crate::error::ErrorKind::Internal, "errors.sync.failed", &[])
}

/// What a capture found to do, for the caller to act on after its
/// transaction.
#[derive(Default)]
pub(crate) struct CaptureOut {
    pub written: usize,
    pub tombstones: BTreeMap<String, usize>,
    /// Records whose local id must be rewritten (a lost id was matched).
    pub remapped: BTreeSet<(String, String)>,
    pub conflicts: usize,
    pub approvals: Vec<(String, String)>,
}

impl SyncEngine {
    /// Read `device.json`, open the replica, and finish unfinished
    /// projections.
    pub fn open(env: EngineEnv, sources: Vec<Box<dyn Source>>) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&env.local_dir)?;
        let lock_path = env.local_dir.join(LOCK_FILE);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;
        let (lock, other_engine) = match file.try_lock() {
            Ok(()) => (Some(file), false),
            Err(std::fs::TryLockError::WouldBlock) => {
                log::warn!("sync: another copy of Kage is syncing this profile; this one won't");
                (None, true)
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        };
        let mut specs = BTreeMap::new();
        for (i, s) in sources.iter().enumerate() {
            for spec in s.specs() {
                specs.insert(spec.ns.clone(), (i, spec));
            }
        }
        let prefs = DevicePrefs::load(&env.local_dir);
        let mut engine = Self {
            env,
            sources,
            specs,
            prefs,
            core: None,
            _lock: lock,
            other_engine,
            rt: Runtime::default(),
            kdf: KdfCost::DEFAULT,
        };
        if engine.prefs.enabled && !engine.other_engine {
            if let Err(e) = engine.start_core() {
                log::warn!("sync: couldn't open this computer's sync state: {e:#}");
            }
        }
        Ok(engine)
    }

    /// The Argon2 cost of new key files. Tests use a cheap one.
    #[doc(hidden)]
    pub fn set_kdf_cost(&mut self, cost: KdfCost) {
        self.kdf = cost;
    }

    pub(crate) fn now(&self) -> i64 {
        self.env.clock.now_ms()
    }

    pub(crate) fn save_prefs(&self) -> anyhow::Result<()> {
        self.prefs.save(&self.env.local_dir)
    }

    pub fn tick(&mut self, wake: Wake) -> TickReport {
        let mut rep = TickReport::default();
        if let Err(e) = self.tick_inner(wake, &mut rep) {
            log::warn!("sync: a tick failed: {e:#}");
            if is_disk_full(&e) {
                self.rt.disk_full = true;
            }
        } else if !self.rt.write_failed {
            // A write to the folder that failed for lack of space keeps
            // `disk_full` until a publish goes through.
            self.rt.disk_full = false;
        }
        self.after_tick();
        rep
    }

    fn tick_inner(&mut self, wake: Wake, rep: &mut TickReport) -> anyhow::Result<()> {
        let now = self.now();
        match wake {
            Wake::Local { source, origin } => {
                self.rt.dirty_sources.insert(source);
                self.rt.pending_origin = Some(stronger(self.rt.pending_origin, origin));
            }
            Wake::Manual | Wake::Startup => {
                self.rt
                    .dirty_sources
                    .extend(self.sources.iter().map(|s| s.id()));
            }
            _ => {}
        }
        if !self.prefs.enabled || self.core.is_none() || self.other_engine {
            return Ok(());
        }
        if let Some(join) = self.rt.join.clone() {
            // Joining: read what arrives for the preview; nothing local
            // changes until the user chooses.
            if !self.core_locked() {
                self.check_root(now)?;
                if self.rt.blocked.is_none() {
                    self.ingest(now, rep, false)?;
                }
                let read_all = self.rt.waiting_files == 0 || now - join.started_ms >= JOIN_WAIT_MS;
                if join.auto && read_all && self.rt.blocked.is_none() {
                    if let Err(e) = self.confirm_join(super::JoinChoice::Theirs) {
                        log::warn!("sync: rebuilding from the folder failed: {e}");
                    }
                }
            }
            return Ok(());
        }
        let first = !self.rt.first_tick_done;
        let locked = self.core_locked();
        if !locked {
            let all =
                first || matches!(wake, Wake::Manual | Wake::Startup | Wake::Exit | Wake::Poll);
            rep.captured += self.capture_dirty(all)?;
        }
        let paused = self.prefs.paused_at(now) || self.rt.io_paused_until.is_some_and(|t| t > now);
        if locked || paused {
            self.rt.first_tick_done = !locked;
            return Ok(());
        }
        if !self.check_root(now)? {
            self.rt.first_tick_done = true;
            return Ok(());
        }
        self.check_own_dir(now)?;
        let mut changed = BTreeSet::new();
        if wake != Wake::Exit {
            changed = self.ingest(now, rep, false)?;
            if self.rt.blocked.is_some() {
                return Ok(());
            }
            if self.rt.came_back {
                changed.extend(self.come_back(now)?);
            }
        }
        if wake != Wake::Exit {
            rep.projected += self.project(&changed, first)?;
        }
        self.rt.first_tick_done = true;
        if self.publish_due(now, wake) {
            rep.published = self.publish(now)?;
        }
        if wake != Wake::Exit {
            self.gc_maybe(now)?;
        }
        Ok(())
    }

    /// Capture and publish own edits only, within `budget`. No ingest.
    pub fn flush_for_exit(&mut self, budget: Duration) {
        let start = std::time::Instant::now();
        if !self.prefs.enabled || self.core.is_none() || self.core_locked() {
            return;
        }
        if let Err(e) = self.capture_dirty(true) {
            log::warn!("sync: capture at exit failed: {e:#}");
            return;
        }
        if start.elapsed() >= budget {
            return;
        }
        let now = self.now();
        if self.prefs.paused_at(now) || self.rt.blocked.is_some() {
            return;
        }
        if self.has_own_dirty() {
            if let Err(e) = self.publish(now) {
                log::warn!("sync: publish at exit failed: {e:#}");
            }
        }
    }

    pub fn handle(&mut self, req: EngineRequest) -> Result<EngineReply, AppError> {
        let out = self.dispatch(req);
        self.after_tick();
        out
    }

    pub fn status(&self) -> SyncStatus {
        self.build_status()
    }

    /// When the scheduler should tick next without a wake (ms since the
    /// epoch): a pending publish, or a backoff that ends.
    pub fn next_deadline(&self) -> Option<i64> {
        let mut out: Option<i64> = None;
        let mut at = |t: i64| out = Some(out.map_or(t, |o: i64| o.min(t)));
        if let (Some(first), Some(last)) = (self.rt.first_own_ms, self.rt.last_own_ms) {
            at((last + PUBLISH_QUIET_MS).min(first + PUBLISH_MAX_WAIT_MS));
        }
        if let Some((t, _)) = self.rt.write_backoff {
            at(t);
        }
        if let Some(t) = self.rt.descriptor_recheck_at {
            at(t);
        }
        out
    }

    pub(crate) fn core_locked(&self) -> bool {
        self.core.as_ref().is_none_or(Core::locked)
    }

    /// Emit status and conflict-count changes after a tick or request.
    fn after_tick(&mut self) {
        self.refresh_status_cache();
        let st = self.build_status();
        if self.rt.last_status.as_ref() != Some(&st) {
            self.env.events.status_changed(&st);
            self.rt.last_status = Some(st.clone());
        }
        let open = st.conflicts_open as usize;
        if self.rt.last_conflicts_open != Some(open) {
            self.env.events.conflicts_changed(open);
            self.rt.last_conflicts_open = Some(open);
        }
        self.note_problems(&st);
    }

    /// `sync_problem` telemetry once per UTC day per kind (§16), and an OS
    /// notification for a state the user must act on once it lasted an
    /// hour, at most once a day (§13.8).
    fn note_problems(&mut self, st: &SyncStatus) {
        let now = self.now();
        let day = now.div_euclid(DAY_MS);
        let state = serde_json::to_value(st.state)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let is_problem = !matches!(
            st.state,
            SyncState::Off
                | SyncState::Unavailable
                | SyncState::SettingUp
                | SyncState::UpToDate
                | SyncState::Syncing
                | SyncState::Waiting
                | SyncState::Paused
        );
        let mut kinds: Vec<String> = st.problems.iter().map(|p| p.kind.clone()).collect();
        if is_problem {
            kinds.push(state.clone());
        }
        for kind in kinds {
            if self.rt.problem_days.get(&kind) != Some(&day) {
                self.rt.problem_days.insert(kind.clone(), day);
                self.env.events.track("sync_problem", vec![("kind", kind)]);
            }
        }
        let must_act = matches!(
            st.state,
            SyncState::Locked
                | SyncState::FolderMissing
                | SyncState::UpdateRequired
                | SyncState::Removed
                | SyncState::Reset
        );
        if !must_act {
            self.rt.state_since = None;
            return;
        }
        let since = match self.rt.state_since {
            Some((s, t)) if s == st.state => t,
            _ => {
                self.rt.state_since = Some((st.state, now));
                now
            }
        };
        let key = format!("notified.{state}");
        if now - since >= 3_600_000 && self.rt.problem_days.get(&key) != Some(&day) {
            self.rt.problem_days.insert(key, day);
            self.env.events.notify_user(&state);
        }
    }

    // --- Specs and categories -------------------------------------------------

    /// Whether `ns` (and, for `cfg`, record `id`) syncs on this computer.
    pub(crate) fn syncs_here(&self, ns: &str, id: Option<&str>) -> bool {
        let cat = match (ns, id) {
            (CFG_NS, Some(id)) => catalog::category_of(ns, id),
            _ => self
                .specs
                .get(ns)
                .map(|(_, s)| s.category)
                .or_else(|| catalog::category_of(ns, "")),
        };
        if let Some(ext) = ext_of_ns(ns) {
            if self.prefs.ext_data_is_off(ext) {
                return false;
            }
        }
        match cat {
            Some(c) => self.prefs.category_on(c),
            // An opaque namespace from a newer Kage is always carried.
            None => true,
        }
    }

    /// Whether the cfg namespace syncs at all here (any of its categories).
    fn cfg_any_on(&self) -> bool {
        self.prefs.categories.settings
            || self.prefs.categories.automations
            || self.prefs.categories.extensions
    }

    /// Whether the config namespaces may change now: not while
    /// `config.json` is from a newer build.
    fn config_writable(&self) -> bool {
        !self.env.config.saves_suspended()
    }

    // --- Capture (§8.2) ----------------------------------------------------------

    /// Capture every dirty source (all of them when `all`).
    pub(crate) fn capture_dirty(&mut self, all: bool) -> anyhow::Result<usize> {
        let origin = self.rt.pending_origin.take().unwrap_or(Origin::User);
        let dirty = std::mem::take(&mut self.rt.dirty_sources);
        let mut n = 0;
        for si in 0..self.sources.len() {
            let id = self.sources[si].id();
            if !all && !dirty.contains(id) {
                continue;
            }
            if !self.source_capturable(si) {
                continue;
            }
            let state = self.sources[si].read_local()?;
            let (written, remapped) = self.capture_state(si, &state, origin)?;
            n += written;
            if !remapped.is_empty() {
                // A matched item gets its old id back in the local file.
                self.project(&remapped, false)?;
            }
        }
        Ok(n)
    }

    /// Whether any namespace of source `si` captures here now.
    fn source_capturable(&self, si: usize) -> bool {
        self.specs
            .values()
            .filter(|(i, _)| *i == si)
            .any(|(_, s)| self.ns_capturable(&s.ns))
    }

    fn ns_capturable(&self, ns: &str) -> bool {
        if is_config_ns(ns) && !self.config_writable() {
            return false;
        }
        if ns == CFG_NS {
            return self.cfg_any_on();
        }
        self.syncs_here(ns, None)
    }

    /// Whether this tick must not capture `ns` because `Config::load` reset
    /// it (§8.10): the synced values are projected over it instead.
    fn reset_guarded(&self, ns: &str, id: &str) -> bool {
        if self.rt.reset_handled || !is_config_ns(ns) {
            return false;
        }
        match self.env.config.load_was_reset() {
            LoadReset::None => false,
            LoadReset::All => true,
            LoadReset::Sections(sections) => {
                let path = if ns == CFG_NS {
                    id.to_string()
                } else {
                    catalog::list_spec(ns).map_or("", |l| l.path).to_string()
                };
                let top = path.split('.').next().unwrap_or_default();
                sections.iter().any(|s| s == top)
            }
        }
    }

    /// Capture one read of a source. Returns how many cells and
    /// tombstones it wrote, and the list items whose local id must change
    /// (a lost id matched a record: both ids are named; the caller
    /// projects them).
    pub(crate) fn capture_state(
        &mut self,
        si: usize,
        state: &LocalState,
        origin: Origin,
    ) -> anyhow::Result<(usize, BTreeSet<(String, String)>)> {
        let specs: Vec<NsSpec> = self
            .specs
            .values()
            .filter(|(i, _)| *i == si)
            .map(|(_, s)| s.clone())
            .collect();
        let mut total = 0;
        let mut prefs_changed = false;
        let mut remapped = BTreeSet::new();
        let installed = installed_ids(&*self.env.config);
        let active_conn = specs
            .iter()
            .any(|s| s.ns == "list.connections")
            .then(|| local_active_connection(&*self.env.config))
            .flatten();
        let mut by_ns: BTreeMap<&str, BTreeMap<String, Option<&LocalRecord>>> = BTreeMap::new();
        for ((ns, id), r) in &state.records {
            by_ns
                .entry(ns.as_str())
                .or_default()
                .insert(id.clone(), r.as_ref());
        }
        for spec in specs {
            if !self.ns_capturable(&spec.ns) {
                continue;
            }
            let recs = by_ns.remove(spec.ns.as_str()).unwrap_or_default();
            let out = self.capture_ns(&spec, &recs, origin, &installed, active_conn.as_deref())?;
            total += out.written;
            remapped.extend(out.remapped);
            for (key, h) in out.approvals {
                if self.prefs.approved_runnables.get(&key) != Some(&h) {
                    self.prefs.approved_runnables.insert(key, h);
                    prefs_changed = true;
                }
            }
            if out.written > 0 {
                let now = self.now();
                self.rt.first_own_ms.get_or_insert(now);
                self.rt.last_own_ms = Some(now);
            }
        }
        if prefs_changed {
            self.save_prefs()?;
        }
        Ok((total, remapped))
    }

    /// Capture one namespace in one transaction.
    fn capture_ns(
        &mut self,
        spec: &NsSpec,
        local: &BTreeMap<String, Option<&LocalRecord>>,
        origin: Origin,
        installed: &BTreeSet<String>,
        active_conn: Option<&str>,
    ) -> anyhow::Result<CaptureOut> {
        let now = self.now();
        let os = self.env.os;
        let ns = spec.ns.clone();
        let mut skip: BTreeSet<String> = BTreeSet::new();
        for id in local.keys() {
            if ns == CFG_NS && (!self.syncs_here(&ns, Some(id)) || self.reset_guarded(&ns, id)) {
                skip.insert(id.clone());
            }
        }
        if ns != CFG_NS
            && local
                .keys()
                .next()
                .is_some_and(|id| self.reset_guarded(&ns, id))
        {
            return Ok(CaptureOut::default());
        }
        let frozen = self.rt.frozen.clone();
        let keep_order = self.rt.keep_synced_order;
        let approved = self.prefs.approved_runnables.clone();
        let ro_check = self.specs.get(&ns).map_or(u32::MAX, |(_, s)| s.schema);
        let Some(core) = self.core.as_mut() else {
            return Ok(CaptureOut::default());
        };
        let Some(keys) = core.keys.get(&core.epoch).cloned() else {
            return Ok(CaptureOut::default());
        };
        let me = core.me;
        let Core { replica, hlc, .. } = core;
        let mut churn_hits: Vec<(String, String, String)> = Vec::new();
        let out = replica.tx(|tx| {
            if tx
                .meta(&format!("schema.{ns}"))?
                .and_then(|v| v.parse::<u32>().ok())
                .is_some_and(|seen| seen > ro_check)
            {
                return Ok(CaptureOut::default());
            }
            let mut out = CaptureOut::default();
            let mut cx = Cx {
                tx,
                me,
                os,
                now,
                hlc,
                keys: &keys,
                origin,
                spec,
            };
            let mut projected: BTreeMap<String, BTreeMap<String, Projected>> = BTreeMap::new();
            for (k, p) in cx.tx.projected_in(&ns)? {
                projected.entry(k.id).or_default().insert(k.field, p);
            }
            let mut ids: BTreeSet<String> = local.keys().cloned().collect();
            ids.extend(projected.keys().cloned());
            // Lost ids (§4.4): local items no record knows, and records no
            // local item holds any more.
            let mut alias: BTreeMap<String, String> = BTreeMap::new();
            if catalog::list_spec(&ns).is_some() {
                alias = rematch_lost(&mut cx, local, &projected)?;
                for (orphan, rid) in &alias {
                    out.remapped.insert((ns.clone(), rid.clone()));
                    out.remapped.insert((ns.clone(), orphan.clone()));
                }
            }
            for id in &ids {
                if skip.contains(id) || alias.contains_key(id) {
                    continue;
                }
                let target_id = id;
                let local_rec = match alias.iter().find(|(_, r)| *r == id) {
                    Some((orphan, _)) => local.get(orphan).copied().flatten(),
                    None => local.get(id).copied().flatten(),
                };
                let proj = projected.get(target_id).cloned().unwrap_or_default();
                if ns == CFG_NS {
                    if let Some(ext) =
                        target_id.strip_prefix(super::sources::config::EXTENSION_STATE)
                    {
                        if !installed.contains(ext) {
                            // Uninstalling is not deleting (§8.2).
                            not_installed(&mut cx, target_id, &proj)?;
                            continue;
                        }
                    }
                }
                let wrote = capture_record(
                    &mut cx,
                    target_id,
                    local_rec,
                    &proj,
                    &frozen,
                    &mut out,
                    &mut churn_hits,
                )?;
                if wrote {
                    out.written += 1;
                }
                if let Some(l) = local_rec {
                    if let Some(h) = super::guards::approval_of(
                        &ns,
                        target_id,
                        l,
                        active_conn,
                        &approved,
                        &*cx.tx,
                    )? {
                        out.approvals
                            .push((super::guards::guard_key(&ns, target_id), h));
                    }
                }
            }
            if catalog::list_spec(&ns).is_some() && !keep_order {
                out.written += capture_order(&mut cx, local, &alias)?;
            }
            cx.tx.set_meta("hlc_last", &cx.hlc.last().to_string())?;
            Ok(out)
        })?;
        self.note_churn(churn_hits, me, now);
        for (ns, n) in &out.tombstones {
            self.check_outgoing_mass(ns, *n)?;
        }
        Ok(out)
    }

    /// Count changes of a cell; one that keeps changing from two computers
    /// is frozen for this session (§8.10).
    pub(crate) fn note_churn(
        &mut self,
        hits: Vec<(String, String, String)>,
        dev: DeviceId,
        now: i64,
    ) {
        for key in hits {
            let v = self.rt.churn.entry(key.clone()).or_default();
            v.retain(|(t, _)| now - t < 3_600_000);
            v.push((now, dev));
            let devs: BTreeSet<DeviceId> = v.iter().map(|(_, d)| *d).collect();
            if v.len() > 20 && devs.len() >= 2 && self.rt.frozen.insert(key.clone()) {
                log::warn!(
                    "sync: {}/{} {} kept changing on two computers; stopped syncing it this session",
                    key.0,
                    key.1,
                    key.2
                );
            }
        }
    }

    pub(crate) fn has_own_dirty(&self) -> bool {
        self.rt.first_own_ms.is_some()
    }

    fn publish_due(&self, now: i64, wake: Wake) -> bool {
        if self.rt.publish_paused_until.is_some_and(|t| t > now) {
            return false;
        }
        if let Some((t, _)) = self.rt.write_backoff {
            if t > now && wake != Wake::Manual {
                return false;
            }
        }
        if let (Some(first), Some(last)) = (self.rt.first_own_ms, self.rt.last_own_ms) {
            if matches!(wake, Wake::Manual | Wake::Exit)
                || now - last >= PUBLISH_QUIET_MS
                || now - first >= PUBLISH_MAX_WAIT_MS
            {
                return true;
            }
        }
        let low = matches!(
            self.env.clock.power(),
            crate::os::power::PowerState::LowBattery
        );
        let echo_due = self
            .rt
            .last_publish_ms
            .is_none_or(|t| now - t >= ECHO_EVERY_MS);
        let never = self.core.as_ref().is_some_and(|c| c.my_seq == 0);
        never || (echo_due && !low && self.rt.last_publish_ms.is_some()) || wake == Wake::Manual
    }

    // --- Projection (§8.6) --------------------------------------------------------

    /// Write the effective values of `changed` records (every record when
    /// `all`) into the local files. Returns how many records were written.
    pub(crate) fn project(
        &mut self,
        changed: &BTreeSet<(String, String)>,
        all: bool,
    ) -> anyhow::Result<usize> {
        if self.core.is_none() || self.core_locked() {
            return Ok(0);
        }
        let mut changed = changed.clone();
        changed.extend(self.refresh_guards()?);
        let changed = &changed;
        let mut by_source: BTreeMap<usize, BTreeSet<(String, String)>> = BTreeMap::new();
        for (ns, (si, _)) in &self.specs {
            if !self.ns_capturable(ns) {
                continue;
            }
            let set: BTreeSet<(String, String)> =
                changed.iter().filter(|(n, _)| n == ns).cloned().collect();
            if all || !set.is_empty() {
                by_source.entry(*si).or_default().extend(set);
                if all {
                    by_source
                        .entry(*si)
                        .or_default()
                        .insert((ns.clone(), String::new()));
                }
            }
        }
        let mut n = 0;
        for (si, set) in by_source {
            n += self.project_source(si, &set, all)?;
        }
        if !self.rt.reset_handled {
            self.rt.reset_handled = true;
            if self.env.config.load_was_reset() != LoadReset::None {
                // The synced values were put back over a reset config.
                self.rt.reset_notice = true;
            }
        }
        Ok(n)
    }

    fn project_source(
        &mut self,
        si: usize,
        changed: &BTreeSet<(String, String)>,
        all: bool,
    ) -> anyhow::Result<usize> {
        for _attempt in 0..3 {
            let base = self.sources[si].read_local()?;
            // Every read is captured, so an edit made since the last look
            // is recorded before anything is written over it.
            let (_, remapped) = self.capture_state(si, &base, Origin::User)?;
            let mut changed = changed.clone();
            changed.extend(remapped.iter().cloned());
            let writes = self.compute_writes(si, &base, &changed, all, &remapped)?;
            if writes.is_empty() {
                return Ok(0);
            }
            let paths = applied_paths(&writes);
            if !paths.is_empty() {
                self.env.events.applied(&paths);
            }
            match self.sources[si].project(writes.clone(), Origin::Sync)? {
                ProjectResult::Applied { read_back } => {
                    kill::point("after_config_commit_before_projected");
                    self.after_project(&writes, &read_back)?;
                    self.notify_ext_changes(&writes);
                    return Ok(writes.len());
                }
                ProjectResult::Stale => continue,
            }
        }
        log::warn!("sync: the local files kept changing during a projection; trying next tick");
        Ok(0)
    }

    /// Extension data written by keys or lists mode reaches the extension.
    fn notify_ext_changes(&self, writes: &[LocalWrite]) {
        let mut by_ext: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for w in writes {
            if let Some(ext) = ext_of_ns(&w.ns) {
                if w.ns.contains(".k") || w.ns.contains(".l.") {
                    by_ext.entry(ext).or_default().push(w.id.clone());
                }
            }
        }
        for (ext, keys) in by_ext {
            self.env.events.extension_data_changed(ext, &keys);
        }
    }

    /// The writes for one source's changed records.
    fn compute_writes(
        &mut self,
        si: usize,
        base: &LocalState,
        changed: &BTreeSet<(String, String)>,
        all: bool,
        remapped: &BTreeSet<(String, String)>,
    ) -> anyhow::Result<Vec<LocalWrite>> {
        let specs: Vec<NsSpec> = self
            .specs
            .values()
            .filter(|(i, _)| *i == si)
            .map(|(_, s)| s.clone())
            .collect();
        let os = self.env.os;
        let installed = installed_ids(&*self.env.config);
        let rules = crate::config_export::sensitive::Rules::new(&self.env.config.installed());
        let (_, local_cfg) = self.env.config.snapshot();
        let approved = self.prefs.approved_runnables.clone();
        let mut writes = Vec::new();
        let mut holds: Vec<(String, usize)> = Vec::new();
        for spec in specs {
            let ns = spec.ns.clone();
            if !self.ns_capturable(&ns) {
                continue;
            }
            let wanted: BTreeSet<String> = changed
                .iter()
                .filter(|(n, _)| *n == ns)
                .map(|(_, id)| id.clone())
                .filter(|id| !id.is_empty())
                .collect();
            let whole_ns = all
                || changed.contains(&(ns.clone(), String::new()))
                || (catalog::list_spec(&ns).is_some() && !wanted.is_empty());
            if !whole_ns && wanted.is_empty() {
                continue;
            }
            let cfg_skip: BTreeSet<String> = if ns == CFG_NS {
                base.records
                    .keys()
                    .filter(|(n, _)| *n == ns)
                    .map(|(_, id)| id.clone())
                    .filter(|id| !self.syncs_here(&ns, Some(id)))
                    .collect()
            } else {
                BTreeSet::new()
            };
            let mass_held = self.mass_hold(&ns)?;
            let held_accept = mass_held.as_ref().is_some_and(|h| h.accepted);
            let Some(core) = self.core.as_mut() else {
                return Ok(Vec::new());
            };
            let Core { replica, .. } = core;
            let (ns_writes, deletes) = replica.tx(|tx| {
                let ro = tx
                    .meta(&format!("schema.{ns}"))?
                    .and_then(|v| v.parse::<u32>().ok())
                    .is_some_and(|seen| seen > spec.schema);
                if ro {
                    return Ok((Vec::new(), 0usize));
                }
                let mut ids: BTreeSet<String> = if whole_ns {
                    tx.records(&ns)?.into_iter().map(|r| r.id).collect()
                } else {
                    wanted.clone()
                };
                if whole_ns {
                    ids.extend(
                        base.records
                            .keys()
                            .filter(|(n, _)| *n == ns)
                            .map(|(_, id)| id.clone()),
                    );
                }
                let mut projected: BTreeMap<String, BTreeMap<String, Projected>> = BTreeMap::new();
                for (k, p) in tx.projected_in(&ns)? {
                    projected.entry(k.id).or_default().insert(k.field, p);
                }
                let is_list = catalog::list_spec(&ns).is_some();
                let mut out = Vec::new();
                let mut deletes = 0usize;
                let mut list_changed = false;
                for id in ids {
                    if cfg_skip.contains(&id) {
                        continue;
                    }
                    if tx.pin(&LocalKey::new(&ns, &id, "*"))?.is_some() {
                        continue;
                    }
                    let local = base.get(&ns, &id);
                    let proj = projected.get(&id);
                    let Some(rec) = tx.record(&ns, &id)? else {
                        // A lost id matched another record: the local item
                        // under this id goes; the record is written under
                        // its own id.
                        if local.is_some() && remapped.contains(&(ns.clone(), id.clone())) {
                            out.push(LocalWrite {
                                ns: ns.clone(),
                                id: id.clone(),
                                value: None,
                            });
                            list_changed = true;
                        }
                        continue;
                    };
                    if ns == CFG_NS {
                        if let Some(ext) = id.strip_prefix(super::sources::config::EXTENSION_STATE)
                        {
                            if !installed.contains(ext) {
                                continue;
                            }
                        }
                    }
                    let exists = is_visible(&rec) && scope::exists_here(&rec, os);
                    if !exists {
                        if local.is_some() && proj.is_some_and(|p| !p.is_empty()) {
                            deletes += 1;
                            out.push(LocalWrite {
                                ns: ns.clone(),
                                id: id.clone(),
                                value: None,
                            });
                            list_changed = true;
                        }
                        continue;
                    }
                    let pins = pins_of(tx, &ns, &id)?;
                    let mut target = effective_record(&spec, &rec, local, &pins, os);
                    super::guards::apply(
                        tx,
                        &ns,
                        &id,
                        &mut target,
                        local,
                        &rec,
                        &local_cfg,
                        &installed,
                        &approved,
                        &rules,
                        os,
                    )?;
                    let differs = match local {
                        None => true,
                        Some(l) => !same_record(&target, l),
                    };
                    if differs {
                        list_changed = true;
                    }
                    if differs || is_list {
                        out.push(LocalWrite {
                            ns: ns.clone(),
                            id: id.clone(),
                            value: Some(target),
                        });
                    } else {
                        // Unchanged: point `projected` at the cells the
                        // value comes from, so the next edit's base is right.
                        repoint_projected(
                            tx,
                            &spec,
                            &rec,
                            &id,
                            local.unwrap_or(&target),
                            &pins,
                            os,
                        )?;
                    }
                }
                if is_list && !list_changed && !same_order(&out, base, &ns) {
                    list_changed = true;
                }
                if is_list && !list_changed {
                    out.clear();
                }
                Ok((out, deletes))
            })?;
            let count = self.namespace_count(&ns)?;
            let mut ns_writes = ns_writes;
            if !held_accept
                && (mass_held.is_some() || super::guards::is_mass_change(deletes, count))
                && deletes > 0
            {
                holds.push((ns.clone(), deletes));
                // Held: the deletes wait for the user's answer.
                let removed: BTreeSet<String> = ns_writes
                    .iter()
                    .filter(|w| w.value.is_none())
                    .map(|w| w.id.clone())
                    .collect();
                ns_writes.retain(|w| w.value.is_some());
                if catalog::list_spec(&ns).is_some() {
                    // Keep the held items in the rebuilt list as they are.
                    for id in removed {
                        if let Some(l) = base.get(&ns, &id) {
                            ns_writes.push(LocalWrite {
                                ns: ns.clone(),
                                id,
                                value: Some(as_list_write(l)),
                            });
                        }
                    }
                }
            } else if held_accept && deletes > 0 {
                self.trash_copy(
                    &ns,
                    ns_writes
                        .iter()
                        .filter(|w| w.value.is_none())
                        .map(|w| w.id.clone())
                        .collect(),
                )?;
            }
            writes.extend(ns_writes);
        }
        for (ns, n) in holds {
            self.hold_incoming(&ns, n)?;
        }
        Ok(writes)
    }

    fn namespace_count(&mut self, ns: &str) -> anyhow::Result<u64> {
        let Some(core) = self.core.as_mut() else {
            return Ok(0);
        };
        core.replica.tx(|tx| tx.count_records(ns))
    }

    /// After a projection: `projected` holds the hash of what the files
    /// read back, and a value that didn't survive the round trip is pinned
    /// as lossy, so it is never exported back (§8.6).
    fn after_project(
        &mut self,
        writes: &[LocalWrite],
        read_back: &LocalState,
    ) -> anyhow::Result<()> {
        let os = self.env.os;
        let specs = self.specs.clone();
        let me = self.core.as_ref().map(|c| c.me);
        let now = self.now();
        let mut churn = Vec::new();
        let Some(core) = self.core.as_mut() else {
            return Ok(());
        };
        core.replica.tx(|tx| {
            let mut proj_before: BTreeMap<(String, String), [u8; 32]> = BTreeMap::new();
            for ns in writes
                .iter()
                .map(|w| w.ns.as_str())
                .collect::<BTreeSet<_>>()
            {
                for (k, p) in tx.projected_in(ns)? {
                    proj_before.insert((k.id, k.field), p.0);
                }
                if catalog::list_spec(ns).is_some() {
                    set_known_order(tx, ns, &local_order(read_back, ns))?;
                }
            }
            for w in writes {
                let Some((_, spec)) = specs.get(&w.ns) else {
                    continue;
                };
                let rb = read_back.get(&w.ns, &w.id);
                let Some(target) = &w.value else {
                    if rb.is_none() {
                        drop_projected(tx, &w.ns, &w.id)?;
                    }
                    continue;
                };
                let Some(rec) = tx.record(&w.ns, &w.id)? else {
                    continue;
                };
                let pins = pins_of(tx, &w.ns, &w.id)?;
                let t_split = split_fields(&target.fields, spec);
                let rb_split = rb
                    .map(|r| split_fields(&r.fields, spec))
                    .unwrap_or_default();
                let mut names: BTreeSet<String> = t_split.keys().cloned().collect();
                names.extend(rb_split.keys().cloned());
                names.extend(cell_names(&rec));
                for name in names {
                    if name == AVAIL || name == ORDER {
                        continue;
                    }
                    let tv = t_split
                        .get(&name)
                        .cloned()
                        .unwrap_or_else(|| absent_value(&name));
                    let rbv = match rb_split.get(&name) {
                        Some(v) if is_unchanged(v) => tv.clone(),
                        Some(v) => v.clone(),
                        None => absent_value(&name),
                    };
                    let key = LocalKey::new(&w.ns, &w.id, &name);
                    let pinned = pins.contains_key(&name);
                    let (stamp, scope) = if pinned {
                        (None, None)
                    } else {
                        eff_source(&rec, &name, os)
                    };
                    tx.set_projected(&key, Some((hash(&rbv), stamp, scope)))?;
                    // A value that was written but didn't survive the round
                    // trip: this build can't hold it. A field written as
                    // absent may read back as its type's default; that's
                    // no loss.
                    if !pinned && t_split.contains_key(&name) && !same(&rbv, &tv) && rb.is_some() {
                        tx.set_pin(&key, Some((rbv.clone(), PinReason::Lossy)))?;
                    }
                    if let (Some(s), Some(me)) = (stamp, me) {
                        let was = proj_before
                            .get(&(w.id.clone(), name.clone()))
                            .is_some_and(|h| *h != hash(&rbv));
                        if s.dev != me && was {
                            churn.push(((w.ns.clone(), w.id.clone(), name.clone()), s.dev));
                        }
                    }
                }
                if rec.cells.contains_key(&FieldKey::all(ORDER)) {
                    let c = &rec.cells[&FieldKey::all(ORDER)];
                    tx.set_projected(
                        &LocalKey::new(&w.ns, &w.id, ORDER),
                        Some((hash(&c.value), Some(c.stamp), Some(Scope::All))),
                    )?;
                }
            }
            Ok(())
        })?;
        for (hit, dev) in churn {
            // A remote change counts for the computer that made it.
            self.note_churn(vec![hit], dev, now);
        }
        Ok(())
    }

    // --- Requests -------------------------------------------------------------------

    fn dispatch(&mut self, req: EngineRequest) -> Result<EngineReply, AppError> {
        use EngineRequest as R;
        if self.other_engine && !matches!(req, R::Status) {
            return Err(super::unavailable());
        }
        match req {
            R::Status => Ok(EngineReply::Status(self.build_status())),
            R::SuggestFolders => Ok(EngineReply::Folders(self.env.cloud.suggest_folders())),
            R::InspectFolder(p) => Ok(EngineReply::Folder(super::folder::inspect(
                &*self.env.fs,
                &*self.env.cloud,
                &p,
            ))),
            R::GeneratePassphrase => {
                Ok(EngineReply::Passphrase(super::crypto::generate_passphrase()))
            }
            R::Create(c) => self.create(c).map(EngineReply::Status),
            R::Join(j) => self.join(j).map(EngineReply::JoinPreview),
            R::ConfirmJoin(choice) => self.confirm_join(choice).map(EngineReply::Status),
            R::Unlock { passphrase } => self.unlock(&passphrase).map(EngineReply::Status),
            R::SetPaused { until } => {
                self.require_on()?;
                self.prefs.paused_until = until;
                self.save_prefs().map_err(internal)?;
                Ok(EngineReply::Done)
            }
            R::SyncNow => {
                self.require_on()?;
                self.tick(Wake::Manual);
                Ok(EngineReply::Done)
            }
            R::SetCategory { category, on } => {
                self.require_on()?;
                self.prefs.set_category(category, on);
                self.save_prefs().map_err(internal)?;
                if on {
                    self.rt.first_tick_done = false;
                    self.read_heads_again().map_err(internal)?;
                }
                Ok(EngineReply::Done)
            }
            R::SetExtensionData { ext, on } => {
                self.require_on()?;
                self.prefs.ext_data_off.retain(|e| *e != ext);
                if !on {
                    self.prefs.ext_data_off.push(ext);
                }
                self.save_prefs().map_err(internal)?;
                if on {
                    self.rt.first_tick_done = false;
                    self.read_heads_again().map_err(internal)?;
                }
                Ok(EngineReply::Done)
            }
            R::GetScopes(paths) => self.get_scopes(paths).map(EngineReply::Scopes),
            R::SetScope { path, action } => self.set_scope(&path, action).map(EngineReply::Scope),
            R::ListConflicts(tab) => self.list_conflicts(tab).map(EngineReply::Conflicts),
            R::ConflictDetail(id) => self.conflict_detail(&id).map(EngineReply::ConflictDetail),
            R::ResolveConflict { id, action } => {
                self.resolve_conflict(&id, action)?;
                Ok(EngineReply::Done)
            }
            R::DismissAll(group) => {
                self.dismiss_all(group.as_deref())?;
                Ok(EngineReply::Done)
            }
            R::ListReviews => self.list_reviews().map(EngineReply::Reviews),
            R::ApproveRunnable(key) => {
                self.approve_runnable(&key, true)?;
                Ok(EngineReply::Done)
            }
            R::KeepOff(key) => {
                self.approve_runnable(&key, false)?;
                Ok(EngineReply::Done)
            }
            R::DismissSuggestion { kind, id } => {
                self.require_on()?;
                let key = format!("{kind}:{id}");
                if !self.prefs.install_exclusions.contains(&key) {
                    self.prefs.install_exclusions.push(key);
                }
                self.save_prefs().map_err(internal)?;
                Ok(EngineReply::Done)
            }
            R::AnswerMassChange { ns, accept } => {
                self.answer_mass_change(&ns, accept)?;
                Ok(EngineReply::Done)
            }
            R::ListDevices => self.list_devices().map(EngineReply::Devices),
            R::RenameDevice(name) => {
                self.rename_device(name)?;
                Ok(EngineReply::Done)
            }
            R::RemoveDevice {
                id,
                lost,
                new_passphrase,
            } => {
                self.remove_device(&id, lost, new_passphrase)?;
                Ok(EngineReply::Done)
            }
            R::ChangePassphrase(p) => {
                self.change_passphrase(&p)?;
                Ok(EngineReply::Done)
            }
            R::Leave => {
                self.leave()?;
                Ok(EngineReply::Done)
            }
            R::StartOver { passphrase } => self.start_over(&passphrase).map(EngineReply::Status),
            R::UndoJoin => {
                self.undo_join()?;
                Ok(EngineReply::Done)
            }
            R::PurgeExtension(ext) => {
                self.purge_extension(&ext)?;
                Ok(EngineReply::Done)
            }
            R::FindFolder => self.find_folder().map(EngineReply::FoundFolder),
            R::ApplyFolder(p) => {
                self.apply_folder(p)?;
                Ok(EngineReply::Done)
            }
            R::Ext(e) => self.ext_request(e).map(EngineReply::Ext),
        }
    }

    /// Read every computer's newest head again on the next ingest: a
    /// namespace switched on here was skipped when those heads were read.
    /// Shards already merged are skipped by name.
    fn read_heads_again(&mut self) -> anyhow::Result<()> {
        let Some(core) = self.core.as_mut() else {
            return Ok(());
        };
        core.replica.tx(|tx| {
            for mut d in tx.devices()? {
                d.ingested_seq = 0;
                tx.put_device(&d)?;
            }
            Ok(())
        })
    }

    /// `sync_unavailable` unless sync is on here.
    pub(crate) fn require_on(&self) -> Result<(), AppError> {
        if self.prefs.enabled && self.core.is_some() {
            Ok(())
        } else {
            Err(super::unavailable())
        }
    }

    /// Collections-mode requests go to the source that handles them
    /// (WP-S5's extension source), with a capture context over one
    /// transaction.
    fn ext_request(&mut self, req: super::ExtRequest) -> Result<super::ExtReply, AppError> {
        self.require_on()?;
        if self.core_locked() {
            return Err(super::unavailable());
        }
        let now = self.now();
        let os = self.env.os;
        let specs = self.specs.clone();
        let Some(core) = self.core.as_mut() else {
            return Err(super::unavailable());
        };
        let keys = core.cur_keys().cloned().ok_or_else(super::unavailable)?;
        let me = core.me;
        let Core { replica, hlc, .. } = core;
        let sources = &mut self.sources;
        let result: anyhow::Result<Option<Result<super::ExtReply, AppError>>> = replica.tx(|tx| {
            for s in sources.iter_mut() {
                let mut ctx = ExtCtx {
                    tx: &mut *tx,
                    hlc: &mut *hlc,
                    keys: &keys,
                    me,
                    os,
                    now,
                    specs: &specs,
                };
                if let Some(r) = s.request(&mut ctx, &req) {
                    let hlc_last = ctx.hlc.last();
                    tx.set_meta("hlc_last", &hlc_last.to_string())?;
                    return Ok(Some(r));
                }
            }
            Ok(None)
        });
        match result.map_err(internal)? {
            Some(r) => {
                self.rt.first_own_ms.get_or_insert(now);
                self.rt.last_own_ms = Some(now);
                r
            }
            None => Err(super::unavailable()),
        }
    }
}

/// The stronger of two origins: an import or a reset changes how a
/// capture is recorded.
fn stronger(a: Option<Origin>, b: Origin) -> Origin {
    let rank = |o: Origin| match o {
        Origin::Import => 3,
        Origin::Reset => 2,
        Origin::Join => 1,
        Origin::User | Origin::Sync => 0,
    };
    match a {
        Some(a) if rank(a) >= rank(b) => a,
        _ => b,
    }
}

pub(crate) fn is_disk_full(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::StorageFull)
            || c.downcast_ref::<rusqlite::Error>().is_some_and(|s| {
                matches!(s, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::DiskFull)
            })
    })
}

/// Namespaces stored in `config.json`.
pub(crate) fn is_config_ns(ns: &str) -> bool {
    ns == CFG_NS || ns.starts_with("list.")
}

/// The extension an `ext.<id>.*` namespace belongs to.
pub(crate) fn ext_of_ns(ns: &str) -> Option<&str> {
    let rest = ns.strip_prefix("ext.")?;
    let (id, _) = rest.split_once('.')?;
    Some(id)
}

/// The ids of every installed extension, theme and command pack.
pub(crate) fn installed_ids(port: &dyn ConfigPort) -> BTreeSet<String> {
    port.installed()
        .into_iter()
        .map(|i| i.manifest.id)
        .collect()
}

/// The id of this computer's active agent connection, from `config.json`.
fn local_active_connection(port: &dyn ConfigPort) -> Option<String> {
    let (_, c) = port.snapshot();
    Some(c.acp.active_connection_id).filter(|s| !s.is_empty())
}

/// The catalog paths a projection is about to write, for `sync_applied`.
fn applied_paths(writes: &[LocalWrite]) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    for w in writes {
        if w.ns == CFG_NS {
            out.insert(w.id.clone());
        } else if let Some(l) = catalog::list_spec(&w.ns) {
            out.insert(format!("{}/{}", l.path, w.id));
        }
    }
    out.into_iter().collect()
}

/// The field a cell name belongs to: `f` of `f`, `+f`, `f[]x`, `f{}k`.
pub(crate) fn base_field(name: &str) -> &str {
    match split::parse_name(name) {
        split::CellName::Whole(f) | split::CellName::Group(f) => f,
        split::CellName::Element { field, .. } | split::CellName::Entry { field, .. } => field,
    }
}

/// The scope a new value of `name` goes to by default: `@<id>` or the
/// field's name in [`super::types::Structure::per_os`] makes it per OS.
pub(crate) fn default_scope(spec: &NsSpec, id: &str, name: &str) -> DefaultScope {
    let field = base_field(name);
    if spec
        .structure
        .per_os
        .iter()
        .any(|f| f == field || f.strip_prefix('@') == Some(id))
    {
        DefaultScope::PerOs
    } else {
        spec.default_scope
    }
}

/// A local record's fields as cells, without `order`.
pub(crate) fn split_fields(fields: &Map<String, Value>, spec: &NsSpec) -> BTreeMap<String, Value> {
    let mut f = fields.clone();
    f.remove(ORDER);
    split::split(&f, &spec.structure)
}

/// Every cell name a record has, at any scope.
pub(crate) fn cell_names(r: &Record) -> BTreeSet<String> {
    r.cells
        .keys()
        .map(|k| k.name.clone())
        .filter(|n| n != AVAIL && n != ORDER)
        .collect()
}

/// The pins of one record, by cell name.
pub(crate) fn pins_of(
    tx: &Tx<'_>,
    ns: &str,
    id: &str,
) -> anyhow::Result<BTreeMap<String, (Value, PinReason)>> {
    Ok(tx
        .pins(Some(ns))?
        .into_iter()
        .filter(|(k, _, _)| k.id == id && k.field != "*")
        .map(|(k, v, r)| (k.field, (v, r)))
        .collect())
}

/// The value of `name` on this OS from the cells (§7.5, with explicit
/// `null`s): `None` when no cell for this OS or all computers exists (the
/// local value stays, R4); `Some(Null)` when cells say "no value".
pub(crate) fn eff_cell<'a>(r: &'a Record, name: &str, os: OsKind) -> Option<&'a Cell> {
    let os_cell = r.cell(name, Scope::Os(os));
    let all = r.cell(name, Scope::All);
    match (os_cell, all) {
        (Some(c), _) if !c.value.is_null() => Some(c),
        (_, Some(c)) => Some(c),
        (Some(c), None) => Some(c),
        (None, None) => None,
    }
}

/// The stamp and scope the effective value of `name` comes from.
pub(crate) fn eff_source(r: &Record, name: &str, os: OsKind) -> (Option<Stamp>, Option<Scope>) {
    match eff_cell(r, name, os) {
        Some(c) => {
            let scope = if r.cell(name, Scope::Os(os)).map(|x| x.stamp) == Some(c.stamp) {
                Scope::Os(os)
            } else {
                Scope::All
            };
            (Some(c.stamp), Some(scope))
        }
        None => (None, None),
    }
}

/// The record as this computer should hold it: the local fields, with
/// every field the cells (or a pin) decide replaced, and `order` as its
/// key.
pub(crate) fn effective_record(
    spec: &NsSpec,
    rec: &Record,
    local: Option<&LocalRecord>,
    pins: &BTreeMap<String, (Value, PinReason)>,
    os: OsKind,
) -> LocalRecord {
    let mut cells: BTreeMap<String, Value> = local
        .map(|l| split_fields(&l.fields, spec))
        .unwrap_or_default();
    // A field the source says is unchanged has nothing to keep.
    cells.retain(|_, v| !is_unchanged(v));
    let mut names = cell_names(rec);
    names.extend(pins.keys().cloned());
    for name in names {
        let v = match pins.get(&name) {
            Some((v, _)) => Some(v.clone()),
            None => eff_cell(rec, &name, os).map(|c| c.value.clone()),
        };
        match v {
            Some(v) if v.is_null() || v == Value::Bool(false) && split::is_union(&name) => {
                cells.remove(&name);
            }
            Some(v) => {
                cells.insert(name, v);
            }
            None => {}
        }
    }
    let mut fields = split::join(cells.iter().map(|(k, v)| (k.as_str(), v)), &spec.structure);
    if let Some(c) = rec.cell(ORDER, Scope::All) {
        fields.insert(ORDER.to_string(), c.value.clone());
    }
    LocalRecord {
        fields,
        avail: None,
    }
}

/// Whether a target record matches what the files hold (ignoring the
/// local position, which a list rebuild decides).
fn same_record(target: &LocalRecord, local: &LocalRecord) -> bool {
    let strip = |m: &Map<String, Value>| {
        let mut m = m.clone();
        m.remove(ORDER);
        m.retain(|_, v| !is_unchanged(v));
        Value::Object(m)
    };
    let mut t = strip(&target.fields);
    if let (Value::Object(tm), Some(lf)) = (&mut t, Some(&local.fields)) {
        // A field the source didn't load is the same as before.
        for (k, v) in lf {
            if is_unchanged(v) {
                tm.remove(k);
            }
        }
    }
    same(&t, &strip(&local.fields))
}

/// Whether a list's items are already in the order the writes give them:
/// local positions against `(order key, id)`.
fn same_order(writes: &[LocalWrite], base: &LocalState, ns: &str) -> bool {
    let mut target: Vec<(String, &str)> = writes
        .iter()
        .filter_map(|w| {
            let key = w.value.as_ref()?.fields.get(ORDER)?.as_str()?.to_string();
            Some((key, w.id.as_str()))
        })
        .collect();
    target.sort();
    let mut local: Vec<(u64, &str)> = target
        .iter()
        .filter_map(|(_, id)| {
            let pos = base.get(ns, id)?.fields.get(ORDER)?.as_u64()?;
            Some((pos, *id))
        })
        .collect();
    local.sort();
    let t: Vec<&str> = target.iter().map(|(_, id)| *id).collect();
    let l: Vec<&str> = local.iter().map(|(_, id)| *id).collect();
    t == l
}

/// A local item as a list write: its fields, with its current key.
fn as_list_write(l: &LocalRecord) -> LocalRecord {
    let mut l = l.clone();
    l.fields.remove(ORDER);
    l
}

/// Point `projected` at the cells an unchanged local record's values come
/// from.
fn repoint_projected(
    tx: &mut Tx<'_>,
    spec: &NsSpec,
    rec: &Record,
    id: &str,
    local: &LocalRecord,
    pins: &BTreeMap<String, (Value, PinReason)>,
    os: OsKind,
) -> anyhow::Result<()> {
    let cells = split_fields(&local.fields, spec);
    let mut names = cell_names(rec);
    names.extend(cells.keys().cloned());
    for name in names {
        let v = cells
            .get(&name)
            .cloned()
            .unwrap_or_else(|| absent_value(&name));
        if is_unchanged(&v) {
            continue;
        }
        let (stamp, scope) = if pins.contains_key(&name) {
            (None, None)
        } else {
            eff_source(rec, &name, os)
        };
        tx.set_projected(
            &LocalKey::new(&spec.ns, id, &name),
            Some((hash(&v), stamp, scope)),
        )?;
    }
    Ok(())
}

pub(crate) fn drop_projected(tx: &mut Tx<'_>, ns: &str, id: &str) -> anyhow::Result<()> {
    for (k, _) in tx.projected_in(ns)? {
        if k.id == id {
            tx.set_projected(&k, None)?;
        }
    }
    Ok(())
}

/// What capture borrows for one transaction.
pub(crate) struct Cx<'a, 'b> {
    pub tx: &'a mut Tx<'b>,
    pub me: DeviceId,
    pub os: OsKind,
    pub now: i64,
    pub hlc: &'a mut Hlc,
    pub keys: &'a Keys,
    pub origin: Origin,
    pub spec: &'a NsSpec,
}

impl Cx<'_, '_> {
    pub fn bucket(&self, ns: &str, id: &str) -> u32 {
        self.keys.bucket(ns, id, 1 << 16)
    }

    pub fn stamp(&mut self, floor: u64) -> Stamp {
        Stamp::new(self.hlc.tick(self.now, floor), self.me)
    }

    /// Store a record changed here.
    pub fn write(&mut self, r: &Record) -> anyhow::Result<()> {
        let b = match self.tx.bucket_of(&r.ns, &r.id)? {
            Some(b) => b,
            None => self.bucket(&r.ns, &r.id),
        };
        self.tx.write(r, b)
    }

    /// Record conflicts found here, as every detector writes them.
    pub fn record_conflicts(&mut self, bodies: &[ConflictBody]) -> anyhow::Result<usize> {
        if bodies.is_empty() {
            return Ok(0);
        }
        let keys = self.keys.clone();
        let f = move |ns: &str, id: &str| keys.bucket(ns, id, 1 << 16);
        let name_key = *self.keys.name_key();
        conflicts::record_detected(self.tx, &name_key, bodies, &f)
    }
}

/// A pin's answer to a local edit of its field.
enum PinEdit {
    /// The field stays pinned, now holding this value.
    Keep(Value),
    /// The pin goes; the edit is captured as usual.
    Release,
}

fn on_pinned_edit(reason: PinReason, ns: &str, id: &str, name: &str, v: &Value) -> PinEdit {
    match reason {
        PinReason::ThisComputer | PinReason::NotInstalled | PinReason::OriginMismatch => {
            PinEdit::Keep(v.clone())
        }
        PinReason::HeldBack => {
            if super::guards::held_back(ns, id, name, v) {
                PinEdit::Keep(v.clone())
            } else {
                PinEdit::Release
            }
        }
        // The user changed it here: a local edit (turning a held item on by
        // hand counts as approval, which capture records).
        PinReason::Lossy
        | PinReason::InvalidHere
        | PinReason::NewerSchema
        | PinReason::Consent
        | PinReason::NeedsApproval => PinEdit::Release,
    }
}

/// An extension that isn't installed here: its synced on/off is pinned
/// `not_installed` and capture writes nothing for it (§8.2).
fn not_installed(
    cx: &mut Cx<'_, '_>,
    id: &str,
    proj: &BTreeMap<String, Projected>,
) -> anyhow::Result<()> {
    let ns = cx.spec.ns.clone();
    if !proj.is_empty() {
        drop_projected(cx.tx, &ns, id)?;
    }
    if let Some(rec) = cx.tx.record(&ns, id)? {
        if let Some(c) = eff_cell(&rec, super::sources::config::FIELD, cx.os) {
            let key = LocalKey::new(&ns, id, super::sources::config::FIELD);
            if cx.tx.pin(&key)?.is_none() {
                cx.tx
                    .set_pin(&key, Some((c.value.clone(), PinReason::NotInstalled)))?;
            }
        }
    }
    Ok(())
}

/// Capture one record (§8.2). Returns whether it wrote anything.
fn capture_record(
    cx: &mut Cx<'_, '_>,
    id: &str,
    local: Option<&LocalRecord>,
    proj: &BTreeMap<String, Projected>,
    frozen: &HashSet<(String, String, String)>,
    out: &mut CaptureOut,
    churn: &mut Vec<(String, String, String)>,
) -> anyhow::Result<bool> {
    let ns = cx.spec.ns.clone();
    let star = LocalKey::new(&ns, id, "*");
    if let Some((_, reason)) = cx.tx.pin(&star)? {
        // The item lives only here: its pin holds the local copy.
        match local {
            Some(l) => cx
                .tx
                .set_pin(&star, Some((Value::Object(l.fields.clone()), reason)))?,
            None => cx.tx.set_pin(&star, None)?,
        }
        return Ok(false);
    }
    let mut rec = cx.tx.record(&ns, id)?;
    let Some(l) = local else {
        // Gone locally: a delete, if this computer showed it.
        if proj.is_empty() {
            return Ok(false);
        }
        let Some(mut r) = rec else {
            drop_projected(cx.tx, &ns, id)?;
            return Ok(false);
        };
        if !is_visible(&r) || !scope::exists_here(&r, cx.os) {
            drop_projected(cx.tx, &ns, id)?;
            return Ok(false);
        }
        let floor = r.max_stamp().map_or(0, |s| s.hlc);
        let mut seen: Vec<Stamp> = proj.values().filter_map(|p| p.1).collect();
        seen.sort_unstable_by(|a, b| b.cmp(a));
        seen.dedup();
        seen.truncate(MAX_ANCESTORS);
        let stamp = cx.stamp(floor);
        r.del = Some(Tomb {
            stamp,
            wall_ms: cx.now,
            anc: seen,
        });
        cx.write(&r)?;
        cx.tx.clear_acks(&ns, id)?;
        drop_projected(cx.tx, &ns, id)?;
        *out.tombstones.entry(ns.clone()).or_default() += 1;
        return Ok(true);
    };
    let cells_local = split_fields(&l.fields, cx.spec);
    let mut names: BTreeSet<String> = cells_local.keys().cloned().collect();
    names.extend(proj.keys().filter(|n| *n != ORDER && *n != AVAIL).cloned());
    let mut wrote = false;
    let mut found: Vec<ConflictBody> = Vec::new();
    for name in names {
        let lv = cells_local.get(&name);
        if lv.is_some_and(is_unchanged) {
            continue;
        }
        let v_local = lv.cloned().unwrap_or_else(|| absent_value(&name));
        let h = hash(&v_local);
        let p = proj.get(&name);
        if p.map(|p| p.0) == Some(h) {
            continue;
        }
        if frozen.contains(&(ns.clone(), id.to_string(), name.clone())) {
            continue;
        }
        let key = LocalKey::new(&ns, id, &name);
        if let Some((_, reason)) = cx.tx.pin(&key)? {
            match on_pinned_edit(reason, &ns, id, &name, &v_local) {
                PinEdit::Keep(v) => {
                    cx.tx.set_pin(&key, Some((v, reason)))?;
                    cx.tx.set_projected(&key, Some((h, None, None)))?;
                    continue;
                }
                PinEdit::Release => cx.tx.set_pin(&key, None)?,
            }
        }
        if super::guards::held_back(&ns, id, &name, &v_local) {
            cx.tx
                .set_pin(&key, Some((v_local.clone(), PinReason::HeldBack)))?;
            cx.tx.set_projected(&key, Some((h, None, None)))?;
            continue;
        }
        if lv.is_none() && p.is_none() {
            continue;
        }
        let r = rec.get_or_insert_with(|| Record::new(ns.clone(), id.to_string()));
        let default = default_scope(cx.spec, id, &name);
        let target = scope::edit_target(r, &name, cx.os, default);
        let base: Option<Cell> = p.and_then(|(_, s, sc)| {
            let s = (*s)?;
            let at = sc.unwrap_or(Scope::All);
            r.cells
                .get(&FieldKey::new(name.clone(), at))
                .filter(|c| c.stamp == s)
                .cloned()
                .or(Some(Cell {
                    value: Value::Null,
                    stamp: s,
                    wall_ms: 0,
                    anc: Vec::new(),
                }))
        });
        let cur = r.cells.get(&FieldKey::new(name.clone(), target)).cloned();
        if let Some(c) = &cur {
            if same(&c.value, &v_local) {
                cx.tx
                    .set_projected(&key, Some((h, Some(c.stamp), Some(target))))?;
                continue;
            }
        }
        if cur.is_none() && (v_local.is_null() || v_local == Value::Bool(false)) {
            cx.tx.set_projected(&key, Some((h, None, None)))?;
            continue;
        }
        let floor = base
            .as_ref()
            .map(|c| c.stamp)
            .into_iter()
            .chain(cur.as_ref().map(|c| c.stamp))
            .max()
            .map_or(0, |s| s.hlc);
        let stamp = cx.stamp(floor);
        let anc = ancestors(base.as_ref(), cx.me);
        let new = Cell {
            value: v_local.clone(),
            stamp,
            wall_ms: cx.now,
            anc: anc.clone(),
        };
        if let Some(c) = &cur {
            let unseen = base.as_ref().map(|b| b.stamp) != Some(c.stamp)
                && c.stamp.dev != cx.me
                && !split::is_union(&name)
                && !anc.contains(&c.stamp)
                && !ns.starts_with("sync.");
            let side = |c: &Cell, value: bool| Side {
                stamp: c.stamp,
                wall_ms: c.wall_ms,
                deleted: false,
                value: value.then(|| c.value.clone()),
            };
            if unseen {
                found.push(ConflictBody {
                    ns: ns.clone(),
                    rid: id.to_string(),
                    field: Some(FieldKey::new(name.clone(), target)),
                    kind: ConflictKind::ConcurrentEdit,
                    loser: side(c, true),
                    winner: Some(side(&new, false)),
                });
            } else if cx.origin == Origin::Import && !ns.starts_with("sync.") {
                // A restored backup: the replaced value can come back.
                let body = ConflictBody {
                    ns: ns.clone(),
                    rid: id.to_string(),
                    field: Some(FieldKey::new(name.clone(), target)),
                    kind: ConflictKind::RestoreBackup,
                    loser: side(c, true),
                    winner: Some(side(&new, false)),
                };
                let keys = cx.keys.clone();
                let f = move |ns: &str, id: &str| keys.bucket(ns, id, 1 << 16);
                let name_key = *cx.keys.name_key();
                conflicts::record_action(
                    cx.tx,
                    &name_key,
                    &body,
                    conflicts::OPEN,
                    cx.now,
                    cx.me,
                    cx.hlc,
                    &f,
                )?;
                out.conflicts += 1;
            }
        }
        r.cells.insert(FieldKey::new(name.clone(), target), new);
        cx.tx
            .set_projected(&key, Some((h, Some(stamp), Some(target))))?;
        churn.push((ns.clone(), id.to_string(), name.clone()));
        wrote = true;
    }
    if let Some(r) = rec.as_mut() {
        if wrote && catalog::list_spec(&ns).is_some() && r.cell(AVAIL, Scope::All).is_none() {
            let avail = l.avail.clone().unwrap_or_else(|| "all".to_string());
            let stamp = cx.stamp(0);
            r.cells.insert(
                FieldKey::all(AVAIL),
                Cell {
                    value: Value::String(avail),
                    stamp,
                    wall_ms: cx.now,
                    anc: Vec::new(),
                },
            );
        }
        if wrote {
            cx.write(r)?;
        }
    }
    out.conflicts += cx.record_conflicts(&found)?;
    Ok(wrote)
}

/// Match local items without a known record against records no local item
/// holds any more (§4.4 "Lost ids"). Returns `orphan id → record id`.
fn rematch_lost(
    cx: &mut Cx<'_, '_>,
    local: &BTreeMap<String, Option<&LocalRecord>>,
    projected: &BTreeMap<String, BTreeMap<String, Projected>>,
) -> anyhow::Result<BTreeMap<String, String>> {
    let ns = cx.spec.ns.clone();
    let mut orphans = BTreeMap::new();
    for (id, l) in local {
        let Some(l) = l else { continue };
        if projected.contains_key(id) || cx.tx.record(&ns, id)?.is_some() {
            continue;
        }
        orphans.insert(id.clone(), l.fields.clone());
    }
    if orphans.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut missing = BTreeMap::new();
    for id in projected.keys() {
        if local.get(id).is_some_and(Option::is_some) {
            continue;
        }
        let Some(rec) = cx.tx.record(&ns, id)? else {
            continue;
        };
        if !is_visible(&rec) {
            continue;
        }
        let pins = pins_of(cx.tx, &ns, id)?;
        let mut fields = effective_record(cx.spec, &rec, None, &pins, cx.os).fields;
        // Compare positions as positions.
        fields.remove(ORDER);
        missing.insert(id.clone(), fields);
    }
    let name_field = catalog::label_field(&ns);
    let mut orphans_cmp = orphans.clone();
    for f in orphans_cmp.values_mut() {
        f.remove(ORDER);
    }
    let pairs = super::sources::lists::rematch(&orphans_cmp, &missing, name_field);
    Ok(pairs.into_iter().collect())
}

/// Turn local positions into fractional keys that move as few items as
/// possible (§4.4): the longest run of items already in key order keeps
/// its keys; every other item gets a key between its neighbours.
fn capture_order(
    cx: &mut Cx<'_, '_>,
    local: &BTreeMap<String, Option<&LocalRecord>>,
    alias: &BTreeMap<String, String>,
) -> anyhow::Result<usize> {
    let ns = cx.spec.ns.clone();
    let mut seq: Vec<(u64, String)> = Vec::new();
    for (id, l) in local {
        let Some(l) = l else { continue };
        let Some(pos) = l.fields.get(ORDER).and_then(Value::as_u64) else {
            continue;
        };
        let rid = alias.get(id).cloned().unwrap_or_else(|| id.clone());
        if cx.tx.pin(&LocalKey::new(&ns, &rid, "*"))?.is_some() {
            continue;
        }
        seq.push((pos, rid));
    }
    seq.sort();
    let mut recs: Vec<Option<Record>> = Vec::with_capacity(seq.len());
    let mut keys: Vec<Option<(String, String)>> = Vec::with_capacity(seq.len());
    for (_, id) in &seq {
        let r = cx.tx.record(&ns, id)?;
        let k = r
            .as_ref()
            .and_then(|r| r.cell(ORDER, Scope::All))
            .and_then(|c| c.value.as_str().map(str::to_string))
            .map(|k| (k, id.clone()));
        keys.push(k);
        recs.push(r);
    }
    // The order the file showed when sync last looked. A local order that
    // still matches it is stale, not a reorder: newer keys may have
    // arrived since, and the projection puts them in place.
    let ids: Vec<String> = seq.iter().map(|(_, id)| id.clone()).collect();
    let known = known_order(cx.tx, &ns)?;
    set_known_order(cx.tx, &ns, &ids)?;
    let moved = {
        let in_known: BTreeSet<&String> = known.iter().collect();
        let in_local: BTreeSet<&String> = ids.iter().collect();
        let a: Vec<&String> = known.iter().filter(|id| in_local.contains(id)).collect();
        let b: Vec<&String> = ids.iter().filter(|id| in_known.contains(id)).collect();
        a != b
    };
    let all_keyed = keys.iter().all(Option::is_some);
    if !moved && all_keyed {
        return Ok(0);
    }
    let keep = if moved {
        lis(&keys)
    } else {
        // Only new items need keys; everything else keeps its own.
        (0..keys.len()).filter(|i| keys[*i].is_some()).collect()
    };
    let mut n = 0;
    let mut assigned: Vec<Option<String>> = keys
        .iter()
        .map(|k| k.as_ref().map(|(k, _)| k.clone()))
        .collect();
    for i in 0..seq.len() {
        if keep.contains(&i) {
            continue;
        }
        let Some(mut r) = recs[i].take() else {
            continue;
        };
        if !is_visible(&r) {
            continue;
        }
        let prev = (0..i)
            .rev()
            .find_map(|j| assigned[j].clone().filter(|_| keep.contains(&j) || j < i));
        let next = (i + 1..seq.len()).find_map(|j| {
            if keep.contains(&j) {
                assigned[j].clone()
            } else {
                None
            }
        });
        let key = key_between(prev.as_deref(), next.as_deref());
        assigned[i] = Some(key.clone());
        let cur = r.cell(ORDER, Scope::All).cloned();
        let floor = cur.as_ref().map_or(0, |c| c.stamp.hlc);
        let stamp = cx.stamp(floor);
        let anc = ancestors(cur.as_ref(), cx.me);
        r.cells.insert(
            FieldKey::all(ORDER),
            Cell {
                value: Value::String(key.clone()),
                stamp,
                wall_ms: cx.now,
                anc,
            },
        );
        cx.write(&r)?;
        cx.tx.set_projected(
            &LocalKey::new(&ns, &seq[i].1, ORDER),
            Some((hash(&Value::String(key)), Some(stamp), Some(Scope::All))),
        )?;
        n += 1;
    }
    Ok(n)
}

/// The item order of list `ns` as the local file last showed it.
fn known_order(tx: &Tx<'_>, ns: &str) -> anyhow::Result<Vec<String>> {
    Ok(tx
        .meta(&format!("order.{ns}"))?
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default())
}

fn set_known_order(tx: &mut Tx<'_>, ns: &str, ids: &[String]) -> anyhow::Result<()> {
    tx.set_meta(&format!("order.{ns}"), &serde_json::to_string(ids)?)
}

/// The item order of list `ns` in a read of the local files.
fn local_order(state: &LocalState, ns: &str) -> Vec<String> {
    let mut seq: Vec<(u64, &str)> = state
        .records
        .iter()
        .filter(|((n, _), _)| n == ns)
        .filter_map(|((_, id), r)| Some((r.as_ref()?.fields.get(ORDER)?.as_u64()?, id.as_str())))
        .collect();
    seq.sort();
    seq.into_iter().map(|(_, id)| id.to_string()).collect()
}

/// Indices of a longest strictly increasing run of the keys present.
pub(crate) fn lis(keys: &[Option<(String, String)>]) -> BTreeSet<usize> {
    let idx: Vec<usize> = (0..keys.len()).filter(|i| keys[*i].is_some()).collect();
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; keys.len()];
    for &i in &idx {
        let k = keys[i].as_ref();
        let pos = tails.partition_point(|&t| keys[t].as_ref() < k);
        if pos > 0 {
            prev[i] = Some(tails[pos - 1]);
        }
        if pos == tails.len() {
            tails.push(i);
        } else {
            tails[pos] = i;
        }
    }
    let mut out = BTreeSet::new();
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        out.insert(i);
        cur = prev[i];
    }
    out
}

const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn digit(c: u8) -> usize {
    DIGITS.iter().position(|d| *d == c).unwrap_or(0)
}

/// The shortest base-62 key strictly between `a` and `b` (`None`: the
/// start or the end). Keys never end in `0`, so every key has room before
/// it.
pub fn key_between(a: Option<&str>, b: Option<&str>) -> String {
    let a = a.unwrap_or("").as_bytes();
    let mut b = b.map(str::as_bytes).filter(|b| *b > a);
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let x = a.get(i).map_or(0, |c| digit(*c));
        let y = match b {
            None => DIGITS.len(),
            Some(b) => b.get(i).map_or(0, |c| digit(*c)),
        };
        if y > x + 1 {
            out.push(DIGITS[(x + y) / 2]);
            break;
        }
        out.push(DIGITS[x]);
        if y == x + 1 {
            b = None;
        }
        i += 1;
        if i > 64 {
            out.push(DIGITS[DIGITS.len() / 2]);
            break;
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

/// The capture context handed to a source for an extension request.
struct ExtCtx<'a, 'b> {
    tx: &'a mut Tx<'b>,
    hlc: &'a mut Hlc,
    keys: &'a Keys,
    me: DeviceId,
    os: OsKind,
    now: i64,
    specs: &'a BTreeMap<String, (usize, NsSpec)>,
}

impl super::sources::CaptureCtx for ExtCtx<'_, '_> {
    fn capture(
        &mut self,
        ns: &str,
        id: &str,
        local: Option<&LocalRecord>,
        origin: Origin,
        initial_stamp_ms: Option<i64>,
    ) -> anyhow::Result<super::sources::CaptureResult> {
        let Some((_, spec)) = self.specs.get(ns) else {
            return Ok(super::sources::CaptureResult::Rejected("bad_collection"));
        };
        let spec = spec.clone();
        let mut proj: BTreeMap<String, Projected> = BTreeMap::new();
        for (k, p) in self.tx.projected_in(ns)? {
            if k.id == id {
                proj.insert(k.field, p);
            }
        }
        // An initial push stamps from the record's own time, with no floor
        // and without moving the clock (§8.2).
        let mut hlc = match initial_stamp_ms {
            Some(ms) => Hlc::new(super::hlc::physical(ms.min(self.now)).saturating_sub(1)),
            None => *self.hlc,
        };
        let now = initial_stamp_ms.map_or(self.now, |ms| ms.min(self.now));
        let mut cx = Cx {
            tx: self.tx,
            me: self.me,
            os: self.os,
            now,
            hlc: &mut hlc,
            keys: self.keys,
            origin,
            spec: &spec,
        };
        let mut out = CaptureOut::default();
        let mut churn = Vec::new();
        let wrote = capture_record(
            &mut cx,
            id,
            local,
            &proj,
            &HashSet::new(),
            &mut out,
            &mut churn,
        )?;
        if initial_stamp_ms.is_none() {
            *self.hlc = hlc;
        }
        Ok(if wrote {
            super::sources::CaptureResult::Written
        } else {
            super::sources::CaptureResult::Unchanged
        })
    }

    fn merged(&self, ns: &str, id: &str) -> anyhow::Result<Option<LocalRecord>> {
        let Some((_, spec)) = self.specs.get(ns) else {
            return Ok(None);
        };
        let Some(rec) = self.tx.record(ns, id)? else {
            return Ok(None);
        };
        if !is_visible(&rec) {
            return Ok(None);
        }
        let pins = pins_of(self.tx, ns, id)?;
        Ok(Some(effective_record(spec, &rec, None, &pins, self.os)))
    }

    fn store(&mut self) -> &mut dyn super::replica::ExtStore {
        self.tx
    }

    fn keys(&self) -> &Keys {
        self.keys
    }

    fn device(&self) -> DeviceId {
        self.me
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_between_sort_between_and_never_end_in_zero() {
        let mut keys = vec![key_between(None, None)];
        for _ in 0..200 {
            let last = keys.last().cloned();
            keys.push(key_between(last.as_deref(), None));
        }
        for w in keys.windows(2) {
            assert!(w[0] < w[1], "{:?}", w);
        }
        let mut a = "a".to_string();
        let b = "b".to_string();
        for _ in 0..100 {
            let m = key_between(Some(&a), Some(&b));
            assert!(a < m && m < b, "{a} {m} {b}");
            assert!(!m.ends_with('0'));
            a = m;
        }
        let first = key_between(None, Some("1"));
        assert!(first.as_str() < "1");
        // Equal or reversed neighbours (two computers chose the same key)
        // fall back to after the first.
        let m = key_between(Some("V"), Some("V"));
        assert!(m.as_str() > "V");
    }

    #[test]
    fn longest_run_keeps_most_keys() {
        let k = |s: &str| Some((s.to_string(), String::new()));
        let keys = vec![k("a"), k("c"), k("b"), k("d"), None, k("e")];
        let keep = lis(&keys);
        assert_eq!(keep.len(), 4);
        assert!(keep.contains(&0) && keep.contains(&3) && keep.contains(&5));
    }

    #[test]
    fn default_scope_follows_the_structure() {
        let mut spec = super::super::sources::config::spec();
        assert_eq!(default_scope(&spec, "hotkey", "v"), DefaultScope::PerOs);
        assert_eq!(default_scope(&spec, "ui.theme", "v"), DefaultScope::Shared);
        spec.structure.per_os = vec!["shortcut".into()];
        assert_eq!(default_scope(&spec, "s1", "shortcut"), DefaultScope::PerOs);
        assert_eq!(base_field("+status"), "status");
        assert_eq!(base_field("tags[]\"a\""), "tags");
    }

    #[test]
    fn effective_values_follow_this_os_then_all_with_explicit_nulls() {
        let d = DeviceId([1; 10]);
        let cell = |v: Value, n: u64| Cell {
            value: v,
            stamp: Stamp::new(n, d),
            wall_ms: 0,
            anc: vec![],
        };
        let mut r = Record::new("list.shortcuts", "s1");
        r.cells.insert(
            FieldKey::os("path", OsKind::Macos),
            cell(Value::from("/mac"), 1),
        );
        r.cells.insert(FieldKey::all("url"), cell(Value::Null, 2));
        assert_eq!(
            eff_cell(&r, "path", OsKind::Macos).unwrap().value,
            Value::from("/mac")
        );
        assert!(eff_cell(&r, "path", OsKind::Windows).is_none());
        assert!(eff_cell(&r, "url", OsKind::Windows)
            .unwrap()
            .value
            .is_null());
        let spec =
            super::super::sources::lists::spec_of(catalog::list_spec("list.shortcuts").unwrap());
        let mut local = LocalRecord::default();
        local.fields.insert("url".into(), Value::from("old"));
        local.fields.insert("path".into(), Value::from("C:\\win"));
        let t = effective_record(&spec, &r, Some(&local), &BTreeMap::new(), OsKind::Windows);
        assert!(t.fields.get("url").is_none(), "an explicit null clears");
        assert_eq!(
            t.fields["path"],
            Value::from("C:\\win"),
            "no cell keeps local"
        );
    }
}
