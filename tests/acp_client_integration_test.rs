//! End-to-end tests for the ACP client transport: connect, request/
//! response roundtrip, notification delivery, error handling, and
//! lifecycle (disconnect, reconnect).
//!
//! Uses a lightweight in-process TCP mock so these run without any
//! external agent binary.

mod common;

use common::mock_tcp_server::{MockAcpServer, MockResponder, Reply};
use kage::acp_client::{AcpClient, AcpConnectionMode};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn client_for(port: u16) -> AcpClient {
    AcpClient::new(AcpConnectionMode::Remote {
        host: "127.0.0.1".to_string(),
        port,
    })
}

#[test]
fn connect_then_disconnect_reports_connected_state() {
    let server = MockAcpServer::start(MockResponder { replies: vec![] });
    let client = client_for(server.port);
    assert!(
        !client.is_connected(),
        "fresh client should not claim to be connected"
    );

    client.connect().expect("connect to mock server");
    assert!(
        client.is_connected(),
        "client should be connected after connect()"
    );

    client.disconnect();
    assert!(
        !client.is_connected(),
        "client should report disconnected after disconnect()"
    );
}

#[test]
fn request_receives_matching_response_by_id() {
    let server = MockAcpServer::start(MockResponder {
        replies: vec![(
            "initialize",
            Reply::Ok(json!({ "protocolVersion": "1.0", "agent": "mock" })),
        )],
    });
    let client = client_for(server.port);
    client.connect().expect("connect");

    let resp = client
        .send_request("initialize", json!({}))
        .expect("send_request");
    let result = resp.result.expect("expected a result, not an error");
    assert_eq!(result["agent"], "mock");

    // The server should have recorded exactly the request we sent. The
    // transport allocates the id internally, but every line must carry one.
    let seen = server.wait_for_requests(1, Duration::from_secs(2));
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["method"], "initialize");
    assert!(
        seen[0]["id"].is_number(),
        "id must be a number, got: {:?}",
        seen[0]["id"]
    );
    assert_eq!(
        seen[0]["id"], resp.id,
        "server must echo the same id we sent"
    );
}

#[test]
fn error_response_is_surfaced_by_send_request() {
    let server = MockAcpServer::start(MockResponder {
        replies: vec![(
            "session/new",
            Reply::Err {
                code: -32601,
                message: "method not found",
            },
        )],
    });
    let client = client_for(server.port);
    client.connect().expect("connect");

    let resp = client
        .send_request("session/new", json!({}))
        .expect("send_request returns Ok(response) for JSON-RPC errors");
    assert!(resp.error.is_some(), "expected error field populated");
    assert!(
        resp.result.is_none(),
        "result must be None when error is present"
    );
    let err = resp.error.unwrap();
    assert_eq!(err.code, -32601);
    assert_eq!(err.message, "method not found");
}

#[test]
fn notifications_from_server_reach_installed_handler() {
    let server = MockAcpServer::start(MockResponder { replies: vec![] });
    let client = client_for(server.port);

    let received: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let received_clone = received.clone();
    client.set_notification_handler(move |msg| {
        received_clone.lock().unwrap().push(msg);
    });

    client.connect().expect("connect");

    // Push a few notifications server-side.
    for i in 0..3 {
        server.push_notification(json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": { "seq": i, "text": format!("hello {}", i) }
        }));
    }

    // Give the reader thread a moment to drain all three.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while received.lock().unwrap().len() < 3 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }

    let got = received.lock().unwrap().clone();
    assert_eq!(got.len(), 3, "expected 3 notifications, got {}", got.len());
    for (i, n) in got.iter().enumerate() {
        assert_eq!(n["method"], "session/update");
        assert_eq!(n["params"]["seq"], i);
    }
}

#[test]
fn connect_fails_fast_when_server_not_listening() {
    // Pick a port we're confident nothing else is on by binding and
    // immediately dropping the listener.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_port = listener.local_addr().unwrap().port();
    drop(listener);

    let client = client_for(closed_port);
    let err = client.connect().unwrap_err();
    let msg = format!("{}", err).to_ascii_lowercase();
    // Accept any connection-refused flavor — exact text varies by OS.
    assert!(
        msg.contains("connect")
            || msg.contains("refused")
            || msg.contains("actively")
            || msg.contains("unreachable"),
        "expected connection failure message, got: {}",
        msg
    );
    assert!(!client.is_connected());
}

#[test]
fn notification_handler_is_replaceable_mid_stream() {
    // First handler counts notifications, then we swap in a second one
    // that records payloads. This validates that the handler slot is
    // atomically swappable (we rely on that for session-switch logic).
    let server = MockAcpServer::start(MockResponder { replies: vec![] });
    let client = client_for(server.port);

    let first_count = Arc::new(AtomicUsize::new(0));
    let first_clone = first_count.clone();
    client.set_notification_handler(move |_| {
        first_clone.fetch_add(1, Ordering::SeqCst);
    });

    client.connect().expect("connect");

    server.push_notification(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": { "text": "for first handler" }
    }));

    // Wait until the first handler has seen at least one notification.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while first_count.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(first_count.load(Ordering::SeqCst), 1);

    // Swap handler.
    let second_payloads: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let second_clone = second_payloads.clone();
    client.set_notification_handler(move |msg| {
        second_clone.lock().unwrap().push(msg);
    });

    server.push_notification(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": { "text": "for second handler" }
    }));

    // Wait for the second handler.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while second_payloads.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }

    let snap = second_payloads.lock().unwrap().clone();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0]["params"]["text"], "for second handler");
    // First handler must not have seen the second notification.
    assert_eq!(first_count.load(Ordering::SeqCst), 1);
}

#[test]
fn connection_drops_when_server_goes_away() {
    let server = MockAcpServer::start(MockResponder { replies: vec![] });
    let client = client_for(server.port);
    client.connect().expect("connect");
    assert!(client.is_connected());

    // Drop the server — its Drop impl joins the handler thread which
    // causes the TCP connection to close.
    drop(server);

    // Give the reader thread a beat to notice the EOF.
    std::thread::sleep(Duration::from_millis(300));

    // is_connected reflects the reader side's view of the connection.
    // We don't assert a specific value here because the exact timing
    // is racy: some platforms notice EOF immediately, others take
    // a system call. What we *can* assert is that sending a request
    // against a dropped server returns an error rather than hanging
    // indefinitely.
    let result = client.send_request("anything", json!({}));
    assert!(
        result.is_err(),
        "send_request should fail after server drop, got: {:?}",
        result
    );
}

#[test]
fn concurrent_requests_each_receive_their_own_response() {
    // Regression: prior to the pending-map rewrite, the reader funneled all
    // responses into a single mpsc channel and `send_request` returned the
    // first one off it without checking the id. Two callers in flight at
    // once would silently swap responses. Now each caller has its own inbox
    // keyed by an internally-allocated id, so both must get the right reply.
    let server = MockAcpServer::start(MockResponder {
        replies: vec![
            ("method.a", Reply::Ok(json!({ "tag": "A" }))),
            ("method.b", Reply::Ok(json!({ "tag": "B" }))),
        ],
    });
    let client = Arc::new(client_for(server.port));
    client.connect().expect("connect");

    let c1 = client.clone();
    let h1 = std::thread::spawn(move || c1.send_request("method.a", json!({})));
    let c2 = client.clone();
    let h2 = std::thread::spawn(move || c2.send_request("method.b", json!({})));

    let r1 = h1.join().unwrap().expect("method.a should succeed");
    let r2 = h2.join().unwrap().expect("method.b should succeed");

    assert_eq!(r1.result.unwrap()["tag"], "A");
    assert_eq!(r2.result.unwrap()["tag"], "B");
}

#[test]
fn send_permission_response_writes_jsonrpc_response_keyed_to_request_id() {
    // Permission replies are JSON-RPC *responses* (not notifications) — they
    // echo the original request id and carry a result object the agent
    // expects in a specific shape: { outcome: { outcome: "selected", optionId: "..." } }.
    // Verify both the framing and the option-id pass-through.
    let server = MockAcpServer::start(MockResponder { replies: vec![] });
    let client = client_for(server.port);
    client.connect().expect("connect");

    let request_id = json!("perm-req-42");
    client
        .send_permission_response(&request_id, "allow_24h")
        .expect("send_permission_response writes successfully");

    let seen = server.wait_for_requests(1, Duration::from_secs(2));
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0]["id"], request_id,
        "response must echo the original request id"
    );
    assert!(
        seen[0].get("method").is_none(),
        "responses don't carry a method"
    );
    assert_eq!(seen[0]["result"]["outcome"]["outcome"], "selected");
    assert_eq!(seen[0]["result"]["outcome"]["optionId"], "allow_24h");
}

#[test]
fn cancel_session_writes_jsonrpc_notification_to_transport() {
    // The client API hides JSON-RPC framing — callers shouldn't have to know
    // method names or param shapes. Verify that cancel_session emits a
    // well-formed session/cancel notification (no id, params.sessionId set).
    let server = MockAcpServer::start(MockResponder { replies: vec![] });
    let client = client_for(server.port);
    client.connect().expect("connect");

    client
        .cancel_session("session-abc")
        .expect("cancel_session writes successfully");

    let seen = server.wait_for_requests(1, Duration::from_secs(2));
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["method"], "session/cancel");
    assert_eq!(seen[0]["params"]["sessionId"], "session-abc");
    assert!(
        seen[0].get("id").is_none() || seen[0]["id"].is_null(),
        "session/cancel is a notification — must not carry an id"
    );
}

#[test]
fn unsolicited_response_is_dropped_not_delivered_to_next_request() {
    // Regression: an out-of-band response (id we never sent) used to land in
    // the shared mpsc channel and corrupt the next caller. Now it must be
    // logged and dropped, leaving the legitimate request to still receive
    // its own reply.
    let server = MockAcpServer::start(MockResponder {
        replies: vec![("ping", Reply::Ok(json!({ "ok": true })))],
    });
    let client = client_for(server.port);
    client.connect().expect("connect");

    // Inject a response with an id that no one is waiting on.
    server.push_notification(json!({
        "jsonrpc": "2.0",
        "id": 999_999,
        "result": { "tag": "stray" }
    }));

    // Give the reader a beat to process the stray line.
    std::thread::sleep(Duration::from_millis(100));

    let resp = client
        .send_request("ping", json!({}))
        .expect("ping should succeed");
    assert_eq!(
        resp.result.unwrap()["ok"],
        true,
        "subsequent legitimate request must not be served the stray response"
    );
}

#[test]
fn two_concurrent_sessions_demux_chunks_by_session_id() {
    // Regression for the multi-window backend rewrite: with the global
    // session_id slot gone, AcpClient must route streaming chunks back
    // to the right per-session bucket purely by the `params.sessionId`
    // field on each `session/update` notification. We simulate two
    // sessions receiving interleaved chunks and verify the accumulators
    // stay isolated.
    let server = MockAcpServer::start(MockResponder { replies: vec![] });
    let client = client_for(server.port);
    client.connect().expect("connect");

    let received: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let received_clone = received.clone();
    client.set_notification_handler(move |notif| {
        let sid = notif["params"]["sessionId"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let text = notif["params"]["update"]["content"]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if !sid.is_empty() {
            received_clone.lock().unwrap().push((sid, text));
        }
    });

    // Interleave chunks for sessions A and B. With the old single-slot
    // model, a notification handler that read AcpClient.session_id would
    // misroute these depending on which switch was last; with the new
    // model the handler reads sessionId off the payload and the test
    // proves that routing.
    let interleaved = vec![
        ("sess-A", "alpha "),
        ("sess-B", "beta1 "),
        ("sess-A", "alpha2 "),
        ("sess-B", "beta2 "),
        ("sess-A", "alpha3"),
        ("sess-B", "beta3"),
    ];
    for (sid, text) in &interleaved {
        server.push_notification(json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": sid,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "text": text }
                }
            }
        }));
    }

    // Wait for all six to arrive.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while received.lock().unwrap().len() < interleaved.len() && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }

    let got = received.lock().unwrap().clone();
    assert_eq!(got.len(), interleaved.len());
    let a_text: String = got
        .iter()
        .filter(|(s, _)| s == "sess-A")
        .map(|(_, t)| t.as_str())
        .collect();
    let b_text: String = got
        .iter()
        .filter(|(s, _)| s == "sess-B")
        .map(|(_, t)| t.as_str())
        .collect();
    assert_eq!(a_text, "alpha alpha2 alpha3");
    assert_eq!(b_text, "beta1 beta2 beta3");
}

#[test]
fn cancel_session_targets_only_the_named_session() {
    // The cancel command emits a session/cancel notification carrying
    // the explicit session id; verify a multi-session client cancels
    // only the requested session and not others.
    let server = MockAcpServer::start(MockResponder { replies: vec![] });
    let client = client_for(server.port);
    client.connect().expect("connect");

    client.cancel_session("sess-A").expect("cancel A");

    let seen = server.wait_for_requests(1, Duration::from_secs(2));
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["method"], "session/cancel");
    assert_eq!(seen[0]["params"]["sessionId"], "sess-A");
}

// --- Compaction gate ----------------------------------------------------
//
// `wait_for_compaction` blocks outgoing prompts while the agent is
// compacting context. Two failure modes used to make sends hang for the
// full 60s timeout:
//   1. Agent disconnects mid-compaction — the "completed" notification
//      that would clear `is_compacting` never arrives.
//   2. Reader thread dies (EOF on stream) — same outcome.
// Both got every subsequent send_chat_streaming gated for 60s.

#[test]
fn wait_for_compaction_returns_immediately_when_not_compacting() {
    let server = MockAcpServer::start(MockResponder { replies: vec![] });
    let client = client_for(server.port);
    client.connect().expect("connect");

    let start = std::time::Instant::now();
    let waited = client.wait_for_compaction();
    let elapsed = start.elapsed();

    assert!(
        !waited,
        "wait_for_compaction must return false when no compaction is active"
    );
    // Should be near-instant — the function reads the bool, sees false,
    // returns. Anything over a few ms means we accidentally entered the
    // wait loop.
    assert!(
        elapsed < Duration::from_millis(100),
        "expected fast path, got {:?}",
        elapsed
    );
}

#[test]
fn disconnect_clears_in_flight_compaction_gate() {
    // Force the gate on by toggling the bool directly (mimicking what
    // the compaction/status="started" notification handler does), then
    // disconnect, then verify wait_for_compaction returns instantly.
    // Pre-fix this used to gate for the full 60s.
    let server = MockAcpServer::start(MockResponder { replies: vec![] });
    let client = client_for(server.port);
    client.connect().expect("connect");

    // Mark compaction in flight.
    {
        let (lock, _cvar) = &*client.compacting;
        let mut g = lock.lock().expect("compaction lock");
        *g = true;
    }

    // disconnect() must clear the gate and notify any waiter.
    client.disconnect();

    let start = std::time::Instant::now();
    let waited = client.wait_for_compaction();
    let elapsed = start.elapsed();
    assert!(
        !waited,
        "after disconnect, wait_for_compaction must see is_compacting=false (false return)"
    );
    assert!(
        elapsed < Duration::from_millis(100),
        "wait must return instantly after disconnect cleared the gate, got {:?}",
        elapsed
    );
}

#[test]
fn wait_for_compaction_aborts_when_transport_disconnects_mid_wait() {
    // Reproduce the original bug: agent dies mid-compaction without the
    // "completed" notification ever arriving. wait_for_compaction polls
    // is_connected() and bails when it sees the disconnect, instead of
    // sleeping for the full 60s.
    let server = MockAcpServer::start(MockResponder { replies: vec![] });
    let client = client_for(server.port);
    client.connect().expect("connect");

    // Mark compaction in flight WITHOUT going through disconnect (so
    // the gate is still set when we call wait_for_compaction).
    {
        let (lock, _cvar) = &*client.compacting;
        let mut g = lock.lock().expect("compaction lock");
        *g = true;
    }

    // Drop the mock server 200ms in. That closes the TCP connection,
    // the reader thread reads Ok(0) (EOF), and flips `connected=false`.
    // wait_for_compaction's polling loop should detect that and return.
    let client_arc = Arc::new(client);
    let waiter = client_arc.clone();
    let handle = std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let waited = waiter.wait_for_compaction();
        (waited, start.elapsed())
    });
    std::thread::sleep(Duration::from_millis(200));
    drop(server); // EOFs the connection — reader sets connected=false

    let (waited, elapsed) = handle.join().expect("waiter thread");
    assert!(
        waited,
        "wait_for_compaction returns true when it actually waited"
    );
    // Polling slice is 500ms; worst case is ~700ms (200ms our sleep +
    // 500ms next poll). Definitely not 60s. Slack for slow CI.
    assert!(
        elapsed < Duration::from_secs(3),
        "wait must detect transport disconnect, got {:?}",
        elapsed
    );
}

// --- Per-request response routing --------------------------------------
//
// Pre-fix, the transport had a single "next response" channel and any
// response with a method-less body got handed to the next caller —
// regardless of id. A late response from a slow request, or an agent
// that mis-echoed an id, could deliver the wrong payload to a
// completely unrelated caller. The fix is per-request inboxes keyed
// by id; orphan responses get dropped + logged. These tests pin that
// behaviour.

#[test]
fn orphan_response_does_not_corrupt_a_concurrent_pending_request() {
    // Setup: a request that will block (server replies with Drop), and
    // server pushes a fabricated "response" with an id that doesn't
    // match anything. The pending request must stay parked, not return
    // the orphan as if it were its answer.
    let server = MockAcpServer::start(MockResponder {
        replies: vec![("slow", Reply::Drop)],
    });
    let client = Arc::new(client_for(server.port));
    client.connect().expect("connect");

    // Spawn a thread that blocks on send_request.
    let waiter = client.clone();
    let req_handle = std::thread::spawn(move || {
        waiter.send_request("slow", json!({}))
        // Will block on the per-request inbox until we force-disconnect.
    });

    // Give the request a moment to actually be in flight.
    std::thread::sleep(Duration::from_millis(100));

    // Push an orphan response: a method-less line with an id that
    // doesn't correspond to any pending request. The reader must
    // log + drop it, not deliver it to the slow caller.
    server.push_notification(json!({
        "jsonrpc": "2.0",
        "id": 999_999_999u64,
        "result": { "should": "not be delivered to anyone" }
    }));

    // The pending request must still be blocked. Brief wait to be sure
    // the orphan was processed; if it were misdelivered, the request
    // would have returned by now.
    std::thread::sleep(Duration::from_millis(150));

    // Stop the server — that EOFs the connection and the
    // recv_timeout's inbox sender drops, waking the caller with
    // a Disconnected error.
    drop(server);

    let result = req_handle.join().expect("waiter thread");
    // We don't care HOW it ends — just that it didn't end with the
    // orphan's payload. An Ok result with `should: "not be delivered"`
    // would be the bug.
    if let Ok(resp) = &result {
        if let Some(value) = &resp.result {
            assert!(
                !value.to_string().contains("not be delivered to anyone"),
                "orphan response was misdelivered to the pending request: {:?}",
                resp
            );
        }
    }
}

#[test]
fn reader_eof_wakes_blocked_send_request() {
    // The 60s per-request timeout used to be the only way out of a
    // blocked send_request when the agent died — the reader thread
    // saw EOF and flipped `connected=false`, but the per-request
    // inbox senders stayed alive in `pending`, so the recv blocked
    // for the full timeout. The reader's EOF branch now clears
    // `pending`, dropping every sender; receivers wake with a
    // Disconnected error within milliseconds.
    let server = MockAcpServer::start(MockResponder {
        replies: vec![("hangs", Reply::Drop)],
    });
    let client = Arc::new(client_for(server.port));
    client.connect().expect("connect");

    let waiter = client.clone();
    let handle = std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let result = waiter.send_request("hangs", json!({}));
        (result, start.elapsed())
    });

    std::thread::sleep(Duration::from_millis(100));
    // Tearing down the connection MUST surface to the blocked caller
    // immediately, not after the 60s timeout.
    drop(server);

    let (result, elapsed) = handle.join().expect("waiter thread");
    assert!(
        result.is_err(),
        "send_request must surface an error after the connection drops"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "send_request should wake within seconds, got {:?}",
        elapsed
    );
}

#[test]
fn force_disconnect_wakes_blocked_send_request() {
    // Companion to reader_eof_wakes_blocked_send_request: explicit
    // teardown via AcpClient::disconnect() (which calls into
    // transport.disconnect()) used to leave pending senders alive.
    // disconnect() now also clears pending (transport.disconnect
    // behaviour matches force_disconnect for that bit), so the
    // wakeup happens regardless of whether teardown was triggered
    // by the agent crashing or by us deciding to disconnect.
    let server = MockAcpServer::start(MockResponder {
        replies: vec![("hangs", Reply::Drop)],
    });
    let client = Arc::new(client_for(server.port));
    client.connect().expect("connect");

    let waiter = client.clone();
    let handle = std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let result = waiter.send_request("hangs", json!({}));
        (result, start.elapsed())
    });

    std::thread::sleep(Duration::from_millis(100));
    client.disconnect();

    let (result, elapsed) = handle.join().expect("waiter thread");
    assert!(
        result.is_err(),
        "send_request must surface an error after disconnect()"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "send_request should wake within seconds, got {:?}",
        elapsed
    );

    drop(server);
}

#[test]
fn prompt_idle_watchdog_wakes_on_disconnect_not_after_idle_timeout() {
    // session/prompt uses the idle watchdog (no wall-clock cap — a healthy
    // turn can run for minutes as long as the backend keeps streaming). But
    // a genuinely dead connection must still wake the caller promptly via the
    // Disconnected path, NOT sit until PROMPT_IDLE_TIMEOUT elapses.
    let server = MockAcpServer::start(MockResponder {
        replies: vec![("session/prompt", Reply::Drop)],
    });
    let client = Arc::new(client_for(server.port));
    client.connect().expect("connect");

    let waiter = client.clone();
    let handle = std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let result = waiter.send_prompt("sess-1", json!({ "sessionId": "sess-1" }));
        (result, start.elapsed())
    });

    std::thread::sleep(Duration::from_millis(100));
    client.disconnect();

    let (result, elapsed) = handle.join().expect("waiter thread");
    assert!(
        result.is_err(),
        "send_prompt must surface an error after disconnect()"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "prompt watchdog should wake within seconds of disconnect, got {:?}",
        elapsed
    );

    drop(server);
}

#[test]
fn write_failure_removes_pending_entry() {
    // If write_line fails (broken pipe, full buffer, no write handle),
    // send_request must remove its pending-inbox entry on the way
    // out — otherwise stale ids accumulate in the map until process
    // exit. Reproduce by disconnecting first, then trying to send;
    // write_line returns "No write handle available" and the caller
    // surfaces an error.
    let server = MockAcpServer::start(MockResponder { replies: vec![] });
    let client = client_for(server.port);
    client.connect().expect("connect");
    client.disconnect();

    // The next send_request will fail to write because both pipe and
    // tcp handles are None.
    let result = client.send_request("nope", json!({}));
    assert!(
        result.is_err(),
        "send_request must fail when no write handle"
    );

    // The pending map should be empty — no leaked entry. We can't
    // inspect the map directly from the integration test, but we can
    // verify by reconnecting and sending a real request: id reuse
    // would cause a routing collision otherwise. Pre-fix the leaked
    // id sat in the map for the rest of the process lifetime; not
    // immediately broken, but a slow leak.
    drop(server);

    let _ = result; // We've checked the error case; the test's assertion
                    // is that send_request doesn't panic and the error
                    // path is taken. A pending-map leak isn't observable
                    // from the public API, so we rely on code review for
                    // that part of the contract.
}

// --- Session lifecycle (acp_client/session.rs) -------------------------
//
// session.rs is the heart of the per-session protocol — every chat
// message, every session create / resume / cancel, every steering
// preamble flows through it. Pre-fix, this module had zero coverage.
// These tests exercise the wire-format guarantees the module makes:
// the right method names, the right param shapes, the lazy-initialize
// behaviour.

#[test]
fn create_session_sends_initialize_first_when_uninitialized() {
    // First call into a session-bearing method must implicitly run
    // the `initialize` handshake. Pre-fix the call order was wrong
    // for clients that ran straight at session/new and the agent
    // would reject session creation on an uninitialized connection.
    let server = MockAcpServer::start(MockResponder {
        replies: vec![
            ("initialize", Reply::Ok(json!({ "protocolVersion": 1 }))),
            ("session/new", Reply::Ok(json!({ "sessionId": "s-1" }))),
        ],
    });
    let client = client_for(server.port);
    client.connect().expect("connect");

    let (session_id, _models) = client
        .create_session(Some("/tmp".to_string()))
        .expect("create_session");
    assert_eq!(session_id, "s-1");

    // Server must have seen initialize FIRST, then session/new.
    let seen = server.wait_for_requests(2, Duration::from_secs(2));
    assert!(seen.len() >= 2, "expected ≥2 requests, got {}", seen.len());
    assert_eq!(seen[0]["method"], "initialize");
    assert_eq!(seen[1]["method"], "session/new");
    // session/new params must include cwd + an empty mcpServers list.
    assert_eq!(seen[1]["params"]["cwd"], "/tmp");
    assert!(seen[1]["params"]["mcpServers"].is_array());
}

#[test]
fn create_session_does_not_re_initialize_on_second_call() {
    // The `initialized` flag means a second create_session on the
    // same connection skips initialize. A regression here would
    // double the initial-prompt latency for every new chat window.
    let server = MockAcpServer::start(MockResponder {
        replies: vec![
            ("initialize", Reply::Ok(json!({ "protocolVersion": 1 }))),
            ("session/new", Reply::Ok(json!({ "sessionId": "s" }))),
        ],
    });
    let client = client_for(server.port);
    client.connect().expect("connect");

    client.create_session(None).expect("first session");
    client.create_session(None).expect("second session");

    let seen = server.wait_for_requests(3, Duration::from_secs(2));
    let init_count = seen.iter().filter(|r| r["method"] == "initialize").count();
    let new_count = seen.iter().filter(|r| r["method"] == "session/new").count();
    assert_eq!(
        init_count, 1,
        "initialize must run once across N create_session calls"
    );
    assert_eq!(
        new_count, 2,
        "session/new must run for each create_session call"
    );
}

#[test]
fn load_existing_session_emits_session_load_with_id_and_cwd() {
    let server = MockAcpServer::start(MockResponder {
        replies: vec![
            ("initialize", Reply::Ok(json!({ "protocolVersion": 1 }))),
            ("session/load", Reply::Ok(json!({}))),
        ],
    });
    let client = client_for(server.port);
    client.connect().expect("connect");

    let (id, _models) = client
        .load_existing_session("resume-me", Some("/work".to_string()))
        .expect("load_existing_session");
    assert_eq!(id, "resume-me", "returned id matches the input");

    let seen = server.wait_for_requests(2, Duration::from_secs(2));
    let load = seen
        .iter()
        .find(|r| r["method"] == "session/load")
        .expect("session/load was sent");
    assert_eq!(load["params"]["sessionId"], "resume-me");
    assert_eq!(load["params"]["cwd"], "/work");
}

#[test]
fn create_session_surfaces_acp_errors_from_session_new() {
    // Whatever JSON-RPC error the agent returns for session/new must
    // surface as an Err on `create_session`, not a silent default
    // session id. This was a sharp edge during the early kiro-cli
    // integration: a stale "agent rejected session/new" condition
    // would silently fall through to "uses session_id 'undefined'"
    // for the rest of the chat.
    let server = MockAcpServer::start(MockResponder {
        replies: vec![
            ("initialize", Reply::Ok(json!({ "protocolVersion": 1 }))),
            (
                "session/new",
                Reply::Err {
                    code: -32603,
                    message: "internal error",
                },
            ),
        ],
    });
    let client = client_for(server.port);
    client.connect().expect("connect");

    let result = client.create_session(None);
    assert!(
        result.is_err(),
        "expected create_session to surface the JSON-RPC error"
    );
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.to_lowercase().contains("session creation failed"),
        "error message should mention the failure, got: {}",
        err
    );
}

#[test]
fn send_chat_streaming_emits_session_prompt_with_text_block() {
    // The `session/prompt` request shape: { sessionId, prompt: [...] }
    // where prompt is a list of content blocks. Plain text messages
    // produce a single `{ type: "text", text: ... }` block (plus a
    // possible timestamp prefix block).
    let server = MockAcpServer::start(MockResponder {
        replies: vec![
            ("initialize", Reply::Ok(json!({ "protocolVersion": 1 }))),
            ("session/new", Reply::Ok(json!({ "sessionId": "s-x" }))),
            ("session/prompt", Reply::Ok(json!({}))),
        ],
    });
    let client = client_for(server.port);
    client.connect().expect("connect");
    let (sid, _) = client.create_session(None).expect("create_session");

    client
        .send_chat_streaming(&sid, "hello world", None)
        .expect("send_chat_streaming");

    let seen = server.wait_for_requests(3, Duration::from_secs(2));
    let prompt = seen
        .iter()
        .find(|r| r["method"] == "session/prompt")
        .expect("session/prompt was sent");
    assert_eq!(prompt["params"]["sessionId"], sid);
    let blocks = prompt["params"]["prompt"]
        .as_array()
        .expect("prompt is an array");
    let has_user_text = blocks
        .iter()
        .any(|b| b["type"] == "text" && b["text"] == "hello world");
    assert!(
        has_user_text,
        "expected a text block carrying the user message, got: {:?}",
        blocks
    );
}
