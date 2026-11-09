//! Tauri commands for reading a file the user picks
//! (docs/design/EXTENSION_FILE_TEXT.md §3.5). Extensions holding
//! `file_text` call them through the sandbox host, which force-injects
//! `extensionId` (and `displayName` for `pick_file_text`). The work
//! happens in `crate::file_text`.

use super::*;
use crate::file_text::{self, FileText, FileTextSupport, PickFileTextArgs};

/// Ask the user to pick a file, then return its text. `null` when they
/// picked nothing or the request was cancelled while the dialog was open.
#[tauri::command]
pub async fn pick_file_text<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::WebviewWindow<R>,
    features: State<'_, FeatureServices>,
    extension_id: String,
    display_name: String,
    args: Option<PickFileTextArgs>,
) -> Result<Option<FileText>, AppError> {
    let config = features.config.clone();
    file_text::pick(app, window, config, extension_id, display_name, args).await
}

/// Cancel the extension's `pick_file_text` request, if it is the one
/// running. Returns whether something was cancelled.
#[tauri::command]
pub async fn cancel_file_text(extension_id: String) -> Result<bool, AppError> {
    Ok(file_text::cancel(&extension_id))
}

/// What this Kage can read: formats with their extensions, and the limits.
#[tauri::command]
pub fn file_text_support() -> Result<FileTextSupport, AppError> {
    Ok(file_text::support())
}
