//! Typed JSON-RPC 2.0 framing for the MCP server binary.
//!
//! `src/bin/computer_control_mcp.rs` previously hand-rolled JSON-RPC
//! parsing and response construction inline:
//!
//! ```ignore
//! let id = request.get("id").cloned().unwrap_or(Value::Null);
//! let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
//! let params = request.get("params").cloned().unwrap_or(json!({}));
//! ```
//!
//! That ad-hoc shape made every call site re-derive the same defaults
//! and made it easy to forget that, per the JSON-RPC 2.0 spec, the id of
//! a parse-error response must be `null` and the error codes have
//! specific reserved meanings.
//!
//! This module provides:
//!
//! - [`JsonRpcRequest`] — a typed view of an incoming line.
//! - [`parse_request`] — turns a raw line into either a request or a
//!   pre-formed parse-error response string.
//! - [`success`] / [`error`] / [`tool_result_text`] — response builders
//!   that produce serialized JSON-RPC strings ready to write to stdout.
//! - [`ErrorCode`] — the spec's reserved codes plus a few MCP extensions.
//!
//! Lives in `lib.rs` rather than `src/bin/` so it's reachable from
//! tests (Cargo doesn't expose binary modules to the test harness).

use serde::Deserialize;
use serde_json::Value;

/// A parsed JSON-RPC 2.0 request line.
///
/// The id field is `Value` rather than a typed enum because the spec
/// allows string, number, or null and we don't otherwise care which.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    /// The id field. Defaults to `Value::Null` if the line was a
    /// notification (no id) — for our purposes the difference is
    /// handled at the dispatch layer (notifications get no response).
    #[serde(default)]
    pub id: Value,
    /// The method name. Defaults to empty string for malformed input.
    #[serde(default)]
    pub method: String,
    /// The params object. Defaults to an empty object so handlers can
    /// always call `.get(...)` without an unwrap.
    #[serde(default = "default_params")]
    pub params: Value,
}

fn default_params() -> Value {
    Value::Object(Default::default())
}

impl JsonRpcRequest {
    /// True if this is a JSON-RPC notification (no `id` member). The
    /// spec says notifications get no response. We model that by
    /// treating `Value::Null` id as "may be a notification" — the
    /// dispatch layer decides per-method (e.g. `ping` always responds
    /// even with a null id).
    pub fn is_notification(&self) -> bool {
        self.id.is_null()
    }
}

/// JSON-RPC 2.0 reserved error codes plus a few MCP-side extensions
/// we care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// -32700 — invalid JSON received.
    ParseError,
    /// -32600 — Request object isn't valid JSON-RPC.
    InvalidRequest,
    /// -32601 — method not found.
    MethodNotFound,
    /// -32602 — invalid method parameters.
    InvalidParams,
    /// -32603 — internal server error.
    InternalError,
    /// Application-defined custom code in the -32000..=-32099 range.
    Custom(i32),
}

impl ErrorCode {
    pub fn code(self) -> i32 {
        match self {
            Self::ParseError => -32700,
            Self::InvalidRequest => -32600,
            Self::MethodNotFound => -32601,
            Self::InvalidParams => -32602,
            Self::InternalError => -32603,
            Self::Custom(c) => c,
        }
    }
}

/// Build a successful JSON-RPC response, serialized to a single-line
/// string ready to write to stdout.
pub fn success(id: &Value, result: Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
    .to_string()
}

/// Build an error JSON-RPC response, serialized to a single-line string.
pub fn error(id: &Value, code: ErrorCode, message: &str) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code.code(), "message": message },
    })
    .to_string()
}

/// MCP-specific helper: build a `tools/call` result that wraps a single
/// text content block. `is_error` toggles the `isError` flag so the
/// host knows whether the call succeeded.
pub fn tool_result_text(id: &Value, text: &str, is_error: bool) -> String {
    success(
        id,
        serde_json::json!({
            "content": [{ "type": "text", "text": text }],
            "isError": is_error,
        }),
    )
}

/// Outcome of attempting to parse a single JSON-RPC line.
pub enum ParseOutcome {
    /// Line was empty / whitespace-only — caller should skip without
    /// emitting a response.
    Empty,
    /// Line parsed cleanly into a request.
    Ok(JsonRpcRequest),
    /// Parse failed; this string is a pre-formed JSON-RPC ParseError
    /// response that the caller should write to stdout. Per the spec,
    /// the response uses `id: null` since we don't know which request
    /// we couldn't parse.
    ParseError(String),
}

/// Parse a single JSON-RPC line.
///
/// Returns:
/// - [`ParseOutcome::Empty`] for whitespace-only input
/// - [`ParseOutcome::Ok`] for valid JSON-RPC requests
/// - [`ParseOutcome::ParseError`] with a serialized error response for
///   anything that doesn't deserialize as JSON
pub fn parse_request(raw: &str) -> ParseOutcome {
    if raw.trim().is_empty() {
        return ParseOutcome::Empty;
    }
    match serde_json::from_str::<JsonRpcRequest>(raw) {
        Ok(req) => ParseOutcome::Ok(req),
        Err(e) => ParseOutcome::ParseError(error(
            &Value::Null,
            ErrorCode::ParseError,
            &format!("Parse error: {}", e),
        )),
    }
}

/// Convenience: produce the canonical "message exceeds size limit"
/// response that the MCP loop emits when the input length cap is hit.
pub fn oversized_error() -> String {
    error(
        &Value::Null,
        ErrorCode::ParseError,
        "Parse error: message exceeds size limit",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse_response(s: &str) -> Value {
        serde_json::from_str(s).expect("response must be JSON")
    }

    // ---- success / error / tool_result_text builders ----------------------

    #[test]
    fn success_carries_id_and_result_and_jsonrpc_version() {
        let resp = success(&json!(7), json!({"x": 1}));
        let v = parse_response(&resp);
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 7);
        assert_eq!(v["result"], json!({"x": 1}));
        assert!(v.get("error").is_none(), "success must not include error");
    }

    #[test]
    fn success_preserves_string_id() {
        // MCP hosts often use string ids; numeric is allowed but not required.
        let resp = success(&json!("req-42"), json!(null));
        let v = parse_response(&resp);
        assert_eq!(v["id"], "req-42");
    }

    #[test]
    fn error_serializes_code_and_message() {
        let resp = error(&json!(1), ErrorCode::MethodNotFound, "no such method");
        let v = parse_response(&resp);
        assert_eq!(v["error"]["code"], -32601);
        assert_eq!(v["error"]["message"], "no such method");
        assert!(v.get("result").is_none(), "error must not include result");
    }

    #[test]
    fn error_codes_match_jsonrpc_spec() {
        assert_eq!(ErrorCode::ParseError.code(), -32700);
        assert_eq!(ErrorCode::InvalidRequest.code(), -32600);
        assert_eq!(ErrorCode::MethodNotFound.code(), -32601);
        assert_eq!(ErrorCode::InvalidParams.code(), -32602);
        assert_eq!(ErrorCode::InternalError.code(), -32603);
        assert_eq!(ErrorCode::Custom(-32050).code(), -32050);
    }

    #[test]
    fn tool_result_text_wraps_text_content_block() {
        let resp = tool_result_text(&json!(3), "hello", false);
        let v = parse_response(&resp);
        assert_eq!(v["result"]["isError"], false);
        assert_eq!(v["result"]["content"][0]["type"], "text");
        assert_eq!(v["result"]["content"][0]["text"], "hello");
    }

    #[test]
    fn tool_result_text_marks_errors_distinctly() {
        let ok = parse_response(&tool_result_text(&json!(1), "fine", false));
        let err = parse_response(&tool_result_text(&json!(2), "boom", true));
        assert_eq!(ok["result"]["isError"], false);
        assert_eq!(err["result"]["isError"], true);
    }

    // ---- parse_request -----------------------------------------------------

    #[test]
    fn parse_skips_blank_lines() {
        match parse_request("   \r\n  ") {
            ParseOutcome::Empty => {}
            _ => panic!("expected Empty for whitespace input"),
        }
    }

    #[test]
    fn parse_returns_typed_request_for_valid_json() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
        let req = match parse_request(raw) {
            ParseOutcome::Ok(req) => req,
            other => panic!("expected Ok, got {:?}", other_kind(&other)),
        };
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, json!(1));
        assert!(!req.is_notification());
    }

    #[test]
    fn parse_defaults_missing_id_to_null_and_marks_as_notification() {
        // JSON-RPC notifications omit the id member entirely.
        let raw = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let req = match parse_request(raw) {
            ParseOutcome::Ok(req) => req,
            _ => panic!("expected Ok"),
        };
        assert!(req.is_notification());
        assert_eq!(req.id, Value::Null);
    }

    #[test]
    fn parse_defaults_missing_params_to_empty_object() {
        // Some hosts (and our own ping handler) send method-only requests.
        let raw = r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#;
        let req = match parse_request(raw) {
            ParseOutcome::Ok(req) => req,
            _ => panic!("expected Ok"),
        };
        assert_eq!(req.params, json!({}));
    }

    #[test]
    fn parse_emits_canonical_parse_error_for_invalid_json() {
        // The canonical response: id=null, code=-32700, message starts
        // with "Parse error". We don't pin the exact serde message
        // because that varies across versions.
        let raw = "{ this isn't json";
        let resp_str = match parse_request(raw) {
            ParseOutcome::ParseError(s) => s,
            _ => panic!("expected ParseError"),
        };
        let v = parse_response(&resp_str);
        assert_eq!(v["error"]["code"], -32700);
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .starts_with("Parse error"),
            "got {:?}",
            v["error"]["message"]
        );
        assert_eq!(v["id"], Value::Null);
    }

    #[test]
    fn oversized_error_uses_parse_error_code_and_null_id() {
        let v = parse_response(&oversized_error());
        assert_eq!(v["error"]["code"], -32700);
        assert_eq!(v["id"], Value::Null);
        assert!(v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("size limit"));
    }

    /// Exists purely so the panic message in `parse_returns_typed_request_for_valid_json`
    /// can name the variant we got back without exposing the type to callers.
    fn other_kind(o: &ParseOutcome) -> &'static str {
        match o {
            ParseOutcome::Empty => "Empty",
            ParseOutcome::Ok(_) => "Ok",
            ParseOutcome::ParseError(_) => "ParseError",
        }
    }
}
