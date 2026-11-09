//! Tauri commands for folder sync (docs/design/SYNC.md §19.5). Settings →
//! Sync, the tray and the scope controls call them; every one is `null` in
//! `COMMAND_CAPABILITIES`, so no extension can.
//!
//! These are stubs with the final signatures. WP-S4 writes the bodies,
//! which go to the engine thread through `SyncHandle::request`. Until then
//! sync reports `unavailable` (Settings hides it), there are no folders to
//! suggest, and everything else answers `sync_unavailable` and touches
//! nothing.

use crate::config::Config;
use crate::error::AppError;
use crate::lock_ext::LockExt;
use crate::os::cloud_files::SuggestedFolder;
use crate::state::FeatureServices;
use crate::sync::folder::FolderInfo;
use crate::sync::prefs::SyncCategories;
use crate::sync::status::{
    ConflictDetail, ConflictSummary, DeviceView, JoinPreview, Reviews, ScopeView, SyncStatus,
};
use crate::sync::types::{Category, ScopeAction};
use crate::sync::{unavailable, ConflictAction, ConflictTab, DestructiveAction, JoinChoice};
use serde::Serialize;
use std::collections::HashMap;
use tauri::State;

/// The status line, categories, computers and counts.
#[tauri::command]
pub async fn sync_get_status() -> Result<SyncStatus, AppError> {
    Ok(SyncStatus::unavailable())
}

/// Detected sync roots for setup's quick picks.
#[tauri::command]
pub async fn sync_suggest_folders() -> Result<Vec<SuggestedFolder>, AppError> {
    Ok(Vec::new())
}

/// What the folder the user picked holds.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_inspect_folder(path: String) -> Result<FolderInfo, AppError> {
    Err(unavailable())
}

/// "Make one for me".
#[tauri::command]
pub async fn sync_generate_passphrase() -> Result<String, AppError> {
    Err(unavailable())
}

/// Create a sync folder from this computer.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_create(
    path: String,
    passphrase: String,
    device_name: String,
    categories: SyncCategories,
) -> Result<SyncStatus, AppError> {
    Err(unavailable())
}

/// Unlock an existing sync folder and preview what joining changes.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_join(
    path: String,
    passphrase: String,
    device_name: String,
    categories: SyncCategories,
) -> Result<JoinPreview, AppError> {
    Err(unavailable())
}

/// Join, with the other computers' values or this one's winning.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_confirm_join(choice: JoinChoice) -> Result<SyncStatus, AppError> {
    Err(unavailable())
}

#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_unlock(passphrase: String) -> Result<SyncStatus, AppError> {
    Err(unavailable())
}

/// Pause until an RFC 3339 time or `forever`; `None` resumes.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_set_paused(until: Option<String>) -> Result<(), AppError> {
    Err(unavailable())
}

#[tauri::command]
pub async fn sync_now() -> Result<(), AppError> {
    Err(unavailable())
}

/// Turn a category on or off on this computer.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_set_category(category: Category, on: bool) -> Result<(), AppError> {
    Err(unavailable())
}

/// Turn one extension's data sync on or off on this computer.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_set_extension_data(extension_id: String, on: bool) -> Result<(), AppError> {
    Err(unavailable())
}

/// The where-it-applies state of catalog paths (all of them for `None`).
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_get_scopes(
    paths: Option<Vec<String>>,
) -> Result<HashMap<String, ScopeView>, AppError> {
    Err(unavailable())
}

#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_set_scope(path: String, action: ScopeAction) -> Result<ScopeView, AppError> {
    Err(unavailable())
}

#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_list_conflicts(tab: ConflictTab) -> Result<Vec<ConflictSummary>, AppError> {
    Err(unavailable())
}

#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_conflict_detail(id: String) -> Result<ConflictDetail, AppError> {
    Err(unavailable())
}

#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_resolve_conflict(id: String, action: ConflictAction) -> Result<(), AppError> {
    Err(unavailable())
}

/// Dismiss every open entry, or those of one group.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_dismiss_all(group: Option<String>) -> Result<(), AppError> {
    Err(unavailable())
}

/// Held runnables, install suggestions, removals, bulk changes and
/// extensions that need approval here.
#[tauri::command]
pub async fn sync_list_reviews() -> Result<Reviews, AppError> {
    Err(unavailable())
}

/// Let a synced item that starts programs run on this computer.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_approve_runnable(key: String) -> Result<(), AppError> {
    Err(unavailable())
}

#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_keep_off(key: String) -> Result<(), AppError> {
    Err(unavailable())
}

#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_dismiss_suggestion(kind: String, id: String) -> Result<(), AppError> {
    Err(unavailable())
}

/// Accept or refuse a held bulk change in namespace `ns`.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_answer_mass_change(ns: String, accept: bool) -> Result<(), AppError> {
    Err(unavailable())
}

#[tauri::command]
pub async fn sync_list_devices() -> Result<Vec<DeviceView>, AppError> {
    Err(unavailable())
}

/// Rename this computer.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_rename_device(name: String) -> Result<(), AppError> {
    Err(unavailable())
}

/// A one-use token, valid for 60 s, for one destructive command.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_prepare_destructive(action: DestructiveAction) -> Result<String, AppError> {
    Err(unavailable())
}

/// Remove another computer. `lost`: also make a new key and passphrase.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_remove_device(
    device_id: String,
    lost: bool,
    new_passphrase: Option<String>,
    confirm: String,
) -> Result<(), AppError> {
    Err(unavailable())
}

#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_change_passphrase(new_passphrase: String) -> Result<(), AppError> {
    Err(unavailable())
}

/// Turn off sync on this computer.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_leave(confirm: String) -> Result<(), AppError> {
    Err(unavailable())
}

#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_start_over(passphrase: String, confirm: String) -> Result<SyncStatus, AppError> {
    Err(unavailable())
}

#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_undo_join(confirm: String) -> Result<(), AppError> {
    Err(unavailable())
}

/// Remove an extension's data from the sync folder.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-S4.
pub async fn sync_purge_extension(extension_id: String, confirm: String) -> Result<(), AppError> {
    Err(unavailable())
}

/// Look for the missing sync folder in the detected cloud roots.
#[tauri::command]
pub async fn sync_find_folder() -> Result<Option<String>, AppError> {
    Err(unavailable())
}

/// `get_config_snapshot`: the config and the revision a later
/// `save_config` names as its base.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigSnapshot {
    pub revision: u64,
    pub config: Config,
}

/// The current config and its revision (§9.5). Until WP-S4 keeps a
/// revision ring, the revision is always 0, which `save_config` doesn't
/// read yet.
#[tauri::command]
pub async fn get_config_snapshot(
    features: State<'_, FeatureServices>,
) -> Result<ConfigSnapshot, AppError> {
    let config = features.config.lock_or_recover().clone();
    Ok(ConfigSnapshot {
        revision: 0,
        config,
    })
}
