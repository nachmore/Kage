//! Tier-2 harness: a real `tauri::App` on the MockRuntime — real
//! `.manage()` / `state()` / IPC dispatch, no display or webview
//! process — so the code that only runs "inside Tauri" gets executed in
//! plain `cargo test` on CI.
//!
//! Layout (deliberate, per review feedback):
//!   - one `#[test]` per area, so a red run names the broken area
//!     directly in the test list;
//!   - the full command sweep is a single batch test that invokes EVERY
//!     registered command and reports ALL offenders at once (same
//!     report-everything convention as the parity tests) rather than
//!     dying on the first failure.
//!
//! What the sweep catches: "state not managed" wiring bugs (the class
//! that shipped in the changelog-cache nightly), command-arg
//! deserialization mismatches that reject before the fn body runs, and
//! panics inside handlers reachable with empty args. It does NOT
//! assert domain behavior — a command returning a domain AppError is a
//! PASS here.

use kage::acp_client::{AcpClient, AcpConnectionMode};
use kage::state::{build_managed_state, ManagedState};
use std::sync::Arc;
use tauri::test::{mock_builder, MockRuntime};
use tauri::Manager;

/// Build a mock app managing EXACTLY the state production manages, via
/// the same constructor `main.rs::run` uses.
fn mock_app() -> tauri::App<MockRuntime> {
    let config = Arc::new(std::sync::Mutex::new(kage::config::Config::default()));
    let acp_client = Arc::new(AcpClient::new(AcpConnectionMode::Local {
        // `true` is the conventional no-op command; the client doesn't
        // spawn anything until connect() is called, which no test here
        // does.
        spawn_command: "true".to_string(),
    }));
    let ManagedState {
        acp_handles,
        ui_state,
        child_processes,
        feature_services,
    } = build_managed_state(config, acp_client, false);

    mock_builder()
        // Same plugins production registers in main.rs, minus the ones
        // that can't run headless (single-instance IPC, aptabase
        // networking, deep-link OS registration). Commands that reach
        // for a plugin's managed state would otherwise report a false
        // "state not managed" — in production these ARE managed.
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_global_shortcut::Builder::default().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(acp_handles)
        .manage(ui_state)
        .manage(child_processes)
        .manage(feature_services)
        .invoke_handler(kage::commands::registry::invoke_handler())
        .build({
            // The updater plugin refuses to initialize without its
            // config section (production gets it from tauri.conf.json).
            let mut ctx = tauri::test::mock_context(tauri::test::noop_assets());
            ctx.config_mut().plugins.0.insert(
                "updater".into(),
                serde_json::json!({ "pubkey": "test", "endpoints": [] }),
            );
            ctx
        })
        .expect("mock app must build")
}

fn mock_webview(app: &tauri::App<MockRuntime>) -> tauri::WebviewWindow<MockRuntime> {
    tauri::WebviewWindowBuilder::new(app, "main", Default::default())
        .build()
        .expect("mock webview must build")
}

// ---------------------------------------------------------------------------
// Area tests — one per concern so failures name their area.
// ---------------------------------------------------------------------------

#[test]
fn managed_state_is_retrievable() {
    let app = mock_app();
    // state::<T>() panics on unmanaged T — each line is the crash-class
    // assertion for one managed type.
    let _ = app.state::<kage::state::AcpHandles>();
    let _ = app.state::<kage::state::UiState>();
    let _ = app.state::<kage::state::ChildProcesses>();
    let _ = app.state::<kage::state::FeatureServices>();
}

#[test]
fn setup_changelog_cache_channel_read_works() {
    // Regression for the shipped state()-before-manage() panic: the
    // channel read that spawn_changelog_cache_refresh performs must
    // work against the production state set. (The spawn itself needs
    // the async runtime; the state access was the crash.)
    let app = mock_app();
    let features = app.state::<kage::state::FeatureServices>();
    let channel = features.config.lock().unwrap().updates.channel;
    let _ = channel.as_str();
}

#[test]
fn update_activation_policy_survives_mock_app() {
    // Runtime-generic setup helper — must not panic without windows.
    let app = mock_app();
    kage::setup::update_activation_policy(app.handle());
}

// ---------------------------------------------------------------------------
// Full command sweep — batch, report-everything.
// ---------------------------------------------------------------------------

/// Wire names of every registered command, parsed from registry.rs at
/// test time so this sweep can never drift from what production
/// registers — a command added to the registry is swept automatically.
/// (Tauri's wire name is the final path segment of the handler path.)
fn all_commands() -> Vec<String> {
    let registry =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/commands/registry.rs");
    let src = std::fs::read_to_string(&registry).expect("read registry.rs");
    let mut names: Vec<String> = src
        .lines()
        .filter_map(|line| {
            let t = line.trim();
            let path = t.strip_prefix("crate::")?.strip_suffix(',')?;
            Some(path.rsplit("::").next().unwrap_or(path).to_string())
        })
        .collect();
    names.sort();
    names.dedup();
    assert!(
        names.len() > 150,
        "registry parse looks broken — only {} commands found",
        names.len()
    );
    names
}

/// Commands the sweep must NOT dispatch. MockRuntime isolates TAURI
/// (no real windows/webviews), not the OS — a command that talks to
/// the OS directly still does it for real. Two exclusion classes:
///   1. process-level side effects that kill/wedge the test run;
///   2. OS UI that blocks on a human (native dialogs, key capture) —
///      pick_folder genuinely opened a folder picker on a dev machine
///      mid-sweep.
///
/// Everything here still gets registry/state coverage from the area
/// tests; what it loses is only the empty-args dispatch probe. Keep
/// this justified and short — every entry is a hole in the sweep.
const SWEEP_DENYLIST: &[(&str, &str)] = &[
    // -- class 1: process control --
    ("quit_app", "calls std::process::exit"),
    ("restart_app", "respawns + std::process::exit"),
    ("download_and_install_update", "runs the OS installer"),
    // -- class 2: blocking OS UI --
    ("pick_folder", "opens a native folder picker (rfd)"),
    ("capture_hotkey_combo", "blocks on real key capture"),
    (
        "check_for_update",
        "hits the real update endpoint over the network",
    ),
];

/// Commands whose empty-args invoke is expected to be rejected by arg
/// deserialization (they take required args). That's a PASS — the
/// dispatch worked and the rejection is the correct wire behavior.
/// What this sweep is hunting is the OTHER failure text: "state not
/// managed", or a handler panic.
fn is_acceptable_failure(err: &serde_json::Value) -> Option<String> {
    let text = err.to_string();
    if text.contains("state not managed") {
        return Some(format!("STATE NOT MANAGED: {text}"));
    }
    None
}

#[test]
fn command_sweep_no_wiring_failures() {
    let app = mock_app();
    let webview = mock_webview(&app);
    let commands = all_commands();

    let mut offenses: Vec<String> = Vec::new();
    for cmd in &commands {
        if SWEEP_DENYLIST.iter().any(|(name, _)| name == cmd) {
            continue;
        }
        // Progress marker: with --nocapture this identifies a hanging
        // command instantly (the last printed name is the culprit).
        eprintln!("[sweep] {cmd}");
        // Panics inside a handler unwind through on_message; catch them
        // so one bad command doesn't hide the rest of the sweep.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tauri::test::get_ipc_response(
                &webview,
                tauri::webview::InvokeRequest {
                    cmd: cmd.clone(),
                    callback: tauri::ipc::CallbackFn(0),
                    error: tauri::ipc::CallbackFn(1),
                    url: "http://tauri.localhost".parse().unwrap(),
                    body: tauri::ipc::InvokeBody::default(),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.to_string(),
                },
            )
        }));
        match outcome {
            Err(panic) => {
                let msg = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "<non-string panic>".into());
                // An async handler that panics does so on a tokio worker;
                // here that surfaces as a RecvError (the response channel
                // died). The real panic message is in this test's stderr
                // just above — point the reader at it.
                let msg = if msg.contains("RecvError") {
                    format!("handler panicked on a worker thread (see stderr above for the real panic, e.g. 'state not managed'): {msg}")
                } else {
                    msg
                };
                offenses.push(format!("  {cmd}: PANICKED: {msg}"));
            }
            Ok(Err(err)) => {
                if let Some(reason) = is_acceptable_failure(&err) {
                    offenses.push(format!("  {cmd}: {reason}"));
                }
            }
            Ok(Ok(_)) => {}
        }
    }

    if !offenses.is_empty() {
        panic!(
            "Command sweep found wiring failures ({} of {} commands).\n\
             These commands panicked or requested unmanaged state when\n\
             invoked through real IPC dispatch:\n\n{}",
            offenses.len(),
            commands.len(),
            offenses.join("\n")
        );
    }
}
