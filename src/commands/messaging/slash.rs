//! Slash-command discovery/execute, available models, and steering sends.

use super::*;

#[tauri::command]
pub async fn get_slash_commands(
    acp: State<'_, AcpHandles>,
) -> Result<Vec<crate::state::SlashCommand>, AppError> {
    let cmds = acp
        .slash_commands
        .lock()
        .map_err(|_| AppError::keyed(ErrorKind::LockError, "errors.lock.acquire_failed", &[]))?;
    Ok(cmds.clone())
}

#[tauri::command]
pub async fn execute_slash_command<R: tauri::Runtime>(
    session_id: Option<String>,
    command: String,
    args: Option<serde_json::Value>,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    window: WebviewWindow<R>,
    app: tauri::AppHandle<R>,
) -> Result<serde_json::Value, AppError> {
    let client = acp.client.clone();
    // Snapshot before the move into spawn_blocking — we need both for
    // telemetry after the call returns.
    let cmd_for_event = command.clone();
    let args_for_event = args.clone();

    let result = async_runtime::spawn_blocking(move || -> Result<serde_json::Value, AppError> {
        if !client.is_connected() {
            return Err(AppError::keyed(
                ErrorKind::ConnectionLost,
                "errors.connection.not_connected",
                &[],
            ));
        }
        // Require a pinned session — slash commands are informational
        // (context, model, compact, …) and should never create a new
        // session as a side effect. The calling window is responsible
        // for obtaining a session before invoking.
        let session_id = match session_id {
            Some(id) if !id.trim().is_empty() => id,
            _ => {
                return Err(AppError::keyed(
                    ErrorKind::Internal,
                    "errors.session.no_session",
                    &[],
                ));
            }
        };
        let cmd_name = command.strip_prefix('/').unwrap_or(&command);

        let response = client
            .send_request(
                &client.vendor_method("commands/execute"),
                serde_json::json!({
                    "sessionId": session_id,
                    "command": { "command": cmd_name, "args": args.unwrap_or(serde_json::json!({})) }
                }),
            )
            .map_err(|e| {
                AppError::keyed(
                    ErrorKind::Internal,
                    "errors.command.failed",
                    &[("reason", &e.to_string())],
                )
            })?;
        if let Some(error) = response.error {
            return Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.agent.protocol_error",
                &[
                    ("reason", error.message.as_str()),
                    ("code", &error.code.to_string()),
                ],
            ));
        }
        Ok(response.result.unwrap_or(serde_json::json!(null)))
    })
    .await
    .map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.task.failed",
            &[("reason", &e.to_string())],
        )
    })??;

    // Telemetry — fire AFTER the call so we only count successful
    // command executions, not failed ones (though we don't bother
    // tracking the failure case as a separate event; that would be
    // adding noise without driving a decision).
    let cmd_name = cmd_for_event
        .strip_prefix('/')
        .unwrap_or(&cmd_for_event)
        .to_string();
    crate::telemetry::track(
        &app,
        "slash_command_used",
        Some(serde_json::json!({ "command": cmd_name })),
    );

    // `/model <name>` is a model-switch under the hood. Surface it as
    // a typed event so the dashboard doesn't have to filter slash
    // commands by argument shape. Model names are public identifiers
    // (claude-3-7-sonnet, gpt-4o, etc.) so passing them through is
    // safe — but we only do it for the model command, not arbitrary
    // slash command args (which can carry user content).
    if cmd_name == "model" {
        if let Some(model_name) = args_for_event
            .as_ref()
            .and_then(|v| v.get("modelName"))
            .and_then(|v| v.as_str())
        {
            crate::telemetry::track(
                &app,
                "model_changed",
                Some(serde_json::json!({ "model": model_name })),
            );
        }
    }

    // Per-agent prettifying. Some agents return rich structured `data` that
    // their one-line `message` discards (e.g. Kiro's /context breakdown).
    // The active agent's formatter turns that into markdown, attached as
    // `displayMessage` so the frontend renders it while `message` stays
    // intact for callers that parse it (e.g. chat's context-ring %-scrape).
    // Agents with no formatter (Claude/Codex today) leave the result as-is.
    let mut result = result;
    let agent_kind = {
        let config = features.config.lock().map_err(|_| {
            AppError::keyed(ErrorKind::LockError, "errors.lock.acquire_failed", &[])
        })?;
        crate::agent_presets::detect(&config)
    };
    if let Some(md) = crate::slash_format::format_slash_result(agent_kind, &cmd_name, &result) {
        if let Some(obj) = result.as_object_mut() {
            obj.insert("displayMessage".to_string(), serde_json::Value::String(md));
        }
    }

    // Targeted emit back to the calling window — `WebviewWindow::emit`
    // dispatches to every webview with a listener, but `slash_command_result`
    // is a single-source-single-sink reply, so scope it.
    let _ = app.emit_to(
        tauri::EventTarget::webview_window(window.label()),
        "slash_command_result",
        result.clone(),
    );
    Ok(result)
}

#[tauri::command]
pub async fn get_slash_command_options(_command: String) -> Result<serde_json::Value, AppError> {
    Ok(serde_json::json!({ "options": [] }))
}

#[tauri::command]
pub async fn send_steering_message(
    session_id: String,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
) -> Result<bool, AppError> {
    let steering_msg = {
        // Snapshot fields under the lock, drop the guard, then do disk
        // reads. Holding the global config Mutex across blocking I/O
        // would block every concurrent config reader for the duration.
        let inputs = {
            let config = features.config.lock().map_err(|_| {
                AppError::keyed(ErrorKind::LockError, "errors.lock.acquire_failed", &[])
            })?;
            crate::commands::system::SteeringInputs::from_config(&config)
        };
        let parts = crate::commands::system::assemble_steering_parts(&inputs);
        format!(
            "{} {}",
            crate::commands::system::STEERING_MSG_PREFIX,
            parts.join("\n\n---\n\n")
        )
    };

    let client = acp.client.clone();
    async_runtime::spawn_blocking(move || {
        if client.is_connected() {
            if let Err(e) = client.send_chat_streaming(&session_id, &steering_msg, None) {
                warn!("Failed to send auto-steering message: {}", e);
            }
        }
    });

    Ok(true)
}

#[tauri::command]
pub async fn get_available_models(
    acp: State<'_, AcpHandles>,
) -> Result<Vec<serde_json::Value>, AppError> {
    let models = acp
        .available_models
        .lock()
        .map_err(|_| AppError::keyed(ErrorKind::LockError, "errors.lock.acquire_failed", &[]))?;
    Ok(models
        .iter()
        .map(|m| {
            serde_json::json!({
                "modelId": m.model_id,
                "name": m.name,
                "description": m.description,
            })
        })
        .collect())
}
