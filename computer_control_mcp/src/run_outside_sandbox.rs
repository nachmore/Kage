//! `run_outside_sandbox`: ask the Kage app to run a shell command that the
//! agent's own sandbox refused (Homebrew can't nest its sandbox inside the
//! agent's, …).
//!
//! This sidecar is a child of the sandboxed agent, so it can't run the
//! command itself: it sends the request over the host-command bridge
//! (`kage_core::host_bridge`, a socket / named pipe only this user can
//! reach) and waits. Kage shows the user the exact command, where it runs
//! and the agent's reason, and runs it only once they approve — or a rule
//! they made covers it — then replies with the exit code and the output's
//! tail, which goes back to the agent as this tool's result.

use kage_core::host_bridge::{
    self, HostCommandRequest, HostCommandResponse, LineRead, ResponseStatus,
    DEFAULT_RUN_TIMEOUT_SECS, MAX_RUN_TIMEOUT_SECS, OUTPUT_TAIL_BYTES,
};
use kage_core::mcp_json_rpc::tool_result_text;
use std::io::Write;

pub(crate) const TOOL_NAME: &str = "run_outside_sandbox";

pub(crate) const DESCRIPTION: &str = "Run a shell command on the user's computer OUTSIDE your sandbox, through the Kage app. \
Use this ONLY after your own shell tool already failed because your sandbox blocked the command \
(for example `brew install` failing with \"sandbox-exec: sandbox_apply: Operation not permitted\"). \
For everything else keep using your own shell tool. \
Never use it to read, print or copy credentials or secrets (AWS or SSH keys, tokens, the keychain, environment dumps). \
The user sees the exact command, its working directory and your reason, and approves every run unless they made a rule for it. \
If they deny it, don't retry the same command unless they ask you to. \
It runs in the user's login shell with no input (nothing interactive; sudo can't ask for a password). \
Returns the exit code, the last 32 KiB of output and how long it took.";

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
    fn send(&self, request: &HostCommandRequest) -> Result<HostCommandResponse, BridgeError>;
}

/// The real bridge: the endpoint Kage computes the same way.
pub(crate) struct OsBridge;

impl Bridge for OsBridge {
    fn send(&self, request: &HostCommandRequest) -> Result<HostCommandResponse, BridgeError> {
        use kage_core::os::host_bridge as transport;
        let endpoint = transport::default_endpoint().ok_or(BridgeError::NotRunning)?;
        let mut stream = transport::connect(&endpoint).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                BridgeError::NotRunning
            }
            _ => BridgeError::Unreachable(e.to_string()),
        })?;
        stream
            .set_read_timeout(Some(host_bridge::reply_wait(request.timeout_seconds)))
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

/// The tool result for one call: `(text, is_error)`.
pub(crate) fn result_for(
    outcome: Result<HostCommandResponse, BridgeError>,
    timeout_seconds: u64,
) -> (String, bool) {
    let response = match outcome {
        Ok(r) => r,
        Err(BridgeError::NotRunning) => {
            return (
                "Kage isn't running (or its command bridge didn't start), so nothing ran. \
                 Ask the user to start Kage, or to run the command themselves."
                    .into(),
                true,
            )
        }
        Err(BridgeError::Unreachable(detail)) => {
            return (
                format!(
                    "Couldn't reach Kage to run the command, so nothing ran ({detail}). \
                     Your sandbox may block connecting to Kage's local bridge; ask the user \
                     to run the command themselves."
                ),
                true,
            )
        }
        Err(BridgeError::TimedOut) => {
            return (
                "No answer from Kage in time; whether the command ran is unknown. \
                 Ask the user before trying again."
                    .into(),
                true,
            )
        }
        Err(BridgeError::Protocol(detail)) => {
            return (
                format!(
                    "Kage's reply couldn't be read ({detail}); whether the command ran is unknown."
                ),
                true,
            )
        }
    };
    match response.status {
        ResponseStatus::Ran => {
            let Some(run) = response.run else {
                return ("Kage said the command ran but sent no result.".into(), true);
            };
            let failed = run.outcome != "exited" || run.exit_code != Some(0);
            let (output, cut) = host_bridge::output_tail(&run.output, OUTPUT_TAIL_BYTES);
            let truncated = run.truncated || cut;
            let note = match run.outcome.as_str() {
                "timed_out" => Some(format!(
                    "The command was still running after {timeout_seconds} seconds and was stopped."
                )),
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
        ResponseStatus::Error | ResponseStatus::Unknown => (
            format!(
                "Kage didn't run the command: {}",
                response.message.unwrap_or_else(|| "no reason given".into())
            ),
            true,
        ),
    }
}

/// Handle one `tools/call`. Blocks until Kage answers: the user's decision
/// plus the run itself (see [`host_bridge::reply_wait`]).
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
    let outcome = bridge.send(&request);
    match &outcome {
        Ok(r) => log::info!("[run_outside_sandbox] Kage answered: {:?}", r.status),
        Err(e) => log::warn!("[run_outside_sandbox] no answer: {e:?}"),
    }
    let (text, is_error) = result_for(outcome, request.timeout_seconds);
    tool_result_text(id, &text, is_error)
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
        seen: Mutex<Vec<HostCommandRequest>>,
    }

    impl FakeBridge {
        fn answering(reply: Result<HostCommandResponse, BridgeError>) -> Self {
            Self {
                reply: Mutex::new(Some(reply)),
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl Bridge for FakeBridge {
        fn send(&self, request: &HostCommandRequest) -> Result<HostCommandResponse, BridgeError> {
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
        })
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
            assert!(
                bridge.seen.lock().unwrap().is_empty(),
                "{args} reached the bridge"
            );
        }
    }

    #[test]
    fn a_valid_call_sends_exactly_the_command() {
        let bridge = FakeBridge::answering(Ok(ran(0, "==> Pouring awscli\n")));
        let args = json!({
            "command": "brew install awscli",
            "cwd": "~/src",
            "reason": "sandbox-exec: sandbox_apply: Operation not permitted",
        });
        let (text, is_error) = result(&handle(&json!(7), &args, &bridge));
        assert!(!is_error, "{text}");
        let sent = bridge.seen.lock().unwrap()[0].clone();
        assert_eq!(sent.command, "brew install awscli");
        assert_eq!(sent.cwd.as_deref(), Some("~/src"));
        assert_eq!(sent.timeout_seconds, DEFAULT_RUN_TIMEOUT_SECS);
        assert_eq!(sent.v, host_bridge::PROTOCOL_VERSION);
        let body: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["exit_code"], json!(0));
        assert_eq!(body["output"], json!("==> Pouring awscli\n"));
        assert_eq!(body["approved_by"], json!("user"));
        assert_eq!(body["duration_ms"], json!(1500));
    }

    #[test]
    fn a_failing_command_is_a_tool_error_with_its_output() {
        let (text, is_error) = result_for(Ok(ran(1, "Error: no formula\n")), 600);
        assert!(is_error);
        assert!(text.contains("no formula"));
        assert!(text.contains("\"exit_code\": 1"));
    }

    #[test]
    fn long_output_is_cut_to_its_tail_and_says_so() {
        let output = format!("{}END", "x".repeat(OUTPUT_TAIL_BYTES * 2));
        let (text, _) = result_for(Ok(ran(0, &output)), 600);
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
        let (text, _) = result_for(Ok(resp), 600);
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
            let (text, is_error) = result_for(outcome, 600);
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
        let (text, is_error) = result_for(Ok(resp), 90);
        assert!(is_error);
        assert!(text.contains("after 90 seconds"), "{text}");
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
        assert_eq!(OsBridge.send(&req), Err(BridgeError::NotRunning));
    }
}
