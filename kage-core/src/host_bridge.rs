//! The host-command bridge: how Kage's MCP sidecar asks the Kage app to run
//! a shell command outside the agent's sandbox (`run_outside_sandbox`).
//!
//! The sidecar is a child of the (sandboxed) agent, so it is sandboxed too;
//! the Kage app, which isn't, runs the command — after the user approves it
//! in Kage's own window, or under a rule they made. This module is the wire
//! protocol both sides share: the limits, the request and its strict
//! parsing, and the reply. The transport (an AF_UNIX socket or a named
//! pipe, authenticated by the OS as the same user) is in
//! [`crate::os::host_bridge`]. See docs/SECURITY_MODEL.md § "Commands an
//! agent runs outside its sandbox".
//!
//! One request per connection: the sidecar writes one JSON object and a
//! newline, Kage answers with one JSON object and a newline, and the
//! connection closes. No secret is minted or exchanged; the OS vouches for
//! who is on the other end.
//!
//! Version 2 (jobs): Kage owns each command as a job with an id, so no
//! single tool call has to last as long as the user's answer plus the run
//! (agents cut MCP calls off: kiro-cli after about two minutes). A `run`
//! request starts a job and is answered when it finishes or, after
//! [`LONG_POLL`], with [`ResponseStatus::Pending`] and the job's id; a
//! `poll` request waits on that job the same way. A poll only reads a
//! job: it can't start, approve or change one. Version 1 (one request,
//! answered only once the command finished) is still served, for a
//! sidecar from an older Kage.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::time::Duration;

/// Wire protocol version this side speaks: jobs (`run` / `poll`).
pub const PROTOCOL_VERSION: u32 = 2;

/// The version an older sidecar speaks: one request, answered only once
/// the command finished. Kage still serves it.
pub const ONE_SHOT_VERSION: u32 = 1;

/// Longest request line accepted, in bytes (newline excluded).
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// Longest command accepted, in UTF-8 bytes. The same limit as "Run it
/// here" (`user_shell::runner::MAX_COMMAND_BYTES`).
pub const MAX_COMMAND_BYTES: usize = 32 * 1024;

/// Longest `reason` accepted, in UTF-8 bytes.
pub const MAX_REASON_BYTES: usize = 2 * 1024;

/// Longest `cwd` accepted, in UTF-8 bytes.
pub const MAX_CWD_BYTES: usize = 4 * 1024;

/// Longest `agent` label accepted, in UTF-8 bytes.
pub const MAX_AGENT_BYTES: usize = 128;

/// Longest reply line the sidecar reads, in bytes. The output tail is at
/// most [`OUTPUT_TAIL_BYTES`], which JSON escaping can grow up to six-fold.
pub const MAX_RESPONSE_BYTES: usize = 512 * 1024;

/// Output handed back to the agent: the last this-many bytes.
pub const OUTPUT_TAIL_BYTES: usize = 32 * 1024;

/// How long a request waits for the user's answer before it is denied.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// A run's time limit when the agent doesn't give one, in seconds.
pub const DEFAULT_RUN_TIMEOUT_SECS: u64 = 600;

/// The longest time limit an agent may ask for, in seconds.
pub const MAX_RUN_TIMEOUT_SECS: u64 = 1800;

/// On top of approval + run time, how long the sidecar keeps waiting for
/// the reply (matching the tool call, starting the shell, draining output).
pub const REPLY_SLACK: Duration = Duration::from_secs(60);

/// How long a version-1 sidecar waits for its reply: the whole approval
/// window, the run itself, and some slack.
pub fn reply_wait(timeout_seconds: u64) -> Duration {
    APPROVAL_TIMEOUT + Duration::from_secs(timeout_seconds.min(MAX_RUN_TIMEOUT_SECS)) + REPLY_SLACK
}

/// How long Kage holds a version-2 `run` or `poll` open for the job to
/// finish before answering [`ResponseStatus::Pending`]: well under the two
/// minutes kiro-cli gives a tool call, with room for the reply. Counted
/// from when the request arrived.
pub const LONG_POLL: Duration = Duration::from_secs(80);

/// How long the sidecar waits for a reply to a version-2 request:
/// [`LONG_POLL`] plus room to read, match and answer it. Still under the
/// agent's two minutes.
pub const LONG_POLL_REPLY_WAIT: Duration = Duration::from_secs(100);

/// How long a finished job's result is kept for the agent to poll.
pub const RESULT_RETENTION: Duration = Duration::from_secs(15 * 60);

/// Longest job id accepted, in bytes.
pub const MAX_JOB_ID_BYTES: usize = 64;

/// A "still running" reply carries at most this many of the output's last
/// lines …
pub const PROGRESS_LINES: usize = 12;

/// … and at most this many bytes of them.
pub const PROGRESS_BYTES: usize = 2 * 1024;

/// What a version-2 request asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// Start a job for a command ([`HostCommandRequest`]).
    Run,
    /// Wait on a job ([`PollRequest`]).
    Poll,
}

/// One request: run `command` (exactly as written) for the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostCommandRequest {
    /// [`PROTOCOL_VERSION`], or [`ONE_SHOT_VERSION`] from an older sidecar.
    pub v: u32,
    /// [`RequestKind::Run`] in version 2; absent in version 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<RequestKind>,
    /// The command line, run by the user's login shell.
    pub command: String,
    /// Where it runs (absolute, or `~`-relative); the home folder if absent.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Why the agent's own shell can't run it, in the agent's words. Shown
    /// to the user; never trusted for anything.
    pub reason: String,
    /// The run's time limit, 1..=[`MAX_RUN_TIMEOUT_SECS`].
    pub timeout_seconds: u64,
    /// The program that spawned the sidecar (the agent), as the sidecar
    /// sees it. Informational: shown to the user when Kage can't match the
    /// request to one of its own sessions.
    #[serde(default)]
    pub agent: Option<String>,
}

/// Why a request was refused before anything was shown or run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    TooLarge,
    InvalidUtf8,
    /// Not the JSON object this module defines (a missing, unknown,
    /// duplicate or mistyped field); the parser's message.
    Malformed(String),
    UnsupportedVersion(u32),
    EmptyCommand,
    CommandTooLong,
    /// A NUL byte in a field (it can't be passed to a program).
    NulByte(&'static str),
    EmptyReason,
    ReasonTooLong,
    CwdTooLong,
    AgentTooLong,
    BadTimeout,
    /// A `kind` the version doesn't have (version 1 has none).
    BadKind,
    /// A job id that's empty, too long, or not `[A-Za-z0-9_-]`.
    BadJobId,
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "request larger than {MAX_REQUEST_BYTES} bytes"),
            Self::InvalidUtf8 => write!(f, "request is not valid UTF-8"),
            Self::Malformed(e) => write!(f, "malformed request: {e}"),
            Self::UnsupportedVersion(v) => write!(
                f,
                "unsupported protocol version {v} (this Kage speaks {ONE_SHOT_VERSION} and {PROTOCOL_VERSION})"
            ),
            Self::EmptyCommand => write!(f, "command is empty"),
            Self::CommandTooLong => {
                write!(f, "command is longer than {MAX_COMMAND_BYTES} bytes")
            }
            Self::NulByte(field) => write!(f, "{field} contains a NUL byte"),
            Self::EmptyReason => write!(f, "reason is required"),
            Self::ReasonTooLong => write!(f, "reason is longer than {MAX_REASON_BYTES} bytes"),
            Self::CwdTooLong => write!(f, "cwd is longer than {MAX_CWD_BYTES} bytes"),
            Self::AgentTooLong => write!(f, "agent is longer than {MAX_AGENT_BYTES} bytes"),
            Self::BadTimeout => write!(
                f,
                "timeout_seconds must be between 1 and {MAX_RUN_TIMEOUT_SECS}"
            ),
            Self::BadKind => write!(f, "unknown request kind for this protocol version"),
            Self::BadJobId => write!(
                f,
                "job_id must be 1 to {MAX_JOB_ID_BYTES} letters, digits, '-' or '_'"
            ),
        }
    }
}

impl std::error::Error for RequestError {}

impl HostCommandRequest {
    /// A version-2 `run` request.
    pub fn new(
        command: impl Into<String>,
        cwd: Option<String>,
        reason: impl Into<String>,
        timeout_seconds: u64,
        agent: Option<String>,
    ) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            kind: Some(RequestKind::Run),
            command: command.into(),
            cwd,
            reason: reason.into(),
            timeout_seconds,
            agent,
        }
    }

    /// The same request as an older sidecar sends it (version 1).
    pub fn one_shot(mut self) -> Self {
        self.v = ONE_SHOT_VERSION;
        self.kind = None;
        self
    }

    /// Every field within its limits (the version and kind too). Commands
    /// and reasons are never trimmed or changed: what's shown is what runs.
    pub fn validate(&self) -> Result<(), RequestError> {
        match (self.v, self.kind) {
            (ONE_SHOT_VERSION, None) | (PROTOCOL_VERSION, Some(RequestKind::Run)) => {}
            (ONE_SHOT_VERSION | PROTOCOL_VERSION, _) => return Err(RequestError::BadKind),
            (v, _) => return Err(RequestError::UnsupportedVersion(v)),
        }
        if self.command.trim().is_empty() {
            return Err(RequestError::EmptyCommand);
        }
        if self.command.len() > MAX_COMMAND_BYTES {
            return Err(RequestError::CommandTooLong);
        }
        if self.command.contains('\0') {
            return Err(RequestError::NulByte("command"));
        }
        if self.reason.trim().is_empty() {
            return Err(RequestError::EmptyReason);
        }
        if self.reason.len() > MAX_REASON_BYTES {
            return Err(RequestError::ReasonTooLong);
        }
        if self.reason.contains('\0') {
            return Err(RequestError::NulByte("reason"));
        }
        if let Some(cwd) = &self.cwd {
            if cwd.len() > MAX_CWD_BYTES {
                return Err(RequestError::CwdTooLong);
            }
            if cwd.contains('\0') {
                return Err(RequestError::NulByte("cwd"));
            }
        }
        if let Some(agent) = &self.agent {
            if agent.len() > MAX_AGENT_BYTES {
                return Err(RequestError::AgentTooLong);
            }
            if agent.contains('\0') {
                return Err(RequestError::NulByte("agent"));
            }
        }
        if !(1..=MAX_RUN_TIMEOUT_SECS).contains(&self.timeout_seconds) {
            return Err(RequestError::BadTimeout);
        }
        Ok(())
    }

    /// The request as one line for the wire (JSON never holds a raw
    /// newline, so the line ends where the object does).
    pub fn to_line(&self) -> String {
        let mut s = serde_json::to_string(self).unwrap_or_default();
        s.push('\n');
        s
    }
}

/// A version-2 request to wait on job `job_id`. It can only read the job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PollRequest {
    /// [`PROTOCOL_VERSION`].
    pub v: u32,
    /// [`RequestKind::Poll`].
    pub kind: RequestKind,
    pub job_id: String,
}

impl PollRequest {
    pub fn new(job_id: impl Into<String>) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            kind: RequestKind::Poll,
            job_id: job_id.into(),
        }
    }

    pub fn validate(&self) -> Result<(), RequestError> {
        if self.v == ONE_SHOT_VERSION {
            // Version 1 has no jobs to poll.
            return Err(RequestError::BadKind);
        }
        if self.v != PROTOCOL_VERSION {
            return Err(RequestError::UnsupportedVersion(self.v));
        }
        if self.kind != RequestKind::Poll {
            return Err(RequestError::BadKind);
        }
        if !is_job_id(&self.job_id) {
            return Err(RequestError::BadJobId);
        }
        Ok(())
    }
}

/// Whether `id` can be a job id: 1 to [`MAX_JOB_ID_BYTES`] ASCII letters,
/// digits, `-` or `_`.
pub fn is_job_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_JOB_ID_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// What one connection asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeRequest {
    /// Version 1: run it, and answer only once it finished.
    OneShot(HostCommandRequest),
    /// Version 2: start a job for it; answered when it finishes or, after
    /// [`LONG_POLL`], with "pending" and the job's id.
    Run(HostCommandRequest),
    /// Version 2: wait on a job, the same way.
    Poll(PollRequest),
}

impl BridgeRequest {
    /// The request as one line for the wire.
    pub fn to_line(&self) -> String {
        match self {
            Self::OneShot(r) | Self::Run(r) => r.to_line(),
            Self::Poll(p) => {
                let mut s = serde_json::to_string(p).unwrap_or_default();
                s.push('\n');
                s
            }
        }
    }

    /// The protocol version it speaks (its reply speaks the same).
    pub fn version(&self) -> u32 {
        match self {
            Self::OneShot(_) => ONE_SHOT_VERSION,
            Self::Run(_) | Self::Poll(_) => PROTOCOL_VERSION,
        }
    }
}

/// Parse one request line (its newline already removed): at most
/// [`MAX_REQUEST_BYTES`], UTF-8, exactly the fields of the request its
/// version and kind name ([`HostCommandRequest`] or [`PollRequest`]: an
/// unknown, duplicate or mistyped field is refused), version
/// [`ONE_SHOT_VERSION`] or [`PROTOCOL_VERSION`], and every field within its
/// limits.
pub fn parse_request(bytes: &[u8]) -> Result<BridgeRequest, RequestError> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(RequestError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| RequestError::InvalidUtf8)?;
    // The version first, so a newer sidecar gets "unsupported version"
    // rather than a confusing field error; then which request it is. The
    // request itself is parsed from the text, so a duplicate field (which
    // a `Value` quietly collapses) is still refused.
    let mut poll = false;
    if let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(text) {
        match obj.get("v").and_then(serde_json::Value::as_u64) {
            Some(v) if v != u64::from(ONE_SHOT_VERSION) && v != u64::from(PROTOCOL_VERSION) => {
                return Err(RequestError::UnsupportedVersion(
                    u32::try_from(v).unwrap_or(u32::MAX),
                ));
            }
            _ => {}
        }
        poll = obj.get("kind").and_then(serde_json::Value::as_str) == Some("poll");
    }
    if poll {
        let req: PollRequest =
            serde_json::from_str(text).map_err(|e| RequestError::Malformed(e.to_string()))?;
        req.validate()?;
        return Ok(BridgeRequest::Poll(req));
    }
    let req: HostCommandRequest =
        serde_json::from_str(text).map_err(|e| RequestError::Malformed(e.to_string()))?;
    req.validate()?;
    Ok(if req.v == ONE_SHOT_VERSION {
        BridgeRequest::OneShot(req)
    } else {
        BridgeRequest::Run(req)
    })
}

/// What became of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    /// It ran (to whatever end: see [`RunSummary::outcome`]).
    Ran,
    /// The user said no.
    Denied,
    /// Nobody answered within [`APPROVAL_TIMEOUT`].
    ApprovalTimedOut,
    /// Withdrawn before the user answered (the turn was stopped, the agent
    /// disconnected, its window closed …), so nothing ran: see the message.
    /// Version 2; a version-1 reply says [`Self::Error`].
    Withdrawn,
    /// Version 2: not finished yet ([`HostCommandResponse::job`] says
    /// where it is). Poll the job for the rest.
    Pending,
    /// Version 2: no such job (its result expired, or Kage restarted).
    NotFound,
    /// Refused or failed before running (invalid request, too many at
    /// once, the shell couldn't start …): see the message.
    Error,
    /// A status this side doesn't know (a newer Kage).
    #[serde(other)]
    Unknown,
}

impl ResponseStatus {
    /// Whether the job is over (anything but [`Self::Pending`]).
    pub fn is_final(self) -> bool {
        self != Self::Pending
    }
}

/// Where an unfinished job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Kage is waiting for the user's answer.
    WaitingForApproval,
    /// The command is running.
    Running,
    #[serde(other)]
    Unknown,
}

/// How far an unfinished job got, for a "pending" reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobProgress {
    pub state: JobState,
    /// Since Kage got the request.
    pub elapsed_ms: u64,
    /// Running: since the command started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_ms: Option<u64>,
    /// Running: the output's last lines so far ([`progress_lines`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tail: Option<String>,
    /// Waiting: how long until Kage stops waiting for the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_expires_in_ms: Option<u64>,
}

/// Who let a command run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovedBy {
    /// The user, in Kage's prompt.
    User,
    /// A rule the user made ("Always allow commands starting with …").
    Rule,
    /// The "run every command without asking" switch.
    AllowAll,
    #[serde(other)]
    Unknown,
}

/// How a run went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    /// `exited`, `cancelled` (the user pressed Stop) or `timed_out`.
    pub outcome: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub signal: Option<i32>,
    pub duration_ms: u64,
    /// The last [`OUTPUT_TAIL_BYTES`] of stdout and stderr, interleaved.
    pub output: String,
    /// Whether earlier output was dropped.
    pub truncated: bool,
    /// Everything the command printed, in bytes.
    pub output_bytes: u64,
    /// The directory it ran in.
    pub cwd: String,
    pub approved_by: ApprovedBy,
    /// The rule's prefix, when a rule let it run.
    #[serde(default)]
    pub rule: Option<String>,
    /// The run's time limit, in seconds (version 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

/// Kage's answer to one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostCommandResponse {
    pub v: u32,
    pub status: ResponseStatus,
    /// Version 2: the job this is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// [`ResponseStatus::Pending`]: how far the job got.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<JobProgress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunSummary>,
    /// English, for the agent: why it was denied or refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl HostCommandResponse {
    pub fn ran(run: RunSummary) -> Self {
        Self {
            run: Some(run),
            ..Self::status(ResponseStatus::Ran, None)
        }
    }

    pub fn denied(message: impl Into<String>) -> Self {
        Self::status(ResponseStatus::Denied, Some(message.into()))
    }

    pub fn approval_timed_out() -> Self {
        Self::status(ResponseStatus::ApprovalTimedOut, None)
    }

    pub fn withdrawn(message: impl Into<String>) -> Self {
        Self::status(ResponseStatus::Withdrawn, Some(message.into()))
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::status(ResponseStatus::Error, Some(message.into()))
    }

    /// Job `job_id` isn't finished: this is how far it got.
    pub fn pending(job_id: impl Into<String>, progress: JobProgress) -> Self {
        Self {
            job_id: Some(job_id.into()),
            job: Some(progress),
            ..Self::status(ResponseStatus::Pending, None)
        }
    }

    /// There's no job `job_id`.
    pub fn not_found(job_id: impl Into<String>) -> Self {
        Self {
            job_id: Some(job_id.into()),
            ..Self::status(
                ResponseStatus::NotFound,
                Some(format!(
                    "Kage has no such job: its result is kept for {} minutes after the command \
                     ends, and none survives a restart of Kage.",
                    RESULT_RETENTION.as_secs() / 60
                )),
            )
        }
    }

    /// The same answer, naming job `job_id`.
    pub fn for_job(mut self, job_id: impl Into<String>) -> Self {
        self.job_id = Some(job_id.into());
        self
    }

    /// The answer as a client speaking `version` reads it: version 1 has
    /// no jobs, and no "withdrawn" (an error with the same message).
    pub fn for_version(mut self, version: u32) -> Self {
        if version != ONE_SHOT_VERSION {
            return self;
        }
        self.v = ONE_SHOT_VERSION;
        self.job_id = None;
        self.job = None;
        if let Some(run) = self.run.as_mut() {
            run.timeout_seconds = None;
        }
        match self.status {
            ResponseStatus::Withdrawn | ResponseStatus::NotFound => {
                self.status = ResponseStatus::Error;
            }
            ResponseStatus::Pending => {
                self.status = ResponseStatus::Error;
                self.message = Some("The command hasn't finished.".into());
            }
            _ => {}
        }
        self
    }

    fn status(status: ResponseStatus, message: Option<String>) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            status,
            job_id: None,
            job: None,
            run: None,
            message,
        }
    }

    pub fn to_line(&self) -> String {
        let mut s = serde_json::to_string(self).unwrap_or_default();
        s.push('\n');
        s
    }
}

/// Parse a reply line (lenient about fields a newer Kage adds).
pub fn parse_response(bytes: &[u8]) -> Result<HostCommandResponse, String> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err("reply too large".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "reply is not valid UTF-8".to_string())?;
    serde_json::from_str(text).map_err(|e| format!("malformed reply: {e}"))
}

/// The last `cap` bytes of `text`, cut on a character boundary, and
/// whether anything was cut.
pub fn output_tail(text: &str, cap: usize) -> (&str, bool) {
    if text.len() <= cap {
        return (text, false);
    }
    let mut cut = text.len() - cap;
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    (&text[cut..], true)
}

/// Terminal escape sequences out of `line`: CSI (`ESC [ … final`), OSC
/// (`ESC ] … BEL` / `ESC \`), and any other `ESC x`; other control
/// characters but tab go too.
fn strip_escapes(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if !c.is_control() || c == '\t' {
                out.push(c);
            }
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{07}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The last lines of a command's output as a terminal would show them,
/// for a "still running" reply: at most [`PROGRESS_LINES`] lines and
/// [`PROGRESS_BYTES`] bytes, a line overwritten with `\r` (a progress
/// bar) as its last state, escape sequences and control characters
/// removed, blank lines at the end dropped.
pub fn progress_lines(output: &str) -> String {
    let mut lines: Vec<String> = output
        .split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            strip_escapes(line.rsplit('\r').next().unwrap_or(line))
        })
        .collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let start = lines.len().saturating_sub(PROGRESS_LINES);
    let text = lines[start..].join("\n");
    output_tail(&text, PROGRESS_BYTES).0.to_string()
}

/// How a bounded line read ended.
#[derive(Debug, PartialEq, Eq)]
pub enum LineRead {
    /// A line (newline removed), or the bytes before EOF.
    Line(Vec<u8>),
    /// More than the cap arrived before a newline.
    TooLong,
    /// The other end closed without sending anything.
    Eof,
}

/// Read one line of at most `max` bytes (newline excluded) from `r`,
/// never buffering more than `max + 1` bytes.
pub fn read_line_bounded<R: Read>(r: R, max: usize) -> std::io::Result<LineRead> {
    let mut reader = BufReader::new(r.take(max as u64 + 1));
    let mut buf = Vec::new();
    let n = reader.read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(LineRead::Eof);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        return Ok(LineRead::Line(buf));
    }
    if buf.len() > max {
        return Ok(LineRead::TooLong);
    }
    Ok(LineRead::Line(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> HostCommandRequest {
        HostCommandRequest::new(
            "brew install awscli",
            Some("/tmp".into()),
            "Homebrew can't nest its sandbox inside mine",
            600,
            Some("kiro-cli".into()),
        )
    }

    fn line(req: &HostCommandRequest) -> Vec<u8> {
        let mut l = req.to_line().into_bytes();
        assert_eq!(l.pop(), Some(b'\n'));
        l
    }

    fn poll_line(job_id: &str) -> Vec<u8> {
        let mut l = BridgeRequest::Poll(PollRequest::new(job_id))
            .to_line()
            .into_bytes();
        assert_eq!(l.pop(), Some(b'\n'));
        l
    }

    #[test]
    fn a_valid_request_round_trips() {
        let req = good();
        assert_eq!(
            parse_request(&line(&req)).unwrap(),
            BridgeRequest::Run(req.clone())
        );
        assert_eq!(
            String::from_utf8(line(&req)).unwrap(),
            BridgeRequest::Run(req).to_line().trim_end()
        );
        assert_eq!(
            parse_request(&poll_line("hc-0123abcd")).unwrap(),
            BridgeRequest::Poll(PollRequest::new("hc-0123abcd"))
        );
    }

    #[test]
    fn an_older_sidecars_one_shot_request_is_still_served() {
        let req = good().one_shot();
        let raw = line(&req);
        assert!(!String::from_utf8_lossy(&raw).contains("kind"));
        let parsed = parse_request(&raw).unwrap();
        assert_eq!(parsed, BridgeRequest::OneShot(req));
        assert_eq!(parsed.version(), ONE_SHOT_VERSION);

        // What a version-1 sidecar sends, byte for byte.
        let raw = br#"{"v":1,"command":"ls","reason":"r","timeout_seconds":5}"#;
        assert!(matches!(
            parse_request(raw),
            Ok(BridgeRequest::OneShot(HostCommandRequest {
                kind: None,
                ..
            }))
        ));
        // Version 1 has no kinds, and no polls.
        for raw in [
            &br#"{"v":1,"kind":"run","command":"ls","reason":"r","timeout_seconds":5}"#[..],
            br#"{"v":1,"kind":"poll","job_id":"hc-1"}"#,
        ] {
            assert_eq!(
                parse_request(raw),
                Err(RequestError::BadKind),
                "{}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    #[test]
    fn a_version_2_run_must_say_so() {
        let raw = br#"{"v":2,"command":"ls","reason":"r","timeout_seconds":5}"#;
        assert_eq!(parse_request(raw), Err(RequestError::BadKind));
        let raw = br#"{"v":2,"kind":"cancel","command":"ls","reason":"r","timeout_seconds":5}"#;
        assert!(matches!(
            parse_request(raw),
            Err(RequestError::Malformed(_))
        ));
    }

    #[test]
    fn a_poll_carries_only_a_job_id() {
        for raw in [
            // Anything that would start, approve or change a job.
            &br#"{"v":2,"kind":"poll","job_id":"hc-1","command":"rm -rf ~"}"#[..],
            br#"{"v":2,"kind":"poll","job_id":"hc-1","decision":"run_once"}"#,
            br#"{"v":2,"kind":"poll","job_id":"hc-1","kind":"run"}"#,
            br#"{"v":2,"kind":"poll"}"#,
            br#"{"v":2,"kind":"poll","job_id":7}"#,
        ] {
            assert!(
                matches!(parse_request(raw), Err(RequestError::Malformed(_))),
                "{}",
                String::from_utf8_lossy(raw)
            );
        }
        for id in ["", "hc 1", "hc/../1", "hc-1\n", "hç-1"] {
            assert_eq!(PollRequest::new(id).validate(), Err(RequestError::BadJobId));
        }
        let long = "a".repeat(MAX_JOB_ID_BYTES + 1);
        assert_eq!(
            parse_request(&poll_line(&long)),
            Err(RequestError::BadJobId)
        );
        assert!(is_job_id(&"a".repeat(MAX_JOB_ID_BYTES)));
    }

    #[test]
    fn optional_fields_may_be_absent() {
        let raw = br#"{"v":2,"kind":"run","command":"ls","reason":"r","timeout_seconds":5}"#;
        let Ok(BridgeRequest::Run(req)) = parse_request(raw) else {
            panic!("not a run");
        };
        assert_eq!(req.cwd, None);
        assert_eq!(req.agent, None);
    }

    #[test]
    fn unknown_duplicate_or_mistyped_fields_are_refused() {
        for raw in [
            &br#"{"v":1,"command":"ls","reason":"r","timeout_seconds":5,"run_now":true}"#[..],
            br#"{"v":1,"command":"ls","command":"rm -rf ~","reason":"r","timeout_seconds":5}"#,
            br#"{"v":1,"command":["ls"],"reason":"r","timeout_seconds":5}"#,
            br#"{"v":1,"command":"ls","reason":"r","timeout_seconds":5.5}"#,
            br#"{"v":1,"command":"ls","reason":"r","timeout_seconds":-1}"#,
            br#"{"v":1,"command":"ls","timeout_seconds":5}"#,
            br#"{"v":2,"kind":"run","command":"ls","reason":"r","timeout_seconds":5,"job_id":"x"}"#,
            br#"{"v":2,"kind":"run","kind":"run","command":"ls","reason":"r","timeout_seconds":5}"#,
            br#"{"v":2,"kind":"run","command":"ls","command":"rm -rf ~","reason":"r","timeout_seconds":5}"#,
            br#"["ls"]"#,
            b"not json",
        ] {
            assert!(
                matches!(parse_request(raw), Err(RequestError::Malformed(_))),
                "{}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    #[test]
    fn other_versions_are_refused_by_name() {
        let raw = br#"{"v":3,"command":"ls","reason":"r","timeout_seconds":5,"new_field":1}"#;
        assert_eq!(parse_request(raw), Err(RequestError::UnsupportedVersion(3)));
        let raw = br#"{"v":3,"kind":"poll","job_id":"hc-1"}"#;
        assert_eq!(parse_request(raw), Err(RequestError::UnsupportedVersion(3)));
        let mut req = good();
        req.v = 0;
        assert_eq!(
            parse_request(&line(&req)),
            Err(RequestError::UnsupportedVersion(0))
        );
    }

    #[test]
    fn limits_are_enforced() {
        let mut req = good();
        req.command = "x".repeat(MAX_COMMAND_BYTES);
        assert!(req.validate().is_ok());
        req.command.push('x');
        assert_eq!(req.validate(), Err(RequestError::CommandTooLong));

        let mut req = good();
        req.command = "  \n\t".into();
        assert_eq!(req.validate(), Err(RequestError::EmptyCommand));

        let mut req = good();
        req.command = "ls\0rm".into();
        assert_eq!(req.validate(), Err(RequestError::NulByte("command")));

        let mut req = good();
        req.reason = " ".into();
        assert_eq!(req.validate(), Err(RequestError::EmptyReason));
        req.reason = "r".repeat(MAX_REASON_BYTES + 1);
        assert_eq!(req.validate(), Err(RequestError::ReasonTooLong));

        let mut req = good();
        req.cwd = Some("/".repeat(MAX_CWD_BYTES + 1));
        assert_eq!(req.validate(), Err(RequestError::CwdTooLong));

        let mut req = good();
        req.agent = Some("a".repeat(MAX_AGENT_BYTES + 1));
        assert_eq!(req.validate(), Err(RequestError::AgentTooLong));

        for t in [0, MAX_RUN_TIMEOUT_SECS + 1] {
            let mut req = good();
            req.timeout_seconds = t;
            assert_eq!(req.validate(), Err(RequestError::BadTimeout), "{t}");
        }
        let mut req = good();
        req.timeout_seconds = MAX_RUN_TIMEOUT_SECS;
        assert!(req.validate().is_ok());
    }

    #[test]
    fn oversized_and_non_utf8_requests_are_refused() {
        assert_eq!(
            parse_request(&vec![b' '; MAX_REQUEST_BYTES + 1]),
            Err(RequestError::TooLarge)
        );
        assert_eq!(parse_request(b"\xff\xfe"), Err(RequestError::InvalidUtf8));
    }

    #[test]
    fn the_command_is_never_changed() {
        let mut req = good();
        req.command = "  echo 'two  spaces'  \n".into();
        let Ok(BridgeRequest::Run(parsed)) = parse_request(&line(&req)) else {
            panic!("not a run");
        };
        assert_eq!(parsed.command, req.command);
    }

    fn summary() -> RunSummary {
        RunSummary {
            outcome: "exited".into(),
            exit_code: Some(0),
            signal: None,
            duration_ms: 12,
            output: "ok\n".into(),
            truncated: false,
            output_bytes: 3,
            cwd: "/tmp".into(),
            approved_by: ApprovedBy::Rule,
            rule: Some("brew install".into()),
            timeout_seconds: Some(600),
        }
    }

    fn round_trip(resp: &HostCommandResponse) -> HostCommandResponse {
        let mut l = resp.to_line().into_bytes();
        assert_eq!(l.pop(), Some(b'\n'));
        parse_response(&l).unwrap()
    }

    #[test]
    fn responses_round_trip_and_tolerate_new_fields() {
        let resp = HostCommandResponse::ran(summary()).for_job("hc-1");
        assert_eq!(round_trip(&resp), resp);

        let pending = HostCommandResponse::pending(
            "hc-2",
            JobProgress {
                state: JobState::Running,
                elapsed_ms: 81_000,
                running_ms: Some(70_000),
                output_tail: Some("==> Downloading".into()),
                approval_expires_in_ms: None,
            },
        );
        assert_eq!(round_trip(&pending), pending);
        assert!(!pending.status.is_final());
        assert!(HostCommandResponse::withdrawn("x").status.is_final());

        let newer = br#"{"v":2,"status":"queued","extra":1,"message":"hi","job":{"state":"paused","elapsed_ms":1}}"#;
        let parsed = parse_response(newer).unwrap();
        assert_eq!(parsed.status, ResponseStatus::Unknown);
        assert_eq!(parsed.message.as_deref(), Some("hi"));
        assert_eq!(parsed.job.map(|j| j.state), Some(JobState::Unknown));
    }

    #[test]
    fn a_one_shot_reply_reads_as_version_1_did() {
        // What a version-1 sidecar parses: no job fields, no new statuses.
        let ran = HostCommandResponse::ran(summary())
            .for_job("hc-1")
            .for_version(ONE_SHOT_VERSION);
        let text = ran.to_line();
        assert!(!text.contains("job"), "{text}");
        assert!(!text.contains("timeout_seconds"), "{text}");
        assert_eq!(ran.v, ONE_SHOT_VERSION);

        let withdrawn = HostCommandResponse::withdrawn("The turn was stopped.")
            .for_job("hc-2")
            .for_version(ONE_SHOT_VERSION);
        assert_eq!(withdrawn.status, ResponseStatus::Error);
        assert_eq!(withdrawn.message.as_deref(), Some("The turn was stopped."));

        // Version 2 keeps everything.
        let kept = HostCommandResponse::withdrawn("w").for_job("hc-3");
        assert_eq!(kept.clone().for_version(PROTOCOL_VERSION), kept);
    }

    #[test]
    fn progress_shows_the_last_lines_as_a_terminal_would() {
        let out = "one\ntwo\r\n\u{1b}[1;32m==> Pouring\u{1b}[0m jq\n 10%\r 55%\r100%\n\n\n";
        assert_eq!(progress_lines(out), "one\ntwo\n==> Pouring jq\n100%");
        // An OSC title and a bell are dropped too.
        assert_eq!(progress_lines("\u{1b}]0;title\u{07}done\u{07}\n"), "done");

        let many: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let shown = progress_lines(&many);
        assert_eq!(shown.lines().count(), PROGRESS_LINES);
        assert!(shown.ends_with("line 99"));

        let wide = format!("{}\n", "日".repeat(PROGRESS_BYTES));
        assert!(progress_lines(&wide).len() <= PROGRESS_BYTES);
        assert_eq!(progress_lines(""), "");
    }

    #[test]
    fn the_long_poll_fits_inside_an_agents_tool_call() {
        // kiro-cli gives an MCP call about two minutes.
        assert!(LONG_POLL < LONG_POLL_REPLY_WAIT);
        assert!(LONG_POLL_REPLY_WAIT < Duration::from_secs(115));
        assert!(RESULT_RETENTION > APPROVAL_TIMEOUT);
    }

    #[test]
    fn output_tail_cuts_on_a_character_boundary() {
        assert_eq!(output_tail("abc", 8), ("abc", false));
        let (tail, cut) = output_tail("ab日本", 4);
        assert!(cut);
        assert_eq!(tail, "本");
        assert!(tail.len() <= 4);
    }

    #[test]
    fn bounded_lines() {
        assert_eq!(
            read_line_bounded(&b"abc\nrest"[..], 8).unwrap(),
            LineRead::Line(b"abc".to_vec())
        );
        assert_eq!(
            read_line_bounded(&b"abc"[..], 8).unwrap(),
            LineRead::Line(b"abc".to_vec())
        );
        assert_eq!(
            read_line_bounded(&b"12345678\n"[..], 8).unwrap(),
            LineRead::Line(b"12345678".to_vec())
        );
        assert_eq!(
            read_line_bounded(&b"123456789\n"[..], 8).unwrap(),
            LineRead::TooLong
        );
        assert_eq!(read_line_bounded(&b""[..], 8).unwrap(), LineRead::Eof);
    }

    #[test]
    fn the_reply_wait_covers_approval_and_the_run() {
        assert!(reply_wait(600) > APPROVAL_TIMEOUT + Duration::from_secs(600));
        assert_eq!(reply_wait(u64::MAX), reply_wait(MAX_RUN_TIMEOUT_SECS));
    }
}
