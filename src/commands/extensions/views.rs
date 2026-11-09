//! Tauri commands for extension view windows
//! (docs/design/EXTENSION_VIEWS.md §16.1). First-party host code calls
//! them; every one is `null` in `COMMAND_CAPABILITIES`, so no extension
//! can. The work happens in `crate::ext_views`.

use super::*;
use crate::ext_views::{self, ExtViewBoot, ExtViewFocusState, ExtViewInfo, ExtViewOpened};
use tauri::{AppHandle, Runtime, WebviewWindow};

/// Open (or show again) an extension's view window from the calling
/// window. Called by the launcher, chat, settings and view pages after
/// their own checks; Rust repeats them against the installed manifest and
/// grant.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
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
    ext_views::open(
        &app,
        &window,
        &features,
        &ui.ext_views,
        &extension_id,
        &view_id,
        params,
        source.as_deref(),
    )
    .await
}

/// The boot record of the calling view window, derived from its label.
#[tauri::command]
pub async fn ext_view_boot<R: Runtime>(
    window: WebviewWindow<R>,
    ui: State<'_, UiState>,
    features: State<'_, FeatureServices>,
) -> Result<ExtViewBoot, AppError> {
    ext_views::boot(&window, &features, &ui.ext_views)
}

/// Put `text` in the launcher's input and show the launcher. Never sends.
#[tauri::command]
pub async fn ext_view_compose<R: Runtime>(
    text: String,
    window: WebviewWindow<R>,
    app: AppHandle<R>,
    ui: State<'_, UiState>,
) -> Result<(), AppError> {
    ext_views::compose(&app, &window, &ui.ext_views, text)
}

/// Copy plain text to the clipboard for the focused view window.
#[tauri::command]
pub async fn ext_view_copy_text<R: Runtime>(
    text: String,
    window: WebviewWindow<R>,
    ui: State<'_, UiState>,
) -> Result<(), AppError> {
    ext_views::copy_text(&window, &ui.ext_views, text).await
}

/// Whether a view window has focus, for the launcher's blur handler.
/// `false` at once when no view window is visible; otherwise waits up to
/// 120 ms for one to report focus.
#[tauri::command]
pub async fn ext_view_focus_state<R: Runtime>(
    app: AppHandle<R>,
) -> Result<ExtViewFocusState, AppError> {
    let Some(ui) = app.try_state::<UiState>() else {
        return Ok(ExtViewFocusState::default());
    };
    let registry = ui.ext_views.clone();
    Ok(ext_views::focus_state(&registry).await)
}

/// The view windows that exist, visible or cached hidden, for Settings.
#[tauri::command]
#[allow(unused_variables)] // The window list comes from the registry.
pub async fn ext_view_list<R: Runtime>(
    app: AppHandle<R>,
    ui: State<'_, UiState>,
) -> Result<Vec<ExtViewInfo>, AppError> {
    Ok(ui.ext_views.list())
}

/// Forget saved window geometry for one view, or for every view of the
/// extension when `view_id` is `None`. Returns the number of entries
/// removed.
#[tauri::command]
pub async fn ext_view_reset_geometry(
    extension_id: String,
    view_id: Option<String>,
) -> Result<u32, AppError> {
    let removed = tauri::async_runtime::spawn_blocking(move || {
        ext_views::geometry::remove_entries(&extension_id, view_id.as_deref())
    })
    .await
    .map_err(|e| std::io::Error::other(e.to_string()))
    .and_then(|r| r)
    .map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.ext_view.geometry_write_failed",
            &[("reason", &e.to_string())],
        )
    })?;
    info!("Reset {removed} saved view window position(s)");
    Ok(removed)
}
