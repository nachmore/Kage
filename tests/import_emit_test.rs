//! `import_config_bundle` must emit `CONFIG_IMPORTED` after `CONFIG_UPDATED`
//! for a FULL backup, and emit ONLY `CONFIG_UPDATED` for a settings-only
//! backup. The frontend's chat/floating listeners force-remount every
//! loaded extension sandbox on `CONFIG_IMPORTED` so stale pre-import
//! sandbox state (spotify "no creds" cache, calendar `_events` …) is
//! dropped. Settings-only imports leave extension data alone, so no
//! force-remount is needed — and we must NOT fire the event, otherwise
//! the fast-path optimisation is defeated for sync-applied/regular saves
//! too (both of which also route through `CONFIG_UPDATED`).

use kage::acp_client::{AcpClient, AcpConnectionMode};
use kage::config::Config;
use kage::config_export::{self, ExportMode};
use kage::state::{build_managed_state, ManagedState};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tauri::test::{mock_builder, MockRuntime};
use tauri::Listener;

/// Tests that save `config.json` (one isolated file per process) take turns.
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn isolate_user_dirs() {
    let root = std::env::temp_dir().join(format!(
        "kage-import-emit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    kage::paths::isolate(&root);
    assert!(
        kage::paths::is_isolated(),
        "refusing to run against the real per-user directories"
    );
}

fn mock_app() -> tauri::App<MockRuntime> {
    isolate_user_dirs();
    kage::os::login_env::set_enabled(false);
    let config = Arc::new(Mutex::new(Config::default()));
    let acp_client = Arc::new(AcpClient::new(AcpConnectionMode::Local {
        spawn_command: "true".to_string(),
    }));
    let ManagedState {
        acp_handles,
        ui_state,
        child_processes,
        feature_services,
    } = build_managed_state(config, acp_client, false);
    mock_builder()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
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

fn count(app: &tauri::App<MockRuntime>, event: &str) -> Arc<AtomicUsize> {
    let n = Arc::new(AtomicUsize::new(0));
    let m = n.clone();
    app.listen_any(event, move |_| {
        m.fetch_add(1, Ordering::SeqCst);
    });
    n
}

/// Write bytes to a tempfile in the test-isolated scratch root.
fn write_tmp(bytes: &[u8], name: &str) -> std::path::PathBuf {
    let dir = kage::paths::cache_dir()
        .expect("cache dir")
        .join("import-emit-test");
    std::fs::create_dir_all(&dir).expect("mkdir cache");
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write tmp");
    path
}

#[test]
fn full_backup_import_emits_both_config_updated_and_config_imported() {
    let _serial = serial();
    let app = mock_app();
    let w = window(&app);
    let updated = count(&app, kage::events::CONFIG_UPDATED);
    let imported = count(&app, kage::events::CONFIG_IMPORTED);

    // Produce a FULL (encrypted) backup from an empty default config.
    let bundle = config_export::export(
        &Config::default(),
        ExportMode::Encrypted,
        Some("test-passphrase"),
        &[],
    )
    .expect("build full backup");
    let path = write_tmp(&bundle, "full.kage-backup");

    let result = invoke(
        &w,
        "import_config_bundle",
        json!({ "path": path.to_string_lossy(), "passphrase": "test-passphrase" }),
    )
    .expect("import_config_bundle (full)");
    // Sanity: it ran to completion with the Full kind.
    assert_eq!(result["kind"].as_str(), Some("full"));

    assert_eq!(
        updated.load(Ordering::SeqCst),
        1,
        "config_updated must fire exactly once for a full import"
    );
    assert_eq!(
        imported.load(Ordering::SeqCst),
        1,
        "config_imported must fire exactly once for a full import"
    );
}

#[test]
fn settings_only_import_emits_only_config_updated() {
    let _serial = serial();
    let app = mock_app();
    let w = window(&app);
    let updated = count(&app, kage::events::CONFIG_UPDATED);
    let imported = count(&app, kage::events::CONFIG_IMPORTED);

    // A settings-only backup is unencrypted.
    let bundle = config_export::export(&Config::default(), ExportMode::SettingsOnly, None, &[])
        .expect("build settings-only backup");
    let path = write_tmp(&bundle, "settings-only.kage-backup");

    let result = invoke(
        &w,
        "import_config_bundle",
        json!({ "path": path.to_string_lossy(), "passphrase": Value::Null }),
    )
    .expect("import_config_bundle (settings-only)");
    assert_eq!(result["kind"].as_str(), Some("settings_only"));

    assert_eq!(
        updated.load(Ordering::SeqCst),
        1,
        "config_updated must fire exactly once"
    );
    assert_eq!(
        imported.load(Ordering::SeqCst),
        0,
        "config_imported must NOT fire for a settings-only import — \
         the fast-path optimisation must stay intact"
    );
}
