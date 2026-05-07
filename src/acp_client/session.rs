//! ACP session and protocol methods: initialize, create/load sessions,
//! send chat messages, steering, and error recovery.

use anyhow::{Context, Result};
use log::{info, warn};
use std::sync::Mutex;
use std::time::Instant;

use super::types::format_acp_error;
use super::AcpClient;
use crate::lock_ext::LockExt;

/// Track when we last injected a timestamp into a user message.
/// Refreshed every 15 minutes to keep the agent's sense of time current.
static LAST_TIMESTAMP_INJECTION: std::sync::LazyLock<Mutex<Option<Instant>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));
static LAST_TIMESTAMP_DATE: std::sync::LazyLock<Mutex<String>> =
    std::sync::LazyLock::new(|| Mutex::new(String::new()));

const TIMESTAMP_REFRESH_SECS: u64 = 15 * 60; // 15 minutes

impl AcpClient {
    // --- Protocol handshake ---

    pub fn initialize(&self) -> Result<()> {
        info!("Initializing ACP connection");

        let response = self.send_request(
            "initialize",
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": true, "writeTextFile": true },
                    "terminal": true
                },
                "clientInfo": {
                    "name": "kage",
                    "title": "Kage",
                    "version": "0.1.0"
                }
            }),
        )?;
        if let Some(error) = response.error {
            anyhow::bail!("Initialize failed: {}", format_acp_error(&error));
        }

        info!("ACP initialized successfully");
        *self.initialized.lock_or_recover() = true;
        Ok(())
    }

    // --- Session management ---

    pub fn create_session(&self, cwd: Option<String>) -> Result<(String, Vec<serde_json::Value>)> {
        info!("Creating new ACP session");

        {
            let init = self.initialized.lock_or_recover();
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

        let response = self.send_request(
            "session/new",
            serde_json::json!({ "cwd": cwd, "mcpServers": [] }),
        )?;
        if let Some(error) = response.error {
            anyhow::bail!("Session creation failed: {}", format_acp_error(&error));
        }

        let result = response
            .result
            .context("No result in session/new response")?;
        let session_id = result
            .get("sessionId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .context("No sessionId in response")?;

        let mut models_list = Vec::new();
        if let Some(models) = result
            .get("models")
            .and_then(|m| m.get("availableModels"))
            .and_then(|a| a.as_array())
        {
            info!("Session has {} available models", models.len());
            models_list = models.clone();
        }

        info!("Session created: {}", session_id);
        *self.session_id.lock_or_recover() = Some(session_id.clone());
        Ok((session_id, models_list))
    }

    pub fn load_existing_session(&self, session_id: &str, cwd: Option<String>) -> Result<String> {
        info!("Loading existing ACP session: {}", session_id);

        {
            let init = self.initialized.lock_or_recover();
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

        let response = self.send_request(
            "session/load",
            serde_json::json!({
                "sessionId": session_id,
                "cwd": cwd,
                "mcpServers": []
            }),
        )?;
        if let Some(error) = response.error {
            anyhow::bail!("Session load failed: {}", format_acp_error(&error));
        }

        info!("Session loaded: {}", session_id);
        *self.session_id.lock_or_recover() = Some(session_id.to_string());
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
            crate::auto_steering::STEERING_MSG_PREFIX,
            crate::auto_steering::BUILTIN_STEERING
        );

        self.reset_session_accumulator(&session_id);

        let result = self.send_request(
            "session/prompt",
            serde_json::json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": steering_msg }]
            }),
        );

        match result {
            Ok(_) => info!("Built-in steering sent to session {}", session_id),
            Err(e) => warn!("Failed to send built-in steering: {}", e),
        }
    }

    // --- Chat streaming ---

    pub fn send_chat_streaming(
        &self,
        content: &str,
        attachments: Option<&[serde_json::Value]>,
    ) -> Result<()> {
        // Wait for any in-progress compaction to finish before sending
        self.wait_for_compaction();

        let debug = *self.transport.debug_mode.lock_or_recover();

        if debug {
            info!(
                "[CHAT] Sending message ({} chars): {}",
                content.chars().count(),
                content
            );
        } else {
            info!("Sending chat message ({} chars)", content.chars().count());
        }

        let session_id = {
            let guard = self.session_id.lock_or_recover();
            if let Some(ref id) = *guard {
                id.clone()
            } else {
                drop(guard);
                let (id, _) = self.create_session(None)?;
                self.send_builtin_steering();
                id
            }
        };

        // Reset only this session's bucket — other sessions' in-flight
        // accumulators (auto-steering, sub-agents) must not be wiped.
        self.reset_session_accumulator(&session_id);

        let mut prompt: Vec<serde_json::Value> = Vec::new();

        // Periodically inject current timestamp so the agent's sense of time stays fresh
        {
            let mut last = LAST_TIMESTAMP_INJECTION.lock_or_recover();
            let elapsed = last.map(|t| t.elapsed().as_secs()).unwrap_or(u64::MAX);
            if elapsed >= TIMESTAMP_REFRESH_SECS {
                let now = chrono::Local::now();
                let today = now.format("%Y-%m-%d").to_string();
                let time = now.format("%H:%M").to_string();

                let mut last_date = LAST_TIMESTAMP_DATE.lock_or_recover();
                let ts = if *last_date == today {
                    // Same day — just the time
                    format!("[Current time: {}]", time)
                } else {
                    // Date changed — include full date
                    *last_date = today.clone();
                    format!("[Current time: {} {}]", today, time)
                };
                prompt.push(serde_json::json!({ "type": "text", "text": ts }));
                *last = Some(Instant::now());
            }
        }

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

        let response = self.send_request(
            "session/prompt",
            serde_json::json!({
                "sessionId": session_id,
                "prompt": prompt
            }),
        )?;

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
                    warn!(
                        "Prompt failed again after session reload ({}), trying fresh session…",
                        err_str
                    );
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
        err_str.contains("invalid conversation history") || err_str.contains("panicked")
    }

    // --- Sub-agent invocation ---

    /// Invoke a sub-agent with a specific task query.
    /// The sub-agent runs in a fresh context and returns its result
    /// through the normal streaming notification handler. Returns the
    /// accumulated response text — the bucket is consumed, so the caller
    /// gets the response as a value rather than reaching into the
    /// accumulator map themselves.
    pub fn invoke_subagent(&self, query: &str) -> Result<String> {
        let session_id = {
            let guard = self.session_id.lock_or_recover();
            if let Some(ref id) = *guard {
                id.clone()
            } else {
                drop(guard);
                let (id, _) = self.create_session(None)?;
                self.send_builtin_steering();
                id
            }
        };

        info!(
            "Invoking sub-agent with query: {}",
            &query[..query.len().min(100)]
        );

        // Reset this session's bucket so we read just the sub-agent's reply
        self.reset_session_accumulator(&session_id);

        let command = serde_json::json!({
            "command": "invoke_subagents",
            "content": {
                "subagents": [{
                    "query": query
                }]
            }
        });

        let response = self.send_request(
            "session/prompt",
            serde_json::json!({
                "sessionId": session_id,
                "prompt": [{
                    "type": "text",
                    "text": serde_json::to_string(&command).unwrap_or_default()
                }]
            }),
        )?;

        if let Some(error) = response.error {
            let detail = error.data.as_ref().and_then(|d| d.as_str()).unwrap_or("");
            if detail.is_empty() {
                anyhow::bail!("Sub-agent error: {}", error.message);
            } else {
                anyhow::bail!("Sub-agent error: {} — {}", error.message, detail);
            }
        }

        // Get the accumulated response from the sub-agent and clear its
        // bucket — this read is one-shot, no other caller wants it.
        let result = self.take_session_accumulator(&session_id);
        info!("Sub-agent completed ({} chars)", result.len());
        Ok(result)
    }
}
