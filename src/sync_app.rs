//! The app side of folder sync (docs/design/SYNC.md §14, §19.4, WP-S4):
//! the ports the engine reaches the app through, and the small pieces of
//! state the sync commands share.
//!
//! - [`TauriConfigPort`]: `config.json` as the engine sees it. Every save
//!   goes through the revision ring, [`ConfigRevisions`].
//! - [`TauriHostEvents`]: Tauri events, the tray line, OS notifications and
//!   telemetry, sent from the engine thread.
//! - [`SystemClock`] and [`fingerprint`]: time, power, and this computer's
//!   identity check (§8.9).
//! - [`start`] and [`flush_for_exit`]: the engine thread's life (§14.1).
//! - [`Confirmations`]: the one-use tokens destructive commands need.
//! - [`merge`]: the `save_config` three-way merge (§9.5).

pub mod merge;

use crate::config::Config;
use crate::extensions::InstalledItem;
use crate::lock_ext::LockExt;
use crate::os::power::PowerState;
use crate::state::FeatureServices;
use crate::sync::engine::{Clock, CommitError, ConfigPort, EngineEnv, HostEvents, LoadReset};
use crate::sync::prefs::DevicePrefs;
use crate::sync::sources::Source;
use crate::sync::status::{DeviceState, SyncState, SyncStatus};
use crate::sync::types::{Origin, OsKind};
use crate::sync::DestructiveAction;
use serde::Serialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter, EventTarget, Manager, Runtime};

// --- Timing (§14.1) ------------------------------------------------------------

/// How long after `setup()` the engine thread starts its work. Nothing of
/// sync runs on the startup path.
pub const START_DELAY: Duration = Duration::from_secs(10);

/// The longest the engine waits past [`START_DELAY`] for the first window
/// to paint.
const PAINT_WAIT: Duration = Duration::from_secs(20);

/// The exit flush: own edits are captured and published within it. What
/// doesn't finish is published at the next start; it is in the replica.
pub const EXIT_FLUSH: Duration = Duration::from_millis(1500);

/// The exit flush when an update is about to install.
pub const EXIT_FLUSH_UPDATE: Duration = Duration::from_millis(500);

/// The budget of the exit flush.
pub fn exit_budget(updating: bool) -> Duration {
    if updating {
        EXIT_FLUSH_UPDATE
    } else {
        EXIT_FLUSH
    }
}

/// Start the engine thread: it waits `delay` (or until a window asks for
/// sync), then until the first window painted (at most [`PAINT_WAIT`]
/// more), then opens the engine. Hooks (`sync::notify`) wake it from then
/// on. A build without sync starts nothing.
pub fn start<R: Runtime>(app: &AppHandle<R>, delay: Duration) {
    let Some(features) = app.try_state::<FeatureServices>() else {
        return;
    };
    let handle = features.sync.clone();
    crate::sync::install_global(handle.clone());
    let app = app.clone();
    handle.start(
        move || {
            wait_for_first_paint(&app);
            engine_env(&app)
        },
        delay,
    );
}

fn wait_for_first_paint<R: Runtime>(app: &AppHandle<R>) {
    let Some(ui) = app.try_state::<crate::state::UiState>() else {
        return;
    };
    let until = Instant::now() + PAINT_WAIT;
    while !ui.frontend_ready.load(Ordering::SeqCst) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Capture and publish own edits before Kage exits, within `budget`.
pub fn flush_for_exit<R: Runtime>(app: &AppHandle<R>, budget: Duration) {
    if let Some(features) = app.try_state::<FeatureServices>() {
        features.sync.flush_for_exit(budget);
    }
}

/// Everything the engine needs, for this app and its user folders. Runs
/// on the engine thread.
pub fn engine_env<R: Runtime>(
    app: &AppHandle<R>,
) -> anyhow::Result<(EngineEnv, Vec<Box<dyn Source>>)> {
    use crate::commands::extensions::AppExtensionHost;
    use crate::sync::keystore::{default_key_store, key_store_for, KeyStoreKind};
    use crate::sync::sources::extensions::{ExtensionHost, ExtensionsSource};
    use crate::sync::sources::{chats, host_sources, HostDirs};
    let features = app
        .try_state::<FeatureServices>()
        .ok_or_else(|| anyhow::anyhow!("the app's state isn't managed"))?;
    let local_dir = crate::paths::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("no local data folder"))?
        .join("kage")
        .join("sync");
    let config_dir =
        crate::paths::config_dir().ok_or_else(|| anyhow::anyhow!("no config folder"))?;
    let os = this_os();
    let port = Arc::new(TauriConfigPort::new(
        app.clone(),
        features.config.clone(),
        features.config_revisions.clone(),
    ));
    let events = Arc::new(TauriHostEvents::new(
        app.clone(),
        local_dir.clone(),
        port.clone(),
    ));
    // A key kept in the file stays found after the keychain becomes
    // reliable (§11.4).
    let prefs = DevicePrefs::load(&local_dir);
    let keys = match prefs.key_store.as_deref().and_then(KeyStoreKind::from_wire) {
        Some(kind) => key_store_for(kind, &local_dir),
        None => default_key_store(&local_dir),
    };
    let history: Arc<dyn chats::ChatHistory> = Arc::new(chats::RegistryHistory(
        features.agent_session_registry.clone(),
    ));
    let mut sources = host_sources(
        port.clone(),
        HostDirs {
            config_kage_dir: config_dir.join("kage"),
            sync_dir: local_dir.clone(),
        },
        os,
        Some(history),
    );
    // Extension data (WP-S5, SYNC.md §19.4). The host installs synced store
    // themes and works out where older installs came from.
    let ext_host: Arc<dyn ExtensionHost> = Arc::new(AppExtensionHost { app: app.clone() });
    sources.push(Box::new(ExtensionsSource::new(
        port.clone(),
        config_dir.join("kage").join("extension-data"),
        Some(ext_host),
    )));
    let env = EngineEnv {
        local_dir,
        os,
        kage_version: env!("CARGO_PKG_VERSION").to_string(),
        fingerprint: fingerprint(&config_dir),
        fs: Arc::new(crate::sync::fsio::RealFs::new()),
        cloud: crate::os::cloud_files::system(),
        keys,
        clock: Arc::new(SystemClock::default()),
        events,
        config: port,
    };
    Ok((env, sources))
}

/// The OS this build runs on.
pub fn this_os() -> OsKind {
    OsKind::from_name(std::env::consts::OS).unwrap_or(OsKind::Linux)
}

/// This computer's identity check (§8.9): the first 16 hex digits of
/// SHA-256 of the computer's name and the config folder's path. A profile
/// copied to another computer or to another user gets a new fingerprint,
/// and the engine takes a new device id there. A copy that keeps both (a
/// cloned VM) is caught by the engine's check of its own directory.
pub fn fingerprint(config_dir: &Path) -> String {
    let name = whoami::devicename()
        .or_else(|_| whoami::hostname())
        .unwrap_or_default();
    fingerprint_of(&name, config_dir)
}

fn fingerprint_of(computer: &str, config_dir: &Path) -> String {
    let mut h = Sha256::new();
    h.update(computer.trim().to_lowercase().as_bytes());
    h.update([0]);
    h.update(config_dir.to_string_lossy().as_bytes());
    hex::encode(h.finalize())[..16].to_string()
}

// --- Config revisions (§9.5) ------------------------------------------------------

/// How many configs [`ConfigRevisions`] keeps.
pub const KEEP_REVISIONS: usize = 64;

/// The last configs by revision. A revision names one state of the
/// in-memory config: [`ConfigRevisions::observe`] gives a new one whenever
/// the config holds other values than the newest one kept, whichever code
/// path changed it. `get_config_snapshot` hands it out, `save_config`
/// merges against the config it names, and the sync engine's commits fail
/// as stale when it moved.
#[derive(Default)]
pub struct ConfigRevisions {
    ring: Mutex<Ring>,
}

#[derive(Default)]
struct Ring {
    last: u64,
    kept: VecDeque<Kept>,
}

/// One kept config, as canonical JSON (compact: 64 of them stay small).
struct Kept {
    revision: u64,
    hash: [u8; 32],
    json: Arc<[u8]>,
}

impl ConfigRevisions {
    pub fn new() -> Self {
        Self::default()
    }

    /// The revision `config` is at. Call it with the config lock held, so
    /// the revision and the values belong together.
    pub fn observe(&self, config: &Config) -> u64 {
        let value = serde_json::to_value(config).unwrap_or(Value::Null);
        let bytes = crate::sync::canonical::canonical(&value);
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        let mut ring = self.ring.lock_or_recover();
        if let Some(k) = ring.kept.back() {
            if k.hash == hash {
                return k.revision;
            }
        }
        ring.last += 1;
        let revision = ring.last;
        ring.kept.push_back(Kept {
            revision,
            hash,
            json: bytes.into(),
        });
        while ring.kept.len() > KEEP_REVISIONS {
            ring.kept.pop_front();
        }
        revision
    }

    /// The config at `revision`, as JSON, while it is one of the last
    /// [`KEEP_REVISIONS`].
    pub fn at(&self, revision: u64) -> Option<Value> {
        let json = {
            let ring = self.ring.lock_or_recover();
            ring.kept
                .iter()
                .find(|k| k.revision == revision)?
                .json
                .clone()
        };
        serde_json::from_slice(&json).ok()
    }
}

// --- Destructive commands (§19.5) -----------------------------------------------------

/// How long a token from `sync_prepare_destructive` stays valid.
pub const CONFIRM_FOR: Duration = Duration::from_secs(60);

/// One-use tokens for the destructive commands, so the mock-app sweep and
/// stray calls can't do damage.
#[derive(Default)]
pub struct Confirmations {
    tokens: Mutex<HashMap<String, (DestructiveAction, Instant)>>,
}

impl Confirmations {
    /// A new token for `action`.
    pub fn issue(&self, action: DestructiveAction) -> String {
        self.issue_at(action, Instant::now())
    }

    /// Use up `token` for `action`: true when it was issued for that action
    /// less than [`CONFIRM_FOR`] ago and not used yet.
    pub fn take(&self, token: &str, action: DestructiveAction) -> bool {
        self.take_at(token, action, Instant::now())
    }

    fn issue_at(&self, action: DestructiveAction, now: Instant) -> String {
        let token = format!(
            "{:016x}{:016x}",
            rand::random::<u64>(),
            rand::random::<u64>()
        );
        let mut tokens = self.tokens.lock_or_recover();
        tokens.retain(|_, (_, until)| *until > now);
        tokens.insert(token.clone(), (action, now + CONFIRM_FOR));
        token
    }

    fn take_at(&self, token: &str, action: DestructiveAction, now: Instant) -> bool {
        let mut tokens = self.tokens.lock_or_recover();
        tokens.retain(|_, (_, until)| *until > now);
        match tokens.get(token) {
            Some((a, _)) if *a == action => {
                tokens.remove(token);
                true
            }
            _ => false,
        }
    }
}

/// The process's tokens.
pub fn confirmations() -> &'static Confirmations {
    static TOKENS: LazyLock<Confirmations> = LazyLock::new(Confirmations::default);
    &TOKENS
}

// --- Time and power -------------------------------------------------------------------

/// How long a power reading is reused.
const POWER_FOR: Duration = Duration::from_secs(60);

/// The real clock, and the OS power state (read at most once a minute).
#[derive(Default)]
pub struct SystemClock {
    power: Mutex<Option<(Instant, PowerState)>>,
}

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    fn power(&self) -> PowerState {
        let mut cached = self.power.lock_or_recover();
        if let Some((at, p)) = *cached {
            if at.elapsed() < POWER_FOR {
                return p;
            }
        }
        let p = crate::os::power::get_power_state();
        *cached = Some((Instant::now(), p));
        p
    }
}

// --- The config port ---------------------------------------------------------------------

/// How long a list of installed items is reused when nothing changed.
const INSTALLED_FOR: Duration = Duration::from_secs(60);

/// What the installed list was read at: the config revision (an install or
/// uninstall changes grants and switches) and the item folders' times (an
/// entry added or removed changes them).
#[derive(PartialEq)]
struct InstalledStamp {
    revision: u64,
    dirs: Vec<Option<SystemTime>>,
}

struct InstalledCache {
    stamp: InstalledStamp,
    at: Instant,
    items: Vec<InstalledItem>,
}

/// `config.json` as the engine sees it: the shared in-memory config, saved
/// through the same path as `save_config`.
pub struct TauriConfigPort<R: Runtime> {
    app: AppHandle<R>,
    config: Arc<Mutex<Config>>,
    revisions: Arc<ConfigRevisions>,
    installed: Mutex<Option<InstalledCache>>,
}

impl<R: Runtime> TauriConfigPort<R> {
    pub fn new(
        app: AppHandle<R>,
        config: Arc<Mutex<Config>>,
        revisions: Arc<ConfigRevisions>,
    ) -> Self {
        Self {
            app,
            config,
            revisions,
            installed: Mutex::new(None),
        }
    }
}

/// The installed items' folders.
const ITEM_KINDS: [&str; 3] = ["extension", "theme", "commands"];

fn item_dir_times() -> Vec<Option<SystemTime>> {
    ITEM_KINDS
        .iter()
        .map(|kind| {
            let dir = crate::extensions::kind_to_subdir(kind)
                .and_then(crate::extensions::user_item_dir)
                .ok()?;
            std::fs::metadata(dir).and_then(|m| m.modified()).ok()
        })
        .collect()
}

impl<R: Runtime> ConfigPort for TauriConfigPort<R> {
    fn snapshot(&self) -> (u64, Config) {
        let config = self.config.lock_or_recover();
        (self.revisions.observe(&config), config.clone())
    }

    fn commit(&self, base: u64, next: Config, origin: Origin) -> Result<u64, CommitError> {
        use crate::commands::system::config_io::{
            commit_config_change, preserve_backend_owned_fields, ConfigChange,
        };
        if Config::saves_suspended() {
            return Err(CommitError::Suspended);
        }
        let (change, revision) = {
            let mut current = self.config.lock_or_recover();
            if self.revisions.observe(&current) != base {
                return Err(CommitError::Stale);
            }
            let mut next = next;
            preserve_backend_owned_fields(&mut next, &current);
            let change = ConfigChange::between(&current, &next);
            let before = std::mem::replace(&mut *current, next);
            if let Err(e) = current.save() {
                *current = before;
                return Err(CommitError::Io(e));
            }
            (change, self.revisions.observe(&current))
        };
        log::info!("sync: saved config.json with values from the sync folder ({origin:?})");
        commit_config_change(&self.app, change);
        Ok(revision)
    }

    fn load_was_reset(&self) -> LoadReset {
        Config::load_was_reset()
    }

    fn saves_suspended(&self) -> bool {
        Config::saves_suspended()
    }

    fn installed(&self) -> Vec<InstalledItem> {
        let (revision, states) = {
            let config = self.config.lock_or_recover();
            (
                self.revisions.observe(&config),
                config.extension_states.clone(),
            )
        };
        let stamp = InstalledStamp {
            revision,
            dirs: item_dir_times(),
        };
        let mut cache = self.installed.lock_or_recover();
        if let Some(c) = cache.as_ref() {
            if c.stamp == stamp && c.at.elapsed() < INSTALLED_FOR {
                return c.items.clone();
            }
        }
        let items: Vec<InstalledItem> = ITEM_KINDS
            .iter()
            .flat_map(|kind| crate::extensions::discover_all_items(kind, &states))
            .collect();
        *cache = Some(InstalledCache {
            stamp,
            at: Instant::now(),
            items: items.clone(),
        });
        items
    }
}

// --- Host events ---------------------------------------------------------------------------

/// The newest status the engine reported, for `sync_get_status` while the
/// engine thread is busy.
static LAST_STATUS: Mutex<Option<SyncStatus>> = Mutex::new(None);

/// The newest status the engine reported.
pub fn last_status() -> Option<SyncStatus> {
    LAST_STATUS.lock_or_recover().clone()
}

/// Emit to Kage's own windows: every one but extension view windows.
fn emit_first_party<R: Runtime, S: Serialize + Clone>(
    app: &AppHandle<R>,
    event: &str,
    payload: &S,
) {
    let sent = app.emit_filter(event, payload, |t| match t {
        EventTarget::Window { label }
        | EventTarget::Webview { label }
        | EventTarget::WebviewWindow { label }
        | EventTarget::AnyLabel { label } => !crate::window_labels::is_ext_view_label(label),
        EventTarget::Any | EventTarget::App => true,
        _ => false,
    });
    if let Err(e) = sent {
        log::debug!("sync: emitting {event} failed: {e}");
    }
}

/// What the engine tells the app, from the engine thread.
pub struct TauriHostEvents<R: Runtime> {
    app: AppHandle<R>,
    /// `<data_local>/kage/sync`, for this computer's sync choices.
    local_dir: PathBuf,
    port: Arc<TauriConfigPort<R>>,
}

impl<R: Runtime> TauriHostEvents<R> {
    pub fn new(app: AppHandle<R>, local_dir: PathBuf, port: Arc<TauriConfigPort<R>>) -> Self {
        Self {
            app,
            local_dir,
            port,
        }
    }

    /// How many extensions' data syncs on this computer: installed, granted
    /// `sync`, and not switched off here.
    fn ext_data_count(&self, status: &SyncStatus) -> usize {
        if !status.categories.extensions {
            return 0;
        }
        let prefs = DevicePrefs::load(&self.local_dir);
        let installed: HashSet<String> = self
            .port
            .installed()
            .into_iter()
            .map(|i| i.manifest.id)
            .collect();
        let grants = self.port.config.lock_or_recover().extension_grants.clone();
        grants
            .iter()
            .filter(|(id, g)| {
                installed.contains(*id)
                    && g.granted.iter().any(|c| c == "sync")
                    && !prefs.ext_data_is_off(id)
            })
            .count()
    }

    /// `sync_daily`, with the first status after `app_daily_active` while
    /// sync is on (§16).
    fn daily(&self, status: &SyncStatus) {
        if !DAILY_DUE.load(Ordering::SeqCst) {
            return;
        }
        match status.state {
            SyncState::Unavailable => return,
            SyncState::Off | SyncState::OtherEngine => {
                DAILY_DUE.store(false, Ordering::SeqCst);
                return;
            }
            _ => {}
        }
        if DAILY_DUE.swap(false, Ordering::SeqCst) {
            let props = daily_props(status, self.ext_data_count(status));
            track_sync(&self.app, "sync_daily", props);
        }
    }
}

impl<R: Runtime> HostEvents for TauriHostEvents<R> {
    fn status_changed(&self, s: &SyncStatus) {
        *LAST_STATUS.lock_or_recover() = Some(s.clone());
        emit_first_party(&self.app, crate::events::SYNC_STATUS_CHANGED, s);
        crate::tray::set_sync_status(&self.app, s);
        self.daily(s);
    }

    fn conflicts_changed(&self, open: usize) {
        emit_first_party(
            &self.app,
            crate::events::SYNC_CONFLICTS_CHANGED,
            &json!({ "open": open }),
        );
    }

    fn applied(&self, keys: &[String]) {
        emit_first_party(
            &self.app,
            crate::events::SYNC_APPLIED,
            &json!({ "keys": keys }),
        );
    }

    fn extension_data_changed(&self, ext: &str, keys: &[String]) {
        use crate::extension_data_events::{record, record_bulk, Change, ChangeOp, ChangeSource};
        if keys.is_empty() {
            record_bulk(&self.app, ext, ChangeSource::Sync);
            return;
        }
        for key in keys {
            record(
                &self.app,
                ext,
                Change {
                    key: key.clone(),
                    op: ChangeOp::Save,
                    source: ChangeSource::Sync,
                    window: None,
                },
            );
        }
    }

    fn extension_sync_changes(&self, ext: &str, collections: &[String], reason: &str) {
        crate::event_targets::emit_to_extension_hosts(
            &self.app,
            crate::events::EXTENSION_SYNC_CHANGES,
            &json!({ "extension_id": ext, "collections": collections, "reason": reason }),
        );
    }

    fn notify_user(&self, state: &str) {
        use tauri_plugin_notification::NotificationExt;
        let Some(body) = notice_key(state) else {
            return;
        };
        // The notification plugin is registered in main.rs on every build.
        let shown = self
            .app
            .notification()
            .builder()
            .title(crate::t!("sync.notify.title"))
            .body(crate::t!(body))
            .show();
        if let Err(e) = shown {
            log::warn!("sync: couldn't show the {state} notification: {e}");
        }
    }

    fn track(&self, event: &'static str, props: Vec<(&'static str, String)>) {
        track_sync(&self.app, event, props);
    }
}

/// The body of the OS notification for a state the user must act on
/// (§13.8). Others never notify.
fn notice_key(state: &str) -> Option<&'static str> {
    Some(match state {
        "locked" => "sync.status.locked",
        "folder_missing" => "sync.status.folder_missing",
        "update_required" => "sync.status.update_required",
        "removed" => "sync.notify.removed",
        "reset" => "sync.notify.reset",
        _ => return None,
    })
}

// --- Telemetry (§16) ---------------------------------------------------------------------

const COUNT_0_4: &[&str] = &["0", "1", "2", "3", "4"];

/// The states of §13.4 and the problems shown beside them, as
/// `sync_problem`'s `kind`.
const PROBLEM_KINDS: &[&str] = &[
    "waiting",
    "placeholders",
    "paused",
    "locked",
    "folder_missing",
    "folder_denied",
    "folder_empty",
    "reset",
    "removed",
    "update_required",
    "disk_full",
    "write_failed",
    "folder_reverting",
    "other_engine",
    "clock_warning",
    "unreadable",
    "device_collision",
    "held_back",
    "not_this_version",
    "churn",
    "duplicates",
    "config_reset",
];

/// One telemetry event: its props, and every value each prop may take.
pub type EventSpec = (
    &'static str,
    &'static [(&'static str, &'static [&'static str])],
);

/// The sync events of §16. Nothing else reaches telemetry from sync, and
/// only with exactly these props and values: counts in buckets and fixed
/// Kage states, never names, paths, values or ids.
pub const SYNC_EVENTS: &[EventSpec] = &[
    (
        "sync_enabled",
        &[("role", &["create", "join"]), ("categories", COUNT_0_4)],
    ),
    (
        "sync_disabled",
        &[("reason", &["leave", "removed", "reset"])],
    ),
    (
        "sync_daily",
        &[
            ("computers", &["1", "2", "3-5", "6+"]),
            ("categories", COUNT_0_4),
            ("ext_data", &["0", "1-2", "3-5", "6+"]),
            ("conflicts_open", &["0", "1-5", "6+"]),
            ("held", &["0", "1+"]),
            ("problems", &["0", "1", "2+"]),
        ],
    ),
    (
        "sync_conflict_action",
        &[
            ("action", &["restore", "dismiss", "dismiss_all"]),
            (
                "category",
                &["settings", "automations", "extensions", "chats"],
            ),
        ],
    ),
    ("sync_problem", &[("kind", PROBLEM_KINDS)]),
    (
        "sync_scope_changed",
        &[(
            "action",
            &[
                "all",
                "os",
                "use_shared",
                "this_computer",
                "stop_this_computer",
                "avail_all",
                "avail_os",
                "avail_this_computer",
            ],
        )],
    ),
];

/// The props of a sync event as §16 allows them: `None` when the event
/// isn't listed, or a prop is missing, extra or has a value not listed.
pub fn allowed_props(event: &str, props: &[(&str, String)]) -> Option<Value> {
    let (_, spec) = SYNC_EVENTS.iter().find(|(e, _)| *e == event)?;
    let mut out = Map::new();
    for (name, value) in props {
        let (_, values) = spec.iter().find(|(n, _)| n == name)?;
        if !values.contains(&value.as_str()) {
            return None;
        }
        out.insert(name.to_string(), Value::String(value.clone()));
    }
    (out.len() == spec.len()).then_some(Value::Object(out))
}

/// Set once `app_daily_active` went out this run; the next status while
/// sync is on sends `sync_daily` with it.
static DAILY_DUE: AtomicBool = AtomicBool::new(false);

/// `app_daily_active` was sent: send `sync_daily` with it.
pub fn mark_daily_due() {
    DAILY_DUE.store(true, Ordering::SeqCst);
}

fn bucket(n: usize, buckets: &[(usize, &'static str)], last: &'static str) -> String {
    buckets
        .iter()
        .find(|(upto, _)| n <= *upto)
        .map_or(last, |(_, b)| b)
        .to_string()
}

/// `sync_daily`'s props for a status (§16).
pub fn daily_props(status: &SyncStatus, ext_data: usize) -> Vec<(&'static str, String)> {
    let computers = status
        .devices
        .iter()
        .filter(|d| d.state != DeviceState::Retired)
        .count()
        .max(1);
    vec![
        (
            "computers",
            bucket(computers, &[(1, "1"), (2, "2"), (5, "3-5")], "6+"),
        ),
        (
            "categories",
            [
                status.categories.settings,
                status.categories.automations,
                status.categories.extensions,
                status.categories.chats,
            ]
            .iter()
            .filter(|on| **on)
            .count()
            .to_string(),
        ),
        (
            "ext_data",
            bucket(ext_data, &[(0, "0"), (2, "1-2"), (5, "3-5")], "6+"),
        ),
        (
            "conflicts_open",
            bucket(
                status.conflicts_open as usize,
                &[(0, "0"), (5, "1-5")],
                "6+",
            ),
        ),
        ("held", bucket(status.held as usize, &[(0, "0")], "1+")),
        (
            "problems",
            bucket(status.problems.len(), &[(0, "0"), (1, "1")], "2+"),
        ),
    ]
}

/// While `sync_dismiss_all` runs: the categories of the entries it
/// dismissed, so one `dismiss_all` goes out per category instead of one
/// `dismiss` per entry.
static DISMISSING_ALL: Mutex<Option<BTreeSet<String>>> = Mutex::new(None);

/// `sync_dismiss_all` starts.
pub fn begin_dismiss_all() {
    *DISMISSING_ALL.lock_or_recover() = Some(BTreeSet::new());
}

/// `sync_dismiss_all` finished: one `sync_conflict_action` per category.
pub fn end_dismiss_all<R: Runtime>(app: &AppHandle<R>) {
    let categories = DISMISSING_ALL.lock_or_recover().take().unwrap_or_default();
    for category in categories {
        track_sync(
            app,
            "sync_conflict_action",
            vec![
                ("action", "dismiss_all".to_string()),
                ("category", category),
            ],
        );
    }
}

/// Send a sync event, if §16 allows it with these props.
fn track_sync<R: Runtime>(app: &AppHandle<R>, event: &str, props: Vec<(&str, String)>) {
    if event == "sync_conflict_action" {
        let mut dismissing = DISMISSING_ALL.lock_or_recover();
        if let Some(seen) = dismissing.as_mut() {
            let value = |name: &str| {
                props
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| v.clone())
            };
            if value("action").as_deref() == Some("dismiss") {
                if let Some(c) = value("category") {
                    seen.insert(c);
                }
                return;
            }
        }
    }
    match allowed_props(event, &props) {
        Some(p) => crate::telemetry::track(app, event, Some(p)),
        None => log::debug!(
            "sync: telemetry event {event} dropped: not with the props docs/design/SYNC.md §16 lists"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::status::DeviceView;

    #[test]
    fn a_revision_names_one_state_of_the_config() {
        let revs = ConfigRevisions::new();
        let mut c = Config::default();
        let r1 = revs.observe(&c);
        assert_eq!(
            revs.observe(&c.clone()),
            r1,
            "the same values, the same revision"
        );
        c.ui.theme = "dark".into();
        let r2 = revs.observe(&c);
        assert!(r2 > r1);
        assert_eq!(revs.at(r1).unwrap()["ui"]["theme"], json!("system"));
        assert_eq!(revs.at(r2).unwrap()["ui"]["theme"], json!("dark"));
        // A change back is a new revision too: the base of a later save is
        // what the window saw, not an older equal state.
        c.ui.theme = "system".into();
        assert!(revs.observe(&c) > r2);
    }

    #[test]
    fn only_the_last_revisions_are_kept() {
        let revs = ConfigRevisions::new();
        let mut c = Config::default();
        let first = revs.observe(&c);
        for n in 0..KEEP_REVISIONS as u8 {
            c.ui.font_size = 10 + n;
            revs.observe(&c);
        }
        assert!(revs.at(first).is_none());
        assert!(revs.at(first + 1).is_some());
    }

    #[test]
    fn a_token_is_for_one_action_and_one_use() {
        let c = Confirmations::default();
        let now = Instant::now();
        let t = c.issue_at(DestructiveAction::Leave, now);
        assert!(
            !c.take_at(&t, DestructiveAction::StartOver, now),
            "another action"
        );
        assert!(c.take_at(&t, DestructiveAction::Leave, now));
        assert!(!c.take_at(&t, DestructiveAction::Leave, now), "used up");
        assert!(!c.take_at("", DestructiveAction::Leave, now));
        let late = c.issue_at(DestructiveAction::UndoJoin, now);
        assert!(!c.take_at(&late, DestructiveAction::UndoJoin, now + CONFIRM_FOR));
        assert_eq!(c.issue(DestructiveAction::Leave).len(), 32);
    }

    #[test]
    fn the_exit_flush_has_its_budgets() {
        assert_eq!(exit_budget(false), Duration::from_millis(1500));
        assert_eq!(exit_budget(true), Duration::from_millis(500));
        assert_eq!(START_DELAY, Duration::from_secs(10));
    }

    #[test]
    fn the_fingerprint_follows_the_computer_and_the_profile() {
        let a = fingerprint_of("Work laptop", Path::new("/home/a/.config"));
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(
            a,
            fingerprint_of(" work LAPTOP ", Path::new("/home/a/.config"))
        );
        assert_ne!(a, fingerprint_of("Home Mac", Path::new("/home/a/.config")));
        assert_ne!(
            a,
            fingerprint_of("Work laptop", Path::new("/home/b/.config"))
        );
        assert_eq!(fingerprint(Path::new("/x")), fingerprint(Path::new("/x")));
    }

    #[test]
    fn this_os_is_the_build_target() {
        assert_eq!(
            this_os().as_str(),
            match std::env::consts::OS {
                "macos" => "macos",
                "windows" => "windows",
                _ => "linux",
            }
        );
    }

    #[test]
    fn only_states_the_user_must_act_on_notify() {
        for state in [
            "locked",
            "folder_missing",
            "update_required",
            "removed",
            "reset",
        ] {
            let key = notice_key(state).unwrap();
            assert_ne!(crate::i18n::translate_in("en", key, &[]), key);
        }
        for state in ["up_to_date", "paused", "clock_warning", "waiting"] {
            assert!(notice_key(state).is_none(), "{state}");
        }
        assert_ne!(
            crate::i18n::translate_in("en", "sync.notify.title", &[]),
            "sync.notify.title"
        );
    }

    #[test]
    fn telemetry_takes_only_the_listed_props_and_values() {
        let p = |v: &[(&'static str, &str)]| -> Vec<(&'static str, String)> {
            v.iter().map(|(n, x)| (*n, x.to_string())).collect()
        };
        assert_eq!(
            allowed_props("sync_enabled", &p(&[("role", "join"), ("categories", "3")])),
            Some(json!({ "role": "join", "categories": "3" }))
        );
        assert!(
            allowed_props("sync_enabled", &p(&[("role", "join")])).is_none(),
            "missing"
        );
        assert!(
            allowed_props(
                "sync_enabled",
                &p(&[("role", "join"), ("categories", "3"), ("folder", "Dropbox")])
            )
            .is_none(),
            "extra"
        );
        assert!(
            allowed_props(
                "sync_enabled",
                &p(&[("role", "maybe"), ("categories", "3")])
            )
            .is_none(),
            "a value not listed"
        );
        assert!(allowed_props("sync_secret", &[]).is_none());
        assert!(allowed_props("sync_problem", &p(&[("kind", "Work PC")])).is_none());
    }

    fn status_with(devices: usize, conflicts: u32, held: u32, problems: usize) -> SyncStatus {
        SyncStatus {
            state: SyncState::UpToDate,
            devices: (0..devices)
                .map(|n| DeviceView {
                    id: format!("d{n}"),
                    name: format!("Computer {n}"),
                    os: OsKind::Linux,
                    kage_version: "1".into(),
                    last_seen_at: 0,
                    this: n == 0,
                    state: DeviceState::Ok,
                })
                .collect(),
            conflicts_open: conflicts,
            held,
            problems: (0..problems)
                .map(|_| crate::sync::status::Problem {
                    kind: "churn".into(),
                    key: "sync.problem.churn".into(),
                    params: Default::default(),
                })
                .collect(),
            ..SyncStatus::default()
        }
    }

    #[test]
    fn daily_props_are_buckets_section_16_lists() {
        for (devices, conflicts, held, problems, ext) in [
            (1, 0, 0, 0, 0),
            (2, 3, 1, 1, 2),
            (4, 6, 9, 2, 4),
            (9, 40, 0, 7, 12),
            (0, 1, 0, 0, 1),
        ] {
            let props = daily_props(&status_with(devices, conflicts, held, problems), ext);
            assert!(
                allowed_props("sync_daily", &props).is_some(),
                "{props:?} isn't what §16 lists"
            );
        }
        let props = daily_props(&status_with(4, 6, 9, 2), 4);
        let get = |n: &str| props.iter().find(|(k, _)| *k == n).unwrap().1.clone();
        assert_eq!(get("computers"), "3-5");
        assert_eq!(get("conflicts_open"), "6+");
        assert_eq!(get("held"), "1+");
        assert_eq!(get("problems"), "2+");
        assert_eq!(get("ext_data"), "3-5");
        assert_eq!(get("categories"), "3");
    }

    /// Every `track` call of the sync engine and of this module sends an
    /// event §16 lists, with exactly its props; string values written at
    /// the call site are ones §16 allows.
    #[test]
    fn telemetry_call_sites_match_section_16() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![root.join("sync_app.rs")];
        let mut dirs = vec![root.join("sync"), root.join("sync_app")];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    files.push(p);
                }
            }
        }
        let tuple = regex::Regex::new(r#"\(\s*"([a-z_]+)"\s*,\s*("([a-z_0-9+-]+)")?"#).unwrap();
        let first = regex::Regex::new(r#"^\s*"([a-z_]+)""#).unwrap();
        let mut seen = BTreeSet::new();
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap();
            for (at, _) in src
                .match_indices(".track(")
                .chain(src.match_indices("track_sync("))
            {
                let start = at + src[at..].find('(').unwrap() + 1;
                let mut depth = 1;
                let mut end = start;
                for (i, ch) in src[start..].char_indices() {
                    match ch {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    if depth == 0 {
                        end = start + i;
                        break;
                    }
                }
                let call = &src[start..end];
                // `track_sync(app, "event", …)`: skip the handle argument.
                let call = call
                    .split_once(',')
                    .filter(|_| !call.trim_start().starts_with('"'))
                    .map_or(call, |(_, rest)| rest);
                let Some(event) = first.captures(call).map(|c| c[1].to_string()) else {
                    continue; // a forwarded name (`event`), not a call site
                };
                if !event.starts_with("sync_") {
                    continue;
                }
                let (_, spec) = SYNC_EVENTS
                    .iter()
                    .find(|(e, _)| *e == event)
                    .unwrap_or_else(|| panic!("{}: {event} isn't in §16", f.display()));
                let mut names = BTreeSet::new();
                for c in tuple.captures_iter(call) {
                    let name = c[1].to_string();
                    let (_, values) = spec
                        .iter()
                        .find(|(n, _)| *n == name)
                        .unwrap_or_else(|| panic!("{}: {event} has no prop {name}", f.display()));
                    if let Some(v) = c.get(3) {
                        assert!(
                            values.contains(&v.as_str()),
                            "{event}.{name} = {}",
                            v.as_str()
                        );
                    }
                    names.insert(name);
                }
                // `sync_daily`'s props come from `daily_props`, tested above.
                if event != "sync_daily" {
                    let want: BTreeSet<String> = spec.iter().map(|(n, _)| n.to_string()).collect();
                    assert_eq!(names, want, "{}: {event}", f.display());
                }
                seen.insert(event);
            }
        }
        for (event, _) in SYNC_EVENTS {
            assert!(seen.contains(*event), "nothing sends {event}");
        }
    }

    /// The events are listed for review, and the privacy policy says what
    /// sync sends.
    #[test]
    fn sync_events_are_disclosed() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let js = std::fs::read_to_string(root.join("ui/js/shared/telemetry.js")).unwrap();
        for (event, _) in SYNC_EVENTS {
            assert!(
                js.contains(&format!("'{event}'")),
                "{event} isn't in KNOWN_EVENTS"
            );
        }
        let privacy = std::fs::read_to_string(root.join("docs/PRIVACY.md")).unwrap();
        assert!(privacy.contains("Sync data never touches Kage servers"));
    }
}
