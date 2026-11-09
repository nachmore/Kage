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

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::time::Duration;

/// Wire protocol version. A request with any other version is refused.
pub const PROTOCOL_VERSION: u32 = 1;

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

/// How long the sidecar waits for a reply to a request with this run
/// timeout: the whole approval window, the run itself, and some slack.
pub fn reply_wait(timeout_seconds: u64) -> Duration {
    APPROVAL_TIMEOUT + Duration::from_secs(timeout_seconds.min(MAX_RUN_TIMEOUT_SECS)) + REPLY_SLACK
}

/// One request: run `command` (exactly as written) for the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostCommandRequest {
    /// [`PROTOCOL_VERSION`].
    pub v: u32,
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
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "request larger than {MAX_REQUEST_BYTES} bytes"),
            Self::InvalidUtf8 => write!(f, "request is not valid UTF-8"),
            Self::Malformed(e) => write!(f, "malformed request: {e}"),
            Self::UnsupportedVersion(v) => write!(
                f,
                "unsupported protocol version {v} (this Kage speaks {PROTOCOL_VERSION})"
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
        }
    }
}

impl std::error::Error for RequestError {}

impl HostCommandRequest {
    /// A version-1 request.
    pub fn new(
        command: impl Into<String>,
        cwd: Option<String>,
        reason: impl Into<String>,
        timeout_seconds: u64,
        agent: Option<String>,
    ) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            command: command.into(),
            cwd,
            reason: reason.into(),
            timeout_seconds,
            agent,
        }
    }

    /// Every field within its limits (the version too). Commands and
    /// reasons are never trimmed or changed: what's shown is what runs.
    pub fn validate(&self) -> Result<(), RequestError> {
        if self.v != PROTOCOL_VERSION {
            return Err(RequestError::UnsupportedVersion(self.v));
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

/// Parse one request line (its newline already removed): at most
/// [`MAX_REQUEST_BYTES`], UTF-8, exactly the fields of
/// [`HostCommandRequest`] (an unknown, duplicate or mistyped field is
/// refused), version [`PROTOCOL_VERSION`], and every field within its
/// limits.
pub fn parse_request(bytes: &[u8]) -> Result<HostCommandRequest, RequestError> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(RequestError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| RequestError::InvalidUtf8)?;
    // The version first, so a newer sidecar gets "unsupported version"
    // rather than a confusing field error.
    if let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(text) {
        match obj.get("v").and_then(serde_json::Value::as_u64) {
            Some(v) if v != u64::from(PROTOCOL_VERSION) => {
                return Err(RequestError::UnsupportedVersion(
                    u32::try_from(v).unwrap_or(u32::MAX),
                ));
            }
            _ => {}
        }
    }
    let req: HostCommandRequest =
        serde_json::from_str(text).map_err(|e| RequestError::Malformed(e.to_string()))?;
    req.validate()?;
    Ok(req)
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
    /// Refused or failed before running (invalid request, too many at
    /// once, the shell couldn't start …): see the message.
    Error,
    /// A status this side doesn't know (a newer Kage).
    #[serde(other)]
    Unknown,
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
}

/// Kage's answer to one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostCommandResponse {
    pub v: u32,
    pub status: ResponseStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunSummary>,
    /// English, for the agent: why it was denied or refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl HostCommandResponse {
    pub fn ran(run: RunSummary) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            status: ResponseStatus::Ran,
            run: Some(run),
            message: None,
        }
    }

    pub fn denied(message: impl Into<String>) -> Self {
        Self::status(ResponseStatus::Denied, Some(message.into()))
    }

    pub fn approval_timed_out() -> Self {
        Self::status(ResponseStatus::ApprovalTimedOut, None)
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::status(ResponseStatus::Error, Some(message.into()))
    }

    fn status(status: ResponseStatus, message: Option<String>) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            status,
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

    #[test]
    fn a_valid_request_round_trips() {
        let req = good();
        assert_eq!(parse_request(&line(&req)).unwrap(), req);
    }

    #[test]
    fn optional_fields_may_be_absent() {
        let raw = br#"{"v":1,"command":"ls","reason":"r","timeout_seconds":5}"#;
        let req = parse_request(raw).unwrap();
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
        let raw = br#"{"v":2,"command":"ls","reason":"r","timeout_seconds":5,"new_field":1}"#;
        assert_eq!(parse_request(raw), Err(RequestError::UnsupportedVersion(2)));
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
        let parsed = parse_request(&line(&req)).unwrap();
        assert_eq!(parsed.command, req.command);
    }

    #[test]
    fn responses_round_trip_and_tolerate_new_fields() {
        let resp = HostCommandResponse::ran(RunSummary {
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
        });
        let mut l = resp.to_line().into_bytes();
        l.pop();
        assert_eq!(parse_response(&l).unwrap(), resp);

        let newer = br#"{"v":1,"status":"queued","extra":1,"message":"hi"}"#;
        let parsed = parse_response(newer).unwrap();
        assert_eq!(parsed.status, ResponseStatus::Unknown);
        assert_eq!(parsed.message.as_deref(), Some("hi"));
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
