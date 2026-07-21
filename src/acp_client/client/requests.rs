use anyhow::Result;
use log::info;

use super::super::AcpClient;

impl AcpClient {
    /// Send a JSON-RPC notification (no id, fire-and-forget).
    pub fn send_notification(&self, method: &str, params: serde_json::Value) -> Result<()> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let line = serde_json::to_string(&notification)?;
        self.transport.write_line(&line)
    }

    /// Notify consumers that recovery continued the live turn on a new
    /// session. This preserves the existing notification-handler fan-out.
    pub fn notify_session_migrated(&self, old_id: &str, new_id: &str) {
        if old_id == new_id {
            return;
        }
        info!("Session migrated mid-turn: {} → {}", old_id, new_id);
        self.transport
            .dispatch_synthetic_notification(serde_json::json!({
                "jsonrpc": "2.0",
                "method": "_kage/session_migrated",
                "params": { "oldSessionId": old_id, "newSessionId": new_id },
            }));
    }

    /// Cancel the prompt currently in flight for one session.
    pub fn cancel_session(&self, session_id: &str) -> Result<()> {
        info!("Sending session/cancel for session {}", session_id);
        self.send_notification(
            "session/cancel",
            serde_json::json!({ "sessionId": session_id }),
        )
    }

    /// Reply to the protocol's `session/request_permission` request.
    pub fn send_permission_response(
        &self,
        request_id: &serde_json::Value,
        option_id: &str,
    ) -> Result<()> {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": { "outcome": { "outcome": "selected", "optionId": option_id } }
        });
        let line = serde_json::to_string(&response)?;
        self.transport.write_line(&line)
    }
}
