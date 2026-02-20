// Basic unit tests for ACP client structure
// Note: Full integration tests require a running kiro-cli instance

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn test_acp_request_serialization() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": "test-123",
            "method": "chat",
            "params": {
                "message": "Hello"
            }
        });

        let serialized = serde_json::to_string(&request).unwrap();
        assert!(serialized.contains("jsonrpc"));
        assert!(serialized.contains("2.0"));
        assert!(serialized.contains("chat"));
    }

    #[test]
    fn test_acp_response_deserialization() {
        let response_json = r#"{
            "jsonrpc": "2.0",
            "id": "test-123",
            "result": {
                "content": "Hello back!"
            }
        }"#;

        let response: serde_json::Value = serde_json::from_str(response_json).unwrap();
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], "test-123");
        assert_eq!(response["result"]["content"], "Hello back!");
    }

    #[test]
    fn test_acp_error_response() {
        let error_json = r#"{
            "jsonrpc": "2.0",
            "id": "test-123",
            "error": {
                "code": -32600,
                "message": "Invalid request"
            }
        }"#;

        let response: serde_json::Value = serde_json::from_str(error_json).unwrap();
        assert_eq!(response["error"]["code"], -32600);
        assert_eq!(response["error"]["message"], "Invalid request");
    }
}
