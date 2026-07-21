//! ACP session and protocol methods: initialize, create/load sessions,
//! send chat messages, steering, and error recovery.

use anyhow::{Context, Result};
use log::{info, warn};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::types::format_acp_error;
use super::AcpClient;
use crate::lock_ext::LockExt;

/// RAII guard that marks a `session/load` replay window open for one
/// session id for its lifetime and clears it on drop — including the `?`
/// early-return paths in `load_existing_session`. While a session id is in
/// the set, the notification handler drops the conversation-history
/// `session/update`s that kiro-cli replays in answer to `session/load` for
/// that session, without touching live updates for other sessions (loads
/// overlap in the multi-session world). See `AcpClient::loading_sessions`.
struct LoadReplayGuard {
    set: Arc<Mutex<HashSet<String>>>,
    session_id: String,
}

impl LoadReplayGuard {
    fn new(set: &Arc<Mutex<HashSet<String>>>, session_id: &str) -> Self {
        set.lock_or_recover().insert(session_id.to_string());
        Self {
            set: set.clone(),
            session_id: session_id.to_string(),
        }
    }
}

impl Drop for LoadReplayGuard {
    fn drop(&mut self) {
        self.set.lock_or_recover().remove(&self.session_id);
    }
}

/// Track when we last injected a timestamp into a user message.
/// Refreshed every 15 minutes to keep the agent's sense of time current.
static LAST_TIMESTAMP_INJECTION: std::sync::LazyLock<Mutex<Option<Instant>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));
static LAST_TIMESTAMP_DATE: std::sync::LazyLock<Mutex<String>> =
    std::sync::LazyLock::new(|| Mutex::new(String::new()));

const TIMESTAMP_REFRESH_SECS: u64 = 15 * 60; // 15 minutes

/// Resolve an optional caller-supplied cwd, falling back to the process
/// working directory and finally to `/`. Used by `session/new` and
/// `session/load`, which must always send a concrete `cwd`.
fn resolve_cwd(cwd: Option<String>) -> String {
    cwd.unwrap_or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "/".to_string())
    })
}

/// Bail with `"<context>: <message>"`, appending ` — <detail>` when the
/// error carries a string `data` field. Used by the prompt/sub-agent paths
/// where we want the raw detail string rather than `format_acp_error`'s
/// `(code: N)` rendering.
fn bail_acp_error(context: &str, error: &super::types::AcpError) -> anyhow::Error {
    let detail = error.data.as_ref().and_then(|d| d.as_str()).unwrap_or("");
    if detail.is_empty() {
        anyhow::anyhow!("{}: {}", context, error.message)
    } else {
        anyhow::anyhow!("{}: {} — {}", context, error.message, detail)
    }
}

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

        let cwd = resolve_cwd(cwd);

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
        Ok((session_id, models_list))
    }

    /// Load an existing session and return any model list the agent included
    /// in its response. The ACP spec doesn't mandate `models.availableModels`
    /// for `session/load` — but kiro-cli ships the same session resource
    /// for both, so when it populates `models` on `session/new` it tends
    /// to populate it on `session/load` too. Returning the (possibly empty)
    /// list lets callers refresh the model dropdown after a resume; the
    /// previous "frontend will refetch when needed" comment was wishful
    /// thinking — nothing in the frontend ever refetched.
    pub fn load_existing_session(
        &self,
        session_id: &str,
        cwd: Option<String>,
    ) -> Result<(String, Vec<serde_json::Value>)> {
        info!("Loading existing ACP session: {}", session_id);

        {
            let init = self.initialized.lock_or_recover();
            if !*init {
                drop(init);
                self.initialize()?;
            }
        }

        let cwd = resolve_cwd(cwd);

        // Gate the conversation-history replay. kiro-cli answers
        // `session/load` by re-emitting every prior turn as a burst of
        // `session/update` notifications (on the reader thread) *before*
        // the load response returns. The notification handler drops
        // session/update for THIS session while it's in the loading set, so
        // the prior conversation doesn't dump into the floating window or
        // poison the streaming accumulators — while live chunks from other
        // sessions (and other windows' overlapping loads) stay unaffected.
        // The reader processes lines in order, so by the time `send_request`
        // returns the response, every replay notification ahead of it has
        // already been handled — clearing here (via the guard's Drop) can't
        // race with a still-pending replay chunk.
        let _replay_guard = LoadReplayGuard::new(&self.loading_sessions, session_id);

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

        let mut models_list = Vec::new();
        if let Some(result) = response.result.as_ref() {
            if let Some(models) = result
                .get("models")
                .and_then(|m| m.get("availableModels"))
                .and_then(|a| a.as_array())
            {
                info!("Resumed session has {} available models", models.len());
                models_list = models.clone();
            }
        }

        info!("Session loaded: {}", session_id);
        Ok((session_id.to_string(), models_list))
    }

    // --- Steering ---

    pub fn send_builtin_steering(&self, session_id: &str) {
        let steering_msg = format!(
            "{} {}",
            crate::auto_steering::STEERING_MSG_PREFIX,
            crate::auto_steering::BUILTIN_STEERING
        );

        // `send_prompt` resets the accumulator under the prompt lock.
        let result = self.send_prompt(
            session_id,
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
        session_id: &str,
        content: &str,
        attachments: Option<&[serde_json::Value]>,
    ) -> Result<()> {
        // Wait for any in-progress compaction to finish before sending
        self.wait_for_compaction();

        let debug = *self.transport.debug_mode.lock_or_recover();

        if debug {
            info!(
                "[CHAT] Sending message on {} ({} chars): {}",
                session_id,
                content.chars().count(),
                content
            );
        } else {
            info!(
                "Sending chat message on {} ({} chars)",
                session_id,
                content.chars().count()
            );
        }

        // `send_prompt` (below) resets only this session's bucket, under
        // the prompt lock — other sessions' in-flight accumulators
        // (auto-steering, sub-agents) are untouched, and a caller waiting
        // on the lock can't wipe the in-flight prompt's partial stream.
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

        let response = self.send_prompt(
            session_id,
            serde_json::json!({
                "sessionId": session_id,
                "prompt": prompt
            }),
        )?;

        if let Some(error) = response.error {
            return Err(bail_acp_error("ACP error", &error));
        }

        info!("Prompt completed");
        Ok(())
    }

    // --- Recovery ---

    /// The single "rebuild from scratch" recovery primitive: restart the
    /// connection (force-disconnect → respawn → initialize), create a fresh
    /// session, and prime it with built-in steering. Returns the new session
    /// id for the caller to adopt.
    ///
    /// Every "we can't keep the old session, start clean" path funnels
    /// through here — the corrupted-session branch and last-chance attempt of
    /// `send_chat_streaming_with_recovery`, plus the image-error handler in
    /// `commands::messaging`. Keeping it in one place means there's exactly
    /// one respawn-and-reset sequence to reason about (and the respawn itself
    /// is the single chokepoint in `transport::spawn_backend_process`, which
    /// reaps the previous child first).
    pub fn restart_with_fresh_session(&self) -> Result<String> {
        self.restart_connection()?;
        let (new_id, _) = self.create_session(None)?;
        self.send_builtin_steering(&new_id);
        Ok(new_id)
    }

    /// Send a chat message with automatic recovery on timeout/disconnect.
    ///
    /// Strategy: try normally → restart + reload session → restart + fresh session.
    /// Returns the session id that was actually used to send — same as the
    /// input on success or transient-failure recovery, but on
    /// "session corrupted" / "fresh session" recovery the returned id is the
    /// new session id that the caller should adopt.
    pub fn send_chat_streaming_with_recovery(
        &self,
        session_id: String,
        content: String,
        attachments: Option<Vec<serde_json::Value>>,
    ) -> Result<String> {
        let att_ref = attachments.as_deref();

        // --- Attempt 1: normal send ---
        match self.send_chat_streaming(&session_id, &content, att_ref) {
            Ok(()) => return Ok(session_id),
            Err(e) => {
                let err_str = format!("{}", e);
                if Self::is_recoverable_error(&err_str) {
                    warn!("Prompt failed ({}), attempting recovery…", err_str);
                    if Self::is_corrupted_session(&err_str) {
                        warn!("Session corrupted — skipping reload, creating fresh session");
                        let new_id = self.restart_with_fresh_session()?;
                        // The fresh session's steering reply accumulated under
                        // new_id; drop it so the resend renders clean, then tell
                        // live windows to adopt new_id before the resend streams.
                        self.reset_session_accumulator(&new_id);
                        self.notify_session_migrated(&session_id, &new_id);
                        self.send_chat_streaming(&new_id, &content, att_ref)?;
                        return Ok(new_id);
                    }
                } else {
                    return Err(e);
                }
            }
        }

        // --- Attempt 2: restart + reload session + resend ---
        self.restart_connection()?;

        let original_id = session_id.clone();
        let session_id = match self.load_existing_session(&session_id, None) {
            Ok(_) => {
                info!("Session {} reloaded successfully", session_id);
                session_id
            }
            Err(e) => {
                warn!("Could not reload session {}: {}", session_id, e);
                info!("Creating fresh session for retry");
                let (new_id, _) = self.create_session(None)?;
                self.send_builtin_steering(&new_id);
                // Drop the steering reply and tell live windows to adopt the
                // fresh id before the resend streams under it. Reloading onto
                // the same id needs neither (the id didn't change).
                self.reset_session_accumulator(&new_id);
                self.notify_session_migrated(&original_id, &new_id);
                new_id
            }
        };

        match self.send_chat_streaming(&session_id, &content, att_ref) {
            Ok(()) => return Ok(session_id),
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
        let new_id = self.restart_with_fresh_session()?;
        self.reset_session_accumulator(&new_id);
        self.notify_session_migrated(&session_id, &new_id);
        self.send_chat_streaming(&new_id, &content, att_ref)?;
        Ok(new_id)
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
            // The agent was killed and lazily respawned: the fresh process
            // has no record of our session id, so the first `session/prompt`
            // comes back as "No session found with id". Treat it as
            // recoverable (but NOT corrupted) so we fall through to attempt 2,
            // which re-initializes and tries `session/load` to restore the
            // conversation before giving up and creating a fresh session.
            || err_str.contains("No session found")
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
    pub fn invoke_subagent(&self, session_id: &str, query: &str) -> Result<String> {
        info!(
            "Invoking sub-agent on {} with query: {}",
            session_id,
            query.chars().take(100).collect::<String>()
        );

        // `send_prompt` resets this session's bucket under the prompt
        // lock, so the read below sees just the sub-agent's reply.
        let command = serde_json::json!({
            "command": "invoke_subagents",
            "content": {
                "subagents": [{
                    "query": query
                }]
            }
        });

        let response = self.send_prompt(
            session_id,
            serde_json::json!({
                "sessionId": session_id,
                "prompt": [{
                    "type": "text",
                    "text": serde_json::to_string(&command).unwrap_or_default()
                }]
            }),
        )?;

        if let Some(error) = response.error {
            return Err(bail_acp_error("Sub-agent error", &error));
        }

        // Get the accumulated response from the sub-agent and clear its
        // bucket — this read is one-shot, no other caller wants it.
        let result = self.take_session_accumulator(session_id);
        info!("Sub-agent completed ({} chars)", result.len());
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::{AcpClient, LoadReplayGuard};
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    #[test]
    fn load_replay_guard_sets_and_clears_own_session() {
        let set: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        {
            let _g = LoadReplayGuard::new(&set, "session-a");
            assert!(
                set.lock().unwrap().contains("session-a"),
                "session marked loading for guard lifetime"
            );
        }
        assert!(
            !set.lock().unwrap().contains("session-a"),
            "session cleared when guard drops"
        );
    }

    #[test]
    fn overlapping_load_guards_are_independent() {
        // The regression this design fixes: two loads overlap, the first
        // finishing must NOT unmask the second's still-running replay, and
        // each gate only masks its own session.
        let set: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let g_a = LoadReplayGuard::new(&set, "session-a");
        let g_b = LoadReplayGuard::new(&set, "session-b");
        assert!(
            !set.lock().unwrap().contains("session-c"),
            "unrelated session unmasked"
        );
        drop(g_a);
        assert!(
            set.lock().unwrap().contains("session-b"),
            "finishing load A must not unmask load B's replay"
        );
        assert!(!set.lock().unwrap().contains("session-a"));
        drop(g_b);
        assert!(set.lock().unwrap().is_empty());
    }

    #[test]
    fn no_session_found_is_recoverable_but_not_corrupted() {
        // A killed-then-respawned agent rejects the first prompt with
        // "No session found with id" because the fresh process never saw
        // our session. This must route through attempt 2 (re-init +
        // session/load), NOT the corrupted-session path that throws the
        // prior conversation away on a fresh session.
        let err = "ACP error: Internal error — No session found with id";
        assert!(AcpClient::is_recoverable_error(err));
        assert!(!AcpClient::is_corrupted_session(err));
    }

    #[test]
    fn corrupted_history_skips_reload() {
        // Genuinely corrupted history can't be reloaded — go straight to a
        // fresh session.
        let err = "ACP error: invalid conversation history";
        assert!(AcpClient::is_recoverable_error(err));
        assert!(AcpClient::is_corrupted_session(err));
    }

    #[test]
    fn transient_errors_are_recoverable_but_not_corrupted() {
        for err in [
            "Timeout waiting for response to session/prompt",
            "Connection lost while waiting for response",
            "No write handle available",
        ] {
            assert!(AcpClient::is_recoverable_error(err), "{err}");
            assert!(!AcpClient::is_corrupted_session(err), "{err}");
        }
    }

    #[test]
    fn unrelated_errors_are_not_recoverable() {
        let err = "ACP error: model declined to respond";
        assert!(!AcpClient::is_recoverable_error(err));
    }

    #[test]
    fn query_log_truncation_is_char_boundary_safe() {
        // invoke_subagent truncates the query for its info! log. The old
        // `&query[..query.len().min(100)]` byte-slice panicked when a
        // multibyte char straddled byte 100 (any non-ASCII query — the app
        // ships 31 locales). The char-based form must never panic.
        // A 3-byte char (、) repeated so a char boundary does NOT fall on 100.
        let query = "、".repeat(60); // 180 bytes, boundaries at multiples of 3
        assert!(
            !query.is_char_boundary(100),
            "byte 100 must be mid-char for this test to be meaningful"
        );
        let truncated: String = query.chars().take(100).collect();
        // 100 chars of a 60-char string is the whole string; take a longer one.
        let long = "、".repeat(200);
        let truncated_long: String = long.chars().take(100).collect();
        assert_eq!(truncated, query);
        assert_eq!(truncated_long.chars().count(), 100);
    }
}
