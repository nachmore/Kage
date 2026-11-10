//! Tauri commands for folder sync (docs/design/SYNC.md §19.5). Settings →
//! Sync, the tray and the scope controls call them; every one is `null` in
//! `COMMAND_CAPABILITIES`, so no extension can.
//!
//! Each command asks the `kage-sync` engine thread through
//! `SyncHandle::request`, from the blocking pool, so no async worker waits
//! and nothing touches the folder on a Tokio worker. Requests wait 10 s;
//! setup, join, unlock, passphrase changes, start over, Sync now and the
//! folder search wait 60 s.
//!
//! Until the engine thread runs (a build without sync, the mock app), only
//! `sync_get_status` (the `unavailable` status) and `sync_suggest_folders`
//! (no folders) answer; every other command returns `sync_unavailable` and
//! touches nothing. Destructive commands need a one-use token from
//! `sync_prepare_destructive` first and return `needs_confirmation`
//! without one.

use crate::config::Config;
use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use crate::os::cloud_files::SuggestedFolder;
use crate::state::FeatureServices;
use crate::sync::folder::FolderInfo;
use crate::sync::prefs::SyncCategories;
use crate::sync::status::{
    ConflictDetail, ConflictSummary, DeviceView, JoinPreview, Reviews, ScopeView, SyncStatus,
};
use crate::sync::types::{Category, ScopeAction};
use crate::sync::{
    needs_confirmation, unavailable, ConflictAction, ConflictTab, CreateReq, DestructiveAction,
    EngineReply, EngineRequest, JoinChoice, JoinReq,
};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tauri::State;

/// How long a request waits for the engine thread.
const TIMEOUT: Duration = Duration::from_secs(10);
/// For requests that run Argon2, read the folder or tick: setup, join,
/// unlock, passphrase changes, start over, Sync now, finding the folder.
const LONG_TIMEOUT: Duration = Duration::from_secs(60);

/// The engine answered with something else than this command expects.
fn unexpected() -> AppError {
    log::warn!("sync: the engine answered a request with an unexpected reply");
    AppError::keyed(ErrorKind::Internal, "errors.sync.failed", &[])
}

/// Arguments the engine never sees: a folder path that isn't absolute, a
/// pause time that isn't one.
fn bad_request() -> AppError {
    AppError::keyed(ErrorKind::InvalidInput, "errors.sync.bad_request", &[])
}

/// Ask the engine thread, from the blocking pool.
async fn ask(
    features: &FeatureServices,
    req: EngineRequest,
    timeout: Duration,
) -> Result<EngineReply, AppError> {
    let handle = features.sync.clone();
    tauri::async_runtime::spawn_blocking(move || handle.request(req, timeout))
        .await
        .map_err(|e| {
            log::warn!("sync: a request didn't finish: {e}");
            AppError::keyed(ErrorKind::Internal, "errors.sync.failed", &[])
        })?
}

/// Ask, and expect `Done`.
async fn ask_done(
    features: &FeatureServices,
    req: EngineRequest,
    timeout: Duration,
) -> Result<(), AppError> {
    match ask(features, req, timeout).await? {
        EngineReply::Done => Ok(()),
        _ => Err(unexpected()),
    }
}

/// Use up a token from `sync_prepare_destructive`.
fn confirmed(token: &str, action: DestructiveAction) -> Result<(), AppError> {
    if crate::sync_app::confirmations().take(token, action) {
        Ok(())
    } else {
        Err(needs_confirmation())
    }
}

/// A folder the user picked: an absolute path.
fn folder_path(path: &str) -> Result<PathBuf, AppError> {
    let p = PathBuf::from(path);
    if path.trim().is_empty() || !p.is_absolute() {
        return Err(bad_request());
    }
    Ok(p)
}

/// The status line, categories, computers and counts.
#[tauri::command]
pub async fn sync_get_status(features: State<'_, FeatureServices>) -> Result<SyncStatus, AppError> {
    if !features.sync.started() {
        return Ok(SyncStatus::unavailable());
    }
    match ask(&features, EngineRequest::Status, TIMEOUT).await {
        Ok(EngineReply::Status(s)) => Ok(s),
        Ok(_) => Err(unexpected()),
        // The engine didn't open: Settings hides sync.
        Err(e) if matches!(e.kind, ErrorKind::SyncUnavailable) => Ok(SyncStatus::unavailable()),
        // Busy in a long tick: the newest status it reported.
        Err(e) if matches!(e.kind, ErrorKind::Timeout) => crate::sync_app::last_status().ok_or(e),
        Err(e) => Err(e),
    }
}

/// Detected sync roots for setup's quick picks.
#[tauri::command]
pub async fn sync_suggest_folders(
    features: State<'_, FeatureServices>,
) -> Result<Vec<SuggestedFolder>, AppError> {
    if !features.sync.started() {
        return Ok(Vec::new());
    }
    match ask(&features, EngineRequest::SuggestFolders, TIMEOUT).await? {
        EngineReply::Folders(f) => Ok(f),
        _ => Err(unexpected()),
    }
}

/// What the folder the user picked holds.
#[tauri::command]
pub async fn sync_inspect_folder(
    path: String,
    features: State<'_, FeatureServices>,
) -> Result<FolderInfo, AppError> {
    let path = folder_path(&path)?;
    match ask(&features, EngineRequest::InspectFolder(path), TIMEOUT).await? {
        EngineReply::Folder(f) => Ok(f),
        _ => Err(unexpected()),
    }
}

/// "Make one for me".
#[tauri::command]
pub async fn sync_generate_passphrase(
    features: State<'_, FeatureServices>,
) -> Result<String, AppError> {
    match ask(&features, EngineRequest::GeneratePassphrase, TIMEOUT).await? {
        EngineReply::Passphrase(p) => Ok(p),
        _ => Err(unexpected()),
    }
}

/// Create a sync folder from this computer.
#[tauri::command]
pub async fn sync_create(
    path: String,
    passphrase: String,
    device_name: String,
    categories: SyncCategories,
    features: State<'_, FeatureServices>,
) -> Result<SyncStatus, AppError> {
    let req = CreateReq {
        path: folder_path(&path)?,
        passphrase,
        device_name,
        categories,
    };
    match ask(&features, EngineRequest::Create(req), LONG_TIMEOUT).await? {
        EngineReply::Status(s) => Ok(s),
        _ => Err(unexpected()),
    }
}

/// Unlock an existing sync folder and preview what joining changes.
#[tauri::command]
pub async fn sync_join(
    path: String,
    passphrase: String,
    device_name: String,
    categories: SyncCategories,
    features: State<'_, FeatureServices>,
) -> Result<JoinPreview, AppError> {
    let req = JoinReq {
        path: folder_path(&path)?,
        passphrase,
        device_name,
        categories,
    };
    match ask(&features, EngineRequest::Join(req), LONG_TIMEOUT).await? {
        EngineReply::JoinPreview(p) => Ok(p),
        _ => Err(unexpected()),
    }
}

/// Join, with the other computers' values or this one's winning.
#[tauri::command]
pub async fn sync_confirm_join(
    choice: JoinChoice,
    features: State<'_, FeatureServices>,
) -> Result<SyncStatus, AppError> {
    match ask(&features, EngineRequest::ConfirmJoin(choice), LONG_TIMEOUT).await? {
        EngineReply::Status(s) => Ok(s),
        _ => Err(unexpected()),
    }
}

#[tauri::command]
pub async fn sync_unlock(
    passphrase: String,
    features: State<'_, FeatureServices>,
) -> Result<SyncStatus, AppError> {
    match ask(
        &features,
        EngineRequest::Unlock { passphrase },
        LONG_TIMEOUT,
    )
    .await?
    {
        EngineReply::Status(s) => Ok(s),
        _ => Err(unexpected()),
    }
}

/// Pause until an RFC 3339 time or `forever`; `None` resumes.
#[tauri::command]
pub async fn sync_set_paused(
    until: Option<String>,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    if let Some(t) = until.as_deref() {
        if t != crate::sync::prefs::FOREVER && chrono::DateTime::parse_from_rfc3339(t).is_err() {
            return Err(bad_request());
        }
    }
    ask_done(&features, EngineRequest::SetPaused { until }, TIMEOUT).await
}

/// An immediate tick.
#[tauri::command]
pub async fn sync_now(features: State<'_, FeatureServices>) -> Result<(), AppError> {
    ask_done(&features, EngineRequest::SyncNow, LONG_TIMEOUT).await
}

/// Turn a category on or off on this computer.
#[tauri::command]
pub async fn sync_set_category(
    category: Category,
    on: bool,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    ask_done(
        &features,
        EngineRequest::SetCategory { category, on },
        TIMEOUT,
    )
    .await
}

/// Turn one extension's data sync on or off on this computer.
#[tauri::command]
pub async fn sync_set_extension_data(
    extension_id: String,
    on: bool,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    crate::extensions::validate_extension_id(&extension_id).map_err(|_| bad_request())?;
    let req = EngineRequest::SetExtensionData {
        ext: extension_id,
        on,
    };
    ask_done(&features, req, TIMEOUT).await
}

/// The where-it-applies state of catalog paths (all of them for `None`).
#[tauri::command]
pub async fn sync_get_scopes(
    paths: Option<Vec<String>>,
    features: State<'_, FeatureServices>,
) -> Result<HashMap<String, ScopeView>, AppError> {
    match ask(&features, EngineRequest::GetScopes(paths), TIMEOUT).await? {
        EngineReply::Scopes(s) => Ok(s),
        _ => Err(unexpected()),
    }
}

#[tauri::command]
pub async fn sync_set_scope(
    path: String,
    action: ScopeAction,
    features: State<'_, FeatureServices>,
) -> Result<ScopeView, AppError> {
    match ask(&features, EngineRequest::SetScope { path, action }, TIMEOUT).await? {
        EngineReply::Scope(s) => Ok(s),
        _ => Err(unexpected()),
    }
}

#[tauri::command]
pub async fn sync_list_conflicts(
    tab: ConflictTab,
    features: State<'_, FeatureServices>,
) -> Result<Vec<ConflictSummary>, AppError> {
    match ask(&features, EngineRequest::ListConflicts(tab), TIMEOUT).await? {
        EngineReply::Conflicts(c) => Ok(c),
        _ => Err(unexpected()),
    }
}

#[tauri::command]
pub async fn sync_conflict_detail(
    id: String,
    features: State<'_, FeatureServices>,
) -> Result<ConflictDetail, AppError> {
    match ask(&features, EngineRequest::ConflictDetail(id), TIMEOUT).await? {
        EngineReply::ConflictDetail(d) => Ok(d),
        _ => Err(unexpected()),
    }
}

#[tauri::command]
pub async fn sync_resolve_conflict(
    id: String,
    action: ConflictAction,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    ask_done(
        &features,
        EngineRequest::ResolveConflict { id, action },
        TIMEOUT,
    )
    .await
}

/// Dismiss every open entry, or those of one group.
#[tauri::command]
pub async fn sync_dismiss_all<R: tauri::Runtime>(
    group: Option<String>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    crate::sync_app::begin_dismiss_all();
    let out = ask_done(&features, EngineRequest::DismissAll(group), TIMEOUT).await;
    crate::sync_app::end_dismiss_all(&app);
    out
}

/// Held runnables, install suggestions, removals, bulk changes and
/// extensions that need approval here.
#[tauri::command]
pub async fn sync_list_reviews(features: State<'_, FeatureServices>) -> Result<Reviews, AppError> {
    match ask(&features, EngineRequest::ListReviews, TIMEOUT).await? {
        EngineReply::Reviews(r) => Ok(r),
        _ => Err(unexpected()),
    }
}

/// Let a synced item that starts programs run on this computer.
#[tauri::command]
pub async fn sync_approve_runnable(
    key: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    ask_done(&features, EngineRequest::ApproveRunnable(key), TIMEOUT).await
}

#[tauri::command]
pub async fn sync_keep_off(
    key: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    ask_done(&features, EngineRequest::KeepOff(key), TIMEOUT).await
}

#[tauri::command]
pub async fn sync_dismiss_suggestion(
    kind: String,
    id: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    ask_done(
        &features,
        EngineRequest::DismissSuggestion { kind, id },
        TIMEOUT,
    )
    .await
}

/// Accept or refuse a held bulk change in namespace `ns`.
#[tauri::command]
pub async fn sync_answer_mass_change(
    ns: String,
    accept: bool,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    ask_done(
        &features,
        EngineRequest::AnswerMassChange { ns, accept },
        TIMEOUT,
    )
    .await
}

#[tauri::command]
pub async fn sync_list_devices(
    features: State<'_, FeatureServices>,
) -> Result<Vec<DeviceView>, AppError> {
    match ask(&features, EngineRequest::ListDevices, TIMEOUT).await? {
        EngineReply::Devices(d) => Ok(d),
        _ => Err(unexpected()),
    }
}

/// Rename this computer.
#[tauri::command]
pub async fn sync_rename_device(
    name: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    ask_done(&features, EngineRequest::RenameDevice(name), TIMEOUT).await
}

/// A one-use token, valid for 60 s, for one destructive command.
#[tauri::command]
pub async fn sync_prepare_destructive(
    action: DestructiveAction,
    features: State<'_, FeatureServices>,
) -> Result<String, AppError> {
    if !features.sync.started() {
        return Err(unavailable());
    }
    Ok(crate::sync_app::confirmations().issue(action))
}

/// Remove another computer. `lost`: also make a new key and passphrase.
#[tauri::command]
pub async fn sync_remove_device(
    device_id: String,
    lost: bool,
    new_passphrase: Option<String>,
    confirm: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    confirmed(&confirm, DestructiveAction::RemoveDevice)?;
    let req = EngineRequest::RemoveDevice {
        id: device_id,
        lost,
        new_passphrase,
    };
    ask_done(&features, req, LONG_TIMEOUT).await
}

#[tauri::command]
pub async fn sync_change_passphrase(
    new_passphrase: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    ask_done(
        &features,
        EngineRequest::ChangePassphrase(new_passphrase),
        LONG_TIMEOUT,
    )
    .await
}

/// Turn off sync on this computer.
#[tauri::command]
pub async fn sync_leave(
    confirm: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    confirmed(&confirm, DestructiveAction::Leave)?;
    ask_done(&features, EngineRequest::Leave, LONG_TIMEOUT).await
}

#[tauri::command]
pub async fn sync_start_over(
    passphrase: String,
    confirm: String,
    features: State<'_, FeatureServices>,
) -> Result<SyncStatus, AppError> {
    confirmed(&confirm, DestructiveAction::StartOver)?;
    match ask(
        &features,
        EngineRequest::StartOver { passphrase },
        LONG_TIMEOUT,
    )
    .await?
    {
        EngineReply::Status(s) => Ok(s),
        _ => Err(unexpected()),
    }
}

#[tauri::command]
pub async fn sync_undo_join(
    confirm: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    confirmed(&confirm, DestructiveAction::UndoJoin)?;
    ask_done(&features, EngineRequest::UndoJoin, LONG_TIMEOUT).await
}

/// Remove an extension's data from the sync folder.
#[tauri::command]
pub async fn sync_purge_extension(
    extension_id: String,
    confirm: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    confirmed(&confirm, DestructiveAction::PurgeExtension)?;
    crate::extensions::validate_extension_id(&extension_id).map_err(|_| bad_request())?;
    ask_done(
        &features,
        EngineRequest::PurgeExtension(extension_id),
        TIMEOUT,
    )
    .await
}

/// Look for the missing sync folder in the detected cloud roots.
#[tauri::command]
pub async fn sync_find_folder(
    features: State<'_, FeatureServices>,
) -> Result<Option<String>, AppError> {
    match ask(&features, EngineRequest::FindFolder, LONG_TIMEOUT).await? {
        EngineReply::FoundFolder(p) => Ok(p.map(|p| p.to_string_lossy().into_owned())),
        _ => Err(unexpected()),
    }
}

/// Use the sync folder at `path` again: one `sync_find_folder` found, or
/// one the user picked after it moved or access was refused. It must be
/// the same sync folder (`errors.sync.not_this_folder`).
#[tauri::command]
pub async fn sync_apply_folder(
    path: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    let path = folder_path(&path)?;
    ask_done(&features, EngineRequest::ApplyFolder(path), LONG_TIMEOUT).await
}

/// `get_config_snapshot`: the config and the revision a later
/// `save_config` names as its base.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigSnapshot {
    pub revision: u64,
    pub config: Config,
}

/// The current config and its revision (§9.5).
#[tauri::command]
pub async fn get_config_snapshot(
    features: State<'_, FeatureServices>,
) -> Result<ConfigSnapshot, AppError> {
    let config = features.config.lock_or_recover();
    Ok(ConfigSnapshot {
        revision: features.config_revisions.observe(&config),
        config: config.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_must_be_an_absolute_path() {
        assert!(folder_path("").is_err());
        assert!(folder_path("   ").is_err());
        assert!(folder_path("Dropbox").is_err());
        let abs = std::env::temp_dir();
        assert_eq!(folder_path(abs.to_str().unwrap()).unwrap(), abs);
    }

    #[test]
    fn a_destructive_command_needs_its_own_token() {
        let err = confirmed("nope", DestructiveAction::Leave).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::NeedsConfirmation));
        let token = crate::sync_app::confirmations().issue(DestructiveAction::Leave);
        assert!(confirmed(&token, DestructiveAction::UndoJoin).is_err());
        assert!(confirmed(&token, DestructiveAction::Leave).is_ok());
        assert!(
            confirmed(&token, DestructiveAction::Leave).is_err(),
            "one use"
        );
    }

    #[test]
    fn errors_have_english_strings() {
        crate::i18n::init(Some("en"));
        for e in [bad_request(), unexpected()] {
            assert_ne!(crate::i18n::translate_in("en", &e.key, &[]), e.key);
        }
    }
}
