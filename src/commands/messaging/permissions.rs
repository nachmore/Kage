//! Tool-permission responses and extension-tool follow-up plumbing.

use super::*;

#[tauri::command]
pub async fn send_permission_response<R: tauri::Runtime>(
    session_id: Option<String>,
    request_id: serde_json::Value,
    option_id: String,
    tool_title: String,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
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

/// Approve those of the given pending requests that the policy allows now.
/// The prompt calls this right after the user trusts a category ("Trust all
/// commands" / "Trust all tools from <server>") for the requests of that
/// category already queued in its window: they were asked before the trust
/// existed, and the backend only auto-approves new ones.
///
/// Every request is decided again here, by the same rules the permission
/// handler enforces, rather than taken on the prompt's word: a tool the
/// user set to Deny after the request was queued stays denied, and a
/// background session's request stays outside category trust. Returns the
/// ids approved; the rest stay pending for the user to answer.
#[tauri::command]
pub async fn approve_permissions_if_allowed<R: tauri::Runtime>(
    request_ids: Vec<serde_json::Value>,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<Vec<serde_json::Value>, AppError> {
    // Never hold the pending map and the config lock at the same time.
    let candidates: Vec<(String, crate::state::PendingPermission)> = {
        let pending = acp.pending_permissions.lock_or_recover();
        request_ids
            .iter()
            .filter_map(|id| {
                let key = crate::state::permission_key(id);
                pending.get(&key).cloned().map(|p| (key, p))
            })
            .collect()
    };
    let allowed: Vec<String> = {
        let config = features.config.lock_or_recover();
        candidates
            .into_iter()
            .filter(|(_, p)| {
                config.tool_permissions.resolve_request(&p.request)
                    == crate::config::PolicyKind::Allow
            })
            .map(|(key, _)| key)
            .collect()
    };

    let mut approved = Vec::new();
    for key in allowed {
        // Claim it first: a request answered elsewhere meanwhile is gone,
        // and must not get a second answer.
        let Some(entry) = acp.pending_permissions.lock_or_recover().remove(&key) else {
            continue;
        };
        if let Err(e) = acp
            .client
            .send_permission_response(&entry.request_id, "allow_once")
        {
            warn!("Failed to approve a queued permission request: {}", e);
            acp.pending_permissions.lock_or_recover().insert(key, entry);
            continue;
        }
        info!(
            "Approved a queued permission request under a new category trust: {}",
            entry.request.title
        );
        crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(
            crate::permission_audit::AuditEvent::Granted {
                tool: entry.request.title.clone(),
                grant_type: crate::config::GrantType::Once,
                session_id: entry.session_id.clone(),
                args_preview: entry.args_preview.clone(),
            },
        ));
        // Other windows showing or queueing it drop it, like any answer.
        crate::event_targets::emit_permission_audience(
            &app,
            events::PERMISSION_DISMISSED,
            &serde_json::json!({ "requestId": entry.request_id }),
        );
        approved.push(entry.request_id);
    }
    Ok(approved)
}

/// Trust (or stop trusting) every tool request of an ACP tool kind — today
/// only `"execute"`, i.e. "trust all CLI commands". Recorded in the audit
/// log as a standing grant / revoke, and broadcast as `config_updated` so
/// an open Settings window reflects it.
#[tauri::command]
pub async fn set_tool_kind_trusted<R: tauri::Runtime>(
    kind: String,
    trusted: bool,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    {
        // lock_or_recover: revoking must work even if another thread
        // poisoned the mutex — the permission handler keeps running.
        // Save under the lock (like save_config / update_tool_policy) so it
        // orders against the other lock-holding savers, and roll the
        // in-memory change back if it can't be persisted, so the setting is
        // never on in memory but off on disk (or vice versa).
        let mut config = features.config.lock_or_recover();
        let changed = config
            .tool_permissions
            .set_kind_trusted(&kind, trusted)
            .ok_or_else(|| {
                AppError::keyed(
                    ErrorKind::Internal,
                    "errors.permission.unknown_tool_kind",
                    &[("kind", &kind)],
                )
            })?;
        if !changed {
            return Ok(());
        }
        if let Err(e) = config.save() {
            let _ = config.tool_permissions.set_kind_trusted(&kind, !trusted);
            return Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.config.save_failed",
                &[("reason", &e.to_string())],
            ));
        }
    }
    info!("Tool kind '{}' trusted={}", kind, trusted);
    let label = format!("All tools of kind '{}'", kind);
    let event = if trusted {
        crate::permission_audit::AuditEvent::Granted {
            tool: label,
            grant_type: crate::config::GrantType::Always,
            session_id: None,
            args_preview: None,
        }
    } else {
        crate::permission_audit::AuditEvent::Revoked {
            tool: label,
            prior_policy: crate::config::PolicyKind::Allow,
            prior_grant_type: Some(crate::config::GrantType::Always),
        }
    };
    crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(event));
    if let Err(e) = app.emit(events::CONFIG_UPDATED, ()) {
        warn!("Failed to emit config_updated: {}", e);
    }
    Ok(())
}

/// Trust (or stop trusting) every tool from an MCP server — "Trust all
/// tools from <server>". Same contract as `set_tool_kind_trusted`: refuses
/// an invalid server name, saves under the lock and rolls back if the save
/// fails, records a standing grant / revoke in the audit log, and broadcasts
/// `config_updated`.
#[tauri::command]
pub async fn set_mcp_server_trusted<R: tauri::Runtime>(
    server: String,
    trusted: bool,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    {
        // lock_or_recover so revoking works even after a poisoning panic;
        // see set_tool_kind_trusted for why the save happens under the lock.
        let mut config = features.config.lock_or_recover();
        let prior = config.tool_permissions.trusted_mcp_servers.clone();
        let changed = config
            .tool_permissions
            .set_mcp_server_trusted(&server, trusted)
            .ok_or_else(|| {
                AppError::keyed(
                    ErrorKind::Internal,
                    "errors.permission.invalid_mcp_server",
                    &[("server", &server)],
                )
            })?;
        if !changed {
            return Ok(());
        }
        if let Err(e) = config.save() {
            config.tool_permissions.trusted_mcp_servers = prior;
            return Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.config.save_failed",
                &[("reason", &e.to_string())],
            ));
        }
    }
    info!("MCP server {:?} trusted={}", server, trusted);
    let label = format!("All tools from MCP server '{}'", server);
    let event = if trusted {
        crate::permission_audit::AuditEvent::Granted {
            tool: label,
            grant_type: crate::config::GrantType::Always,
            session_id: None,
            args_preview: None,
        }
    } else {
        crate::permission_audit::AuditEvent::Revoked {
            tool: label,
            prior_policy: crate::config::PolicyKind::Allow,
            prior_grant_type: Some(crate::config::GrantType::Always),
        }
    };
    crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(event));
    if let Err(e) = app.emit(events::CONFIG_UPDATED, ()) {
        warn!("Failed to emit config_updated: {}", e);
    }
    Ok(())
}

/// Dismiss pending permission requests. With `session_id`, only requests
/// belonging to that session (or carrying no session) are dismissed —
/// another window's blocked prompt is left alone. Without it, everything
/// pending is dismissed (legacy behaviour).
#[tauri::command]
pub async fn dismiss_pending_permission<R: tauri::Runtime>(
    session_id: Option<String>,
    acp: State<'_, AcpHandles>,
    app: tauri::AppHandle<R>,
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
#[allow(clippy::too_many_arguments)] // Tauri commands take state via parameters.
pub async fn extension_tool_response<R: tauri::Runtime>(
    session_id: String,
    extension_id: String,
    tool_name: String,
    result_json: String,
    success: bool,
    acp: State<'_, AcpHandles>,
    ui: State<'_, UiState>,
    window: WebviewWindow<R>,
) -> Result<(), AppError> {
    info!(
        "Extension tool response: ext={}, tool={}, success={}, len={}",
        extension_id,
        tool_name,
        success,
        result_json.len()
    );

    let client = acp.client.clone();
    let originators = ui.pending_prompt_originators.clone();
    let window_label = window.label().to_string();

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

        // The originating send's epilogue already dropped its originator
        // entry; re-tag so permission prompts raised by the continuation
        // route to this window instead of falling back to floating.
        if let Ok(mut m) = originators.lock() {
            m.insert(session_id.clone(), window_label.clone());
        }

        // Send as a follow-up user message so the agent continues
        let result = client.send_chat_streaming(&session_id, &content, None);

        // Only drop the tag if it's still ours — a user send from another
        // window may have re-tagged the session meanwhile.
        if let Ok(mut m) = originators.lock() {
            if m.get(&session_id) == Some(&window_label) {
                m.remove(&session_id);
            }
        }

        let app = window.app_handle();
        super::notifications::flush_session_chunks(app, &client, &session_id);
        if let Err(e) = result {
            crate::event_targets::emit_to_self(
                &window,
                events::MESSAGE_ERROR,
                &format!("Failed to send tool result: {}", e),
            );
            // Peers only hear broadcasts; clear their activity badge.
            crate::event_targets::emit_streaming_audience(
                app,
                events::SESSION_ACTIVITY,
                &serde_json::json!({ "sessionId": &session_id, "kind": "failed" }),
            );
            return;
        }

        // Tell streaming-aware peers that the follow-up response is done.
        crate::event_targets::emit_streaming_audience(
            app,
            events::MESSAGE_COMPLETE,
            &serde_json::json!({ "sessionId": &session_id }),
        );
    });

    Ok(())
}

/// Send extension tool definitions to the agent as a hidden steering message.
/// Called by the frontend after extensions load and again whenever the
/// window's extension tools change (settings, enable/disable, install), so
/// the agent knows which local extension tools are available right now.
/// An empty block means the window has no extension tools any more. With
/// no session id (the window has none right now) the block is only
/// recorded, and replayed to the window's sessions before their next prompt.
#[tauri::command]
pub async fn send_extension_tool_steering<R: tauri::Runtime>(
    session_id: Option<String>,
    tool_steering: String,
    acp: State<'_, AcpHandles>,
    window: tauri::WebviewWindow<R>,
) -> Result<(), AppError> {
    let Some(session_id) = session_id.filter(|s| !s.is_empty()) else {
        acp.tool_steering
            .lock_or_recover()
            .publish(window.label(), &tool_steering);
        return Ok(());
    };
    // Deduplicate per session: skip if THIS session already has (or is
    // receiving) this exact block, and turn an empty block into a
    // withdrawal for sessions that were told about tools (see
    // ToolSteeringState). The claim is released if the send fails.
    let Some(claim) =
        acp.tool_steering
            .lock_or_recover()
            .claim(window.label(), &session_id, &tool_steering)
    else {
        return Ok(());
    };

    let client = acp.client.clone();
    let state = acp.tool_steering.clone();

    async_runtime::spawn_blocking(move || {
        deliver_tool_steering(&client, &state, &session_id, &claim);
    });

    Ok(())
}

/// Send a claimed extension tool steering block on `session_id` as a muted
/// prompt (its "ack" never streams into a window). Releases the claim when
/// the send doesn't happen, so the next attempt retries. Blocking: waits
/// for any delivery already under way on the session, then skips if a
/// newer claim replaced this one while it waited.
pub(super) fn deliver_tool_steering(
    client: &crate::acp_client::AcpClient,
    state: &std::sync::Mutex<crate::state::ToolSteeringState>,
    session_id: &str,
    claim: &crate::state::SteeringClaim,
) {
    // Bound first: the state guard must not live across the send.
    let delivery_lock = state.lock_or_recover().delivery_lock(session_id);
    let _delivering = delivery_lock.lock_or_recover();
    if !state.lock_or_recover().is_current(session_id, claim) {
        info!(
            "Extension tool steering on {} superseded before it was sent",
            session_id
        );
        return;
    }
    if !client.is_connected() {
        state.lock_or_recover().release(session_id, claim);
        return;
    }

    info!(
        "Sending extension tool steering on {} ({} chars)",
        session_id,
        claim.body.len()
    );
    let msg = format!(
        "{} {}\n\n---\n\n<instructions>Respond with only \"ack\" to confirm receipt. Do not summarize or comment on the content above.</instructions>",
        crate::commands::system::STEERING_MSG_PREFIX,
        claim.body
    );

    // A Stop on this session ends this prompt early (still Ok, stop reason
    // "cancelled") and the agent may not have kept it: release, so the next
    // prompt replays it, whichever thread is sending that prompt.
    let cancel_epoch = client.cancel_epoch(session_id);
    match client.send_chat_streaming_background(session_id, &msg, false) {
        Ok(()) if client.cancel_epoch(session_id) != cancel_epoch => {
            info!(
                "Extension tool steering on {} cancelled; will replay",
                session_id
            );
            state.lock_or_recover().release(session_id, claim);
        }
        Ok(()) => {
            state.lock_or_recover().confirm(session_id, claim);
            info!("Extension tool steering sent");
        }
        Err(e) => {
            warn!("Failed to send extension tool steering: {}", e);
            state.lock_or_recover().release(session_id, claim);
            // The idle watchdog only trips after total silence on every
            // session, so the connection is wedged. It still reads as
            // connected, though, and a user send queued behind this (the
            // replay in send_message_streaming) would sit out a second idle
            // timeout before its recovery ladder respawns the agent. Drop the
            // connection so that send fails fast straight into recovery.
            if e.to_string().contains("Timeout waiting for response") {
                warn!("Tool steering timed out; dropping the wedged ACP connection");
                client.force_disconnect();
            }
        }
    }
}

/// Block until no steering delivery is claimed or under way on
/// `session_id`. Returns whether there was one to wait for. Blocking.
pub(super) fn wait_for_tool_steering(
    state: &std::sync::Mutex<crate::state::ToolSteeringState>,
    session_id: &str,
) -> bool {
    // A claim's delivery thread may not have taken the delivery lock yet;
    // that gap is milliseconds, so poll it briefly. The wait on the lock
    // itself can be a whole agent turn and isn't bounded.
    const GAP: std::time::Duration = std::time::Duration::from_secs(2);
    let mut gap_deadline = std::time::Instant::now() + GAP;
    let mut waited = false;
    loop {
        let delivery_lock = state.lock_or_recover().delivery_lock(session_id);
        if delivery_lock.try_lock().is_err() {
            waited = true;
            drop(delivery_lock.lock_or_recover());
            // Only time spent with the lock free counts as the gap.
            gap_deadline = std::time::Instant::now() + GAP;
        }
        if !state.lock_or_recover().has_in_flight(session_id) {
            return waited;
        }
        if std::time::Instant::now() >= gap_deadline {
            warn!(
                "Extension tool steering on {} still claimed but not delivering; sending anyway",
                session_id
            );
            return waited;
        }
        waited = true;
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
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

#[cfg(test)]
mod tests {
    use super::wait_for_tool_steering;
    use crate::state::ToolSteeringState;
    use kage_core::lock_ext::LockExt;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn prompt_waits_for_a_claimed_steering_delivery() {
        let state = Arc::new(Mutex::new(ToolSteeringState::default()));
        let claim = state.lock_or_recover().claim("main", "S", "tools").unwrap();
        let (st, c) = (state.clone(), claim.clone());
        let delivery = std::thread::spawn(move || {
            // The spawned delivery hasn't taken the lock yet...
            std::thread::sleep(Duration::from_millis(30));
            let lock = st.lock_or_recover().delivery_lock("S");
            let _held = lock.lock_or_recover();
            // ...then holds it for the length of an agent turn.
            std::thread::sleep(Duration::from_millis(30));
            st.lock_or_recover().confirm("S", &c);
        });
        assert!(wait_for_tool_steering(&state, "S"));
        assert!(
            !state.lock_or_recover().has_in_flight("S"),
            "delivered first"
        );
        delivery.join().unwrap();
    }

    #[test]
    fn prompt_does_not_wait_when_nothing_is_claimed() {
        let state = Mutex::new(ToolSteeringState::default());
        assert!(!wait_for_tool_steering(&state, "S"));
    }
}
