//! Provider-specific *chrome* Tauri commands for the Kage Desktop
//! session viewer — list workspaces, open the containing folder, delete
//! a session file. The hot path (list / load / check_updated) lives
//! behind the generic `agent_sessions` surface and dispatches through
//! `AgentSessionProvider`. These typed commands are kept because
//! they're well-defined per provider and only make sense for the
//! kage-desktop on-disk layout.

use crate::agent_sessions::kage_desktop::{list_workspaces, KageDesktopWorkspace};
use crate::error::{AppError, ErrorKind};
use log::info;

#[tauri::command]
pub async fn kage_desktop_workspaces() -> Result<Vec<KageDesktopWorkspace>, AppError> {
    list_workspaces()
}

#[tauri::command]
pub async fn kage_desktop_open_folder(file_path: String) -> Result<(), AppError> {
    let path = std::path::Path::new(&file_path);
    let dir = path.parent().ok_or_else(|| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.fs.path_invalid",
            &[("reason", "path has no parent directory")],
        )
    })?;
    crate::os::shell::open_path(&dir.to_string_lossy()).map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.fs.read_failed",
            &[("reason", &e.to_string())],
        )
    })
}

/// Delete a `kage.kageagent`-managed session JSON file. The provider's
/// per-file caches don't need explicit eviction — their next-scan
/// `retain(|k| seen_keys.contains(k))` already drops entries for files
/// no longer on disk.
#[tauri::command]
pub async fn kage_desktop_delete_session(file_path: String) -> Result<(), AppError> {
    let path = std::path::Path::new(&file_path);
    if !path.exists() {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.session.not_found",
            &[],
        ));
    }
    // Safety: only delete .json files in the kage.kageagent directory
    let path_str = path.to_string_lossy();
    if !path_str.contains("kage.kageagent") || !path_str.ends_with(".json") {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.fs.path_invalid",
            &[(
                "reason",
                "session deletion is restricted to .json files inside kage.kageagent",
            )],
        ));
    }
    std::fs::remove_file(path).map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.session.delete_failed",
            &[("reason", &e.to_string())],
        )
    })?;
    info!("Deleted Kage Desktop session: {}", file_path);
    Ok(())
}
