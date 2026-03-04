//! ACP session and protocol methods: initialize, create/load sessions,
//! send chat messages, steering, and error recovery.

use anyhow::{Context, Result};
use log::{info, warn};

use super::AcpClient;
use super::types::{AcpRequest, format_acp_error};

impl AcpClient {
    // --- Protocol handshake ---

    pub fn initialize(&self) -> Result<()> {
        info!("Initializing ACP connection");

        let request = AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(0),
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": true, "writeTextFile": true },
                    "terminal": true
                },
                "clientInfo": {
                    "name": "kiro-assistant",
                    "title": "Kiro Assistant",
                    "version": "0.1.0"
                }
            }),
        };

        let response = self.send_request(&request)?;
        if let Some(error) = response.error {
            anyhow::bail!("Initialize failed: {}", format_acp_error(&error));
        }

        info!("ACP initialized successfully");
        *self.initialized.lock().unwrap() = true;
        Ok(())
    }

    // --- Session management ---

    pub fn create_session(&self, cwd: Option<String>) -> Result<(String, Vec<serde_json::Value>)> {
        info!("Creating new ACP session");

        {
            let init = self.initialized.lock().unwrap();
            if !*init {
                drop(init);
                self.initialize()?;
            }
        }

        let cwd = cwd.unwrap_or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "/".to_string())
        });

        let request = AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            method: "session/new".to_string(),
            params: serde_json::json!({ "cwd": cwd, "mcpServers": [] }),
        };

        let response = self.send_request(&request)?;
        if let Some(error) = response.error {
            anyhow::bail!("Session creation failed: {}", format_acp_error(&error));
        }

        let result = response.result.context("No result in session/new response")?;
        let session_id = result.get("sessionId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .context("No sessionId in response")?;

        let mut models_list = Vec::new();
        if let Some(models) = result.get("models").and_then(|m| m.get("availableModels")).and_then(|a| a.as_array()) {
            info!("Session has {} available models", models.len());
            models_list = models.clone();
        }

        info!("Session created: {}", session_id);
        *self.session_id.lock().unwrap() = Some(session_id.clone());
        Ok((session_id, models_list))
    }

    pub fn load_existing_session(&self, session_id: &str, cwd: Option<String>) -> Result<String> {
        info!("Loading existing ACP session: {}", session_id);

        {
            let init = self.initialized.lock().unwrap();
            if !*init {
                drop(init);
                self.initialize()?;
            }
        }

        let cwd = cwd.unwrap_or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "/".to_string())
        });

        let request = AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            method: "session/load".to_string(),
            params: serde_json::json!({
                "sessionId": session_id,
                "cwd": cwd,
                "mcpServers": []
            }),
        };

        let response = self.send_request(&request)?;
        if let Some(error) = response.error {
            anyhow::bail!("Session load failed: {}", format_acp_error(&error));
        }

        info!("Session loaded: {}", session_id);
        *self.session_id.lock().unwrap() = Some(session_id.to_string());
        Ok(session_id.to_string())
    }

    // --- Steering ---

    pub fn send_builtin_steering(&self) {
        let session_id = match self.get_session_id() {
            Some(id) => id,
            None => return,
        };

        let steering_msg = format!(
            "{} {}",
            crate::commands::system::STEERING_MSG_PREFIX,
            crate::commands::system::BUILTIN_STEERING
        );

        *self.streaming_accumulator.lock().unwrap() = String::new();

        let request = AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(98),
            method: "session/prompt".to_string(),
            params: serde_json::json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": steering_msg }]
            }),
        };

        match self.send_request(&request) {
            Ok(_) => info!("Built-in steering sent to session {}", session_id),
            Err(e) => warn!("Failed to send built-in steering: {}", e),
        }
    }

    // --- Chat streaming ---

    pub fn send_chat_streaming(&self, content: &str, attachments: Option<&[serde_json::Value]>) -> Result<()> {
        let debug = *self.transport.debug_mode.lock().unwrap();

        *self.streaming_accumulator.lock().unwrap() = String::new();

        if debug {
            info!("[CHAT] Sending message ({} chars): {}", content.chars().count(), content);
        } else {
            info!("Sending chat message ({} chars)", content.chars().count());
        }

        let session_id = {
            let guard = self.session_id.lock().unwrap();
            if let Some(ref id) = *guard {
                id.clone()
            } else {
                drop(guard);
                let (id, _) = self.create_session(None)?;
                self.send_builtin_steering();
                id
            }
        };

        let mut prompt: Vec<serde_json::Value> = Vec::new();
        if !content.is_empty() {
            prompt.push(serde_json::json!({ "type": "text", "text": content }));
        }
        if let Some(att) = attachments {
            for block in att {
                prompt.push(block.clone());
            }
        }
        if prompt.is_empty() {
            prompt.push(serde_json::json!({ "type": "text", "text": "" }));
        }

        let request = AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(2),
            method: "session/prompt".to_string(),
            params: serde_json::json!({
                "sessionId": session_id,
                "prompt": prompt
            }),
        };

        let response = self.send_request(&request)?;

        if let Some(error) = response.error {
            let detail = error.data.as_ref().and_then(|d| d.as_str()).unwrap_or("");
            if detail.is_empty() {
                anyhow::bail!("ACP error: {}", error.message);
            } else {
                anyhow::bail!("ACP error: {} — {}", error.message, detail);
            }
        }

        info!("Prompt completed");
        Ok(())
    }

    // --- Recovery ---

    /// Send a chat message with automatic recovery on timeout/disconnect.
    ///
    /// Strategy: try normally → restart + reload session → restart + fresh session.
    pub fn send_chat_streaming_with_recovery(
        &self,
        content: String,
        attachments: Option<Vec<serde_json::Value>>,
    ) -> Result<()> {
        let att_ref = attachments.as_deref();

        // --- Attempt 1: normal send ---
        match self.send_chat_streaming(&content, att_ref) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let err_str = format!("{}", e);
                if Self::is_recoverable_error(&err_str) {
                    warn!("Prompt failed ({}), attempting recovery…", err_str);
                    if Self::is_corrupted_session(&err_str) {
                        warn!("Session corrupted — skipping reload, creating fresh session");
                        self.restart_connection()?;
                        self.set_session_id(None);
                        self.create_session(None)?;
                        self.send_builtin_steering();
                        return self.send_chat_streaming(&content, att_ref);
                    }
                } else {
                    return Err(e);
                }
            }
        }

        // --- Attempt 2: restart + reload session + resend ---
        let old_session_id = self.get_session_id();
        self.restart_connection()?;

        let mut session_restored = false;
        if let Some(ref sid) = old_session_id {
            info!("Attempting to reload session {} after restart", sid);
            match self.load_existing_session(sid, None) {
                Ok(_) => {
                    info!("Session {} reloaded successfully", sid);
                    session_restored = true;
                }
                Err(e) => {
                    warn!("Could not reload session {}: {}", sid, e);
                }
            }
        }

        if !session_restored {
            info!("Creating fresh session for retry");
            self.set_session_id(None);
            self.create_session(None)?;
            self.send_builtin_steering();
        }

        match self.send_chat_streaming(&content, att_ref) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let err_str = format!("{}", e);
                if Self::is_recoverable_error(&err_str) {
                    warn!("Prompt failed again after session reload ({}), trying fresh session…", err_str);
                } else {
                    return Err(e);
                }
            }
        }

        // --- Attempt 3: restart + brand-new session + resend (last chance) ---
        self.restart_connection()?;
        self.set_session_id(None);
        self.create_session(None)?;
        self.send_builtin_steering();

        self.send_chat_streaming(&content, att_ref)
    }

    // --- Error classification ---

    fn is_recoverable_error(err_str: &str) -> bool {
        err_str.contains("Timeout waiting for response")
            || err_str.contains("Connection lost")
            || err_str.contains("No reader thread")
            || err_str.contains("No write handle")
            || err_str.contains("Broken pipe")
            || err_str.contains("invalid conversation history")
            || err_str.contains("panicked")
    }

    fn is_corrupted_session(err_str: &str) -> bool {
        if err_str.contains("Timeout") || err_str.contains("Connection lost") {
            return false;
        }
        err_str.contains("invalid conversation history")
            || err_str.contains("panicked")
    }
}
