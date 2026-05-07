//! End-to-end tests for the ACP client transport: connect, request/
//! response roundtrip, notification delivery, error handling, and
//! lifecycle (disconnect, reconnect).
//!
//! Uses a lightweight in-process TCP mock so these run without any
//! external kage-cli binary.

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
