//! Background agent calls end to end (docs/design/EXTENSION_AGENT_CALLS.md
//! §12): the real commands, service, strict ephemeral sessions and ACP
//! client, on a mock Tauri app, against a scripted mock agent over TCP
//! (`tests/mock_acp_server.rs`).
//!
//! Every per-user directory is redirected to a scratch dir first
//! (`kage::paths::isolate`), so nothing here touches a real config, audit
//! log or session list.
//!
//! The production ACP notification handler
//! (`commands::messaging::setup_notification_handler`) needs the Wry
//! runtime, so these tests install a small handler that routes the same
//! way for what background calls need: reply chunks into the session's
//! bucket, and for a strict session a refused permission request, a tool
//! call stopped through `ephemeral_session::note_tool_attempt`, and the
//! disconnect signal. The strict decision itself is unit-tested beside the
//! real handler.
//!
//! Tests run one at a time: the service's strict-session hooks are
//! process-wide, and the newest started service owns them.

#[path = "mock_acp_server.rs"]
mod mock_acp_server;

use kage::acp_client::{AcpClient, AcpConnectionMode};
use kage::permission_audit::AuditEvent;
use kage::state::{build_managed_state, FeatureServices, ManagedState};
use mock_acp_server::{ScriptedAgent, Step, Turn};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::Duration;
use tauri::test::{mock_builder, MockRuntime};
use tauri::{Listener, Manager};

/// One test at a time (see the module docs).
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

fn root() -> PathBuf {
    std::env::temp_dir().join(format!("kage-agent-calls-test-{}", std::process::id()))
}

fn isolate_user_dirs() {
    kage::paths::isolate(root());
    assert!(
        kage::paths::is_isolated(),
        "refusing to run against the real per-user directories"
    );
}

/// Turns don't start on low battery; such a machine can't run these.
fn low_battery() -> bool {
    let low = kage::os::power::get_power_state() == kage::os::power::PowerState::LowBattery;
    if low {
        eprintln!("skipped: background agent calls wait while the battery is low");
    }
    low
}

const WAIT: Duration = Duration::from_secs(15);

/// Which harness is current. The disconnect signal and the strict-session
/// registry are process-wide, and the newest service owns the hooks, so an
/// earlier test's client, which can notice its agent closing after that
/// test ended, would end the current test's call. Only the current
/// harness's client is routed.
static HARNESS: AtomicUsize = AtomicUsize::new(0);

/// Route what the agent sends, as the production handler does for a
/// background call (see the module docs).
fn route_notifications(client: &Arc<AcpClient>) {
    let weak = Arc::downgrade(client);
    let mine = HARNESS.fetch_add(1, Ordering::SeqCst) + 1;
    client.set_notification_handler(move |msg: Value| {
        if HARNESS.load(Ordering::SeqCst) != mine {
            return;
        }
        let Some(client) = weak.upgrade() else {
            return;
        };
        let method = msg["method"].as_str().unwrap_or("");
        let session = msg["params"]["sessionId"].as_str().unwrap_or("");
        let strict = kage::ephemeral_session::is_strict(session);
        match method {
            "_kage/agent_disconnected" => kage::ephemeral_session::notify_disconnected(),
            "session/request_permission" if strict => {
                let reject = msg["params"]["options"]
                    .as_array()
                    .and_then(|o| o.iter().find(|o| o["kind"] == "reject_once"))
                    .and_then(|o| o["optionId"].as_str())
                    .unwrap_or("reject_once")
                    .to_string();
                let _ = client.send_permission_response(&msg["id"], &reject);
                kage::ephemeral_session::note_tool_attempt(&client, session);
            }
            "session/update" => {
                let update = &msg["params"]["update"];
                match update["sessionUpdate"].as_str() {
                    Some("agent_message_chunk") => {
                        if let Some(text) = update["content"]["text"].as_str() {
                            client.accumulate_chunk(session, text);
                        }
                    }
                    Some("tool_call" | "tool_call_update") if strict => {
                        kage::ephemeral_session::note_tool_attempt(&client, session);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    });
}

struct Harness {
    app: tauri::App<MockRuntime>,
    agent: ScriptedAgent,
    client: Arc<AcpClient>,
    /// The launcher: where an extension's sandbox host calls from.
    floating: tauri::WebviewWindow<MockRuntime>,
    /// Settings: the host panel.
    settings: tauri::WebviewWindow<MockRuntime>,
    ext: String,
    sessions_dir: PathBuf,
    /// `agent_calls_activity` payloads the launcher received.
    activity: mpsc::Receiver<Value>,
}

fn extensions_dir() -> PathBuf {
    kage::paths::config_dir()
        .unwrap()
        .join("kage")
        .join("extensions")
}

/// Install a fake extension that declares `agentBackground`.
fn install(id: &str) {
    let dir = extensions_dir().join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = json!({
        "id": id,
        "name": format!("Test {id}"),
        "version": "1.0.0",
        "type": "extension",
        "permissions": ["storage", "agent_background"],
        "agentBackground": { "reason": "To sort notes", "purposes": ["organize"] },
        "config": { "api_key": { "type": "string", "sensitive": true } }
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
}

/// A mock app whose agent is `turns`, with `ext` installed, granted
/// `agent_background` and (when `connect`) background AI turned on the way
/// Settings does it.
fn harness(ext: &str, turns: Vec<Turn>, connect: bool) -> Harness {
    isolate_user_dirs();
    kage::os::login_env::set_enabled(false);
    let sessions_dir = root().join(format!("sessions-{ext}"));
    std::fs::create_dir_all(&sessions_dir).unwrap();
    let agent = ScriptedAgent::start(turns, Some(sessions_dir.clone()));
    // Without `connect`, the client points at a port nothing listens on.
    let port = if connect {
        agent.port
    } else {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        closed.local_addr().unwrap().port()
    };

    let mut config = kage::config::Config::default();
    {
        let conn = config
            .acp
            .active_connection_mut()
            .expect("the default config has a connection");
        conn.name = "Test agent".into();
        conn.preset_id = None;
        conn.mode = kage::config::AcpMode::Remote {
            host: "127.0.0.1".into(),
            port,
            timeout_ms: 5_000,
        };
        conn.sessions_directory = Some(sessions_dir.to_string_lossy().into_owned());
    }
    config.extension_grants.insert(
        ext.to_string(),
        kage::config::ExtensionGrant {
            granted: vec!["storage".into(), "agent_background".into()],
            approved_version: "1.0.0".into(),
            approved_at: String::new(),
            origin: None,
        },
    );
    let config = Arc::new(std::sync::Mutex::new(config));
    let client = Arc::new(AcpClient::new(AcpConnectionMode::Remote {
        host: "127.0.0.1".into(),
        port,
    }));
    route_notifications(&client);
    if connect {
        client.connect().expect("connect to the mock agent");
    }

    let ManagedState {
        acp_handles,
        ui_state,
        child_processes,
        feature_services,
    } = build_managed_state(config, client.clone(), false);
    let app = mock_builder()
        .manage(acp_handles)
        .manage(ui_state)
        .manage(child_processes)
        .manage(feature_services)
        .invoke_handler(kage::commands::registry::invoke_handler())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app must build");
    kage::commands::agent_calls::start_service(app.handle());
    install(ext);

    let floating = mock_window(&app, "floating");
    let settings = mock_window(&app, "settings");
    let (tx, activity) = mpsc::channel::<Value>();
    floating.listen("agent_calls_activity", move |event| {
        let _ = tx.send(serde_json::from_str(event.payload()).unwrap_or(Value::Null));
    });

    let h = Harness {
        app,
        agent,
        client,
        floating,
        settings,
        ext: ext.to_string(),
        sessions_dir,
        activity,
    };
    // Settings: the token from `get`, back to `set`.
    let panel = h.host("get_extension_agent_access", json!({}));
    assert_eq!(panel.as_ref().unwrap()["access"], "off");
    let token = panel.unwrap()["consent_token"].clone();
    assert!(token.is_string(), "a configured agent has a consent token");
    h.host(
        "set_extension_agent_access",
        json!({ "enabled": true, "consentToken": token }),
    )
    .expect("turn background AI on");
    h
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

fn err_key(result: &Result<Value, Value>) -> String {
    match result {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => e["key"].as_str().unwrap_or_default().to_string(),
    }
}

impl Harness {
    fn features(&self) -> tauri::State<'_, FeatureServices> {
        self.app.state::<FeatureServices>()
    }

    /// A host-only command for this extension, from Settings.
    fn host(&self, cmd: &str, mut args: Value) -> Result<Value, Value> {
        args["extensionId"] = json!(self.ext);
        invoke(&self.settings, cmd, args)
    }

    /// An extension command, as the sandbox host sends it (`extensionId`
    /// injected).
    fn ext_call(&self, cmd: &str, args: Value) -> Result<Value, Value> {
        invoke(
            &self.floating,
            cmd,
            json!({ "extensionId": self.ext, "args": args }),
        )
    }

    fn reserve(&self) -> String {
        let r = self
            .ext_call("agent_call_reserve", json!({ "purpose": "organize" }))
            .expect("reserve");
        assert_eq!(r["position"], 1);
        r["call_id"].as_str().unwrap().to_string()
    }

    /// Reserve, wait for the turn, and start sending `request` on another
    /// thread. Returns the call id and the send's result.
    fn start_call(&self, request: Value) -> (String, mpsc::Receiver<Result<Value, Value>>) {
        let call_id = self.reserve();
        let turn = self
            .ext_call("agent_call_ready", json!({ "call_id": call_id }))
            .expect("the turn starts");
        assert_eq!(turn["call_id"], call_id.as_str());
        assert!(turn["send_by"].is_string());
        let (tx, rx) = mpsc::channel();
        let floating = self.floating.clone();
        let args = json!({
            "extensionId": self.ext,
            "args": { "call_id": call_id, "request": request },
        });
        std::thread::spawn(move || {
            let _ = tx.send(invoke(&floating, "agent_call_send", args));
        });
        (call_id, rx)
    }

    fn call(&self, request: Value) -> (String, Result<Value, Value>) {
        let (id, rx) = self.start_call(request);
        (id, rx.recv_timeout(WAIT).expect("the send answers"))
    }

    /// Wait for the agent to receive the call's prompt; returns its session.
    fn prompt_session(&self) -> String {
        assert!(
            self.agent
                .wait_until(WAIT, |a| !a.params_of("session/prompt").is_empty()),
            "the prompt reaches the agent"
        );
        self.agent.params_of("session/prompt")[0]["sessionId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// [`Harness::prompt_session`] for a call from [`Harness::start_call`]:
    /// when the prompt never arrives, the panic says what the send answered
    /// instead, so a refused or failed call is told apart from a slow one.
    fn prompt_session_of(&self, rx: &mpsc::Receiver<Result<Value, Value>>) -> String {
        let arrived = self
            .agent
            .wait_until(WAIT, |a| !a.params_of("session/prompt").is_empty());
        assert!(
            arrived,
            "the prompt reaches the agent (the send answered: {:?})",
            rx.try_recv()
        );
        self.prompt_session()
    }

    fn assert_cancelled(&self, session: &str) {
        assert!(
            self.agent
                .wait_until(WAIT, |a| a.cancelled().iter().any(|s| s == session)),
            "session/cancel reaches the agent for {session}"
        );
    }

    fn assert_session_files_gone(&self, session: &str) {
        for ext in ["json", "jsonl", "lock"] {
            let path = self.sessions_dir.join(format!("{session}.{ext}"));
            assert!(wait(|| !path.exists()), "{} is deleted", path.display());
        }
    }

    /// This extension's audit entries, newest first.
    fn audit(&self) -> Vec<AuditEvent> {
        kage::permission_audit::read_recent_default(500)
            .into_iter()
            .map(|e| e.event)
            .filter(|e| match e {
                AuditEvent::ExtensionAgentCall { extension_id, .. }
                | AuditEvent::ExtensionAgentAccessChanged { extension_id, .. } => {
                    extension_id == &self.ext
                }
                _ => false,
            })
            .collect()
    }

    fn last_call_entry(&self) -> Value {
        assert!(
            wait(|| self
                .audit()
                .iter()
                .any(|e| matches!(e, AuditEvent::ExtensionAgentCall { .. }))),
            "the call has an audit entry"
        );
        let entry = self
            .audit()
            .into_iter()
            .find(|e| matches!(e, AuditEvent::ExtensionAgentCall { .. }))
            .unwrap();
        serde_json::to_value(entry).unwrap()
    }

    /// Mark the user active, so idle calls wait.
    fn user_active(&self) {
        invoke(&self.floating, "touch_floating_activity", json!({})).expect("activity");
    }
}

fn wait(done: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + WAIT;
    while std::time::Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

fn plain(instructions: &str) -> Value {
    json!({ "instructions": instructions, "input": "Some notes" })
}

// --- Tests -----------------------------------------------------------------------

#[test]
fn a_call_runs_on_a_strict_throwaway_session_and_is_recorded() {
    let _serial = serial();
    if low_battery() {
        return;
    }
    let h = harness(
        "ac3-round-trip",
        vec![Turn(vec![Step::Chunks(vec![
            "```json\n{\"items\": ".into(),
            "[1, 2]}\n```".into(),
        ])])],
        true,
    );
    let (call_id, result) = h.call(json!({
        "label": "Organize 2 notes",
        "instructions": "Number these notes.",
        "input": { "notes": ["first", "second"] },
        "schema": {
            "type": "object", "required": ["items"],
            "properties": { "items": { "type": "array", "items": { "type": "integer" } } }
        },
        "maxResponseChars": 2000,
    }));
    let result = result.expect("the call succeeds");
    assert_eq!(result["call_id"], call_id.as_str());
    assert_eq!(result["json"], json!({ "items": [1, 2] }));
    assert!(result["text"].as_str().unwrap().contains("[1, 2]"));
    assert!(result["prompt_chars"].as_u64().unwrap() > 0);
    assert_eq!(result["redactions"], 0);

    // The agent got one framed prompt, nothing else in front of it, in
    // the empty scratch folder.
    let new = h.agent.params_of("session/new");
    assert_eq!(new.len(), 1);
    let scratch = kage::ephemeral_session::scratch_dir().unwrap();
    assert_eq!(new[0]["cwd"], scratch.to_string_lossy().as_ref());
    let prompt = &h.agent.params_of("session/prompt")[0];
    let blocks = prompt["prompt"].as_array().unwrap();
    assert_eq!(blocks.len(), 1, "no time stamp or hidden block");
    let text = blocks[0]["text"].as_str().unwrap();
    assert!(text.starts_with("[Kage background task"));
    assert!(text.contains("Number these notes."));
    assert!(text.contains("\"first\""));

    // The throwaway session is gone, recorded and hidden.
    let session = prompt["sessionId"].as_str().unwrap().to_string();
    h.assert_session_files_gone(&session);
    assert!(kage::ephemeral_session::is_hidden(&session));
    assert!(kage::ephemeral_session::is_strict(&session));
    // An agent that writes its files back still never lists it.
    std::fs::write(h.sessions_dir.join(format!("{session}.json")), "{}").unwrap();
    std::fs::write(h.sessions_dir.join("user-chat.json"), "{}").unwrap();
    let listed = invoke(&h.settings, "list_sessions", json!({ "force": true })).unwrap();
    let ids: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["session_id"].as_str())
        .collect();
    assert!(ids.contains(&"user-chat"), "{ids:?}");
    assert!(!ids.contains(&session.as_str()), "{ids:?}");

    // One audit entry, with counts only.
    let entry = h.last_call_entry();
    assert_eq!(entry["call_id"], call_id.as_str());
    assert_eq!(entry["outcome"], "ok");
    assert_eq!(entry["purpose"], "organize");
    assert_eq!(entry["label"], "Organize 2 notes");
    assert_eq!(entry["agent"], "Test agent");
    assert_eq!(entry["prompt_chars"], result["prompt_chars"]);
    assert!(h.audit().iter().any(|e| matches!(
        e,
        AuditEvent::ExtensionAgentAccessChanged { enabled: true, .. }
    )));

    // Status and the panel.
    let status = h
        .ext_call_status()
        .expect("status answers for the extension");
    assert_eq!(status["access"], "on");
    assert_eq!(status["connected"], true);
    assert_eq!(status["running"], false);
    assert_eq!(status["waiting"], 0);
    assert_eq!(status["usage"]["calls_today"], 1);
    assert!(
        status.get("agent").is_none(),
        "status never names the agent"
    );
    let panel = h.host("get_extension_agent_access", json!({})).unwrap();
    assert_eq!(panel["access"], "on");
    assert_eq!(panel["agent"]["label"], "Test agent");
    assert_eq!(panel["viewable"], json!([call_id]));
    assert!(panel["log"].as_array().unwrap().len() >= 2);
    let detail = h
        .host(
            "get_extension_agent_request_detail",
            json!({ "callId": call_id }),
        )
        .unwrap();
    assert_eq!(detail["prompt"].as_str().unwrap(), text);
    assert!(detail["reply"].as_str().unwrap().contains("[1, 2]"));

    // First-party windows heard the call start and end.
    let events: Vec<Value> =
        std::iter::from_fn(|| h.activity.recv_timeout(Duration::from_secs(1)).ok())
            .take(20)
            .collect();
    assert!(events
        .iter()
        .any(|e| e["running"][0]["extension_id"] == h.ext.as_str()
            && e["running"][0]["purpose"] == "organize"));
    assert_eq!(events.last().unwrap()["running"], json!([]));
}

impl Harness {
    fn ext_call_status(&self) -> Result<Value, Value> {
        invoke(
            &self.floating,
            "agent_call_status",
            json!({ "extensionId": self.ext }),
        )
    }
}

#[test]
fn a_tool_the_agent_asks_for_is_refused_and_stops_the_call() {
    let _serial = serial();
    if low_battery() {
        return;
    }
    let h = harness(
        "ac3-permission",
        vec![Turn(vec![
            Step::AskPermission,
            Step::WaitForCancel(WAIT),
            Step::Chunks(vec!["done".into()]),
        ])],
        true,
    );
    let (_, result) = h.call(plain("Read this."));
    assert_eq!(err_key(&result), "errors.agent_call.tool_attempted");
    let session = h.prompt_session();
    // The answer and the stop go out together; the call can end before the
    // agent has read the answer.
    assert!(
        h.agent
            .wait_until(WAIT, |a| !a.permission_answers().is_empty()),
        "the permission request is answered"
    );
    assert_eq!(
        h.agent.permission_answers(),
        vec![json!({ "outcome": { "outcome": "selected", "optionId": "mock-reject" } })],
        "refused with the request's own reject option"
    );
    h.assert_cancelled(&session);
    h.assert_session_files_gone(&session);
    assert_eq!(h.last_call_entry()["outcome"], "tool_attempted");
}

#[test]
fn a_tool_the_agent_starts_on_its_own_stops_the_call() {
    let _serial = serial();
    if low_battery() {
        return;
    }
    let h = harness(
        "ac3-tool-call",
        vec![Turn(vec![
            Step::Chunks(vec!["Let me check.".into()]),
            Step::ToolCall,
            Step::WaitForCancel(WAIT),
        ])],
        true,
    );
    let (_, result) = h.call(plain("Summarise."));
    assert_eq!(err_key(&result), "errors.agent_call.tool_attempted");
    let session = h.prompt_session();
    h.assert_cancelled(&session);
    assert_eq!(h.last_call_entry()["outcome"], "tool_attempted");
}

#[test]
fn a_slow_reply_times_out_and_is_cancelled() {
    let _serial = serial();
    if low_battery() {
        return;
    }
    let h = harness(
        "ac3-timeout",
        vec![Turn(vec![
            Step::Chunks(vec!["Thinking".into()]),
            Step::WaitForCancel(Duration::from_secs(30)),
        ])],
        true,
    );
    let (_, rx) = h.start_call(json!({
        "instructions": "Take your time.", "input": "x", "timeoutMs": 5000,
    }));
    let result = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("the timeout answers");
    assert_eq!(err_key(&result), "errors.agent_call.timeout");
    assert_eq!(result.as_ref().unwrap_err()["kind"], "timeout");
    h.assert_cancelled(&h.prompt_session());
    assert_eq!(h.last_call_entry()["outcome"], "timeout");
}

#[test]
fn a_long_reply_is_stopped_while_it_streams() {
    let _serial = serial();
    if low_battery() {
        return;
    }
    let h = harness(
        "ac3-too-long",
        vec![Turn(vec![
            Step::Chunks(vec!["x".repeat(400)]),
            Step::WaitForCancel(WAIT),
        ])],
        true,
    );
    let (_, result) = h.call(json!({
        "instructions": "Be brief.", "input": "x", "maxResponseChars": 20,
    }));
    assert_eq!(err_key(&result), "errors.agent_call.response_too_large");
    h.assert_cancelled(&h.prompt_session());
    assert_eq!(h.last_call_entry()["outcome"], "response_too_large");
}

#[test]
fn a_disconnect_mid_call_ends_it() {
    let _serial = serial();
    if low_battery() {
        return;
    }
    let h = harness(
        "ac3-disconnect",
        vec![Turn(vec![
            Step::Chunks(vec!["partial".into()]),
            Step::Sleep(Duration::from_millis(200)),
            Step::Hangup,
        ])],
        true,
    );
    let (_, result) = h.call(plain("Go."));
    assert_eq!(err_key(&result), "errors.agent_call.agent_disconnected");
    assert_eq!(result.unwrap_err()["kind"], "connection_lost");
    assert!(wait(|| !h.client.is_connected()));
    assert_eq!(h.last_call_entry()["outcome"], "connection_lost");
}

#[test]
fn without_a_connected_agent_nothing_is_reserved_or_dialled() {
    let _serial = serial();
    let h = harness("ac3-no-agent", Vec::new(), false);
    let result = h.ext_call("agent_call_reserve", json!({ "purpose": "organize" }));
    assert_eq!(err_key(&result), "errors.agent_call.agent_unavailable");
    assert_eq!(result.unwrap_err()["kind"], "connection_lost");
    let status = h.ext_call_status().unwrap();
    assert_eq!(status["access"], "on");
    assert_eq!(status["connected"], false);
    assert!(!h.client.is_connected(), "the agent was never dialled");
}

#[test]
fn a_user_prompt_stops_the_running_call() {
    let _serial = serial();
    if low_battery() {
        return;
    }
    let h = harness(
        "ac3-user-prompt",
        vec![
            Turn(vec![
                Step::Chunks(vec!["Working".into()]),
                Step::WaitForCancel(WAIT),
            ]),
            Turn::reply("Hello!"),
        ],
        true,
    );
    let (_, rx) = h.start_call(plain("Organise."));
    let session = h.prompt_session_of(&rx);
    let main = mock_window(&h.app, "main");
    invoke(
        &main,
        "send_message_streaming",
        json!({ "sessionId": "user-chat", "message": "Hi" }),
    )
    .expect("the user's prompt is accepted");
    let result = rx.recv_timeout(WAIT).expect("the send answers");
    assert_eq!(
        err_key(&result),
        "errors.agent_call.cancelled_user_activity"
    );
    assert_eq!(result.unwrap_err()["kind"], "cancelled");
    h.assert_cancelled(&session);
    let entry = h.last_call_entry();
    assert_eq!(entry["outcome"], "cancelled");
    assert_eq!(entry["cancel_reason"], "activity");
}

#[test]
fn turning_the_extension_off_or_uninstalling_it_ends_its_calls() {
    let _serial = serial();
    if low_battery() {
        return;
    }
    let h = harness(
        "ac3-turned-off",
        vec![Turn(vec![
            Step::Chunks(vec!["Working".into()]),
            Step::WaitForCancel(WAIT),
        ])],
        true,
    );
    // A running call ends when the extension is turned off…
    let (_, rx) = h.start_call(plain("Organise."));
    let session = h.prompt_session_of(&rx);
    invoke(
        &h.settings,
        "set_extension_enabled",
        json!({ "id": h.ext, "enabled": false }),
    )
    .unwrap();
    let result = rx.recv_timeout(WAIT).expect("the send answers");
    assert_eq!(err_key(&result), "errors.agent_call.cancelled_access_off");
    h.assert_cancelled(&session);
    let entry = h.last_call_entry();
    assert_eq!(entry["cancel_reason"], "access_off");
    // …and a turned-off extension can't reserve. Its consent stays.
    let refused = h.ext_call("agent_call_reserve", json!({ "purpose": "organize" }));
    assert_eq!(err_key(&refused), "errors.agent_call.extension_disabled");
    assert!(h
        .features()
        .config
        .lock()
        .unwrap()
        .extension_agent_access
        .contains_key(&h.ext));

    // Turned on again, a reservation waits while the user is active, and
    // ends when the extension is uninstalled, which also forgets the consent.
    invoke(
        &h.settings,
        "set_extension_enabled",
        json!({ "id": h.ext, "enabled": true }),
    )
    .unwrap();
    h.user_active();
    let call_id = h.reserve();
    let ready = {
        let floating = h.floating.clone();
        let args = json!({ "extensionId": h.ext, "args": { "call_id": call_id } });
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(invoke(&floating, "agent_call_ready", args));
        });
        rx
    };
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(h.ext_call_status().unwrap()["waiting"], 1);
    invoke(
        &h.settings,
        "uninstall_extension",
        json!({ "id": h.ext, "kind": "extension" }),
    )
    .unwrap();
    let result = ready.recv_timeout(WAIT).expect("ready answers");
    assert_eq!(err_key(&result), "errors.agent_call.cancelled_access_off");
    assert!(!h
        .features()
        .config
        .lock()
        .unwrap()
        .extension_agent_access
        .contains_key(&h.ext));
}

#[test]
fn an_agent_change_makes_waiting_calls_stale() {
    let _serial = serial();
    if low_battery() {
        return;
    }
    let h = harness("ac3-agent-change", Vec::new(), true);
    h.user_active();
    let call_id = h.reserve();
    let (tx, ready) = mpsc::channel();
    {
        let floating = h.floating.clone();
        let args = json!({ "extensionId": h.ext, "args": { "call_id": call_id } });
        std::thread::spawn(move || {
            let _ = tx.send(invoke(&floating, "agent_call_ready", args));
        });
    }
    std::thread::sleep(Duration::from_millis(300));
    // Another agent connection becomes active (as a settings save or a
    // synced change would), and every window hears `config_updated`.
    {
        let features = h.features();
        let mut cfg = features.config.lock().unwrap();
        let conn = cfg.acp.active_connection_mut().unwrap();
        conn.mode = kage::config::AcpMode::Remote {
            host: "127.0.0.1".into(),
            port: 1,
            timeout_ms: 5_000,
        };
    }
    tauri::Emitter::emit(h.app.handle(), "config_updated", ()).unwrap();
    let result = ready.recv_timeout(WAIT).expect("ready answers");
    assert_eq!(err_key(&result), "errors.agent_call.consent_stale");
    assert_eq!(h.ext_call_status().unwrap()["access"], "consent_stale");
    // Settings asks again: an old token is refused.
    let stale = h.host(
        "set_extension_agent_access",
        json!({ "enabled": true, "consentToken": "0".repeat(64) }),
    );
    assert_eq!(err_key(&stale), "errors.agent_call.agent_changed");
}

#[test]
fn stop_now_cancels_and_pauses_the_extension() {
    let _serial = serial();
    if low_battery() {
        return;
    }
    let h = harness(
        "ac3-stop-now",
        vec![Turn(vec![
            Step::Chunks(vec!["Working".into()]),
            Step::WaitForCancel(WAIT),
        ])],
        true,
    );
    let (_, rx) = h.start_call(plain("Organise."));
    let session = h.prompt_session_of(&rx);
    h.host("stop_extension_agent_calls", json!({})).unwrap();
    let result = rx.recv_timeout(WAIT).expect("the send answers");
    assert_eq!(err_key(&result), "errors.agent_call.cancelled_by_user");
    h.assert_cancelled(&session);
    assert_eq!(h.last_call_entry()["cancel_reason"], "user");
    let status = h.ext_call_status().unwrap();
    assert!(status["paused_until"].is_string());
    let refused = h.ext_call("agent_call_reserve", json!({ "purpose": "organize" }));
    assert_eq!(err_key(&refused), "errors.agent_call.paused");
    // Turning it off is recorded, and clears the consent.
    h.host("set_extension_agent_access", json!({ "enabled": false }))
        .unwrap();
    assert!(h.audit().iter().any(|e| matches!(
        e,
        AuditEvent::ExtensionAgentAccessChanged { enabled: false, .. }
    )));
    assert_eq!(h.ext_call_status().unwrap()["access"], "off");
}

#[test]
fn shutdown_cancels_the_call_and_cleans_up() {
    let _serial = serial();
    if low_battery() {
        return;
    }
    let h = harness(
        "ac3-shutdown",
        vec![Turn(vec![
            Step::Chunks(vec!["Working".into()]),
            Step::WaitForCancel(WAIT),
        ])],
        true,
    );
    let (_, rx) = h.start_call(plain("Organise."));
    let session = h.prompt_session_of(&rx);
    kage::commands::system::graceful_shutdown(h.app.handle());
    let result = rx.recv_timeout(WAIT).expect("the send answers");
    assert_eq!(err_key(&result), "errors.agent_call.cancelled_shutdown");
    h.assert_cancelled(&session);
    h.assert_session_files_gone(&session);
    let entry = h.last_call_entry();
    assert_eq!(entry["outcome"], "cancelled");
    assert_eq!(entry["cancel_reason"], "shutdown");
    let refused = h.ext_call("agent_call_reserve", json!({ "purpose": "organize" }));
    assert_eq!(err_key(&refused), "errors.agent_call.cancelled_shutdown");
}

#[test]
fn another_extensions_call_ids_are_unknown() {
    let _serial = serial();
    if low_battery() {
        return;
    }
    let h = harness("ac3-owner", Vec::new(), true);
    h.user_active();
    let call_id = h.reserve();
    let other = |cmd: &str| {
        invoke(
            &h.floating,
            cmd,
            json!({ "extensionId": "ac3-someone-else", "args": { "call_id": call_id } }),
        )
    };
    assert_eq!(
        err_key(&other("agent_call_ready")),
        "errors.agent_call.unknown_call"
    );
    other("agent_call_cancel").expect("cancel of an unknown id is fine");
    assert_eq!(h.ext_call_status().unwrap()["waiting"], 1, "still waiting");
    h.ext_call("agent_call_cancel", json!({ "call_id": call_id }))
        .unwrap();
    assert_eq!(h.ext_call_status().unwrap()["waiting"], 0);
}
