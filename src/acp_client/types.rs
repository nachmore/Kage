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

/// Connection mode for the ACP client
pub enum AcpConnectionMode {
    Local { spawn_command: String },
    Remote { host: String, port: u16 },
}

/// Callback type for handling notifications from the background reader
pub type NotificationHandler = Arc<Mutex<Option<Box<dyn Fn(serde_json::Value) + Send>>>>;
