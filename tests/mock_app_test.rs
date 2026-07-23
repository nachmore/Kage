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
        // that can't run headless: single-instance IPC, aptabase
        // networking, deep-link OS registration, and global-shortcut —
        // whose plugin INIT registers a real OS event hook and fails on
        // macOS CI runners ("File exists" / "Operation timed out").
        // Commands that reach for a skipped plugin's managed state
        // report "state not managed"; the sweep allowlists exactly
        // those plugin state types (see is_acceptable_failure) so real
        // app-state wiring bugs still fail.
        .plugin(tauri_plugin_shell::init())
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
    mock_window(app, "main")
}

fn mock_window(app: &tauri::App<MockRuntime>, label: &str) -> tauri::WebviewWindow<MockRuntime> {
    tauri::WebviewWindowBuilder::new(app, label, Default::default())
        .build()
        .unwrap_or_else(|e| panic!("mock webview '{label}' must build: {e}"))
}

/// Invoke a registered command through real IPC dispatch with a JSON
/// args payload. Returns Ok(response body) or Err(error payload).
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

/// Poll `predicate` for up to `ms` milliseconds. Listener handlers and
/// spawned tasks run on the async runtime, so effects are eventually
/// visible rather than synchronous.
fn wait_for(ms: u64, predicate: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while std::time::Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    predicate()
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

#[test]
fn automation_scheduler_wires_signal_sender_synchronously() {
    // FeatureServices.automation_signal_tx starts as None and is
    // populated by spawn_automation_scheduler. If that wiring regresses,
    // extensions' emit_automation_signal becomes a silent no-op — no
    // crash, no log, just dead automations. The sender must be stashed
    // synchronously (not behind the spawned loop) so there's no startup
    // race window either.
    let app = mock_app();
    kage::setup::spawn_automation_scheduler(&app);
    let features = app.state::<kage::state::FeatureServices>();
    assert!(
        features.automation_signal_tx.lock().unwrap().is_some(),
        "automation signal sender not wired after spawn_automation_scheduler"
    );
}

#[test]
fn hotkey_hot_reload_skips_reregistration_when_unchanged() {
    // This listener shipped a real bug (a try_lock variant silently
    // dropped the user's hotkey change). The snapshot-gating half is
    // testable headless: an unchanged config must early-return without
    // touching hotkey state. The re-registration half needs the
    // global-shortcut plugin (real OS hooks — can't init on MockRuntime,
    // see the plugin note in mock_app()), so a sentinel that survives
    // proves the gate; the change path stays smoke-test territory.
    use tauri::Emitter;
    let app = mock_app();
    let initial_config = kage::config::Config::default();
    kage::setup::install_hotkey_hot_reload(&app, &initial_config);

    let ui = app.state::<kage::state::UiState>();
    let sentinel = ("slot".to_string(), "hotkey".to_string());
    ui.hotkey_registration_failures
        .lock()
        .unwrap()
        .push(sentinel.clone());

    // Unchanged config → snapshot matches → listener early-returns and
    // must NOT touch the failures vector (register_all_hotkeys would
    // overwrite it).
    app.emit(kage::events::CONFIG_UPDATED, ()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        ui.hotkey_registration_failures
            .lock()
            .unwrap()
            .contains(&sentinel),
        "listener re-registered hotkeys on an unchanged config (snapshot gating broken)"
    );
}

#[test]
fn show_sessions_listener_reads_state_without_panicking() {
    // The single-instance "second launch" path: SHOW_SESSIONS must route
    // to the most recently focused chat window when it still exists.
    // With a stale label (window gone) the listener falls back to MAIN —
    // that fallback calls open_chat_window, which needs webview creation
    // we can't complete headless, so here we assert the live-label path.
    use tauri::Emitter;
    let app = mock_app();
    let _chat = mock_window(&app, "chat-11111111-2222-3333-4444-555555555555");
    kage::setup::install_show_sessions_listener(&app);

    {
        let ui = app.state::<kage::state::UiState>();
        *ui.last_focused_chat.lock().unwrap() =
            Some("chat-11111111-2222-3333-4444-555555555555".to_string());
    }
    app.emit(kage::events::SHOW_SESSIONS, ()).unwrap();
    // The handler runs on the async runtime; give it time to execute its
    // state reads + window lookup. Success == no panic (a state-wiring
    // regression in the handler aborts the test process).
    std::thread::sleep(std::time::Duration::from_millis(500));
}

// ---------------------------------------------------------------------------
// Real-args IPC tests — wire contracts and state mutation through the
// same dispatch path production uses.
// ---------------------------------------------------------------------------

#[test]
fn window_session_ipc_roundtrip() {
    // Locks the camelCase wire contract (label / sessionId) and the
    // UiState mutation. A Rust param rename silently breaks every JS
    // caller — this is the test that turns that into a red build.
    let app = mock_app();
    let webview = mock_webview(&app);
    let chat_label = "chat-11111111-2222-3333-4444-555555555555";
    let _chat = mock_window(&app, chat_label);

    invoke(
        &webview,
        "set_window_session",
        serde_json::json!({ "label": chat_label, "sessionId": "session-1" }),
    )
    .expect("set_window_session must succeed");

    {
        let ui = app.state::<kage::state::UiState>();
        assert_eq!(
            ui.window_sessions.lock().unwrap().get(chat_label),
            Some(&"session-1".to_string()),
            "session not recorded under the window label"
        );
    }

    let body = invoke(&webview, "list_chat_windows", serde_json::json!({}))
        .expect("list_chat_windows must succeed");
    let list: serde_json::Value = match body {
        tauri::ipc::InvokeResponseBody::Json(s) => serde_json::from_str(&s).unwrap(),
        other => panic!("unexpected response body: {other:?}"),
    };
    let windows = list.as_array().expect("list_chat_windows returns an array");
    assert_eq!(
        windows[0]["label"], "main",
        "main window must sort first for the tray submenu"
    );
    let chat_entry = windows
        .iter()
        .find(|w| w["label"] == chat_label)
        .expect("chat window missing from list");
    assert_eq!(chat_entry["session_id"], "session-1");

    invoke(
        &webview,
        "clear_window_session",
        serde_json::json!({ "label": chat_label }),
    )
    .expect("clear_window_session must succeed");
    let ui = app.state::<kage::state::UiState>();
    assert!(
        !ui.window_sessions.lock().unwrap().contains_key(chat_label),
        "session still pinned after clear_window_session"
    );
}

#[test]
fn close_chat_window_refuses_privileged_and_non_chat_labels() {
    // Guardrails: closing main/floating would kill the tray-persist
    // model; closing arbitrary labels (settings, store) is a routing
    // bug. Both must come back as typed AppError payloads the frontend
    // can pattern-match, not silent successes.
    let app = mock_app();
    let webview = mock_webview(&app);

    for (label, expected_key) in [
        ("main", "errors.window.privileged_close_refused"),
        ("floating", "errors.window.privileged_close_refused"),
        ("settings", "errors.window.non_chat_close_refused"),
    ] {
        let err = invoke(
            &webview,
            "close_chat_window",
            serde_json::json!({ "label": label }),
        )
        .expect_err(&format!("closing '{label}' must be refused"));
        // AppError serializes as { kind, key, message }. Assert on the
        // stable i18n key — the rendered message needs the catalog,
        // which isn't initialized in the test process.
        assert_eq!(
            err.get("kind").and_then(|v| v.as_str()),
            Some("internal"),
            "'{label}' refusal is not a typed AppError: {err}"
        );
        assert_eq!(
            err.get("key").and_then(|v| v.as_str()),
            Some(expected_key),
            "'{label}' refused with the wrong error key: {err}"
        );
    }

    // A real chat window closes cleanly and scrubs its state.
    let chat_label = "chat-99999999-8888-7777-6666-555555555555";
    let _chat = mock_window(&app, chat_label);
    {
        let ui = app.state::<kage::state::UiState>();
        ui.window_sessions
            .lock()
            .unwrap()
            .insert(chat_label.to_string(), "sess".into());
        ui.pending_prompt_originators
            .lock()
            .unwrap()
            .insert("sess".to_string(), chat_label.to_string());
    }
    invoke(
        &webview,
        "close_chat_window",
        serde_json::json!({ "label": chat_label }),
    )
    .expect("closing a real chat window must succeed");
    let ui = app.state::<kage::state::UiState>();
    assert!(!ui.window_sessions.lock().unwrap().contains_key(chat_label));
    assert!(
        !ui.pending_prompt_originators
            .lock()
            .unwrap()
            .values()
            .any(|owner| owner == chat_label),
        "pending prompt originator not scrubbed on close"
    );
}

#[test]
fn pending_permission_state_survives_failed_dismissal() {
    // The code documents this exact hazard: clearing local pending state
    // when the agent didn't ack the dismissal desyncs and stalls the
    // next prompt on the agent's "prompt already in progress" guard.
    // The harness's AcpClient is never connected, so the send fails —
    // precisely the branch that must NOT remove entries.
    let app = mock_app();
    let webview = mock_webview(&app);

    {
        let acp = app.state::<kage::state::AcpHandles>();
        let mut pending = acp.pending_permissions.lock().unwrap();
        pending.insert(
            kage::state::permission_key(&serde_json::json!(42)),
            kage::state::PendingPermission {
                request_id: serde_json::json!(42),
                session_id: Some("s1".into()),
            },
        );
        pending.insert(
            kage::state::permission_key(&serde_json::json!("req-str")),
            kage::state::PendingPermission {
                request_id: serde_json::json!("req-str"),
                session_id: Some("s2".into()),
            },
        );
    }

    // has_pending_permission: number id found, unknown id not.
    let body = invoke(
        &webview,
        "has_pending_permission",
        serde_json::json!({ "requestId": 42 }),
    )
    .expect("has_pending_permission must succeed");
    assert!(matches!(body, tauri::ipc::InvokeResponseBody::Json(ref s) if s == "true"));
    let body = invoke(
        &webview,
        "has_pending_permission",
        serde_json::json!({ "requestId": 999 }),
    )
    .expect("has_pending_permission must succeed");
    assert!(matches!(body, tauri::ipc::InvokeResponseBody::Json(ref s) if s == "false"));

    // Dismissal targets s1, transport send fails (client disconnected) —
    // the command must error AND both entries must remain.
    invoke(
        &webview,
        "dismiss_pending_permission",
        serde_json::json!({ "sessionId": "s1" }),
    )
    .expect_err("dismissal must fail when the transport send fails");
    let acp = app.state::<kage::state::AcpHandles>();
    assert_eq!(
        acp.pending_permissions.lock().unwrap().len(),
        2,
        "local pending state was cleared despite the agent never acking — desync hazard"
    );
}

#[test]
fn deep_link_parse_gates_unsafe_install_ids() {
    // Security-adjacent boundary: kage://install/<id> arrives from the
    // OS (any process can open a URL). The parse must accept the two
    // legitimate URL shapes and reject traversal/injection junk before
    // anything reaches the store window.
    use kage::setup::{parse_deep_link, DeepLinkIntent};

    let parse = |s: &str| parse_deep_link(&url::Url::parse(s).unwrap());

    // Accepted shapes — host-parsed and opaque-path variants.
    assert_eq!(
        parse("kage://install/spotify"),
        Some(DeepLinkIntent::Install("spotify".into()))
    );
    assert_eq!(
        parse("kage:install/link-preview"),
        Some(DeepLinkIntent::Install("link-preview".into()))
    );

    // Rejected: traversal, encoding tricks, wrong scheme/verb, empty.
    for bad in [
        "kage://install/../../../etc/passwd",
        "kage://install/%2e%2e",
        "kage://install/UPPER CASE",
        "kage://install/",
        "kage://uninstall/spotify",
        "https://install/spotify",
        "kage://install/-leading-dash",
    ] {
        assert_eq!(parse(bad), None, "must reject: {bad}");
    }
}

#[test]
fn welcome_window_shown_only_on_first_run() {
    // First-run gating regression = every user sees the welcome screen
    // on every launch, or new users never see the consent step (which
    // gates telemetry). Window creation works headless on MockRuntime.
    let app = mock_app();

    // Completed first run → no welcome window, even after the helper's
    // internal 500ms delay.
    kage::setup::maybe_show_welcome_window(app.handle(), true);
    std::thread::sleep(std::time::Duration::from_millis(900));
    assert!(
        app.get_webview_window("welcome").is_none(),
        "welcome window must not appear when first_run_completed = true"
    );

    // Fresh install → welcome window appears.
    kage::setup::maybe_show_welcome_window(app.handle(), false);
    let shown = wait_for(3_000, || app.get_webview_window("welcome").is_some());
    assert!(
        shown,
        "welcome window must appear when first_run_completed = false"
    );
}

#[test]
fn emit_audience_filters_respect_window_labels() {
    // event_targets exists because WebviewWindow::emit LOOKS targeted
    // but broadcasts (documented past bug: MESSAGE_COMPLETE not reaching
    // peer windows caused a user-visible hang). Assert each audience
    // helper reaches exactly its intended labels.
    use std::sync::Mutex;
    use tauri::Listener;

    let app = mock_app();
    let fired: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    for label in [
        "main",
        "chat-11111111-2222-3333-4444-555555555555",
        "floating",
        "inline-assist",
    ] {
        let w = mock_window(&app, label);
        let fired = fired.clone();
        let label = label.to_string();
        w.listen("probe", move |_| fired.lock().unwrap().push(label.clone()));
    }

    kage::event_targets::emit_streaming_audience(app.handle(), "probe", &serde_json::json!({}));
    let ok = wait_for(2_000, || fired.lock().unwrap().len() >= 3);
    let mut got = fired.lock().unwrap().clone();
    got.sort();
    assert!(ok, "streaming audience events not delivered: {got:?}");
    assert_eq!(
        got,
        vec![
            "chat-11111111-2222-3333-4444-555555555555".to_string(),
            "floating".to_string(),
            "main".to_string(),
        ],
        "streaming audience must reach main + chat hosts + floating and NOT inline-assist"
    );

    fired.lock().unwrap().clear();
    kage::event_targets::emit_to_floating(app.handle(), "probe", &serde_json::json!({}));
    wait_for(1_000, || !fired.lock().unwrap().is_empty());
    assert_eq!(
        *fired.lock().unwrap(),
        vec!["floating".to_string()],
        "emit_to_floating must reach only the floating window"
    );
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
    // -- class 2: blocking OS UI / OS services --
    ("pick_folder", "opens a native folder picker (rfd)"),
    ("capture_hotkey_combo", "blocks on real key capture"),
    (
        "check_for_update",
        "hits the real update endpoint over the network",
    ),
    // hours arg is Option → empty args deserialize and the command
    // REALLY queries the WinRT appointment store, which blocks forever
    // in a headless CI server session (wedged the Windows runner ~40min).
    (
        "get_calendar_events",
        "queries the real OS calendar (WinRT broker hangs headless)",
    ),
    (
        "get_user_info",
        "spawns OS subprocesses for avatar/name lookup",
    ),
    // No required args → really spawns the pocket-tts Python server
    // and waits on its startup (hung 20s+ on the macOS runner; would
    // leave a stray python process behind anywhere it succeeded).
    ("pocket_tts_start", "spawns a real Python TTS server"),
    (
        "pocket_tts_check_install",
        "spawns python subprocess probes (seconds, not millis)",
    ),
    (
        "dump_thread_info",
        "walks every OS thread via ToolHelp/Wdk (seconds, not millis)",
    ),
    // No required args → the real body synthesizes a Cmd/Ctrl+C
    // keystroke (selection capture) and reads the OS clipboard —
    // blocks on headless macOS runners.
    (
        "show_inline_assist",
        "synthesizes real copy keystrokes + clipboard reads",
    ),
    // poll_interval is Option → empty args deserialize, and the real
    // body opens the activity sqlite DB in the USER's config dir and
    // spawns a persistent foreground-window poller — hung 5s+ on the
    // headless Windows runner, and would leave a live poller (plus a
    // real activity.db write) anywhere it succeeded.
    (
        "start_activity_tracker",
        "opens the real activity DB + spawns an OS foreground-window poller",
    ),
];

/// Commands that get a longer watchdog than [`COMMAND_TIMEOUT`], with a
/// justification. MockRuntime stubs Tauri, not the OS — these handlers
/// legitimately spawn real subprocesses (`where` lookups, `--version`
/// probes), and on a cold CI runner those can take well over 5s while
/// still being healthy: first-touch Defender scans of node.exe plus
/// npm's own startup blew the default watchdog on the Windows runner.
/// A longer leash keeps their dispatch coverage instead of punching a
/// SWEEP_DENYLIST hole. Keep entries rare — every one slows detection
/// of a REAL hang in that command to its extended timeout.
const SWEEP_SLOW_COMMANDS: &[(&str, std::time::Duration, &str)] = &[
    (
        "check_npm_available",
        std::time::Duration::from_secs(30),
        "spawns `where npm.cmd` + `npm --version`; cold-runner AV scan + npm startup exceed 5s",
    ),
    (
        "switch_acp_session",
        std::time::Duration::from_secs(30),
        "mock client is never healthy → restart_connection runs 3 spawn attempts \
         behind 300/600/1200ms backoff sleeps (2.1s floor) before erroring",
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
        // The global-shortcut plugin can't initialize headless (its init
        // registers a real OS hook — fails on macOS CI), so its managed
        // state is legitimately absent here but IS managed in production
        // (plugin registered unconditionally in main.rs). Don't flag it.
        if text.contains("GlobalShortcut") {
            return None;
        }
        return Some(format!("STATE NOT MANAGED: {text}"));
    }
    None
}

#[test]
fn command_sweep_no_wiring_failures() {
    let app = mock_app();
    let webview = mock_webview(&app);
    let commands = all_commands();

    // Per-command watchdog + whole-sweep budget. A handler that blocks
    // on a real OS service (WinRT brokers, subprocess waits — headless
    // CI sessions hang where desktops answer) must surface as a NAMED
    // failure, fast. On a mock runtime every legitimate command returns
    // in milliseconds, so 3s is already generous. The budget bounds the
    // pathological case (a headless runner where MANY OS commands hang:
    // pre-budget that wedged CI 40+ minutes with zero output, because
    // cargo captures test output until the test finishes) — when blown,
    // we fail immediately and the panic dumps everything collected so
    // far plus which commands were never reached.
    //
    // Timed-out invokes leak their thread (it's blocked in the OS call);
    // the test process reaps them at exit.
    // 5s not 3s: commands answer in millis on the mock runtime, but the
    // sweep shares the machine with 12 parallel tests — a 3s cutoff
    // produced boundary flakes under load without catching anything 5s
    // doesn't.
    const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    const SWEEP_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);

    let sweep_start = std::time::Instant::now();
    let mut offenses: Vec<String> = Vec::new();
    let mut hung: Vec<String> = Vec::new();
    for (idx, cmd) in commands.iter().enumerate() {
        if SWEEP_DENYLIST.iter().any(|(name, _)| name == cmd) {
            continue;
        }
        if sweep_start.elapsed() > SWEEP_BUDGET {
            let remaining: Vec<_> = commands[idx..].iter().map(|s| s.as_str()).collect();
            offenses.push(format!(
                "  SWEEP BUDGET EXCEEDED after {} commands. Hung so far: [{}]. \
                 Never reached: [{}]",
                idx,
                hung.join(", "),
                remaining.join(", ")
            ));
            break;
        }
        // Progress marker: shows in the failure dump (cargo prints
        // captured output for failed tests) — the last line names the
        // command that was running when things went sideways.
        eprintln!("[sweep] {cmd}");
        let (tx, rx) = std::sync::mpsc::channel();
        let webview = webview.clone();
        let cmd_for_thread = cmd.clone();
        std::thread::spawn(move || {
            // Panics inside a handler unwind through on_message; catch
            // them so one bad command doesn't hide the rest of the sweep.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tauri::test::get_ipc_response(
                    &webview,
                    tauri::webview::InvokeRequest {
                        cmd: cmd_for_thread,
                        callback: tauri::ipc::CallbackFn(0),
                        error: tauri::ipc::CallbackFn(1),
                        url: "http://tauri.localhost".parse().unwrap(),
                        body: tauri::ipc::InvokeBody::default(),
                        headers: Default::default(),
                        invoke_key: tauri::test::INVOKE_KEY.to_string(),
                    },
                )
            }));
            let _ = tx.send(outcome);
        });
        let timeout = SWEEP_SLOW_COMMANDS
            .iter()
            .find(|(name, _, _)| name == cmd)
            .map(|(_, t, _)| *t)
            .unwrap_or(COMMAND_TIMEOUT);
        let outcome = match rx.recv_timeout(timeout) {
            Ok(outcome) => outcome,
            Err(_) => {
                hung.push(cmd.clone());
                offenses.push(format!(
                    "  {cmd}: HUNG (no response in {timeout:?}) — \
                     blocks on a real OS service headless? Add to SWEEP_DENYLIST \
                     (or SWEEP_SLOW_COMMANDS if it's just slow) with a justification."
                ));
                continue;
            }
        };
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
