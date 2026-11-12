//! `run_outside_sandbox`: ask the Kage app to run a shell command that the
//! agent's own sandbox refused (Homebrew can't nest its sandbox inside the
//! agent's, …); and `get_outside_sandbox_result`, which waits for one that
//! wasn't finished when the first call answered.
//!
//! This sidecar is a child of the sandboxed agent, so it can't run the
//! command itself: it sends the request over the host-command bridge
//! (`kage_core::host_bridge`, a socket / named pipe only this user can
//! reach) and waits. Kage shows the user the exact command, where it runs
//! and the agent's reason, and runs it only once they approve — or a rule
//! they made covers it — then replies with the exit code and the output's
//! tail, which goes back to the agent as this tool's result.
//!
//! Kage owns the request as a job, and no call waits on it for longer than
//! the bridge's long poll (80 s): agents cut off a tool call that takes
//! too long (kiro-cli after about two minutes), and the user's answer plus
//! a `brew install` easily does. A call that ends first returns "pending"
//! with the job's id, and the agent calls `get_outside_sandbox_result`
//! with it until the result is there.

use kage_core::host_bridge::{
    self, BridgeRequest, HostCommandRequest, HostCommandResponse, JobProgress, JobState, LineRead,
    PollRequest, ResponseStatus, DEFAULT_RUN_TIMEOUT_SECS, MAX_RUN_TIMEOUT_SECS, OUTPUT_TAIL_BYTES,
};
use kage_core::mcp_json_rpc::tool_result_text;
use std::io::Write;

pub(crate) const TOOL_NAME: &str = "run_outside_sandbox";

/// The tool that waits on a job `run_outside_sandbox` answered "pending"
/// for. It only reads the job.
pub(crate) const RESULT_TOOL_NAME: &str = "get_outside_sandbox_result";

pub(crate) const DESCRIPTION: &str = "Run a shell command on the user's computer OUTSIDE your sandbox, through the Kage app. \
Use this ONLY after your own shell tool already failed because your sandbox blocked the command \
(for example `brew install` failing with \"sandbox-exec: sandbox_apply: Operation not permitted\"). \
For everything else keep using your own shell tool. \
Never use it to read, print or copy credentials or secrets (AWS or SSH keys, tokens, the keychain, environment dumps). \
The user sees the exact command, its working directory and your reason, and approves every run unless they made a rule for it. \
If they deny it, don't retry the same command unless they ask you to. \
It runs in the user's login shell with no input (nothing interactive; sudo can't ask for a password). \
Returns the exit code, the last 32 KiB of output and how long it took. \
This call waits at most about 80 seconds: if the user hasn't answered yet, or the command is still running, it returns \
status \"pending\" with a job_id instead. Then call get_outside_sandbox_result with that job_id, again each time it says \
\"pending\", until it returns the final result. Never call run_outside_sandbox again for the same command to retry or \
to check on it: that asks the user a second time and may run it twice.";

pub(crate) const RESULT_DESCRIPTION: &str = "Wait for the result of a command you asked run_outside_sandbox to run, when \
it returned status \"pending\" with a job_id (the user hadn't answered Kage's prompt yet, or the command was still running). \
Waits at most about 80 seconds: if it says \"pending\" again, call it again with the same job_id until you get the final \
result (exit code and output, or denied, timed out waiting for approval, or withdrawn). It only reads the job: it never \
starts, approves or changes a command. Kage keeps a finished result for 15 minutes.";

pub(crate) fn input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "The exact command line to run (the user's login shell on macOS/Linux, PowerShell on Windows). Shown to the user verbatim and run exactly as written."
            },
            "cwd": {
                "type": "string",
                "description": "Absolute directory to run it in (a leading ~ means the home folder). Defaults to the user's home folder."
            },
            "reason": {
                "type": "string",
                "description": "Why your own shell can't run this command: quote the sandbox error you got. Shown to the user."
            },
            "timeout_seconds": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_RUN_TIMEOUT_SECS,
                "default": DEFAULT_RUN_TIMEOUT_SECS,
                "description": "Stop the command after this many seconds (default 600, at most 1800). Waiting for the user's approval doesn't count."
            }
        },
        "required": ["command", "reason"]
    })
}

pub(crate) fn result_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "job_id": {
                "type": "string",
                "maxLength": host_bridge::MAX_JOB_ID_BYTES,
                "description": "The job_id a \"pending\" result of run_outside_sandbox (or of this tool) gave you, exactly as given."
            }
        },
        "required": ["job_id"]
    })
}

/// Why the bridge produced no reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BridgeError {
    /// Nothing listens: Kage isn't running (or its bridge didn't start).
    NotRunning,
    /// Something is there but we couldn't use it (the agent's sandbox may
    /// block the connection; the server isn't this user).
    Unreachable(String),
    /// No reply in time.
    TimedOut,
    /// A reply we couldn't read.
    Protocol(String),
}

/// How the tool reaches Kage. A trait so tests can stand in a fake.
pub(crate) trait Bridge {
    fn send(&self, request: &BridgeRequest) -> Result<HostCommandResponse, BridgeError>;
}

/// The real bridge: the endpoint Kage computes the same way.
pub(crate) struct OsBridge;

impl Bridge for OsBridge {
    fn send(&self, request: &BridgeRequest) -> Result<HostCommandResponse, BridgeError> {
        use kage_core::os::host_bridge as transport;
        let endpoint = transport::default_endpoint().ok_or(BridgeError::NotRunning)?;
        let mut stream = transport::connect(&endpoint).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                BridgeError::NotRunning
            }
            _ => BridgeError::Unreachable(e.to_string()),
        })?;
        let wait = match request {
            BridgeRequest::OneShot(r) => host_bridge::reply_wait(r.timeout_seconds),
            BridgeRequest::Run(_) | BridgeRequest::Poll(_) => host_bridge::LONG_POLL_REPLY_WAIT,
        };
        stream
            .set_read_timeout(Some(wait))
            .map_err(|e| BridgeError::Unreachable(e.to_string()))?;
        stream
            .write_all(request.to_line().as_bytes())
            .and_then(|()| stream.flush())
            .map_err(|e| BridgeError::Unreachable(e.to_string()))?;
        match host_bridge::read_line_bounded(&mut stream, host_bridge::MAX_RESPONSE_BYTES) {
            Ok(LineRead::Line(line)) => {
                host_bridge::parse_response(&line).map_err(BridgeError::Protocol)
            }
            Ok(LineRead::TooLong) => Err(BridgeError::Protocol("reply too large".into())),
            Ok(LineRead::Eof) => Err(BridgeError::Protocol(
                "Kage closed the connection without answering".into(),
            )),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Err(BridgeError::TimedOut)
            }
            Err(e) => Err(BridgeError::Protocol(e.to_string())),
        }
    }
}

/// The agent's arguments as a bridge request. `command` and `reason` are
/// required strings; `cwd` an optional string; `timeout_seconds` an
/// optional integer in 1..=1800. Other keys are ignored.
pub(crate) fn parse_args(args: &serde_json::Value) -> Result<HostCommandRequest, String> {
    let Some(obj) = args.as_object() else {
        return Err("Arguments must be an object with `command` and `reason`.".into());
    };
    let string = |key: &str| -> Result<Option<String>, String> {
        match obj.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(format!("`{key}` must be a string.")),
        }
    };
    let command = string("command")?
        .filter(|c| !c.trim().is_empty())
        .ok_or("Missing required parameter: command")?;
    let reason = string("reason")?
        .filter(|r| !r.trim().is_empty())
        .ok_or("Missing required parameter: reason (say why your own shell can't run it)")?;
    let cwd = string("cwd")?.filter(|c| !c.trim().is_empty());
    let timeout_seconds = match obj.get("timeout_seconds") {
        None | Some(serde_json::Value::Null) => DEFAULT_RUN_TIMEOUT_SECS,
        Some(v) => v
            .as_u64()
            .filter(|t| (1..=MAX_RUN_TIMEOUT_SECS).contains(t))
            .ok_or_else(|| {
                format!("`timeout_seconds` must be an integer from 1 to {MAX_RUN_TIMEOUT_SECS}.")
            })?,
    };
    let request = HostCommandRequest::new(command, cwd, reason, timeout_seconds, agent_label());
    request
        .validate()
        .map_err(|e| format!("Invalid request: {e}."))?;
    Ok(request)
}

/// `get_outside_sandbox_result`'s argument: the `job_id` a "pending"
/// result gave, exactly (surrounding spaces aside).
pub(crate) fn parse_job_id(args: &serde_json::Value) -> Result<String, String> {
    let id = match args.as_object().and_then(|o| o.get("job_id")) {
        None | Some(serde_json::Value::Null) => {
            return Err("Missing required parameter: job_id (from a \"pending\" result).".into())
        }
        Some(serde_json::Value::String(s)) => s.trim().to_string(),
        Some(_) => return Err("`job_id` must be a string.".into()),
    };
    if !host_bridge::is_job_id(&id) {
        return Err(format!(
            "`job_id` isn't one Kage gave out (expected letters, digits and '-', at most {} characters). \
             Pass the job_id from the \"pending\" result exactly.",
            host_bridge::MAX_JOB_ID_BYTES
        ));
    }
    Ok(id)
}

/// The program that spawned this sidecar (the agent), for the user to see
/// when Kage can't tell which of its sessions asked.
fn agent_label() -> Option<String> {
    #[cfg(unix)]
    {
        let name = kage_core::os::process::get_process_name(std::os::unix::process::parent_id())?;
        let base = name.rsplit('/').next().unwrap_or(&name);
        let clean: String = base
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '))
            .take(64)
            .collect();
        let clean = clean.trim().to_string();
        (!clean.is_empty()).then_some(clean)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Which tool call a reply answers, for its wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Call {
    /// `run_outside_sandbox`, with its time limit.
    Run { timeout_seconds: u64 },
    /// `get_outside_sandbox_result` for this job.
    Poll { job_id: String },
}

fn secs(ms: u64) -> u64 {
    ms / 1000
}

/// "pending": where the job is, its id, and what to do next. Not an
/// error: the agent just keeps waiting.
fn pending_text(job_id: &str, job: Option<&JobProgress>) -> String {
    let next = format!(
        "Job id: {job_id}\nCall {RESULT_TOOL_NAME} with {{\"job_id\": \"{job_id}\"}} to keep waiting for the result. \
         Call it right away: it waits for the result itself (up to about {} s), so don't sleep or end your turn first. \
         Don't call {TOOL_NAME} again for this command: that would ask the user again and could run it twice.",
        host_bridge::LONG_POLL.as_secs()
    );
    match job {
        Some(p) if p.state == JobState::Running => {
            let mut text = format!(
                "Pending: the command is still running ({} s so far).\n{next}",
                secs(p.running_ms.unwrap_or(p.elapsed_ms))
            );
            match p.output_tail.as_deref().filter(|t| !t.trim().is_empty()) {
                Some(tail) => {
                    text.push_str("\nLast output lines:\n");
                    text.push_str(tail);
                }
                None => text.push_str("\nNo output yet."),
            }
            text
        }
        Some(p) => {
            let left = p
                .approval_expires_in_ms
                .map(|ms| {
                    format!(
                        "; Kage stops waiting in about {} min",
                        ms.div_ceil(60_000).max(1)
                    )
                })
                .unwrap_or_default();
            format!(
                "Pending: Kage is still waiting for the user to approve the command (asked {} s ago{left}). \
                 Nothing has run yet.\n{next}",
                secs(p.elapsed_ms)
            )
        }
        None => format!("Pending: the command hasn't finished.\n{next}"),
    }
}

/// What to do when a poll got no usable answer: poll again (it only
/// reads the job), never start the command a second time.
fn poll_again(job_id: &str) -> String {
    format!(
        "Call {RESULT_TOOL_NAME} again with {{\"job_id\": \"{job_id}\"}}; don't call {TOOL_NAME} \
         again for this command. If it keeps failing, ask the user whether the command ran."
    )
}

/// The tool result for one call: `(text, is_error)`.
pub(crate) fn result_for(
    outcome: Result<HostCommandResponse, BridgeError>,
    call: &Call,
) -> (String, bool) {
    let poll_id = match call {
        Call::Poll { job_id } => Some(job_id.as_str()),
        Call::Run { .. } => None,
    };
    let response = match outcome {
        Ok(r) => r,
        Err(BridgeError::NotRunning) => {
            return (
                match poll_id {
                    None => "Kage isn't running (or its command bridge didn't start), so nothing ran. \
                             Ask the user to start Kage, or to run the command themselves."
                        .into(),
                    Some(_) => "Kage isn't running (or its command bridge didn't start), so the job's \
                                result can't be read. Ask the user whether the command ran before trying again."
                        .into(),
                },
                true,
            )
        }
        Err(BridgeError::Unreachable(detail)) => {
            return (
                match poll_id {
                    None => format!(
                        "Couldn't reach Kage to run the command, so nothing ran ({detail}). \
                         Your sandbox may block connecting to Kage's local bridge; ask the user \
                         to run the command themselves."
                    ),
                    // The run itself got through, so this is passing: a
                    // poll only reads the job, and asking again is safe.
                    Some(id) => format!(
                        "Couldn't reach Kage to read the job's result ({detail}). {}",
                        poll_again(id)
                    ),
                },
                true,
            )
        }
        Err(BridgeError::TimedOut) => {
            return (
                match poll_id {
                    None => "No answer from Kage in time; whether the command ran is unknown. \
                             Ask the user before trying again."
                        .into(),
                    Some(id) => format!("No answer from Kage in time. {}", poll_again(id)),
                },
                true,
            )
        }
        Err(BridgeError::Protocol(detail)) => {
            return (
                match poll_id {
                    None => format!(
                        "Kage's reply couldn't be read ({detail}); whether the command ran is unknown."
                    ),
                    Some(id) => format!(
                        "Kage's reply couldn't be read ({detail}). {}",
                        poll_again(id)
                    ),
                },
                true,
            )
        }
    };
    match response.status {
        ResponseStatus::Pending => match response.job_id.as_deref().or(poll_id) {
            Some(id) if host_bridge::is_job_id(id) => {
                (pending_text(id, response.job.as_ref()), false)
            }
            _ => (
                "Kage said the command isn't finished but gave no job id; whether it runs is unknown. \
                 Ask the user before trying again."
                    .into(),
                true,
            ),
        },
        ResponseStatus::Ran => {
            let Some(run) = response.run else {
                return ("Kage said the command ran but sent no result.".into(), true);
            };
            let failed = run.outcome != "exited" || run.exit_code != Some(0);
            let (output, cut) = host_bridge::output_tail(&run.output, OUTPUT_TAIL_BYTES);
            let truncated = run.truncated || cut;
            let limit = run.timeout_seconds.or(match call {
                Call::Run { timeout_seconds } => Some(*timeout_seconds),
                Call::Poll { .. } => None,
            });
            let note = match run.outcome.as_str() {
                "timed_out" => Some(match limit {
                    Some(s) => {
                        format!("The command was still running after {s} seconds and was stopped.")
                    }
                    None => "The command ran out of time and was stopped.".into(),
                }),
                "cancelled" => Some("The user stopped the command before it finished.".into()),
                _ => None,
            };
            let mut body = serde_json::json!({
                "outcome": run.outcome,
                "exit_code": run.exit_code,
                "signal": run.signal,
                "duration_ms": run.duration_ms,
                "cwd": run.cwd,
                "approved_by": run.approved_by,
                "output": output,
                "output_truncated": truncated,
                "output_bytes": run.output_bytes,
            });
            if let Some(rule) = run.rule {
                body["rule"] = serde_json::Value::String(rule);
            }
            if truncated {
                body["output_note"] = serde_json::Value::String(format!(
                    "Only the last {} KiB of output is included; earlier output was dropped.",
                    OUTPUT_TAIL_BYTES / 1024
                ));
            }
            if let Some(note) = note {
                body["note"] = serde_json::Value::String(note);
            }
            (
                serde_json::to_string_pretty(&body).unwrap_or_default(),
                failed,
            )
        }
        ResponseStatus::Denied => (
            format!(
                "Denied by the user: they chose not to run this command outside your sandbox{}. \
                 Don't retry it unless they ask you to.",
                response
                    .message
                    .filter(|m| !m.is_empty())
                    .map(|m| format!(" ({m})"))
                    .unwrap_or_default()
            ),
            true,
        ),
        ResponseStatus::ApprovalTimedOut => (
            format!(
                "Timed out waiting for approval: nobody answered Kage's prompt within {} minutes, \
                 so the command did not run.",
                host_bridge::APPROVAL_TIMEOUT.as_secs() / 60
            ),
            true,
        ),
        ResponseStatus::Withdrawn => (
            format!(
                "Withdrawn: {}",
                response
                    .message
                    .unwrap_or_else(|| "the request ended before the user answered, so the command did not run.".into())
            ),
            true,
        ),
        ResponseStatus::NotFound => (
            format!(
                "Kage has no job {}. {} Whether the command ran is unknown; ask the user before \
                 running it again.",
                response.job_id.as_deref().or(poll_id).unwrap_or("with that id"),
                response
                    .message
                    .filter(|m| !m.trim().is_empty())
                    .unwrap_or_else(|| "It may have expired.".into())
            ),
            true,
        ),
        // From a poll this is the job's own end (the command couldn't
        // start, too many were running, Kage is quitting …) as often as a
        // problem with the poll, so it isn't worded as either.
        ResponseStatus::Error | ResponseStatus::Unknown => (
            format!(
                "{}: {}",
                if poll_id.is_some() {
                    "Kage reported an error for this job"
                } else {
                    "Kage didn't run the command"
                },
                response.message.unwrap_or_else(|| "no reason given".into())
            ),
            true,
        ),
    }
}

/// Handle one `run_outside_sandbox` call. Blocks until Kage answers: the
/// final result, or "pending" with the job's id after the long poll.
pub(crate) fn handle(
    id: &serde_json::Value,
    args: &serde_json::Value,
    bridge: &dyn Bridge,
) -> String {
    let request = match parse_args(args) {
        Ok(r) => r,
        Err(msg) => return tool_result_text(id, &msg, true),
    };
    log::info!(
        "[run_outside_sandbox] asking Kage to run a {}-byte command (timeout {} s)",
        request.command.len(),
        request.timeout_seconds
    );
    let call = Call::Run {
        timeout_seconds: request.timeout_seconds,
    };
    let outcome = bridge.send(&BridgeRequest::Run(request));
    log_outcome(&outcome);
    let (text, is_error) = result_for(outcome, &call);
    tool_result_text(id, &text, is_error)
}

/// Handle one `get_outside_sandbox_result` call: wait on the job at most
/// the long poll. It only reads the job.
pub(crate) fn handle_result(
    id: &serde_json::Value,
    args: &serde_json::Value,
    bridge: &dyn Bridge,
) -> String {
    let job_id = match parse_job_id(args) {
        Ok(j) => j,
        Err(msg) => return tool_result_text(id, &msg, true),
    };
    log::info!("[get_outside_sandbox_result] waiting on job {job_id}");
    let outcome = bridge.send(&BridgeRequest::Poll(PollRequest::new(job_id.clone())));
    log_outcome(&outcome);
    let (text, is_error) = result_for(outcome, &Call::Poll { job_id });
    tool_result_text(id, &text, is_error)
}

fn log_outcome(outcome: &Result<HostCommandResponse, BridgeError>) {
    match outcome {
        Ok(r) => log::info!("[run_outside_sandbox] Kage answered: {:?}", r.status),
        Err(e) => log::warn!("[run_outside_sandbox] no answer: {e:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kage_core::host_bridge::{ApprovedBy, RunSummary};
    use serde_json::json;
    use std::sync::Mutex;

    /// Records what it was asked and answers with a canned reply.
    struct FakeBridge {
        reply: Mutex<Option<Result<HostCommandResponse, BridgeError>>>,
        seen: Mutex<Vec<BridgeRequest>>,
    }

    impl FakeBridge {
        fn answering(reply: Result<HostCommandResponse, BridgeError>) -> Self {
            Self {
                reply: Mutex::new(Some(reply)),
                seen: Mutex::new(Vec::new()),
            }
        }

        fn sent(&self) -> Vec<BridgeRequest> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl Bridge for FakeBridge {
        fn send(&self, request: &BridgeRequest) -> Result<HostCommandResponse, BridgeError> {
            self.seen.lock().unwrap().push(request.clone());
            self.reply.lock().unwrap().take().expect("one call")
        }
    }

    fn result(text: &str) -> (String, bool) {
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        let r = &v["result"];
        (
            r["content"][0]["text"].as_str().unwrap().to_string(),
            r["isError"].as_bool().unwrap_or(false),
        )
    }

    fn ran(exit: i32, output: &str) -> HostCommandResponse {
        HostCommandResponse::ran(RunSummary {
            outcome: "exited".into(),
            exit_code: Some(exit),
            signal: None,
            duration_ms: 1500,
            output: output.into(),
            truncated: false,
            output_bytes: output.len() as u64,
            cwd: "/Users/me".into(),
            approved_by: ApprovedBy::User,
            rule: None,
            timeout_seconds: None,
        })
    }

    fn run_call() -> Call {
        Call::Run {
            timeout_seconds: 600,
        }
    }

    fn waiting(elapsed_ms: u64) -> JobProgress {
        JobProgress {
            state: JobState::WaitingForApproval,
            elapsed_ms,
            running_ms: None,
            output_tail: None,
            approval_expires_in_ms: Some(220_000),
        }
    }

    fn running(running_ms: u64, tail: &str) -> JobProgress {
        JobProgress {
            state: JobState::Running,
            elapsed_ms: running_ms + 30_000,
            running_ms: Some(running_ms),
            output_tail: Some(tail.into()),
            approval_expires_in_ms: None,
        }
    }

    #[test]
    fn the_schema_requires_command_and_reason() {
        let schema = input_schema();
        assert_eq!(schema["required"], json!(["command", "reason"]));
        assert_eq!(
            schema["properties"]["timeout_seconds"]["maximum"],
            json!(1800)
        );
        assert_eq!(
            schema["properties"]["timeout_seconds"]["default"],
            json!(600)
        );
        assert!(DESCRIPTION.contains("ONLY after your own shell tool already failed"));
        assert!(DESCRIPTION.contains("credentials"));
        assert!(DESCRIPTION.contains("approves every run unless they made a rule"));
    }

    #[test]
    fn the_descriptions_say_how_to_wait_and_never_to_rerun() {
        assert!(DESCRIPTION.contains("\"pending\" with a job_id"));
        assert!(DESCRIPTION.contains(RESULT_TOOL_NAME));
        assert!(DESCRIPTION.contains("Never call run_outside_sandbox again for the same command"));
        assert!(RESULT_DESCRIPTION.contains("call it again with the same job_id"));
        assert!(RESULT_DESCRIPTION.contains("only reads the job"));
        let schema = result_input_schema();
        assert_eq!(schema["required"], json!(["job_id"]));
        assert_eq!(schema["properties"]["job_id"]["type"], json!("string"));
    }

    #[test]
    fn arguments_are_checked_before_anything_is_sent() {
        for (args, needle) in [
            (json!({"reason": "r"}), "command"),
            (json!({"command": "  ", "reason": "r"}), "command"),
            (json!({"command": "ls"}), "reason"),
            (json!({"command": 5, "reason": "r"}), "string"),
            (
                json!({"command": "ls", "reason": "r", "timeout_seconds": 0}),
                "timeout",
            ),
            (
                json!({"command": "ls", "reason": "r", "timeout_seconds": 1801}),
                "timeout",
            ),
            (
                json!({"command": "ls", "reason": "r", "timeout_seconds": "60"}),
                "timeout",
            ),
            (json!({"command": "ls", "reason": "r", "cwd": 1}), "string"),
            (json!(["ls"]), "object"),
        ] {
            let bridge = FakeBridge::answering(Err(BridgeError::NotRunning));
            let (text, is_error) = result(&handle(&json!(1), &args, &bridge));
            assert!(is_error, "{args}");
            assert!(text.contains(needle), "{args}: {text}");
            assert!(bridge.sent().is_empty(), "{args} reached the bridge");
        }
    }

    #[test]
    fn a_valid_call_sends_exactly_the_command_as_a_job() {
        let bridge = FakeBridge::answering(Ok(ran(0, "==> Pouring awscli\n").for_job("hc-1")));
        let args = json!({
            "command": "brew install awscli",
            "cwd": "~/src",
            "reason": "sandbox-exec: sandbox_apply: Operation not permitted",
        });
        let (text, is_error) = result(&handle(&json!(7), &args, &bridge));
        assert!(!is_error, "{text}");
        let sent = match bridge.sent().remove(0) {
            BridgeRequest::Run(r) => r,
            other => panic!("sent {other:?}"),
        };
        assert_eq!(sent.command, "brew install awscli");
        assert_eq!(sent.cwd.as_deref(), Some("~/src"));
        assert_eq!(sent.timeout_seconds, DEFAULT_RUN_TIMEOUT_SECS);
        assert_eq!(sent.v, host_bridge::PROTOCOL_VERSION);
        assert_eq!(sent.kind, Some(host_bridge::RequestKind::Run));
        let body: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["exit_code"], json!(0));
        assert_eq!(body["output"], json!("==> Pouring awscli\n"));
        assert_eq!(body["approved_by"], json!("user"));
        assert_eq!(body["duration_ms"], json!(1500));
    }

    #[test]
    fn an_unanswered_prompt_comes_back_pending_with_the_job_id() {
        let bridge = FakeBridge::answering(Ok(HostCommandResponse::pending(
            "hc-0123456789abcdef01234567",
            waiting(80_400),
        )));
        let args = json!({ "command": "brew install --cask wallspace", "reason": "sandboxed" });
        let (text, is_error) = result(&handle(&json!(1), &args, &bridge));
        assert!(!is_error, "pending isn't a failure: {text}");
        assert!(text.starts_with("Pending: Kage is still waiting for the user to approve"));
        assert!(text.contains("asked 80 s ago"), "{text}");
        assert!(text.contains("about 4 min"), "{text}");
        assert!(text.contains("Nothing has run yet"));
        assert!(text.contains(
            "Call get_outside_sandbox_result with {\"job_id\": \"hc-0123456789abcdef01234567\"}"
        ));
        assert!(text.contains("Don't call run_outside_sandbox again"));
    }

    #[test]
    fn a_running_command_comes_back_pending_with_its_time_and_last_lines() {
        let (text, is_error) = result_for(
            Ok(HostCommandResponse::pending(
                "hc-2",
                running(95_000, "==> Downloading\n==> Installing Cask wallspace"),
            )),
            &run_call(),
        );
        assert!(!is_error);
        assert!(text.starts_with("Pending: the command is still running (95 s so far)."));
        assert!(text.contains("Job id: hc-2"));
        assert!(
            text.ends_with("Last output lines:\n==> Downloading\n==> Installing Cask wallspace")
        );

        let (text, _) = result_for(
            Ok(HostCommandResponse::pending("hc-2", running(3_000, ""))),
            &run_call(),
        );
        assert!(text.ends_with("No output yet."), "{text}");
    }

    #[test]
    fn a_pending_reply_without_a_usable_job_id_is_an_error() {
        let mut resp = HostCommandResponse::pending("hc-1", waiting(1));
        resp.job_id = None;
        let (text, is_error) = result_for(Ok(resp.clone()), &run_call());
        assert!(is_error);
        assert!(text.contains("gave no job id"));
        resp.job_id = Some("not a job id".into());
        assert!(result_for(Ok(resp), &run_call()).1);
    }

    #[test]
    fn the_result_tool_checks_its_job_id_before_asking_kage() {
        for (args, needle) in [
            (json!({}), "Missing required parameter: job_id"),
            (json!({"job_id": 5}), "must be a string"),
            (json!({"job_id": "  "}), "isn't one Kage gave out"),
            (
                json!({"job_id": "hc-1; rm -rf ~"}),
                "isn't one Kage gave out",
            ),
            (json!({"job_id": "x".repeat(65)}), "isn't one Kage gave out"),
            (json!("hc-1"), "Missing required parameter"),
        ] {
            let bridge = FakeBridge::answering(Err(BridgeError::NotRunning));
            let (text, is_error) = result(&handle_result(&json!(1), &args, &bridge));
            assert!(is_error, "{args}");
            assert!(text.contains(needle), "{args}: {text}");
            assert!(bridge.sent().is_empty(), "{args} reached the bridge");
        }
    }

    #[test]
    fn the_result_tool_polls_the_job_and_returns_its_result() {
        let bridge =
            FakeBridge::answering(Ok(ran(0, "🍺 wallspace was installed!\n").for_job("hc-9")));
        let (text, is_error) = result(&handle_result(
            &json!(3),
            &json!({ "job_id": " hc-9 " }),
            &bridge,
        ));
        assert!(!is_error, "{text}");
        assert_eq!(
            bridge.sent(),
            vec![BridgeRequest::Poll(PollRequest::new("hc-9"))]
        );
        let body: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["exit_code"], json!(0));
        assert!(body["output"].as_str().unwrap().contains("was installed"));

        // Still going: pending again, naming the same job.
        let bridge = FakeBridge::answering(Ok(HostCommandResponse::pending(
            "hc-9",
            running(170_000, "==> Installing"),
        )));
        let (text, is_error) = result(&handle_result(
            &json!(4),
            &json!({ "job_id": "hc-9" }),
            &bridge,
        ));
        assert!(!is_error);
        assert!(text.contains("{\"job_id\": \"hc-9\"}"), "{text}");
    }

    #[test]
    fn a_poll_says_what_it_can_when_there_is_no_result() {
        let poll = Call::Poll {
            job_id: "hc-7".into(),
        };
        let (text, is_error) = result_for(Ok(HostCommandResponse::not_found("hc-7")), &poll);
        assert!(is_error);
        assert_eq!(
            text,
            "Kage has no job hc-7. A job's result is kept for 15 minutes after the command ends, \
             and none survives a restart of Kage. Whether the command ran is unknown; ask the \
             user before running it again."
        );

        // A poll that got no usable answer: ask again (it only reads the
        // job), never run the command a second time. The run reached Kage,
        // so this isn't the sandbox blocking the bridge.
        for outcome in [
            Err(BridgeError::TimedOut),
            Err(BridgeError::Unreachable("Resource busy".into())),
            Err(BridgeError::Protocol(
                "Kage closed the connection without answering".into(),
            )),
        ] {
            let (text, is_error) = result_for(outcome, &poll);
            assert!(is_error);
            assert!(
                text.contains("Call get_outside_sandbox_result again with {\"job_id\": \"hc-7\"}"),
                "{text}"
            );
            assert!(
                text.contains("don't call run_outside_sandbox again"),
                "{text}"
            );
            assert!(!text.contains("run the command themselves"), "{text}");
        }

        let (text, _) = result_for(Err(BridgeError::NotRunning), &poll);
        assert!(text.contains("can't be read"), "{text}");

        // The job's own end, or a problem with the poll: worded as neither.
        let (text, _) = result_for(Ok(HostCommandResponse::error("starting up")), &poll);
        assert_eq!(text, "Kage reported an error for this job: starting up");
    }

    #[test]
    fn a_pending_reply_says_to_poll_at_once() {
        for job in [waiting(80_000), running(5_000, "==> Pouring")] {
            let (text, _) = result_for(Ok(HostCommandResponse::pending("hc-3", job)), &run_call());
            assert!(text.contains("Call it right away"), "{text}");
            assert!(
                text.contains("don't sleep or end your turn first"),
                "{text}"
            );
        }
        // A run call that couldn't reach Kage still says nothing ran.
        let (text, _) = result_for(Err(BridgeError::Unreachable("denied".into())), &run_call());
        assert!(text.contains("so nothing ran"), "{text}");
        assert!(text.contains("run the command themselves"), "{text}");
    }

    #[test]
    fn a_withdrawn_request_says_nothing_ran() {
        let (text, is_error) = result_for(
            Ok(HostCommandResponse::withdrawn(
                "The request was withdrawn before the user answered: the agent disconnected \
                 from Kage, so the command did not run.",
            )),
            &Call::Poll {
                job_id: "hc-1".into(),
            },
        );
        assert!(is_error);
        assert!(text.starts_with("Withdrawn: "));
        assert!(text.contains("did not run"));
    }

    #[test]
    fn a_failing_command_is_a_tool_error_with_its_output() {
        let (text, is_error) = result_for(Ok(ran(1, "Error: no formula\n")), &run_call());
        assert!(is_error);
        assert!(text.contains("no formula"));
        assert!(text.contains("\"exit_code\": 1"));
    }

    #[test]
    fn long_output_is_cut_to_its_tail_and_says_so() {
        let output = format!("{}END", "x".repeat(OUTPUT_TAIL_BYTES * 2));
        let (text, _) = result_for(Ok(ran(0, &output)), &run_call());
        let body: serde_json::Value = serde_json::from_str(&text).unwrap();
        let shown = body["output"].as_str().unwrap();
        assert!(shown.len() <= OUTPUT_TAIL_BYTES);
        assert!(shown.ends_with("END"));
        assert_eq!(body["output_truncated"], json!(true));
        assert!(body["output_note"].as_str().unwrap().contains("32 KiB"));
    }

    #[test]
    fn a_rule_approval_names_the_rule() {
        let mut resp = ran(0, "");
        if let Some(run) = resp.run.as_mut() {
            run.approved_by = ApprovedBy::Rule;
            run.rule = Some("brew install".into());
        }
        let (text, _) = result_for(Ok(resp), &run_call());
        let body: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["approved_by"], json!("rule"));
        assert_eq!(body["rule"], json!("brew install"));
    }

    #[test]
    fn every_failure_says_clearly_what_happened() {
        let cases = [
            (Ok(HostCommandResponse::denied("")), "Denied by the user"),
            (
                Ok(HostCommandResponse::approval_timed_out()),
                "Timed out waiting for approval",
            ),
            (Err(BridgeError::NotRunning), "Kage isn't running"),
            (
                Err(BridgeError::Unreachable("permission denied".into())),
                "Couldn't reach Kage",
            ),
            (Err(BridgeError::TimedOut), "No answer from Kage"),
            (
                Ok(HostCommandResponse::error("too many commands waiting")),
                "too many commands waiting",
            ),
        ];
        for (outcome, needle) in cases {
            let (text, is_error) = result_for(outcome, &run_call());
            assert!(is_error, "{needle}");
            assert!(text.contains(needle), "{text}");
        }
    }

    #[test]
    fn a_stopped_or_timed_out_run_says_so() {
        let mut resp = ran(0, "");
        if let Some(run) = resp.run.as_mut() {
            run.outcome = "timed_out".into();
            run.exit_code = None;
            run.signal = Some(15);
        }
        let (text, is_error) = result_for(
            Ok(resp.clone()),
            &Call::Run {
                timeout_seconds: 90,
            },
        );
        assert!(is_error);
        assert!(text.contains("after 90 seconds"), "{text}");

        // From a poll, the limit comes with the result.
        if let Some(run) = resp.run.as_mut() {
            run.timeout_seconds = Some(120);
        }
        let (text, _) = result_for(
            Ok(resp),
            &Call::Poll {
                job_id: "hc-1".into(),
            },
        );
        assert!(text.contains("after 120 seconds"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn without_kage_the_real_bridge_reports_not_running() {
        // The sidecar's test process has no Kage listening on an isolated
        // profile.
        let tmp = tempfile::tempdir().unwrap();
        if !kage_core::paths::isolate(tmp.path()) {
            return; // another test isolated first; nothing listens there either
        }
        let req = HostCommandRequest::new("true", None, "test", 5, None);
        assert_eq!(
            OsBridge.send(&BridgeRequest::Run(req)),
            Err(BridgeError::NotRunning)
        );
        assert_eq!(
            OsBridge.send(&BridgeRequest::Poll(PollRequest::new("hc-1"))),
            Err(BridgeError::NotRunning)
        );
    }
}
