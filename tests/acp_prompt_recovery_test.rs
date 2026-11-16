//! The prompt recovery ladder against a scripted agent: an internal agent
//! error before the turn did anything restarts the agent once, reloads the
//! session and resends once; anything the turn already did, or a sign-in
//! demand, is reported as it is; and a restart that didn't help keeps the
//! next failure from restarting again for a while (the loop guard).
//!
//! The agent is a loopback TCP mock (Remote mode), so a "restart" is a
//! fresh connection the mock can answer differently. Ephemeral port,
//! test-only, bound to 127.0.0.1.

use kage::acp_client::turn_progress::PromptOutcome;
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
    /// Prompts succeed, but `session/load` answers `-32603`.
    LoadFails,
    /// Prompts succeed, and `session/load` takes 600 ms.
    SlowLoad,
    /// A `session/request_permission` request, then `-32603` (the agent
    /// asked, was answered, acted, and then failed).
    PermissionThenFailure,
    /// The connection closes on the first prompt (the agent died mid-turn).
    ConnectionCloses,
    /// Prompts are never answered while the connection lives.
    Hang,
    /// The first prompt fails with `-32603` after 300 ms; later prompts on
    /// the same connection succeed after 500 ms.
    SlowFailThenSlowOk,
    /// A `session/request_permission` request, then the connection closes
    /// (the agent acted on the answer and crashed before replying).
    PermissionThenConnectionCloses,
    /// A `tool_call` update, then `-32603` "panicked" (a corrupted
    /// session, after the turn already ran a tool).
    ToolCallThenPanic,
    /// The first prompt is never answered; the second closes the
    /// connection (the agent crashed with two turns running).
    HangThenCloseOnSecond,
    /// `session/new` answers `-32000` with kiro-cli's signed-out text.
    NewAuthRequired,
    /// `session/load` answers `-32000` with kiro-cli's signed-out text.
    LoadAuthRequired,
    /// `initialize` answers `-32000` with kiro-cli's signed-out text.
    InitAuthRequired,
    /// `session/new` answers `-32603` (a failure that isn't a sign-in).
    NewFails,
}

/// What kiro-cli answers `session/new` with when it is signed out.
const KIRO_SIGNED_OUT: &str = "Authentication required. Not logged in. Set the KIRO_API_KEY \
environment variable or run kiro-cli login first.";

struct MockAgent {
    port: u16,
    connections: Arc<AtomicUsize>,
    /// (connection index, method, params) of every request, in arrival
    /// order.
    requests: Arc<Mutex<Vec<(usize, String, Value)>>>,
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
            .filter(|(_, m, _)| m == method)
            .count()
    }

    /// How many requests of `method` connection `index` had.
    fn count_on(&self, index: usize, method: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(i, m, _)| *i == index && m == method)
            .count()
    }

    /// The params of every request of `method`, in arrival order.
    fn params(&self, method: &str) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, m, _)| m == method)
            .map(|(_, _, p)| p.clone())
            .collect()
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
    log: Arc<Mutex<Vec<(usize, String, Value)>>>,
) {
    let mut writer = stream.try_clone().unwrap();
    let mut prompts_here = 0usize;
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
        log.lock()
            .unwrap()
            .push((index, method.to_string(), msg["params"].clone()));
        let sid = msg["params"]["sessionId"].clone();
        let mut lines = Vec::new();
        let reply = match method {
            "session/prompt" => {
                prompts_here += 1;
                let internal = json!({
                    "code": -32603, "message": "Internal error", "data": INCIDENT_DETAIL
                });
                match behaviour {
                    Prompt::Ok
                    | Prompt::LoadFails
                    | Prompt::SlowLoad
                    | Prompt::NewAuthRequired
                    | Prompt::LoadAuthRequired
                    | Prompt::InitAuthRequired
                    | Prompt::NewFails => {
                        json!({ "result": { "stopReason": "end_turn" } })
                    }
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
                    Prompt::PermissionThenFailure => {
                        lines.push(json!({
                            "jsonrpc": "2.0", "id": "perm-1",
                            "method": "session/request_permission",
                            "params": { "sessionId": sid, "toolCall": {
                                "toolCallId": "t1", "title": "Delete build/"
                            }, "options": [] }
                        }));
                        json!({ "error": internal })
                    }
                    Prompt::ConnectionCloses => return,
                    Prompt::PermissionThenConnectionCloses => {
                        let request = json!({
                            "jsonrpc": "2.0", "id": "perm-1",
                            "method": "session/request_permission",
                            "params": { "sessionId": sid, "toolCall": {
                                "toolCallId": "t1", "title": "rm -rf build/"
                            }, "options": [] }
                        });
                        let _ = writeln!(writer, "{request}");
                        return;
                    }
                    Prompt::ToolCallThenPanic => {
                        lines.push(json!({
                            "jsonrpc": "2.0", "method": "session/update",
                            "params": { "sessionId": sid, "update": {
                                "sessionUpdate": "tool_call", "toolCallId": "t1",
                                "title": "Delete build/", "status": "in_progress"
                            } }
                        }));
                        json!({ "error": {
                            "code": -32603, "message": "Internal error",
                            "data": "thread 'tokio-runtime-worker' panicked at src/agent.rs:42"
                        } })
                    }
                    Prompt::Hang => continue,
                    Prompt::HangThenCloseOnSecond => {
                        if prompts_here == 1 {
                            continue;
                        }
                        return;
                    }
                    Prompt::SlowFailThenSlowOk => {
                        if prompts_here == 1 {
                            std::thread::sleep(std::time::Duration::from_millis(300));
                            json!({ "error": internal })
                        } else {
                            std::thread::sleep(std::time::Duration::from_millis(500));
                            json!({ "result": { "stopReason": "end_turn" } })
                        }
                    }
                }
            }
            "initialize" if matches!(behaviour, Prompt::InitAuthRequired) => json!({
                "error": { "code": -32000, "message": KIRO_SIGNED_OUT }
            }),
            "session/new" if matches!(behaviour, Prompt::NewAuthRequired) => json!({
                "error": { "code": -32000, "message": KIRO_SIGNED_OUT }
            }),
            "session/new" if matches!(behaviour, Prompt::NewFails) => json!({
                "error": { "code": -32603, "message": "Internal error", "data": "disk full" }
            }),
            "session/load" if matches!(behaviour, Prompt::LoadAuthRequired) => json!({
                "error": { "code": -32000, "message": KIRO_SIGNED_OUT }
            }),
            "session/new" => json!({ "result": { "sessionId": "NEW" } }),
            "session/load" if matches!(behaviour, Prompt::LoadFails) => json!({
                "error": { "code": -32603, "message": "Internal error", "data": "session store unavailable" }
            }),
            "session/load" if matches!(behaviour, Prompt::SlowLoad) => {
                std::thread::sleep(std::time::Duration::from_millis(600));
                json!({ "result": {} })
            }
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

/// Record the methods of every notification the client hands its handler.
fn record_notifications(client: &AcpClient) -> Arc<Mutex<Vec<Value>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    client.set_notification_handler(move |n| sink.lock().unwrap().push(n));
    seen
}

fn methods(seen: &Arc<Mutex<Vec<Value>>>) -> Vec<String> {
    seen.lock()
        .unwrap()
        .iter()
        .filter_map(|n| n["method"].as_str().map(str::to_string))
        .collect()
}

/// Wait until the agent has had `n` requests of `method`.
fn wait_for(agent: &MockAgent, method: &str, n: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while agent.count(method) < n {
        assert!(std::time::Instant::now() < deadline, "no {method} arrived");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn internal_error_with_no_output_restarts_reloads_and_resends_once() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::DispatchFailure, Prompt::Ok]);
    let client = agent.client();
    let seen = record_notifications(&client);

    let id = send(&client).expect("recovered by the restart");

    assert_eq!(id, "S1", "the reloaded session, not a fresh one");
    assert_eq!(agent.count("session/prompt"), 2);
    assert_eq!(agent.connections(), 2);
    assert_eq!(agent.count("session/load"), 1);
    assert_eq!(agent.count("session/new"), 0);
    // The old reader exits as superseded and reports nothing, so the
    // restart says it is tearing a live agent down (its pending prompts
    // to the user are withdrawn) and, having reconnected, nothing more.
    let methods = methods(&seen);
    assert_eq!(
        methods
            .iter()
            .filter(|m| *m == "_kage/agent_restarting")
            .count(),
        1,
        "{methods:?}"
    );
    assert!(
        !methods.iter().any(|m| m == "_kage/agent_disconnected"),
        "{methods:?}"
    );
}

#[test]
fn a_session_that_wont_reload_after_the_restart_is_reported_not_replaced() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::DispatchFailure, Prompt::LoadFails]);
    let client = agent.client();
    let seen = record_notifications(&client);

    let err = send(&client).unwrap_err();

    match err.downcast_ref::<PromptRecoveryError>() {
        Some(PromptRecoveryError::AgentUnreachable { detail }) => {
            assert!(detail.contains("session store unavailable"), "{detail}")
        }
        None => panic!("expected AgentUnreachable, got {err}"),
    }
    assert_eq!(agent.count("session/load"), 1);
    assert_eq!(agent.count("session/new"), 0, "no fresh session");
    assert_eq!(
        agent.count("session/prompt"),
        1,
        "not resent without its context"
    );
    assert!(
        !methods(&seen).iter().any(|m| m == "_kage/session_migrated"),
        "no window is moved to an empty session"
    );
}

#[test]
fn a_stop_pressed_during_the_automatic_restart_is_not_resent() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::DispatchFailure, Prompt::SlowLoad]);
    let client = Arc::new(agent.client());

    let sender = {
        let client = client.clone();
        std::thread::spawn(move || send(&client))
    };
    // Attempt 1 has failed and the restart is reloading the session: no
    // turn is open, so only the per-session stop count can catch this.
    wait_for(&agent, "session/load", 1);
    let _ = client.cancel_session("S1");

    let id = sender
        .join()
        .unwrap()
        .expect("a stopped turn ends as a Stop does");
    assert_eq!(id, "S1");
    assert_eq!(
        agent.count("session/prompt"),
        1,
        "the stopped prompt was resent"
    );
}

#[test]
fn a_stop_pressed_during_the_transport_restart_is_not_resent() {
    isolate_user_dirs();
    // Connection 0 drops the prompt (the agent died mid-turn); the restart
    // reconnects and reloads slowly, and the user stops meanwhile.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let prompts = Arc::new(AtomicUsize::new(0));
    let loads = Arc::new(AtomicUsize::new(0));
    {
        let (prompts, loads) = (prompts.clone(), loads.clone());
        std::thread::spawn(move || {
            for (index, stream) in listener.incoming().enumerate() {
                let Ok(stream) = stream else { return };
                let (prompts, loads) = (prompts.clone(), loads.clone());
                std::thread::spawn(move || {
                    let mut writer = stream.try_clone().unwrap();
                    for line in BufReader::new(stream).lines() {
                        let Ok(line) = line else { return };
                        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                            continue;
                        };
                        if msg.get("id").is_none() {
                            continue;
                        }
                        let reply = match msg["method"].as_str().unwrap_or("") {
                            "session/prompt" => {
                                prompts.fetch_add(1, Ordering::SeqCst);
                                if index == 0 {
                                    return; // closes the connection
                                }
                                json!({ "stopReason": "end_turn" })
                            }
                            "session/load" => {
                                loads.fetch_add(1, Ordering::SeqCst);
                                std::thread::sleep(std::time::Duration::from_millis(600));
                                json!({})
                            }
                            _ => json!({}),
                        };
                        let reply = json!({ "jsonrpc": "2.0", "id": msg["id"], "result": reply });
                        if writeln!(writer, "{reply}").is_err() {
                            return;
                        }
                    }
                });
            }
        });
    }
    let client = Arc::new(AcpClient::new(AcpConnectionMode::Remote {
        host: "127.0.0.1".to_string(),
        port,
    }));
    client.connect().expect("connect to the mock agent");

    let sender = {
        let client = client.clone();
        std::thread::spawn(move || send(&client))
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while loads.load(Ordering::SeqCst) < 1 {
        assert!(
            std::time::Instant::now() < deadline,
            "no session/load arrived"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let _ = client.cancel_session("S1");

    let id = sender
        .join()
        .unwrap()
        .expect("a stopped turn ends as a Stop does");
    assert_eq!(id, "S1");
    assert_eq!(
        prompts.load(Ordering::SeqCst),
        1,
        "the stopped prompt was resent"
    );
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

#[test]
fn a_background_prompt_in_flight_blocks_the_restart() {
    // An automation's or inline-assist's prompt sets no in-flight user
    // prompt; the restart would still kill it mid-work.
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::DispatchFailure, Prompt::Ok]);
    let client = agent.client();
    client
        .background_prompt_sessions
        .lock()
        .unwrap()
        .insert("SCRATCH".to_string());

    let err = send(&client).unwrap_err();

    assert_eq!(err.downcast_ref::<AgentRpcError>().unwrap().code, -32603);
    assert_eq!(agent.count("session/prompt"), 1);
    assert_eq!(agent.connections(), 1);
}

#[test]
fn a_restart_would_interrupt_any_turn_on_a_live_agent() {
    // Reconnect's guard: any turn counts, the asking window's own included
    // (a stale Reconnect clicked while its next prompt runs).
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::Ok]);
    let client = agent.client();
    assert!(!client.restart_would_interrupt_work());

    client.set_in_flight_prompt("S1", "a prompt running its tools");
    assert!(client.restart_would_interrupt_work());
    client.clear_in_flight_prompt("S1");
    assert!(!client.restart_would_interrupt_work());

    // A dead agent has nothing left to cut off: Reconnect goes ahead.
    client.set_in_flight_prompt("S1", "a prompt on a dead agent");
    client.disconnect();
    assert!(!client.restart_would_interrupt_work());
}

// --- The session's own working directory on every recovery reload ---

const PROJECT: &str = "/tmp/original-project";

/// A session created in `PROJECT`, on a client of `agent`.
fn client_with_project_session(agent: &MockAgent) -> (AcpClient, String) {
    let client = agent.client();
    let (id, _) = client
        .create_session(Some(PROJECT.to_string()))
        .expect("session/new");
    assert_eq!(agent.params("session/new")[0]["cwd"], PROJECT);
    (client, id)
}

#[test]
fn the_automatic_restart_reloads_the_session_in_its_own_directory() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::DispatchFailure, Prompt::Ok]);
    let (client, id) = client_with_project_session(&agent);

    client
        .send_chat_streaming_with_recovery(id, "hello".into(), None, None)
        .expect("recovered by the restart");

    let loads = agent.params("session/load");
    assert_eq!(loads.len(), 1);
    assert_eq!(loads[0]["cwd"], PROJECT, "{loads:?}");
}

#[test]
fn the_transport_restart_reloads_the_session_in_its_own_directory() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::ConnectionCloses, Prompt::Ok]);
    let (client, id) = client_with_project_session(&agent);

    client
        .send_chat_streaming_with_recovery(id, "hello".into(), None, None)
        .expect("recovered by the restart");

    let loads = agent.params("session/load");
    assert_eq!(loads.len(), 1);
    assert_eq!(loads[0]["cwd"], PROJECT, "{loads:?}");
}

#[test]
fn a_replacement_session_is_created_in_the_original_directory() {
    isolate_user_dirs();
    // Connection 0 drops the prompt; connection 1 can't load the session,
    // so the ladder replaces it with a fresh one.
    let agent = MockAgent::start(vec![Prompt::ConnectionCloses, Prompt::LoadFails]);
    let (client, id) = client_with_project_session(&agent);

    client
        .send_chat_streaming_with_recovery(id, "hello".into(), None, None)
        .expect("resent on a fresh session");

    let news = agent.params("session/new");
    assert_eq!(news.len(), 2, "{news:?}");
    assert_eq!(news[1]["cwd"], PROJECT, "{news:?}");
}

#[test]
fn a_reload_without_a_directory_uses_the_one_the_session_was_created_in() {
    // Reconnect reloads the window's session on the restarted agent; when
    // its history file names no directory, the one recorded this run holds.
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::Ok]);
    let (client, id) = client_with_project_session(&agent);

    client.restart_connection().expect("restart");
    client
        .load_existing_session(&id, None)
        .expect("session/load");

    assert_eq!(agent.params("session/load")[0]["cwd"], PROJECT);
}

// --- Answers to the agent's requests stay on their connection ---

#[test]
fn a_permission_answer_is_never_written_to_a_restarted_agent() {
    // A restarted agent numbers its requests afresh: an answer meant for
    // the old process's request must not reach the new one, where the same
    // id may be an unrelated request.
    isolate_user_dirs();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    // (connection index, line) of every answer (a line with no method).
    let answers = Arc::new(Mutex::new(Vec::<(usize, Value)>::new()));
    {
        let answers = answers.clone();
        std::thread::spawn(move || {
            for (index, stream) in listener.incoming().enumerate() {
                let Ok(stream) = stream else { return };
                let answers = answers.clone();
                std::thread::spawn(move || {
                    let mut writer = stream.try_clone().unwrap();
                    for line in BufReader::new(stream).lines() {
                        let Ok(line) = line else { return };
                        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                            continue;
                        };
                        if msg.get("method").is_none() {
                            answers.lock().unwrap().push((index, msg));
                            continue;
                        }
                        if msg.get("id").is_none() {
                            continue;
                        }
                        let reply = json!({ "jsonrpc": "2.0", "id": msg["id"], "result": {} });
                        if writeln!(writer, "{reply}").is_err() {
                            return;
                        }
                    }
                });
            }
        });
    }
    let client = AcpClient::new(AcpConnectionMode::Remote {
        host: "127.0.0.1".to_string(),
        port,
    });
    client.connect().expect("connect");
    let old = client.connection_generation();

    client.restart_connection().expect("restart");
    let current = client.connection_generation();
    assert_ne!(old, current);

    assert!(!client
        .send_permission_response_on(old, &json!(5), "allow_always")
        .expect("no write error"));
    assert!(client
        .send_permission_response_on(current, &json!(5), "reject_once")
        .expect("written"));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while answers.lock().unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline, "no answer arrived");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    let answers = answers.lock().unwrap().clone();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0].0, 1, "on the restarted connection");
    assert_eq!(answers[0].1["result"]["outcome"]["optionId"], "reject_once");
}

// --- Whether a failed prompt may be resent ---

fn report(client: &AcpClient, session: &str) -> kage::acp_client::PromptReport {
    client.send_chat_streaming_with_recovery_report(
        session.to_string(),
        "hello".to_string(),
        None,
        None,
    )
}

#[test]
fn a_turn_that_asked_permission_then_failed_is_not_resendable() {
    // The agent asked, the user allowed it, the action ran, then -32603:
    // nothing was displayed, but resending would repeat the action.
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::PermissionThenFailure, Prompt::Ok]);
    let client = agent.client();

    let r = report(&client, "S1");

    assert!(r.result.is_err());
    assert!(
        !r.resendable,
        "a turn that asked for permission was resendable"
    );
    assert_eq!(r.session_id, "S1");
    assert_eq!(
        agent.count("session/prompt"),
        1,
        "no automatic resend either"
    );
}

#[test]
fn a_turn_that_did_nothing_before_still_failing_is_resendable() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::DispatchFailure]);
    let client = agent.client();

    let r = report(&client, "S1");

    assert!(r.result.is_err());
    assert!(r.resendable);
    assert_eq!(
        r.outcome,
        PromptOutcome::Answered,
        "the agent answered both"
    );
    assert_eq!(r.session_id, "S1");
}

#[test]
fn a_turn_that_streamed_text_is_not_resendable() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::ChunkThenFailure]);
    let client = agent.client();

    let r = report(&client, "S1");

    assert!(r.result.is_err());
    assert!(!r.resendable);
}

#[test]
fn what_an_automatic_resend_did_counts_for_the_whole_turn() {
    // The connection broke on the first attempt; the resend streamed text
    // and then failed.
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::ConnectionCloses, Prompt::ChunkThenFailure]);
    let client = agent.client();

    let r = report(&client, "S1");

    assert!(r.result.is_err());
    assert!(!r.resendable);
}

#[test]
fn a_turn_stopped_while_recovering_is_not_resendable() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::DispatchFailure, Prompt::SlowLoad]);
    let client = Arc::new(agent.client());
    let sender = {
        let client = client.clone();
        std::thread::spawn(move || report(&client, "S1"))
    };
    wait_for(&agent, "session/load", 1);
    let _ = client.cancel_session("S1");

    let r = sender.join().unwrap();
    assert!(r.result.is_ok(), "a stopped turn ends as a Stop does");
    assert!(!r.resendable);
}

// --- Restarts never cut off work admitted meanwhile ---

#[test]
fn reconnect_does_not_admit_a_prompt_between_its_idle_check_and_the_teardown() {
    // Another window sends the moment Reconnect has found the agent idle.
    // Without a gate the prompt reaches the old agent, the teardown cuts
    // it off, and the transport recovery resends work it may have done.
    isolate_user_dirs();
    let agent = Arc::new(MockAgent::start(vec![Prompt::Hang, Prompt::Ok]));
    let client = Arc::new(agent.client());

    type Sender = std::thread::JoinHandle<anyhow::Result<String>>;
    let sender: Arc<Mutex<Option<Sender>>> = Arc::new(Mutex::new(None));
    {
        let (weak, agent, sender) = (Arc::downgrade(&client), agent.clone(), sender.clone());
        client.set_restart_idle_check_hook(Some(Box::new(move || {
            let client = weak.upgrade().expect("client alive");
            let handle = std::thread::spawn(move || {
                client.send_chat_streaming_with_recovery(
                    "S2".to_string(),
                    "other window".to_string(),
                    None,
                    None,
                )
            });
            *sender.lock().unwrap() = Some(handle);
            // Give the prompt every chance to reach the old agent before
            // the teardown.
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(800);
            while agent.count("session/prompt") == 0 && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        })));
    }

    assert_eq!(
        client.restart_if_idle().expect("restart"),
        kage::acp_client::RestartIfIdle::Restarted
    );
    client.set_restart_idle_check_hook(None);
    let handle = sender.lock().unwrap().take().expect("the hook ran");
    handle
        .join()
        .unwrap()
        .expect("the other window's prompt went through");

    assert_eq!(
        agent.count_on(0, "session/prompt"),
        0,
        "admitted to the agent being torn down"
    );
    assert_eq!(agent.count("session/prompt"), 1, "the prompt was resent");
}

#[test]
fn reconnect_refuses_while_a_background_prompt_runs() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::Hang, Prompt::Ok]);
    let client = Arc::new(agent.client());
    {
        let client = client.clone();
        std::thread::spawn(move || client.send_chat_streaming_background("BG", "steer", false));
    }
    wait_for(&agent, "session/prompt", 1);

    assert_eq!(
        client.restart_if_idle().expect("no restart attempted"),
        kage::acp_client::RestartIfIdle::Busy
    );
    assert_eq!(agent.connections(), 1);
}

#[test]
fn the_automatic_restart_waits_for_a_turn_queued_on_the_same_session() {
    // Window A's turn on S1 fails before doing anything, while window B's
    // prompt on S1 is queued behind it. The restart would cut B's turn
    // off mid-work and its recovery would resend it.
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::SlowFailThenSlowOk, Prompt::Ok]);
    let client = Arc::new(agent.client());

    let a = {
        let client = client.clone();
        std::thread::spawn(move || send(&client))
    };
    wait_for(&agent, "session/prompt", 1);
    let b = {
        let client = client.clone();
        std::thread::spawn(move || {
            client.send_chat_streaming_with_recovery(
                "S1".to_string(),
                "window B".to_string(),
                None,
                None,
            )
        })
    };

    let a_err = a.join().unwrap().unwrap_err();
    b.join().unwrap().expect("B's turn completed");

    assert_eq!(a_err.downcast_ref::<AgentRpcError>().unwrap().code, -32603);
    assert_eq!(
        agent.connections(),
        1,
        "the agent was restarted under B's turn"
    );
    assert_eq!(agent.count("session/prompt"), 2, "B's prompt was resent");
}

// --- Recovery never resends a turn that already did something ---

#[test]
fn a_dropped_connection_after_a_permission_request_is_not_resent() {
    // The agent asked to run `rm -rf build/`, the user allowed it, it ran,
    // and the agent crashed before replying. Resending would run it again.
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::PermissionThenConnectionCloses, Prompt::Ok]);
    let client = agent.client();

    let r = report(&client, "S1");

    assert!(r.result.is_err(), "the turn is reported failed");
    assert!(!r.resendable);
    assert_eq!(
        agent.count("session/prompt"),
        1,
        "the backend resent a turn that had asked for permission"
    );
}

#[test]
fn a_corrupted_session_after_a_tool_call_is_not_resent_on_a_fresh_one() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::ToolCallThenPanic, Prompt::Ok]);
    let client = agent.client();

    let r = report(&client, "S1");

    assert!(r.result.is_err(), "the turn is reported failed");
    assert!(!r.resendable);
    assert_eq!(r.session_id, "S1", "moved to a fresh session anyway");
    assert_eq!(
        agent.count("session/prompt"),
        1,
        "the backend resent a turn that had run a tool"
    );
    assert_eq!(agent.count("session/new"), 0);
}

// --- No recovery restart cuts off another window's turn ---

/// `(connection, sessionId)` of every `session/prompt`.
type PromptLog = Arc<Mutex<Vec<(usize, String)>>>;

/// A mock whose first connection runs turn "B" for 1.5 s (a tool call
/// first) and fails turn "A" after 300 ms with `a_error`; later
/// connections answer every prompt at once. Returns the port and the
/// `(connection, sessionId)` of every `session/prompt`.
fn start_two_turn_agent(a_error: Value) -> (u16, PromptLog, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let prompts: PromptLog = Arc::new(Mutex::new(Vec::new()));
    let connections = Arc::new(AtomicUsize::new(0));
    {
        let (prompts, connections) = (prompts.clone(), connections.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let index = connections.fetch_add(1, Ordering::SeqCst);
                let (prompts, a_error) = (prompts.clone(), a_error.clone());
                std::thread::spawn(move || {
                    let writer = Arc::new(Mutex::new(stream.try_clone().unwrap()));
                    let write = |w: &Arc<Mutex<std::net::TcpStream>>, v: Value| {
                        let _ = writeln!(w.lock().unwrap(), "{v}");
                    };
                    for line in BufReader::new(stream).lines() {
                        let Ok(line) = line else { return };
                        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                            continue;
                        };
                        if msg.get("id").is_none() {
                            continue;
                        }
                        let id = msg["id"].clone();
                        let method = msg["method"].as_str().unwrap_or("").to_string();
                        let sid = msg["params"]["sessionId"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        if method == "session/prompt" {
                            prompts.lock().unwrap().push((index, sid.clone()));
                        }
                        let reply = match (method.as_str(), index, sid.as_str()) {
                            ("session/prompt", 0, "B") => {
                                write(
                                    &writer,
                                    json!({
                                        "jsonrpc": "2.0", "method": "session/update",
                                        "params": { "sessionId": "B", "update": {
                                            "sessionUpdate": "tool_call", "toolCallId": "t1",
                                            "title": "Delete build/", "status": "in_progress"
                                        } }
                                    }),
                                );
                                let w = writer.clone();
                                std::thread::spawn(move || {
                                    std::thread::sleep(std::time::Duration::from_millis(1500));
                                    write(
                                        &w,
                                        json!({ "jsonrpc": "2.0", "id": id,
                                        "result": { "stopReason": "end_turn" } }),
                                    );
                                });
                                continue;
                            }
                            ("session/prompt", 0, "A") => {
                                let (w, e) = (writer.clone(), a_error.clone());
                                std::thread::spawn(move || {
                                    std::thread::sleep(std::time::Duration::from_millis(300));
                                    write(&w, json!({ "jsonrpc": "2.0", "id": id, "error": e }));
                                });
                                continue;
                            }
                            ("session/prompt", _, _) => json!({ "stopReason": "end_turn" }),
                            ("session/new", _, _) => json!({ "sessionId": "NEW" }),
                            _ => json!({}),
                        };
                        write(
                            &writer,
                            json!({ "jsonrpc": "2.0", "id": id, "result": reply }),
                        );
                    }
                });
            }
        });
    }
    (port, prompts, connections)
}

/// Turn B runs on connection 0; turn A fails with `a_error` meanwhile.
/// Returns A's report, B's result, every prompt and the connection count.
fn run_two_turns(
    a_error: Value,
) -> (
    kage::acp_client::PromptReport,
    anyhow::Result<String>,
    Vec<(usize, String)>,
    usize,
) {
    isolate_user_dirs();
    let (port, prompts, connections) = start_two_turn_agent(a_error);
    let client = Arc::new(AcpClient::new(AcpConnectionMode::Remote {
        host: "127.0.0.1".to_string(),
        port,
    }));
    client.connect().expect("connect to the mock agent");

    let b = {
        let client = client.clone();
        std::thread::spawn(move || {
            client.send_chat_streaming_with_recovery(
                "B".to_string(),
                "window B".to_string(),
                None,
                None,
            )
        })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while prompts.lock().unwrap().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "B never reached the agent"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let a = report(&client, "A");
    let b = b.join().unwrap();
    let prompts = prompts.lock().unwrap().clone();
    (a, b, prompts, connections.load(Ordering::SeqCst))
}

fn prompts_on(prompts: &[(usize, String)], session: &str) -> usize {
    prompts.iter().filter(|(_, s)| s == session).count()
}

#[test]
fn a_corrupted_session_restart_waits_for_another_windows_turn() {
    let (a, b, prompts, connections) = run_two_turns(json!({
        "code": -32603, "message": "Internal error",
        "data": "thread 'tokio-runtime-worker' panicked at src/agent.rs:42"
    }));

    assert!(a.result.is_err(), "A is reported failed, not recovered");
    b.expect("B's turn completed on the agent it started on");
    assert_eq!(connections, 1, "the agent was restarted under B's turn");
    assert_eq!(prompts_on(&prompts, "B"), 1, "B's prompt was sent twice");
}

#[test]
fn a_transport_restart_waits_for_another_windows_turn() {
    // A's request timed out on a live agent (the agent never answered it):
    // restarting would cut B off mid-tool.
    let (a, b, prompts, connections) = run_two_turns(json!({
        "code": -32603, "message": "Internal error",
        "data": "Timeout waiting for response from the model"
    }));

    assert!(a.result.is_err(), "A is reported failed, not recovered");
    b.expect("B's turn completed on the agent it started on");
    assert_eq!(connections, 1, "the agent was restarted under B's turn");
    assert_eq!(prompts_on(&prompts, "B"), 1, "B's prompt was sent twice");
}

#[test]
fn a_fresh_session_restart_for_an_image_error_waits_for_another_windows_turn() {
    // The image-error reset in `send_message_streaming`: a prompt with an
    // attachment failed; another window's turn is still running.
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::Hang, Prompt::Ok]);
    let client = Arc::new(agent.client());
    {
        let client = client.clone();
        std::thread::spawn(move || {
            client.send_chat_streaming_with_recovery("B".to_string(), "b".to_string(), None, None)
        });
    }
    wait_for(&agent, "session/prompt", 1);

    let fresh = client
        .restart_with_fresh_session_if_idle(Some("S1"))
        .expect("no restart attempted");

    assert_eq!(fresh, None, "a fresh session was made by restarting");
    assert_eq!(
        agent.connections(),
        1,
        "the agent was restarted under B's turn"
    );
}

#[test]
fn a_fresh_session_restart_for_an_image_error_goes_ahead_when_idle() {
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::Ok]);
    let client = agent.client();

    let fresh = client
        .restart_with_fresh_session_if_idle(Some("S1"))
        .expect("restart");

    assert_eq!(fresh.as_deref(), Some("NEW"));
    assert_eq!(agent.connections(), 2);
}

#[test]
fn two_turns_cut_off_by_a_crash_share_one_restart() {
    // Both turns were on a connection that died before either did
    // anything: each one's recovery restarts or shares the restart, and
    // both are resent once on the new agent.
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::HangThenCloseOnSecond, Prompt::Ok]);
    let client = Arc::new(agent.client());
    let turn = |session: &'static str| {
        let client = client.clone();
        std::thread::spawn(move || {
            client.send_chat_streaming_with_recovery(
                session.to_string(),
                "x".to_string(),
                None,
                None,
            )
        })
    };
    let a = turn("A");
    wait_for(&agent, "session/prompt", 1);
    let b = turn("B");

    a.join().unwrap().expect("A recovered");
    b.join().unwrap().expect("B recovered");
    assert_eq!(agent.count_on(0, "session/prompt"), 2);
    assert_eq!(
        agent.count("session/prompt"),
        4,
        "each turn resent exactly once"
    );
}

// --- A failure whose outcome is unknown is never resendable ---

#[test]
fn a_timed_out_turn_whose_restart_was_refused_is_not_resendable() {
    // Turn A gets no reply and no activity at all: the agent may still be
    // running it (its tool call can come later, after A's progress record
    // is gone). A background prompt that went out after A keeps the
    // recovery from restarting. Reconnect must not resend A once the
    // background prompt is done: that ran A's action twice.
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::Hang, Prompt::Ok]);
    let client = Arc::new(agent.client());
    client.set_prompt_idle_timeout(Some(std::time::Duration::from_millis(1600)));

    let a = {
        let client = client.clone();
        std::thread::spawn(move || report(&client, "S1"))
    };
    wait_for(&agent, "session/prompt", 1);
    std::thread::sleep(std::time::Duration::from_millis(700));
    {
        let client = client.clone();
        std::thread::spawn(move || client.send_chat_streaming_background("BG", "steer", false));
    }
    wait_for(&agent, "session/prompt", 2);

    let r = a.join().unwrap();

    assert!(r.result.is_err(), "A is reported failed");
    assert_eq!(
        agent.connections(),
        1,
        "restarted under the background prompt"
    );
    assert_eq!(agent.count("session/prompt"), 2, "A was resent");
    assert!(
        !r.resendable,
        "a turn that timed out on a live agent was resendable"
    );
    assert_eq!(r.outcome, PromptOutcome::Unknown);
}

#[test]
fn a_dropped_connection_whose_resend_got_an_agent_error_is_not_resendable() {
    // The first attempt was cut off after it was written: whatever the
    // agent did with it is unknown, even though the resend was answered
    // with an error and nothing showed.
    isolate_user_dirs();
    let agent = MockAgent::start(vec![Prompt::ConnectionCloses, Prompt::AuthRequired]);
    let client = agent.client();

    let r = report(&client, "S1");

    assert!(r.result.is_err());
    assert_eq!(agent.count("session/prompt"), 2, "resent once by recovery");
    assert!(!r.resendable, "an unknown first attempt was resendable");
}

// ---------------------------------------------------------------------------
// A signed-out agent refusing to set up a session (`initialize`,
// `session/new`, `session/load`) reaches the window as `auth_required`, like
// a refused prompt, so the window can offer Sign in.
// ---------------------------------------------------------------------------

/// A mock app whose config names a Local Kiro connection (`local_kiro`; what
/// decides the sign-in message) or an unnamed Remote one, while its client
/// talks to `agent` over TCP either way.
fn session_app(
    agent: &MockAgent,
    local_kiro: bool,
    connect: bool,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    tauri::WebviewWindow<tauri::test::MockRuntime>,
) {
    isolate_user_dirs();
    let mut config = kage::config::Config::default();
    {
        let conn = config
            .acp
            .active_connection_mut()
            .expect("the default config has a connection");
        if local_kiro {
            conn.preset_id = Some("kiro".into());
            conn.mode = kage::config::AcpMode::Local {
                spawn_command: "kiro-cli acp".into(),
            };
        } else {
            conn.preset_id = None;
            conn.mode = kage::config::AcpMode::Remote {
                host: "127.0.0.1".into(),
                port: agent.port,
                timeout_ms: 5_000,
            };
        }
    }
    let client = Arc::new(AcpClient::new(AcpConnectionMode::Remote {
        host: "127.0.0.1".into(),
        port: agent.port,
    }));
    if connect {
        client.connect().expect("connect to the mock agent");
    }
    let kage::state::ManagedState {
        acp_handles,
        ui_state,
        child_processes,
        feature_services,
    } = kage::state::build_managed_state(Arc::new(Mutex::new(config)), client, false);
    let app = tauri::test::mock_builder()
        .manage(acp_handles)
        .manage(ui_state)
        .manage(child_processes)
        .manage(feature_services)
        .invoke_handler(kage::commands::registry::invoke_handler())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app must build");
    let window = tauri::WebviewWindowBuilder::new(&app, "floating", Default::default())
        .build()
        .expect("mock floating window must build");
    (app, window)
}

/// Invoke `cmd` as the window would; an error is the JSON it receives.
fn invoke(
    window: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    args: Value,
) -> Result<Value, Value> {
    tauri::test::get_ipc_response(
        window,
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

fn assert_kiro_sign_in(err: &Value) {
    assert_eq!(err["kind"], "auth_required", "{err}");
    assert_eq!(err["key"], "errors.agent.auth_required_kiro", "{err}");
    let message = err["message"].as_str().unwrap_or_default();
    assert!(message.contains("kiro-cli login"), "{err}");
    assert!(
        !message.contains("Failed to create session"),
        "the old passthrough text reached the window: {err}"
    );
}

#[test]
fn a_signed_out_agent_refusing_session_new_reaches_the_window_as_auth_required() {
    let agent = MockAgent::start(vec![Prompt::NewAuthRequired]);
    let (_app, window) = session_app(&agent, true, true);

    let err = invoke(&window, "switch_acp_session", json!({ "sessionId": null })).unwrap_err();

    assert_kiro_sign_in(&err);
    assert_eq!(agent.count("session/new"), 1);
}

#[test]
fn a_signed_out_agent_refusing_initialize_reaches_the_window_as_auth_required() {
    let agent = MockAgent::start(vec![Prompt::InitAuthRequired]);
    let (_app, window) = session_app(&agent, true, true);

    let err = invoke(&window, "switch_acp_session", json!({ "sessionId": null })).unwrap_err();

    assert_kiro_sign_in(&err);
    assert_eq!(agent.count("session/new"), 0);
}

#[test]
fn a_signed_out_agent_refusing_the_restart_before_a_session_reaches_the_window_as_auth_required() {
    // Not connected: the command restarts the agent first, and that
    // restart's `initialize` is what's refused.
    let agent = MockAgent::start(vec![Prompt::InitAuthRequired]);
    let (_app, window) = session_app(&agent, true, false);

    let err = invoke(&window, "switch_acp_session", json!({ "sessionId": null })).unwrap_err();

    assert_kiro_sign_in(&err);
    assert_eq!(agent.count("session/new"), 0);
}

#[test]
fn a_signed_out_agent_refusing_session_load_reaches_the_window_as_auth_required() {
    let agent = MockAgent::start(vec![Prompt::LoadAuthRequired]);
    let (_app, window) = session_app(&agent, true, true);

    let err = invoke(&window, "switch_acp_session", json!({ "sessionId": "S1" })).unwrap_err();

    assert_kiro_sign_in(&err);
}

#[test]
fn another_agent_refusing_session_new_gets_the_generic_sign_in_message() {
    let agent = MockAgent::start(vec![Prompt::NewAuthRequired]);
    let (_app, window) = session_app(&agent, false, true);

    let err = invoke(&window, "switch_acp_session", json!({ "sessionId": null })).unwrap_err();

    assert_eq!(err["kind"], "auth_required", "{err}");
    assert_eq!(err["key"], "errors.agent.auth_required", "{err}");
}

#[test]
fn a_session_new_failure_that_is_not_a_sign_in_keeps_its_text() {
    let agent = MockAgent::start(vec![Prompt::NewFails]);
    let (_app, window) = session_app(&agent, true, true);

    let err = invoke(&window, "switch_acp_session", json!({ "sessionId": null })).unwrap_err();

    assert_ne!(err["kind"], "auth_required", "{err}");
    let message = err["message"].as_str().unwrap_or_default();
    assert!(message.starts_with("Failed to create session:"), "{err}");
    assert!(message.contains("disk full"), "{err}");
}

// ---------------------------------------------------------------------------
// Inline assist's "open in chat" (`open_chat_with_message`) against a
// signed-out agent: the prompt never goes out, and main hears so as
// `auth_required` naming its turn and `resendable` (nothing of it got
// through), so main's Sign in sends it after the sign-in, once.
// ---------------------------------------------------------------------------

/// `session_app` plus the main window, and every MESSAGE_ERROR payload.
fn open_chat_app(
    agent: &MockAgent,
    connect: bool,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    tauri::WebviewWindow<tauri::test::MockRuntime>,
    Arc<Mutex<Vec<Value>>>,
) {
    use tauri::Listener;
    let (app, _floating) = session_app(agent, true, connect);
    let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("mock main window must build");
    let errors = Arc::new(Mutex::new(Vec::new()));
    let sink = errors.clone();
    app.listen_any("message_error", move |event| {
        sink.lock()
            .unwrap()
            .push(serde_json::from_str(event.payload()).unwrap());
    });
    (app, main, errors)
}

/// The first MESSAGE_ERROR, within a few seconds.
fn first_error(errors: &Arc<Mutex<Vec<Value>>>) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(e) = errors.lock().unwrap().first() {
            return e.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no message_error reached main"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn assert_unsent_sign_in(err: &Value, session_id: Value) {
    assert_kiro_sign_in(err);
    assert_eq!(err["turnId"], "assist-turn-1", "{err}");
    assert_eq!(err["sessionId"], session_id, "{err}");
    assert_eq!(
        err["resendable"], true,
        "nothing of the prompt went out: {err}"
    );
}

#[test]
fn open_in_chat_refused_by_session_new_reaches_main_naming_its_turn_as_resendable() {
    let agent = MockAgent::start(vec![Prompt::NewAuthRequired]);
    let (_app, main, errors) = open_chat_app(&agent, true);

    invoke(
        &main,
        "open_chat_with_message",
        json!({ "sessionId": null, "message": "explain this", "turnId": "assist-turn-1" }),
    )
    .expect("the command itself answers at once");

    assert_unsent_sign_in(&first_error(&errors), Value::Null);
    assert_eq!(agent.count("session/prompt"), 0);
}

#[test]
fn open_in_chat_refused_by_initialize_reaches_main_naming_its_turn_as_resendable() {
    // Not connected: the command connects first; the agent refuses its
    // handshake.
    let agent = MockAgent::start(vec![Prompt::InitAuthRequired]);
    let (_app, main, errors) = open_chat_app(&agent, false);

    invoke(
        &main,
        "open_chat_with_message",
        json!({ "sessionId": null, "message": "explain this", "turnId": "assist-turn-1" }),
    )
    .expect("the command itself answers at once");

    assert_unsent_sign_in(&first_error(&errors), Value::Null);
    assert_eq!(agent.count("session/new"), 0);
    assert_eq!(agent.count("session/prompt"), 0);
}

#[test]
fn open_in_chat_on_mains_session_refused_by_initialize_reaches_main_as_auth_required() {
    // Main has a session, so none is created: the handshake of the
    // connection the command makes is the agent's first chance to refuse,
    // and the prompt must not go out on a connection it refused.
    let agent = MockAgent::start(vec![Prompt::InitAuthRequired]);
    let (_app, main, errors) = open_chat_app(&agent, false);

    invoke(
        &main,
        "open_chat_with_message",
        json!({ "sessionId": "S1", "message": "explain this", "turnId": "assist-turn-1" }),
    )
    .expect("the command itself answers at once");

    assert_unsent_sign_in(&first_error(&errors), json!("S1"));
    assert_eq!(agent.count("initialize"), 1);
    assert_eq!(agent.count("session/prompt"), 0);
}

/// Every payload of `event` the app emits, in order.
fn record_event(app: &tauri::App<tauri::test::MockRuntime>, event: &str) -> Arc<Mutex<Vec<Value>>> {
    use tauri::Listener;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    app.listen_any(event, move |e| {
        sink.lock()
            .unwrap()
            .push(serde_json::from_str(e.payload()).unwrap());
    });
    seen
}

/// Wait until `agent` has had `n` requests of `method`.
fn wait_for_count(agent: &MockAgent, method: &str, n: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while agent.count(method) < n {
        assert!(
            std::time::Instant::now() < deadline,
            "{method} never reached the agent"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn open_in_chat_tells_main_the_session_its_prompt_is_sent_on() {
    // Main's pin is S1 (main may show another session meanwhile: one being
    // selected, or one it could not set up): `initial_message` names S1,
    // the session the prompt goes to, so main records the turn there and
    // shows it.
    let agent = MockAgent::start(vec![Prompt::Ok]);
    let (app, main, _errors) = open_chat_app(&agent, true);
    let announced = record_event(&app, "initial_message");

    invoke(
        &main,
        "open_chat_with_message",
        json!({ "sessionId": "S1", "message": "explain this", "turnId": "assist-turn-1" }),
    )
    .expect("the command itself answers at once");

    assert_eq!(
        *announced.lock().unwrap(),
        vec![json!({ "message": "explain this", "turnId": "assist-turn-1", "sessionId": "S1" })]
    );
    wait_for_count(&agent, "session/prompt", 1);
    assert_eq!(agent.params("session/prompt")[0]["sessionId"], "S1");
    assert_eq!(agent.count("session/new"), 0);
}

#[test]
fn open_in_chat_with_no_session_tells_main_none_and_then_the_one_made() {
    // No pin (a blank id is none): `initial_message` names no session, and
    // `initial_message_session` the one made for the prompt, which is
    // where the prompt goes.
    let agent = MockAgent::start(vec![Prompt::Ok]);
    let (app, main, _errors) = open_chat_app(&agent, true);
    let announced = record_event(&app, "initial_message");
    let made = record_event(&app, "initial_message_session");

    invoke(
        &main,
        "open_chat_with_message",
        json!({ "sessionId": " ", "message": "explain this", "turnId": "assist-turn-1" }),
    )
    .expect("the command itself answers at once");

    assert_eq!(
        *announced.lock().unwrap(),
        vec![json!({ "message": "explain this", "turnId": "assist-turn-1", "sessionId": null })]
    );
    wait_for_count(&agent, "session/prompt", 1);
    assert_eq!(agent.params("session/prompt")[0]["sessionId"], "NEW");
    assert_eq!(*made.lock().unwrap(), vec![json!({ "sessionId": "NEW" })]);
}
