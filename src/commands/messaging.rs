use crate::error::AppError;
use crate::lock_ext::LockExt;
use crate::state::{AcpHandles, FeatureServices, UiState};
use log::{error, info, warn};
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
    let pending_chunks: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, String>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    spawn_chunk_flush_thread(app_handle.clone(), pending_chunks.clone());

    // Throttle config saves for last_seen updates — at most once per 60s
    let last_config_save: std::sync::Arc<std::sync::Mutex<std::time::Instant>> =
        std::sync::Arc::new(std::sync::Mutex::new(
            std::time::Instant::now() - std::time::Duration::from_secs(60),
        ));

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
            // We forward chunks and tool_call events to *all* windows
            // tagged with the session id; each window's frontend filters
            // by `acceptSessionId`, so a window only renders updates for
            // its own pinned session. Backgrounded sessions still
            // accumulate their bytes into the per-session bucket (used
            // by auto_steering and sub-agent reads) but their windows
            // see no chunks because no window has them pinned.
            let update_session_id = notification
                .get("params")
                .and_then(|p| p.get("sessionId"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());

            if let Some(update) = notification.get("params").and_then(|p| p.get("update")) {
                if let Some(kind) = update.get("sessionUpdate").and_then(|v| v.as_str()) {
                    if kind == "agent_message_chunk" {
                        if let Some(text) = update.get("content").and_then(|c| c.get("text")).and_then(|t| t.as_str()) {
                            let Some(sid) = update_session_id else {
                                // No session id on the chunk — can't route. Drop.
                                return;
                            };
                            let emitted_owned: Option<String> = client_for_handler
                                .accumulate_chunk(&sid, text)
                                .map(|s| s.to_string());

                            if let Some(emitted) = emitted_owned {
                                if !emitted.is_empty() {
                                    // Append to the per-session pending
                                    // buffer. The flush thread emits
                                    // `message_chunk` events with this
                                    // text every CHUNK_FLUSH_INTERVAL_MS,
                                    // one per non-empty session bucket.
                                    // Frontend filters by sessionId.
                                    if let Ok(mut map) = pending_chunks.lock() {
                                        map.entry(sid).or_default().push_str(&emitted);
                                    }
                                }
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
                        // Forward unconditionally; frontend filters by
                        // sessionId in the payload.
                        let _ = app_handle.emit("tool_call_update", &notification);
                        return;
                    }
                }
            }
            return;
        }

        // Vendor extension dispatch. Two ACP vendor namespaces are
        // recognised — `_kage.dev/*` and `_kiro.dev/*` — with an
        // identical extension surface. Match by suffix and pin the
        // agent's preferred prefix on the AcpClient (used by outgoing
        // requests). See `acp_client::vendor_method_suffix`.
        if let Some(suffix) = crate::acp_client::vendor_method_suffix(method) {
            client_for_handler.observe_vendor_prefix(method);
            match suffix {
                "commands/available" => {
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
                }
                "metadata" => {
                    let _ = app_handle.emit("context_metadata", &notification);
                }
                "compaction/status" => {
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
                }
                "error/rate_limit" => {
                    let message = notification.get("params")
                        .and_then(|p| p.get("message"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("Rate limit exceeded. Please wait a moment before trying again.");
                    warn!("Rate limit hit: {}", message);
                    let _ = app_handle.emit("message_error", message);
                }
                _ => {
                    // Unknown vendor extension — forward to the frontend
                    // as a generic tool_call_update, mirroring previous
                    // behaviour.
                    let _ = app_handle.emit("tool_call_update", &notification);
                }
            }
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
                let alive =
                    crate::chunk_batcher::drain_and_emit_pending(&pending, |session_id, text| {
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
        tool_names
            .lock()
            .ok()
            .and_then(|names| names.get(tool_call_id).cloned())
            .unwrap_or_else(|| fallback_title.to_string())
    } else {
        fallback_title.to_string()
    };

    let timestamp = chrono::Utc::now().to_rfc3339();
    let mut config_guard = config.lock_or_recover();

    let existing = config_guard
        .tool_permissions
        .tools
        .iter_mut()
        .find(|t| t.title == tool_title);
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
        config_guard
            .tool_permissions
            .tools
            .push(crate::config::ToolPolicy {
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

    let policy = if config_guard.tool_permissions.terminator_mode
        || config_guard.tool_permissions.trust_all
    {
        "allow".to_string()
    } else {
        config_guard
            .tool_permissions
            .tools
            .iter()
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
                    request_id: notification
                        .get("id")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                });
            }

            // Route the modal back to the window that issued the
            // prompt this permission belongs to. The session id arrives
            // on every permission notification; the originator map was
            // written by `send_message_streaming` before the ACP call.
            // Falling back to "floating" preserves the historical
            // default for hotkey-driven prompts that bypass the map
            // (e.g. inline-assist).
            let session_id = notification
                .get("params")
                .and_then(|p| p.get("sessionId"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
            let source = app_handle
                .try_state::<UiState>()
                .and_then(|state| {
                    let sid = session_id.as_deref()?;
                    state
                        .pending_prompt_originators
                        .lock()
                        .ok()
                        .and_then(|m| m.get(sid).cloned())
                })
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
#[allow(clippy::too_many_arguments)] // Tauri commands take state via parameters; can't be condensed.
pub async fn send_message_streaming(
    session_id: String,
    message: String,
    attachments: Option<Vec<serde_json::Value>>,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    ui: State<'_, UiState>,
    window: WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), String> {
    info!("Sending message on {}: {}", session_id, message);
    let client = acp.client.clone();
    let config = features.config.clone();
    let config_for_title = features.config.clone();
    let session_cache_for_send = features.session_cache.clone();
    let app_for_send = app.clone();
    let window_clone = window.clone();
    let window_label = window.label().to_string();
    let originators = ui.pending_prompt_originators.clone();

    // Tag this session as originated from this window. The permission
    // handler reads this map to decide which window the modal goes to;
    // we clear it on every send termination (success or error).
    if let Ok(mut m) = originators.lock() {
        m.insert(session_id.clone(), window_label.clone());
    }

    async_runtime::spawn_blocking(move || {
        let client_arc = client.clone();
        let config_arc = config.clone();

        if !client.is_connected() {
            if let Err(e) = client.connect() {
                let _ = window.emit("message_error", format!("Unable to connect: {}", e));
                if let Ok(mut m) = originators.lock() {
                    m.remove(&session_id);
                }
                return;
            }
        }

        // The notification handler (set up at app init) handles all streaming
        // chunks, permissions, and tool calls via Tauri events.
        let had_attachments = attachments.as_ref().is_some_and(|a| !a.is_empty());
        let send_result =
            client.send_chat_streaming_with_recovery(session_id.clone(), message, attachments);

        // Recovery may have moved us to a fresh session; pick that up
        // for the post-send epilogue (auto_steering, originator clear).
        let active_session_id = match &send_result {
            Ok(id) => id.clone(),
            Err(_) => session_id.clone(),
        };

        if let Err(e) = send_result {
            let error_str = format!("{}", e);
            let is_image_error = had_attachments
                && (error_str.contains("Internal error")
                    || error_str.contains("image")
                    || error_str.contains("unsupported")
                    || error_str.contains("response stream"));

            if is_image_error {
                // The ACP connection is likely stuck after an image error.
                // Disconnect, reconnect, create a fresh session, and tell
                // the originating window to adopt it. Other windows'
                // sessions are unaffected — they'll reconnect lazily on
                // their next send.
                info!("Image-related error detected — resetting ACP connection and session");
                client.disconnect();

                let new_session_id = match client.connect() {
                    Ok(_) => match client.create_session(None) {
                        Ok((id, _)) => {
                            client.send_builtin_steering(&id);
                            Some(id)
                        }
                        Err(e) => {
                            error!("Failed to create new session after image error: {}", e);
                            None
                        }
                    },
                    Err(e) => {
                        error!("Failed to reconnect after image error: {}", e);
                        None
                    }
                };

                // Broadcast to all windows; any window pinned to the
                // dead `oldSessionId` adopts `newSessionId`. Per-window
                // emit was correct in the single-session world but
                // would leave a peer window holding a dead id when
                // both windows are pinned to the same session.
                let _ = app_for_send.emit(
                    "session_reset",
                    serde_json::json!({
                        "reason": "image_unsupported",
                        "reconnected": new_session_id.is_some(),
                        "oldSessionId": &session_id,
                        "newSessionId": new_session_id,
                    }),
                );
            } else {
                let _ = window.emit("message_error", format!("Failed to send: {}", error_str));
            }
            if let Ok(mut m) = originators.lock() {
                m.remove(&session_id);
            }
            return;
        }

        let _ = window_clone.emit(
            "message_complete",
            serde_json::json!({ "sessionId": &active_session_id }),
        );

        // Refresh the window title now that there's a user message
        // available — fresh sessions get their first real title here.
        // Invalidate the session cache first so the JSONL re-extract
        // sees the new content rather than the cached "New Chat".
        {
            let mut cache = session_cache_for_send.lock_or_recover();
            *cache = None;
        }
        crate::commands::sessions::update_window_title(
            &app_for_send,
            &config_for_title,
            &session_cache_for_send,
            &window_label,
            &active_session_id,
        );

        // Clear the originator now that the prompt is finished.
        if let Ok(mut m) = originators.lock() {
            m.remove(&active_session_id);
            if active_session_id != session_id {
                m.remove(&session_id);
            }
        }

        // Trigger auto-steering generation periodically on the session
        // the message just landed on (post-recovery if applicable).
        crate::auto_steering::maybe_generate_steering(client_arc, config_arc, active_session_id);
    });

    Ok(())
}

#[tauri::command]
pub async fn send_permission_response(
    session_id: Option<String>,
    request_id: serde_json::Value,
    option_id: String,
    tool_title: String,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    info!("Permission response: {}={}", tool_title, option_id);

    acp.client
        .send_permission_response(&request_id, &option_id)
        .map_err(|e| AppError::connection_lost(format!("Permission response failed: {}", e)))?;

    if option_id == "allow_always" {
        let mut config = features
            .config
            .lock()
            .map_err(|e| AppError::lock(format!("{}", e)))?;
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
            .map_err(|e| AppError::internal(format!("Save: {}", e)))?;
    }

    // Audit log: record the user's decision. We classify option_id into
    // our event kinds; unrecognised strings are recorded as Denied with
    // the raw option_id in the tool field so the UI still shows them.
    // Frontend passes the session id from the permission notification
    // payload — None is tolerated for legacy callers.
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
    crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(audit_event));

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
    acp.client
        .connect()
        .map_err(|e| AppError::connection_lost(format!("Failed to reconnect: {}", e)))?;
    Ok(true)
}

#[tauri::command]
pub async fn cancel_generation(
    session_id: String,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    // Signal automation plan loop to stop
    features
        .automation_plan_cancelled
        .store(true, std::sync::atomic::Ordering::Relaxed);

    acp.client
        .cancel_session(&session_id)
        .map_err(|e| AppError::connection_lost(format!("Cancel failed: {}", e)))?;

    Ok(())
}

#[tauri::command]
pub async fn open_chat_with_message(
    session_id: String,
    message: String,
    acp: State<'_, AcpHandles>,
    ui: State<'_, UiState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if let Some(floating) = app.get_webview_window("floating") {
        let _ = floating.hide();
    }
    // Tag the in-flight prompt so permission notifications route to
    // the main chat window, not the (now hidden) floating one.
    if let Ok(mut m) = ui.pending_prompt_originators.lock() {
        m.insert(session_id.clone(), "main".to_string());
    }
    if let Some(main) = app.get_webview_window("main") {
        // Center on the active monitor
        crate::commands::window::center_window_on_active_monitor(&main);
        let _ = main.show();
        let _ = main.set_focus();
        let _ = main.emit("initial_message", message.clone());

        let client = acp.client.clone();
        let window = main.clone();
        let originators = ui.pending_prompt_originators.clone();

        async_runtime::spawn_blocking(move || {
            if !client.is_connected() {
                if let Err(e) = client.connect() {
                    let _ = window.emit("message_error", format!("Unable to connect: {}", e));
                    if let Ok(mut m) = originators.lock() {
                        m.remove(&session_id);
                    }
                    return;
                }
            }
            let result =
                client.send_chat_streaming_with_recovery(session_id.clone(), message, None);
            let active_session_id = match &result {
                Ok(id) => id.clone(),
                Err(_) => session_id.clone(),
            };
            if let Err(e) = result {
                let _ = window.emit("message_error", format!("Failed to send: {}", e));
                if let Ok(mut m) = originators.lock() {
                    m.remove(&session_id);
                }
                return;
            }
            let _ = window.emit(
                "message_complete",
                serde_json::json!({ "sessionId": &active_session_id }),
            );
            if let Ok(mut m) = originators.lock() {
                m.remove(&active_session_id);
                if active_session_id != session_id {
                    m.remove(&session_id);
                }
            }
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
        let guard = acp
            .pending_permission
            .lock()
            .map_err(|e| AppError::lock(format!("{}", e)))?;
        guard.clone()
    };

    if let Some(perm) = pending {
        if let Err(e) = acp
            .client
            .send_permission_response(&perm.request_id, "reject_once")
        {
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
    let guard = acp
        .pending_permission
        .lock()
        .map_err(|e| AppError::lock(format!("{}", e)))?;
    Ok(guard.is_some())
}

#[tauri::command]
pub async fn get_slash_commands(
    acp: State<'_, AcpHandles>,
) -> Result<Vec<crate::state::SlashCommand>, String> {
    let cmds = acp
        .slash_commands
        .lock()
        .map_err(|e| format!("Lock: {}", e))?;
    Ok(cmds.clone())
}

#[tauri::command]
pub async fn execute_slash_command(
    session_id: String,
    command: String,
    args: Option<serde_json::Value>,
    acp: State<'_, AcpHandles>,
    window: WebviewWindow,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let client = acp.client.clone();
    // Snapshot before the move into spawn_blocking — we need both for
    // telemetry after the call returns.
    let cmd_for_event = command.clone();
    let args_for_event = args.clone();

    let result = async_runtime::spawn_blocking(move || {
        if !client.is_connected() {
            return Err("Not connected".to_string());
        }
        let cmd_name = command.strip_prefix('/').unwrap_or(&command);

        let response = client.send_request(
            &client.vendor_method("commands/execute"),
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

    let _ = window.emit("slash_command_result", &result);
    Ok(result)
}

#[tauri::command]
pub async fn get_slash_command_options(_command: String) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({ "options": [] }))
}

#[tauri::command]
pub async fn send_steering_message(
    session_id: String,
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
) -> Result<Vec<serde_json::Value>, String> {
    let models = acp
        .available_models
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
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

/// Execute an automation plan step by step using sub-agents.
/// Each step is executed in a fresh sub-agent context, keeping the main
/// session clean and avoiding context window bloat.
///
/// The plan is a JSON array of steps, each with "step", "task", and "details" fields.
/// Progress events are emitted to the frontend as each step completes.
#[tauri::command]
pub async fn execute_automation_plan(
    session_id: String,
    plan_json: String,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    window: WebviewWindow,
) -> Result<(), String> {
    info!("Executing automation plan");

    // Parse the plan
    let plan: Vec<serde_json::Value> =
        serde_json::from_str(&plan_json).map_err(|e| format!("Invalid plan JSON: {}", e))?;

    if plan.is_empty() {
        return Err("Empty plan".to_string());
    }

    let total_steps = plan.len();
    info!("Plan has {} steps", total_steps);

    // Emit plan start event
    let _ = window.emit(
        "automation_plan_start",
        serde_json::json!({
            "totalSteps": total_steps,
            "plan": plan,
        }),
    );

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
            let task = step
                .get("task")
                .and_then(|t| t.as_str())
                .unwrap_or("Unknown task");
            let details = step.get("details").and_then(|d| d.as_str()).unwrap_or("");

            info!("Executing step {}/{}: {}", step_num, total_steps, task);

            // Emit step start event
            let _ = window.emit(
                "automation_step_start",
                serde_json::json!({
                    "step": step_num,
                    "totalSteps": total_steps,
                    "task": task,
                    "details": details,
                }),
            );

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
            match client.invoke_subagent(&session_id, &query) {
                Ok(result) => {
                    info!(
                        "Step {}/{} completed: {} chars",
                        step_num,
                        total_steps,
                        result.len()
                    );

                    // Check if the sub-agent reported a failure in its response text.
                    // The ACP call succeeded (we got a response), but the agent may
                    // have said "FAILED" because it couldn't actually perform the task.
                    let result_lower = result.to_lowercase();
                    let agent_reported_failure = result_lower.starts_with("failed")
                        || result_lower.contains("\nfailed")
                        || result_lower.contains("failed —")
                        || result_lower.contains("failed -");

                    if agent_reported_failure {
                        warn!(
                            "Step {}/{} agent reported failure: {}",
                            step_num,
                            total_steps,
                            &result[..result.len().min(200)]
                        );
                    }

                    let success = !agent_reported_failure;

                    let _ = window.emit(
                        "automation_step_complete",
                        serde_json::json!({
                            "step": step_num,
                            "totalSteps": total_steps,
                            "task": task,
                            "result": result,
                            "success": success,
                        }),
                    );

                    if !success {
                        warn!(
                            "Aborting automation plan: step {}/{} failed",
                            step_num, total_steps
                        );
                        // Mark remaining steps as stopped
                        for (j, remaining) in plan.iter().enumerate().skip(i + 1) {
                            let remaining_task = remaining
                                .get("task")
                                .and_then(|t| t.as_str())
                                .unwrap_or("Unknown task");
                            let _ = window.emit(
                                "automation_step_complete",
                                serde_json::json!({
                                    "step": j + 1,
                                    "totalSteps": total_steps,
                                    "task": remaining_task,
                                    "result": "Skipped due to earlier step failure",
                                    "success": false,
                                    "stopped": true,
                                }),
                            );
                        }
                        break;
                    }
                }
                Err(e) => {
                    let error_msg = format!("{}", e);
                    warn!("Step {}/{} failed: {}", step_num, total_steps, error_msg);

                    let _ = window.emit(
                        "automation_step_complete",
                        serde_json::json!({
                            "step": step_num,
                            "totalSteps": total_steps,
                            "task": task,
                            "result": error_msg,
                            "success": false,
                        }),
                    );

                    // Abort on transport/protocol errors too
                    warn!(
                        "Aborting automation plan: step {}/{} errored",
                        step_num, total_steps
                    );
                    for (j, remaining) in plan.iter().enumerate().skip(i + 1) {
                        let remaining_task = remaining
                            .get("task")
                            .and_then(|t| t.as_str())
                            .unwrap_or("Unknown task");
                        let _ = window.emit(
                            "automation_step_complete",
                            serde_json::json!({
                                "step": j + 1,
                                "totalSteps": total_steps,
                                "task": remaining_task,
                                "result": "Skipped due to earlier step failure",
                                "success": false,
                                "stopped": true,
                            }),
                        );
                    }
                    break;
                }
            }
        }

        // Emit plan complete event
        let _ = window.emit(
            "automation_plan_complete",
            serde_json::json!({
                "totalSteps": total_steps,
            }),
        );

        let _ = window.emit("message_complete", ());
    });

    Ok(())
}

/// Receive the result of a local extension tool call from the webview,
/// and send it back to the ACP agent as a follow-up message so the LLM
/// can continue its response with the data.
#[tauri::command]
pub async fn extension_tool_response(
    session_id: String,
    extension_id: String,
    tool_name: String,
    result_json: String,
    success: bool,
    acp: State<'_, AcpHandles>,
    window: WebviewWindow,
) -> Result<(), String> {
    info!(
        "Extension tool response: ext={}, tool={}, success={}, len={}",
        extension_id,
        tool_name,
        success,
        result_json.len()
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
        if let Err(e) = client.send_chat_streaming(&session_id, &content, None) {
            let _ = window.emit(
                "message_error",
                format!("Failed to send tool result: {}", e),
            );
        }

        // Emit message_complete so the frontend knows the follow-up response is done
        let _ = window.emit(
            "message_complete",
            serde_json::json!({ "sessionId": &session_id }),
        );
    });

    Ok(())
}

/// Send extension tool definitions to the agent as a hidden steering message.
/// Called by the frontend after extensions are loaded, so the agent knows
/// which local extension tools are available.
#[tauri::command]
pub async fn send_extension_tool_steering(
    session_id: String,
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

    info!(
        "Sending extension tool steering ({} chars)",
        tool_steering.len()
    );

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

        match client.send_chat_streaming(&session_id, &msg, None) {
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
        if !config
            .tool_permissions
            .tools
            .iter()
            .any(|t| t.title == tool_title)
        {
            config
                .tool_permissions
                .tools
                .push(crate::config::ToolPolicy {
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
    let existing = config
        .tool_permissions
        .tools
        .iter_mut()
        .find(|t| t.title == tool_title);

    if let Some(tool) = existing {
        tool.last_seen = timestamp;
        let policy = tool.policy.clone();
        if let Err(e) = config.save() {
            warn!("Failed to save config (tool policy lookup): {}", e);
        }
        Ok(policy)
    } else {
        // First time seeing this tool — register with "ask" policy
        config
            .tool_permissions
            .tools
            .push(crate::config::ToolPolicy {
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

/// Send a message for inline assist and stream the response to the
/// inline-assist window. The frontend passes the session id to use
/// (typically the floating window's session).
#[tauri::command]
pub async fn send_inline_assist(
    session_id: String,
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
        if let Err(e) = client.send_chat_streaming(&session_id, &message, None) {
            let _ = app.emit("inline_assist_error", format!("Failed: {}", e));
            return;
        }

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
/// Returns the final result text. The caller passes the session id that
/// AI-prompt steps should land on (typically the calling window's
/// pinned session).
#[tauri::command]
pub async fn execute_macro(
    session_id: String,
    steps: Vec<serde_json::Value>,
    initial_input: String,
    acp: State<'_, AcpHandles>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let client = acp.client.clone();
    let step_count = steps.len();
    let app_for_event = app.clone();
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
                    if let Err(e) = client.send_chat_streaming(&session_id, &full_prompt, None) {
                        return Err(format!("Step {} failed: {}", i + 1, e));
                    }
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

    // Telemetry — fire on success only. step_count is captured before
    // the move; we don't include macro names because those are user-typed.
    if result.is_ok() {
        crate::telemetry::track(
            &app_for_event,
            "macro_executed",
            Some(serde_json::json!({ "step_count": step_count })),
        );
    }

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
        "remove_blank_lines" => input
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
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
            input
                .lines()
                .filter(|l| seen.insert(*l))
                .collect::<Vec<_>>()
                .join("\n")
        }
        "number_lines" => input
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{:>4}  {}", i + 1, l))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => input.to_string(),
    }
}
