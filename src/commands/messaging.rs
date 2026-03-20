use crate::error::{AppError, ErrorKind};
use crate::state::AppState;
use log::{info, warn, error};
use tauri::{async_runtime, Emitter, Manager, State, WebviewWindow};

/// Set up the notification handler on the ACP client.
/// This should be called once after the client is created.
/// The handler dispatches all ACP notifications to the appropriate Tauri events.
#[allow(clippy::too_many_arguments)]
pub fn setup_notification_handler(
    client: &crate::acp_client::AcpClient,
    app: &tauri::AppHandle,
    state_config: std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    pipe_stdin: std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<std::sync::Mutex<std::process::ChildStdin>>>>>,
    tcp_writer: std::sync::Arc<std::sync::Mutex<Option<std::net::TcpStream>>>,
    slash_commands: std::sync::Arc<std::sync::Mutex<Vec<crate::state::SlashCommand>>>,
    pending_permission: std::sync::Arc<std::sync::Mutex<Option<crate::state::PendingPermission>>>,
) {
    let app_handle = app.clone();
    let config = state_config;
    let pipe_stdin = pipe_stdin;
    let tcp_writer = tcp_writer;
    let slash_cmds = slash_commands;
    let pending_perm = pending_permission;
    let accumulated = client.streaming_accumulator.clone();
    let compacting = client.compacting.clone();

    // Map toolCallId → first tool name (e.g. "write") from the initial tool_call update.
    // The permission request arrives later with a descriptive title (e.g. "Creating hello.txt")
    // but we want to track policies by the actual tool name.
    let tool_names: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    // Throttle config saves for last_seen updates — at most once per 60s
    let last_config_save: std::sync::Arc<std::sync::Mutex<std::time::Instant>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(60)));

    client.set_notification_handler(move |notification: serde_json::Value| {
        let method = notification.get("method").and_then(|m| m.as_str()).unwrap_or("");

        if method == "session/request_permission" {
            handle_permission_notification(
                &notification, &app_handle, &config, &pipe_stdin, &tcp_writer, &pending_perm, &tool_names, &last_config_save,
            );
            return;
        }

        if method == "session/update" {
            if let Some(update) = notification.get("params").and_then(|p| p.get("update")) {
                if let Some(kind) = update.get("sessionUpdate").and_then(|v| v.as_str()) {
                    if kind == "agent_message_chunk" {
                        if let Some(text) = update.get("content").and_then(|c| c.get("text")).and_then(|t| t.as_str()) {
                            let mut acc = accumulated.lock().unwrap();
                            if acc.len() < crate::acp_client::MAX_ACCUMULATOR_SIZE {
                                let remaining = crate::acp_client::MAX_ACCUMULATOR_SIZE - acc.len();
                                if text.len() <= remaining {
                                    acc.push_str(text);
                                } else {
                                    acc.push_str(&text[..remaining]);
                                    log::warn!("Streaming accumulator hit {}MB cap — truncating", crate::acp_client::MAX_ACCUMULATOR_SIZE / (1024 * 1024));
                                }
                            }
                            let _ = app_handle.emit("message_chunk", acc.clone());
                        }
                        return;
                    }
                    if kind == "tool_call" || kind == "tool_call_update" {
                        // Track the first title for each toolCallId — that's the real tool name
                        if let (Some(call_id), Some(title)) = (
                            update.get("toolCallId").and_then(|v| v.as_str()),
                            update.get("title").and_then(|v| v.as_str()),
                        ) {
                            if let Ok(mut names) = tool_names.lock() {
                                names.entry(call_id.to_string()).or_insert_with(|| title.to_string());
                            }
                        }
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
            // Emit metadata (context usage) to frontend
            if method == "_kiro.dev/metadata" {
                let _ = app_handle.emit("context_metadata", &notification);
                return;
            }
            // Emit compaction status to frontend
            if method == "_kiro.dev/compaction/status" {
                // Gate outgoing prompts while compaction is in progress
                if let Some(status) = notification.get("params")
                    .and_then(|p| p.get("status"))
                    .and_then(|s| s.get("type"))
                    .and_then(|t| t.as_str())
                {
                    let (lock, cvar) = &*compacting;
                    let mut is_compacting = lock.lock().unwrap();
                    match status {
                        "started" => {
                            info!("Compaction started — gating outgoing prompts");
                            *is_compacting = true;
                        }
                        "completed" => {
                            info!("Compaction completed — releasing prompt gate");
                            *is_compacting = false;
                            cvar.notify_all();
                        }
                        _ => {}
                    }
                }
                let _ = app_handle.emit("compaction_status", &notification);
                return;
            }
            // Rate limit error — emit as a user-visible error
            if method == "_kiro.dev/error/rate_limit" {
                let message = notification.get("params")
                    .and_then(|p| p.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("Rate limit exceeded. Please wait a moment before trying again.");
                warn!("Rate limit hit: {}", message);
                let _ = app_handle.emit("message_error", message);
                return;
            }
            let _ = app_handle.emit("tool_call_update", &notification);
            return;
        }

        info!("Unhandled notification: {}", method);
    });
}
/// Write a JSON string directly to the ACP pipe/TCP transport, bypassing the
/// AcpClient lock. This is used for permission responses and cancellation
/// signals that must be sent while `send_message_streaming` holds the client lock.
///
/// Tries pipe_stdin first; falls back to tcp_writer.
fn write_raw_json(
    pipe_stdin: &std::sync::Mutex<Option<std::sync::Arc<std::sync::Mutex<std::process::ChildStdin>>>>,
    tcp_writer: &std::sync::Mutex<Option<std::net::TcpStream>>,
    json: &str,
) -> Result<(), AppError> {
    use std::io::Write;

    // Try pipe first
    let stdin_arc: Option<std::sync::Arc<std::sync::Mutex<std::process::ChildStdin>>> = {
        let guard = pipe_stdin.lock().map_err(|e| AppError::lock(format!("{}", e)))?;
        guard.as_ref().map(|a| a.clone())
    };
    if let Some(arc) = stdin_arc {
        let mut stdin = arc.lock().map_err(|e| AppError::lock(format!("{}", e)))?;
        writeln!(stdin, "{}", json).map_err(|e| AppError::connection_lost(format!("Write: {}", e)))?;
        stdin.flush().map_err(|e| AppError::connection_lost(format!("Flush: {}", e)))?;
        return Ok(());
    }

    // Fall back to TCP
    let guard = tcp_writer.lock().map_err(|e| AppError::lock(format!("{}", e)))?;
    if let Some(ref stream) = *guard {
        let mut ws: std::net::TcpStream = stream.try_clone().map_err(|e| AppError::connection_lost(format!("Clone: {}", e)))?;
        drop(guard);
        writeln!(ws, "{}", json).map_err(|e| AppError::connection_lost(format!("Write: {}", e)))?;
        ws.flush().map_err(|e| AppError::connection_lost(format!("Flush: {}", e)))?;
        return Ok(());
    }

    Err(AppError::connection_lost("No write handle available"))
}

/// Convenience: serialize a JSON value and write it via `write_raw_json`.
/// Silently ignores errors (used in fire-and-forget contexts like the notification handler).
fn write_raw_json_silent(
    pipe_stdin: &std::sync::Mutex<Option<std::sync::Arc<std::sync::Mutex<std::process::ChildStdin>>>>,
    tcp_writer: &std::sync::Mutex<Option<std::net::TcpStream>>,
    value: &serde_json::Value,
) {
    if let Ok(json) = serde_json::to_string(value) {
        if let Err(e) = write_raw_json(pipe_stdin, tcp_writer, &json) {
            warn!("Failed to write raw JSON to transport: {}", e);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_permission_notification(
    notification: &serde_json::Value,
    app_handle: &tauri::AppHandle,
    config: &std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    pipe_stdin: &std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<std::sync::Mutex<std::process::ChildStdin>>>>>,
    tcp_writer: &std::sync::Arc<std::sync::Mutex<Option<std::net::TcpStream>>>,
    pending_perm: &std::sync::Arc<std::sync::Mutex<Option<crate::state::PendingPermission>>>,
    tool_names: &std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
    last_config_save: &std::sync::Arc<std::sync::Mutex<std::time::Instant>>,
) {
    // The permission request has a descriptive title (e.g. "Creating hello.txt")
    // but we want the actual tool name (e.g. "write") from the first tool_call update.
    let tool_call_id = notification
        .get("params")
        .and_then(|p| p.get("toolCall"))
        .and_then(|tc| tc.get("toolCallId"))
        .and_then(|t| t.as_str())
        .unwrap_or("");

    let fallback_title = notification
        .get("params")
        .and_then(|p| p.get("toolCall"))
        .and_then(|tc| tc.get("title"))
        .and_then(|t| t.as_str())
        .unwrap_or("unknown");

    // Look up the real tool name from the first tool_call update
    let tool_title = if !tool_call_id.is_empty() {
        tool_names.lock().ok()
            .and_then(|names| names.get(tool_call_id).cloned())
            .unwrap_or_else(|| fallback_title.to_string())
    } else {
        fallback_title.to_string()
    };

    let timestamp = chrono::Utc::now().to_rfc3339();
    let mut config_guard = config.lock().unwrap();

    let existing = config_guard.tool_permissions.tools.iter_mut().find(|t| t.title == tool_title);
    if let Some(tool) = existing {
        // Update last_seen in memory — throttle disk writes to at most once per 60s
        tool.last_seen = timestamp;
        let mut last_save = last_config_save.lock().unwrap();
        if last_save.elapsed() >= std::time::Duration::from_secs(60) {
            if let Err(e) = config_guard.save() {
                warn!("Failed to save config (periodic): {}", e);
            }
            *last_save = std::time::Instant::now();
        }
    } else {
        config_guard.tool_permissions.tools.push(crate::config::ToolPolicy {
            title: tool_title.to_string(),
            policy: "ask".to_string(),
            last_seen: timestamp,
        });
        // New tool discovered — save immediately
        if let Err(e) = config_guard.save() {
            warn!("Failed to save config (new tool): {}", e);
        }
        *last_config_save.lock().unwrap() = std::time::Instant::now();
    }

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
            write_raw_json_silent(pipe_stdin, tcp_writer, &response);
        }
    };

    match policy.as_str() {
        "allow" => send_response("allow_once"),
        "deny" => send_response("reject_once"),
        _ => {
            if let Ok(mut pending) = pending_perm.lock() {
                *pending = Some(crate::state::PendingPermission {
                    request_id: notification.get("id").cloned().unwrap_or(serde_json::Value::Null),
                });
            }
            let _ = app_handle.emit("permission_request", serde_json::json!({
                "notification": notification,
                "auto_approve": false,
                "toolName": tool_title
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
        let had_attachments = attachments.as_ref().is_some_and(|a| !a.is_empty());
        if let Err(e) = client.send_chat_streaming_with_recovery(message, attachments) {
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
                            Ok(_) => {
                                client.send_builtin_steering();
                                true
                            }
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
) -> Result<(), AppError> {
    info!("Permission response: {}={}", tool_title, option_id);

    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": { "outcome": { "outcome": "selected", "optionId": option_id } }
    });
    let json = serde_json::to_string(&response).map_err(|e| AppError::new(ErrorKind::SerializeError, format!("{}", e)))?;

    write_raw_json(&state.pipe_stdin, &state.tcp_writer, &json)?;

    if option_id == "allow_always" {
        let mut config = state.config.lock().map_err(|e| AppError::lock(format!("{}", e)))?;
        if let Some(tool) = config.tool_permissions.tools.iter_mut().find(|t| t.title == tool_title) {
            tool.policy = "allow".to_string();
        }
        config.save().map_err(|e| AppError::internal(format!("Save: {}", e)))?;
    }

    if let Ok(mut pending) = state.pending_permission.lock() {
        *pending = None;
    }

    Ok(())
}

#[tauri::command]
pub async fn check_connection(state: State<'_, AppState>) -> Result<bool, AppError> {
    let client = state.acp_client.lock().await;
    Ok(client.is_connected())
}

#[tauri::command]
pub async fn reconnect_acp(state: State<'_, AppState>) -> Result<bool, AppError> {
    let client = state.acp_client.lock().await;
    client.connect().map_err(|e| AppError::connection_lost(format!("Failed to reconnect: {}", e)))?;
    Ok(true)
}

#[tauri::command]
pub async fn cancel_generation(state: State<'_, AppState>) -> Result<(), AppError> {
    // Signal automation plan loop to stop
    state.automation_plan_cancelled.store(true, std::sync::atomic::Ordering::Relaxed);

    let session_id = {
        let client = state.acp_client.try_lock()
            .map_err(|_| "ACP client busy (streaming) — sending cancel directly".to_string());
        match client {
            Ok(c) => c.get_session_id(),
            Err(_) => {
                state.floating_session_id.lock().ok().and_then(|s| s.clone())
            }
        }
    };

    let session_id = session_id.ok_or_else(|| AppError::internal("No active session to cancel"))?;
    info!("Sending session/cancel for session {}", session_id);

    let notification = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    });
    let json = serde_json::to_string(&notification)
        .map_err(|e| AppError::new(ErrorKind::SerializeError, format!("{}", e)))?;

    write_raw_json(&state.pipe_stdin, &state.tcp_writer, &json)?;

    Ok(())
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
    // Route notifications to the chat window while this message is in flight
    if let Ok(mut s) = state.notification_source.lock() {
        *s = "main".to_string();
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
            if let Err(e) = client.send_chat_streaming_with_recovery(message, None) {
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
) -> Result<bool, AppError> {
    let pending = {
        let guard = state.pending_permission.lock().map_err(|e| AppError::lock(format!("{}", e)))?;
        guard.clone()
    };

    if let Some(perm) = pending {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": perm.request_id,
            "result": { "outcome": { "outcome": "selected", "optionId": "reject_once" } }
        });

        // Write directly via pipe/tcp handles to avoid deadlock with send_message_streaming
        write_raw_json_silent(&state.pipe_stdin, &state.tcp_writer, &response);

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
pub async fn has_pending_permission(state: State<'_, AppState>) -> Result<bool, AppError> {
    let guard = state.pending_permission.lock().map_err(|e| AppError::lock(format!("{}", e)))?;
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
    let steering_msg = {
        let config = state.config.lock().map_err(|e| format!("Lock: {}", e))?;
        let parts = crate::commands::system::assemble_steering_parts(&config);
        format!(
            "{} {}",
            crate::commands::system::STEERING_MSG_PREFIX,
            parts.join("\n\n---\n\n")
        )
    };

    let client = state.acp_client.clone();
    async_runtime::spawn_blocking(move || {
        let client = async_runtime::block_on(client.lock());
        if client.is_connected() {
            if let Err(e) = client.send_chat_streaming(&steering_msg, None) {
                warn!("Failed to send auto-steering message: {}", e);
            }
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
    Ok(models.iter().map(|m| serde_json::json!({
        "modelId": m.model_id,
        "name": m.name,
        "description": m.description,
    })).collect())
}

/// Execute an automation plan step by step using sub-agents.
/// Each step is executed in a fresh sub-agent context, keeping the main
/// session clean and avoiding context window bloat.
///
/// The plan is a JSON array of steps, each with "step", "task", and "details" fields.
/// Progress events are emitted to the frontend as each step completes.
#[tauri::command]
pub async fn execute_automation_plan(
    plan_json: String,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<(), String> {
    info!("Executing automation plan");

    // Parse the plan
    let plan: Vec<serde_json::Value> = serde_json::from_str(&plan_json)
        .map_err(|e| format!("Invalid plan JSON: {}", e))?;

    if plan.is_empty() {
        return Err("Empty plan".to_string());
    }

    let total_steps = plan.len();
    info!("Plan has {} steps", total_steps);

    // Emit plan start event
    let _ = window.emit("automation_plan_start", serde_json::json!({
        "totalSteps": total_steps,
        "plan": plan,
    }));

    let client = state.acp_client.clone();
    let cancelled = state.automation_plan_cancelled.clone();

    // Reset cancellation flag at the start
    cancelled.store(false, std::sync::atomic::Ordering::Relaxed);

    async_runtime::spawn_blocking(move || {
        let client = async_runtime::block_on(client.lock());

        if !client.is_connected() {
            if let Err(e) = client.connect() {
                let _ = window.emit("automation_plan_error", format!("Unable to connect: {}", e));
                return;
            }
        }

        for (i, step) in plan.iter().enumerate() {
            // Check cancellation before starting each step
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                info!("Automation plan cancelled by user at step {}", i + 1);
                break;
            }

            let step_num = i + 1;
            let task = step.get("task").and_then(|t| t.as_str()).unwrap_or("Unknown task");
            let details = step.get("details").and_then(|d| d.as_str()).unwrap_or("");

            info!("Executing step {}/{}: {}", step_num, total_steps, task);

            // Emit step start event
            let _ = window.emit("automation_step_start", serde_json::json!({
                "step": step_num,
                "totalSteps": total_steps,
                "task": task,
                "details": details,
            }));

            // Build the sub-agent query with full context
            let query = format!(
                "You are a UI automation sub-agent. Execute this specific task:\n\n\
                 Task: {}\n\
                 Details: {}\n\n\
                 RULES:\n\
                 1. FIRST: Call get_app_steering(task='{}', details='{}') for app-specific tips.\n\
                 2. Use computer-control MCP tools (prefer compound tools like \
                 launch_and_get_tree, click_and_get_tree, click_and_read_result).\n\
                 3. NEVER use screenshot() — use get_ui_tree() or find_elements() instead.\n\
                 4. You MUST call at least one tool. Do NOT claim success without tool evidence.\n\
                 5. Report the ACTUAL tool output. If a tool returns an error, report the error.\n\
                 6. Do NOT fabricate or hallucinate results. Only report what tools actually returned.\n\
                 7. If the task fails, say FAILED and explain why with the actual error message.\n\
                 8. Be concise — just report what happened.",
                task, details, task, details
            );

            // Invoke the sub-agent
            match client.invoke_subagent(&query) {
                Ok(()) => {
                    let result = client.streaming_accumulator.lock().unwrap().clone();
                    info!("Step {}/{} completed: {} chars", step_num, total_steps, result.len());

                    // Check if the sub-agent reported a failure in its response text.
                    // The ACP call succeeded (we got a response), but the agent may
                    // have said "FAILED" because it couldn't actually perform the task.
                    let result_lower = result.to_lowercase();
                    let agent_reported_failure = result_lower.starts_with("failed")
                        || result_lower.contains("\nfailed")
                        || result_lower.contains("failed —")
                        || result_lower.contains("failed -");

                    if agent_reported_failure {
                        warn!("Step {}/{} agent reported failure: {}", step_num, total_steps,
                            &result[..result.len().min(200)]);
                    }

                    let success = !agent_reported_failure;

                    let _ = window.emit("automation_step_complete", serde_json::json!({
                        "step": step_num,
                        "totalSteps": total_steps,
                        "task": task,
                        "result": result,
                        "success": success,
                    }));

                    if !success {
                        warn!("Aborting automation plan: step {}/{} failed", step_num, total_steps);
                        // Mark remaining steps as stopped
                        for j in (i + 1)..plan.len() {
                            let remaining_task = plan[j].get("task")
                                .and_then(|t| t.as_str()).unwrap_or("Unknown task");
                            let _ = window.emit("automation_step_complete", serde_json::json!({
                                "step": j + 1,
                                "totalSteps": total_steps,
                                "task": remaining_task,
                                "result": "Skipped due to earlier step failure",
                                "success": false,
                                "stopped": true,
                            }));
                        }
                        break;
                    }
                }
                Err(e) => {
                    let error_msg = format!("{}", e);
                    warn!("Step {}/{} failed: {}", step_num, total_steps, error_msg);

                    let _ = window.emit("automation_step_complete", serde_json::json!({
                        "step": step_num,
                        "totalSteps": total_steps,
                        "task": task,
                        "result": error_msg,
                        "success": false,
                    }));

                    // Abort on transport/protocol errors too
                    warn!("Aborting automation plan: step {}/{} errored", step_num, total_steps);
                    for j in (i + 1)..plan.len() {
                        let remaining_task = plan[j].get("task")
                            .and_then(|t| t.as_str()).unwrap_or("Unknown task");
                        let _ = window.emit("automation_step_complete", serde_json::json!({
                            "step": j + 1,
                            "totalSteps": total_steps,
                            "task": remaining_task,
                            "result": "Skipped due to earlier step failure",
                            "success": false,
                            "stopped": true,
                        }));
                    }
                    break;
                }
            }
        }

        // Emit plan complete event
        let _ = window.emit("automation_plan_complete", serde_json::json!({
            "totalSteps": total_steps,
        }));

        let _ = window.emit("message_complete", ());
    });

    Ok(())
}

/// Receive the result of a local extension tool call from the webview,
/// and send it back to the ACP agent as a follow-up message so the LLM
/// can continue its response with the data.
#[tauri::command]
pub async fn extension_tool_response(
    extension_id: String,
    tool_name: String,
    result_json: String,
    success: bool,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<(), String> {
    info!(
        "Extension tool response: ext={}, tool={}, success={}, len={}",
        extension_id, tool_name, success, result_json.len()
    );

    let client = state.acp_client.clone();

    async_runtime::spawn_blocking(move || {
        let client = async_runtime::block_on(client.lock());

        if !client.is_connected() {
            let _ = window.emit("message_error", "Not connected to agent".to_string());
            return;
        }

        // Build a message that the LLM will see as the tool result
        let content = if success {
            format!(
                "[Extension tool result: {}/{}]\n{}",
                extension_id, tool_name, result_json
            )
        } else {
            format!(
                "[Extension tool error: {}/{}]\n{}",
                extension_id, tool_name, result_json
            )
        };

        // Send as a follow-up user message so the agent continues
        if let Err(e) = client.send_chat_streaming(&content, None) {
            let _ = window.emit("message_error", format!("Failed to send tool result: {}", e));
        }

        // Emit message_complete so the frontend knows the follow-up response is done
        let _ = window.emit("message_complete", ());
    });

    Ok(())
}

/// Send extension tool definitions to the agent as a hidden steering message.
/// Called by the frontend after extensions are loaded, so the agent knows
/// which local extension tools are available.
#[tauri::command]
pub async fn send_extension_tool_steering(
    tool_steering: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if tool_steering.trim().is_empty() {
        return Ok(());
    }

    // Deduplicate: skip if the steering content hasn't changed since last send
    let hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        tool_steering.hash(&mut hasher);
        hasher.finish()
    };
    {
        let mut last = state.last_tool_steering_hash.lock().unwrap();
        if *last == hash {
            return Ok(());
        }
        *last = hash;
    }

    info!("Sending extension tool steering ({} chars)", tool_steering.len());

    let client = state.acp_client.clone();

    async_runtime::spawn_blocking(move || {
        let client = async_runtime::block_on(client.lock());
        if !client.is_connected() {
            return;
        }

        let msg = format!(
            "{} {}\n\n---\n\n<instructions>Respond with only \"ack\" to confirm receipt. Do not summarize or comment on the content above.</instructions>",
            crate::commands::system::STEERING_MSG_PREFIX,
            tool_steering
        );

        match client.send_chat_streaming(&msg, None) {
            Ok(_) => info!("Extension tool steering sent"),
            Err(e) => warn!("Failed to send extension tool steering: {}", e),
        }
    });

    Ok(())
}

/// Check the permission policy for an extension tool call.
/// Registers the tool in the config if not seen before (defaults to "ask").
/// Returns the policy: "allow", "deny", or "ask".
#[tauri::command]
pub async fn check_extension_tool_permission(
    extension_id: String,
    tool_name: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let tool_title = format!("ext:{}/{}", extension_id, tool_name);
    let mut config = state.config.lock().map_err(|e| format!("Lock: {}", e))?;

    // Check trust_all first
    if config.tool_permissions.trust_all {
        // Still register the tool so it shows up in settings
        let timestamp = chrono::Utc::now().to_rfc3339();
        if !config.tool_permissions.tools.iter().any(|t| t.title == tool_title) {
            config.tool_permissions.tools.push(crate::config::ToolPolicy {
                title: tool_title,
                policy: "allow".to_string(),
                last_seen: timestamp,
            });
            let _ = config.save();
        }
        return Ok("allow".to_string());
    }

    let timestamp = chrono::Utc::now().to_rfc3339();
    let existing = config.tool_permissions.tools.iter_mut().find(|t| t.title == tool_title);

    if let Some(tool) = existing {
        tool.last_seen = timestamp;
        let policy = tool.policy.clone();
        if let Err(e) = config.save() {
            warn!("Failed to save config (tool policy lookup): {}", e);
        }
        Ok(policy)
    } else {
        // First time seeing this tool — register with "ask" policy
        config.tool_permissions.tools.push(crate::config::ToolPolicy {
            title: tool_title,
            policy: "ask".to_string(),
            last_seen: timestamp,
        });
        if let Err(e) = config.save() {
            warn!("Failed to save config (new tool registration): {}", e);
        }
        Ok("ask".to_string())
    }
}

// ---------------------------------------------------------------------------
// Inline Assist messaging
// ---------------------------------------------------------------------------

/// Send a message for inline assist and stream the response to the inline-assist window.
#[tauri::command]
pub async fn send_inline_assist(
    message: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let client = state.acp_client.clone();

    async_runtime::spawn_blocking(move || {
        let client = async_runtime::block_on(client.lock());

        if !client.is_connected() {
            if let Err(e) = client.connect() {
                let _ = app.emit("inline_assist_error", format!("Unable to connect: {}", e));
                return;
            }
        }

        // Clear the accumulator so we can track this response
        client.streaming_accumulator.lock().unwrap().clear();

        if let Err(e) = client.send_chat_streaming(&message, None) {
            let _ = app.emit("inline_assist_error", format!("Failed: {}", e));
            return;
        }

        // The response is accumulated by the notification handler.
        // Read the final result from the accumulator.
        let result = client.streaming_accumulator.lock().unwrap().clone();
        if result.trim().is_empty() {
            let _ = app.emit("inline_assist_error", "Empty response");
        } else {
            let _ = app.emit("inline_assist_chunk", &result);
            let _ = app.emit("inline_assist_complete", ());
        }
    });

    Ok(())
}
