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
            .lock()
            .publish(window.label(), &tool_steering);
        return Ok(());
    };
    // Deduplicate per session: skip if THIS session already has (or is
    // receiving) this exact block, and turn an empty block into a
    // withdrawal for sessions that were sent tools (see ToolSteeringState).
    // The delivery settles the claim however it ends.
    let Some(claim) = acp
        .tool_steering
        .lock()
        .claim(window.label(), &session_id, &tool_steering)
    else {
        return Ok(());
    };

    let client = acp.client.clone();
    let steering = acp.tool_steering.clone();

    async_runtime::spawn_blocking(move || {
        deliver_tool_steering(&client, &steering, &session_id, &claim);
    });

    Ok(())
}

/// Absolute time a hidden steering turn gets once sent, tied to its own
/// session: activity on other sessions doesn't extend it (the ACP idle
/// watchdog's does). An "ack" takes a few seconds; past this the agent is
/// stuck on it, and a user prompt may be queued behind it.
pub(crate) const STEERING_TURN_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

/// After a steering turn is cancelled, how long the agent gets to end it
/// before the prompt slot is released regardless.
pub(crate) const STEERING_CANCEL_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

pub(crate) const STEERING_LIMITS: crate::acp_client::PromptLimits =
    crate::acp_client::PromptLimits {
        budget: STEERING_TURN_BUDGET,
        cancel_grace: STEERING_CANCEL_GRACE,
    };

/// Most a user prompt is held for steering in all: one steering turn and
/// its cancel grace, plus a margin (see [`steer_before_user_prompt`]).
pub(crate) const USER_PROMPT_STEERING_LIMIT: std::time::Duration = STEERING_TURN_BUDGET
    .saturating_add(STEERING_CANCEL_GRACE)
    .saturating_add(std::time::Duration::from_secs(1));

/// Shortest steering turn worth starting for a user prompt, as a share of
/// a full turn's budget (4 s of 10 s); with less of its limit left, the
/// prompt goes without (the block is sent before the next one).
const MIN_PROMPT_STEERING_TURN_SHARE: f64 = 0.4;

/// Send a claimed extension tool steering block on `session_id` as a muted
/// prompt (its "ack" never streams into a window), then settle the claim
/// with how that went, so a send that didn't happen or didn't finish is
/// retried. Blocking: waits for any delivery already under way on the
/// session, and skips if a newer claim replaced this one or a user prompt
/// took over meanwhile. The turn itself is bounded by [`STEERING_LIMITS`].
pub(super) fn deliver_tool_steering(
    client: &crate::acp_client::AcpClient,
    steering: &crate::state::ToolSteering,
    session_id: &str,
    claim: &crate::state::SteeringClaim,
) -> crate::state::SteeringOutcome {
    deliver_tool_steering_within(client, steering, session_id, claim, STEERING_LIMITS, None)
}

/// A user prompt's own steering delivery: ended by the prompt's Stop, and
/// fitted into what is left of its steering limit.
struct ForPrompt {
    /// The session's cancel epoch when the user sent.
    cancel_epoch: u64,
    deadline: std::time::Instant,
}

impl ForPrompt {
    fn stopped(&self, client: &crate::acp_client::AcpClient, session_id: &str) -> bool {
        client.cancel_epoch(session_id) != self.cancel_epoch
    }

    /// The budget a steering turn started now would get, or `None` if too
    /// little is left to be worth it.
    fn budget(&self, full: crate::acp_client::PromptLimits) -> Option<std::time::Duration> {
        let left = self
            .deadline
            .saturating_duration_since(std::time::Instant::now());
        let budget = left.saturating_sub(full.cancel_grace).min(full.budget);
        (budget >= full.budget.mul_f64(MIN_PROMPT_STEERING_TURN_SHARE)).then_some(budget)
    }
}

fn deliver_tool_steering_within(
    client: &crate::acp_client::AcpClient,
    steering: &crate::state::ToolSteering,
    session_id: &str,
    claim: &crate::state::SteeringClaim,
    limits: crate::acp_client::PromptLimits,
    for_prompt: Option<&ForPrompt>,
) -> crate::state::SteeringOutcome {
    use crate::acp_client::HiddenPromptOutcome;
    use crate::state::SteeringOutcome;

    let stopped = || for_prompt.is_some_and(|p| p.stopped(client, session_id));
    // The budget to send under right now (`None`: don't).
    let budget = || match for_prompt {
        Some(p) => p.budget(limits),
        None => Some(limits.budget),
    };

    // Before taking the delivery slot: a compaction can take a while, and
    // nothing else should wait on this delivery meanwhile.
    client.wait_for_compaction();
    let slot = steering.begin_delivery(session_id);
    if !steering.lock().is_current(session_id, claim) || stopped() || budget().is_none() {
        info!(
            "Extension tool steering on {} not sent (superseded, stopped or out of time)",
            session_id
        );
        steering.settle(session_id, claim, SteeringOutcome::NotSent);
        return SteeringOutcome::NotSent;
    }
    if !client.is_connected() {
        steering.settle(session_id, claim, SteeringOutcome::NotSent);
        return SteeringOutcome::NotSent;
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

    // The budget it went out with: one cut short to fit a prompt's limit
    // and then timed out says nothing about the agent being slow.
    let sent_budget = std::cell::Cell::new(limits.budget);
    let outcome = match client.send_hidden_prompt_bounded(
        session_id,
        &msg,
        for_prompt.map(|p| p.cancel_epoch),
        // While queued for the session: stop as soon as a newer claim
        // replaces this one, a user prompt takes the steering over, or (for
        // a prompt's own) the user stops or the prompt runs out of time.
        &|| !steering.lock().is_current(session_id, claim) || stopped() || budget().is_none(),
        &mut || {
            if stopped() {
                return None;
            }
            let budget = budget()?;
            if !steering.lock().begin_transmit(session_id, claim) {
                return None;
            }
            sent_budget.set(budget);
            Some(crate::acp_client::PromptLimits { budget, ..limits })
        },
    ) {
        HiddenPromptOutcome::Completed => {
            info!("Extension tool steering sent");
            SteeringOutcome::Delivered
        }
        HiddenPromptOutcome::NotSent(None) => {
            info!(
                "Extension tool steering on {} not sent (superseded, taken over, stopped or out of time)",
                session_id
            );
            SteeringOutcome::NotSent
        }
        HiddenPromptOutcome::NotSent(Some(e)) => {
            warn!("Failed to send extension tool steering: {}", e);
            SteeringOutcome::NotSent
        }
        HiddenPromptOutcome::Rejected(e) => {
            warn!("Agent rejected extension tool steering: {}", e);
            SteeringOutcome::NotSent
        }
        // The agent may or may not have kept it: re-sent before the
        // session's next prompt, whichever thread sends that.
        HiddenPromptOutcome::Stopped => {
            info!(
                "Extension tool steering on {} cancelled; will replay",
                session_id
            );
            SteeringOutcome::Unknown
        }
        HiddenPromptOutcome::Lost(e) => {
            warn!(
                "Extension tool steering on {} lost ({}); will replay",
                session_id, e
            );
            SteeringOutcome::Unknown
        }
        HiddenPromptOutcome::TimedOut(why) => {
            warn!(
                "Extension tool steering on {} timed out ({})",
                session_id, why
            );
            if sent_budget.get() < limits.budget {
                SteeringOutcome::Unknown
            } else {
                SteeringOutcome::TimedOut
            }
        }
    };
    steering.settle(session_id, claim, outcome);
    drop(slot);
    outcome
}

/// Before a user prompt from `window` goes out on `session_id`: get the
/// window's current extension tool steering to the agent first, since the
/// prompt may need the new tools. `cancel_epoch` is the session's cancel
/// epoch when the user sent; a Stop since then ends this. Returns whether
/// steering held the session (or the user stopped), so the caller can
/// honour a Stop pressed meanwhile. Blocking.
///
/// Bounded by `total` in all, and by `limits` per steering turn:
///
/// - Steering only queued (claimed, or waiting for the session's prompt
///   slot) costs no wait: the prompt takes it over and sends it itself,
///   ahead of its own turn, and other deliveries on the session step aside
///   until it is done. That send is fitted into what is left of `total`.
/// - A steering turn already on the wire holds the agent's prompt slot, so
///   it is waited out (the prompt couldn't go before it anyway). Its own
///   deadline ends it; a block whose full-length turn ran out of time isn't
///   retried for prompts.
/// - A Stop ends the wait at once, and no steering turn starts after it.
pub(super) fn steer_before_user_prompt(
    client: &crate::acp_client::AcpClient,
    steering: &crate::state::ToolSteering,
    window: &str,
    session_id: &str,
    cancel_epoch: u64,
    limits: crate::acp_client::PromptLimits,
    total: std::time::Duration,
) -> bool {
    use crate::state::{PromptSteering, SteeringOutcome};
    // Each pass either waits out the turn on the wire or sends the block;
    // more than one is needed only when a turn ends with an older block.
    const PASSES: usize = 4;
    let own = ForPrompt {
        cancel_epoch,
        deadline: std::time::Instant::now() + total,
    };
    let stopped = || own.stopped(client, session_id);
    let _prompting = steering.begin_prompt(session_id);
    let mut held = false;
    for _ in 0..PASSES {
        if stopped() {
            return true;
        }
        let next = steering.lock().claim_for_prompt(window, session_id);
        match next {
            PromptSteering::Ready => return held,
            PromptSteering::Transmitting => {
                held = true;
                if !steering.wait_while_transmitting(session_id, own.deadline, &stopped) {
                    if !stopped() {
                        warn!(
                            "Extension tool steering on {} still sending; prompt goes after it",
                            session_id
                        );
                    }
                    return true;
                }
            }
            PromptSteering::Send(claim) => {
                held = true;
                if own.budget(limits).is_none() {
                    steering.lock().abandon(session_id, &claim);
                    warn!(
                        "No time left to steer {} before the prompt; sending it next time",
                        session_id
                    );
                    return true;
                }
                let outcome = deliver_tool_steering_within(
                    client,
                    steering,
                    session_id,
                    &claim,
                    limits,
                    Some(&own),
                );
                // Unsent because of a Stop or the clock: the next pass
                // would only find the same.
                if outcome == SteeringOutcome::NotSent
                    && (stopped() || own.budget(limits).is_none())
                {
                    return true;
                }
            }
        }
    }
    warn!(
        "Extension tool steering on {} kept changing; sending the prompt",
        session_id
    );
    held
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
    use super::{deliver_tool_steering_within, steer_before_user_prompt};
    use crate::acp_client::{AcpClient, AcpConnectionMode, PromptLimits};
    use crate::state::{SteeringOutcome, ToolSteering};
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// One request the fake agent received.
    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        session: String,
        /// The prompt's text, for session/prompt.
        text: String,
    }

    impl Seen {
        fn is_steering(&self) -> bool {
            self.method == "session/prompt"
                && self
                    .text
                    .contains(crate::commands::system::STEERING_MSG_PREFIX)
        }
    }

    /// A fake agent on loopback (ephemeral port, test-only). Steering
    /// prompts are answered after `steering_reply` (never if `None`, and
    /// session/cancel is ignored: wedged on them); any other prompt is
    /// answered at once. Session "T" streams updates all the while, so the
    /// connection is never idle.
    struct FakeAgent {
        port: u16,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl FakeAgent {
        fn start(steering_reply: Option<Duration>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let log = seen.clone();
            std::thread::spawn(move || {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let writer = Arc::new(Mutex::new(stream.try_clone().unwrap()));
                let ticker = writer.clone();
                let until = Instant::now() + Duration::from_secs(20);
                std::thread::spawn(move || {
                    while Instant::now() < until {
                        let update = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "params": { "sessionId": "T", "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": "." } } }
                        });
                        if writeln!(ticker.lock().unwrap(), "{update}").is_err() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                });
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { return };
                    let msg: serde_json::Value = serde_json::from_str(&line).unwrap();
                    let seen = Seen {
                        method: msg["method"].as_str().unwrap_or("").to_string(),
                        session: msg["params"]["sessionId"]
                            .as_str()
                            .unwrap_or("")
                            .to_string(),
                        text: msg["params"]["prompt"].to_string(),
                    };
                    log.lock().unwrap().push(seen.clone());
                    if seen.method != "session/prompt" {
                        continue;
                    }
                    let reply = serde_json::json!({
                        "jsonrpc": "2.0", "id": msg["id"],
                        "result": { "stopReason": "end_turn" }
                    });
                    let delay = if seen.is_steering() {
                        match steering_reply {
                            Some(d) => d,
                            None => continue,
                        }
                    } else if seen.text.contains("slow title") {
                        // Another prompt holding the session's slot (the
                        // titler, auto-steering, a peer window).
                        Duration::from_millis(400)
                    } else {
                        Duration::ZERO
                    };
                    let w = writer.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(delay);
                        let _ = writeln!(w.lock().unwrap(), "{reply}");
                    });
                }
            });
            Self { port, seen }
        }

        fn client(&self) -> Arc<AcpClient> {
            let client = Arc::new(AcpClient::new(AcpConnectionMode::Remote {
                host: "127.0.0.1".to_string(),
                port: self.port,
            }));
            client.connect().unwrap();
            client
        }

        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }

        /// session/prompt requests on `session`, as "steer:<text>" or "user".
        fn prompts(&self, session: &str) -> Vec<String> {
            self.seen()
                .iter()
                .filter(|s| s.method == "session/prompt" && s.session == session)
                .map(|s| {
                    if s.is_steering() {
                        format!("steer:{}", if s.text.contains("B2") { "B2" } else { "B1" })
                    } else {
                        "user".to_string()
                    }
                })
                .collect()
        }

        fn wait_for(&self, what: &str, pred: impl Fn(&[Seen]) -> bool) {
            let start = Instant::now();
            while !pred(&self.seen()) {
                assert!(
                    start.elapsed() < Duration::from_secs(10),
                    "never saw {what}"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    const LIMITS: PromptLimits = PromptLimits {
        budget: Duration::from_millis(600),
        cancel_grace: Duration::from_millis(200),
    };
    /// What the user path is allowed in all, as in production: one turn,
    /// its grace and a margin.
    const TOTAL: Duration = Duration::from_millis(600 + 200 + 300);

    /// The user send path as send_message_streaming runs it: steering, the
    /// Stop check, then the prompt. Returns whether the prompt was sent.
    fn user_send(
        client: &AcpClient,
        steering: &ToolSteering,
        window: &str,
        session: &str,
        cancel_epoch: u64,
    ) -> bool {
        let held = steer_before_user_prompt(
            client,
            steering,
            window,
            session,
            cancel_epoch,
            LIMITS,
            TOTAL,
        );
        if held && client.cancel_epoch(session) != cancel_epoch {
            return false;
        }
        client.send_chat_streaming(session, "hello", None).unwrap();
        true
    }

    fn background(
        client: &Arc<AcpClient>,
        steering: &Arc<ToolSteering>,
        claim: &crate::state::SteeringClaim,
    ) -> std::thread::JoinHandle<SteeringOutcome> {
        let (c, st, cl) = (client.clone(), steering.clone(), claim.clone());
        std::thread::spawn(move || deliver_tool_steering_within(&c, &st, "S", &cl, LIMITS, None))
    }

    #[test]
    fn stuck_steering_turn_does_not_hold_a_user_prompt_with_another_session_active() {
        // The regression: a steering turn the agent never answered held the
        // session forever (the idle watchdog kept resetting on session T's
        // updates), and the user's prompt waited on it without a deadline.
        let agent = FakeAgent::start(None);
        let client = agent.client();
        let steering = Arc::new(ToolSteering::default());
        let claim = steering.lock().claim("main", "S", "B1 tools").unwrap();
        let delivery = background(&client, &steering, &claim);
        agent.wait_for("the steering turn", |s| s.iter().any(Seen::is_steering));

        let start = Instant::now();
        assert!(user_send(&client, &steering, "main", "S", 0));
        let elapsed = start.elapsed();
        assert!(
            elapsed < TOTAL + Duration::from_millis(500),
            "held for {elapsed:?}"
        );

        assert_eq!(delivery.join().unwrap(), SteeringOutcome::TimedOut);
        let cancels: Vec<_> = agent
            .seen()
            .into_iter()
            .filter(|s| s.method == "session/cancel")
            .map(|s| s.session)
            .collect();
        assert_eq!(
            cancels,
            vec!["S"],
            "only the stuck session's turn is cancelled"
        );
        // The timed-out block isn't retried for the prompt, now or next time.
        assert!(user_send(&client, &steering, "main", "S", 0));
        assert_eq!(agent.prompts("S"), vec!["steer:B1", "user", "user"]);
        // It still counts as sent: turning the tools off withdraws them.
        assert!(steering.lock().claim("main", "S", "").is_some());
    }

    #[test]
    fn prompt_takes_over_queued_steering_and_sends_it_first() {
        let agent = FakeAgent::start(Some(Duration::ZERO));
        let client = agent.client();
        let steering = ToolSteering::default();
        // Claimed by a window, its delivery not started yet.
        let queued = steering.lock().claim("main", "S", "B1 tools").unwrap();
        let start = Instant::now();
        assert!(user_send(&client, &steering, "main", "S", 0));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(agent.prompts("S"), vec!["steer:B1", "user"]);
        // The queued delivery, once it runs, finds it was taken over.
        assert_eq!(
            deliver_tool_steering_within(&client, &steering, "S", &queued, LIMITS, None),
            SteeringOutcome::NotSent
        );
        assert_eq!(agent.prompts("S").len(), 2);
    }

    #[test]
    fn a_newer_block_queued_behind_a_sending_turn_still_goes_before_the_prompt() {
        // The review's case: B1 on the wire, B2 claimed behind it, then the
        // user sends. The prompt must not overtake B2.
        let agent = FakeAgent::start(Some(Duration::from_millis(300)));
        let client = agent.client();
        let steering = Arc::new(ToolSteering::default());
        let b1 = steering.lock().claim("main", "S", "B1 tools").unwrap();
        let d1 = background(&client, &steering, &b1);
        agent.wait_for("B1", |s| s.iter().any(Seen::is_steering));
        let b2 = steering.lock().claim("main", "S", "B2 tools").unwrap();
        let d2 = background(&client, &steering, &b2);

        assert!(user_send(&client, &steering, "main", "S", 0));
        assert_eq!(d1.join().unwrap(), SteeringOutcome::Delivered);
        assert_eq!(d2.join().unwrap(), SteeringOutcome::NotSent, "taken over");
        assert_eq!(agent.prompts("S"), vec!["steer:B1", "steer:B2", "user"]);
        assert!(steering.lock().claim_latest("main", "S").is_none());
    }

    #[test]
    fn stop_while_a_prompt_waits_on_steering_sends_nothing_more() {
        // The review's case: Stop pressed while the prompt waited on a
        // steering turn. Neither a fresh steering turn nor the prompt may
        // go out after it.
        let agent = FakeAgent::start(None);
        let client = agent.client();
        let steering = Arc::new(ToolSteering::default());
        let claim = steering.lock().claim("main", "S", "B1 tools").unwrap();
        let delivery = background(&client, &steering, &claim);
        agent.wait_for("the steering turn", |s| s.iter().any(Seen::is_steering));

        let epoch = client.cancel_epoch("S");
        let c = client.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            // What cancel_generation does on Stop.
            c.note_cancel("S");
            c.cancel_session("S").unwrap();
        });
        let start = Instant::now();
        assert!(
            !user_send(&client, &steering, "main", "S", epoch),
            "prompt sent after Stop"
        );
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "{:?}",
            start.elapsed()
        );
        stopper.join().unwrap();
        assert_eq!(delivery.join().unwrap(), SteeringOutcome::Unknown);
        assert_eq!(agent.prompts("S"), vec!["steer:B1"]);
        // The slot is free again, and a Stop that cut short the session's
        // first tool list still leaves it one that "no tools" applies to.
        client.send_chat_streaming("S", "later", None).unwrap();
        assert!(steering.lock().claim("main", "S", "").is_some());
    }

    #[test]
    fn stop_while_a_prompts_own_steering_waits_for_the_slot_starts_no_turn() {
        // The second review's case: the prompt's own steering is queued
        // behind another prompt on the session when the user presses Stop.
        // It must give up, not start a hidden turn once the slot frees.
        let agent = FakeAgent::start(Some(Duration::ZERO));
        let client = agent.client();
        let steering = ToolSteering::default();
        steering.lock().publish("main", "B1 tools");
        let c = client.clone();
        let other = std::thread::spawn(move || c.send_chat_streaming("S", "slow title", None));
        agent.wait_for("the other prompt", |s| {
            s.iter().any(|s| s.text.contains("slow title"))
        });

        let epoch = client.cancel_epoch("S");
        let c = client.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            c.note_cancel("S");
        });
        assert!(
            !user_send(&client, &steering, "main", "S", epoch),
            "prompt sent after Stop"
        );
        stopper.join().unwrap();
        other.join().unwrap().unwrap();
        assert_eq!(
            agent.prompts("S"),
            vec!["user"],
            "only the other prompt went out"
        );
        // Not sent, so the next prompt sends it.
        assert!(user_send(
            &client,
            &steering,
            "main",
            "S",
            client.cancel_epoch("S")
        ));
        assert_eq!(agent.prompts("S"), vec!["user", "steer:B1", "user"]);
    }

    #[test]
    fn a_turn_shortened_to_fit_a_prompt_that_times_out_is_retried() {
        // Cut to fit what was left of a prompt's limit: running out of that
        // says nothing about the agent being too slow.
        let agent = FakeAgent::start(None);
        let client = agent.client();
        let steering = ToolSteering::default();
        steering.lock().publish("main", "B1 tools");
        let crate::state::PromptSteering::Send(claim) =
            steering.lock().claim_for_prompt("main", "S")
        else {
            panic!("nothing to send");
        };
        let own = super::ForPrompt {
            cancel_epoch: client.cancel_epoch("S"),
            deadline: Instant::now() + Duration::from_millis(500),
        };
        let guard = steering.begin_prompt("S");
        let outcome =
            deliver_tool_steering_within(&client, &steering, "S", &claim, LIMITS, Some(&own));
        drop(guard);
        assert_eq!(outcome, SteeringOutcome::Unknown);
        assert!(matches!(
            steering.lock().claim_for_prompt("main", "S"),
            crate::state::PromptSteering::Send(_)
        ));
    }

    #[test]
    fn steering_without_a_connection_is_not_sent_and_is_retried() {
        // Never connected.
        let client = AcpClient::new(AcpConnectionMode::Remote {
            host: "127.0.0.1".to_string(),
            port: 9,
        });
        let steering = ToolSteering::default();
        let claim = steering.lock().claim("main", "S", "tools").unwrap();
        let outcome = super::deliver_tool_steering(&client, &steering, "S", &claim);
        assert_eq!(outcome, SteeringOutcome::NotSent);
        // Nothing was transmitted, so there's nothing to withdraw...
        assert!(steering.lock().claim("main", "S", "").is_none());
        // ...and with the tools back they are claimed again.
        assert!(steering.lock().claim("main", "S", "tools").is_some());
    }

    #[test]
    fn prompt_does_not_wait_when_there_is_no_steering() {
        let client = AcpClient::new(AcpConnectionMode::Remote {
            host: "127.0.0.1".to_string(),
            port: 9,
        });
        let steering = ToolSteering::default();
        let long = Duration::from_secs(60);
        let limits = PromptLimits {
            budget: long,
            cancel_grace: long,
        };
        assert!(!steer_before_user_prompt(
            &client, &steering, "main", "S", 0, limits, long
        ));
        // A window with no tools, and a session never sent any.
        steering.lock().publish("main", "");
        assert!(!steer_before_user_prompt(
            &client, &steering, "main", "S", 0, limits, long
        ));
    }
}
