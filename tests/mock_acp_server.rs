//! A scripted mock ACP agent over TCP.
//!
//! Integration tests drive a real `AcpClient` (remote mode) against it:
//! `tests/agent_calls_test.rs` includes this file as a module. Each
//! `session/prompt` plays the next scripted [`Turn`]: reply chunks, a tool
//! call the agent starts on its own, a permission request it waits on, a
//! pause, a wait for `session/cancel`, or a dropped connection. Every line
//! the client sends is recorded for the test to check.
//!
//! It also runs by hand, as an agent that answers every prompt with "ok":
//! `cargo test --test mock_acp_server -- --ignored --nocapture`, then
//! point a remote connection at 127.0.0.1:8765.

#![allow(dead_code)] // Each including test crate uses part of it.

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// One thing the agent does while it answers a prompt.
pub enum Step {
    /// Stream these `agent_message_chunk` updates.
    Chunks(Vec<String>),
    /// Report a `tool_call` it started without asking the client.
    ToolCall,
    /// Send `session/request_permission` and wait (up to 5 s) for the
    /// client's answer.
    AskPermission,
    /// Pause. A `session/cancel` ends the pause, and the turn.
    Sleep(Duration),
    /// Wait for `session/cancel` (at most this long). The turn then ends
    /// as cancelled.
    WaitForCancel(Duration),
    /// Close the connection without answering.
    Hangup,
}

/// The agent's answer to one `session/prompt`. After its steps, the
/// prompt is answered `end_turn`, or `cancelled` if the client cancelled.
pub struct Turn(pub Vec<Step>);

impl Turn {
    /// A whole reply in one chunk.
    pub fn reply(text: &str) -> Self {
        Turn(vec![Step::Chunks(vec![text.to_string()])])
    }
}

#[derive(Default)]
struct Shared {
    /// Every message the client sent, in order.
    received: Vec<Value>,
    /// Sessions the client sent `session/cancel` for.
    cancelled: Vec<String>,
    turns: VecDeque<Turn>,
    sessions: usize,
    permission_requests: usize,
    /// Open connections, so a hang-up or drop can close them.
    streams: Vec<TcpStream>,
}

struct Inner {
    state: Mutex<Shared>,
    changed: Condvar,
    /// Where `session/new` writes the session's files, as an agent would.
    sessions_dir: Option<PathBuf>,
    stop: AtomicBool,
}

impl Inner {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn is_cancelled(&self, session_id: &str) -> bool {
        self.lock().cancelled.iter().any(|s| s == session_id)
    }

    /// Wait until `session_id` is cancelled or `max` passes. Returns
    /// whether it was cancelled.
    fn wait_cancel(&self, session_id: &str, max: Duration) -> bool {
        let deadline = Instant::now() + max;
        let mut st = self.lock();
        loop {
            if st.cancelled.iter().any(|s| s == session_id) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline || self.stop.load(Ordering::SeqCst) {
                return false;
            }
            st = self
                .changed
                .wait_timeout(st, deadline - now)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    fn hang_up(&self) {
        for s in self.lock().streams.drain(..) {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

/// A mock agent listening on 127.0.0.1. Dropping it closes everything.
pub struct ScriptedAgent {
    pub port: u16,
    inner: Arc<Inner>,
}

impl ScriptedAgent {
    /// Listen on an ephemeral port and play `turns`, one per prompt (a
    /// plain "ok" once they run out).
    pub fn start(turns: Vec<Turn>, sessions_dir: Option<PathBuf>) -> Self {
        Self::start_on("127.0.0.1:0", turns, sessions_dir)
    }

    pub fn start_on(addr: &str, turns: Vec<Turn>, sessions_dir: Option<PathBuf>) -> Self {
        let listener = TcpListener::bind(addr).expect("bind the mock agent");
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let inner = Arc::new(Inner {
            state: Mutex::new(Shared {
                turns: turns.into(),
                ..Default::default()
            }),
            changed: Condvar::new(),
            sessions_dir,
            stop: AtomicBool::new(false),
        });
        let accept = inner.clone();
        thread::spawn(move || {
            while !accept.stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        accept.lock().streams.push(stream.try_clone().unwrap());
                        let conn = accept.clone();
                        thread::spawn(move || serve(conn, stream));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return,
                }
            }
        });
        Self { port, inner }
    }

    /// Every message the client sent so far.
    pub fn received(&self) -> Vec<Value> {
        self.inner.lock().received.clone()
    }

    /// The `params` of every request with this method.
    pub fn params_of(&self, method: &str) -> Vec<Value> {
        self.received()
            .into_iter()
            .filter(|m| m["method"] == method)
            .map(|m| m["params"].clone())
            .collect()
    }

    /// Sessions the client cancelled.
    pub fn cancelled(&self) -> Vec<String> {
        self.inner.lock().cancelled.clone()
    }

    /// The client's answers to permission requests (`result` of each).
    pub fn permission_answers(&self) -> Vec<Value> {
        self.received()
            .into_iter()
            .filter(|m| m.get("method").is_none() && m["id"].as_str().is_some())
            .map(|m| m["result"].clone())
            .collect()
    }

    /// Wait until `done` holds for what was received, at most `max`.
    pub fn wait_until(&self, max: Duration, done: impl Fn(&ScriptedAgent) -> bool) -> bool {
        let deadline = Instant::now() + max;
        while Instant::now() < deadline {
            if done(self) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        done(self)
    }
}

impl Drop for ScriptedAgent {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        self.inner.changed.notify_all();
        self.inner.hang_up();
    }
}

type Out = Arc<Mutex<TcpStream>>;

fn send(out: &Out, msg: &Value) {
    let mut s = out.lock().unwrap_or_else(|p| p.into_inner());
    let _ = writeln!(s, "{msg}");
    let _ = s.flush();
}

fn respond(out: &Out, id: &Value, result: Value) {
    send(
        out,
        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    );
}

fn serve(inner: Arc<Inner>, stream: TcpStream) {
    let out: Out = Arc::new(Mutex::new(stream.try_clone().unwrap()));
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else {
            return;
        };
        let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        inner.lock().received.push(msg.clone());
        inner.changed.notify_all();
        let method = msg["method"].as_str().unwrap_or("");
        let id = msg.get("id").cloned();
        match (method, id) {
            ("session/cancel", None) => {
                if let Some(sid) = msg["params"]["sessionId"].as_str() {
                    inner.lock().cancelled.push(sid.to_string());
                    inner.changed.notify_all();
                }
            }
            ("initialize", Some(id)) => respond(
                &out,
                &id,
                json!({ "protocolVersion": 1, "agentCapabilities": {}, "authMethods": [] }),
            ),
            ("session/new", Some(id)) => {
                let sid = {
                    let mut st = inner.lock();
                    st.sessions += 1;
                    format!("mock-session-{}", st.sessions)
                };
                if let Some(dir) = &inner.sessions_dir {
                    let _ = std::fs::create_dir_all(dir);
                    let _ = std::fs::write(dir.join(format!("{sid}.json")), "{}");
                    let _ = std::fs::write(dir.join(format!("{sid}.jsonl")), "");
                }
                respond(&out, &id, json!({ "sessionId": sid }));
            }
            ("session/prompt", Some(id)) => {
                let sid = msg["params"]["sessionId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let turn = inner
                    .lock()
                    .turns
                    .pop_front()
                    .unwrap_or_else(|| Turn::reply("ok"));
                let (inner, out) = (inner.clone(), out.clone());
                thread::spawn(move || play(&inner, &out, id, &sid, turn));
            }
            ("", Some(_)) => {
                // The client answered one of our requests (a permission).
                inner.changed.notify_all();
            }
            (_, Some(id)) => respond(&out, &id, json!({})),
            _ => {}
        }
    }
}

fn update(out: &Out, sid: &str, update: Value) {
    send(
        out,
        &json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": { "sessionId": sid, "update": update },
        }),
    );
}

fn play(inner: &Inner, out: &Out, id: Value, sid: &str, turn: Turn) {
    for step in turn.0 {
        if inner.is_cancelled(sid) {
            break;
        }
        match step {
            Step::Chunks(chunks) => {
                for text in chunks {
                    update(
                        out,
                        sid,
                        json!({
                            "sessionUpdate": "agent_message_chunk",
                            "content": { "type": "text", "text": text },
                        }),
                    );
                }
            }
            Step::ToolCall => update(
                out,
                sid,
                json!({
                    "sessionUpdate": "tool_call",
                    "toolCallId": "mock-tool-1",
                    "title": "Run a command",
                    "kind": "execute",
                    "status": "in_progress",
                }),
            ),
            Step::AskPermission => {
                let request_id = {
                    let mut st = inner.lock();
                    st.permission_requests += 1;
                    format!("mock-permission-{}", st.permission_requests)
                };
                send(
                    out,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "method": "session/request_permission",
                        "params": {
                            "sessionId": sid,
                            "toolCall": {
                                "toolCallId": "mock-tool-1",
                                "title": "Run a command",
                                "kind": "execute",
                            },
                            "options": [
                                { "optionId": "mock-allow", "name": "Allow", "kind": "allow_once" },
                                { "optionId": "mock-reject", "name": "Reject", "kind": "reject_once" },
                            ],
                        },
                    }),
                );
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut st = inner.lock();
                while !st.received.iter().any(|m| m["id"] == request_id.as_str())
                    && Instant::now() < deadline
                {
                    st = inner
                        .changed
                        .wait_timeout(st, Duration::from_millis(50))
                        .unwrap_or_else(|p| p.into_inner())
                        .0;
                }
            }
            Step::Sleep(d) => {
                inner.wait_cancel(sid, d);
            }
            Step::WaitForCancel(max) => {
                inner.wait_cancel(sid, max);
            }
            Step::Hangup => {
                inner.hang_up();
                return;
            }
        }
    }
    let reason = if inner.is_cancelled(sid) {
        "cancelled"
    } else {
        "end_turn"
    };
    respond(out, &id, json!({ "stopReason": reason }));
}

#[test]
#[ignore] // Run by hand with --ignored.
fn run_mock_acp_server() {
    let agent = ScriptedAgent::start_on("127.0.0.1:8765", Vec::new(), None);
    println!(
        "Mock ACP agent listening on 127.0.0.1:{}. Press Ctrl+C to stop.",
        agent.port
    );
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}
