//! ACP messaging commands, split by theme:
//!   - `notifications` — the ACP notification handler, chunk-flush thread,
//!     permission-request routing, and standard-ACP command discovery.
//!   - `streaming` — send/cancel a prompt, connection check/reconnect, and
//!     opening a chat with a message.
//!   - `permissions` — permission responses and extension-tool plumbing.
//!   - `slash` — slash-command discovery/execute, models, and steering.
//!   - `automation` — automation plans, inline-assist, script/macro execution.
//!
//! Submodules pull this module's shared imports via `use super::*`, and the
//! flat re-exports below preserve the original `commands::messaging::*`
//! surface so callers (and `tauri::generate_handler!`) are unaffected.

use crate::error::{AppError, ErrorKind};
use crate::events;
use crate::lock_ext::LockExt;
use crate::state::{AcpHandles, FeatureServices, UiState};
use crate::window_labels;
use log::{error, info, warn};
use tauri::{async_runtime, Emitter, Manager, State, WebviewWindow};

mod automation;
mod notifications;
mod permissions;
mod slash;
mod streaming;

// Flat re-export preserves the previous `commands::messaging::*` surface.
pub use automation::*;
pub use notifications::*;
pub use permissions::*;
pub use slash::*;
pub use streaming::*;

/// Resolve a caller-supplied session id into a usable *real* session,
/// creating a fresh one if the caller passed `None`/empty.
///
/// Frontend callers fetch their window's session with
/// `get_window_session(label).catch(() => null)`, which yields `null`
/// whenever that window hasn't pinned a session yet (e.g. the floating
/// or main window was never opened this run). Commands used to take
/// `session_id: String`, so that `null` crashed at Tauri's arg
/// deserialization with "invalid type: null, expected a string" before
/// the handler even ran. Taking `Option<String>` and routing through
/// here turns that into a graceful "make a real session and use it".
///
/// This is deliberately NOT an ephemeral session: these paths continue
/// (or start) a conversation the user will see and keep — inline-assist
/// on the floating session, slash commands, opening a chat. The session
/// gets built-in steering primed, exactly like a normal new chat. Call
/// from a blocking context (the ACP calls are synchronous).
fn resolve_or_create_session(
    client: &std::sync::Arc<crate::acp_client::AcpClient>,
    session_id: Option<String>,
) -> Result<String, AppError> {
    if let Some(id) = session_id {
        if !id.trim().is_empty() {
            return Ok(id);
        }
    }
    let (id, _) = client.create_session(None).map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.session.create_failed",
            &[("reason", &e.to_string())],
        )
    })?;
    client.send_builtin_steering(&id);
    Ok(id)
}
