//! Ephemeral ACP sessions: create a throwaway session, do one self-
//! contained piece of work on it, and tear it down so nothing leaks into
//! the user's chat UI or the on-disk session list.
//!
//! # Why this exists
//!
//! Several features need the agent for a one-shot task that isn't part of
//! any conversation: generating a script from a prompt (Settings → script
//! editor), and — candidates for migration — AI session titling and
//! quit-time steering. The wrong way to do this is to borrow a real
//! window's session: it may not exist (Settings is often open with no
//! chat window, so `get_window_session` returns `null`), and even when it
//! does, the hidden prompt and its reply pollute the user's actual
//! conversation history.
//!
//! [`run`] is the single primitive: it ensures the connection is up,
//! creates a fresh session, hands the id to a caller-supplied closure,
//! and guarantees the session files are removed afterwards (via an RAII
//! guard, so an early `?` or a panic in the closure still cleans up). The
//! teardown deletes the session files directly rather than routing through
//! the `delete_session` command, because we don't want its chat-host
//! `session_changed` broadcast — no window ever knew about this session.
//!
//! Run it from a blocking context (`spawn_blocking`): the ACP client's
//! `create_session` / `send_chat_streaming` are synchronous and block the
//! calling thread until the agent replies.

use anyhow::{Context, Result};
use log::warn;
use std::sync::{Arc, Mutex};

use crate::acp_client::AcpClient;
use crate::config::Config;
use crate::lock_ext::LockExt;

/// Deletes an ephemeral session's on-disk files when dropped. Best-effort:
/// a failure leaves a stray file the user can delete manually, so we log
/// rather than propagate. Holding this as a `let _guard = …` binding means
/// cleanup runs on every exit path out of [`run`] — normal return, `?`
/// early-return, or a panic unwinding through the closure.
struct SessionCleanup {
    config: Arc<Mutex<Config>>,
    session_id: String,
}

impl Drop for SessionCleanup {
    fn drop(&mut self) {
        let sessions_dir = {
            let cfg = self.config.lock_or_recover();
            crate::agent_presets::resolve_sessions_dir(&cfg)
        };
        let Some(dir) = sessions_dir else {
            warn!(
                "Could not resolve sessions dir to clean up ephemeral session {}",
                self.session_id
            );
            return;
        };
        for ext in &["json", "jsonl", "lock"] {
            let path = dir.join(format!("{}.{}", self.session_id, ext));
            if path.exists() {
                if let Err(e) = std::fs::remove_file(&path) {
                    warn!("Failed to delete ephemeral session file {:?}: {}", path, e);
                }
            }
        }
    }
}

/// Run `work` against a fresh, throwaway ACP session, then delete it.
///
/// Connects the client if needed, creates a new session (no steering
/// primer — ephemeral work is self-contained, not a conversation), invokes
/// `work` with the new session id, and tears the session down regardless of
/// how `work` returns. The closure's value is propagated to the caller.
///
/// Call from a blocking context — the ACP calls inside `work` (and the
/// `create_session` here) are synchronous.
///
/// ```ignore
/// let reply = ephemeral_session::run(&client, &config, |sid| {
///     client.send_chat_streaming(sid, &prompt, None)?;
///     Ok(client.take_session_accumulator(sid))
/// })?;
/// ```
pub fn run<T, F>(client: &Arc<AcpClient>, config: &Arc<Mutex<Config>>, work: F) -> Result<T>
where
    F: FnOnce(&str) -> Result<T>,
{
    if !client.is_connected() {
        client
            .connect()
            .context("Failed to connect for ephemeral session")?;
    }

    let cwd = {
        let cfg = config.lock_or_recover();
        cfg.acp.agent.working_directory.clone()
    };

    let (session_id, _) = client
        .create_session(cwd)
        .context("Failed to create ephemeral session")?;

    // Arm cleanup before running the work so an error or panic in `work`
    // still removes the session files.
    let _cleanup = SessionCleanup {
        config: config.clone(),
        session_id: session_id.clone(),
    };

    work(&session_id)
}

/// Convenience wrapper for the common "send one prompt, get the full
/// reply" shape. Runs `prompt` on a fresh ephemeral session and returns
/// the agent's accumulated response text.
pub fn prompt_once(
    client: &Arc<AcpClient>,
    config: &Arc<Mutex<Config>>,
    prompt: &str,
) -> Result<String> {
    run(client, config, |session_id| {
        // send_chat_streaming resets this session's accumulator on entry
        // and blocks until the prompt completes; the full reply is then
        // sitting in the accumulator for us to take.
        client.send_chat_streaming(session_id, prompt, None)?;
        Ok(client.take_session_accumulator(session_id))
    })
}
