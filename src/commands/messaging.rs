use crate::state::AppState;
use log::{error, info};
use tauri::{async_runtime, Emitter, Manager, State, WebviewWindow};

#[tauri::command]
pub async fn send_message_streaming(
    message: String,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<(), String> {
    info!("Sending message: {}", message);
    let client = state.acp_client.clone();
    let config = state.config.clone();
    let pipe_stdin_handle = state.pipe_stdin.clone();
    let tcp_writer_handle = state.tcp_writer.clone();
    let pending_perm = state.pending_permission.clone();
    let window_clone = window.clone();

    async_runtime::spawn_blocking(move || {
        let client = async_runtime::block_on(client.lock());

        if !client.is_connected() {
            info!("Not connected, attempting to connect...");
            if let Err(e) = client.connect() {
                error!("Connection failed: {}", e);
                let error_msg = format!(
                    "Unable to connect to Kiro CLI. Please ensure kiro-cli is running.\n\nError: {}",
                    e
                );
                let _ = window.emit("message_error", error_msg);
                return;
            }
        }

        // Create permission callback
        let window_for_permission = window_clone.clone();
        let config_for_permission = config.clone();
        let pipe_stdin_for_perm = pipe_stdin_handle.clone();
        let tcp_writer_for_perm = tcp_writer_handle.clone();
        let pending_perm_for_cb = pending_perm.clone();
        let permission_callback = Box::new(move |notification: serde_json::Value| {
            let mut config_guard = async_runtime::block_on(config_for_permission.lock());

            let tool_title = notification
                .get("params")
                .and_then(|p| p.get("toolCall"))
                .and_then(|tc| tc.get("title"))
                .and_then(|t| t.as_str())
                .unwrap_or("unknown");

            let timestamp = chrono::Utc::now().to_rfc3339();
            let existing = config_guard
                .tool_permissions
                .tools
                .iter_mut()
                .find(|t| t.title == tool_title);
            if let Some(tool) = existing {
                tool.last_seen = timestamp;
            } else {
                config_guard
                    .tool_permissions
                    .tools
                    .push(crate::config::ToolPolicy {
                        title: tool_title.to_string(),
                        policy: "ask".to_string(),
                        last_seen: timestamp,
                    });
            }
            let _ = config_guard.save();

            let policy = if config_guard.tool_permissions.trust_all {
                "allow".to_string()
            } else {
                config_guard
                    .tool_permissions
                    .tools
                    .iter()
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
                    if let Ok(response_json) = serde_json::to_string(&response) {
                        use std::io::Write;
                        info!(
                            "📤 Auto-responding permission ({}): {}",
                            option_id, response_json
                        );

                        if let Ok(guard) = pipe_stdin_for_perm.lock() {
                            if let Some(ref stdin_arc) = *guard {
                                let stdin_clone = stdin_arc.clone();
                                drop(guard);
                                if let Ok(mut stdin) = stdin_clone.lock() {
                                    let _ = write!(stdin, "{}\n", response_json);
                                    let _ = stdin.flush();
                                    info!("✅ Auto-response sent via Pipe");
                                    return;
                                };
                            }
                        }
                        if let Ok(guard) = tcp_writer_for_perm.lock() {
                            if let Some(ref stream) = *guard {
                                if let Ok(mut ws) = stream.try_clone() {
                                    drop(guard);
                                    let _ = write!(ws, "{}\n", response_json);
                                    let _ = ws.flush();
                                    info!("✅ Auto-response sent via TCP");
                                    return;
                                }
                            }
                        }
                        error!("❌ Failed to send auto-response: no write handle");
                    }
                }
            };

            match policy.as_str() {
                "allow" => {
                    info!("🔓 Policy=allow for tool: {}", tool_title);
                    send_response("allow_once");
                }
                "deny" => {
                    info!("🚫 Policy=deny for tool: {}", tool_title);
                    send_response("reject_once");
                }
                _ => {
                    info!("❓ Policy=ask for tool: {}", tool_title);
                    // Track the pending permission so other windows can detect and auto-deny it
                    let session_id = notification
                        .get("params")
                        .and_then(|p| p.get("sessionId"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Ok(mut pending) = pending_perm_for_cb.lock() {
                        *pending = Some(crate::state::PendingPermission {
                            request_id: notification.get("id").cloned().unwrap_or(serde_json::Value::Null),
                            tool_title: tool_title.to_string(),
                            session_id,
                        });
                    }
                    let _ = window_for_permission.emit(
                        "permission_request",
                        serde_json::json!({
                            "notification": notification,
                            "auto_approve": false
                        }),
                    );
                }
            }
        });

        // Create notification callback for tool_call updates
        let window_for_notif = window.clone();
        let notification_callback = Box::new(move |notification: serde_json::Value| {
            let _ = window_for_notif.emit("tool_call_update", notification);
        });

        if let Err(e) = client.send_chat_streaming(
            message,
            |chunk| {
                let _ = window.emit("message_chunk", chunk);
            },
            Some(permission_callback),
            Some(notification_callback),
        ) {
            error!("Send error: {}", e);
            let error_msg = format!(
                "Failed to send message. The connection may have been lost.\n\nError: {}",
                e
            );
            let _ = window.emit("message_error", error_msg);
            return;
        }

        let _ = window.emit("message_complete", ());
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
    info!(
        "Sending permission response: option_id={}, tool_title={}",
        option_id, tool_title
    );

    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": {
            "outcome": {
                "outcome": "selected",
                "optionId": option_id
            }
        }
    });
    let response_json =
        serde_json::to_string(&response).map_err(|e| format!("Failed to serialize: {}", e))?;

    info!("📤 Permission response JSON: {}", response_json);

    use std::io::Write;

    let sent = {
        let pipe_guard = state
            .pipe_stdin
            .lock()
            .map_err(|e| format!("Failed to lock pipe_stdin: {}", e))?;
        if let Some(ref stdin_arc) = *pipe_guard {
            let stdin_clone = stdin_arc.clone();
            drop(pipe_guard);
            let mut stdin = stdin_clone
                .lock()
                .map_err(|e| format!("Failed to lock stdin: {}", e))?;
            write!(stdin, "{}\n", response_json)
                .map_err(|e| format!("Failed to write: {}", e))?;
            stdin
                .flush()
                .map_err(|e| format!("Failed to flush: {}", e))?;
            info!("✅ Permission response sent via Pipe");
            true
        } else {
            drop(pipe_guard);
            let tcp_guard = state
                .tcp_writer
                .lock()
                .map_err(|e| format!("Failed to lock tcp_writer: {}", e))?;
            if let Some(ref stream) = *tcp_guard {
                let mut write_stream = stream
                    .try_clone()
                    .map_err(|e| format!("Failed to clone stream: {}", e))?;
                drop(tcp_guard);
                write!(write_stream, "{}\n", response_json)
                    .map_err(|e| format!("Failed to write: {}", e))?;
                write_stream
                    .flush()
                    .map_err(|e| format!("Failed to flush: {}", e))?;
                info!("✅ Permission response sent via TCP");
                true
            } else {
                drop(tcp_guard);
                false
            }
        }
    };

    if !sent {
        return Err("Not connected - no write handle available".to_string());
    }

    if option_id == "allow_always" {
        let mut config = state.config.lock().await;
        if let Some(tool) = config
            .tool_permissions
            .tools
            .iter_mut()
            .find(|t| t.title == tool_title)
        {
            tool.policy = "allow".to_string();
        }
        config
            .save()
            .map_err(|e| format!("Failed to save config: {}", e))?;
    }

    // Clear the pending permission tracker
    if let Ok(mut pending) = state.pending_permission.lock() {
        *pending = None;
    }

    Ok(())
}

#[tauri::command]
pub async fn check_connection(state: State<'_, AppState>) -> Result<bool, String> {
    let client = state.acp_client.lock().await;
    let is_connected = client.is_connected();
    info!(
        "Connection check: {}",
        if is_connected {
            "connected"
        } else {
            "disconnected"
        }
    );
    Ok(is_connected)
}

#[tauri::command]
pub async fn reconnect_acp(state: State<'_, AppState>) -> Result<bool, String> {
    info!("Manual reconnection requested");
    let client = state.acp_client.lock().await;

    match client.connect() {
        Ok(_) => {
            info!("Reconnection successful");
            Ok(true)
        }
        Err(e) => {
            error!("Reconnection failed: {}", e);
            Err(format!("Failed to reconnect: {}", e))
        }
    }
}

#[tauri::command]
pub async fn open_chat_with_message(
    message: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    info!("Opening chat with message: {}", message);

    if let Some(floating_window) = app.get_webview_window("floating") {
        let _ = floating_window.hide();
    }

    if let Some(main_window) = app.get_webview_window("main") {
        let _ = main_window.show();
        let _ = main_window.set_focus();

        let _ = main_window.emit("initial_message", message.clone());

        let client = state.acp_client.clone();
        let window = main_window.clone();

        async_runtime::spawn_blocking(move || {
            let client = async_runtime::block_on(client.lock());

            if !client.is_connected() {
                info!("Not connected, attempting to connect...");
                if let Err(e) = client.connect() {
                    error!("Connection failed: {}", e);
                    let error_msg = format!(
                        "Unable to connect to Kiro CLI. Please ensure kiro-cli is running.\n\nError: {}",
                        e
                    );
                    let _ = window.emit("message_error", error_msg);
                    return;
                }
            }

            if let Err(e) = client.send_chat_streaming(
                message,
                |chunk| {
                    let _ = window.emit("message_chunk", chunk);
                },
                None,
                None,
            ) {
                error!("Send error: {}", e);
                let error_msg = format!(
                    "Failed to send message. The connection may have been lost.\n\nError: {}",
                    e
                );
                let _ = window.emit("message_error", error_msg);
                return;
            }

            let _ = window.emit("message_complete", ());
        });
    }

    Ok(())
}


/// Send steering content as the first message of a new session.
/// This should only be called for brand new sessions, not when loading existing ones.
/// The steering message is prefixed with STEERING_MSG_PREFIX so the UI can hide it.
#[tauri::command]
pub async fn send_steering_message(
    state: State<'_, AppState>,
    _window: WebviewWindow,
) -> Result<bool, String> {
    let config = state.config.lock().await;
    let assistant = &config.acp.assistant;

    let mut parts: Vec<String> = Vec::new();

    // User steering (takes precedence)
    if let Some(ref path) = assistant.user_steering_path {
        if !path.is_empty() {
            if let Ok(content) = std::fs::read_to_string(path) {
                if !content.trim().is_empty() {
                    parts.push(content);
                }
            }
        }
    }

    // Auto steering
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

    info!("Sending steering message ({} chars)", steering_msg.len());

    let client = state.acp_client.clone();

    async_runtime::spawn_blocking(move || {
        let client = async_runtime::block_on(client.lock());

        if !client.is_connected() {
            error!("Cannot send steering: not connected");
            return;
        }

        if let Err(e) = client.send_chat_streaming(
            steering_msg,
            |_chunk| { /* discard streaming output for steering */ },
            None,
            None,
        ) {
            error!("Failed to send steering message: {}", e);
        } else {
            info!("Steering message sent successfully");
        }
    });

    Ok(true)
}

/// Dismiss any pending permission request by auto-denying it.
/// Called by the floating window before sending a new message when the session
/// has a stalled permission request from the main chat window.
#[tauri::command]
pub async fn dismiss_pending_permission(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    let pending = {
        let guard = state
            .pending_permission
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        guard.clone()
    };

    if let Some(perm) = pending {
        info!(
            "Auto-denying pending permission for tool '{}' (session {})",
            perm.tool_title, perm.session_id
        );

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": perm.request_id,
            "result": {
                "outcome": {
                    "outcome": "selected",
                    "optionId": "reject_once"
                }
            }
        });
        let response_json =
            serde_json::to_string(&response).map_err(|e| format!("Failed to serialize: {}", e))?;

        use std::io::Write;

        let sent = {
            let pipe_guard = state
                .pipe_stdin
                .lock()
                .map_err(|e| format!("Lock: {}", e))?;
            if let Some(ref stdin_arc) = *pipe_guard {
                let stdin_clone = stdin_arc.clone();
                drop(pipe_guard);
                let ok = {
                    if let Ok(mut stdin) = stdin_clone.lock() {
                        let _ = write!(stdin, "{}\n", response_json);
                        let _ = stdin.flush();
                        true
                    } else {
                        false
                    }
                };
                ok
            } else {
                drop(pipe_guard);
                let tcp_guard = state
                    .tcp_writer
                    .lock()
                    .map_err(|e| format!("Lock: {}", e))?;
                if let Some(ref stream) = *tcp_guard {
                    let mut ws = stream
                        .try_clone()
                        .map_err(|e| format!("Clone: {}", e))?;
                    drop(tcp_guard);
                    let _ = write!(ws, "{}\n", response_json);
                    let _ = ws.flush();
                    true
                } else {
                    false
                }
            }
        };

        if sent {
            // Clear the pending permission
            if let Ok(mut guard) = state.pending_permission.lock() {
                *guard = None;
            }
            // Tell the main chat window to dismiss its permission modal
            if let Some(main_window) = app.get_webview_window("main") {
                let _ = main_window.emit("permission_dismissed", ());
            }
            info!("Pending permission auto-denied successfully");
            Ok(true)
        } else {
            Err("Failed to send deny response".to_string())
        }
    } else {
        Ok(false)
    }
}

/// Check if there is a pending (unhandled) permission request.
#[tauri::command]
pub async fn has_pending_permission(
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let guard = state
        .pending_permission
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    Ok(guard.is_some())
}
