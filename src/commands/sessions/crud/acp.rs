//! ACP session adoption and creation.

use super::super::*;

/// Adopt or create a session for the calling window.
pub async fn switch_acp_session<R: tauri::Runtime>(
    session_id: Option<String>,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    ui: State<'_, crate::state::UiState>,
    window: tauri::WebviewWindow<R>,
    app: tauri::AppHandle<R>,
) -> Result<String, AppError> {
    let client_guard = acp.client.clone();
    let available_models = acp.available_models.clone();
    let config = features.config.clone();
    let session_cache = features.session_cache.clone();
    let window_sessions = ui.window_sessions.clone();
    let window_label = window.label().to_string();

    tauri::async_runtime::spawn_blocking(move || -> Result<String, AppError> {
        switch_acp_session_blocking(
            session_id,
            client_guard,
            available_models,
            config,
            session_cache,
            window_sessions,
            window_label,
            app,
        )
    })
    .await
    .map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.session.load_failed",
            &[("reason", &e.to_string())],
        )
    })?
}

#[allow(clippy::too_many_arguments)] // Mirrors the Tauri command's state params.
fn switch_acp_session_blocking<R: tauri::Runtime>(
    session_id: Option<String>,
    client_guard: std::sync::Arc<crate::acp_client::AcpClient>,
    available_models: std::sync::Arc<std::sync::Mutex<Vec<crate::state::AcpModel>>>,
    config: std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    session_cache: std::sync::Arc<std::sync::Mutex<Option<SessionCache>>>,
    window_sessions: std::sync::Arc<std::sync::Mutex<HashMap<String, String>>>,
    window_label: String,
    app: tauri::AppHandle<R>,
) -> Result<String, AppError> {
    if !client_guard.is_healthy() {
        info!("Connection not healthy, restarting for session switch...");
        if let Err(e) = client_guard.restart_connection() {
            error!("Connection restart failed: {}", e);
            return Err(AppError::keyed(
                ErrorKind::ConnectionLost,
                "errors.session.connect_failed",
                &[("reason", &e.to_string())],
            ));
        }
    }

    match session_id {
        Some(id) => load_existing_session(
            &id,
            client_guard,
            available_models,
            config,
            session_cache,
            window_sessions,
            window_label,
            app,
        ),
        None => create_session(
            client_guard,
            available_models,
            config,
            session_cache,
            window_sessions,
            window_label,
            app,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn load_existing_session<R: tauri::Runtime>(
    id: &str,
    client: std::sync::Arc<crate::acp_client::AcpClient>,
    available_models: std::sync::Arc<std::sync::Mutex<Vec<crate::state::AcpModel>>>,
    config: std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    session_cache: std::sync::Arc<std::sync::Mutex<Option<SessionCache>>>,
    window_sessions: std::sync::Arc<std::sync::Mutex<HashMap<String, String>>>,
    window_label: String,
    app: tauri::AppHandle<R>,
) -> Result<String, AppError> {
    info!("Switching to existing session: {}", id);
    let cwd = {
        let sessions_dir = resolve_sessions_dir_locked(&config)?;
        let json_path = sessions_dir.join(format!("{}.json", id));
        fs::read_to_string(json_path)
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
            .and_then(|data| {
                data.get("cwd")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            })
    };
    let (loaded_id, models_json) = client.load_existing_session(id, cwd).map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.session.load_failed",
            &[("reason", &e.to_string())],
        )
    })?;
    crate::telemetry::track(
        &app,
        "session_resumed",
        Some(serde_json::json!({ "source": "manual" })),
    );
    update_available_models(&available_models, models_json, true);
    if let Ok(mut sessions) = window_sessions.lock() {
        sessions.insert(window_label.clone(), loaded_id.clone());
    }
    update_window_title(&app, &config, &session_cache, &window_label, &loaded_id);
    Ok(loaded_id)
}

#[allow(clippy::too_many_arguments)]
fn create_session<R: tauri::Runtime>(
    client: std::sync::Arc<crate::acp_client::AcpClient>,
    available_models: std::sync::Arc<std::sync::Mutex<Vec<crate::state::AcpModel>>>,
    config: std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    session_cache: std::sync::Arc<std::sync::Mutex<Option<SessionCache>>>,
    window_sessions: std::sync::Arc<std::sync::Mutex<HashMap<String, String>>>,
    window_label: String,
    app: tauri::AppHandle<R>,
) -> Result<String, AppError> {
    info!("Creating new session");
    let cwd = config.lock_or_recover().acp.agent.working_directory.clone();
    let (session_id, models_json) = client
        .create_session(cwd)
        .map_err(|e| format!("Failed to create session: {}", e))?;
    crate::telemetry::track(
        &app,
        "session_created",
        Some(serde_json::json!({ "source": "manual" })),
    );
    update_available_models(&available_models, models_json, false);

    let (default_model, steering_inputs) = {
        let cfg = config.lock_or_recover();
        (
            cfg.acp.agent.default_model.clone(),
            crate::commands::system::SteeringInputs::from_config(&cfg),
        )
    };
    if let Some(default_model) = default_model.filter(|model| !model.is_empty()) {
        info!("Applying default model to new session: {}", default_model);
        if let Err(e) = client.send_request(
            &client.vendor_method("commands/execute"),
            serde_json::json!({
                "sessionId": session_id,
                "command": { "command": "model", "args": { "modelName": default_model } }
            }),
        ) {
            error!("Failed to apply default model: {}", e);
        }
    }
    let parts = crate::commands::system::assemble_steering_parts(&steering_inputs);
    let steering_msg = format!(
        "{} {}",
        crate::commands::system::STEERING_MSG_PREFIX,
        parts.join("\n\n---\n\n")
    );
    let _ = client.send_chat_streaming(&session_id, &steering_msg, None);

    *session_cache.lock_or_recover() = None;
    if let Ok(mut sessions) = window_sessions.lock() {
        sessions.insert(window_label.clone(), session_id.clone());
    }
    update_window_title(&app, &config, &session_cache, &window_label, &session_id);
    Ok(session_id)
}

fn update_available_models(
    available_models: &std::sync::Arc<std::sync::Mutex<Vec<crate::state::AcpModel>>>,
    models_json: Vec<serde_json::Value>,
    keep_existing_when_empty: bool,
) {
    if keep_existing_when_empty && models_json.is_empty() {
        return;
    }
    if let Ok(models) =
        serde_json::from_value::<Vec<crate::state::AcpModel>>(serde_json::Value::Array(models_json))
    {
        if let Ok(mut available) = available_models.lock() {
            *available = models;
        }
    }
}
