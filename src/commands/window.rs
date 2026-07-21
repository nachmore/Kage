//! Stable public facade for window commands and helpers.
//!
//! Tauri command macros generate module-scoped dispatch helpers, so command
//! definitions remain here even though their implementations live in a child module.

mod implementation;

use crate::error::AppError;
use tauri::WebviewWindow;

pub use implementation::{
    center_floating_on_active_monitor, center_window_on_active_monitor, mark_focused_chat,
    schedule_chat_shutdown_check_public, show_floating_at_mouse, show_inline_assist_with_context,
    toggle_floating_window, ChatWindowInfo,
};

#[tauri::command]
pub async fn test_floating_window(app: tauri::AppHandle) -> Result<String, AppError> {
    implementation::test_floating_window(app).await
}

#[tauri::command]
pub async fn start_drag_window(window: WebviewWindow) -> Result<(), AppError> {
    implementation::start_drag_window(window).await
}

#[tauri::command]
pub async fn open_chat_window(app: tauri::AppHandle) -> Result<(), AppError> {
    implementation::open_chat_window(app).await
}

#[tauri::command]
pub async fn open_new_chat_window(
    resume_session_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<String, AppError> {
    implementation::open_new_chat_window(resume_session_id, app).await
}

#[tauri::command]
pub async fn close_chat_window(
    label: String,
    app: tauri::AppHandle,
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<(), AppError> {
    implementation::close_chat_window(label, app, ui).await
}

#[tauri::command]
pub async fn list_chat_windows(
    app: tauri::AppHandle,
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<Vec<ChatWindowInfo>, AppError> {
    implementation::list_chat_windows(app, ui).await
}

#[tauri::command]
pub async fn resize_floating_window(
    window: WebviewWindow,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<(), AppError> {
    implementation::resize_floating_window(window, width, height).await
}

#[tauri::command]
pub async fn open_settings_window(
    app: tauri::AppHandle,
    section: Option<String>,
    sub_section: Option<String>,
) -> Result<(), AppError> {
    implementation::open_settings_window(app, section, sub_section).await
}

#[tauri::command]
pub async fn show_context_menu(x: i32, y: i32, app: tauri::AppHandle) -> Result<(), AppError> {
    implementation::show_context_menu(x, y, app).await
}

#[tauri::command]
pub async fn set_floating_opacity(app: tauri::AppHandle, opacity: f64) -> Result<(), AppError> {
    implementation::set_floating_opacity(app, opacity).await
}

#[tauri::command]
pub async fn apply_chat_window_size(
    app: tauri::AppHandle,
    width: u32,
    height: u32,
) -> Result<(), AppError> {
    implementation::apply_chat_window_size(app, width, height).await
}

#[tauri::command]
pub async fn save_window_position(
    features: tauri::State<'_, crate::state::FeatureServices>,
    x: i32,
    y: i32,
) -> Result<(), AppError> {
    implementation::save_window_position(features, x, y).await
}

#[tauri::command]
pub async fn save_chat_window_geometry(
    features: tauri::State<'_, crate::state::FeatureServices>,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
) -> Result<(), AppError> {
    implementation::save_chat_window_geometry(features, width, height, x, y).await
}

#[tauri::command]
pub async fn get_last_selection(
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<Option<String>, AppError> {
    implementation::get_last_selection(ui).await
}

#[tauri::command]
pub fn notify_frontend_ready(ui: tauri::State<'_, crate::state::UiState>) {
    implementation::notify_frontend_ready(ui);
}

#[tauri::command]
pub async fn get_source_window(
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<Option<serde_json::Value>, AppError> {
    implementation::get_source_window(ui).await
}

#[tauri::command]
pub async fn get_screen_context(
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<Option<String>, AppError> {
    implementation::get_screen_context(ui).await
}

#[tauri::command]
pub async fn show_inline_assist(app: tauri::AppHandle) -> Result<(), AppError> {
    implementation::show_inline_assist(app).await
}

#[tauri::command]
pub async fn inline_assist_apply(
    text: String,
    app: tauri::AppHandle,
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<(), AppError> {
    implementation::inline_assist_apply(text, app, ui).await
}
