//! Tool-permission responses and extension-tool follow-up plumbing.

use super::*;

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
        .map_err(|e| {
            AppError::keyed(
                ErrorKind::ConnectionLost,
                "errors.permission.response_failed",
                &[("reason", &e.to_string())],
            )
        })?;

    if option_id == "allow_always" {
        // Mutate under the lock, clone a snapshot, save OUTSIDE the lock.
        // The ACP reader thread's handle_permission_notification takes the
        // same mutex per notification — holding it across write+fsync+rename
        // stalls ACP parsing (streaming chunks, responses) for the fsync
        // duration. Same pattern as notifications.rs.
        let snapshot = {
            let mut config = features.config.lock().map_err(|_| {
                AppError::keyed(ErrorKind::LockError, "errors.lock.acquire_failed", &[])
            })?;
            if let Some(tool) = config
                .tool_permissions
                .tools
                .iter_mut()
                .find(|t| t.title == tool_title)
            {
                tool.policy = crate::config::PolicyKind::Allow;
            }
            config.clone()
        };
        snapshot.save().map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.config.save_failed",
                &[("reason", &e.to_string())],
            )
        })?;
    }

    // Audit log: record the user's decision. We classify option_id into
    // our event kinds; unrecognised strings are recorded as Denied with
    // the raw option_id in the tool field so the UI still shows them.
    // Frontend passes the session id from the permission notification
    // payload — None is tolerated for legacy callers.
    let audit_event = match option_id.as_str() {
        "allow_once" => crate::permission_audit::AuditEvent::Granted {
            tool: tool_title.clone(),
            grant_type: crate::config::GrantType::Once,
            session_id,
            args_preview: None,
        },
        "allow_24h" => crate::permission_audit::AuditEvent::Granted {
            tool: tool_title.clone(),
            grant_type: crate::config::GrantType::Hours24,
            session_id,
            args_preview: None,
        },
        "allow_always" => crate::permission_audit::AuditEvent::Granted {
            tool: tool_title.clone(),
            grant_type: crate::config::GrantType::Always,
            session_id,
            args_preview: None,
        },
        _ => crate::permission_audit::AuditEvent::Denied {
            tool: tool_title.clone(),
            session_id,
        },
    };
    crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(audit_event));

    if let Ok(mut pending) = acp.pending_permissions.lock() {
        pending.remove(&crate::state::permission_key(&request_id));
    }

    // Tell the permission audience (chat hosts + floating + settings) to
    // close their modal for THIS request. Windows with a different
    // pending request keep theirs open — the payload carries the
    // request id so listeners can match.
    crate::event_targets::emit_permission_audience(
        &app,
        events::PERMISSION_DISMISSED,
        &serde_json::json!({ "requestId": request_id }),
    );

    Ok(())
}

/// Dismiss pending permission requests. With `session_id`, only requests
/// belonging to that session (or carrying no session) are dismissed —
/// another window's blocked prompt is left alone. Without it, everything
/// pending is dismissed (legacy behaviour).
#[tauri::command]
pub async fn dismiss_pending_permission(
    session_id: Option<String>,
    acp: State<'_, AcpHandles>,
    app: tauri::AppHandle,
) -> Result<bool, AppError> {
    let targets: Vec<(String, crate::state::PendingPermission)> = {
        let guard = acp.pending_permissions.lock().map_err(|_| {
            AppError::keyed(ErrorKind::LockError, "errors.lock.acquire_failed", &[])
        })?;
        guard
            .iter()
            .filter(|(_, p)| match (&session_id, &p.session_id) {
                (Some(want), Some(have)) => want == have,
                _ => true,
            })
            .map(|(k, p)| (k.clone(), p.clone()))
            .collect()
    };

    if targets.is_empty() {
        return Ok(false);
    }

    let mut dismissed_any = false;
    let mut last_err: Option<String> = None;
    for (key, perm) in targets {
        // Only clear the local entry if the agent actually accepted the
        // dismissal. If the send fails (broken pipe, transport error)
        // the agent still believes a prompt is open — clearing locally
        // would desynchronize state and stall the next user message on
        // the agent's "prompt already in progress" guard.
        match acp
            .client
            .send_permission_response(&perm.request_id, "reject_once")
        {
            Ok(()) => {
                if let Ok(mut guard) = acp.pending_permissions.lock() {
                    guard.remove(&key);
                }
                // PERMISSION_DISMISSED listens in chat hosts + floating +
                // settings; carries the request id so only the matching
                // modal closes.
                crate::event_targets::emit_permission_audience(
                    &app,
                    events::PERMISSION_DISMISSED,
                    &serde_json::json!({ "requestId": perm.request_id }),
                );
                dismissed_any = true;
            }
            Err(e) => {
                warn!(
                    "Failed to dismiss pending permission, keeping local state: {}",
                    e
                );
                last_err = Some(e.to_string());
            }
        }
    }

    match (dismissed_any, last_err) {
        (false, Some(e)) => Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.permission.dismiss_failed",
            &[("reason", &e)],
        )),
        _ => Ok(dismissed_any),
    }
}

/// With `request_id`, checks whether THAT request is still pending;
/// without, whether anything is.
#[tauri::command]
pub async fn has_pending_permission(
    request_id: Option<serde_json::Value>,
    acp: State<'_, AcpHandles>,
) -> Result<bool, AppError> {
    let guard = acp
        .pending_permissions
        .lock()
        .map_err(|_| AppError::keyed(ErrorKind::LockError, "errors.lock.acquire_failed", &[]))?;
    Ok(match request_id {
        Some(id) => guard.contains_key(&crate::state::permission_key(&id)),
        None => !guard.is_empty(),
    })
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
) -> Result<(), AppError> {
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
            crate::event_targets::emit_to_self(
                &window,
                events::MESSAGE_ERROR,
                &"Not connected to agent".to_string(),
            );
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
            crate::event_targets::emit_to_self(
                &window,
                events::MESSAGE_ERROR,
                &format!("Failed to send tool result: {}", e),
            );
        }

        // Tell streaming-aware peers that the follow-up response is done.
        crate::event_targets::emit_streaming_audience(
            window.app_handle(),
            events::MESSAGE_COMPLETE,
            &serde_json::json!({ "sessionId": &session_id }),
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
) -> Result<(), AppError> {
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
) -> Result<String, AppError> {
    let tool_title = format!("ext:{}/{}", extension_id, tool_name);

    // Mutate in-memory state under the lock, clone a snapshot, then save
    // outside — the ACP reader thread contends on this mutex per
    // notification and must never wait behind a write+fsync+rename.
    let (policy_str, save_snapshot) = {
        let mut config = features.config.lock().map_err(|_| {
            AppError::keyed(ErrorKind::LockError, "errors.lock.acquire_failed", &[])
        })?;

        let timestamp = chrono::Utc::now().to_rfc3339();

        // Check trust_all first
        if config.tool_permissions.trust_all {
            // Still register the tool so it shows up in settings
            let snap = if !config
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
                        policy: crate::config::PolicyKind::Allow,
                        last_seen: timestamp.clone(),
                        granted_at: timestamp,
                        grant_type: crate::config::GrantType::Always,
                    });
                Some(config.clone())
            } else {
                None
            };
            (crate::config::PolicyKind::Allow.as_str().to_string(), snap)
        } else if let Some(tool) = config
            .tool_permissions
            .tools
            .iter_mut()
            .find(|t| t.title == tool_title)
        {
            tool.last_seen = timestamp;
            let policy = tool.policy;
            (policy.as_str().to_string(), Some(config.clone()))
        } else {
            // First time seeing this tool — register with "ask" policy
            config
                .tool_permissions
                .tools
                .push(crate::config::ToolPolicy {
                    title: tool_title,
                    policy: crate::config::PolicyKind::Ask,
                    last_seen: timestamp,
                    granted_at: String::new(),
                    grant_type: crate::config::GrantType::Once,
                });
            (
                crate::config::PolicyKind::Ask.as_str().to_string(),
                Some(config.clone()),
            )
        }
    };

    if let Some(snap) = save_snapshot {
        if let Err(e) = snap.save() {
            warn!("Failed to save config (extension tool permission): {}", e);
        }
    }

    Ok(policy_str)
}
