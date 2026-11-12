//! End to end over the real host-command bridge: Kage's listener on the
//! real OS transport (an AF_UNIX socket / named pipe in isolated user
//! dirs), a client that connects and talks exactly as the MCP sidecar's
//! `run_outside_sandbox` and `get_outside_sandbox_result` do (and as an
//! older sidecar's one-shot version 1 did), and Kage's whole request flow
//! on a mock app — matching the agent's tool call, the job, the rules, the
//! prompt, the run, the reply and the polls, the audit log.
//!
//! The agent is faked by feeding its tool call to the tracker the way the
//! ACP notification handler does. Commands run under a plain `sh -c` (or
//! `cmd /C`), never the developer's login shell, and the prompt's
//! five-minute wait and the 80-second long poll are cut to seconds. Every
//! test holds `SERIAL`: they share the process-wide isolation root, so the
//! bridge's endpoint, and the tracker.

use kage::acp_client::{AcpClient, AcpConnectionMode};
use kage::commands::system::user_shell::runner::ShellInvocation;
use kage::config::HostCommandRule;
use kage::host_commands::approvals::Answer;
use kage::host_commands::{HostCommandOptions, HostCommands};
use kage::state::{build_managed_state, ChildProcesses, ManagedState};
use kage_core::host_bridge::{
    self, ApprovedBy, BridgeRequest, HostCommandRequest, HostCommandResponse, JobState, LineRead,
    PollRequest, ResponseStatus,
};
use kage_core::os::host_bridge as transport;
use std::io::Write;
#[cfg(unix)]
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

/// Options for a test: the prompt waits `approval_timeout`, a call waits
/// on its job `long_poll`, and results are kept a minute.
fn options(approval_timeout: Duration, long_poll: Duration) -> HostCommandOptions {
    HostCommandOptions {
        approval_timeout,
        shell: Some(plain_shell),
        long_poll,
        result_retention: Duration::from_secs(60),
    }
}

/// A mock app managing the state production manages, with these rules
/// and a bridge set up with `options`.
fn mock_app(rules: Vec<HostCommandRule>, options: HostCommandOptions) -> tauri::App<MockRuntime> {
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
    child_processes.host_commands = Arc::new(HostCommands::with_options(options));
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
    /// A bridge whose calls wait on their job long enough for every
    /// test that doesn't exercise "pending".
    fn start(rules: Vec<HostCommandRule>, approval_timeout: Duration) -> Self {
        Self::start_with(rules, options(approval_timeout, Duration::from_secs(30)))
    }

    fn start_with(rules: Vec<HostCommandRule>, options: HostCommandOptions) -> Self {
        let app = mock_app(rules, options);
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

    fn answer(&self, id: &str, answer: Answer) {
        self.host_commands()
            .approvals
            .answer(id, FLOATING, answer)
            .expect("the prompt is waiting");
    }

    fn prompts(&self) -> usize {
        self.host_commands().approvals.payloads_for(FLOATING).len()
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

/// The agent's MCP client reporting that call ended (`completed` once the
/// sidecar returned "pending", `failed` when the agent gave up on it), as
/// the notification handler hands it on: with the title and kind the call
/// was first seen with, which the handler remembers per call.
fn agent_tool_call_ended(bridge: &Bridge, session: &str, call_id: &str, status: &str) -> bool {
    let ended = kage::host_commands::observe_tool_call(
        Some(session),
        &serde_json::json!({ "toolCallId": call_id, "status": status }),
        Some("Running: @kage-computer-control/run_outside_sandbox"),
        Some("other"),
    );
    assert_eq!(ended.as_deref(), Some(call_id));
    bridge
        .host_commands()
        .tool_call_ended(call_id, Some(status))
}

/// One request, sent and read the way the sidecar's `OsBridge` does.
fn send_request(endpoint: &transport::Endpoint, request: &BridgeRequest) -> HostCommandResponse {
    let mut stream = transport::connect(endpoint).expect("the bridge accepts");
    let wait = match request {
        BridgeRequest::OneShot(r) => host_bridge::reply_wait(r.timeout_seconds),
        _ => host_bridge::LONG_POLL_REPLY_WAIT,
    };
    stream.set_read_timeout(Some(wait)).unwrap();
    stream.write_all(request.to_line().as_bytes()).unwrap();
    stream.flush().unwrap();
    match host_bridge::read_line_bounded(&mut stream, host_bridge::MAX_RESPONSE_BYTES).unwrap() {
        LineRead::Line(line) => host_bridge::parse_response(&line).expect("a reply"),
        other => panic!("no reply: {other:?}"),
    }
}

/// `run_outside_sandbox`'s request.
fn send(endpoint: &transport::Endpoint, request: &HostCommandRequest) -> HostCommandResponse {
    send_request(endpoint, &BridgeRequest::Run(request.clone()))
}

/// `get_outside_sandbox_result`'s request.
fn poll(endpoint: &transport::Endpoint, job_id: &str) -> HostCommandResponse {
    send_request(endpoint, &BridgeRequest::Poll(PollRequest::new(job_id)))
}

/// Poll until the job's result is in (up to `max`), as the agent would.
fn poll_until_final(
    endpoint: &transport::Endpoint,
    job_id: &str,
    max: Duration,
) -> HostCommandResponse {
    let deadline = Instant::now() + max;
    loop {
        let response = poll(endpoint, job_id);
        if response.status.is_final() {
            return response;
        }
        assert!(Instant::now() < deadline, "still pending: {response:?}");
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

/// [`audited_actions`], once it reads `want` (up to 10 s).
fn wait_for_audit(command: &str, want: &[&str]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = audited_actions(command);
        if got == want {
            return;
        }
        assert!(Instant::now() < deadline, "audited {got:?}, want {want:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn temp_cwd() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    (dir, path)
}

/// A command that prints `marker`, then takes about `secs` seconds.
fn slow_command(marker: &str, secs: u32) -> String {
    #[cfg(unix)]
    {
        format!("echo {marker} && sleep {secs}")
    }
    #[cfg(windows)]
    {
        format!("echo {marker} && ping -n {} 127.0.0.1 > nul", secs + 1)
    }
}

#[test]
fn a_command_a_rule_covers_runs_without_asking_and_its_output_comes_back() {
    let _serial = serial();
    let bridge = Bridge::start(vec![rule("echo")], Duration::from_secs(60));
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-rule-ok";
    agent_tool_call("hb-session-rule", "hb-call-rule", command, &cwd);

    let response = send(&bridge.endpoint, &request(command, &cwd));

    // Finished within the first call: the final result, naming its job.
    assert_eq!(response.status, ResponseStatus::Ran, "{response:?}");
    assert!(response
        .job_id
        .as_deref()
        .is_some_and(|id| id.starts_with("hc-")));
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
    assert_eq!(run.timeout_seconds, Some(30));
    // Nothing asked the user.
    assert_eq!(bridge.prompts(), 0);
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
    assert_eq!(bridge.prompts(), 0);
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
    assert!(approvals.answer(&id, "chat-2", Answer::RunOnce).is_err());
    approvals.answer(&id, FLOATING, Answer::RunOnce).unwrap();
    assert!(approvals.answer(&id, FLOATING, Answer::RunOnce).is_err());

    let response = client.join().unwrap();
    assert_eq!(response.status, ResponseStatus::Ran, "{response:?}");
    assert_eq!(response.job_id.as_deref(), Some(id.as_str()));
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
fn a_long_wait_comes_back_pending_and_a_poll_returns_the_result() {
    let _serial = serial();
    let bridge = Bridge::start_with(
        Vec::new(),
        options(Duration::from_secs(60), Duration::from_secs(1)),
    );
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-pending-then-polled";
    agent_tool_call("hb-session-poll", "hb-call-poll", command, &cwd);

    // The first call ends at the long poll, before the user answered:
    // "pending", with the job's id and where it is.
    let started = Instant::now();
    let first = send(&bridge.endpoint, &request(command, &cwd));
    assert_eq!(first.status, ResponseStatus::Pending, "{first:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    let job_id = first.job_id.clone().expect("a job id");
    let progress = first.job.expect("progress");
    assert_eq!(progress.state, JobState::WaitingForApproval);
    assert!(progress.approval_expires_in_ms.unwrap() > 50_000);
    let prompt = bridge.wait_for_prompt(FLOATING);
    assert_eq!(prompt["requestId"], job_id.as_str());

    // That call's connection closed, and the agent reports the call done
    // (that's how "pending" ends it): the prompt waits on.
    assert!(!agent_tool_call_ended(
        &bridge,
        "hb-session-poll",
        "hb-call-poll",
        "completed"
    ));
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(bridge.prompts(), 1, "the prompt was withdrawn");

    // A poll while it waits: "pending" again, the same job.
    let again = poll(&bridge.endpoint, &job_id);
    assert_eq!(again.status, ResponseStatus::Pending, "{again:?}");
    assert_eq!(again.job_id.as_deref(), Some(job_id.as_str()));

    // The user approves; the next poll gets the result.
    bridge.answer(&job_id, Answer::RunOnce);
    let done = poll_until_final(&bridge.endpoint, &job_id, Duration::from_secs(20));
    assert_eq!(done.status, ResponseStatus::Ran, "{done:?}");
    assert_eq!(done.job_id.as_deref(), Some(job_id.as_str()));
    let run = done.run.unwrap();
    assert!(run.output.contains("kage-bridge-pending-then-polled"));
    assert_eq!(run.approved_by, ApprovedBy::User);

    // Kept for later polls too.
    let later = poll(&bridge.endpoint, &job_id);
    assert_eq!(later.status, ResponseStatus::Ran);
    assert_eq!(
        audited_actions(command),
        ["requested", "approved", "finished"]
    );
}

#[test]
fn a_running_command_reports_its_progress_to_a_poll() {
    let _serial = serial();
    let bridge = Bridge::start_with(
        Vec::new(),
        options(Duration::from_secs(60), Duration::from_secs(1)),
    );
    let (_dir, cwd) = temp_cwd();
    let command = slow_command("kage-bridge-progress-line", 3);
    agent_tool_call("hb-session-progress", "hb-call-progress", &command, &cwd);

    let endpoint = bridge.endpoint.clone();
    let req = request(&command, &cwd);
    let client = std::thread::spawn(move || send(&endpoint, &req));
    let id = bridge.wait_for_prompt(FLOATING)["requestId"]
        .as_str()
        .unwrap()
        .to_string();
    bridge.answer(&id, Answer::RunOnce);
    let first = client.join().unwrap();
    assert!(matches!(first.status, ResponseStatus::Pending), "{first:?}");

    // Running, with what it printed so far.
    let deadline = Instant::now() + Duration::from_secs(10);
    let progress = loop {
        let r = poll(&bridge.endpoint, &id);
        assert_eq!(r.status, ResponseStatus::Pending, "finished early: {r:?}");
        let p = r.job.unwrap();
        if p.state == JobState::Running
            && p.output_tail
                .as_deref()
                .is_some_and(|t| t.contains("kage-bridge-progress-line"))
        {
            break p;
        }
        assert!(Instant::now() < deadline, "no progress: {p:?}");
    };
    assert!(progress.running_ms.is_some());

    let done = poll_until_final(&bridge.endpoint, &id, Duration::from_secs(20));
    assert_eq!(done.status, ResponseStatus::Ran, "{done:?}");
    assert_eq!(done.run.unwrap().exit_code, Some(0));
}

#[test]
fn a_slow_command_finishes_through_polls_and_no_call_outlasts_the_long_poll() {
    let _serial = serial();
    // `brew install` behind a prompt the user takes a while to answer,
    // scaled down: the real long poll is 80 s against an agent's two
    // minutes and a `sleep 150`; here it's 1 s against a user who answers
    // after more than one of them and a run of 4 s.
    let long_poll = Duration::from_secs(1);
    let bridge = Bridge::start_with(Vec::new(), options(Duration::from_secs(60), long_poll));
    let (_dir, cwd) = temp_cwd();
    let command = slow_command("kage-bridge-slow-started", 4);
    agent_tool_call("hb-session-slow", "hb-call-slow", &command, &cwd);

    // Every call the agent makes ends within the long poll (and the time
    // to answer it), whatever the job is doing: well inside its budget,
    // and well short of the run (a call that waited for it would take
    // about 4 s).
    let budget = long_poll + Duration::from_millis(1500);
    let mut calls = Vec::new();
    let mut timed = |what: &str, f: &dyn Fn() -> HostCommandResponse| {
        let started = Instant::now();
        let response = f();
        let took = started.elapsed();
        assert!(took < budget, "{what} took {took:?}: {response:?}");
        calls.push((response.job.as_ref().map(|p| p.state), took));
        response
    };

    let started = Instant::now();
    let req = request(&command, &cwd);
    let first = timed("run_outside_sandbox", &|| send(&bridge.endpoint, &req));
    assert_eq!(first.status, ResponseStatus::Pending, "{first:?}");
    let job_id = first.job_id.clone().expect("a job id");
    assert_eq!(first.job.unwrap().state, JobState::WaitingForApproval);
    // How the first call ends when it returned "pending".
    agent_tool_call_ended(&bridge, "hb-session-slow", "hb-call-slow", "completed");

    // The user hasn't answered yet: still waiting, the prompt still up.
    let waiting = timed("a poll while waiting", &|| poll(&bridge.endpoint, &job_id));
    assert_eq!(waiting.status, ResponseStatus::Pending, "{waiting:?}");
    assert_eq!(waiting.job.unwrap().state, JobState::WaitingForApproval);
    assert_eq!(bridge.prompts(), 1);

    bridge.answer(&job_id, Answer::RunOnce);
    let done = loop {
        let r = timed("a poll", &|| poll(&bridge.endpoint, &job_id));
        if r.status.is_final() {
            break r;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "still pending: {r:?}"
        );
    };
    assert_eq!(done.status, ResponseStatus::Ran, "{done:?}");
    assert_eq!(done.job_id.as_deref(), Some(job_id.as_str()));
    let run = done.run.unwrap();
    assert_eq!(run.exit_code, Some(0), "{run:?}");
    assert!(run.output.contains("kage-bridge-slow-started"), "{run:?}");
    assert_eq!(run.approved_by, ApprovedBy::User);

    // It took several calls, at least two of them while it ran: no single
    // call waited for the user's answer and the run together.
    let running = calls
        .iter()
        .filter(|(state, _)| *state == Some(JobState::Running))
        .count();
    assert!(running >= 2, "{calls:?}");
    assert!(started.elapsed() > long_poll * 4, "{calls:?}");
    assert_eq!(
        audited_actions(&command),
        ["requested", "approved", "finished"]
    );
}

#[test]
fn a_requester_that_leaves_before_kage_answers_withdraws_its_prompt() {
    let _serial = serial();
    let bridge = Bridge::start(Vec::new(), Duration::from_secs(60));
    let (dir, cwd) = temp_cwd();
    let marker = dir.path().join("ran-early");
    let command = "echo ran > ran-early";
    agent_tool_call("hb-session-left", "hb-call-left", command, &cwd);

    // The request goes, and the requester is gone before any answer:
    // nobody has the job's id, so nobody could read the result.
    {
        let mut stream = transport::connect(&bridge.endpoint).unwrap();
        stream
            .write_all(
                BridgeRequest::Run(request(command, &cwd))
                    .to_line()
                    .as_bytes(),
            )
            .unwrap();
        stream.flush().unwrap();
    }
    wait_for_audit(command, &["requested", "withdrawn"]);
    assert_eq!(bridge.prompts(), 0);
    assert!(!marker.exists());
}

#[test]
fn a_stopped_turn_withdraws_its_prompt_and_nothing_runs() {
    let _serial = serial();
    let bridge = Bridge::start(Vec::new(), Duration::from_secs(60));
    let (dir, cwd) = temp_cwd();
    let marker = dir.path().join("ran-stop");
    // Writes a file in `sh` and `cmd` alike.
    let command = "echo ran > ran-stop";
    agent_tool_call("hb-session-stop", "hb-call-stop", command, &cwd);

    let endpoint = bridge.endpoint.clone();
    let req = request(command, &cwd);
    let client = std::thread::spawn(move || send(&endpoint, &req));
    bridge.wait_for_prompt(FLOATING);

    // What `cancel_generation` does when the user stops the turn.
    bridge.host_commands().cancel_session("hb-session-stop");

    let response = client.join().unwrap();
    assert_eq!(response.status, ResponseStatus::Withdrawn, "{response:?}");
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

/// Every `host_command_update` the app emits from now on, as
/// `(requestId, phase)`, in order.
fn record_updates(bridge: &Bridge) -> Arc<Mutex<Vec<(String, String)>>> {
    use tauri::Listener;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let into = seen.clone();
    bridge
        .app
        .listen_any(kage::events::HOST_COMMAND_UPDATE, move |event| {
            let v: serde_json::Value = serde_json::from_str(event.payload()).unwrap_or_default();
            if let (Some(id), Some(phase)) = (v["requestId"].as_str(), v["phase"].as_str()) {
                into.lock()
                    .unwrap()
                    .push((id.to_string(), phase.to_string()));
            }
        });
    seen
}

fn phases(seen: &Mutex<Vec<(String, String)>>, request_id: &str) -> Vec<String> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|(id, _)| id == request_id)
        .map(|(_, phase)| phase.clone())
        .collect()
}

#[test]
fn the_card_hears_the_result_went_back_only_once_the_agent_took_it() {
    let _serial = serial();
    let bridge = Bridge::start_with(
        Vec::new(),
        options(Duration::from_secs(60), Duration::from_secs(1)),
    );
    let seen = record_updates(&bridge);
    let (_dir, cwd) = temp_cwd();
    let command = slow_command("kage-bridge-kept-for-later", 1);
    agent_tool_call("hb-session-kept", "hb-call-kept", &command, &cwd);
    let job_id = send(&bridge.endpoint, &request(&command, &cwd))
        .job_id
        .expect("pending");
    agent_tool_call_ended(&bridge, "hb-session-kept", "hb-call-kept", "completed");
    bridge.answer(&job_id, Answer::RunOnce);

    // The user stops the agent's turn while the run they approved goes
    // on: it finishes, and nobody has collected the result.
    bridge.host_commands().cancel_session("hb-session-kept");
    let job = bridge.host_commands().jobs.get(&job_id).expect("the job");
    let deadline = Instant::now() + Duration::from_secs(20);
    while job.result().is_none() {
        assert!(Instant::now() < deadline, "the run didn't finish");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(phases(&seen, &job_id), ["started", "finished"]);

    // The agent polls in a later turn: now it has the result.
    let done = poll(&bridge.endpoint, &job_id);
    assert_eq!(done.status, ResponseStatus::Ran, "{done:?}");
    assert!(done
        .run
        .unwrap()
        .output
        .contains("kage-bridge-kept-for-later"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while phases(&seen, &job_id).len() < 3 {
        assert!(Instant::now() < deadline, "{:?}", phases(&seen, &job_id));
        std::thread::sleep(Duration::from_millis(20));
    }
    // Once, however often it's collected.
    assert_eq!(poll(&bridge.endpoint, &job_id).status, ResponseStatus::Ran);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(phases(&seen, &job_id), ["started", "finished", "delivered"]);
}

#[test]
fn a_stopped_turn_withdraws_a_job_the_agent_is_polling() {
    let _serial = serial();
    let bridge = Bridge::start_with(
        Vec::new(),
        options(Duration::from_secs(60), Duration::from_secs(1)),
    );
    let (dir, cwd) = temp_cwd();
    let marker = dir.path().join("ran-polled");
    let command = "echo ran > ran-polled";
    agent_tool_call("hb-session-stop2", "hb-call-stop2", command, &cwd);
    let first = send(&bridge.endpoint, &request(command, &cwd));
    assert_eq!(first.status, ResponseStatus::Pending, "{first:?}");
    let job_id = first.job_id.unwrap();

    bridge.host_commands().cancel_session("hb-session-stop2");

    let done = poll(&bridge.endpoint, &job_id);
    assert_eq!(done.status, ResponseStatus::Withdrawn, "{done:?}");
    assert!(!marker.exists(), "the command ran");
    assert_eq!(bridge.prompts(), 0);
}

#[test]
fn a_disconnected_agent_withdraws_its_sessions_jobs() {
    let _serial = serial();
    let bridge = Bridge::start_with(
        Vec::new(),
        options(Duration::from_secs(60), Duration::from_secs(1)),
    );
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-disconnected";
    agent_tool_call("hb-session-gone", "hb-call-gone", command, &cwd);
    let first = send(&bridge.endpoint, &request(command, &cwd));
    let job_id = first.job_id.expect("pending");

    // What the notification handler does on `_kage/agent_disconnected`.
    bridge.host_commands().agent_disconnected();

    let done = poll(&bridge.endpoint, &job_id);
    assert_eq!(done.status, ResponseStatus::Withdrawn, "{done:?}");
    assert!(done
        .message
        .as_deref()
        .is_some_and(|m| m.contains("disconnected")));
    assert_eq!(audited_actions(command), ["requested", "withdrawn"]);
}

#[test]
fn a_job_the_agent_is_polling_still_times_out_unanswered() {
    let _serial = serial();
    let bridge = Bridge::start_with(
        Vec::new(),
        options(Duration::from_secs(2), Duration::from_secs(1)),
    );
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-polled-expired";
    agent_tool_call("hb-session-exp", "hb-call-exp", command, &cwd);
    let first = send(&bridge.endpoint, &request(command, &cwd));
    let job_id = first.job_id.expect("pending");
    agent_tool_call_ended(&bridge, "hb-session-exp", "hb-call-exp", "completed");

    let done = poll_until_final(&bridge.endpoint, &job_id, Duration::from_secs(10));
    assert_eq!(done.status, ResponseStatus::ApprovalTimedOut, "{done:?}");
    assert_eq!(audited_actions(command), ["requested", "expired"]);
}

#[test]
fn kage_quitting_ends_a_waiting_job() {
    let _serial = serial();
    let bridge = Bridge::start_with(
        Vec::new(),
        options(Duration::from_secs(60), Duration::from_secs(1)),
    );
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-quitting";
    agent_tool_call("hb-session-quit", "hb-call-quit", command, &cwd);
    let job_id = send(&bridge.endpoint, &request(command, &cwd))
        .job_id
        .expect("pending");
    let job = bridge.host_commands().jobs.get(&job_id).expect("the job");

    bridge.host_commands().stop();

    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        if let Some(r) = job.result() {
            break r;
        }
        assert!(Instant::now() < deadline, "still waiting");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(result.status, ResponseStatus::Error);
    assert!(result.message.unwrap().contains("quitting"));
    assert_eq!(bridge.prompts(), 0);
}

#[test]
fn a_tool_call_that_ends_before_its_id_was_given_withdraws_its_prompt() {
    let _serial = serial();
    let bridge = Bridge::start(Vec::new(), Duration::from_secs(60));
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-gave-up";
    agent_tool_call("hb-session-end", "hb-call-end", command, &cwd);

    let endpoint = bridge.endpoint.clone();
    let req = request(command, &cwd);
    let client = std::thread::spawn(move || send(&endpoint, &req));
    bridge.wait_for_prompt(FLOATING);

    // The agent gave up on the call (its own limit, say) before Kage
    // answered "pending": the notification handler hands the ended call
    // on.
    assert!(agent_tool_call_ended(
        &bridge,
        "hb-session-end",
        "hb-call-end",
        "failed"
    ));

    let response = client.join().unwrap();
    assert_eq!(response.status, ResponseStatus::Withdrawn, "{response:?}");
    assert!(response.run.is_none());
}

#[test]
fn a_call_the_agent_cut_off_as_kage_answered_pending_withdraws_its_prompt() {
    let _serial = serial();
    let bridge = Bridge::start_with(
        Vec::new(),
        options(Duration::from_secs(60), Duration::from_secs(1)),
    );
    let (dir, cwd) = temp_cwd();
    let marker = dir.path().join("ran-cut-off");
    let command = "echo ran > ran-cut-off";
    agent_tool_call("hb-session-cut", "hb-call-cut", command, &cwd);

    // Kage wrote "pending", but the agent's own limit cut the call off at
    // that moment: the call ends `failed`, not `completed`, so the agent
    // never took the reply or the id in it. Nobody can read the result.
    let first = send(&bridge.endpoint, &request(command, &cwd));
    assert_eq!(first.status, ResponseStatus::Pending, "{first:?}");
    let job_id = first.job_id.expect("a job id");
    assert_eq!(bridge.prompts(), 1);
    assert!(
        agent_tool_call_ended(&bridge, "hb-session-cut", "hb-call-cut", "failed"),
        "the prompt of a call the agent never took the answer of stayed up"
    );

    let done = poll(&bridge.endpoint, &job_id);
    assert_eq!(done.status, ResponseStatus::Withdrawn, "{done:?}");
    assert_eq!(bridge.prompts(), 0);
    assert!(!marker.exists(), "the command ran");
    assert_eq!(audited_actions(command), ["requested", "withdrawn"]);
}

#[test]
fn a_late_second_end_of_a_call_that_took_pending_changes_nothing() {
    let _serial = serial();
    let bridge = Bridge::start_with(
        Vec::new(),
        options(Duration::from_secs(60), Duration::from_secs(1)),
    );
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-second-end";
    agent_tool_call("hb-session-2end", "hb-call-2end", command, &cwd);
    let job_id = send(&bridge.endpoint, &request(command, &cwd))
        .job_id
        .expect("pending");
    assert!(!agent_tool_call_ended(
        &bridge,
        "hb-session-2end",
        "hb-call-2end",
        "completed"
    ));
    // An agent that reports the same call again (`cancelled`, say) once
    // it had taken "pending": only its first end counts.
    assert!(!agent_tool_call_ended(
        &bridge,
        "hb-session-2end",
        "hb-call-2end",
        "cancelled"
    ));
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(bridge.prompts(), 1, "the prompt was withdrawn");
    bridge.answer(&job_id, Answer::Deny);
    let done = poll_until_final(&bridge.endpoint, &job_id, Duration::from_secs(10));
    assert_eq!(done.status, ResponseStatus::Denied, "{done:?}");
}

#[test]
fn two_polls_at_once_both_get_the_result() {
    let _serial = serial();
    let bridge = Bridge::start_with(
        Vec::new(),
        options(Duration::from_secs(60), Duration::from_secs(1)),
    );
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-two-polls";
    agent_tool_call("hb-session-2poll", "hb-call-2poll", command, &cwd);
    let job_id = send(&bridge.endpoint, &request(command, &cwd))
        .job_id
        .expect("pending");
    // Two polls waiting when the user answers (no poll open at all for a
    // moment in between is the first call's own case above).
    let pollers: Vec<_> = (0..2)
        .map(|_| {
            let (endpoint, id) = (bridge.endpoint.clone(), job_id.clone());
            std::thread::spawn(move || poll_until_final(&endpoint, &id, Duration::from_secs(20)))
        })
        .collect();
    std::thread::sleep(Duration::from_millis(300));
    bridge.answer(&job_id, Answer::RunOnce);
    for p in pollers {
        let done = p.join().unwrap();
        assert_eq!(done.status, ResponseStatus::Ran, "{done:?}");
        assert!(done.run.unwrap().output.contains("kage-bridge-two-polls"));
    }
    // It ran once.
    assert_eq!(
        audited_actions(command),
        ["requested", "approved", "finished"]
    );
}

#[test]
fn a_poll_only_reads_a_job() {
    let _serial = serial();
    let bridge = Bridge::start_with(
        Vec::new(),
        options(Duration::from_secs(60), Duration::from_millis(300)),
    );
    let (dir, cwd) = temp_cwd();
    let marker = dir.path().join("ran-read-only");
    let command = "echo ran > ran-read-only";
    agent_tool_call("hb-session-ro", "hb-call-ro", command, &cwd);
    let job_id = send(&bridge.endpoint, &request(command, &cwd))
        .job_id
        .expect("pending");

    // However often it's polled, the job waits for the user.
    for _ in 0..3 {
        let r = poll(&bridge.endpoint, &job_id);
        assert_eq!(r.status, ResponseStatus::Pending);
    }
    assert_eq!(bridge.prompts(), 1);
    assert!(!marker.exists(), "a poll ran it");

    // A poll carrying anything more than an id is refused unread.
    for line in [
        format!(r#"{{"v":2,"kind":"poll","job_id":"{job_id}","decision":"run_once"}}"#),
        format!(r#"{{"v":2,"kind":"poll","job_id":"{job_id}","command":"echo x"}}"#),
    ] {
        let mut stream = transport::connect(&bridge.endpoint).unwrap();
        stream.write_all(format!("{line}\n").as_bytes()).unwrap();
        stream.flush().unwrap();
        let LineRead::Line(reply) =
            host_bridge::read_line_bounded(&mut stream, host_bridge::MAX_RESPONSE_BYTES).unwrap()
        else {
            panic!("no reply");
        };
        let reply = host_bridge::parse_response(&reply).unwrap();
        assert_eq!(reply.status, ResponseStatus::Error, "{line}");
    }
    assert_eq!(bridge.prompts(), 1);
    assert!(!marker.exists());

    // An id Kage never gave out names nothing.
    let unknown = poll(&bridge.endpoint, "hc-000000000000000000000000");
    assert_eq!(unknown.status, ResponseStatus::NotFound);
}

#[test]
fn a_finished_result_expires() {
    let _serial = serial();
    let mut opts = options(Duration::from_secs(60), Duration::from_secs(5));
    opts.result_retention = Duration::from_secs(1);
    let bridge = Bridge::start_with(vec![rule("echo")], opts);
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-expiring";
    agent_tool_call("hb-session-ttl", "hb-call-ttl", command, &cwd);
    let done = send(&bridge.endpoint, &request(command, &cwd));
    assert_eq!(done.status, ResponseStatus::Ran);
    let job_id = done.job_id.unwrap();
    assert_eq!(poll(&bridge.endpoint, &job_id).status, ResponseStatus::Ran);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        poll(&bridge.endpoint, &job_id).status,
        ResponseStatus::NotFound
    );
}

#[test]
fn an_older_sidecars_one_shot_request_still_works() {
    let _serial = serial();
    let bridge = Bridge::start_with(
        vec![rule("echo")],
        // Even with a long poll this short, a version-1 call waits for
        // the whole job.
        options(Duration::from_secs(60), Duration::from_millis(200)),
    );
    let (_dir, cwd) = temp_cwd();
    let command = "echo kage-bridge-one-shot";
    agent_tool_call("hb-session-v1", "hb-call-v1", command, &cwd);
    let response = send_request(
        &bridge.endpoint,
        &BridgeRequest::OneShot(request(command, &cwd).one_shot()),
    );
    assert_eq!(response.v, 1);
    assert_eq!(response.status, ResponseStatus::Ran, "{response:?}");
    assert!(response.job_id.is_none() && response.job.is_none());
    let run = response.run.unwrap();
    assert!(run.output.contains("kage-bridge-one-shot"));
    assert_eq!(run.timeout_seconds, None);

    // Waiting on the user past the long poll, then withdrawn: version 1
    // hears an error (it has no "withdrawn"), never "pending". (Not a
    // simple command, so the rule doesn't cover it.)
    let command = "echo kage-bridge-one-shot-stopped && echo again";
    agent_tool_call("hb-session-v1b", "hb-call-v1b", command, &cwd);
    let endpoint = bridge.endpoint.clone();
    let req = request(command, &cwd).one_shot();
    let client = std::thread::spawn(move || send_request(&endpoint, &BridgeRequest::OneShot(req)));
    bridge.wait_for_prompt(FLOATING);
    std::thread::sleep(Duration::from_millis(500));
    bridge.host_commands().cancel_session("hb-session-v1b");
    let response = client.join().unwrap();
    assert_eq!(response.v, 1);
    assert_eq!(response.status, ResponseStatus::Error, "{response:?}");
    assert!(response.message.unwrap().contains("did not run"));
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
