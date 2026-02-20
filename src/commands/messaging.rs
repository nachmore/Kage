use crate::state::AppState;
use log::{info, error};
use tauri::{async_runtime, Emitter, Manager, State, WebviewWindow};

/// Set up the notification handler on the ACP client.
/// This should be called once after the client is created.
/// The handler dispatches all ACP notifications to the appropriate Tauri events.
pub fn setup_notification_handler(
    client: &crate::acp_client::AcpClient,
    app: &tauri::AppHandle,
    state_config: std::sync::Arc<tokio::sync::Mutex<crate::config::Config>>,
    pipe_stdin: std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<std::sync::Mutex<std::process::ChildStdin>>>>>,
    tcp_writer: std::sync::Arc<std::sync::Mutex<Option<std::net::TcpStream>>>,
    slash_commands: std::sync::Arc<std::sync::Mutex<Vec<crate::state::SlashCommand>>>,
    pending_permission: std::sync::Arc<std::sync::Mutex<Option<crate::state::PendingPermission>>>,
    available_models: std::sync::Arc<std::sync::Mutex<Vec<crate::state::AcpModel>>>,
) {
    let app_handle = app.clone();
    let config = state_config;
    let pipe_stdin = pipe_stdin;
    let tcp_writer = tcp_writer;
    let slash_cmds = slash_commands;
    let pending_perm = pending_permission;
    let _models = available_models;
    let accumulated = client.streaming_accumulator.clone();

    client.set_notification_handler(move |notification: serde_json::Value| {
        let method = notification.get("method").and_then(|m| m.as_str()).unwrap_or("");

        if method == "session/request_permission" {
            handle_permission_notification(
                &notification, &app_handle, &config, &pipe_stdin, &tcp_writer, &pending_perm,
            );
            return;
        }

        if method == "session/update" {
            if let Some(update) = notification.get("params").and_then(|p| p.get("update")) {
                if let Some(kind) = update.get("sessionUpdate").and_then(|v| v.as_str()) {
                    if kind == "agent_message_chunk" {
                        if let Some(text) = update.get("content").and_then(|c| c.get("text")).and_then(|t| t.as_str()) {
                            let mut acc = accumulated.lock().unwrap();
                            acc.push_str(text);
                            let _ = app_handle.emit("message_chunk", acc.clone());
                        }
                        return;
                    }
                    if kind == "tool_call" || kind == "tool_call_update" {
                        let _ = app_handle.emit("tool_call_update", &notification);
                        return;
                    }
                }
            }
            return;
        }

        if method == "_kiro.dev/commands/available" {
            if let Some(commands) = notification.get("params")
                .and_then(|p| p.get("commands"))
                .and_then(|c| c.as_array())
            {
                if let Ok(parsed) = serde_json::from_value::<Vec<crate::state::SlashCommand>>(
                    serde_json::Value::Array(commands.clone()),
                ) {
                    info!("Received {} slash commands from ACP", parsed.len());
                    if let Ok(mut cmds) = slash_cmds.lock() {
                        *cmds = parsed;
                    }
                }
            }
            let _ = app_handle.emit("slash_commands_available", &notification);
            return;
        }

        if method.starts_with("_kiro.dev/") {
            let _ = app_handle.emit("tool_call_update", &notification);
            return;
        }

        info!("Unhandled notification: {}", method);
    });
}

fn handle_permission_notification(
    notification: &serde_json::Value,
    app_handle: &tauri::AppHandle,
    config: &std::sync::Arc<tokio::sync::Mutex<crate::config::Config>>,
    pipe_stdin: &std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<std::sync::Mutex<std::process::ChildStdin>>>>>,
    tcp_writer: &std::sync::Arc<std::sync::Mutex<Option<std::net::TcpStream>>>,
    pending_perm: &std::sync::Arc<std::sync::Mutex<Option<crate::state::PendingPermission>>>,
) {
    let tool_title = notification
        .get("params")
        .and_then(|p| p.get("toolCall"))
        .and_then(|tc| tc.get("title"))
        .and_then(|t| t.as_str())
        .unwrap_or("unknown");

    let timestamp = chrono::Utc::now().to_rfc3339();
    let mut config_guard = async_runtime::block_on(config.lock());

    let existing = config_guard.tool_permissions.tools.iter_mut().find(|t| t.title == tool_title);
    if let Some(tool) = existing {
        tool.last_seen = timestamp;
    } else {
        config_guard.tool_permissions.tools.push(crate::config::ToolPolicy {
            title: tool_title.to_string(),
            policy: "ask".to_string(),
            last_seen: timestamp,
        });
    }
    let _ = config_guard.save();

    let policy = if config_guard.tool_permissions.trust_all {
        "allow".to_string()
    } else {
        config_guard.tool_permissions.tools.iter()
            .find(|t| t.title == tool_title)
            .map(|t| t.policy.clone())
            .unwrap_or_else(|| "ask".to_string())
    };
    drop(config_guard);

    let send_response = |option_id: &str| {
        if let Some(request_id) = notification.get("id") {
            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "outcome": { "outcome": "selected", "optionId": option_id } }
            });
            if let Ok(json) = serde_json::to_string(&response) {
                use std::io::Write;
                let stdin_arc = {
                    let guard = pipe_stdin.lock().ok();
                    guard.and_then(|g| g.as_ref().map(|a| a.clone()))
                };
                if let Some(arc) = stdin_arc {
                    if let Ok(mut stdin) = arc.lock() {
                        let _ = write!(stdin, "{}\n", json);
                        let _ = stdin.flush();
                        return;
                    }
                }
                if let Ok(guard) = tcp_writer.lock() {
                    if let Some(ref stream) = *guard {
                        if let Ok(mut ws) = stream.try_clone() {
                            drop(guard);
                            let _ = write!(ws, "{}\n", json);
                            let _ = ws.flush();
                        }
                    }
                }
            }
        }
    };

    match policy.as_str() {
        "allow" => send_response("allow_once"),
        "deny" => send_response("reject_once"),
        _ => {
            let session_id = notification.get("params")
                .and_then(|p| p.get("sessionId"))
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            if let Ok(mut pending) = pending_perm.lock() {
                *pending = Some(crate::state::PendingPermission {
                    request_id: notification.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    tool_title: tool_title.to_string(),
                    session_id,
                });
            }
            let _ = app_handle.emit("permission_request", serde_json::json!({
                "notification": notification,
                "auto_approve": false
            }));
        }
    }
}

// --- Tauri Commands ---

#[tauri::command]
pub async fn send_message_streaming(
    message: String,
    attachments: Option<Vec<serde_json::Value>>,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<(), String> {
    info!("Sending message: {}", message);
    let client = state.acp_client.clone();
    let config = state.config.clone();
    let window_clone = window.clone();

    async_runtime::spawn_blocking(move || {
        let client_arc = client.clone();
        let config_arc = config.clone();
        let client = async_runtime::block_on(client.lock());

        if !client.is_connected() {
            if let Err(e) = client.connect() {
                let _ = window.emit("message_error", format!("Unable to connect: {}", e));
                return;
            }
        }

        // The notification handler (set up at app init) handles all streaming
        // chunks, permissions, and tool calls via Tauri events.
        let had_attachments = attachments.as_ref().map_or(false, |a| !a.is_empty());
        if let Err(e) = client.send_chat_streaming(message, attachments) {
            let error_str = format!("{}", e);
            let is_image_error = had_attachments && (
                error_str.contains("Internal error")
                || error_str.contains("image")
                || error_str.contains("unsupported")
                || error_str.contains("response stream")
            );

            if is_image_error {
                // The ACP connection is likely stuck after an image error.
                // Disconnect, clear the session, reconnect, and start fresh.
                info!("Image-related error detected — resetting ACP connection and session");
                client.disconnect();
                client.set_session_id(None);

                let reconnected = match client.connect() {
                    Ok(_) => {
                        match client.create_session(None) {
                            Ok(_) => true,
                            Err(e) => {
                                error!("Failed to create new session after image error: {}", e);
                                false
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to reconnect after image error: {}", e);
                        false
                    }
                };

                let _ = window.emit("session_reset", serde_json::json!({
                    "reason": "image_unsupported",
                    "reconnected": reconnected,
                }));
            } else {
                let _ = window.emit("message_error", format!("Failed to send: {}", error_str));
            }
            return;
        }

        let _ = window_clone.emit("message_complete", ());

        // Trigger auto-steering generation periodically
        drop(client); // Release the lock before spawning background task
        crate::auto_steering::maybe_generate_steering(client_arc, config_arc);
    });

    Ok(())
}

#[tauri::command]
pub async fn send_permission_response(
    request_id: serde_json::Value,
    option_id: String,
    tool_title: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    info!("Permission response: {}={}", tool_title, option_id);

    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": { "outcome": { "outcome": "selected", "optionId": option_id } }
    });
    let json = serde_json::to_string(&response).map_err(|e| format!("Serialize: {}", e))?;

    let client = state.acp_client.lock().await;
    client.write_line(&json).map_err(|e| format!("Write: {}", e))?;

    if option_id == "allow_always" {
        let mut config = state.config.lock().await;
        if let Some(tool) = config.tool_permissions.tools.iter_mut().find(|t| t.title == tool_title) {
            tool.policy = "allow".to_string();
        }
        config.save().map_err(|e| format!("Save: {}", e))?;
    }

    if let Ok(mut pending) = state.pending_permission.lock() {
        *pending = None;
    }

    Ok(())
}

#[tauri::command]
pub async fn check_connection(state: State<'_, AppState>) -> Result<bool, String> {
    let client = state.acp_client.lock().await;
    Ok(client.is_connected())
}

#[tauri::command]
pub async fn reconnect_acp(state: State<'_, AppState>) -> Result<bool, String> {
    let client = state.acp_client.lock().await;
    match client.connect() {
        Ok(_) => Ok(true),
        Err(e) => Err(format!("Failed to reconnect: {}", e)),
    }
}

#[tauri::command]
pub async fn open_chat_with_message(
    message: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if let Some(floating) = app.get_webview_window("floating") {
        let _ = floating.hide();
    }
    if let Some(main) = app.get_webview_window("main") {
        // Center on the active monitor
        crate::commands::window::center_window_on_active_monitor(&main);
        let _ = main.show();
        let _ = main.set_focus();
        let _ = main.emit("initial_message", message.clone());

        let client = state.acp_client.clone();
        let window = main.clone();

        async_runtime::spawn_blocking(move || {
            let client = async_runtime::block_on(client.lock());
            if !client.is_connected() {
                if let Err(e) = client.connect() {
                    let _ = window.emit("message_error", format!("Unable to connect: {}", e));
                    return;
                }
            }
            if let Err(e) = client.send_chat_streaming(message, None) {
                let _ = window.emit("message_error", format!("Failed to send: {}", e));
                return;
            }
            let _ = window.emit("message_complete", ());
        });
    }
    Ok(())
}

#[tauri::command]
pub async fn dismiss_pending_permission(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    let pending = {
        let guard = state.pending_permission.lock().map_err(|e| format!("Lock: {}", e))?;
        guard.clone()
    };

    if let Some(perm) = pending {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": perm.request_id,
            "result": { "outcome": { "outcome": "selected", "optionId": "reject_once" } }
        });
        let json = serde_json::to_string(&response).map_err(|e| format!("Serialize: {}", e))?;

        let client = state.acp_client.lock().await;
        client.write_line(&json).map_err(|e| format!("Write: {}", e))?;

        if let Ok(mut guard) = state.pending_permission.lock() {
            *guard = None;
        }
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.emit("permission_dismissed", ());
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

#[tauri::command]
pub async fn has_pending_permission(state: State<'_, AppState>) -> Result<bool, String> {
    let guard = state.pending_permission.lock().map_err(|e| format!("Lock: {}", e))?;
    Ok(guard.is_some())
}

#[tauri::command]
pub async fn get_slash_commands(state: State<'_, AppState>) -> Result<Vec<crate::state::SlashCommand>, String> {
    let cmds = state.slash_commands.lock().map_err(|e| format!("Lock: {}", e))?;
    Ok(cmds.clone())
}

#[tauri::command]
pub async fn execute_slash_command(
    command: String,
    args: Option<serde_json::Value>,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<serde_json::Value, String> {
    let client = state.acp_client.clone();

    let result = async_runtime::spawn_blocking(move || {
        let client = async_runtime::block_on(client.lock());
        if !client.is_connected() {
            return Err("Not connected".to_string());
        }
        let session_id = client.get_session_id().ok_or("No active session")?;
        let cmd_name = command.strip_prefix('/').unwrap_or(&command);

        let request = crate::acp_client::AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(3),
            method: "_kiro.dev/commands/execute".to_string(),
            params: serde_json::json!({
                "sessionId": session_id,
                "command": { "command": cmd_name, "args": args.unwrap_or(serde_json::json!({})) }
            }),
        };

        let response = client.send_request(&request).map_err(|e| format!("Command failed: {}", e))?;
        if let Some(error) = response.error {
            return Err(format!("{} (code: {})", error.message, error.code));
        }
        Ok(response.result.unwrap_or(serde_json::json!(null)))
    })
    .await
    .map_err(|e| format!("Task: {}", e))??;

    let _ = window.emit("slash_command_result", &result);
    Ok(result)
}

#[tauri::command]
pub async fn get_slash_command_options(
    _command: String,
    _state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({ "options": [] }))
}

#[tauri::command]
pub async fn send_steering_message(
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let config = state.config.lock().await;
    let assistant = &config.acp.assistant;

    let mut parts: Vec<String> = Vec::new();

    if let Some(ref path) = assistant.user_steering_path {
        if !path.is_empty() {
            if let Ok(content) = std::fs::read_to_string(path) {
                if !content.trim().is_empty() {
                    parts.push(content);
                }
            }
        }
    }

    if assistant.auto_steering_enabled {
        if let Ok(auto_path) = crate::config::Config::get_auto_steering_path() {
            if auto_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&auto_path) {
                    if !content.trim().is_empty() {
                        parts.push(content);
                    }
                }
            }
        }
    }

    drop(config);

    if parts.is_empty() {
        return Ok(false);
    }

    let steering_msg = format!(
        "{} {}",
        crate::commands::system::STEERING_MSG_PREFIX,
        parts.join("\n\n---\n\n")
    );

    let client = state.acp_client.clone();
    async_runtime::spawn_blocking(move || {
        let client = async_runtime::block_on(client.lock());
        if client.is_connected() {
            let _ = client.send_chat_streaming(steering_msg, None);
        }
    });

    Ok(true)
}

#[tauri::command]
pub async fn get_available_models(
    state: State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let models = state.available_models.lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    info!("get_available_models called, returning {} models", models.len());
    Ok(models.iter().map(|m| serde_json::json!({
        "modelId": m.model_id,
        "name": m.name,
        "description": m.description,
    })).collect())
}
