//! Tauri commands an extension's `context.sync` calls
//! (docs/design/SYNC.md §12.6, §19.5).
//!
//! These are stubs with the final signatures. Until WP-S5 adds the `sync`
//! capability, they are `null` in `COMMAND_CAPABILITIES`, so no extension
//! can call them; WP-S5 maps them to `sync`, has the sandbox host
//! force-inject `extensionId`, and writes the bodies, which validate on
//! the calling thread and forward to the engine thread. Until then every
//! one answers `sync_unavailable`.

use super::*;
use crate::sync::{unavailable, ExtSyncStatus, PullResult, PushResult};

/// §12.6 `status()`.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S5.
pub async fn ext_sync_status(extension_id: String) -> Result<ExtSyncStatus, AppError> {
    Err(unavailable())
}

/// Push records of `collection` after the extension's own write, and the
/// ids it deleted. `initial`: the first push on this computer.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S5.
pub async fn ext_sync_push(
    extension_id: String,
    collection: String,
    records: Vec<serde_json::Value>,
    deleted: Vec<String>,
    initial: bool,
) -> Result<PushResult, AppError> {
    Err(unavailable())
}

/// Up to `max` changes since the extension's last ack.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S5.
pub async fn ext_sync_pull(extension_id: String, max: usize) -> Result<PullResult, AppError> {
    Err(unavailable())
}

/// The extension applied everything up to `cursor`.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S5.
pub async fn ext_sync_ack(extension_id: String, cursor: String) -> Result<(), AppError> {
    Err(unavailable())
}
