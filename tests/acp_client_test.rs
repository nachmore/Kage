use kage::acp_client::types::*;
use kage::acp_client::{AcpClient, AcpConnectionMode};

#[test]
fn test_acp_request_serialization() {
    let request = AcpRequest {
        jsonrpc: "2.0".to_string(),
        id: serde_json::json!("req-1"),
        method: "session/new".to_string(),
        params: serde_json::json!({"cwd": "/home/user"}),
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], "req-1");
    assert_eq!(json["method"], "session/new");
    assert_eq!(json["params"]["cwd"], "/home/user");
}

#[test]
fn test_acp_request_roundtrip() {
    let request = AcpRequest {
        jsonrpc: "2.0".to_string(),
        id: serde_json::json!(42),
        method: "chat".to_string(),
        params: serde_json::json!({"message": "hello"}),
    };
    let json_str = serde_json::to_string(&request).unwrap();
    let deserialized: AcpRequest = serde_json::from_str(&json_str).unwrap();
    assert_eq!(deserialized.method, "chat");
    assert_eq!(deserialized.id, serde_json::json!(42));
}

#[test]
fn test_acp_response_with_result() {
    let json = r#"{
        "jsonrpc": "2.0",
        "id": "test-1",
        "result": {"sessionId": "abc-123"}
    }"#;
    let response: AcpResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.id, "test-1");
    assert!(response.result.is_some());
    assert!(response.error.is_none());
    assert_eq!(response.result.unwrap()["sessionId"], "abc-123");
}

#[test]
fn test_acp_response_with_error() {
    let json = r#"{
        "jsonrpc": "2.0",
        "id": "test-2",
        "error": {"code": -32600, "message": "Invalid request"}
    }"#;
    let response: AcpResponse = serde_json::from_str(json).unwrap();
    assert!(response.result.is_none());
    assert!(response.error.is_some());
    let err = response.error.unwrap();
    assert_eq!(err.code, -32600);
    assert_eq!(err.message, "Invalid request");
}

#[test]
fn test_acp_notification_deserialization() {
    let json = r#"{
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"text": "hello"},
        "id": "notif-1"
    }"#;
    let notif: AcpNotification = serde_json::from_str(json).unwrap();
    assert_eq!(notif.method, "session/update");
    assert_eq!(notif.params["text"], "hello");
    assert_eq!(notif.id, Some(serde_json::json!("notif-1")));
}

#[test]
fn test_acp_notification_without_id() {
    let json = r#"{
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {}
    }"#;
    let notif: AcpNotification = serde_json::from_str(json).unwrap();
    assert!(notif.id.is_none());
}

#[test]
fn test_format_acp_error_without_data() {
    let err = AcpError {
        code: -32601,
        message: "Method not found".to_string(),
        data: None,
    };
    assert_eq!(format_acp_error(&err), "Method not found (code: -32601)");
}

#[test]
fn test_format_acp_error_with_string_data() {
    let err = AcpError {
        code: -32000,
        message: "Server error".to_string(),
        data: Some(serde_json::json!("detailed reason")),
    };
    assert_eq!(format_acp_error(&err), "Server error — detailed reason");
}

#[test]
fn test_format_acp_error_with_object_data() {
    let err = AcpError {
        code: -32000,
        message: "Server error".to_string(),
        data: Some(serde_json::json!({"detail": "something"})),
    };
    let formatted = format_acp_error(&err);
    assert!(formatted.starts_with("Server error — "));
    assert!(formatted.contains("detail"));
}

#[test]
fn test_acp_response_skips_none_fields() {
    // When result is Some and error is None, the serialized JSON should not contain "error"
    let response = AcpResponse {
        jsonrpc: "2.0".to_string(),
        id: serde_json::json!(1),
        result: Some(serde_json::json!("ok")),
        error: None,
    };
    let json_str = serde_json::to_string(&response).unwrap();
    assert!(!json_str.contains("error"));
}

// ---------------------------------------------------------------------------
// Per-session streaming accumulator
// ---------------------------------------------------------------------------
//
// Pre-fix the accumulator was a single global String. If a chunk arrived for
// session A while session B was active in the UI, the bytes appended to the
// shared buffer and rendered as if they were B's response. The rewrite keys
// the accumulator by session id so distinct sessions never see each other's
// bytes — tested here at the helper API surface, since spinning up the full
// notification handler under unit tests is too noisy.

fn fresh_client() -> AcpClient {
    // We never call .connect() — the accumulator helpers don't touch the
    // transport. AcpConnectionMode::Remote is just a placeholder.
    AcpClient::new(AcpConnectionMode::Remote {
        host: "127.0.0.1".to_string(),
        port: 0,
    })
}

#[test]
fn accumulator_isolates_chunks_by_session_id() {
    let client = fresh_client();

    client.accumulate_chunk("session-A", "hello");
    client.accumulate_chunk("session-B", "WORLD");
    client.accumulate_chunk("session-A", " there");

    assert_eq!(client.take_session_accumulator("session-A"), "hello there");
    assert_eq!(client.take_session_accumulator("session-B"), "WORLD");
    // Both buckets consumed.
    assert_eq!(client.take_session_accumulator("session-A"), "");
    assert_eq!(client.take_session_accumulator("session-B"), "");
}

#[test]
fn reset_session_accumulator_only_affects_target_session() {
    let client = fresh_client();
    client.accumulate_chunk("a", "keep me");
    client.accumulate_chunk("b", "wipe me");

    client.reset_session_accumulator("b");

    assert_eq!(client.take_session_accumulator("a"), "keep me");
    assert_eq!(client.take_session_accumulator("b"), "");
}

#[test]
fn accumulate_chunk_returns_truncated_slice_when_capacity_hit() {
    use kage::acp_client::MAX_ACCUMULATOR_SIZE;
    let client = fresh_client();
    // Fill the bucket to one byte short of the cap.
    let big = "x".repeat(MAX_ACCUMULATOR_SIZE - 1);
    let r = client.accumulate_chunk("big", &big);
    assert_eq!(r.map(|s| s.len()), Some(MAX_ACCUMULATOR_SIZE - 1));

    // Next chunk only one byte fits; the rest is dropped, accumulate_chunk
    // returns the truncated slice so the notification handler emits the
    // same delta it accumulated.
    let r2 = client.accumulate_chunk("big", "yz");
    assert_eq!(r2, Some("y"));

    // Past the cap, further chunks are entirely dropped.
    let r3 = client.accumulate_chunk("big", "more");
    assert!(r3.is_none(), "post-cap append must return None");

    // Bucket holds exactly the cap.
    let final_text = client.take_session_accumulator("big");
    assert_eq!(final_text.len(), MAX_ACCUMULATOR_SIZE);
}

#[test]
fn take_on_unknown_session_returns_empty_not_panic() {
    let client = fresh_client();
    assert_eq!(client.take_session_accumulator("never-existed"), "");
}
