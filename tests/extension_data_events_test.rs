//! Storage change events end to end (docs/design/EXTENSION_VIEWS.md §10):
//! a mock app with real IPC dispatch, a few windows, and
//! `save_extension_data` / `delete_extension_data` called from one of them.
//! The extension hosts get one `extension_data_changed` batch; other
//! windows get nothing.
//!
//! The coalescer is one per process, so each test uses its own extension id.

use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::test::{mock_builder, MockRuntime};
use tauri::Listener;

/// Point `kage::paths` at a per-process scratch dir before anything writes.
fn isolate_user_dirs() {
    let root = std::env::temp_dir().join(format!(
        "kage-extension-data-events-test-{}",
        std::process::id()
    ));
    kage::paths::isolate(&root);
    assert!(
        kage::paths::is_isolated(),
        "refusing to run against the real per-user directories"
    );
}

fn mock_app() -> tauri::App<MockRuntime> {
    isolate_user_dirs();
    mock_builder()
        .invoke_handler(kage::commands::registry::invoke_handler())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app must build")
}

fn window(app: &tauri::App<MockRuntime>, label: &str) -> tauri::WebviewWindow<MockRuntime> {
    tauri::WebviewWindowBuilder::new(app, label, Default::default())
        .build()
        .unwrap_or_else(|e| panic!("mock webview '{label}' must build: {e}"))
}

fn invoke(
    webview: &tauri::WebviewWindow<MockRuntime>,
    cmd: &str,
    args: serde_json::Value,
) -> Result<tauri::ipc::InvokeResponseBody, serde_json::Value> {
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
}

/// Every `extension_data_changed` payload `window` receives, with when it
/// arrived.
fn batches(
    window: &tauri::WebviewWindow<MockRuntime>,
) -> mpsc::Receiver<(Instant, serde_json::Value)> {
    let (tx, rx) = mpsc::channel();
    window.listen(kage::events::EXTENSION_DATA_CHANGED, move |event| {
        let payload = serde_json::from_str(event.payload()).expect("JSON payload");
        let _ = tx.send((Instant::now(), payload));
    });
    rx
}

fn data_file(extension_id: &str, key: &str) -> std::path::PathBuf {
    kage::paths::config_dir()
        .expect("config dir")
        .join("kage")
        .join("extension-data")
        .join(extension_id)
        .join(format!("{key}.json"))
}

#[test]
fn a_save_in_one_window_reaches_every_extension_host_as_one_batch() {
    let app = mock_app();
    let floating = window(&app, "floating");
    let settings = window(&app, "settings");
    let view = window(&app, "ext-view:events-e2e:board");
    let store = window(&app, "store");
    let to_settings = batches(&settings);
    let to_view = batches(&view);
    let to_store = batches(&store);
    let ext = "events-e2e";

    let started = Instant::now();
    for (cmd, args) in [
        (
            "save_extension_data",
            serde_json::json!({ "extensionId": ext, "key": "a", "data": "{\"n\":1}" }),
        ),
        (
            "save_extension_data",
            serde_json::json!({ "extensionId": ext, "key": "b", "data": "{\"n\":2}" }),
        ),
        (
            "delete_extension_data",
            serde_json::json!({ "extensionId": ext, "key": "a" }),
        ),
    ] {
        invoke(&floating, cmd, args).unwrap_or_else(|e| panic!("{cmd}: {e}"));
    }

    let expected = serde_json::json!({
        "extension_id": ext,
        "changes": [
            { "key": "b", "op": "save", "source": "window", "window": "floating" },
            { "key": "a", "op": "delete", "source": "window", "window": "floating" },
        ]
    });
    for rx in [&to_settings, &to_view] {
        let (at, payload) = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("a batch reaches every extension host");
        assert_eq!(payload, expected);
        let latency = at.duration_since(started);
        eprintln!("extension_data_changed arrived after {latency:?}");
        // The coalescer waited for the rest of the writes, and no longer
        // than it should (the design asks for 150 ms on a dev build; CI
        // machines get some slack).
        assert!(
            latency >= kage::extension_data_events::COALESCE_WINDOW,
            "{latency:?}"
        );
        assert!(latency < Duration::from_secs(1), "{latency:?}");
    }
    // The data is readable once the event is out.
    assert_eq!(
        std::fs::read_to_string(data_file(ext, "b")).unwrap(),
        "{\"n\":2}"
    );
    assert!(!data_file(ext, "a").exists());

    // One batch, and nothing for a window that runs no extensions.
    std::thread::sleep(Duration::from_millis(300));
    assert!(to_settings.try_recv().is_err(), "a second batch");
    assert!(to_view.try_recv().is_err(), "a second batch");
    assert!(to_store.try_recv().is_err(), "the store got a batch");
}

#[test]
fn a_refused_write_or_a_delete_of_nothing_tells_no_one() {
    let app = mock_app();
    let floating = window(&app, "floating");
    let main = window(&app, "main");
    let to_main = batches(&main);
    let ext = "events-e2e-quiet";

    assert!(invoke(
        &floating,
        "save_extension_data",
        serde_json::json!({ "extensionId": ext, "key": "../escape", "data": "{}" }),
    )
    .is_err());
    invoke(
        &floating,
        "delete_extension_data",
        serde_json::json!({ "extensionId": ext, "key": "never-saved" }),
    )
    .expect("deleting a missing key succeeds");

    std::thread::sleep(kage::extension_data_events::COALESCE_WINDOW * 4);
    assert!(to_main.try_recv().is_err(), "an event for no change");
}

#[test]
fn the_writing_window_is_named_so_it_can_tell_its_own_writes() {
    let app = mock_app();
    let view = window(&app, "ext-view:events-e2e-self:board");
    let main = window(&app, "main");
    let to_main = batches(&main);
    let to_view = batches(&view);
    let ext = "events-e2e-self";

    invoke(
        &view,
        "save_extension_data",
        serde_json::json!({ "extensionId": ext, "key": "mm-view", "data": "{}" }),
    )
    .expect("save");

    for rx in [&to_main, &to_view] {
        let (_, payload) = rx.recv_timeout(Duration::from_secs(2)).expect("a batch");
        assert_eq!(
            payload["changes"][0]["window"],
            "ext-view:events-e2e-self:board"
        );
    }
}
