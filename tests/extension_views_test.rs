//! Extension view windows on a mock Tauri app
//! (docs/design/EXTENSION_VIEWS.md §22.2): open, reopen, boot, the
//! refusals of `ext_view_open`, and the windows going away when the
//! extension is turned off, uninstalled or loses its grant.
//!
//! Every per-user directory is redirected to a scratch dir first
//! (`kage::paths::isolate`), so nothing here touches a real config.
//! Each test installs its own fake extension under its own id, so tests
//! can run in parallel.
//!
//! The mock runtime never delivers window events, so a destroyed window
//! stays in Tauri's window list. These tests check the registry
//! (`ext_view_list`, `ext_view_boot`) instead of the window list.

use kage::acp_client::{AcpClient, AcpConnectionMode};
use kage::state::{build_managed_state, FeatureServices, ManagedState};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tauri::test::{mock_builder, MockRuntime};
use tauri::{Listener, Manager};

fn isolate_user_dirs() {
    let root = std::env::temp_dir().join(format!("kage-ext-views-test-{}", std::process::id()));
    kage::paths::isolate(&root);
    assert!(
        kage::paths::is_isolated(),
        "refusing to run against the real per-user directories"
    );
}

fn mock_app() -> tauri::App<MockRuntime> {
    isolate_user_dirs();
    kage::os::login_env::set_enabled(false);
    let config = Arc::new(std::sync::Mutex::new(kage::config::Config::default()));
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
        .manage(acp_handles)
        .manage(ui_state)
        .manage(child_processes)
        .manage(feature_services)
        .invoke_handler(kage::commands::registry::invoke_handler())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app must build")
}

fn mock_window(app: &tauri::App<MockRuntime>, label: &str) -> tauri::WebviewWindow<MockRuntime> {
    tauri::WebviewWindowBuilder::new(app, label, Default::default())
        .build()
        .unwrap_or_else(|e| panic!("mock webview '{label}' must build: {e}"))
}

fn invoke(
    webview: &tauri::WebviewWindow<MockRuntime>,
    cmd: &str,
    args: Value,
) -> Result<Value, Value> {
    tauri::test::get_ipc_response(
        webview,
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
    .map(|body| match body {
        tauri::ipc::InvokeResponseBody::Json(s) => serde_json::from_str(&s).unwrap(),
        tauri::ipc::InvokeResponseBody::Raw(_) => panic!("{cmd} answered raw bytes"),
    })
}

/// The i18n key of a refused invoke.
fn err_key(result: Result<Value, Value>) -> String {
    match result {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => e["key"].as_str().unwrap_or_default().to_string(),
    }
}

fn extensions_dir() -> PathBuf {
    kage::paths::config_dir()
        .unwrap()
        .join("kage")
        .join("extensions")
}

/// Install a fake extension `id` with an `explorer` view, asking for
/// `storage` and `views`.
fn install(id: &str) {
    let dir = extensions_dir().join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = json!({
        "id": id,
        "name": format!("Test {id}"),
        "version": "1.0.0",
        "type": "extension",
        "icon": "🧪",
        "permissions": ["storage", "views"],
        "contributes": { "views": [
            { "id": "explorer", "title": "Explorer", "module": "./explorer.js" }
        ] }
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
    std::fs::write(
        dir.join("explorer.js"),
        "export default class { mount() {} }",
    )
    .unwrap();
}

fn grant(app: &tauri::App<MockRuntime>, id: &str, caps: &[&str]) {
    let features = app.state::<FeatureServices>();
    features.config.lock().unwrap().extension_grants.insert(
        id.to_string(),
        kage::config::ExtensionGrant {
            granted: caps.iter().map(|c| c.to_string()).collect(),
            approved_version: "1.0.0".into(),
            approved_at: String::new(),
            origin: None,
        },
    );
}

fn open_args(id: &str, view: &str, params: Value, source: &str) -> Value {
    json!({ "extensionId": id, "viewId": view, "params": params, "source": source })
}

fn view_list(app: &tauri::App<MockRuntime>) -> Vec<Value> {
    let settings = app
        .get_webview_window("settings")
        .unwrap_or_else(|| mock_window(app, "settings"));
    match invoke(&settings, "ext_view_list", json!({})).unwrap() {
        Value::Array(list) => list,
        other => panic!("ext_view_list answered {other}"),
    }
}

#[test]
fn open_builds_the_window_then_reuses_it() {
    let app = mock_app();
    install("views-open");
    grant(&app, "views-open", &["storage", "views"]);
    let floating = mock_window(&app, "floating");

    let opened = invoke(
        &floating,
        "ext_view_open",
        open_args(
            "views-open",
            "explorer",
            json!({ "focus": "x" }),
            "launcher",
        ),
    )
    .expect("open with a grant");
    assert_eq!(
        opened,
        json!({ "label": "ext-view:views-open:explorer", "reused": false })
    );
    let view = app
        .get_webview_window("ext-view:views-open:explorer")
        .expect("the view window exists");

    let boot = invoke(&view, "ext_view_boot", json!({})).expect("boot");
    assert_eq!(
        boot,
        json!({
            "extension_id": "views-open", "view_id": "explorer",
            "params": { "focus": "x" }, "open_seq": 1,
            "source": "launcher", "reloaded": false
        })
    );

    let (tx, rx) = mpsc::channel::<String>();
    view.listen("ext_view_reopen", move |event| {
        let _ = tx.send(event.payload().to_string());
    });
    let again = invoke(
        &floating,
        "ext_view_open",
        open_args("views-open", "explorer", json!({ "focus": "y" }), "chat"),
    )
    .expect("second open");
    assert_eq!(
        again,
        json!({ "label": "ext-view:views-open:explorer", "reused": true })
    );
    let payload = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("ext_view_reopen reaches the booted view");
    let payload: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(
        payload,
        json!({ "params": { "focus": "y" }, "open_seq": 2, "source": "chat" })
    );

    // A second boot is a reload in place with the newest params; an
    // unknown source is read as the launcher.
    let boot = invoke(&view, "ext_view_boot", json!({})).unwrap();
    assert_eq!(boot["reloaded"], true);
    assert_eq!(boot["open_seq"], 2);
    assert_eq!(boot["params"], json!({ "focus": "y" }));
    invoke(
        &floating,
        "ext_view_open",
        open_args("views-open", "explorer", Value::Null, "deep-link"),
    )
    .unwrap();
    assert_eq!(
        invoke(&view, "ext_view_boot", json!({})).unwrap()["source"],
        "launcher"
    );

    // A view may open views of its own extension.
    let from_view = invoke(
        &view,
        "ext_view_open",
        open_args("views-open", "explorer", Value::Null, "view"),
    )
    .unwrap();
    assert_eq!(from_view["reused"], true);

    let list = view_list(&app);
    assert_eq!(
        list,
        vec![json!({
            "extension_id": "views-open", "view_id": "explorer",
            "visible": true, "focused": false
        })]
    );
}

#[test]
fn open_refusals() {
    let app = mock_app();
    install("views-refuse");
    let floating = mock_window(&app, "floating");
    let open = |window: &tauri::WebviewWindow<MockRuntime>, id: &str, view: &str, params: Value| {
        err_key(invoke(
            window,
            "ext_view_open",
            open_args(id, view, params, "launcher"),
        ))
    };

    assert_eq!(
        open(&floating, "views-refuse", "explorer", Value::Null),
        "errors.ext_view.not_granted"
    );
    grant(&app, "views-refuse", &["storage"]);
    assert_eq!(
        open(&floating, "views-refuse", "explorer", Value::Null),
        "errors.ext_view.not_granted"
    );
    grant(&app, "views-refuse", &["storage", "views"]);

    assert_eq!(
        open(&floating, "views-refuse", "board", Value::Null),
        "errors.ext_view.unknown_view"
    );
    assert_eq!(
        open(&floating, "views-none-such", "explorer", Value::Null),
        "errors.ext_view.unknown_extension"
    );
    assert_eq!(
        open(&floating, "Bad:Id", "explorer", Value::Null),
        "errors.ext_view.unknown_extension"
    );
    assert_eq!(
        open(&floating, "views-refuse", "explorer", json!([1, 2])),
        "errors.ext_view.params_invalid"
    );
    assert_eq!(
        open(
            &floating,
            "views-refuse",
            "explorer",
            json!({ "s": "x".repeat(5000) })
        ),
        "errors.ext_view.params_too_large"
    );

    let store = mock_window(&app, "store");
    assert_eq!(
        open(&store, "views-refuse", "explorer", Value::Null),
        "errors.ext_view.opener_refused"
    );
    let other_view = mock_window(&app, "ext-view:views-other:explorer");
    assert_eq!(
        open(&other_view, "views-refuse", "explorer", Value::Null),
        "errors.ext_view.wrong_extension"
    );

    app.state::<FeatureServices>()
        .config
        .lock()
        .unwrap()
        .extension_states
        .insert("views-refuse".into(), false);
    assert_eq!(
        open(&floating, "views-refuse", "explorer", Value::Null),
        "errors.ext_view.disabled"
    );
    assert!(view_list(&app).is_empty(), "nothing opened");
}

#[test]
fn view_commands_from_other_windows_are_refused() {
    let app = mock_app();
    let main = mock_window(&app, "main");
    for (cmd, args) in [
        ("ext_view_boot", json!({})),
        ("ext_view_compose", json!({ "text": "hi" })),
        ("ext_view_copy_text", json!({ "text": "hi" })),
    ] {
        assert_eq!(
            err_key(invoke(&main, cmd, args)),
            "errors.ext_view.not_a_view_window",
            "{cmd}"
        );
    }
    // A window with a view label that Kage didn't open as a view.
    let stray = mock_window(&app, "ext-view:views-stray:explorer");
    assert_eq!(
        err_key(invoke(&stray, "ext_view_boot", json!({}))),
        "errors.ext_view.not_a_view_window"
    );
    // No visible view: the launcher's question is answered at once.
    let floating = mock_window(&app, "floating");
    assert_eq!(
        invoke(&floating, "ext_view_focus_state", json!({})).unwrap(),
        json!({ "view_focused": false })
    );
}

#[test]
fn compose_and_copy_need_focus() {
    let app = mock_app();
    install("views-text");
    grant(&app, "views-text", &["views"]);
    let floating = mock_window(&app, "floating");
    invoke(
        &floating,
        "ext_view_open",
        open_args("views-text", "explorer", Value::Null, "launcher"),
    )
    .unwrap();
    let view = app
        .get_webview_window("ext-view:views-text:explorer")
        .unwrap();
    // The mock window system reports no window as focused.
    for cmd in ["ext_view_compose", "ext_view_copy_text"] {
        assert_eq!(
            err_key(invoke(&view, cmd, json!({ "text": "hello" }))),
            "errors.ext_view.not_focused",
            "{cmd}"
        );
    }
}

#[test]
fn turning_an_extension_off_closes_its_views() {
    let app = mock_app();
    install("views-disable");
    grant(&app, "views-disable", &["storage", "views"]);
    let floating = mock_window(&app, "floating");
    invoke(
        &floating,
        "ext_view_open",
        open_args("views-disable", "explorer", Value::Null, "launcher"),
    )
    .unwrap();
    assert_eq!(view_list(&app).len(), 1);
    let view = app
        .get_webview_window("ext-view:views-disable:explorer")
        .unwrap();

    let settings = app.get_webview_window("settings").unwrap();
    invoke(
        &settings,
        "set_extension_enabled",
        json!({ "id": "views-disable", "enabled": false }),
    )
    .unwrap();
    assert!(view_list(&app).is_empty(), "disable closes the view");
    assert_eq!(
        err_key(invoke(&view, "ext_view_boot", json!({}))),
        "errors.ext_view.not_a_view_window"
    );
}

#[test]
fn losing_the_grant_closes_its_views() {
    let app = mock_app();
    install("views-grant");
    grant(&app, "views-grant", &["storage", "views"]);
    let floating = mock_window(&app, "floating");
    let settings = mock_window(&app, "settings");
    let open = || {
        invoke(
            &floating,
            "ext_view_open",
            open_args("views-grant", "explorer", Value::Null, "launcher"),
        )
    };

    open().unwrap();
    // A commit that keeps `views` leaves the window (the page reloads in
    // place); one without it closes it.
    invoke(
        &settings,
        "commit_extension_install",
        json!({ "extensionId": "views-grant", "granted": ["storage", "views"], "approvedVersion": "1.0.0" }),
    )
    .unwrap();
    assert_eq!(view_list(&app).len(), 1);
    invoke(
        &settings,
        "commit_extension_install",
        json!({ "extensionId": "views-grant", "granted": ["storage"], "approvedVersion": "1.0.0" }),
    )
    .unwrap();
    assert!(
        view_list(&app).is_empty(),
        "a smaller grant closes the view"
    );

    grant(&app, "views-grant", &["storage", "views"]);
    assert!(view_list(&app).is_empty());
    invoke(
        &settings,
        "remove_extension_grant",
        json!({ "extensionId": "views-grant" }),
    )
    .unwrap();
    assert_eq!(
        err_key(open()),
        "errors.ext_view.not_granted",
        "no grant, no window"
    );
}

#[test]
fn uninstall_closes_views_and_forgets_their_geometry() {
    use kage::ext_views::geometry::{self, SavedGeometry, SavedMonitor};

    let app = mock_app();
    install("views-gone");
    grant(&app, "views-gone", &["storage", "views"]);
    let entry = SavedGeometry {
        x: 10,
        y: 20,
        width: 800,
        height: 600,
        maximized: false,
        monitor: SavedMonitor {
            name: "M".into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
        },
        saved_at: "2026-11-09T10:00:00Z".into(),
    };
    for (id, view) in [("views-gone", "explorer"), ("views-kept", "explorer")] {
        geometry::update_entry(id, view, |_| Some(entry.clone())).unwrap();
    }
    let floating = mock_window(&app, "floating");
    invoke(
        &floating,
        "ext_view_open",
        open_args("views-gone", "explorer", Value::Null, "launcher"),
    )
    .unwrap();

    let settings = mock_window(&app, "settings");
    invoke(
        &settings,
        "uninstall_extension",
        json!({ "id": "views-gone", "kind": "extension" }),
    )
    .unwrap();
    assert!(view_list(&app).is_empty());
    assert_eq!(geometry::load_entry("views-gone", "explorer"), None);
    assert_eq!(
        geometry::load_entry("views-kept", "explorer"),
        Some(entry),
        "other extensions keep theirs"
    );
}

#[test]
fn reset_geometry_removes_saved_positions() {
    use kage::ext_views::geometry::{self, SavedGeometry, SavedMonitor};

    let app = mock_app();
    let settings = mock_window(&app, "settings");
    let entry = SavedGeometry {
        x: 0,
        y: 0,
        width: 640,
        height: 480,
        maximized: true,
        monitor: SavedMonitor {
            name: String::new(),
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
            scale: 2.0,
        },
        saved_at: "2026-11-09T11:00:00Z".into(),
    };
    for view in ["a", "b", "c"] {
        geometry::update_entry("views-reset", view, |_| Some(entry.clone())).unwrap();
    }
    let removed = invoke(
        &settings,
        "ext_view_reset_geometry",
        json!({ "extensionId": "views-reset", "viewId": "a" }),
    )
    .unwrap();
    assert_eq!(removed, json!(1));
    let removed = invoke(
        &settings,
        "ext_view_reset_geometry",
        json!({ "extensionId": "views-reset" }),
    )
    .unwrap();
    assert_eq!(removed, json!(2));
    assert_eq!(geometry::load_entry("views-reset", "b"), None);
}

#[test]
fn a_full_backup_never_carries_view_geometry() {
    use kage::config_export::{self, ExportMode};
    use kage::ext_views::geometry::{self, SavedGeometry, SavedMonitor};

    isolate_user_dirs();
    // The geometry file lives in local data, outside everything a backup
    // reads (config.json, steering, extension-data/).
    let geometry_file = geometry::file_path().unwrap();
    let config_root = kage::paths::config_dir().unwrap().join("kage");
    assert!(!geometry_file.starts_with(config_root.join("extension-data")));
    assert!(geometry_file.starts_with(kage::paths::data_local_dir().unwrap()));

    geometry::update_entry("views-backup", "explorer", |_| {
        Some(SavedGeometry {
            x: 1,
            y: 2,
            width: 900,
            height: 700,
            maximized: false,
            monitor: SavedMonitor {
                name: "M".into(),
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                scale: 1.0,
            },
            saved_at: "2026-11-09T12:00:00Z".into(),
        })
    })
    .unwrap();
    let data_dir = config_root.join("extension-data").join("views-backup");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("notes.json"), "\"hello\"").unwrap();

    let config = kage::config::Config::default();
    let bundle = config_export::export(&config, ExportMode::Encrypted, Some("a passphrase"), &[])
        .expect("full backup");
    let prepared =
        config_export::prepare_import(bundle, Some("a passphrase")).expect("bundle reads back");
    let (_config, summary, files) = config_export::apply_import(prepared, &config, &[])
        .expect("bundle applies")
        .into_parts();
    files.rollback();
    // The one extension-data file came back; the geometry file isn't an
    // entry of the bundle at all.
    assert_eq!(summary.extensions, 1);
    let _ = std::fs::remove_dir_all(&data_dir);
}
