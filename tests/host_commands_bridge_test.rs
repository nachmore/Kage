//! End to end over the real host-command bridge: Kage's listener on the
//! real OS transport (an AF_UNIX socket / named pipe in isolated user
//! dirs), a client that connects and talks exactly as the MCP sidecar's
//! `run_outside_sandbox` does, and Kage's whole request flow on a mock app
//! — matching the agent's tool call, the rules, the prompt, the run, the
//! reply, the audit log.
//!
//! The agent is faked by feeding its tool call to the tracker the way the
//! ACP notification handler does. Commands run under a plain `sh -c` (or
//! `cmd /C`), never the developer's login shell, and the prompt's
//! five-minute wait is cut to seconds. Every test holds `SERIAL`: they
//! share the process-wide isolation root, so the bridge's endpoint, and the
//! tracker.

use kage::acp_client::{AcpClient, AcpConnectionMode};
use kage::commands::system::user_shell::runner::ShellInvocation;
use kage::config::HostCommandRule;
use kage::host_commands::{HostCommandOptions, HostCommands};
use kage::state::{build_managed_state, ChildProcesses, ManagedState};
use kage_core::host_bridge::{
    self, ApprovedBy, HostCommandRequest, HostCommandResponse, LineRead, ResponseStatus,
};
use kage_core::os::host_bridge as transport;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::test::{mock_builder, MockRuntime};
use tauri::Manager;

static SERIAL: Mutex<()> = Mutex::new(());

/// The window a request no chat window owns is shown in.
const FLOATING: &str = "floating";

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

/// Point every per-user directory (and with it the bridge's socket, or
/// the pipe's name) at a scratch root, once per process.
fn isolate_user_dirs() {
    // Short on Unix: the socket's path must fit in an AF_UNIX address.
    #[cfg(unix)]
    let base = PathBuf::from("/tmp");
    #[cfg(windows)]
    let base = std::env::temp_dir();
    kage::paths::isolate(base.join(format!("kage-hb-{}", std::process::id())));
    assert!(
        kage::paths::is_isolated(),
        "refusing to run against the real per-user directories"
    );
}

/// Runs a command without any login profile (the developer's rc files
/// stay out of it).
fn plain_shell(command: &str) -> ShellInvocation {
    #[cfg(unix)]
    {
        ShellInvocation {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), "--".into(), command.into()],
            raw_tail: None,
            label: "sh".into(),
        }
    }
    #[cfg(windows)]
    {
        kage::commands::system::user_shell::runner::windows_invocation(
            kage::commands::system::user_shell::runner::WindowsShell::Cmd,
            command,
        )
    }
}

fn rule(prefix: &str) -> HostCommandRule {
    HostCommandRule {
        prefix: prefix.into(),
        created_at: "2026-11-09T00:00:00Z".into(),
    }
}

/// A mock app managing the state production manages, with these rules
/// and a bridge that waits `approval_timeout` for the user.
fn mock_app(rules: Vec<HostCommandRule>, approval_timeout: Duration) -> tauri::App<MockRuntime> {
    isolate_user_dirs();
    kage::os::login_env::set_enabled(false);
    let mut config = kage::config::Config::default();
    config.tool_permissions.host_command_rules = rules;
    let config = Arc::new(Mutex::new(config));
    let acp_client = Arc::new(AcpClient::new(AcpConnectionMode::Local {
        spawn_command: "true".to_string(),
    }));
    let ManagedState {
        acp_handles,
        ui_state,
        mut child_processes,
        feature_services,
    } = build_managed_state(config, acp_client, false);
    child_processes.host_commands = Arc::new(HostCommands::with_options(HostCommandOptions {
        approval_timeout,
        shell: Some(plain_shell),
    }));
    mock_builder()
        .manage(acp_handles)
        .manage(ui_state)
        .manage(child_processes)
        .manage(feature_services)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app must build")
}

/// Start the bridge; stopped (and its endpoint removed) on drop.
struct Bridge {
    app: tauri::App<MockRuntime>,
    endpoint: transport::Endpoint,
}

impl Bridge {
    fn start(rules: Vec<HostCommandRule>, approval_timeout: Duration) -> Self {
        let app = mock_app(rules, approval_timeout);
        app.state::<ChildProcesses>()
            .host_commands
            .start(app.handle().clone());
        let endpoint = transport::default_endpoint().expect("an endpoint");
        Self { app, endpoint }
    }

    fn host_commands(&self) -> Arc<HostCommands> {
        self.app.state::<ChildProcesses>().host_commands.clone()
    }

    /// Wait (up to 10 s) for a prompt in `window`, and return it.
    fn wait_for_prompt(&self, window: &str) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(p) = self
                .host_commands()
                .approvals
                .payloads_for(window)
                .into_iter()
                .next()
            {
                return p;
            }
            assert!(Instant::now() < deadline, "no prompt appeared");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.host_commands().stop();
        // Gone before the next test binds the same endpoint (a pipe goes
        // only once the accept thread has let go of its last instance).
        let deadline = Instant::now() + Duration::from_secs(5);
        while transport::connect(&self.endpoint).is_ok() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// What the agent's MCP client reports for its `run_outside_sandbox`
/// call (via the ACP notification handler), so the request that follows
/// is matched to a session the user sees.
fn agent_tool_call(session: &str, call_id: &str, command: &str, cwd: &str) {
    let update = serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": call_id,
        "title": "Running: @kage-computer-control/run_outside_sandbox",
        "kind": "other",
        "status": "in_progress",
        "rawInput": { "command": command, "cwd": cwd, "reason": "sandboxed" },
    });
    kage::host_commands::observe_tool_call(Some(session), &update, None, None);
}

/// One request, sent and read the way the sidecar's `OsBridge` does.
fn send(endpoint: &transport::Endpoint, request: &HostCommandRequest) -> HostCommandResponse {
    let mut stream = transport::connect(endpoint).expect("the bridge accepts");
    stream
        .set_read_timeout(Some(host_bridge::reply_wait(request.timeout_seconds)))
        .unwrap();
    stream.write_all(request.to_line().as_bytes()).unwrap();
    stream.flush().unwrap();
    match host_bridge::read_line_bounded(&mut stream, host_bridge::MAX_RESPONSE_BYTES).unwrap() {
        LineRead::Line(line) => host_bridge::parse_response(&line).expect("a reply"),
        other => panic!("no reply: {other:?}"),
    }
}

fn request(command: &str, cwd: &str) -> HostCommandRequest {
    HostCommandRequest::new(
        command,
        Some(cwd.to_string()),
        "sandbox-exec: sandbox_apply: Operation not permitted",
        30,
        Some("fake-agent".into()),
    )
}

/// The audit log's actions for the request whose command is `command`,
/// oldest first.
fn audited_actions(command: &str) -> Vec<String> {
    // Newest first, as Settings lists them.
    kage::permission_audit::read_recent_default(500)
        .into_iter()
        .rev()
        .filter_map(|e| match e.event {
            kage::permission_audit::AuditEvent::HostCommand {
                action,
                command: Some(c),
                ..
            } if c == command => Some(action),
            _ => None,
        })
        .collect()
}

fn temp_cwd() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    (dir, path)
}

#[test]
fn a_command_a_rule_covers_runs_without_asking_and_its_output_comes_back() {
    let _serial = serial();
    let bridge = Bridge::start(vec![rule("echo")], Duration::from_secs(60));
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-rule-ok";
    agent_tool_call("hb-session-rule", "hb-call-rule", command, &cwd);

    let response = send(&bridge.endpoint, &request(command, &cwd));

    assert_eq!(response.status, ResponseStatus::Ran, "{response:?}");
    let run = response.run.expect("a run summary");
    assert_eq!(run.outcome, "exited");
    assert_eq!(run.exit_code, Some(0));
    assert!(
        run.output.contains("kage-bridge-rule-ok"),
        "{:?}",
        run.output
    );
    assert_eq!(run.approved_by, ApprovedBy::Rule);
    assert_eq!(run.rule.as_deref(), Some("echo"));
    // Nothing asked the user.
    assert!(bridge
        .host_commands()
        .approvals
        .payloads_for(FLOATING)
        .is_empty());
    assert_eq!(
        audited_actions(command),
        ["requested", "auto_approved", "finished"]
    );
}

#[test]
fn a_command_no_rule_covers_waits_for_the_user_then_times_out() {
    let _serial = serial();
    let timeout = Duration::from_secs(2);
    let bridge = Bridge::start(vec![rule("ls")], timeout);
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-unanswered";
    agent_tool_call("hb-session-wait", "hb-call-wait", command, &cwd);

    let endpoint = bridge.endpoint.clone();
    let (req, started) = (request(command, &cwd), Instant::now());
    let client = std::thread::spawn(move || send(&endpoint, &req));

    // The prompt names exactly this request, in the window it's for.
    let prompt = bridge.wait_for_prompt(FLOATING);
    assert_eq!(prompt["command"], command);
    assert_eq!(prompt["window"], FLOATING);
    assert_eq!(prompt["matched"], true);
    assert_eq!(prompt["sessionId"], "hb-session-wait");
    assert_eq!(prompt["toolCallId"], "hb-call-wait");

    let response = client.join().unwrap();
    let waited = started.elapsed();
    assert_eq!(
        response.status,
        ResponseStatus::ApprovalTimedOut,
        "{response:?}"
    );
    assert!(response.run.is_none(), "nothing ran");
    assert!(waited >= timeout, "answered after {waited:?}");
    assert!(
        waited < timeout + Duration::from_secs(8),
        "waited {waited:?}"
    );
    assert!(bridge
        .host_commands()
        .approvals
        .payloads_for(FLOATING)
        .is_empty());
    assert_eq!(audited_actions(command), ["requested", "expired"]);
}

#[test]
fn a_command_the_user_approves_runs_once() {
    let _serial = serial();
    let bridge = Bridge::start(Vec::new(), Duration::from_secs(60));
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-approved";
    agent_tool_call("hb-session-ok", "hb-call-ok", command, &cwd);

    let endpoint = bridge.endpoint.clone();
    let req = request(command, &cwd);
    let client = std::thread::spawn(move || send(&endpoint, &req));
    let prompt = bridge.wait_for_prompt(FLOATING);
    let id = prompt["requestId"].as_str().unwrap().to_string();
    let approvals = &bridge.host_commands().approvals;

    // Only the window it was shown in may answer it, once.
    assert!(approvals
        .answer(
            &id,
            "chat-2",
            kage::host_commands::approvals::Answer::RunOnce
        )
        .is_err());
    approvals
        .answer(
            &id,
            FLOATING,
            kage::host_commands::approvals::Answer::RunOnce,
        )
        .unwrap();
    assert!(approvals
        .answer(
            &id,
            FLOATING,
            kage::host_commands::approvals::Answer::RunOnce
        )
        .is_err());

    let response = client.join().unwrap();
    assert_eq!(response.status, ResponseStatus::Ran, "{response:?}");
    let run = response.run.unwrap();
    assert_eq!(run.exit_code, Some(0));
    assert!(
        run.output.contains("kage-bridge-approved"),
        "{:?}",
        run.output
    );
    assert_eq!(run.approved_by, ApprovedBy::User);
    assert_eq!(
        audited_actions(command),
        ["requested", "approved", "finished"]
    );
}

#[test]
fn a_stopped_turn_withdraws_its_prompt_and_nothing_runs() {
    let _serial = serial();
    let bridge = Bridge::start(Vec::new(), Duration::from_secs(60));
    let (dir, cwd) = temp_cwd();
    let marker = dir.path().join("ran");
    // Writes a file in `sh` and `cmd` alike.
    let command = "echo ran > ran";
    agent_tool_call("hb-session-stop", "hb-call-stop", command, &cwd);

    let endpoint = bridge.endpoint.clone();
    let req = request(command, &cwd);
    let client = std::thread::spawn(move || send(&endpoint, &req));
    bridge.wait_for_prompt(FLOATING);

    // What `cancel_generation` does when the user stops the turn.
    bridge.host_commands().cancel_session("hb-session-stop");

    let response = client.join().unwrap();
    assert_eq!(response.status, ResponseStatus::Error, "{response:?}");
    assert!(
        response
            .message
            .as_deref()
            .is_some_and(|m| m.contains("did not run")),
        "{response:?}"
    );
    assert!(!marker.exists(), "the command ran");
    assert_eq!(audited_actions(command), ["requested", "withdrawn"]);
}

#[test]
fn a_tool_call_that_ends_withdraws_its_prompt() {
    let _serial = serial();
    let bridge = Bridge::start(Vec::new(), Duration::from_secs(60));
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-gave-up";
    agent_tool_call("hb-session-end", "hb-call-end", command, &cwd);

    let endpoint = bridge.endpoint.clone();
    let req = request(command, &cwd);
    let client = std::thread::spawn(move || send(&endpoint, &req));
    bridge.wait_for_prompt(FLOATING);

    // The agent gave up on the call (its own MCP timeout, say): the
    // notification handler hands the ended call to the prompts.
    let ended = kage::host_commands::observe_tool_call(
        Some("hb-session-end"),
        &serde_json::json!({ "toolCallId": "hb-call-end", "status": "failed" }),
        None,
        None,
    );
    assert_eq!(ended.as_deref(), Some("hb-call-end"));
    assert!(bridge
        .host_commands()
        .approvals
        .cancel_tool_call("hb-call-end"));

    let response = client.join().unwrap();
    assert_eq!(response.status, ResponseStatus::Error, "{response:?}");
    assert!(response.run.is_none());
}

#[test]
fn the_endpoint_is_gone_once_the_bridge_stops() {
    let _serial = serial();
    let endpoint = {
        let bridge = Bridge::start(Vec::new(), Duration::from_secs(60));
        assert!(transport::connect(&bridge.endpoint).is_ok());
        bridge.endpoint.clone()
        // Dropping it stops the bridge and waits for the endpoint to go.
    };
    // What the sidecar then tells the agent: Kage isn't running.
    let err = match transport::connect(&endpoint) {
        Ok(_) => panic!("still listening"),
        Err(e) => e,
    };
    assert!(
        matches!(
            err.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
        ),
        "{err:?}"
    );
}

#[test]
fn the_bridge_listens_again_after_an_aborted_shutdown() {
    let _serial = serial();
    let bridge = Bridge::start(vec![rule("echo")], Duration::from_secs(60));
    // What `graceful_shutdown` does before an update installs.
    bridge.host_commands().stop();
    let deadline = Instant::now() + Duration::from_secs(5);
    while transport::connect(&bridge.endpoint).is_ok() {
        assert!(Instant::now() < deadline, "still listening after stop");
        std::thread::sleep(Duration::from_millis(20));
    }
    // A start after stop stays off (a shutdown is under way) …
    bridge.host_commands().start(bridge.app.handle().clone());
    assert!(transport::connect(&bridge.endpoint).is_err());

    // … until the install fails and the app stays up.
    bridge.host_commands().resume(bridge.app.handle());
    let deadline = Instant::now() + Duration::from_secs(10);
    while transport::connect(&bridge.endpoint).is_err() {
        assert!(Instant::now() < deadline, "the bridge didn't come back");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-resumed";
    agent_tool_call("hb-session-resume", "hb-call-resume", command, &cwd);
    let response = send(&bridge.endpoint, &request(command, &cwd));
    assert_eq!(response.status, ResponseStatus::Ran, "{response:?}");
    assert!(response
        .run
        .is_some_and(|r| r.output.contains("kage-bridge-resumed")));
}
