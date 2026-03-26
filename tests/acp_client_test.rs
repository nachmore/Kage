use kage::acp_client::types::*;

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
