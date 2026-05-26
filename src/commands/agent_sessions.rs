//! Tauri commands for the generic agent-session surface.
//!
//! Dispatch by `provider_id` to the registered `AgentSessionProvider`.
//! Adding a new provider (Claude Code, Codex, Ollama, ...) requires no
//! changes here — only an entry in `AgentSessionRegistry::new()`.

use crate::agent_sessions::{AgentMessage, AgentSession, ProviderInfo, SessionLocator};
use crate::error::AppError;
use crate::state::FeatureServices;
use tauri::State;

#[tauri::command]
pub async fn agent_session_providers(
    features: State<'_, FeatureServices>,
) -> Result<Vec<ProviderInfo>, AppError> {
    Ok(features.agent_session_registry.list_providers())
}

#[tauri::command]
pub async fn agent_list_sessions(
    provider_id: String,
    limit: Option<usize>,
    features: State<'_, FeatureServices>,
) -> Result<Vec<AgentSession>, AppError> {
    let registry = features.agent_session_registry.clone();
    let limit = limit.unwrap_or(50);
    tauri::async_runtime::spawn_blocking(move || {
        let provider = registry
            .get(&provider_id)
            .ok_or_else(|| AppError::internal(format!("Unknown provider: {}", provider_id)))?;
        provider.list_sessions(limit)
    })
    .await
    .map_err(|e| AppError::internal(format!("Task: {}", e)))?
}

#[tauri::command]
pub async fn agent_load_session(
    provider_id: String,
    locator: SessionLocator,
    features: State<'_, FeatureServices>,
) -> Result<Vec<AgentMessage>, AppError> {
    let registry = features.agent_session_registry.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let provider = registry
            .get(&provider_id)
            .ok_or_else(|| AppError::internal(format!("Unknown provider: {}", provider_id)))?;
        provider.load_session(&locator)
    })
    .await
    .map_err(|e| AppError::internal(format!("Task: {}", e)))?
}

#[tauri::command]
pub async fn agent_check_session_updated(
    provider_id: String,
    locator: SessionLocator,
    since_ms: i64,
    features: State<'_, FeatureServices>,
) -> Result<Option<i64>, AppError> {
    let registry = features.agent_session_registry.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let provider = registry
            .get(&provider_id)
            .ok_or_else(|| AppError::internal(format!("Unknown provider: {}", provider_id)))?;
        provider.check_session_updated(&locator, since_ms)
    })
    .await
    .map_err(|e| AppError::internal(format!("Task: {}", e)))?
}
