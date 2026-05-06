//! ACP (Agent Communication Protocol) client.
//!
//! Split into:
//! - `types`: Protocol types (AcpRequest, AcpResponse, AcpError, etc.)
//! - `transport`: Connection management, pipe/TCP I/O, background reader thread
//! - This module: `AcpClient` facade composing the above with session/protocol logic

pub mod types;
pub mod transport;
mod session;

// Re-export public types so callers can still use `crate::acp_client::AcpRequest` etc.
#[allow(unused_imports)]
pub use types::{AcpRequest, AcpResponse, AcpNotification, AcpError, AcpConnectionMode, NotificationHandler, format_acp_error};
pub use transport::AcpTransport;

use anyhow::Result;
use log::info;
use std::sync::{Arc, Mutex};

use crate::lock_ext::LockExt;
use crate::process_manager::ProcessManager;

/// Maximum size for the streaming accumulator (10 MB).
/// Prevents OOM if the server sends an unbounded response.
pub const MAX_ACCUMULATOR_SIZE: usize = 10 * 1024 * 1024;

pub struct AcpClient {
    transport: AcpTransport,
    session_id: Arc<Mutex<Option<String>>>,
    initialized: Arc<Mutex<bool>>,
    /// Accumulated streaming text (reset per message)
    pub streaming_accumulator: Arc<Mutex<String>>,
    /// True while the server is compacting context — outgoing prompts should wait
    pub compacting: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl AcpClient {
    pub fn new(mode: AcpConnectionMode) -> Self {
        Self {
            transport: AcpTransport::new(mode),
            session_id: Arc::new(Mutex::new(None)),
            initialized: Arc::new(Mutex::new(false)),
            streaming_accumulator: Arc::new(Mutex::new(String::with_capacity(64 * 1024))),
            compacting: Arc::new((Mutex::new(false), std::sync::Condvar::new())),
        }
    }

    // --- Delegated transport accessors ---

    pub fn set_debug_mode(&self, enabled: bool) {
        *self.transport.debug_mode.lock_or_recover() = enabled;
    }

    pub fn get_process_manager(&self) -> Arc<Mutex<ProcessManager>> {
        self.transport.process_manager.clone()
    }

    pub fn get_pipe_stdin(&self) -> Arc<Mutex<Option<Arc<Mutex<std::process::ChildStdin>>>>> {
        self.transport.pipe_stdin.clone()
    }

    pub fn get_tcp_writer(&self) -> Arc<Mutex<Option<Arc<Mutex<std::net::TcpStream>>>>> {
        self.transport.tcp_writer.clone()
    }

    pub fn set_notification_handler<F: Fn(serde_json::Value) + Send + Sync + 'static>(&self, handler: F) {
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

    /// Cancel any in-flight prompt for the given session. The agent
    /// (kage-cli) treats session/prompt as exclusive per session, so
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
