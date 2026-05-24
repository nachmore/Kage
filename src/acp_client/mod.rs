//! ACP (Agent Communication Protocol) client.
//!
//! Split into:
//! - `types`: Protocol types (AcpRequest, AcpResponse, AcpError, etc.)
//! - `transport`: Connection management, pipe/TCP I/O, background reader thread
//! - This module: `AcpClient` facade composing the above with session/protocol logic

mod session;
pub mod transport;
pub mod types;

// Re-export public types so callers can still use `crate::acp_client::AcpRequest` etc.
pub use transport::AcpTransport;
#[allow(unused_imports)]
pub use types::{
    format_acp_error, AcpConnectionMode, AcpError, AcpNotification, AcpRequest, AcpResponse,
    NotificationHandler,
};

use anyhow::Result;
use log::info;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::lock_ext::LockExt;
use crate::process_manager::ProcessManager;

/// Maximum size for any single session's streaming accumulator (10 MB).
/// Prevents OOM if the server sends an unbounded response on a single session.
pub const MAX_ACCUMULATOR_SIZE: usize = 10 * 1024 * 1024;

/// Accumulated streaming text, keyed by session id. Each session has its own
/// bucket so chunks for session A can't leak into session B if the user
/// switches mid-stream (or auto_steering's hidden prompt overlaps with the
/// user's prompt). The notification handler appends the chunk to the bucket
/// matching its `params.sessionId`; helpers like auto_steering and the
/// inline-assist command read+clear by the session id they sent to.
pub type SessionAccumulators = Arc<Mutex<HashMap<String, String>>>;

pub struct AcpClient {
    transport: AcpTransport,
    session_id: Arc<Mutex<Option<String>>>,
    initialized: Arc<Mutex<bool>>,
    /// Per-session accumulated streaming text. See SessionAccumulators.
    pub streaming_accumulators: SessionAccumulators,
    /// True while the server is compacting context — outgoing prompts should wait
    pub compacting: Arc<(Mutex<bool>, std::sync::Condvar)>,
    /// Vendor extension namespace observed from incoming notifications.
    /// kage-cli uses `_kage.dev/`, kiro-cli uses `_kiro.dev/`; the two
    /// share an identical extension surface (commands/available,
    /// metadata, commands/execute, compaction/status, ...) under
    /// different vendor prefixes. We pin to whichever we see first so
    /// outgoing requests target the right namespace, and the
    /// notification handler matches both interchangeably (see
    /// `vendor_method_suffix`).
    pub vendor_prefix: Arc<Mutex<Option<&'static str>>>,
}

/// Recognised JSON-RPC vendor extension prefixes. Both projects ship
/// the same protocol shape under different prefixes — see the comment
/// on `AcpClient::vendor_prefix`.
pub const VENDOR_PREFIXES: &[&str] = &["_kage.dev/", "_kiro.dev/"];

/// Default outgoing vendor prefix used until we've observed an inbound
/// notification telling us which namespace the agent expects.
pub const DEFAULT_VENDOR_PREFIX: &str = "_kage.dev/";

/// If `method` is a vendor extension call, return the suffix after the
/// prefix (e.g. `_kiro.dev/commands/available` → `Some("commands/available")`).
/// Returns `None` for plain ACP methods like `session/load`.
pub fn vendor_method_suffix(method: &str) -> Option<&str> {
    for p in VENDOR_PREFIXES {
        if let Some(rest) = method.strip_prefix(p) {
            return Some(rest);
        }
    }
    None
}

impl AcpClient {
    pub fn new(mode: AcpConnectionMode) -> Self {
        Self {
            transport: AcpTransport::new(mode),
            session_id: Arc::new(Mutex::new(None)),
            initialized: Arc::new(Mutex::new(false)),
            streaming_accumulators: Arc::new(Mutex::new(HashMap::new())),
            compacting: Arc::new((Mutex::new(false), std::sync::Condvar::new())),
            vendor_prefix: Arc::new(Mutex::new(None)),
        }
    }

    /// Record the vendor prefix observed in an inbound method name. Idempotent
    /// — once pinned, subsequent calls are no-ops. We pin to the first
    /// observed prefix because mixing namespaces inside one session would
    /// indicate a misbehaving agent, not a feature worth supporting.
    pub fn observe_vendor_prefix(&self, method: &str) {
        for p in VENDOR_PREFIXES {
            if method.starts_with(p) {
                let mut slot = self.vendor_prefix.lock_or_recover();
                if slot.is_none() {
                    log::info!("Observed vendor extension prefix from agent: {}", p);
                    *slot = Some(*p);
                }
                return;
            }
        }
    }

    /// Vendor prefix to use for outgoing extension requests. Returns the
    /// observed prefix if any, otherwise the default (`_kage.dev/`). The
    /// returned slice is `'static` because the prefix set is compiled in.
    pub fn vendor_prefix_for_send(&self) -> &'static str {
        self.vendor_prefix
            .lock_or_recover()
            .unwrap_or(DEFAULT_VENDOR_PREFIX)
    }

    /// Build a fully-qualified vendor extension method name for outgoing
    /// requests (e.g. `commands/execute` →
    /// `_kage.dev/commands/execute` or `_kiro.dev/commands/execute`).
    pub fn vendor_method(&self, suffix: &str) -> String {
        format!("{}{}", self.vendor_prefix_for_send(), suffix)
    }

    // --- Per-session accumulator helpers ---

    /// Append `text` to the bucket for `session_id`, capped at
    /// MAX_ACCUMULATOR_SIZE. Returns the slice that was actually appended
    /// (truncated if the cap was hit), so the notification handler can emit
    /// the same delta it accumulated.
    pub fn accumulate_chunk<'a>(&self, session_id: &str, text: &'a str) -> Option<&'a str> {
        let mut map = self.streaming_accumulators.lock_or_recover();
        let acc = map
            .entry(session_id.to_string())
            .or_insert_with(|| String::with_capacity(64 * 1024));
        if acc.len() >= MAX_ACCUMULATOR_SIZE {
            return None;
        }
        let remaining = MAX_ACCUMULATOR_SIZE - acc.len();
        if text.len() <= remaining {
            acc.push_str(text);
            Some(text)
        } else {
            let slice = &text[..remaining];
            acc.push_str(slice);
            log::warn!(
                "Streaming accumulator for session {} hit {}MB cap — truncating",
                session_id,
                MAX_ACCUMULATOR_SIZE / (1024 * 1024)
            );
            Some(slice)
        }
    }

    /// Read the bucket for `session_id` and remove it in one critical
    /// section. Used by send-and-read flows (auto_steering, invoke_subagent,
    /// inline_assist, execute_macro) which know exactly which session they
    /// targeted and don't need the accumulator to outlive their call.
    /// Returns an empty string if no bucket exists.
    pub fn take_session_accumulator(&self, session_id: &str) -> String {
        self.streaming_accumulators
            .lock_or_recover()
            .remove(session_id)
            .unwrap_or_default()
    }

    /// Reset the bucket for `session_id` to empty. Called before send when
    /// a caller wants to read the response back via take_session_accumulator
    /// without contamination from a prior incomplete response.
    pub fn reset_session_accumulator(&self, session_id: &str) {
        self.streaming_accumulators
            .lock_or_recover()
            .remove(session_id);
    }

    // --- Delegated transport accessors ---

    pub fn set_debug_mode(&self, enabled: bool) {
        *self.transport.debug_mode.lock_or_recover() = enabled;
    }

    pub fn get_process_manager(&self) -> Arc<Mutex<ProcessManager>> {
        self.transport.process_manager.clone()
    }

    pub fn set_notification_handler<F: Fn(serde_json::Value) + Send + Sync + 'static>(
        &self,
        handler: F,
    ) {
        self.transport.set_notification_handler(handler);
    }

    pub fn is_connected(&self) -> bool {
        self.transport.is_connected()
    }

    pub fn connect(&self) -> Result<()> {
        self.transport.connect()
    }

    pub fn disconnect(&self) {
        self.transport.disconnect();
    }

    /// Send a JSON-RPC request and wait for its matching response. The
    /// transport allocates the id internally — callers don't choose ids,
    /// which is what makes cross-request response delivery impossible.
    pub fn send_request(&self, method: &str, params: serde_json::Value) -> Result<AcpResponse> {
        self.transport.send_request(method, params)
    }

    /// Send a JSON-RPC notification (no id, fire-and-forget). Used for
    /// protocol messages that don't expect a response, like session/cancel.
    /// The transport handles serialization and write framing; callers
    /// supply the method name and params.
    pub fn send_notification(&self, method: &str, params: serde_json::Value) -> Result<()> {
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let line = serde_json::to_string(&notif)?;
        self.transport.write_line(&line)
    }

    /// Cancel any in-flight prompt for the given session. Both kage-cli
    /// and kiro-cli treat session/prompt as exclusive per session, so
    /// shutdown paths and the user-facing Cancel button must clear that
    /// lock before issuing a new prompt or letting the connection drop.
    /// No-op if there's no current session.
    pub fn cancel_session(&self, session_id: &str) -> Result<()> {
        info!("Sending session/cancel for session {}", session_id);
        self.send_notification(
            "session/cancel",
            serde_json::json!({ "sessionId": session_id }),
        )
    }

    /// Reply to a session/request_permission notification. The agent waits
    /// for a JSON-RPC *response* keyed to the original request id; this is
    /// not a notification — but it doesn't go through send_request because
    /// we're answering, not asking. We write the wire-formatted response
    /// directly through the transport, bypassing the pending-request map.
    ///
    /// `option_id` is one of the protocol-defined choices: "allow_once",
    /// "allow_24h", "allow_always", "reject_once". The agent rejects
    /// anything else. We pass the string through rather than enforcing
    /// here because the set may grow over the protocol's lifetime.
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

    // --- Session state ---

    pub fn get_session_id(&self) -> Option<String> {
        self.session_id.lock_or_recover().clone()
    }

    pub fn set_session_id(&self, session_id: Option<String>) {
        *self.session_id.lock_or_recover() = session_id;
    }

    // --- Connection lifecycle (used by session.rs recovery) ---

    pub(crate) fn force_disconnect(&self) {
        self.transport.force_disconnect();
        *self.initialized.lock_or_recover() = false;
    }

    pub(crate) fn restart_connection(&self) -> Result<()> {
        info!("Restarting ACP connection");
        self.force_disconnect();
        std::thread::sleep(std::time::Duration::from_millis(500));
        self.transport.connect()?;
        self.initialize()?;
        Ok(())
    }

    // Session and protocol methods are in session.rs

    /// Block the current thread until compaction is finished (with a timeout).
    /// Returns true if we waited, false if compaction wasn't active.
    pub fn wait_for_compaction(&self) -> bool {
        let (lock, cvar) = &*self.compacting;
        let compacting = lock.lock_or_recover();
        if !*compacting {
            return false;
        }
        info!("Waiting for compaction to finish before sending prompt...");
        // Wait up to 60 seconds for compaction to complete
        let timeout = std::time::Duration::from_secs(60);
        let result = match cvar.wait_timeout_while(compacting, timeout, |c| *c) {
            Ok(r) => r,
            Err(poisoned) => {
                log::warn!("Compaction condvar poisoned — recovering");
                poisoned.into_inner()
            }
        };
        if result.1.timed_out() {
            log::warn!("Compaction wait timed out after 60s — sending anyway");
        } else {
            info!("Compaction finished, proceeding with prompt");
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_method_suffix_recognises_both_prefixes() {
        assert_eq!(
            vendor_method_suffix("_kage.dev/commands/execute"),
            Some("commands/execute")
        );
        assert_eq!(
            vendor_method_suffix("_kiro.dev/commands/available"),
            Some("commands/available")
        );
        assert_eq!(vendor_method_suffix("_kage.dev/metadata"), Some("metadata"));
    }

    #[test]
    fn vendor_method_suffix_returns_none_for_plain_acp_methods() {
        // Standard ACP methods must NOT be treated as vendor extensions —
        // session/load returning Some("load") would crash the suffix-match
        // dispatch in messaging.rs into wrong branches.
        assert_eq!(vendor_method_suffix("session/load"), None);
        assert_eq!(vendor_method_suffix("session/update"), None);
        assert_eq!(vendor_method_suffix("initialize"), None);
        assert_eq!(vendor_method_suffix(""), None);
    }

    #[test]
    fn vendor_prefix_for_send_defaults_until_observed() {
        let client = AcpClient::new(AcpConnectionMode::Local {
            spawn_command: "true".to_string(),
        });
        assert_eq!(client.vendor_prefix_for_send(), "_kage.dev/");
        assert_eq!(
            client.vendor_method("commands/execute"),
            "_kage.dev/commands/execute"
        );
    }

    #[test]
    fn observe_vendor_prefix_pins_to_first_seen_namespace() {
        // Once the agent has identified itself via an inbound notification,
        // outgoing vendor calls track that namespace. Subsequent observations
        // are no-ops — mixing namespaces inside a single session would
        // indicate a misbehaving agent, not a feature worth supporting.
        let client = AcpClient::new(AcpConnectionMode::Local {
            spawn_command: "true".to_string(),
        });
        client.observe_vendor_prefix("_kiro.dev/commands/available");
        assert_eq!(client.vendor_prefix_for_send(), "_kiro.dev/");
        assert_eq!(client.vendor_method("metadata"), "_kiro.dev/metadata");

        // Pinned — a later kage.dev sighting doesn't override.
        client.observe_vendor_prefix("_kage.dev/metadata");
        assert_eq!(client.vendor_prefix_for_send(), "_kiro.dev/");
    }

    #[test]
    fn observe_vendor_prefix_ignores_non_vendor_methods() {
        let client = AcpClient::new(AcpConnectionMode::Local {
            spawn_command: "true".to_string(),
        });
        client.observe_vendor_prefix("session/update");
        // Still default since no vendor prefix was observed.
        assert_eq!(client.vendor_prefix_for_send(), "_kage.dev/");
    }
}
