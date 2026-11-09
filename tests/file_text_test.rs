//! The file text host with the real `kage-file-text` worker, and its
//! three commands through Tauri IPC (docs/design/EXTENSION_FILE_TEXT.md
//! §12).
//!
//! `build.rs` builds the worker and `tauri-build` copies it next to the
//! test binaries' parent (`target/<profile>/`), where these tests find it.
//! The hang, abort and flood modes are debug-only worker flags.
//!
//! Every test isolates the per-user folders first: the `file_ref` key is
//! written under the local data folder.

use kage::acp_client::{AcpClient, AcpConnectionMode};
use kage::file_text::dialog::{DialogRequest, DialogSource};
use kage::file_text::post::{Events, Phase, Progress, Report};
use kage::file_text::request::Env;
use kage::file_text::slot::Slot;
use kage::file_text::worker::{run_worker, Extractor, Worker, WorkerFailure};
use kage::file_text::{run, FileText, Request};
use kage_core::file_text_wire::{
    Format, WorkerRequest, DEFAULT_MAX_CHARS, WIRE_VERSION, WORKER_BIN_NAME, WORKER_DEADLINE,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Manager;

fn isolate_user_dirs() {
    let root = std::env::temp_dir().join(format!("kage-file-text-test-{}", std::process::id()));
    kage::paths::isolate(&root);
    assert!(
        kage::paths::is_isolated(),
        "refusing to run against the real per-user directories"
    );
}

/// `target/<profile>/kage-file-text`, next to `deps/` where this test runs.
fn worker_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().and_then(Path::parent).unwrap();
    let path = profile_dir.join(format!("{WORKER_BIN_NAME}{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.is_file(),
        "the worker isn't at {}: build.rs stages it on every build of the app crate",
        path.display()
    );
    path
}

struct Fixed(PathBuf);

impl DialogSource for Fixed {
    fn pick(&self, _: &DialogRequest) -> Result<Option<PathBuf>, kage::error::AppError> {
        Ok(Some(self.0.clone()))
    }
}

#[derive(Default)]
struct Recorder {
    phases: Mutex<Vec<Phase>>,
    reports: Mutex<Vec<Report>>,
}

impl Events for Recorder {
    fn progress(&self, progress: &Progress) {
        self.phases.lock().unwrap().push(progress.phase);
    }
    fn finished(&self, report: &Report) {
        self.reports.lock().unwrap().push(report.clone());
    }
    fn still_allowed(&self) -> bool {
        true
    }
}

fn request(formats: &[Format]) -> Request {
    Request {
        extension_id: "mindmap".into(),
        display_name: "Mind Map".into(),
        formats: formats.to_vec(),
        max_chars: DEFAULT_MAX_CHARS,
    }
}

/// One request for `name` holding `bytes`, read by the real worker.
fn read_file(name: &str, bytes: &[u8], formats: &[Format]) -> Result<FileText, String> {
    isolate_user_dirs();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    let slot = Arc::new(Slot::default());
    let guard = slot.take("mindmap", Instant::now()).unwrap();
    let worker = Worker::at(worker_path());
    let events = Recorder::default();
    let env = Env {
        dialog: &Fixed(path),
        extractor: &worker,
        events: &events,
    };
    let got = run(&env, guard, &request(formats)).map_err(|e| e.key.clone());
    assert_eq!(events.reports.lock().unwrap().len(), 1);
    assert_eq!(events.phases.lock().unwrap().last(), Some(&Phase::Done));
    got.map(|t| t.expect("a picked file is read"))
}

#[test]
fn the_worker_reads_text_and_markdown() {
    let bytes = "Hello, w\u{f6}rld  \r\nline two\r\n\r\n\r\n\r\nend".as_bytes();
    let got = read_file("Notes.txt", bytes, &[Format::Text, Format::Markdown]).unwrap();
    assert_eq!(got.format, Format::Text);
    assert_eq!(got.text, "Hello, w\u{f6}rld\nline two\n\nend");
    assert_eq!(got.chars, got.text.chars().count() as u32);
    assert_eq!(got.name, "Notes.txt");
    assert_eq!(got.size, bytes.len() as u64);
    assert_eq!(got.sha256, kage::file_text::file_ref::sha256_hex(bytes));
    assert!(got.file_ref.starts_with("fr1_"));
    assert!(!got.truncated);

    let md = b"# Quarterly plan\n\n- ship it\n";
    let got = read_file("plan.md", md, &[Format::Markdown]).unwrap();
    assert_eq!(got.format, Format::Markdown);
    assert_eq!(got.title.as_deref(), Some("Quarterly plan"));
    assert!(got.text.contains("- ship it"));
}

#[test]
fn the_worker_refuses_what_isnt_what_its_name_says() {
    let binary: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    assert_eq!(
        read_file("data.txt", &binary, &[Format::Text]).unwrap_err(),
        "errors.file_text.not_text"
    );
    assert_eq!(
        read_file("report.pdf", b"just some words", &[Format::Pdf]).unwrap_err(),
        "errors.file_text.wrong_content"
    );
    let ole = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0, 0, 0];
    assert_eq!(
        read_file("old.docx", &ole, &[Format::Docx]).unwrap_err(),
        "errors.file_text.legacy_format"
    );
}

fn small_request() -> WorkerRequest {
    WorkerRequest::new(vec![Format::Text], "txt", 1_000)
}

#[test]
fn a_hanging_worker_is_killed_at_the_deadline() {
    isolate_user_dirs();
    let started = Instant::now();
    let got = run_worker(
        &worker_path(),
        "--v1-test-hang",
        &small_request(),
        b"hello",
        Duration::from_millis(800),
        &AtomicBool::new(false),
    );
    assert_eq!(got, Err(WorkerFailure::Timeout));
    assert!(started.elapsed() < WORKER_DEADLINE);
}

#[test]
fn a_cancel_kills_the_worker() {
    isolate_user_dirs();
    let cancel = Arc::new(AtomicBool::new(false));
    let setter = cancel.clone();
    let timer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        setter.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let started = Instant::now();
    let got = run_worker(
        &worker_path(),
        "--v1-test-hang",
        &small_request(),
        b"hello",
        Duration::from_secs(20),
        &cancel,
    );
    timer.join().unwrap();
    assert_eq!(got, Err(WorkerFailure::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_crashing_worker_is_too_complex_and_a_flooding_one_worker_failed() {
    isolate_user_dirs();
    let never = AtomicBool::new(false);
    let abort = run_worker(
        &worker_path(),
        "--v1-test-abort",
        &small_request(),
        b"hello",
        Duration::from_secs(20),
        &never,
    );
    let failure = match abort {
        Err(f @ WorkerFailure::Crashed(_)) => f,
        other => panic!("an aborting worker must crash: {other:?}"),
    };
    assert_eq!(
        failure.to_error("a.txt", "txt", &[Format::Text]).key,
        "errors.file_text.too_complex"
    );

    let started = Instant::now();
    let flood = run_worker(
        &worker_path(),
        "--v1-test-flood",
        &small_request(),
        b"hello",
        Duration::from_secs(20),
        &never,
    );
    let failure = match flood {
        Err(f @ WorkerFailure::Failed(_)) => f,
        other => panic!("a flooding worker must fail: {other:?}"),
    };
    assert_eq!(
        failure.to_error("a.txt", "txt", &[Format::Text]).key,
        "errors.file_text.worker_failed"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "killed at the cap, not the deadline"
    );
}

#[test]
fn an_unknown_wire_version_is_worker_failed() {
    isolate_user_dirs();
    let mut req = small_request();
    req.v = WIRE_VERSION + 1;
    let got = Worker::at(worker_path()).extract(&req, b"hello", &AtomicBool::new(false));
    assert!(matches!(got, Err(WorkerFailure::Failed(_))), "{got:?}");
}

// --- Through Tauri IPC ---------------------------------------------------------------------

fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
    isolate_user_dirs();
    kage::os::login_env::set_enabled(false);
    let config = Arc::new(std::sync::Mutex::new(kage::config::Config::default()));
    let acp_client = Arc::new(AcpClient::new(AcpConnectionMode::Local {
        spawn_command: "true".to_string(),
    }));
    let state = kage::state::build_managed_state(config, acp_client, false);
    tauri::test::mock_builder()
        .plugin(tauri_plugin_dialog::init())
        .manage(state.acp_handles)
        .manage(state.ui_state)
        .manage(state.child_processes)
        .manage(state.feature_services)
        .invoke_handler(kage::commands::registry::invoke_handler())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app must build")
}

fn invoke(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, serde_json::Value> {
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
        other => panic!("unexpected response body: {other:?}"),
    })
}

fn key_of(err: &serde_json::Value) -> String {
    err.get("key")
        .and_then(|k| k.as_str())
        .unwrap_or_default()
        .to_string()
}

#[test]
fn the_commands_refuse_before_any_dialog() {
    let app = mock_app();
    let webview = tauri::WebviewWindowBuilder::new(&app, "settings", Default::default())
        .build()
        .unwrap();
    let ids = serde_json::json!({ "extensionId": "mindmap", "displayName": "Mind Map" });

    // Empty args: refused on the missing extensionId, before the body runs.
    assert!(invoke(&webview, "pick_file_text", serde_json::json!({})).is_err());
    // No grant.
    let err = invoke(&webview, "pick_file_text", ids.clone()).unwrap_err();
    assert_eq!(key_of(&err), "errors.file_text.not_granted", "{err}");
    assert_eq!(err["kind"], "not_permitted");

    let features = app.state::<kage::state::FeatureServices>();
    {
        let mut config = features.config.lock().unwrap();
        config.extension_grants.insert(
            "mindmap".into(),
            kage::config::ExtensionGrant {
                granted: vec!["storage".into(), "file_text".into()],
                ..Default::default()
            },
        );
        config.extension_states.insert("mindmap".into(), false);
    }
    let err = invoke(&webview, "pick_file_text", ids.clone()).unwrap_err();
    assert_eq!(key_of(&err), "errors.file_text.extension_disabled");

    features
        .config
        .lock()
        .unwrap()
        .extension_states
        .insert("mindmap".into(), true);
    let mut bad = ids.clone();
    bad["args"] = serde_json::json!({ "max_chars": 0 });
    let err = invoke(&webview, "pick_file_text", bad).unwrap_err();
    assert_eq!(key_of(&err), "errors.file_text.invalid_max_chars");
    assert_eq!(err["kind"], "invalid_input");

    // Granted and valid: the worker isn't next to this test binary, so the
    // request stops at `unavailable`, still before any dialog.
    let mut ok = ids.clone();
    ok["args"] = serde_json::json!({ "formats": ["pdf", "markdown"], "max_chars": 5000 });
    let err = invoke(&webview, "pick_file_text", ok).unwrap_err();
    assert_eq!(key_of(&err), "errors.file_text.unavailable");

    let cancelled = invoke(&webview, "cancel_file_text", ids).unwrap();
    assert_eq!(cancelled, serde_json::json!(false));
    assert!(invoke(&webview, "cancel_file_text", serde_json::json!({})).is_err());

    let support = invoke(&webview, "file_text_support", serde_json::json!({})).unwrap();
    assert_eq!(support["max_input_bytes"], 33_554_432);
    assert_eq!(support["default_max_chars"], 100_000);
    assert_eq!(support["formats"][0]["id"], "text");
}
