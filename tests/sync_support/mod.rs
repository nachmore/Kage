//! A simulated cloud and simulated computers for folder sync tests
//! (docs/design/SYNC.md §18.2).
//!
//! - [`SimComputer`]: one Kage install: a `SyncEngine` with its own temp
//!   directories, a `Config` behind a fake `ConfigPort`, the memory key
//!   store, a fake clock and an OS. Driven by `tick()`; no threads, no
//!   timers.
//! - [`SimCloud`]: each computer sees its own copy of the folder (a
//!   `FaultyFs`). `step()` copies changes between the copies, like a sync
//!   app; a filter can hold files back, deliver them truncated first, or
//!   show placeholders.
//!
//! Every test calls [`isolate`] first, so nothing touches real user
//! folders.

#![allow(dead_code)]

use kage::config::Config;
use kage::extensions::InstalledItem;
use kage::sync::crypto::KdfCost;
use kage::sync::engine::{
    CommitError, ConfigPort, EngineEnv, HostEvents, LoadReset, SyncEngine, TickReport, Wake,
};
use kage::sync::prefs::SyncCategories;
use kage::sync::sources::{self, HostDirs, Source};
use kage::sync::status::SyncStatus;
use kage::sync::testing::{FakeClock, FakeCloudFiles, FaultyFs, MemoryKeyStore};
use kage::sync::types::{Origin, OsKind};
use kage::sync::{CreateReq, EngineReply, EngineRequest, JoinChoice, JoinReq};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// The folder every computer picked.
pub const CHOSEN: &str = "/cloud";
/// The passphrase of every test folder.
pub const PASS: &str = "correct horse battery staple";
/// 2026-11-09T10:00:00Z.
pub const T0: i64 = 1_762_682_400_000;
pub const DAY: i64 = 86_400_000;

/// Isolate user dirs for the whole test process (once).
pub fn isolate() {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    let root = ROOT.get_or_init(|| {
        let d = tempfile::tempdir().expect("temp dir");
        let p = d.path().to_path_buf();
        std::mem::forget(d);
        p
    });
    kage::paths::isolate(root.clone());
    kage::i18n::init(Some("en"));
    // `RUST_LOG=kage=info` shows the engine's log in a failing test.
    let _ = env_logger::builder().is_test(true).try_init();
}

/// `config.json` behind a revision counter, as the app's port keeps it.
#[derive(Default)]
pub struct Port {
    state: Mutex<(u64, Config)>,
    pub installed: Mutex<Vec<InstalledItem>>,
    pub reset: Mutex<LoadReset>,
    pub suspended: std::sync::atomic::AtomicBool,
    pub commits: std::sync::atomic::AtomicUsize,
}

impl Port {
    pub fn new(mut c: Config) -> Self {
        c.ensure_item_ids();
        Self {
            state: Mutex::new((1, c)),
            ..Default::default()
        }
    }

    /// The user changes something (a Settings save).
    pub fn edit(&self, f: impl FnOnce(&mut Config)) {
        let mut st = self.state.lock().unwrap();
        f(&mut st.1);
        st.1.ensure_item_ids();
        st.0 += 1;
    }

    pub fn get(&self) -> Config {
        self.state.lock().unwrap().1.clone()
    }

    /// Replace the whole config (a corrupt file reset, a backup restore).
    pub fn replace(&self, c: Config) {
        let mut st = self.state.lock().unwrap();
        st.1 = c;
        st.0 += 1;
    }
}

impl ConfigPort for Port {
    fn snapshot(&self) -> (u64, Config) {
        self.state.lock().unwrap().clone()
    }

    fn commit(&self, base: u64, next: Config, _origin: Origin) -> Result<u64, CommitError> {
        if self.suspended.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(CommitError::Suspended);
        }
        let mut st = self.state.lock().unwrap();
        if st.0 != base {
            return Err(CommitError::Stale);
        }
        st.0 += 1;
        st.1 = next;
        self.commits
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(st.0)
    }

    fn load_was_reset(&self) -> LoadReset {
        self.reset.lock().unwrap().clone()
    }

    fn saves_suspended(&self) -> bool {
        self.suspended.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn installed(&self) -> Vec<InstalledItem> {
        self.installed.lock().unwrap().clone()
    }
}

/// A telemetry event and its props.
pub type Tracked = (String, Vec<(String, String)>);

/// What the engine told the app.
#[derive(Default)]
pub struct Events {
    pub tracked: Mutex<Vec<Tracked>>,
    pub notified: Mutex<Vec<String>>,
    pub applied: Mutex<Vec<String>>,
    pub statuses: Mutex<Vec<SyncStatus>>,
}

impl HostEvents for Events {
    fn status_changed(&self, s: &SyncStatus) {
        self.statuses.lock().unwrap().push(s.clone());
    }
    fn conflicts_changed(&self, _open: usize) {}
    fn applied(&self, keys: &[String]) {
        self.applied.lock().unwrap().extend(keys.iter().cloned());
    }
    fn extension_data_changed(&self, _ext: &str, _keys: &[String]) {}
    fn extension_sync_changes(&self, _ext: &str, _collections: &[String], _reason: &str) {}
    fn notify_user(&self, state: &str) {
        self.notified.lock().unwrap().push(state.to_string());
    }
    fn track(&self, event: &'static str, props: Vec<(&'static str, String)>) {
        self.tracked.lock().unwrap().push((
            event.to_string(),
            props.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        ));
    }
}

/// A shared clock all computers read, so a test moves time for all.
pub fn clock() -> Arc<FakeClock> {
    // Every test starts here: isolate user dirs before anything else.
    isolate();
    Arc::new(FakeClock::new(T0))
}

/// One simulated computer.
pub struct SimComputer {
    pub name: String,
    pub os: OsKind,
    pub engine: SyncEngine,
    pub port: Arc<Port>,
    pub fs: Arc<FaultyFs>,
    pub cloud: Arc<FakeCloudFiles>,
    pub keys: Arc<MemoryKeyStore>,
    pub clock: Arc<FakeClock>,
    pub events: Arc<Events>,
    pub fingerprint: String,
    /// `<data_local>/kage/sync` and `<config>/kage`, in a temp dir.
    pub local: PathBuf,
    pub config_dir: PathBuf,
    /// This computer's chats, when the test gives it some.
    pub chats: Option<Arc<dyn kage::sync::sources::chats::ChatHistory>>,
    _tmp: Arc<tempfile::TempDir>,
}

/// A cheap Argon2 cost for tests (the app uses §11.3's).
pub const TEST_KDF: KdfCost = KdfCost {
    m_kib: 64,
    t: 1,
    p: 1,
};

impl SimComputer {
    pub fn new(name: &str, os: OsKind, clock: Arc<FakeClock>) -> Self {
        Self::with_config(name, os, clock, Config::default())
    }

    pub fn with_config(name: &str, os: OsKind, clock: Arc<FakeClock>, config: Config) -> Self {
        isolate();
        let tmp = Arc::new(tempfile::tempdir().expect("temp dir"));
        let local = tmp.path().join("data_local/kage/sync");
        let config_dir = tmp.path().join("config/kage");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(&config_dir).unwrap();
        let fs = Arc::new(FaultyFs::with_clock(clock.clone()));
        let port = Arc::new(Port::new(config));
        let mut c = Self {
            name: name.to_string(),
            os,
            engine: dummy_engine(),
            port,
            fs,
            cloud: Arc::new(FakeCloudFiles::new()),
            keys: Arc::new(MemoryKeyStore::new()),
            clock,
            events: Arc::new(Events::default()),
            fingerprint: format!("fp-{name}"),
            local,
            config_dir,
            chats: None,
            _tmp: tmp,
        };
        c.reopen();
        c
    }

    /// Start the engine again from the same directories (a restart).
    pub fn reopen(&mut self) {
        let env = EngineEnv {
            local_dir: self.local.clone(),
            os: self.os,
            kage_version: "0.12.0+test".to_string(),
            fingerprint: self.fingerprint.clone(),
            fs: self.fs.clone(),
            cloud: self.cloud.clone(),
            keys: self.keys.clone(),
            clock: self.clock.clone(),
            events: self.events.clone(),
            config: self.port.clone(),
        };
        // Drop the old engine first: it holds the profile's lock.
        self.engine = dummy_engine();
        let mut e = SyncEngine::open(env, self.sources()).expect("engine opens");
        e.set_kdf_cost(TEST_KDF);
        self.engine = e;
    }

    pub fn sources(&self) -> Vec<Box<dyn Source>> {
        sources::host_sources(
            self.port.clone(),
            HostDirs {
                config_kage_dir: self.config_dir.clone(),
                sync_dir: self.local.clone(),
            },
            self.os,
            self.chats.clone(),
        )
    }

    /// A copy of this computer's profile on another machine (a migration
    /// tool, a VM clone): the same files, another fingerprint.
    pub fn clone_profile(&self, name: &str) -> SimComputer {
        let mut c = SimComputer::with_config(name, self.os, self.clock.clone(), self.config());
        copy_dir(&self.local, &c.local);
        c.keys = self.keys.clone();
        c.reopen();
        c
    }

    /// A copy so exact even the fingerprint matches (a VM snapshot): it
    /// keeps the device id until the folder shows the clash.
    pub fn twin(&self, name: &str) -> SimComputer {
        let mut c = SimComputer::with_config(name, self.os, self.clock.clone(), self.config());
        copy_dir(&self.local, &c.local);
        c.keys = self.keys.clone();
        c.fingerprint = self.fingerprint.clone();
        c.reopen();
        c
    }

    /// This computer's device id.
    pub fn device_id(&self) -> String {
        kage::sync::prefs::DevicePrefs::load(&self.local)
            .device_id
            .unwrap_or_default()
    }

    /// The folder path of this computer's directory.
    pub fn own_dir(&self) -> PathBuf {
        PathBuf::from(format!("{CHOSEN}/Kage Sync/devices/{}", self.device_id()))
    }

    pub fn ask(&mut self, req: EngineRequest) -> EngineReply {
        match self.engine.handle(req) {
            Ok(r) => r,
            Err(e) => panic!("{} refused a request: {} ({})", self.name, e.key, e),
        }
    }

    pub fn try_ask(&mut self, req: EngineRequest) -> Result<EngineReply, kage::error::AppError> {
        self.engine.handle(req)
    }

    /// Set up a new sync folder from this computer.
    pub fn create(&mut self) {
        self.ask(EngineRequest::Create(CreateReq {
            path: PathBuf::from(CHOSEN),
            passphrase: PASS.to_string(),
            device_name: self.name.clone(),
            categories: SyncCategories::default(),
        }));
    }

    /// Join the folder and confirm with `choice`.
    pub fn join(&mut self, choice: JoinChoice) {
        self.ask(EngineRequest::Join(JoinReq {
            path: PathBuf::from(CHOSEN),
            passphrase: PASS.to_string(),
            device_name: self.name.clone(),
            categories: SyncCategories::default(),
        }));
        self.ask(EngineRequest::ConfirmJoin(choice));
    }

    /// A tick as "Sync now" runs one.
    pub fn tick(&mut self) -> TickReport {
        self.engine.tick(Wake::Manual)
    }

    /// The user edits `config.json`; the save hook wakes the engine.
    pub fn edit(&mut self, f: impl FnOnce(&mut Config)) {
        self.port.edit(f);
        self.engine.note_wake(Wake::Local {
            source: "config",
            origin: Origin::User,
        });
    }

    pub fn config(&self) -> Config {
        self.port.get()
    }

    pub fn status(&self) -> SyncStatus {
        self.engine.status()
    }

    pub fn tracked(&self, event: &str) -> usize {
        self.events
            .tracked
            .lock()
            .unwrap()
            .iter()
            .filter(|(e, _)| e == event)
            .count()
    }

    pub fn conflicts(&mut self) -> Vec<kage::sync::status::ConflictSummary> {
        match self.ask(EngineRequest::ListConflicts(
            kage::sync::ConflictTab::Replaced,
        )) {
            EngineReply::Conflicts(c) => c,
            _ => panic!("not conflicts"),
        }
    }

    pub fn steering(&self, name: &str) -> String {
        std::fs::read_to_string(self.config_dir.join(name)).unwrap_or_default()
    }

    pub fn write_steering(&mut self, name: &str, text: &str) {
        std::fs::write(self.config_dir.join(name), text).unwrap();
        self.engine.note_wake(Wake::Local {
            source: "steering",
            origin: Origin::User,
        });
    }
}

/// Copy a directory tree (a profile).
pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        let name = e.file_name();
        if name == "engine.lock" {
            continue;
        }
        if p.is_dir() {
            copy_dir(&p, &to.join(&name));
        } else {
            std::fs::copy(&p, to.join(&name)).unwrap();
        }
    }
}

/// An engine with sync off, to stand in while a real one is replaced.
fn dummy_engine() -> SyncEngine {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    isolate();
    let dir = DIR.get_or_init(|| {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().to_path_buf();
        std::mem::forget(d);
        p
    });
    // Each placeholder gets its own directory, so it holds no lock others
    // need.
    let own = dir.join(format!("{}", rand_u64()));
    let env = EngineEnv {
        local_dir: own,
        os: OsKind::Linux,
        kage_version: String::new(),
        fingerprint: String::new(),
        fs: Arc::new(FaultyFs::new()),
        cloud: Arc::new(FakeCloudFiles::new()),
        keys: Arc::new(MemoryKeyStore::new()),
        clock: Arc::new(FakeClock::new(T0)),
        events: Arc::new(Events::default()),
        config: Arc::new(Port::default()),
    };
    SyncEngine::open(env, Vec::new()).expect("a placeholder engine")
}

fn rand_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::SeqCst) ^ (std::process::id() as u64) << 32
}

/// What a file delivery does to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deliver {
    Now,
    /// Not yet: stays in the cloud for a later step.
    Later,
    /// The first half now (a download in progress).
    Truncated,
}

/// The cloud between the computers.
#[derive(Default)]
pub struct SimCloud {
    master: BTreeMap<PathBuf, Vec<u8>>,
    /// What each computer's copy held after the last step, by name.
    last: BTreeMap<String, BTreeMap<PathBuf, Vec<u8>>>,
    /// Files each computer is still downloading: never uploaded back.
    partial: BTreeMap<String, std::collections::BTreeSet<PathBuf>>,
}

/// `name (Who's conflicted copy).ext`, as Dropbox names one.
fn conflicted_copy(p: &Path, who: &str) -> PathBuf {
    let name = p.file_name().unwrap().to_string_lossy();
    let (stem, ext) = name.rsplit_once('.').unwrap_or((&name, ""));
    p.with_file_name(format!("{stem} ({who}'s conflicted copy).{ext}"))
}

fn is_own_temp(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("~kage-"))
}

impl SimCloud {
    pub fn new() -> Self {
        Self::default()
    }

    /// Upload every computer's changes, then deliver everything to all.
    pub fn step(&mut self, comps: &mut [&mut SimComputer]) {
        self.step_with(comps, |_, _| Deliver::Now);
    }

    /// Like [`Self::step`], deciding per computer and file.
    pub fn step_with(
        &mut self,
        comps: &mut [&mut SimComputer],
        policy: impl Fn(&str, &Path) -> Deliver,
    ) {
        for c in comps.iter() {
            let now: BTreeMap<PathBuf, Vec<u8>> =
                c.fs.files_under(CHOSEN)
                    .into_iter()
                    .filter(|(p, _)| !is_own_temp(p))
                    .collect();
            let last = self.last.entry(c.name.clone()).or_default();
            let partial = self.partial.entry(c.name.clone()).or_default();
            for (p, b) in &now {
                if last.get(p) == Some(b) || partial.contains(p) {
                    continue;
                }
                // Another computer wrote this path since this one last
                // synced: the sync app keeps both, as a conflicted copy.
                let clash = self
                    .master
                    .get(p)
                    .is_some_and(|m| m != b && last.get(p) != Some(m));
                if clash {
                    self.master.insert(conflicted_copy(p, &c.name), b.clone());
                } else {
                    self.master.insert(p.clone(), b.clone());
                }
            }
            for p in last.keys() {
                if !now.contains_key(p) {
                    self.master.remove(p);
                }
            }
        }
        for c in comps.iter_mut() {
            let now: BTreeMap<PathBuf, Vec<u8>> = c.fs.files_under(CHOSEN).into_iter().collect();
            for (p, b) in &self.master {
                if now.get(p) == Some(b) {
                    continue;
                }
                let partial = self.partial.entry(c.name.clone()).or_default();
                match policy(&c.name, p) {
                    Deliver::Now => {
                        partial.remove(p);
                        c.fs.put(p, b.clone());
                    }
                    Deliver::Later => {}
                    Deliver::Truncated => {
                        partial.insert(p.clone());
                        c.fs.put(p, b[..b.len() / 2].to_vec());
                    }
                }
            }
            for p in now.keys() {
                if !self.master.contains_key(p) && !is_own_temp(p) {
                    c.fs.delete(p);
                }
            }
            let after: BTreeMap<PathBuf, Vec<u8>> =
                c.fs.files_under(CHOSEN)
                    .into_iter()
                    .filter(|(p, _)| !is_own_temp(p))
                    .collect();
            // A file held back or cut short isn't this computer's to
            // delete: remember only what the cloud has too.
            let mut seen = after.clone();
            for (p, b) in &after {
                if self.master.get(p) != Some(b) {
                    seen.remove(p);
                }
            }
            self.last.insert(c.name.clone(), seen);
        }
    }

    /// Every computer ticks, then the cloud steps, `rounds` times.
    pub fn settle(&mut self, comps: &mut [&mut SimComputer], rounds: usize) {
        for _ in 0..rounds {
            for c in comps.iter_mut() {
                c.tick();
            }
            self.step(comps);
        }
    }

    /// The files under `prefix` in the cloud.
    pub fn files(&self, prefix: &str) -> Vec<PathBuf> {
        self.master
            .keys()
            .filter(|p| p.starts_with(prefix))
            .cloned()
            .collect()
    }
}

/// Two computers on one folder: A created it, B joined with `choice`.
pub fn pair(choice: JoinChoice) -> (SimCloud, SimComputer, SimComputer, Arc<FakeClock>) {
    let clock = clock();
    let mut a = SimComputer::new("Home Mac", OsKind::Macos, clock.clone());
    let mut b = SimComputer::new("Work PC", OsKind::Windows, clock.clone());
    let mut cloud = SimCloud::new();
    a.create();
    cloud.step(&mut [&mut a, &mut b]);
    b.join(choice);
    cloud.settle(&mut [&mut a, &mut b], 3);
    (cloud, a, b, clock)
}

/// Move time on.
pub fn advance(clock: &FakeClock, ms: i64) {
    clock.advance(std::time::Duration::from_millis(ms as u64));
}

/// An installed extension with `permissions`, as `ConfigPort::installed`
/// lists it.
pub fn installed(id: &str, permissions: &[&str]) -> InstalledItem {
    InstalledItem {
        manifest: serde_json::from_value(serde_json::json!({
            "id": id,
            "name": id,
            "version": "1.0.0",
            "type": "extension",
            "permissions": permissions,
        }))
        .expect("a manifest"),
        path: format!("/extensions/{id}"),
        enabled: true,
    }
}

/// A grant of `caps` for `id`.
pub fn grant(c: &mut Config, id: &str, caps: &[&str]) {
    c.extension_grants.insert(
        id.to_string(),
        kage::config::ExtensionGrant {
            granted: caps.iter().map(|s| s.to_string()).collect(),
            approved_version: "1.0.0".into(),
            approved_at: "2026-11-09T10:00:00Z".into(),
            origin: None,
        },
    );
}

/// The reviews of a computer.
pub fn reviews(c: &mut SimComputer) -> kage::sync::status::Reviews {
    match c.ask(EngineRequest::ListReviews) {
        EngineReply::Reviews(r) => r,
        _ => panic!("not reviews"),
    }
}

/// The scope view of `path`.
pub fn scope(c: &mut SimComputer, path: &str) -> kage::sync::status::ScopeView {
    match c.ask(EngineRequest::GetScopes(Some(vec![path.to_string()]))) {
        EngineReply::Scopes(mut m) => m.remove(path).expect("asked for it"),
        _ => panic!("not scopes"),
    }
}
