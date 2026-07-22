//! Tauri command wrappers around `kage_core::folder_tools`.
//!
//! The scan/plan logic itself is pure and lives in kage-core (shared with
//! the MCP sidecar's `scan_folder` / `execute_folder_plan` tools); this
//! module owns only the app-side concerns: async spawn_blocking, the
//! native folder-picker dialog, and AppError conversion.

use crate::error::AppError;
use log::info;
use std::collections::HashMap;
use std::path::PathBuf;
use tauri_plugin_dialog::DialogExt;

// Re-exported so existing `kage::commands::folder_tools::...` paths
// (integration tests, the MCP registration layer) keep working.
pub use kage_core::folder_tools::{
    execute_plan, scan_directory, FolderOperation, PlanExecutionResult, ScanResult,
};

const MAX_DEPTH: usize = 10;

/// Return a map of well-known folder names to their absolute paths on this system.
#[tauri::command]
pub fn get_common_folders() -> HashMap<String, String> {
    kage_core::folder_tools::get_common_folders()
}

/// Open a native folder picker dialog. Returns the selected path or null.
#[tauri::command]
pub async fn pick_folder<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Option<String>, AppError> {
    info!("Opening native folder picker dialog");

    // Use blocking_pick_folder on a blocking thread to avoid blocking the async runtime
    let result = tauri::async_runtime::spawn_blocking(move || {
        let file_resp = app
            .dialog()
            .file()
            .set_title("Select Folder to Organize")
            .blocking_pick_folder();
        file_resp.map(|p| p.to_string())
    })
    .await
    .map_err(|e| format!("Dialog task failed: {}", e))?;

    if let Some(ref path) = result {
        info!("Folder selected: {}", path);
    } else {
        info!("Folder picker cancelled");
    }
    Ok(result)
}

/// Scan a folder recursively and return metadata for all entries.
#[tauri::command]
pub async fn scan_folder(
    path: String,
    max_depth: Option<usize>,
    compute_hashes: Option<bool>,
) -> Result<ScanResult, AppError> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(format!("'{}' is not a directory", path).into());
    }

    let depth_limit = max_depth.unwrap_or(MAX_DEPTH);
    let do_hashes = compute_hashes.unwrap_or(true);

    info!(
        "Scanning folder: {} (depth={}, hashes={})",
        path, depth_limit, do_hashes
    );

    // Run the scan on a blocking thread since it's I/O heavy
    let result =
        tauri::async_runtime::spawn_blocking(move || scan_directory(&root, depth_limit, do_hashes))
            .await
            .map_err(|e| format!("Scan task failed: {}", e))?;

    info!(
        "Scan complete: {} files, {} dirs, {} bytes, {} duplicate groups",
        result.total_files,
        result.total_dirs,
        result.total_size,
        result.duplicates.len()
    );
    Ok(result)
}

/// Execute a folder organization plan (list of move/rename/delete operations).
#[tauri::command]
pub async fn execute_folder_plan(
    root: String,
    operations: Vec<FolderOperation>,
) -> Result<PlanExecutionResult, AppError> {
    let root_path = PathBuf::from(&root);
    if !root_path.is_dir() {
        return Err(format!("'{}' is not a directory", root).into());
    }

    info!(
        "Executing folder plan: {} operations in {}",
        operations.len(),
        root
    );

    let result =
        tauri::async_runtime::spawn_blocking(move || execute_plan(&root_path, &operations))
            .await
            .map_err(|e| format!("Plan execution task failed: {}", e))?;

    info!(
        "Plan execution: {} completed, {} failed",
        result.operations_completed, result.operations_failed
    );
    Ok(result)
}
