use crate::error::AppError;
use crate::lock_ext::LockExt;
use crate::state::{AcpHandles, FeatureServices, UiState};
use log::{info, warn, error};
use tauri::{async_runtime, Emitter, Manager, State, WebviewWindow};

/// Set up the notification handler on the ACP client.
/// This should be called once after the client is created.
/// The handler dispatches all ACP notifications to the appropriate Tauri events.
pub fn setup_notification_handler(
    client: std::sync::Arc<crate::acp_client::AcpClient>,
    app: &tauri::AppHandle,
    state_config: std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    slash_commands: std::sync::Arc<std::sync::Mutex<Vec<crate::state::SlashCommand>>>,
    pending_permission: std::sync::Arc<std::sync::Mutex<Option<crate::state::PendingPermission>>>,
) {
    let app_handle = app.clone();
    let config = state_config;
    let slash_cmds = slash_commands;
    let pending_perm = pending_permission;
    let compacting = client.compacting.clone();
    let client_for_handler = client.clone();

    // Map toolCallId → first tool name (e.g. "write") from the initial tool_call update.
    // The permission request arrives later with a descriptive title (e.g. "Creating hello.txt")
    // but we want to track policies by the actual tool name.
    let tool_names: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    // Per-session pending deltas waiting to be flushed to the UI as
    // batched message_chunk events. The notification handler appends here;
    // a dedicated thread below drains the map every CHUNK_FLUSH_INTERVAL_MS
    // and emits one event per non-empty bucket. With token-level streaming
    // we used to fire one Tauri emit per token (hundreds-to-thousands per
    // response, each costing a JSON serialize + IPC bridge + frontend
    // handler invocation), and the WebView2 emitter has no backpressure —
    // bursts pile up in Tauri's internal queue. Coalescing into ~60 fps
    // batches drops the IPC roundtrip count by 1-2 orders of magnitude
    // without changing the on-screen feel of streaming.
    let pending_chunks: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    spawn_chunk_flush_thread(app_handle.clone(), pending_chunks.clone());

    // Throttle config saves for last_seen updates — at most once per 60s
    let last_config_save: std::sync::Arc<std::sync::Mutex<std::time::Instant>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(60)));

    client.set_notification_handler(move |notification: serde_json::Value| {
        let method = notification.get("method").and_then(|m| m.as_str()).unwrap_or("");

        if method == "session/request_permission" {
            handle_permission_notification(
                &notification, &app_handle, &config, &client_for_handler, &pending_perm, &tool_names, &last_config_save,
            );
            return;
        }

        if method == "session/update" {
            // Every session/update carries the session id it belongs to.
            // Chunks for a session that isn't currently active are dropped
            // from the UI emit — otherwise switching sessions mid-stream
            // (or auto_steering's hidden prompt overlapping with the user's
            // prompt) would leak the wrong tokens into the wrong UI bucket.
            // Server-issued updates may legitimately omit sessionId for
            // some kinds; in that case fall back to the current session.
            let update_session_id = notification
                .get("params")
                .and_then(|p| p.get("sessionId"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
            let current_session_id = client_for_handler.get_session_id();
            let is_current = match (&update_session_id, &current_session_id) {
                (Some(u), Some(c)) => u == c,
                _ => true, // missing on either side — be permissive, see comment above
            };

            if let Some(update) = notification.get("params").and_then(|p| p.get("update")) {
                if let Some(kind) = update.get("sessionUpdate").and_then(|v| v.as_str()) {
                    if kind == "agent_message_chunk" {
                        if let Some(text) = update.get("content").and_then(|c| c.get("text")).and_then(|t| t.as_str()) {
                            // Always accumulate into the *update's* session
                            // bucket — that's the source of truth and helpers
                            // like auto_steering read by the session id they
                            // sent to. The current-session check below only
                            // gates the UI emit.
                            let acc_session = update_session_id
                                .clone()
                                .or_else(|| current_session_id.clone());
                            let emitted_owned: Option<String> = if let Some(sid) = &acc_session {
                                client_for_handler
                                    .accumulate_chunk(sid, text)
                                    .map(|s| s.to_string())
                            } else {
                                None
                            };

                            if is_current {
                                if let Some(emitted) = emitted_owned {
                                    if !emitted.is_empty() {
                                        // Append to the per-session pending
                                        // buffer. The flush thread emits
                                        // `message_chunk` events with this
                                        // text every CHUNK_FLUSH_INTERVAL_MS,
                                        // one per non-empty session bucket.
                                        // The frontend appends `text` on
                                        // each chunk to its local
                                        // accumulator; sending a longer
                                        // string per emit is strictly
                                        // better than one-emit-per-token.
                                        if let Some(sid) = acc_session {
                                            if let Ok(mut map) = pending_chunks.lock() {
                                                map.entry(sid).or_default().push_str(&emitted);
                                            }
                                        }
                                    }
                                }
                            } else {
                                log::debug!(
                                    "dropping message_chunk for non-current session {:?} (current is {:?})",
                                    update_session_id, current_session_id
                                );
                            }
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
                        // Only forward tool-call updates for the active session.
                        // A tool call belonging to a backgrounded session would
                        // confuse the active session's UI (the floating window's
                        // tool indicator would show another session's progress).
                        if is_current {
                            let _ = app_handle.emit("tool_call_update", &notification);
                        }
                        return;
                    }
                }
            }
            return;
        }

        if method == "_kage.dev/commands/available" {
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

        if method.starts_with("_kage.dev/") {
            // Emit metadata (context usage) to frontend
            if method == "_kage.dev/metadata" {
                let _ = app_handle.emit("context_metadata", &notification);
                return;
            }
            // Emit compaction status to frontend
            if method == "_kage.dev/compaction/status" {
                // Gate outgoing prompts while compaction is in progress
                if let Some(status) = notification.get("params")
                    .and_then(|p| p.get("status"))
                    .and_then(|s| s.get("type"))
                    .and_then(|t| t.as_str())
                {
                    let (lock, cvar) = &*compacting;
                    let mut is_compacting = lock.lock_or_recover();
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
            if method == "_kage.dev/error/rate_limit" {
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

/// How often the chunk-flush thread wakes up to drain the pending-chunks
/// map and emit batched `message_chunk` events. ~60 fps — slower than
/// human chunk-perception, fast enough that streaming text doesn't feel
/// stuttery. Anything below 10ms invites IPC backpressure with no
/// user-visible benefit; above ~33ms (30 fps) the streaming starts to
/// feel laggy.
const CHUNK_FLUSH_INTERVAL_MS: u64 = 16;

/// Background thread that drains `pending_chunks` every
/// CHUNK_FLUSH_INTERVAL_MS and emits one `message_chunk` event per non-
/// empty session bucket. Replaces the pre-fix one-emit-per-token path,
/// which fired hundreds-to-thousands of IPC roundtrips per response.
///
/// The thread runs for the AcpClient's lifetime — it's a single OS thread
/// (`acp-chunk-flush`) doing a HashMap drain + 0..N emits per cycle, so
/// the always-on cost is negligible. Exit is by `app_handle.emit` returning
/// an error after the app shuts down; we log and break.
fn spawn_chunk_flush_thread(
    app_handle: tauri::AppHandle,
    pending: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
) {
    let _ = std::thread::Builder::new()
        .name("acp-chunk-flush".into())
        .spawn(move || {
            let interval = std::time::Duration::from_millis(CHUNK_FLUSH_INTERVAL_MS);
            loop {
                std::thread::sleep(interval);
                let alive = crate::chunk_batcher::drain_and_emit_pending(&pending, |session_id, text| {
                    let payload = serde_json::json!({
                        "text": text,
                        "sessionId": session_id,
                    });
                    app_handle
                        .emit("message_chunk", payload)
                        .map_err(|e| format!("{}", e))
                });
                if !alive {
                    return;
                }
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn handle_permission_notification(
    notification: &serde_json::Value,
    app_handle: &tauri::AppHandle,
    config: &std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    client: &crate::acp_client::AcpClient,
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
    let mut config_guard = config.lock_or_recover();

    let existing = config_guard.tool_permissions.tools.iter_mut().find(|t| t.title == tool_title);
    if let Some(tool) = existing {
        // Update last_seen in memory — throttle disk writes to at most once per 60s
        tool.last_seen = timestamp;
        let mut last_save = last_config_save.lock_or_recover();
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
            granted_at: String::new(),
            grant_type: "once".to_string(),
        });
        // New tool discovered — save immediately
        if let Err(e) = config_guard.save() {
            warn!("Failed to save config (new tool): {}", e);
        }
        *last_config_save.lock_or_recover() = std::time::Instant::now();
    }

    let policy = if config_guard.tool_permissions.terminator_mode || config_guard.tool_permissions.trust_all {
        "allow".to_string()
    } else {
        config_guard.tool_permissions.tools.iter()
            .find(|t| t.title == tool_title)
            .map(|t| t.effective_policy().to_string())
            .unwrap_or_else(|| "ask".to_string())
    };
    drop(config_guard);

    let send_response = |option_id: &str| {
        if let Some(request_id) = notification.get("id") {
            if let Err(e) = client.send_permission_response(request_id, option_id) {
                warn!("Failed to send auto permission response: {}", e);
            }
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

            // Determine which window originated this conversation
            let source = app_handle.try_state::<UiState>()
                .and_then(|s| s.notification_source.lock().ok().map(|s| s.clone()))
                .unwrap_or_else(|| "floating".to_string());

            let payload = serde_json::json!({
                "notification": notification,
                "auto_approve": false,
                "toolName": tool_title,
                "source": source,
            });

            // Broadcast to all windows with source info — each window decides whether to show
            let _ = app_handle.emit("permission_request", &payload);

            // If originated from floating and it's hidden, show it (case 3: background permission)
            if source == "floating" {
                if let Some(floating) = app_handle.get_webview_window("floating") {
                    let _ = floating.show();
                    let _ = floating.set_focus();
                }
            }
        }
    }
}

// --- Tauri Commands ---

#[tauri::command]
pub async fn send_message_streaming(
    message: String,
    attachments: Option<Vec<serde_json::Value>>,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    window: WebviewWindow,
) -> Result<(), String> {
    info!("Sending message: {}", message);
    let client = acp.client.clone();
    let config = features.config.clone();
    let window_clone = window.clone();

    async_runtime::spawn_blocking(move || {
        let client_arc = client.clone();
        let config_arc = config.clone();

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

        // Trigger auto-steering generation periodically. No lock to drop —
        // the client is just an Arc<AcpClient> now.
        crate::auto_steering::maybe_generate_steering(client_arc, config_arc);
    });

    Ok(())
}

#[tauri::command]
pub async fn send_permission_response(
    request_id: serde_json::Value,
    option_id: String,
    tool_title: String,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    info!("Permission response: {}={}", tool_title, option_id);

    acp.client.send_permission_response(&request_id, &option_id)
        .map_err(|e| AppError::connection_lost(format!("Permission response failed: {}", e)))?;

    if option_id == "allow_always" {
        let mut config = features.config.lock().map_err(|e| AppError::lock(format!("{}", e)))?;
        if let Some(tool) = config.tool_permissions.tools.iter_mut().find(|t| t.title == tool_title) {
            tool.policy = "allow".to_string();
        }
        config.save().map_err(|e| AppError::internal(format!("Save: {}", e)))?;
    }

    // Audit log: record the user's decision. We classify option_id into
    // our event kinds; unrecognised strings are recorded as Denied with
    // the raw option_id in the tool field so the UI still shows them.
    let session_id = acp.client.get_session_id();
    let audit_event = match option_id.as_str() {
        "allow_once" => crate::permission_audit::AuditEvent::Granted {
            tool: tool_title.clone(),
            grant_type: "once".to_string(),
            session_id,
            args_preview: None,
        },
        "allow_24h" => crate::permission_audit::AuditEvent::Granted {
            tool: tool_title.clone(),
            grant_type: "24h".to_string(),
            session_id,
            args_preview: None,
        },
        "allow_always" => crate::permission_audit::AuditEvent::Granted {
            tool: tool_title.clone(),
            grant_type: "always".to_string(),
            session_id,
            args_preview: None,
        },
        _ => crate::permission_audit::AuditEvent::Denied {
            tool: tool_title.clone(),
            session_id,
        },
    };
    crate::permission_audit::append(
        &crate::permission_audit::AuditEntry::now(audit_event),
    );

    if let Ok(mut pending) = acp.pending_permission.lock() {
        *pending = None;
    }

    // Broadcast dismissal to all windows so they close their permission modals
    let _ = app.emit("permission_dismissed", ());

    Ok(())
}

#[tauri::command]
pub async fn check_connection(acp: State<'_, AcpHandles>) -> Result<bool, AppError> {
    Ok(acp.client.is_connected())
}

#[tauri::command]
pub async fn reconnect_acp(acp: State<'_, AcpHandles>) -> Result<bool, AppError> {
    acp.client.connect().map_err(|e| AppError::connection_lost(format!("Failed to reconnect: {}", e)))?;
    Ok(true)
}

#[tauri::command]
pub async fn cancel_generation(
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    // Signal automation plan loop to stop
    features.automation_plan_cancelled.store(true, std::sync::atomic::Ordering::Relaxed);

    // Direct read — the AcpClient is no longer wrapped in an outer mutex,
    // so this never has to fall back to guessing the floating session id.
    let session_id = acp.client.get_session_id()
        .ok_or_else(|| AppError::internal("No active session to cancel"))?;

    acp.client.cancel_session(&session_id)
        .map_err(|e| AppError::connection_lost(format!("Cancel failed: {}", e)))?;

    Ok(())
}

#[tauri::command]
pub async fn open_chat_with_message(
    message: String,
    acp: State<'_, AcpHandles>,
    ui: State<'_, UiState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if let Some(floating) = app.get_webview_window("floating") {
        let _ = floating.hide();
    }
    // Route notifications to the chat window while this message is in flight
    if let Ok(mut s) = ui.notification_source.lock() {
        *s = "main".to_string();
    }
    if let Some(main) = app.get_webview_window("main") {
        // Center on the active monitor
        crate::commands::window::center_window_on_active_monitor(&main);
        let _ = main.show();
        let _ = main.set_focus();
        let _ = main.emit("initial_message", message.clone());

        let client = acp.client.clone();
        let window = main.clone();

        async_runtime::spawn_blocking(move || {
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
    acp: State<'_, AcpHandles>,
    app: tauri::AppHandle,
) -> Result<bool, AppError> {
    let pending = {
        let guard = acp.pending_permission.lock().map_err(|e| AppError::lock(format!("{}", e)))?;
        guard.clone()
    };

    if let Some(perm) = pending {
        if let Err(e) = acp.client.send_permission_response(&perm.request_id, "reject_once") {
            warn!("Failed to dismiss pending permission: {}", e);
        }

        if let Ok(mut guard) = acp.pending_permission.lock() {
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
pub async fn has_pending_permission(acp: State<'_, AcpHandles>) -> Result<bool, AppError> {
    let guard = acp.pending_permission.lock().map_err(|e| AppError::lock(format!("{}", e)))?;
    Ok(guard.is_some())
}

#[tauri::command]
pub async fn get_slash_commands(acp: State<'_, AcpHandles>) -> Result<Vec<crate::state::SlashCommand>, String> {
    let cmds = acp.slash_commands.lock().map_err(|e| format!("Lock: {}", e))?;
    Ok(cmds.clone())
}

#[tauri::command]
pub async fn execute_slash_command(
    command: String,
    args: Option<serde_json::Value>,
    acp: State<'_, AcpHandles>,
    window: WebviewWindow,
) -> Result<serde_json::Value, String> {
    let client = acp.client.clone();

    let result = async_runtime::spawn_blocking(move || {
        if !client.is_connected() {
            return Err("Not connected".to_string());
        }
        let session_id = client.get_session_id().ok_or("No active session")?;
        let cmd_name = command.strip_prefix('/').unwrap_or(&command);

        let response = client.send_request(
            "_kage.dev/commands/execute",
            serde_json::json!({
                "sessionId": session_id,
                "command": { "command": cmd_name, "args": args.unwrap_or(serde_json::json!({})) }
            }),
        ).map_err(|e| format!("Command failed: {}", e))?;
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
) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({ "options": [] }))
}

#[tauri::command]
pub async fn send_steering_message(
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
) -> Result<bool, String> {
    let steering_msg = {
        let config = features.config.lock().map_err(|e| format!("Lock: {}", e))?;
        let parts = crate::commands::system::assemble_steering_parts(&config);
        format!(
            "{} {}",
            crate::commands::system::STEERING_MSG_PREFIX,
            parts.join("\n\n---\n\n")
        )
    };

    let client = acp.client.clone();
    async_runtime::spawn_blocking(move || {
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
    acp: State<'_, AcpHandles>,
) -> Result<Vec<serde_json::Value>, String> {
    let models = acp.available_models.lock()
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
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
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

    let client = acp.client.clone();
    let cancelled = features.automation_plan_cancelled.clone();

    // Reset cancellation flag at the start
    cancelled.store(false, std::sync::atomic::Ordering::Relaxed);

    async_runtime::spawn_blocking(move || {
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
                Ok(result) => {
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
    acp: State<'_, AcpHandles>,
    window: WebviewWindow,
) -> Result<(), String> {
    info!(
        "Extension tool response: ext={}, tool={}, success={}, len={}",
        extension_id, tool_name, success, result_json.len()
    );

    let client = acp.client.clone();

    async_runtime::spawn_blocking(move || {
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
    acp: State<'_, AcpHandles>,
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
        let mut last = acp.last_tool_steering_hash.lock_or_recover();
        if *last == hash {
            return Ok(());
        }
        *last = hash;
    }

    info!("Sending extension tool steering ({} chars)", tool_steering.len());

    let client = acp.client.clone();

    async_runtime::spawn_blocking(move || {
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
    features: State<'_, FeatureServices>,
) -> Result<String, String> {
    let tool_title = format!("ext:{}/{}", extension_id, tool_name);
    let mut config = features.config.lock().map_err(|e| format!("Lock: {}", e))?;

    // Check trust_all first
    if config.tool_permissions.trust_all {
        // Still register the tool so it shows up in settings
        let timestamp = chrono::Utc::now().to_rfc3339();
        if !config.tool_permissions.tools.iter().any(|t| t.title == tool_title) {
            config.tool_permissions.tools.push(crate::config::ToolPolicy {
                title: tool_title,
                policy: "allow".to_string(),
                last_seen: timestamp.clone(),
                granted_at: timestamp.clone(),
                grant_type: "always".to_string(),
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
            granted_at: String::new(),
            grant_type: "once".to_string(),
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
    acp: State<'_, AcpHandles>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let client = acp.client.clone();

    async_runtime::spawn_blocking(move || {
        if !client.is_connected() {
            if let Err(e) = client.connect() {
                let _ = app.emit("inline_assist_error", format!("Unable to connect: {}", e));
                return;
            }
        }

        // send_chat_streaming resets its own session bucket; once it
        // returns, the response is available in that bucket.
        if let Err(e) = client.send_chat_streaming(&message, None) {
            let _ = app.emit("inline_assist_error", format!("Failed: {}", e));
            return;
        }

        // Read the final result by the session id this prompt landed on.
        let session_id = match client.get_session_id() {
            Some(id) => id,
            None => {
                let _ = app.emit("inline_assist_error", "No active session after send");
                return;
            }
        };
        let result = client.take_session_accumulator(&session_id);
        if result.trim().is_empty() {
            let _ = app.emit("inline_assist_error", "Empty response");
        } else {
            let _ = app.emit("inline_assist_chunk", &result);
            let _ = app.emit("inline_assist_complete", ());
        }
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Macro execution — chained transformation steps (AI, regex, transform, script)
// ---------------------------------------------------------------------------

/// Execute a macro: run each step sequentially, feeding output into the next.
/// Steps can be AI prompts, find/replace, built-in transforms, or JS scripts.
/// Returns the final result text.
#[tauri::command]
pub async fn execute_macro(
    steps: Vec<serde_json::Value>,
    initial_input: String,
    acp: State<'_, AcpHandles>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let client = acp.client.clone();

    let result = async_runtime::spawn_blocking(move || {
        let mut current_input = initial_input;

        for (i, step) in steps.iter().enumerate() {
            let step_type = step.get("step_type").and_then(|v| v.as_str()).unwrap_or("ai_prompt");

            // Emit progress
            let _ = app.emit("macro_progress", serde_json::json!({
                "step": i + 1,
                "total": steps.len(),
                "type": step_type,
            }));

            match step_type {
                "ai_prompt" => {
                    let prompt_template = step.get("prompt").and_then(|v| v.as_str()).unwrap_or("{input}");
                    let prompt = prompt_template.replace("{input}", &current_input);
                    let full_prompt = format!(
                        "{}\n\n[_KAGE_INLINE] Return ONLY the result text. No explanations, no markdown formatting, no code fences.",
                        prompt
                    );

                    if !client.is_connected() {
                        if let Err(e) = client.connect() {
                            return Err(format!("Step {}: Unable to connect: {}", i + 1, e));
                        }
                    }
                    if let Err(e) = client.send_chat_streaming(&full_prompt, None) {
                        return Err(format!("Step {} failed: {}", i + 1, e));
                    }
                    let session_id = client.get_session_id()
                        .ok_or_else(|| format!("Step {}: no active session", i + 1))?;
                    let result = client.take_session_accumulator(&session_id);
                    if result.trim().is_empty() {
                        return Err(format!("Step {} returned empty result", i + 1));
                    }
                    current_input = result.trim().to_string();
                }

                "find_replace" => {
                    let find = step.get("find").and_then(|v| v.as_str()).unwrap_or("");
                    let replace = step.get("replace").and_then(|v| v.as_str()).unwrap_or("");
                    if !find.is_empty() {
                        match regex::Regex::new(find) {
                            Ok(re) => {
                                current_input = re.replace_all(&current_input, replace).to_string();
                            }
                            Err(e) => {
                                return Err(format!("Step {}: Invalid regex '{}': {}", i + 1, find, e));
                            }
                        }
                    }
                }

                "transform" => {
                    let transform = step.get("transform").and_then(|v| v.as_str()).unwrap_or("");
                    current_input = apply_transform(transform, &current_input);
                }

                other => {
                    return Err(format!("Step {}: Unknown step type '{}'", i + 1, other));
                }
            }

            info!("Macro step {}/{} ({}) complete: {} chars", i + 1, steps.len(), step_type, current_input.len());
        }

        Ok(current_input)
    }).await.map_err(|e| format!("Task error: {}", e))?;

    result
}

/// Apply a built-in text transform.
fn apply_transform(name: &str, input: &str) -> String {
    match name {
        "uppercase" => input.to_uppercase(),
        "lowercase" => input.to_lowercase(),
        "trim" => input.trim().to_string(),
        "sort_lines" => {
            let mut lines: Vec<&str> = input.lines().collect();
            lines.sort();
            lines.join("\n")
        }
        "reverse" => input.chars().rev().collect(),
        "reverse_lines" => {
            let lines: Vec<&str> = input.lines().collect();
            lines.into_iter().rev().collect::<Vec<_>>().join("\n")
        }
        "remove_blank_lines" => {
            input.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
        }
        "count_words" => {
            let count = input.split_whitespace().count();
            format!("{} words", count)
        }
        "count_lines" => {
            let count = input.lines().count();
            format!("{} lines", count)
        }
        "count_chars" => {
            format!("{} characters", input.len())
        }
        "base64_encode" => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(input.as_bytes())
        }
        "base64_decode" => {
            use base64::Engine;
            match base64::engine::general_purpose::STANDARD.decode(input.trim()) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                Err(e) => format!("Base64 decode error: {}", e),
            }
        }
        "unique_lines" => {
            let mut seen = std::collections::HashSet::new();
            input.lines().filter(|l| seen.insert(*l)).collect::<Vec<_>>().join("\n")
        }
        "number_lines" => {
            input.lines().enumerate().map(|(i, l)| format!("{:>4}  {}", i + 1, l)).collect::<Vec<_>>().join("\n")
        }
        _ => input.to_string(),
    }
}
