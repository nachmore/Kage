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
        // Before the write, so it holds even if the write fails: whatever
        // this turn ends with, it must not be resent automatically.
        self.mark_turn_cancel_requested(session_id);
        // And when no turn is open (a recovery restart in progress), so
        // the ladder doesn't resend it.
        self.note_cancel(session_id);
        self.send_notification(
            "session/cancel",
            serde_json::json!({ "sessionId": session_id }),
        )
    }

    /// Reply to the protocol's `session/request_permission` request, on
    /// whatever connection is current. The app answers with
    /// [`send_permission_response_on`](Self::send_permission_response_on).
    #[allow(dead_code)] // Integration tests and the mock agents use it.
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

    /// [`send_permission_response`](Self::send_permission_response) to the
    /// connection of `generation` only (see
    /// [`dispatch_generation`](Self::dispatch_generation)). `Ok(false)`:
    /// that connection is gone, nothing was written. A restarted agent
    /// numbers its requests afresh, so the old process's request id may now
    /// name an unrelated request of the new one.
    pub fn send_permission_response_on(
        &self,
        generation: u64,
        request_id: &serde_json::Value,
        option_id: &str,
    ) -> Result<bool> {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": { "outcome": { "outcome": "selected", "optionId": option_id } }
        });
        let line = serde_json::to_string(&response)?;
        self.transport.write_line_on(generation, &line)
    }

    /// The current connection generation; it moves on with every
    /// connection and every teardown.
    pub fn connection_generation(&self) -> u64 {
        self.transport.generation()
    }

    /// The generation of the connection a message being handled came
    /// from: exact on the reader thread (the notification handler).
    pub fn dispatch_generation(&self) -> u64 {
        self.transport.dispatch_generation()
    }
}
