//! Tauri commands for extension view windows
//! (docs/design/EXTENSION_VIEWS.md §16.1). First-party host code calls
//! them; every one is `null` in `COMMAND_CAPABILITIES`, so no extension
//! can.
//!
//! These are stubs with the final signatures. WP-V2 writes the bodies,
//! which call into `crate::ext_views`. Until then nothing opens: the
//! commands that act on a view window answer `not_a_view_window`, and the
//! queries answer "no views".

use super::*;
use crate::ext_views::{ExtViewBoot, ExtViewFocusState, ExtViewInfo, ExtViewOpened};
use tauri::{AppHandle, Runtime, WebviewWindow};

fn not_a_view_window() -> AppError {
    AppError::keyed(
        ErrorKind::NotPermitted,
        "errors.ext_view.not_a_view_window",
        &[],
    )
}

/// Open (or show again) an extension's view window from the calling
/// window. Called by the launcher, chat, settings and view pages after
/// their own checks; Rust repeats them against the installed manifest and
/// grant.
#[tauri::command]
#[allow(clippy::too_many_arguments, unused_variables)] // Stub until WP-V2.
pub async fn ext_view_open<R: Runtime>(
    extension_id: String,
    view_id: String,
    params: Option<serde_json::Value>,
    source: Option<String>,
    window: WebviewWindow<R>,
    app: AppHandle<R>,
    ui: State<'_, UiState>,
    features: State<'_, FeatureServices>,
) -> Result<ExtViewOpened, AppError> {
    info!(
        "ext_view_open({}/{}) from {}: extension views aren't available yet",
        extension_id,
        view_id,
        window.label()
    );
    Err(AppError::keyed(
        ErrorKind::NotPermitted,
        "errors.ext_view.opener_refused",
        &[],
    ))
}

/// The boot record of the calling view window, derived from its label.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-V2.
pub async fn ext_view_boot<R: Runtime>(
    window: WebviewWindow<R>,
    ui: State<'_, UiState>,
    features: State<'_, FeatureServices>,
) -> Result<ExtViewBoot, AppError> {
    Err(not_a_view_window())
}

/// Put `text` in the launcher's input and show the launcher. Never sends.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-V2.
pub async fn ext_view_compose<R: Runtime>(
    text: String,
    window: WebviewWindow<R>,
    app: AppHandle<R>,
    ui: State<'_, UiState>,
) -> Result<(), AppError> {
    Err(not_a_view_window())
}

/// Copy plain text to the clipboard for the focused view window.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-V2.
pub async fn ext_view_copy_text<R: Runtime>(
    text: String,
    window: WebviewWindow<R>,
    ui: State<'_, UiState>,
) -> Result<(), AppError> {
    Err(not_a_view_window())
}

/// Whether a view window has focus, for the launcher's blur handler.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-V2.
pub async fn ext_view_focus_state<R: Runtime>(
    app: AppHandle<R>,
) -> Result<ExtViewFocusState, AppError> {
    Ok(ExtViewFocusState::default())
}

/// The view windows that exist, visible or cached hidden, for Settings.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-V2.
pub async fn ext_view_list<R: Runtime>(
    app: AppHandle<R>,
    ui: State<'_, UiState>,
) -> Result<Vec<ExtViewInfo>, AppError> {
    Ok(Vec::new())
}

/// Forget saved window geometry for one view, or for every view of the
/// extension when `view_id` is `None`. Returns the number of entries
/// removed.
#[tauri::command]
#[allow(unused_variables)] // Stub until WP-V2.
pub async fn ext_view_reset_geometry(
    extension_id: String,
    view_id: Option<String>,
) -> Result<u32, AppError> {
    Ok(0)
}
