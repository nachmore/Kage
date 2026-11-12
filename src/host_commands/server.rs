//! The host-command bridge's listening end, and what happens to a request:
//! match it to the asking session, make its job, check it against the
//! guard and the rules, ask the user when needed, run it, and hand the
//! result to whoever waits on the job — the connection that asked, for at
//! most the long poll, then the agent's polls (`jobs`).
//!
//! Every step is written to the permission audit log (one entry per step,
//! tied together by the request id) and counted in telemetry (counts only:
//! never the command, never a path).

use super::approvals::{Answer, PendingInfo};
use super::guard::{self, CredentialHit};
use super::jobs::{Job, JobsFull, Waited};
use super::rules::{self, AskReason, Decision};
use super::HostCommands;
use crate::commands::system::user_shell::{self, runner};
use crate::lock_ext::LockExt;
use crate::permission_audit::{AuditEntry, AuditEvent};
use crate::state::{AcpHandles, ChildProcesses, FeatureServices, UiState};
use crate::window_labels;
use kage_core::host_bridge::{
    self, ApprovedBy, BridgeRequest, HostCommandRequest, HostCommandResponse, LineRead,
    PollRequest, RunSummary, ONE_SHOT_VERSION, OUTPUT_TAIL_BYTES,
};
use kage_core::os::host_bridge::{default_endpoint, BridgeListener, BridgeStream};
use log::{info, warn};
use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

/// Connections handled at once; more are dropped unread.
const MAX_CONNECTIONS: usize = 16;

/// Agent runs going at once, across windows.
const MAX_AGENT_RUNS: usize = 4;

/// How long a request waits for its tool call to show up in the ACP stream.
const MATCH_WAIT: Duration = Duration::from_millis(1500);

/// A client gets this long, in all, to send its request line: a client
/// that trickles bytes in (or sends none) can't keep a connection slot.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// How often a waiting request looks for an answer, the clock and its
/// requester.
const POLL: Duration = Duration::from_millis(250);

/// The running listener.
pub struct BridgeHandle {
    listener: Arc<BridgeListener>,
    stop: Arc<AtomicBool>,
}

impl BridgeHandle {
    /// Stop accepting and remove the endpoint. The accept thread ends
    /// within half a second; requests in flight finish on their own.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.listener.wake();
        self.listener.remove_endpoint();
        info!("Host bridge: stopped listening");
    }
}

/// Bind the bridge and accept on a background thread. `None` (logged) when
/// it can't start: agents then get "Kage isn't running" from the tool.
pub fn start<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Option<BridgeHandle> {
    let Some(endpoint) = default_endpoint() else {
        warn!("Host bridge: no endpoint for this user; agents can't run commands outside their sandbox");
        return None;
    };
    let listener = match BridgeListener::bind(&endpoint) {
        Ok(l) => Arc::new(l),
        Err(e) => {
            warn!("Host bridge didn't start: {e}");
            return None;
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let (thread_listener, thread_stop) = (listener.clone(), stop.clone());
    let spawned = std::thread::Builder::new()
        .name("host-bridge".into())
        .spawn(move || accept_loop(app, thread_listener, thread_stop));
    if let Err(e) = spawned {
        warn!("Host bridge: couldn't start its thread: {e}");
        listener.remove_endpoint();
        return None;
    }
    info!("Host bridge listening");
    Some(BridgeHandle { listener, stop })
}

fn accept_loop<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    listener: Arc<BridgeListener>,
    stop: Arc<AtomicBool>,
) {
    let active = Arc::new(AtomicUsize::new(0));
    loop {
        match listener.accept(&stop) {
            Ok(Some(stream)) => {
                if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    active.fetch_sub(1, Ordering::SeqCst);
                    warn!("Host bridge: too many connections; dropped one");
                    continue;
                }
                let (app, active_in) = (app.clone(), active.clone());
                let spawned = std::thread::Builder::new()
                    .name("host-bridge-request".into())
                    .spawn(move || {
                        handle_connection(&app, stream);
                        active_in.fetch_sub(1, Ordering::SeqCst);
                    });
                if let Err(e) = spawned {
                    active.fetch_sub(1, Ordering::SeqCst);
                    warn!("Host bridge: couldn't start a request thread: {e}");
                }
            }
            Ok(None) => break,
            Err(e) => {
                warn!("Host bridge: accept failed: {e}");
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

/// What reading a connection's request came to.
enum Incoming {
    Request(BridgeRequest),
    /// Answer with this and close.
    Refused(HostCommandResponse),
    /// Close without answering (nothing arrived, or not in time).
    Drop,
}

/// Read and parse the one request a connection carries, within
/// [`READ_TIMEOUT`] in all.
fn read_request(stream: &mut BridgeStream) -> Incoming {
    match stream.read_line_within(host_bridge::MAX_REQUEST_BYTES, READ_TIMEOUT) {
        Ok(LineRead::Line(line)) => match host_bridge::parse_request(&line) {
            Ok(request) => Incoming::Request(request),
            Err(e) => {
                warn!("Host bridge: refused a request: {e}");
                Incoming::Refused(HostCommandResponse::error(format!(
                    "Kage refused the request: {e}."
                )))
            }
        },
        Ok(LineRead::TooLong) => Incoming::Refused(HostCommandResponse::error(format!(
            "Kage refused the request: larger than {} bytes.",
            host_bridge::MAX_REQUEST_BYTES
        ))),
        Ok(LineRead::Eof) => Incoming::Drop,
        Err(e) => {
            warn!("Host bridge: couldn't read a request: {e}");
            Incoming::Drop
        }
    }
}

fn handle_connection<R: tauri::Runtime>(app: &tauri::AppHandle<R>, mut stream: BridgeStream) {
    let request = match read_request(&mut stream) {
        Incoming::Request(request) => request,
        Incoming::Refused(response) => {
            reply(&mut stream, &response);
            return;
        }
        Incoming::Drop => return,
    };
    // The long poll counts from the request's arrival.
    let received = Instant::now();
    let _ = stream.set_read_timeout(None);
    let Some(host) = app
        .try_state::<ChildProcesses>()
        .map(|p| p.host_commands.clone())
    else {
        let starting = HostCommandResponse::error("Kage is starting up; try again.");
        reply(&mut stream, &starting.for_version(request.version()));
        return;
    };
    match request {
        BridgeRequest::OneShot(req) => one_shot(app, &host, req, stream),
        BridgeRequest::Run(req) => start_and_wait(app, &host, req, received, stream),
        BridgeRequest::Poll(poll) => poll_job(&host, &poll, received, stream),
    }
}

/// Send `response` as the reply. Whether it went.
fn reply(stream: &mut BridgeStream, response: &HostCommandResponse) -> bool {
    match stream
        .write_all(response.to_line().as_bytes())
        .and_then(|()| stream.flush())
    {
        Ok(()) => true,
        Err(e) => {
            info!("Host bridge: the requester went away before the reply ({e})");
            false
        }
    }
}

/// A version-1 request (a sidecar from an older Kage): the connection
/// waits for the whole job, and is bound to it as version 1 always was —
/// the requester going away withdraws a prompt still waiting and stops
/// the run.
fn one_shot<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    host: &HostCommands,
    req: HostCommandRequest,
    mut stream: BridgeStream,
) {
    let job = match start_job(app, host, req, true) {
        Ok(job) => job,
        Err(response) => {
            reply(&mut stream, &response.for_version(ONE_SHOT_VERSION));
            return;
        }
    };
    match job.wait(None, || stream.peer_closed()) {
        Waited::Finished(response) => {
            reply(&mut stream, &response.for_version(ONE_SHOT_VERSION));
        }
        Waited::GaveUp | Waited::Pending(_) => {
            if job.requester_left() {
                info!(
                    "Host command {}: the agent went away; stopping the run",
                    job.id
                );
            }
        }
    }
}

/// A version-2 `run`: make its job, wait for it at most the long poll,
/// then answer "pending" with the job's id. From then on the job doesn't
/// depend on this connection or the tool call behind it.
fn start_and_wait<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    host: &HostCommands,
    req: HostCommandRequest,
    received: Instant,
    mut stream: BridgeStream,
) {
    let job = match start_job(app, host, req, false) {
        Ok(job) => job,
        Err(response) => {
            reply(&mut stream, &response);
            return;
        }
    };
    let deadline = received + host.options().long_poll;
    match job.wait(Some(deadline), || stream.peer_closed()) {
        Waited::Finished(response) => {
            reply(&mut stream, &response.for_job(&job.id));
        }
        Waited::Pending(progress) => {
            // Marked before it's sent, so the tool call ending right after
            // (which is how this reply ends it) can't race it.
            job.mark_id_given();
            if reply(
                &mut stream,
                &HostCommandResponse::pending(&job.id, progress),
            ) {
                info!(
                    "Host command {}: not finished yet; the agent waits on the job",
                    job.id
                );
            } else {
                job.unmark_id_given();
                job.abandon();
            }
        }
        Waited::GaveUp => {
            info!(
                "Host command {}: the requester went away before Kage answered",
                job.id
            );
            job.abandon();
        }
    }
}

/// A version-2 `poll`: wait on the job at most the long poll and say
/// where it is. It only reads the job.
fn poll_job(host: &HostCommands, poll: &PollRequest, received: Instant, mut stream: BridgeStream) {
    let Some(job) = host.jobs.get(&poll.job_id) else {
        info!("Host bridge: a poll named no job Kage holds");
        reply(&mut stream, &HostCommandResponse::not_found(&poll.job_id));
        return;
    };
    let deadline = received + host.options().long_poll;
    let response = match job.wait(Some(deadline), || stream.peer_closed()) {
        Waited::Finished(response) => response.for_job(&job.id),
        Waited::Pending(progress) => HostCommandResponse::pending(&job.id, progress),
        Waited::GaveUp => return,
    };
    reply(&mut stream, &response);
}

// --- One request ---------------------------------------------------------------

/// Everything known about a request once it has been matched and checked.
struct Request {
    id: String,
    req: HostCommandRequest,
    /// The window that shows it (the asking session's, else floating).
    window: String,
    session_id: Option<String>,
    tool_call_id: Option<String>,
    /// Matched to a tool call of a session the user sees.
    visible_origin: bool,
    background: bool,
    agent: Option<String>,
    cwd: PathBuf,
    shell: String,
    hits: Vec<CredentialHit>,
}

impl Request {
    fn hit_names(&self) -> Vec<String> {
        self.hits
            .iter()
            .filter_map(|h| serde_json::to_value(h).ok())
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    }

    fn audit(&self, action: &str) -> AuditStep {
        AuditStep {
            action: action.to_string(),
            request_id: Some(self.id.clone()),
            command: Some(crate::permission_audit::command_preview(&self.req.command)),
            cwd: Some(self.cwd.to_string_lossy().into_owned()),
            agent: self.agent.clone(),
            session_id: self.session_id.clone(),
            ..AuditStep::default()
        }
    }

    fn track<R: tauri::Runtime>(&self, app: &tauri::AppHandle<R>, step: &str) {
        crate::telemetry::track(
            app,
            "host_command",
            Some(serde_json::json!({
                "step": step,
                "guard": u8::from(!self.hits.is_empty()),
                "matched": u8::from(self.visible_origin),
            })),
        );
    }

    fn close<R: tauri::Runtime>(&self, app: &tauri::AppHandle<R>, reason: &str) {
        emit_to(
            app,
            &self.window,
            crate::events::HOST_COMMAND_UPDATE,
            &serde_json::json!({ "requestId": &self.id, "window": &self.window, "phase": "closed", "reason": reason }),
        );
    }
}

/// One audit entry for a request (or a rule change).
#[derive(Default)]
pub(super) struct AuditStep {
    pub action: String,
    pub request_id: Option<String>,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub agent: Option<String>,
    pub session_id: Option<String>,
    pub by: Option<String>,
    pub rule: Option<String>,
    pub guard: Option<Vec<String>>,
    pub outcome: Option<String>,
    pub exit_code: Option<i32>,
}

impl AuditStep {
    pub fn write(self) {
        crate::permission_audit::append(&AuditEntry::now(AuditEvent::HostCommand {
            action: self.action,
            request_id: self.request_id,
            command: self.command,
            cwd: self.cwd,
            agent: self.agent,
            session_id: self.session_id.map(|s| s.chars().take(128).collect()),
            by: self.by,
            rule: self.rule,
            guard: self.guard,
            outcome: self.outcome,
            exit_code: self.exit_code,
        }));
    }
}

fn emit_to<R: tauri::Runtime, S: Serialize + Clone>(
    app: &tauri::AppHandle<R>,
    label: &str,
    event: &str,
    payload: &S,
) {
    if let Err(e) = app.emit_to(
        tauri::EventTarget::webview_window(label),
        event,
        payload.clone(),
    ) {
        log::debug!("emit {event} -> {label} failed: {e}");
    }
}

/// An id no other request or window picks: `<prefix>-<24 hex>`.
fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
        .chars()
        .take(prefix.len() + 25)
        .collect()
}

fn is_background<R: tauri::Runtime>(app: &tauri::AppHandle<R>, session_id: &str) -> bool {
    app.try_state::<AcpHandles>()
        .is_some_and(|acp| acp.client.is_background_prompt_session(session_id))
        || crate::ephemeral_session::is_active(session_id)
}

/// The window that owns `session_id`: the one whose prompt is running it,
/// else one showing it; the floating window when neither is open.
fn window_for_session<R: tauri::Runtime>(app: &tauri::AppHandle<R>, session_id: &str) -> String {
    let Some(ui) = app.try_state::<UiState>() else {
        return window_labels::FLOATING.to_string();
    };
    let originator = ui
        .pending_prompt_originators
        .lock_or_recover()
        .get(session_id)
        .cloned();
    let pinned = || {
        ui.window_sessions
            .lock_or_recover()
            .iter()
            .find(|(_, s)| s.as_str() == session_id)
            .map(|(label, _)| label.clone())
    };
    originator
        .filter(|l| user_shell::caller_allowed(l) && app.get_webview_window(l).is_some())
        .or_else(|| {
            pinned()
                .filter(|l| user_shell::caller_allowed(l) && app.get_webview_window(l).is_some())
        })
        .unwrap_or_else(|| window_labels::FLOATING.to_string())
}

/// Text a request carried, made safe to show: no hidden or control
/// characters, at most `max` characters.
fn clean_label(s: &str, max: usize) -> Option<String> {
    let clean: String = s
        .chars()
        .filter(|c| !c.is_control() && !rules::is_hidden_char(*c))
        .take(max)
        .collect();
    let clean = clean.trim().to_string();
    (!clean.is_empty()).then_some(clean)
}

/// Which agent asked: the connected agent's name for a request from one of
/// Kage's sessions; otherwise what the sidecar reports (the program that
/// started it), which is all Kage knows.
fn agent_label<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    req: &HostCommandRequest,
    visible: bool,
) -> Option<String> {
    let reported = req.agent.as_deref().and_then(|a| clean_label(a, 64));
    if !visible {
        return reported;
    }
    app.try_state::<FeatureServices>()
        .and_then(|f| {
            f.config
                .lock_or_recover()
                .acp
                .active_connection()
                .and_then(|c| clean_label(&c.name, 64))
        })
        .or(reported)
}

/// How `command` runs: the user's login shell, unless the bridge was set
/// up with another ([`super::HostCommandOptions::shell`], tests).
fn shell_for<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    command: &str,
) -> runner::ShellInvocation {
    let custom = app
        .try_state::<ChildProcesses>()
        .and_then(|p| p.host_commands.options().shell);
    match custom {
        Some(shell) => shell(command),
        None => runner::user_shell_invocation(command, None),
    }
}

fn cwd_error(e: &runner::InvalidRun) -> String {
    match e {
        runner::InvalidRun::NoHome => "Kage couldn't find the user's home folder.".into(),
        runner::InvalidRun::CwdNotAbsolute(p) => {
            format!("The working directory isn't an absolute path: {p}")
        }
        runner::InvalidRun::CwdMissing(p) => format!("The working directory doesn't exist: {p}"),
        runner::InvalidRun::CwdNotDirectory(p) => {
            format!("The working directory isn't a folder: {p}")
        }
        other => format!("Invalid request: {other:?}"),
    }
}

/// Check a `run` request, match it to the asking session (the rules and
/// allow-all only ever apply through this match, made from the request
/// itself), and make its job, which a thread of its own takes from there
/// ([`process`]). `Err` carries the reply to a request refused before any
/// job was made.
fn start_job<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    host: &HostCommands,
    req: HostCommandRequest,
    one_shot: bool,
) -> Result<Arc<Job>, Box<HostCommandResponse>> {
    let r = prepare(app, req)?;
    let job = match host.jobs.create(
        &r.id,
        r.session_id.clone(),
        r.tool_call_id.clone(),
        one_shot,
    ) {
        Ok(job) => job,
        Err(full) => {
            warn!(
                "Host command {}: too many waiting or running ({full:?})",
                r.id
            );
            AuditStep {
                outcome: Some("too many commands waiting or running".into()),
                ..r.audit("refused")
            }
            .write();
            r.track(app, "refused");
            return Err(Box::new(HostCommandResponse::error(match full {
                JobsFull::Session => {
                    "Too many commands from this session are waiting for the user or running; \
                     wait for one to finish."
                }
                JobsFull::Total => {
                    "Too many commands are waiting for the user or running; wait for one to finish."
                }
            })));
        }
    };
    let (thread_app, thread_job) = (app.clone(), job.clone());
    let spawned = std::thread::Builder::new()
        .name("host-command-job".into())
        .spawn(move || {
            let response = process(&thread_app, &r, &thread_job);
            thread_job.finish(response);
        });
    if let Err(e) = spawned {
        warn!("Host bridge: couldn't start a job thread: {e}");
        job.finish(HostCommandResponse::error(
            "Kage couldn't take the request; try again.",
        ));
    }
    Ok(job)
}

/// The checks a request gets as it arrives, up to its "requested" audit
/// entry. `Err` carries the reply to one refused.
fn prepare<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    req: HostCommandRequest,
) -> Result<Request, Box<HostCommandResponse>> {
    let id = new_id("hc");
    // A working directory that can't be shown as written: the user would
    // approve a different folder from the one the command runs in.
    if req.cwd.as_deref().is_some_and(|c| !rules::is_plain_path(c)) {
        let why = "The working directory contains hidden or control characters.".to_string();
        AuditStep {
            action: "refused".into(),
            request_id: Some(id),
            command: Some(crate::permission_audit::command_preview(&req.command)),
            outcome: Some(why.clone()),
            ..AuditStep::default()
        }
        .write();
        return Err(Box::new(HostCommandResponse::error(why)));
    }
    let matched =
        super::tracker::tracker().claim_waiting(&req.command, req.cwd.as_deref(), MATCH_WAIT);
    let session_id = matched.as_ref().and_then(|m| m.session_id.clone());
    let tool_call_id = matched.as_ref().map(|m| m.tool_call_id.clone());
    // No tool runs on a strict session (a background agent call), not even
    // with the user's approval: its owner never asked for one.
    if session_id
        .as_deref()
        .is_some_and(crate::ephemeral_session::is_strict)
    {
        info!("Host command {id}: refused, it came from a background agent call");
        AuditStep {
            action: "refused".into(),
            request_id: Some(id),
            command: Some(crate::permission_audit::command_preview(&req.command)),
            session_id,
            outcome: Some("background agent call".into()),
            ..AuditStep::default()
        }
        .write();
        return Err(Box::new(HostCommandResponse::error(
            "Tools can't run in this session.",
        )));
    }
    let background = session_id
        .as_deref()
        .is_some_and(|sid| is_background(app, sid));
    let visible_origin = session_id.is_some() && !background;
    let window = match session_id.as_deref() {
        Some(sid) if visible_origin => window_for_session(app, sid),
        _ => window_labels::FLOATING.to_string(),
    };
    let agent = agent_label(app, &req, visible_origin);

    let home = crate::paths::home_dir();
    let cwd = match runner::resolve_cwd(req.cwd.as_deref(), home.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            let why = cwd_error(&e);
            AuditStep {
                action: "refused".into(),
                request_id: Some(id),
                command: Some(crate::permission_audit::command_preview(&req.command)),
                cwd: req.cwd.clone(),
                agent,
                session_id,
                outcome: Some(why.clone()),
                ..AuditStep::default()
            }
            .write();
            return Err(Box::new(HostCommandResponse::error(why)));
        }
    };
    let hits = guard::request_hits(&req.command, &cwd);
    let r = Request {
        id,
        req,
        window,
        session_id,
        tool_call_id,
        visible_origin,
        background,
        agent,
        cwd,
        shell: shell_for(app, "").label,
        hits,
    };
    info!(
        "Host command {}: requested ({} bytes, program '{}', {}matched to a session{})",
        r.id,
        r.req.command.len(),
        runner::program_label(&r.req.command),
        if r.visible_origin { "" } else { "not " },
        if r.background {
            ", a background one"
        } else {
            ""
        }
    );
    r.audit("requested").write();
    if !r.hits.is_empty() {
        AuditStep {
            guard: Some(r.hit_names()),
            ..r.audit("guard")
        }
        .write();
    }
    Ok(r)
}

/// A job's own thread: the rules, the prompt and the run. Returns the
/// job's result.
fn process<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    r: &Request,
    job: &Arc<Job>,
) -> HostCommandResponse {
    let decision = match app.try_state::<FeatureServices>() {
        Some(features) => {
            let config = features.config.lock_or_recover();
            rules::decide(
                &r.req.command,
                !r.hits.is_empty(),
                r.visible_origin,
                &config.tool_permissions.host_command_rules,
                config.tool_permissions.host_commands_allow_all,
            )
        }
        None => Decision::Ask(AskReason::NoRule),
    };
    let (approved_by, rule) = match decision {
        Decision::Rule(prefix) => {
            info!("Host command {}: allowed by the rule '{}'", r.id, prefix);
            AuditStep {
                by: Some("rule".into()),
                rule: Some(prefix.clone()),
                ..r.audit("auto_approved")
            }
            .write();
            r.track(app, "auto_rule");
            (ApprovedBy::Rule, Some(prefix))
        }
        Decision::AllowAll => {
            info!("Host command {}: allowed by allow-all", r.id);
            AuditStep {
                by: Some("allow_all".into()),
                ..r.audit("auto_approved")
            }
            .write();
            r.track(app, "auto_all");
            (ApprovedBy::AllowAll, None)
        }
        Decision::Ask(reason) => match ask(app, r, reason, job) {
            Asked::Approved(always) => {
                info!("Host command {}: approved by the user", r.id);
                AuditStep {
                    by: Some(if always.is_some() { "always" } else { "once" }.into()),
                    rule: always.clone(),
                    ..r.audit("approved")
                }
                .write();
                r.track(
                    app,
                    if always.is_some() {
                        "approved_always"
                    } else {
                        "approved_once"
                    },
                );
                (ApprovedBy::User, always)
            }
            Asked::Denied => {
                info!("Host command {}: denied by the user", r.id);
                r.audit("denied").write();
                r.track(app, "denied");
                return HostCommandResponse::denied("");
            }
            Asked::Expired => {
                info!("Host command {}: nobody answered", r.id);
                r.audit("expired").write();
                r.track(app, "expired");
                return HostCommandResponse::approval_timed_out();
            }
            Asked::Withdrawn => {
                info!(
                    "Host command {}: the agent went away before it got the job's id",
                    r.id
                );
                AuditStep {
                    outcome: Some("the tool call ended before Kage answered".into()),
                    ..r.audit("withdrawn")
                }
                .write();
                r.track(app, "withdrawn");
                return HostCommandResponse::withdrawn(
                    "The request was withdrawn: the tool call ended before Kage could answer, \
                     so the command did not run.",
                );
            }
            Asked::Busy => {
                warn!("Host command {}: too many prompts waiting", r.id);
                AuditStep {
                    outcome: Some("too many prompts waiting".into()),
                    ..r.audit("refused")
                }
                .write();
                r.track(app, "refused");
                return HostCommandResponse::error(
                    "Too many commands are waiting for the user's answer; try again later.",
                );
            }
            Asked::WindowClosed => {
                info!("Host command {}: its window closed", r.id);
                AuditStep {
                    outcome: Some("window closed".into()),
                    ..r.audit("withdrawn")
                }
                .write();
                r.track(app, "withdrawn");
                return HostCommandResponse::withdrawn(
                    "The window showing the request was closed, so the command did not run.",
                );
            }
            Asked::Cancelled => {
                info!(
                    "Host command {}: its turn was stopped or its tool call ended",
                    r.id
                );
                AuditStep {
                    outcome: Some("the agent's turn or tool call ended".into()),
                    ..r.audit("withdrawn")
                }
                .write();
                r.track(app, "withdrawn");
                return HostCommandResponse::withdrawn(
                    "The request was withdrawn before the user answered (the agent's turn was \
                     stopped, or its tool call ended), so the command did not run.",
                );
            }
            Asked::Disconnected => {
                info!("Host command {}: the agent disconnected", r.id);
                AuditStep {
                    outcome: Some("the agent disconnected".into()),
                    ..r.audit("withdrawn")
                }
                .write();
                r.track(app, "withdrawn");
                return HostCommandResponse::withdrawn(
                    "The request was withdrawn before the user answered: the agent disconnected \
                     from Kage, so the command did not run.",
                );
            }
            Asked::Shutdown => {
                r.close(app, "shutdown");
                return HostCommandResponse::error("Kage is quitting; the command did not run.");
            }
        },
    };
    run(app, r, approved_by, rule, job)
}

enum Asked {
    /// Run; with the rule's prefix when "Always allow" made one.
    Approved(Option<String>),
    Denied,
    Expired,
    /// Nobody can read the result: the requester went away, or its tool
    /// call ended, before it was given the job's id.
    Withdrawn,
    Busy,
    /// The window showing the prompt closed.
    WindowClosed,
    /// Its turn was stopped, or its tool call ended (before the agent got
    /// the id).
    Cancelled,
    /// The agent of the session that asked disconnected.
    Disconnected,
    Shutdown,
}

/// The prompt a window shows for `r`, answered within `timeout`.
fn prompt_payload(
    r: &Request,
    reason: AskReason,
    can_always: bool,
    timeout: Duration,
) -> serde_json::Value {
    let expires_at = chrono::Utc::now()
        + chrono::Duration::from_std(timeout).unwrap_or_else(|_| chrono::Duration::minutes(5));
    serde_json::json!({
        "requestId": &r.id,
        // Tauri hands a targeted event to every window listening globally
        // too: each window shows only what names it.
        "window": &r.window,
        "command": &r.req.command,
        "cwd": r.cwd.to_string_lossy(),
        "shell": &r.shell,
        "reason": &r.req.reason,
        "agent": &r.agent,
        "sessionId": &r.session_id,
        "toolCallId": &r.tool_call_id,
        "matched": r.visible_origin,
        "background": r.background,
        "plain": !matches!(reason, AskReason::HiddenCharacters),
        "canAlways": can_always,
        "suggestedPrefix": if can_always { rules::suggested_prefix(&r.req.command) } else { None },
        "guard": &r.hits,
        "timeoutSeconds": r.req.timeout_seconds,
        "expiresAt": expires_at.timestamp_millis(),
    })
}

fn notify<R: tauri::Runtime>(app: &tauri::AppHandle<R>, agent: Option<&str>) {
    use tauri_plugin_notification::NotificationExt;
    let title = match agent {
        Some(a) => crate::t!("host_commands.notify.title", "agent" => a),
        None => crate::t!("host_commands.notify.title_unknown"),
    };
    if let Err(e) = app
        .notification()
        .builder()
        .title(title)
        .body(crate::t!("host_commands.notify.body"))
        .show()
    {
        warn!("Host bridge: couldn't show the notification: {e}");
    }
}

fn ask<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    r: &Request,
    reason: AskReason,
    job: &Job,
) -> Asked {
    let Some(host) = app.try_state::<ChildProcesses>() else {
        return Asked::Busy;
    };
    let approvals = &host.host_commands.approvals;
    let timeout = host.host_commands.options().approval_timeout;
    let plain = !matches!(reason, AskReason::HiddenCharacters);
    let can_always = plain
        && r.hits.is_empty()
        && r.visible_origin
        && rules::suggested_prefix(&r.req.command).is_some();
    let payload = prompt_payload(r, reason, can_always, timeout);
    let info = PendingInfo {
        command: r.req.command.clone(),
        can_always,
        plain,
        session_id: r.session_id.clone(),
        tool_call_id: r.tool_call_id.clone(),
    };
    let Some(rx) = approvals.register(&r.id, &r.window, info, payload.clone()) else {
        return Asked::Busy;
    };
    let deadline = Instant::now() + timeout;
    job.set_waiting(deadline);
    // Its tool call ended (or its requester left) while it was being
    // checked: nobody could read the result, so it isn't shown at all.
    if job.is_abandoned() && approvals.withdraw(&r.id) {
        return Asked::Withdrawn;
    }
    emit_to(app, &r.window, crate::events::HOST_COMMAND_PROMPT, &payload);
    if r.window == window_labels::FLOATING {
        if let Some(floating) = app.get_webview_window(window_labels::FLOATING) {
            let _ = crate::overlay_windows::present(&floating);
        }
    }
    // A request from outside Kage's chats, or a chat window that isn't in
    // front: say so where the user will see it.
    let in_front = r.window == window_labels::FLOATING
        || app
            .get_webview_window(&r.window)
            .is_some_and(|w| w.is_focused().unwrap_or(false));
    if !r.visible_origin || !in_front {
        notify(app, r.agent.as_deref());
    }
    loop {
        match rx.recv_timeout(POLL) {
            Ok(Answer::RunOnce) => return Asked::Approved(None),
            Ok(Answer::Always(prefix)) => return Asked::Approved(Some(prefix)),
            Ok(Answer::Deny) => return Asked::Denied,
            Ok(Answer::WindowClosed) => return Asked::WindowClosed,
            Ok(Answer::Cancelled) => {
                // The window says so the way it does for an agent that
                // went away.
                r.close(app, "withdrawn");
                return Asked::Cancelled;
            }
            Ok(Answer::AgentDisconnected) => {
                r.close(app, "disconnected");
                return Asked::Disconnected;
            }
            Ok(Answer::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Asked::Shutdown
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // An answer that raced in is still in the channel when the
                // withdraw finds nothing to remove.
                if Instant::now() >= deadline {
                    if approvals.withdraw(&r.id) {
                        r.close(app, "expired");
                        return Asked::Expired;
                    }
                } else if job.is_abandoned() && approvals.withdraw(&r.id) {
                    r.close(app, "withdrawn");
                    return Asked::Withdrawn;
                }
            }
        }
    }
}

fn run<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    r: &Request,
    approved_by: ApprovedBy,
    rule: Option<String>,
    job: &Arc<Job>,
) -> HostCommandResponse {
    let Some(procs) = app.try_state::<ChildProcesses>() else {
        return HostCommandResponse::error("Kage is starting up; try again.");
    };
    let run_id = new_id("kage-run");
    let Some(slot) = procs
        .user_shell_runs
        .begin_agent(&r.window, &run_id, MAX_AGENT_RUNS)
    else {
        r.close(app, "busy");
        AuditStep {
            outcome: Some("too many commands running".into()),
            ..r.audit("refused")
        }
        .write();
        return HostCommandResponse::error(
            "Too many commands are already running outside the sandbox; try again when one finishes.",
        );
    };
    let cancel = slot.cancel_flag();
    let control = runner::RunControl {
        cancel: cancel.clone(),
        timeout: Duration::from_secs(r.req.timeout_seconds),
    };
    job.set_running(cancel.clone());
    emit_to(
        app,
        &r.window,
        crate::events::HOST_COMMAND_UPDATE,
        &serde_json::json!({
            "requestId": &r.id,
            "window": &r.window,
            "phase": "started",
            "runId": &run_id,
            "approvedBy": approved_by,
            "rule": &rule,
            "command": &r.req.command,
            "cwd": r.cwd.to_string_lossy(),
            "shell": &r.shell,
            "reason": &r.req.reason,
            "agent": &r.agent,
            "sessionId": &r.session_id,
            "toolCallId": &r.tool_call_id,
            "timeoutSeconds": r.req.timeout_seconds,
        }),
    );

    let inv = shell_for(app, &r.req.command);
    let (tx, rx) = mpsc::channel();
    let (thread_app, window, thread_run_id, cwd, thread_job) = (
        app.clone(),
        r.window.clone(),
        run_id.clone(),
        r.cwd.clone(),
        job.clone(),
    );
    let spawned = std::thread::Builder::new()
        .name("host-command-run".into())
        .spawn(move || {
            // Held for the whole run, so shutdown and Stop can find it.
            let _slot = slot;
            let report = runner::run(&inv, &cwd, &control, |text, skipped| {
                thread_job.push_output(text);
                emit_to(
                    &thread_app,
                    &window,
                    crate::events::USER_SHELL_OUTPUT,
                    &serde_json::json!({ "runId": &thread_run_id, "text": text, "skipped": skipped }),
                );
            });
            let _ = tx.send(report.map_err(|e| e.to_string()));
        });
    // A thread that never started dropped its sender (and the slot): the
    // wait below ends at once with "ended without a result".
    if let Err(e) = spawned {
        warn!("Host bridge: couldn't start a run thread: {e}");
    }

    // A version-1 requester going away stops the run, as it always did
    // (its connection was the only way back). A version-2 run doesn't
    // depend on any connection: it ends on its own, by Stop, at its time
    // limit, with its window, or when Kage quits.
    let mut stopping = false;
    let outcome = loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(report) => break report,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !stopping && job.run_should_stop() {
                    info!(
                        "Host command {}: the agent went away; stopping the run",
                        r.id
                    );
                    cancel.store(true, Ordering::SeqCst);
                    stopping = true;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err("the run ended without a result".to_string())
            }
        }
    };

    match outcome {
        Ok(report) => {
            info!(
                "Host command {}: {} (exit code {:?}, signal {:?}) after {} ms",
                r.id,
                report.outcome.as_str(),
                report.exit_code,
                report.signal,
                report.duration_ms
            );
            AuditStep {
                outcome: Some(report.outcome.as_str().into()),
                exit_code: report.exit_code,
                rule: rule.clone(),
                ..r.audit("finished")
            }
            .write();
            crate::telemetry::track(
                app,
                "host_command_run",
                Some(serde_json::json!({ "result": runner::outcome_bucket(&report) })),
            );
            emit_to(
                app,
                &r.window,
                crate::events::HOST_COMMAND_UPDATE,
                &serde_json::json!({ "requestId": &r.id, "window": &r.window, "phase": "finished", "runId": &run_id, "result": &report }),
            );
            let (tail, cut) = host_bridge::output_tail(&report.output, OUTPUT_TAIL_BYTES);
            HostCommandResponse::ran(RunSummary {
                outcome: report.outcome.as_str().into(),
                exit_code: report.exit_code,
                signal: report.signal,
                duration_ms: report.duration_ms,
                output: tail.to_string(),
                truncated: report.truncated || cut,
                output_bytes: report.output_bytes,
                cwd: r.cwd.to_string_lossy().into_owned(),
                approved_by,
                rule,
                timeout_seconds: Some(r.req.timeout_seconds),
            })
        }
        Err(error) => {
            warn!("Host command {}: couldn't run: {error}", r.id);
            AuditStep {
                outcome: Some("failed_to_start".into()),
                ..r.audit("finished")
            }
            .write();
            crate::telemetry::track(
                app,
                "host_command_run",
                Some(serde_json::json!({ "result": "spawn_error" })),
            );
            emit_to(
                app,
                &r.window,
                crate::events::HOST_COMMAND_UPDATE,
                &serde_json::json!({ "requestId": &r.id, "window": &r.window, "phase": "failed", "runId": &run_id, "error": &error }),
            );
            HostCommandResponse::error(format!("The command couldn't start: {error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kage_core::os::host_bridge::{connect, BridgeListener};

    fn endpoint_in(root: &std::path::Path) -> kage_core::os::host_bridge::Endpoint {
        #[cfg(unix)]
        {
            let dir = kage_core::os::host_bridge::bridge_dir_in(root);
            let socket = dir.join(kage_core::os::host_bridge::SOCKET_NAME);
            kage_core::os::host_bridge::Endpoint::Unix { dir, socket }
        }
        #[cfg(windows)]
        {
            let _ = root;
            kage_core::os::host_bridge::Endpoint::Pipe(format!(
                r"\\.\pipe\kage-host-bridge-test-{}",
                uuid::Uuid::new_v4().simple()
            ))
        }
    }

    /// A connection to a listener in a temp dir, from a client that sends
    /// `send` (in pieces `gap` apart) on a thread of its own.
    fn connection(
        root: &std::path::Path,
        pieces: Vec<Vec<u8>>,
        gap: Duration,
    ) -> (BridgeStream, std::thread::JoinHandle<()>, BridgeListener) {
        let ep = endpoint_in(root);
        let listener = BridgeListener::bind(&ep).unwrap();
        let client_ep = ep.clone();
        let client = std::thread::spawn(move || {
            let mut s = connect(&client_ep).unwrap();
            for piece in pieces {
                if s.write_all(&piece).and_then(|()| s.flush()).is_err() {
                    break;
                }
                std::thread::sleep(gap);
            }
        });
        let stop = AtomicBool::new(false);
        let stream = listener.accept(&stop).unwrap().expect("a connection");
        (stream, client, listener)
    }

    #[test]
    fn a_client_that_never_finishes_its_request_is_dropped_in_time() {
        let tmp = tempfile::tempdir().unwrap();
        // A byte every 200 ms, never a newline: under the old per-read
        // timeout this held its connection slot for days.
        let pieces = vec![b"{".to_vec(); 60];
        let (mut stream, client, _l) = connection(tmp.path(), pieces, Duration::from_millis(200));
        let started = Instant::now();
        assert!(matches!(read_request(&mut stream), Incoming::Drop));
        let took = started.elapsed();
        assert!(
            took < READ_TIMEOUT + Duration::from_secs(2),
            "held for {took:?}"
        );
        drop(stream);
        client.join().unwrap();
    }

    #[test]
    fn a_valid_request_is_read_and_a_bad_one_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let req = HostCommandRequest::new("ls", None, "r", 5, None);
        let (mut stream, client, _l) =
            connection(tmp.path(), vec![req.to_line().into_bytes()], Duration::ZERO);
        match read_request(&mut stream) {
            Incoming::Request(got) => assert_eq!(got, BridgeRequest::Run(req)),
            _ => panic!("not read"),
        }
        client.join().unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let (mut stream, client, _l) = connection(
            tmp.path(),
            vec![b"{\"v\":1,\"command\":\"ls\"}\n".to_vec()],
            Duration::ZERO,
        );
        match read_request(&mut stream) {
            Incoming::Refused(r) => assert_eq!(r.status, host_bridge::ResponseStatus::Error),
            Incoming::Request(r) => panic!("read {r:?}"),
            Incoming::Drop => panic!("dropped"),
        }
        client.join().unwrap();
    }
}
