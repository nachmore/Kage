//! The prompt recovery ladder against a scripted agent: an internal agent
//! error before the turn did anything restarts the agent once, reloads the
//! session and resends once; anything the turn already did, or a sign-in
//! demand, is reported as it is; and a restart that didn't help keeps the
//! next failure from restarting again for a while (the loop guard).
//!
//! The agent is a loopback TCP mock (Remote mode), so a "restart" is a
//! fresh connection the mock can answer differently. Ephemeral port,
//! test-only, bound to 127.0.0.1.

use kage::acp_client::types::{AgentRpcError, PromptRecoveryError};
use kage::acp_client::{AcpClient, AcpConnectionMode};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const INCIDENT_DETAIL: &str =
    "Encountered an error in the response stream: An unknown error occurred: dispatch failure";

/// How a connection answers `session/prompt`.
#[derive(Clone, Copy)]
enum Prompt {
    Ok,
    /// `-32603` with the incident's text.
    DispatchFailure,
    /// One `agent_message_chunk`, then `-32603`.
    ChunkThenFailure,
    /// A `tool_call` update, then `-32603`.
    ToolCallThenFailure,
    /// `-32000`.
    AuthRequired,
}

struct MockAgent {
    port: u16,
    connections: Arc<AtomicUsize>,
    /// (connection index, method) of every request, in arrival order.
    requests: Arc<Mutex<Vec<(usize, String)>>>,
}

impl MockAgent {
    /// `script[i]` is how connection `i` answers prompts; the last entry
    /// covers any later connection.
    fn start(script: Vec<Prompt>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (conns, log) = (connections.clone(), requests.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let index = conns.fetch_add(1, Ordering::SeqCst);
                let behaviour = script[index.min(script.len() - 1)];
                let log = log.clone();
                std::thread::spawn(move || serve(index, behaviour, stream, log));
            }
        });
        Self {
            port,
            connections,
            requests,
        }
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    fn count(&self, method: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, m)| m == method)
            .count()
    }

    fn client(&self) -> AcpClient {
        let client = AcpClient::new(AcpConnectionMode::Remote {
            host: "127.0.0.1".to_string(),
            port: self.port,
        });
        client.connect().expect("connect to the mock agent");
        client
    }
}

fn serve(
    index: usize,
    behaviour: Prompt,
    stream: std::net::TcpStream,
    log: Arc<Mutex<Vec<(usize, String)>>>,
) {
    let mut writer = stream.try_clone().unwrap();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(method) = msg["method"].as_str() else {
            continue;
        };
        if msg.get("id").is_none() {
            continue; // a notification (session/cancel)
        }
        log.lock().unwrap().push((index, method.to_string()));
        let sid = msg["params"]["sessionId"].clone();
        let mut lines = Vec::new();
        let reply = match method {
            "session/prompt" => {
                let internal = json!({
                    "code": -32603, "message": "Internal error", "data": INCIDENT_DETAIL
                });
                match behaviour {
                    Prompt::Ok => json!({ "result": { "stopReason": "end_turn" } }),
                    Prompt::DispatchFailure => json!({ "error": internal }),
                    Prompt::ChunkThenFailure => {
                        lines.push(json!({
                            "jsonrpc": "2.0", "method": "session/update",
                            "params": { "sessionId": sid, "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": "Partial answer" }
                            } }
                        }));
                        json!({ "error": internal })
                    }
                    Prompt::ToolCallThenFailure => {
                        lines.push(json!({
                            "jsonrpc": "2.0", "method": "session/update",
                            "params": { "sessionId": sid, "update": {
                                "sessionUpdate": "tool_call", "toolCallId": "t1",
                                "title": "Read file", "status": "pending"
                            } }
                        }));
                        json!({ "error": internal })
                    }
                    Prompt::AuthRequired => json!({
                        "error": { "code": -32000, "message": "Authentication required" }
                    }),
                }
            }
            "session/new" => json!({ "result": { "sessionId": "NEW" } }),
            _ => json!({ "result": {} }),
        };
        let mut reply = reply;
        reply["jsonrpc"] = json!("2.0");
        reply["id"] = msg["id"].clone();
        lines.push(reply);
        for l in lines {
            if writeln!(writer, "{l}").is_err() {
                return;
            }
        }
    }
}

fn isolate_user_dirs() {
    kage::paths::isolate(
        std::env::temp_dir().join(format!("kage-prompt-recovery-test-{}", std::process::id())),
    );
    assert!(
        kage::paths::is_isolated(),
        "refusing to run against the real per-user directories"
    );
}

fn send(client: &AcpClient) -> anyhow::Result<String> {
    client.send_chat_streaming_with_recovery("S1".to_string(), "hello".to_string(), None, None)
}

#[test]
fn internal_error_with_no_output_restarts_reloads_and_resends_once() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::DispatchFailure, Prompt::Ok]);
    let client = agent.client();

    let id = send(&client).expect("recovered by the restart");

    assert_eq!(id, "S1", "the reloaded session, not a fresh one");
    assert_eq!(agent.count("session/prompt"), 2);
    assert_eq!(agent.connections(), 2);
    assert_eq!(agent.count("session/load"), 1);
    assert_eq!(agent.count("session/new"), 0);
}

#[test]
fn still_failing_after_the_restart_reports_unreachable_and_latches() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::DispatchFailure]);
    let client = agent.client();

    let err = send(&client).unwrap_err();
    match err.downcast_ref::<PromptRecoveryError>() {
        Some(PromptRecoveryError::AgentUnreachable { detail }) => {
            assert!(detail.contains("dispatch failure"), "{detail}")
        }
        None => panic!("expected AgentUnreachable, got {err}"),
    }
    assert_eq!(agent.count("session/prompt"), 2);
    assert_eq!(agent.connections(), 2);
    assert_eq!(agent.count("session/new"), 0, "no fresh-session attempt");

    // The loop guard: the next failure is reported without a restart.
    let err = send(&client).unwrap_err();
    assert!(
        err.downcast_ref::<PromptRecoveryError>().is_some(),
        "expected AgentUnreachable, got {err}"
    );
    assert_eq!(agent.count("session/prompt"), 3);
    assert_eq!(agent.connections(), 2, "no restart while the latch holds");

    // Reconnect clears it: the next failure goes through the restart path
    // again (reload + one resend). The restart itself coalesces onto the
    // one that just succeeded, so it's the same connection.
    client.clear_unreachable_latch();
    let _ = send(&client).unwrap_err();
    assert_eq!(agent.count("session/prompt"), 5);
    assert_eq!(agent.count("session/load"), 2);
}

#[test]
fn internal_error_after_streamed_text_is_not_resent() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::ChunkThenFailure, Prompt::Ok]);
    let client = agent.client();

    let err = send(&client).unwrap_err();

    let typed = err
        .downcast_ref::<AgentRpcError>()
        .expect("the agent's own error");
    assert_eq!(typed.code, -32603);
    assert_eq!(typed.progress.updates, 1);
    assert_eq!(
        err.to_string(),
        format!("ACP error: Internal error — {INCIDENT_DETAIL}")
    );
    assert_eq!(agent.count("session/prompt"), 1);
    assert_eq!(agent.connections(), 1);
}

#[test]
fn internal_error_after_a_tool_call_is_not_resent() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::ToolCallThenFailure, Prompt::Ok]);
    let client = agent.client();

    let err = send(&client).unwrap_err();

    let typed = err.downcast_ref::<AgentRpcError>().unwrap();
    assert_eq!(typed.progress.tool_calls, 1);
    assert_eq!(agent.count("session/prompt"), 1);
    assert_eq!(agent.connections(), 1);
}

#[test]
fn auth_required_is_never_restarted() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::AuthRequired, Prompt::Ok]);
    let client = agent.client();

    let err = send(&client).unwrap_err();

    assert_eq!(err.downcast_ref::<AgentRpcError>().unwrap().code, -32000);
    assert_eq!(agent.count("session/prompt"), 1);
    assert_eq!(agent.connections(), 1);
}

#[test]
fn another_windows_turn_in_flight_blocks_the_restart() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::DispatchFailure, Prompt::Ok]);
    let client = agent.client();
    client.set_in_flight_prompt("OTHER", "a prompt in another window");

    let err = send(&client).unwrap_err();

    assert_eq!(err.downcast_ref::<AgentRpcError>().unwrap().code, -32603);
    assert_eq!(agent.count("session/prompt"), 1);
    assert_eq!(agent.connections(), 1);
}
