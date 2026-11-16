//! Tool-permission responses and extension-tool follow-up plumbing.

use super::*;

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri commands take state via parameters.
pub async fn send_permission_response<R: tauri::Runtime>(
    session_id: Option<String>,
    request_id: serde_json::Value,
    permission_token: Option<String>,
    option_id: String,
    tool_title: String,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    info!("Permission response: {}={}", tool_title, option_id);

    // Claim the request first, by its token and on the current connection.
    // One that isn't pending was answered elsewhere, or died with an agent
    // that was restarted; its id may now belong to an unrelated request of
    // the new process, so nothing is written.
    let (key, entry) = match claim_pending_permission(
        &acp.pending_permissions,
        &request_id,
        permission_token.as_deref(),
        acp.client.connection_generation(),
    ) {
        PermissionClaim::Claimed { key, entry } => (key, entry),
        PermissionClaim::Gone => {
            info!("Permission response for a request that is no longer pending; not sent");
            crate::event_targets::emit_permission_audience(
                &app,
                events::PERMISSION_DISMISSED,
                &super::notifications::permission_dismissed_payload(
                    &request_id,
                    permission_token.as_deref(),
                ),
            );
            return Ok(());
        }
        PermissionClaim::NotThisRequest => {
            // A live request shares the id: it is someone else's to answer,
            // and its modal stays.
            warn!("Permission response does not name the pending request with its id; not sent");
            return Ok(());
        }
    };
    match acp
        .client
        .send_permission_response_on(entry.generation, &request_id, &option_id)
    {
        Ok(true) => {}
        Ok(false) => {
            // The connection went between the claim and the write.
            info!("Permission response for a request whose agent is gone; not sent");
            crate::event_targets::emit_permission_audience(
                &app,
                events::PERMISSION_DISMISSED,
                &super::notifications::permission_dismissed_payload(
                    &request_id,
                    Some(&entry.token),
                ),
            );
            return Ok(());
        }
        Err(e) => {
            acp.pending_permissions
                .lock_or_recover()
                .insert(key, *entry);
            return Err(AppError::keyed(
                ErrorKind::ConnectionLost,
                "errors.permission.response_failed",
                &[("reason", &e.to_string())],
            ));
        }
    }

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

    // Tell the permission audience (chat hosts + floating + settings) to
    // close their modal for THIS request. Windows with a different
    // pending request keep theirs open — the payload carries the
    // request id so listeners can match.
    crate::event_targets::emit_permission_audience(
        &app,
        events::PERMISSION_DISMISSED,
        &super::notifications::permission_dismissed_payload(&request_id, Some(&entry.token)),
    );

    Ok(())
}

type PendingPermissions =
    std::sync::Mutex<std::collections::HashMap<String, crate::state::PendingPermission>>;

/// What [`claim_pending_permission`] found.
#[derive(Debug)]
enum PermissionClaim {
    /// The request, taken out of the pending ones: answer it now.
    Claimed {
        key: String,
        entry: Box<crate::state::PendingPermission>,
    },
    /// Nothing pending is this request any more (answered elsewhere, or
    /// its agent is gone); its modal should close.
    Gone,
    /// A pending request has this id but is another request (a restarted
    /// agent reused the id), or the answer names none. Left pending.
    NotThisRequest,
}

/// Take the request with `request_id` out of the pending ones, if the
/// answer names it (`token`) and it was asked on the connection that is
/// current now (`generation`): only then may it be answered. One from a
/// connection that is gone is dropped.
fn claim_pending_permission(
    pending: &PendingPermissions,
    request_id: &serde_json::Value,
    token: Option<&str>,
    generation: u64,
) -> PermissionClaim {
    let key = crate::state::permission_key(request_id);
    let mut map = pending.lock_or_recover();
    let Some(entry) = map.get(&key) else {
        return PermissionClaim::Gone;
    };
    if entry.answers(token, generation) {
        let entry = Box::new(map.remove(&key).expect("present above"));
        return PermissionClaim::Claimed { key, entry };
    }
    if token.is_some_and(|t| !t.is_empty() && t == entry.token) {
        // Its own token, but asked on a connection that has gone since.
        map.remove(&key);
        return PermissionClaim::Gone;
    }
    PermissionClaim::NotThisRequest
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
/// permission tokens of those approved (never their JSON-RPC ids: a
/// restarted agent reuses ids, so the prompt removes approved requests by
/// token, and a new request that took an approved one's id stays); the
/// rest stay pending for the user to answer.
///
/// `permission_tokens[i]` is the token the prompt got with
/// `request_ids[i]`; a request is approved only if it matches, and was
/// asked on the current connection (see `send_permission_response`).
#[tauri::command]
pub async fn approve_permissions_if_allowed<R: tauri::Runtime>(
    request_ids: Vec<serde_json::Value>,
    permission_tokens: Option<Vec<Option<String>>>,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<Vec<String>, AppError> {
    let tokens = permission_tokens.unwrap_or_default();
    let generation = acp.client.connection_generation();
    // Never hold the pending map and the config lock at the same time.
    let candidates: Vec<(String, crate::state::PendingPermission)> = {
        let pending = acp.pending_permissions.lock_or_recover();
        request_ids
            .iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let key = crate::state::permission_key(id);
                let token = tokens.get(i).and_then(|t| t.as_deref());
                pending
                    .get(&key)
                    .filter(|p| p.answers(token, generation))
                    .cloned()
                    .map(|p| (key, p))
            })
            .collect()
    };
    let allowed: Vec<crate::state::PendingPermission> = {
        let config = features.config.lock_or_recover();
        candidates
            .into_iter()
            .filter(|(_, p)| {
                config.tool_permissions.resolve_request(&p.request)
                    == crate::config::PolicyKind::Allow
            })
            .map(|(_, p)| p)
            .collect()
    };

    let mut approved = Vec::new();
    for candidate in allowed {
        // Claim it first: a request answered elsewhere meanwhile is gone,
        // and must not get a second answer; nor may a new request that
        // took over its id.
        let PermissionClaim::Claimed { key, entry } = claim_pending_permission(
            &acp.pending_permissions,
            &candidate.request_id,
            Some(&candidate.token),
            generation,
        ) else {
            continue;
        };
        match acp.client.send_permission_response_on(
            entry.generation,
            &entry.request_id,
            "allow_once",
        ) {
            Ok(true) => {}
            Ok(false) => {
                info!("Not approving a queued permission request whose agent is gone");
                continue;
            }
            Err(e) => {
                warn!("Failed to approve a queued permission request: {}", e);
                acp.pending_permissions
                    .lock_or_recover()
                    .insert(key, *entry);
                continue;
            }
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
            &super::notifications::permission_dismissed_payload(
                &entry.request_id,
                Some(&entry.token),
            ),
        );
        approved.push(entry.token);
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
        // the agent's "prompt already in progress" guard. A request asked
        // on a connection that is gone is only dropped: its id may name an
        // unrelated request of the agent now running.
        match acp.client.send_permission_response_on(
            perm.generation,
            &perm.request_id,
            "reject_once",
        ) {
            Ok(written) => {
                if !written {
                    info!("Dropped a pending permission request whose agent is gone");
                }
                if let Ok(mut guard) = acp.pending_permissions.lock() {
                    // Still this request, not a new one that took its id.
                    if guard.get(&key).is_some_and(|p| p.token == perm.token) {
                        guard.remove(&key);
                    }
                }
                // PERMISSION_DISMISSED listens in chat hosts + floating +
                // settings; carries the request's id and token so only the
                // matching modal closes.
                crate::event_targets::emit_permission_audience(
                    &app,
                    events::PERMISSION_DISMISSED,
                    &super::notifications::permission_dismissed_payload(
                        &perm.request_id,
                        Some(&perm.token),
                    ),
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

/// With `request_id`, checks whether THAT request is still pending (and,
/// given its `permission_token`, still the same request); without, whether
/// anything is.
#[tauri::command]
pub async fn has_pending_permission(
    request_id: Option<serde_json::Value>,
    permission_token: Option<String>,
    acp: State<'_, AcpHandles>,
) -> Result<bool, AppError> {
    let guard = acp
        .pending_permissions
        .lock()
        .map_err(|_| AppError::keyed(ErrorKind::LockError, "errors.lock.acquire_failed", &[]))?;
    Ok(match request_id {
        Some(id) => guard
            .get(&crate::state::permission_key(&id))
            .is_some_and(|p| permission_token.as_ref().is_none_or(|t| *t == p.token)),
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

/// Record the calling window's extension tool steering: the agent-facing
/// description of its extension tools, sent when they load and again
/// whenever they change (settings, enable/disable, install). An empty block
/// means the window has no extension tools (any more). Nothing goes to the
/// agent here: the window's next user prompt on a session that doesn't have
/// this block yet carries it (see `ToolSteeringState`).
#[tauri::command]
pub async fn send_extension_tool_steering<R: tauri::Runtime>(
    tool_steering: String,
    acp: State<'_, AcpHandles>,
    window: tauri::WebviewWindow<R>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    let label = window.label();
    let mut steering = acp.tool_steering.lock_or_recover();
    // Checked under the steering lock, which the window's Destroyed handler
    // takes after Tauri has unregistered the window: a publication that
    // raced the window's teardown is either refused here or dropped there.
    if app.get_webview_window(label).is_none() {
        return Ok(());
    }
    steering.publish(label, &tool_steering);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use serde_json::json;

    /// A request as the handler records it: `id`, named `token`, asked on
    /// connection `generation`.
    fn request(
        id: serde_json::Value,
        token: &str,
        generation: u64,
    ) -> crate::state::PendingPermission {
        crate::state::PendingPermission {
            request_id: id,
            token: token.to_string(),
            generation,
            ..Default::default()
        }
    }

    fn pending_with(
        requests: Vec<crate::state::PendingPermission>,
    ) -> Mutex<HashMap<String, crate::state::PendingPermission>> {
        let mut map = HashMap::new();
        for r in requests {
            map.insert(crate::state::permission_key(&r.request_id), r);
        }
        Mutex::new(map)
    }

    fn insert(pending: &PendingPermissions, r: crate::state::PendingPermission) {
        pending
            .lock()
            .unwrap()
            .insert(crate::state::permission_key(&r.request_id), r);
    }

    #[test]
    fn claim_takes_a_pending_request_once() {
        let pending = pending_with(vec![request(json!(7), "t7", 1)]);
        let PermissionClaim::Claimed { key, entry } =
            claim_pending_permission(&pending, &json!(7), Some("t7"), 1)
        else {
            panic!("a pending request is claimed");
        };
        assert_eq!(key, crate::state::permission_key(&json!(7)));
        assert_eq!(entry.request_id, json!(7));
        assert!(matches!(
            claim_pending_permission(&pending, &json!(7), Some("t7"), 1),
            PermissionClaim::Gone
        ));
    }

    #[test]
    fn claim_refuses_a_request_withdrawn_by_a_restart() {
        // A restart withdraws every pending request; a modal answered
        // afterwards must not write its old id to the new process, which
        // may be using that id for an unrelated request.
        let pending = pending_with(vec![request(json!(3), "a", 1), request(json!("r"), "b", 1)]);
        let mut withdrawn: Vec<_> = super::super::notifications::take_pending_permissions(&pending)
            .into_iter()
            .map(|p| p.request_id)
            .collect();
        withdrawn.sort_by_key(|v| v.to_string());
        assert_eq!(withdrawn, vec![json!("r"), json!(3)]);
        assert!(matches!(
            claim_pending_permission(&pending, &json!(3), Some("a"), 1),
            PermissionClaim::Gone
        ));
        assert!(matches!(
            claim_pending_permission(&pending, &json!("r"), Some("b"), 1),
            PermissionClaim::Gone
        ));
    }

    #[test]
    fn a_delayed_reply_never_claims_the_new_processs_request_with_the_same_id() {
        let pending = pending_with(vec![request(json!(5), "old", 1)]);
        super::super::notifications::take_pending_permissions(&pending);
        // The restarted agent numbers its requests afresh and asks again.
        insert(&pending, request(json!(5), "new", 3));
        // The old modal's Allow, delayed across the restart.
        assert!(matches!(
            claim_pending_permission(&pending, &json!(5), Some("old"), 3),
            PermissionClaim::NotThisRequest
        ));
        // Left for its own modal, which can still answer it.
        assert!(matches!(
            claim_pending_permission(&pending, &json!(5), Some("new"), 3),
            PermissionClaim::Claimed { .. }
        ));
    }

    #[test]
    fn a_reply_that_names_no_request_claims_nothing() {
        let pending = pending_with(vec![request(json!(5), "t", 1)]);
        for token in [None, Some("")] {
            assert!(matches!(
                claim_pending_permission(&pending, &json!(5), token, 1),
                PermissionClaim::NotThisRequest
            ));
        }
        assert_eq!(pending.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_request_from_a_connection_that_is_gone_is_dropped_not_answered() {
        // The agent died (no restart drained the map) and a new one is up:
        // the old request can't be answered on the new connection.
        let pending = pending_with(vec![request(json!(5), "t", 1)]);
        assert!(matches!(
            claim_pending_permission(&pending, &json!(5), Some("t"), 3),
            PermissionClaim::Gone
        ));
        assert!(pending.lock().unwrap().is_empty());
    }
}
