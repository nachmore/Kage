//! ACP protocol types: JSON-RPC request, response, notification, and error structures.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AcpError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct AcpNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Format an ACP error including the data field if present
pub fn format_acp_error(error: &AcpError) -> String {
    match &error.data {
        Some(data) => {
            let data_str = match data {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            format!("{} — {}", error.message, data_str)
        }
        None => format!("{} (code: {})", error.message, error.code),
    }
}

/// JSON-RPC "invalid params". Never restarts the agent (only
/// `JSONRPC_INTERNAL_ERROR` can); named for tests and log readers.
#[allow(dead_code)]
pub const JSONRPC_INVALID_PARAMS: i32 = -32602;
/// JSON-RPC "method not found". Never restarts the agent either.
#[allow(dead_code)]
pub const JSONRPC_METHOD_NOT_FOUND: i32 = -32601;
/// JSON-RPC "internal error": the agent's catch-all, and what kiro-cli
/// answers with when it can't reach its model service (no token, outage).
pub const JSONRPC_INTERNAL_ERROR: i32 = -32603;
/// ACP "authentication required": the agent needs the user to sign in.
pub const ACP_AUTH_REQUIRED: i32 = -32000;

/// A JSON-RPC error reply from the agent, kept typed so the recovery ladder
/// can decide on the numeric code instead of on text. `Display` is exactly
/// the text Kage used to build for the same reply, so logs, the UI and every
/// substring check stay as they were.
#[derive(Debug, Clone)]
pub struct AgentRpcError {
    /// The request the agent refused: `session/prompt`, `session/new`,
    /// `session/load` or `initialize`. For debugging and tests.
    #[allow(dead_code)]
    pub method: &'static str,
    pub code: i32,
    pub message: String,
    /// The reply's `data` when it is a non-empty string (the agent's own
    /// detail).
    pub detail: Option<String>,
    /// The legacy rendering, shown by `Display`.
    pub rendered: String,
    /// What the turn did before it failed (prompts only; default otherwise).
    pub progress: super::turn_progress::TurnProgress,
}

impl AgentRpcError {
    /// Build one from an agent reply. `rendered` is the text `Display` gives.
    pub fn from_reply(method: &'static str, error: &AcpError, rendered: String) -> Self {
        let detail = error
            .data
            .as_ref()
            .and_then(|d| d.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Self {
            method,
            code: error.code,
            message: error.message.clone(),
            detail,
            rendered,
            progress: Default::default(),
        }
    }
}

/// A request that provably never reached the agent: there was no
/// connection to write it to, or the write failed before any byte of it
/// went out. `Display` is the underlying error's own text, so the substring
/// checks that classify failures ("No write handle", "Broken pipe") see what
/// they always saw. A write that failed part-way is not this: the agent may
/// have read a whole line by then.
#[derive(Debug, Clone)]
pub struct RequestNotSent {
    pub detail: String,
}

impl std::fmt::Display for RequestNotSent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for RequestNotSent {}

impl std::fmt::Display for AgentRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.rendered)
    }
}

impl std::error::Error for AgentRpcError {}

/// Whether `err` is the agent's ACP "authentication required" reply.
pub fn is_auth_required(err: &anyhow::Error) -> bool {
    err.downcast_ref::<AgentRpcError>()
        .is_some_and(|e| e.code == ACP_AUTH_REQUIRED)
}

/// The end of the automatic recovery ladder: Kage restarted the agent and
/// the prompt failed the same way again (or did so within the last minute).
#[derive(Debug)]
pub enum PromptRecoveryError {
    AgentUnreachable { detail: String },
}

impl std::fmt::Display for PromptRecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentUnreachable { detail } => write!(
                f,
                "Agent still failing after an automatic restart: {}",
                detail
            ),
        }
    }
}

impl std::error::Error for PromptRecoveryError {}

/// Connection mode for the ACP client
#[derive(Clone)]
pub enum AcpConnectionMode {
    Local { spawn_command: String },
    Remote { host: String, port: u16 },
}

/// Callback type for handling notifications from the background reader.
/// The outer `Arc<Mutex<Option<_>>>` lets us swap the handler at runtime.
/// The inner `Arc<dyn Fn ... + Send + Sync>` is cloned out of the mutex before
/// invocation so the reader thread never holds this lock across the callback —
/// that would cause deadlocks whenever the callback re-acquires a lock that
/// the main thread holds while waiting for a response.
pub type NotificationHandler = Arc<Mutex<Option<Arc<dyn Fn(serde_json::Value) + Send + Sync>>>>;
