//! The app side of folder sync (docs/design/SYNC.md WP-S4) on a mock app:
//! the commands before and after the engine thread starts, the start
//! delay, `save_config`'s three-way merge (I5), synced values reaching the
//! app through its config port and events, and the exit flush and its
//! budget. Every test isolates user dirs first (`sync_support::isolate`).

mod sync_support;

use kage::acp_client::{AcpClient, AcpConnectionMode};
use kage::state::{build_managed_state, FeatureServices, ManagedState, UiState};
use kage::sync::engine::{EngineEnv, SyncEngine};
use kage::sync::prefs::{DevicePrefs, SyncCategories};
use kage::sync::sources::{self, HostDirs, Source};
use kage::sync::testing::{FakeClock, FakeCloudFiles, Fault, FaultyFs, FsOp, MemoryKeyStore};
use kage::sync::types::OsKind;
use kage::sync::{EngineRequest, JoinChoice, JoinReq};
use kage::sync_app::{TauriConfigPort, TauriHostEvents};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use sync_support::*;
use tauri::test::{mock_builder, MockRuntime};
use tauri::{Listener, Manager};

/// Tests that save `config.json` (one isolated file for the process) take
/// turns.
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// A mock app managing exactly the state production manages.
fn app() -> tauri::App<MockRuntime> {
    isolate();
    assert!(kage::paths::is_isolated());
    // Nothing here may run the developer's shell start-up files.
    kage::os::login_env::set_enabled(false);
    let config = Arc::new(Mutex::new(kage::config::Config::default()));
    let acp = Arc::new(AcpClient::new(AcpConnectionMode::Local {
        spawn_command: "true".to_string(),
    }));
    let ManagedState {
        acp_handles,
        ui_state,
        child_processes,
        feature_services,
    } = build_managed_state(config, acp, false);
    mock_builder()
        .plugin(tauri_plugin_notification::init())
        .manage(acp_handles)
        .manage(ui_state)
        .manage(child_processes)
        .manage(feature_services)
        .invoke_handler(kage::commands::registry::invoke_handler())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app")
}

fn window(app: &tauri::App<MockRuntime>) -> tauri::WebviewWindow<MockRuntime> {
    tauri::WebviewWindowBuilder::new(app, "settings", Default::default())
        .build()
        .expect("mock window")
}

/// Call a command through real IPC dispatch.
fn invoke(w: &tauri::WebviewWindow<MockRuntime>, cmd: &str, args: Value) -> Result<Value, Value> {
    tauri::test::get_ipc_response(
        w,
        tauri::webview::InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
    .map(|b| b.deserialize::<Value>().unwrap())
}

fn kind(e: Value) -> String {
    e["kind"].as_str().unwrap_or_default().to_string()
}

/// Count `event` on the app.
fn count(app: &tauri::App<MockRuntime>, event: &str) -> Arc<AtomicUsize> {
    let n = Arc::new(AtomicUsize::new(0));
    let m = n.clone();
    app.listen_any(event, move |_| {
        m.fetch_add(1, Ordering::SeqCst);
    });
    n
}

/// Before the engine thread runs nothing touches sync's folder; it starts
/// late, or as soon as a window asks; destructive commands need a token
/// and a token is used once.
#[test]
fn commands_wait_for_the_engine_and_destructive_ones_for_a_token() {
    let app = app();
    let w = window(&app);
    let sync_dir = kage::paths::data_local_dir().unwrap().join("kage/sync");
    let abs = std::env::temp_dir().to_string_lossy().into_owned();

    // No engine thread (a build without sync, the mock-app sweep).
    let st = invoke(&w, "sync_get_status", json!({})).unwrap();
    assert_eq!(st["state"], "unavailable");
    assert_eq!(
        invoke(&w, "sync_suggest_folders", json!({})).unwrap(),
        json!([])
    );
    for (cmd, args) in [
        ("sync_inspect_folder", json!({ "path": abs })),
        ("sync_generate_passphrase", json!({})),
        ("sync_now", json!({})),
        ("sync_list_devices", json!({})),
        ("sync_prepare_destructive", json!({ "action": "leave" })),
    ] {
        assert_eq!(
            kind(invoke(&w, cmd, args).unwrap_err()),
            "sync_unavailable",
            "{cmd}"
        );
    }
    for (cmd, args) in [
        ("sync_leave", json!({ "confirm": "made-up" })),
        ("sync_undo_join", json!({ "confirm": "" })),
        (
            "sync_start_over",
            json!({ "passphrase": PASS, "confirm": "x" }),
        ),
        (
            "sync_purge_extension",
            json!({ "extensionId": "mindmap", "confirm": "x" }),
        ),
        (
            "sync_remove_device",
            json!({ "deviceId": "d", "lost": false, "confirm": "x" }),
        ),
    ] {
        assert_eq!(
            kind(invoke(&w, cmd, args).unwrap_err()),
            "needs_confirmation",
            "{cmd}"
        );
    }
    assert!(!sync_dir.join("engine.lock").exists(), "nothing written");

    // Setup starts the thread; it waits before it touches anything.
    app.state::<UiState>()
        .frontend_ready
        .store(true, Ordering::SeqCst);
    kage::sync_app::start(app.handle(), kage::sync_app::START_DELAY);
    assert!(app.state::<FeatureServices>().sync.started());
    app.state::<FeatureServices>()
        .sync
        .wake(kage::sync::engine::Wake::Focus);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !sync_dir.join("engine.lock").exists(),
        "no folder or database I/O before the start delay"
    );

    // A window asks: the engine opens now. Sync isn't set up here.
    let st = invoke(&w, "sync_get_status", json!({})).unwrap();
    assert_eq!(st["state"], "off");
    assert!(sync_dir.join("engine.lock").exists());
    let pass = invoke(&w, "sync_generate_passphrase", json!({})).unwrap();
    assert!(pass.as_str().unwrap().len() >= 25);
    let info = invoke(&w, "sync_inspect_folder", json!({ "path": abs })).unwrap();
    assert!(info["kind"].is_string());

    // A token, used once. With sync off, the engine then says so.
    let token = invoke(&w, "sync_prepare_destructive", json!({ "action": "leave" })).unwrap();
    let token = token.as_str().unwrap();
    let wrong = invoke(&w, "sync_undo_join", json!({ "confirm": token })).unwrap_err();
    assert_eq!(
        kind(wrong),
        "needs_confirmation",
        "a token is for one action"
    );
    let off = invoke(&w, "sync_leave", json!({ "confirm": token })).unwrap_err();
    assert_eq!(kind(off), "sync_unavailable");
    let again = invoke(&w, "sync_leave", json!({ "confirm": token })).unwrap_err();
    assert_eq!(kind(again), "needs_confirmation", "a token is used once");

    // Arguments the engine never sees.
    for (cmd, args) in [
        ("sync_inspect_folder", json!({ "path": "Dropbox" })),
        ("sync_apply_folder", json!({ "path": "" })),
        ("sync_set_paused", json!({ "until": "tomorrow" })),
        (
            "sync_set_extension_data",
            json!({ "extensionId": "../x", "on": true }),
        ),
    ] {
        assert_eq!(
            kind(invoke(&w, cmd, args).unwrap_err()),
            "invalid_input",
            "{cmd}"
        );
    }
    let on = invoke(&w, "sync_set_paused", json!({ "until": "forever" })).unwrap_err();
    assert_eq!(
        kind(on),
        "sync_unavailable",
        "a valid pause reaches the engine"
    );

    // At exit with nothing to send, the flush returns at once.
    let t = Instant::now();
    kage::sync_app::flush_for_exit(app.handle(), kage::sync_app::EXIT_FLUSH);
    assert!(t.elapsed() < Duration::from_secs(1));
}

/// I5: a window holding an old copy saves another field; the change made
/// since (another window, or sync) survives, and the window's edit lands.
#[test]
fn a_stale_save_keeps_what_changed_since() {
    let _serial = serial();
    let app = app();
    let w = window(&app);
    let snap = invoke(&w, "get_config_snapshot", json!({})).unwrap();
    let r0 = snap["revision"].as_u64().unwrap();
    let stale = snap["config"].clone();

    let mut other = stale.clone();
    other["ui"]["theme"] = json!("dark");
    other["shortcuts"] = json!([{ "name": "gh", "url": "https://github.com" }]);
    let r1 = invoke(
        &w,
        "save_config",
        json!({ "config": other, "baseRevision": r0 }),
    )
    .unwrap();
    let r1 = r1["revision"].as_u64().unwrap();
    assert!(r1 > r0);

    let mut mine = stale.clone();
    mine["ui"]["font_size"] = json!(18);
    let r2 = invoke(
        &w,
        "save_config",
        json!({ "config": mine, "baseRevision": r0 }),
    )
    .unwrap();
    let r2 = r2["revision"].as_u64().unwrap();
    assert!(r2 > r1);

    let now = invoke(&w, "get_config_snapshot", json!({})).unwrap();
    assert_eq!(now["revision"].as_u64(), Some(r2));
    assert_eq!(
        now["config"]["ui"]["theme"], "dark",
        "the other window's change stays"
    );
    assert_eq!(
        now["config"]["ui"]["font_size"], 18,
        "this window's change lands"
    );
    let shortcuts = now["config"]["shortcuts"].as_array().unwrap();
    assert_eq!(shortcuts.len(), 1, "an item added elsewhere stays");
    assert!(
        !shortcuts[0]["sync_id"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "saved items get a stable id"
    );
    let disk = std::fs::read_to_string(kage::paths::config_dir().unwrap().join("kage/config.json"))
        .unwrap();
    let disk: Value = serde_json::from_str(&disk).unwrap();
    assert_eq!(disk["ui"]["theme"], "dark");
    assert_eq!(disk["ui"]["font_size"], 18);

    // Without a base, a save is the whole config as sent (as before).
    let r3 = invoke(&w, "save_config", json!({ "config": stale })).unwrap();
    assert!(r3["revision"].as_u64().unwrap() > r2);
    let back = invoke(&w, "get_config_snapshot", json!({})).unwrap();
    assert_eq!(back["config"]["ui"]["theme"], stale["ui"]["theme"]);
}

/// This computer (`work`) in the app, on a folder `home` shares.
struct Work {
    local: PathBuf,
    config_dir: PathBuf,
    keys: Arc<MemoryKeyStore>,
    fs: Arc<FaultyFs>,
    clock: Arc<FakeClock>,
    _tmp: tempfile::TempDir,
}

impl Work {
    fn new(home: &SimComputer, clock: Arc<FakeClock>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        Self {
            local: tmp.path().join("data_local/kage/sync"),
            config_dir: tmp.path().join("config/kage"),
            keys: Arc::new(MemoryKeyStore::new()),
            fs: home.fs.clone(),
            clock,
            _tmp: tmp,
        }
    }

    /// The engine's environment, with the app's real ports.
    fn env<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
    ) -> (EngineEnv, Vec<Box<dyn Source>>) {
        let features = app.state::<FeatureServices>();
        let port = Arc::new(TauriConfigPort::new(
            app.clone(),
            features.config.clone(),
            features.config_revisions.clone(),
        ));
        let events = Arc::new(TauriHostEvents::new(
            app.clone(),
            self.local.clone(),
            port.clone(),
        ));
        let sources = sources::host_sources(
            port.clone(),
            HostDirs {
                config_kage_dir: self.config_dir.clone(),
                sync_dir: self.local.clone(),
            },
            OsKind::Linux,
            None,
        );
        let env = EngineEnv {
            local_dir: self.local.clone(),
            os: OsKind::Linux,
            kage_version: "0.12.0+test".into(),
            fingerprint: "fp-work".into(),
            fs: self.fs.clone(),
            cloud: Arc::new(FakeCloudFiles::new()),
            keys: self.keys.clone(),
            clock: self.clock.clone(),
            events,
            config: port,
        };
        (env, sources)
    }

    /// Open the engine on this thread and join `home`'s folder.
    fn join<R: tauri::Runtime>(&self, app: &tauri::AppHandle<R>) -> SyncEngine {
        let (env, sources) = self.env(app);
        let mut engine = SyncEngine::open(env, sources).unwrap();
        engine.set_kdf_cost(TEST_KDF);
        engine
            .handle(EngineRequest::Join(JoinReq {
                path: PathBuf::from(CHOSEN),
                passphrase: PASS.into(),
                device_name: "Work PC".into(),
                categories: SyncCategories::default(),
            }))
            .unwrap_or_else(|e| panic!("join: {}", e.key));
        engine
            .handle(EngineRequest::ConfirmJoin(JoinChoice::Theirs))
            .unwrap_or_else(|e| panic!("confirm: {}", e.key));
        engine
    }

    /// The heads this computer published.
    fn heads(&self) -> usize {
        let id = DevicePrefs::load(&self.local).device_id.unwrap();
        let dir = Path::new(CHOSEN).join("Kage Sync/devices").join(id);
        self.fs
            .files_under(&dir)
            .iter()
            .filter(|(p, _)| {
                p.parent() == Some(dir.as_path())
                    && p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("head-")
            })
            .count()
    }
}

/// A value synced from another computer reaches the app's config through
/// its port (one save, `sync_applied` before `config_updated`, a new
/// revision); a stale Settings window then saves another field, and that
/// reaches the other computer without reverting anything.
#[test]
fn synced_values_reach_the_app_through_its_ports() {
    let _serial = serial();
    let clock = clock();
    let mut home = SimComputer::new("Home Mac", OsKind::Macos, clock.clone());
    home.create();
    let app = app();
    let w = window(&app);
    let applied = count(&app, kage::events::SYNC_APPLIED);
    let updated = count(&app, kage::events::CONFIG_UPDATED);
    let statuses = count(&app, kage::events::SYNC_STATUS_CHANGED);
    let work = Work::new(&home, clock.clone());
    let mut engine = work.join(app.handle());
    assert!(
        statuses.load(Ordering::SeqCst) > 0,
        "status changes reach the windows"
    );

    // A Settings window opens now.
    let snap = invoke(&w, "get_config_snapshot", json!({})).unwrap();
    let r0 = snap["revision"].as_u64().unwrap();
    let (applied0, updated0) = (
        applied.load(Ordering::SeqCst),
        updated.load(Ordering::SeqCst),
    );

    home.edit(|c| c.ui.theme = "dark".into());
    home.tick();
    engine.tick(kage::sync::engine::Wake::Manual);
    let features = app.state::<FeatureServices>();
    assert_eq!(features.config.lock().unwrap().ui.theme, "dark");
    assert!(applied.load(Ordering::SeqCst) > applied0);
    assert!(updated.load(Ordering::SeqCst) > updated0);
    let r1 = invoke(&w, "get_config_snapshot", json!({})).unwrap()["revision"]
        .as_u64()
        .unwrap();
    assert!(r1 > r0, "sync's save is a new revision");
    assert_eq!(
        kage::sync_app::last_status().map(|s| s.state),
        Some(engine.status().state)
    );

    // The stale window changes the font size.
    let mut stale = snap["config"].clone();
    stale["ui"]["font_size"] = json!(18);
    invoke(
        &w,
        "save_config",
        json!({ "config": stale, "baseRevision": r0 }),
    )
    .unwrap();
    {
        let c = features.config.lock().unwrap();
        assert_eq!((c.ui.theme.as_str(), c.ui.font_size), ("dark", 18));
    }
    engine.tick(kage::sync::engine::Wake::Manual);
    home.tick();
    assert_eq!(home.config().ui.font_size, 18);
    assert_eq!(home.config().ui.theme, "dark", "nothing reverted");
    assert!(home.conflicts().is_empty());
}

/// The exit flush publishes this computer's edits, and returns within its
/// budget when the folder is slow (500 ms on the updater path).
#[test]
fn the_exit_flush_publishes_within_its_budget() {
    let _serial = serial();
    let clock = clock();
    let mut home = SimComputer::new("Home Mac", OsKind::Macos, clock.clone());
    home.create();
    let app = app();
    let w = window(&app);
    let work = Arc::new(Work::new(&home, clock.clone()));
    drop(work.join(app.handle()));

    // The app's engine thread, on the same directories.
    let features = app.state::<FeatureServices>();
    let (handle, w2) = (app.handle().clone(), work.clone());
    features
        .sync
        .start(move || Ok(w2.env(&handle)), Duration::ZERO);
    let st = invoke(&w, "sync_get_status", json!({})).unwrap();
    assert_ne!(st["state"], "off");
    assert_ne!(st["state"], "locked");

    let before = work.heads();
    features.config.lock().unwrap().ui.theme = "dark".into();
    let t = Instant::now();
    kage::sync_app::flush_for_exit(app.handle(), kage::sync_app::EXIT_FLUSH);
    assert!(t.elapsed() < kage::sync_app::EXIT_FLUSH + Duration::from_millis(300));
    assert!(work.heads() > before, "the edit was published at exit");
    home.tick();
    assert_eq!(home.config().ui.theme, "dark");

    // A folder that takes seconds to write: the flush gives up in time.
    work.fs
        .inject(FsOp::Write, CHOSEN, Fault::Slow(Duration::from_secs(3)));
    features.config.lock().unwrap().ui.font_size = 20;
    let t = Instant::now();
    kage::sync_app::flush_for_exit(app.handle(), kage::sync_app::EXIT_FLUSH_UPDATE);
    let took = t.elapsed();
    assert!(
        took < kage::sync_app::EXIT_FLUSH_UPDATE + Duration::from_millis(400),
        "took {took:?}"
    );
    work.fs.clear_faults();
}
