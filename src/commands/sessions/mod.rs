//! Session commands, split by theme:
//!   - `crud` — list/load/delete sessions, the directory watcher, ACP
//!     session switch/create, and the per-window session pin commands.
//!   - `titles` — the on-disk title cache, JSONL title extraction, window
//!     title updates, manual rename, and the background AI summariser.
//!
//! Submodules pull this module's shared imports and dir-resolution helpers
//! via `use super::*`, and the flat re-exports below preserve the original
//! `commands::sessions::*` surface so callers (and `tauri::generate_handler!`)
//! are unaffected. The shared session-record types and the sessions-directory
//! resolvers live here because both submodules need them.

use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use crate::state::{AcpHandles, FeatureServices};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tauri::{Manager, State};

mod crud;
mod titles;

// Flat re-export preserves the previous `commands::sessions::*` surface.
pub use crud::*;
pub use titles::*;

/// Summary of a session for the sidebar list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A single message in a session conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub kind: String, // "Prompt", "AssistantMessage", "ToolResults"
    pub message_id: String,
    pub content: Vec<MessageContent>,
}

/// Content item within a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageContent {
    pub kind: String, // "text", "toolUse", "toolResult", "json"
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Full session data returned when loading a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub session_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<SessionMessage>,
    /// Map of message_id → ISO timestamp (extracted from turn metadata)
    #[serde(default)]
    pub message_timestamps: HashMap<String, String>,
    /// Map of message_id → turn duration in seconds
    #[serde(default)]
    pub message_durations: HashMap<String, f64>,
}

/// Resolve the sessions directory from config.
/// Priority: 1) explicit sessions_directory, 2) agent preset, 3) probe common paths
fn get_sessions_dir_from_config(config: &crate::config::Config) -> Result<PathBuf, String> {
    crate::agent_presets::resolve_sessions_dir(config)
        .ok_or_else(|| "Failed to get home directory".to_string())
}

/// Lock the config, resolve the sessions dir, drop the lock. The previous
/// pattern was `let config = features.config.lock_or_recover().clone();`
/// followed by `get_sessions_dir_from_config(&config)`, which deep-cloned
/// every nested HashMap (extension grants, extension states, tool
/// permissions list, …) just to read the active connection's directory.
/// Most session commands run on a hot path (load, list, switch, delete)
/// where that overhead adds up.
fn resolve_sessions_dir_locked(
    config: &std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
) -> Result<PathBuf, String> {
    let guard = config.lock_or_recover();
    get_sessions_dir_from_config(&guard)
}

/// Fallback for callers without config access — probes common paths
fn get_sessions_dir() -> Result<PathBuf, String> {
    crate::agent_presets::default_sessions_dir()
        .ok_or_else(|| "Failed to get home directory".to_string())
}
