//! Prompt streaming: send/cancel a message, connection check/reconnect,
//! and opening a chat window pre-seeded with a message.

use super::*;

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri commands take state via parameters; can't be condensed.
pub async fn send_message_streaming<R: tauri::Runtime>(
    session_id: String,
    message: String,
    attachments: Option<Vec<serde_json::Value>>,
    // The window's id for this prompt, echoed as `turnId` on its
    // MESSAGE_ERROR so the window matches the error to this turn, not to a
    // newer one on the same session. Absent (an extension's send): its
    // errors name no turn and are never resent.
    turn_id: Option<String>,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    ui: State<'_, UiState>,
    window: WebviewWindow<R>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    // Never log message *content* by default — app.jsonl is routinely shared
    // in bug reports. Full-content logging is a developer aid for working on
    // Kage itself, gated behind the log_message_content opt-in
    // (Settings → Developer, off by default). Default path logs length only.
    if features.config.lock_or_recover().system.log_message_content {
        info!(
            "Sending message on {} ({} chars): {}",
            session_id,
            message.chars().count(),
            message
        );
    } else {
        info!(
            "Sending message on {} ({} chars)",
            session_id,
            message.chars().count()
        );
    }
    // A user prompt stops a running background agent call; waiting ones
    // stay, held back until the user is away again.
    features.agent_calls.on_user_prompt_started();
    let client = acp.client.clone();
    let config = features.config.clone();
    let config_for_title = features.config.clone();
    let session_cache_for_send = features.session_cache.clone();
    let app_for_send = app.clone();
    let window_label = window.label().to_string();
    let originators = ui.pending_prompt_originators.clone();
    let tool_steering = acp.tool_steering.clone();

    // Tag this session as originated from this window. The permission
    // handler reads this map to decide which window the modal goes to;
    // we clear it on every send termination (success or error).
    if let Ok(mut m) = originators.lock() {
        m.insert(session_id.clone(), window_label.clone());
    }

    // Record the prompt text for mid-stream switch-in: the turn isn't on
    // disk until it completes, so a viewport attaching to this session
    // mid-turn reads the user's message from here. Cleared in the same
    // epilogues that clear the originator entry.
    acp.client.set_in_flight_prompt(&session_id, &message);

    // Tell chat hosts a user turn just started on this session so they
    // can badge it in the sidebar even when they're viewing another
    // session. Fired only for real user prompts — steering/titling
    // prompts go through other paths and stay invisible. The matching
    // terminal signals are MESSAGE_COMPLETE (broadcast) on success and
    // SESSION_ACTIVITY kind="failed" on send failure — the latter exists
    // because MESSAGE_ERROR is emitted only to the originating window
    // and carries no session id, which would leave other windows'
    // activity badges spinning forever.
    crate::event_targets::emit_streaming_audience(
        &app,
        events::SESSION_ACTIVITY,
        &serde_json::json!({
            "sessionId": &session_id,
            "source": &window_label,
            "kind": "started",
        }),
    );

    let turn_id = turn_id.filter(|t| !t.trim().is_empty());

    async_runtime::spawn_blocking(move || {
        let client_arc = client.clone();
        let config_arc = config.clone();

        if !client.is_connected() {
            if let Err(e) = client.connect() {
                crate::event_targets::emit_to_self(
                    &window,
                    events::MESSAGE_ERROR,
                    &connect_failed_payload(&e.to_string(), Some(&session_id), turn_id.as_deref()),
                );
                // The turn started above ends here: badges in other windows.
                crate::event_targets::emit_streaming_audience(
                    &app_for_send,
                    events::SESSION_ACTIVITY,
                    &serde_json::json!({ "sessionId": &session_id, "kind": "failed" }),
                );
                if let Ok(mut m) = originators.lock() {
                    m.remove(&session_id);
                }
                client.clear_in_flight_prompt(&session_id);
                return;
            }
        }

        // The notification handler (set up at app init) handles all streaming
        // chunks, permissions, and tool calls via Tauri events.
        let had_attachments = attachments.as_ref().is_some_and(|a| !a.is_empty());
        let crate::acp_client::PromptReport {
            result: send_result,
            session_id: failed_on,
            resendable,
            outcome,
        } = send_user_prompt_report(
            &client,
            &tool_steering,
            &window_label,
            &session_id,
            message,
            attachments,
        );

        // Recovery may have moved us to a fresh session; pick that up
        // for the post-send epilogue (auto_steering, originator clear).
        let active_session_id = match &send_result {
            Ok(id) => id.clone(),
            Err(_) => session_id.clone(),
        };

        // Every chunk of this turn is already queued in the batcher (the
        // reader handles them before the prompt response); deliver the
        // tail now so no terminal event below can overtake it.
        super::notifications::flush_session_chunks(&app_for_send, &client, &active_session_id);
        if active_session_id != session_id {
            super::notifications::flush_session_chunks(&app_for_send, &client, &session_id);
        }

        if let Err(e) = send_result {
            let error_str = format!("{}", e);
            info!(
                "Prompt on {} failed (outcome {:?}, resendable {})",
                failed_on, outcome, resendable
            );
            // A sign-in demand, or the agent still failing after an
            // automatic restart: say so, and keep the image heuristic below
            // from restarting a second time and dropping the session.
            let for_user = prompt_error_for_user(&e, &config.lock_or_recover());
            let is_image_error = for_user.is_none()
                && had_attachments
                && (error_str.contains("Internal error")
                    || error_str.contains("image")
                    || error_str.contains("unsupported")
                    || error_str.contains("response stream"));

            // The ACP connection is likely stuck after an image error, and
            // an image error corrupts the conversation, so we want a
            // *fresh* session. Not by cutting off another window's turn,
            // though: its recovery would resend work it may have done.
            // This turn is over, so its own prompt no longer counts as
            // work. `Some(new id, or None if the reset failed)` once reset.
            let reset = if is_image_error {
                info!("Image-related error detected — resetting ACP connection and session");
                client.clear_in_flight_prompt(&session_id);
                match client.restart_with_fresh_session_if_idle(Some(&session_id)) {
                    Ok(Some(id)) => Some(Some(id)),
                    Ok(None) => {
                        warn!(
                            "Not resetting the agent after the image error on {}: it has other work in flight",
                            session_id
                        );
                        None
                    }
                    Err(e) => {
                        error!("Failed to reset connection after image error: {}", e);
                        Some(None)
                    }
                }
            } else {
                None
            };

            if let Some(new_session_id) = reset {
                // Tell every streaming-aware window; any window pinned
                // to the dead `oldSessionId` adopts `newSessionId`. Per-
                // window emit was correct in the single-session world but
                // would leave a peer window holding a dead id when both
                // windows are pinned to the same session.
                crate::event_targets::emit_streaming_audience(
                    &app_for_send,
                    "session_reset",
                    &serde_json::json!({
                        "reason": "image_unsupported",
                        "reconnected": new_session_id.is_some(),
                        "oldSessionId": &session_id,
                        "newSessionId": new_session_id,
                    }),
                );
            } else {
                crate::event_targets::emit_to_self(
                    &window,
                    events::MESSAGE_ERROR,
                    &failed_prompt_payload(
                        for_user,
                        &error_str,
                        &failed_on,
                        resendable,
                        turn_id.as_deref(),
                    ),
                );
            }
            // Clear activity badges in windows that aren't the originator
            // (they only hear the broadcast events, not MESSAGE_ERROR).
            crate::event_targets::emit_streaming_audience(
                &app_for_send,
                events::SESSION_ACTIVITY,
                &serde_json::json!({
                    "sessionId": &session_id,
                    "kind": "failed",
                }),
            );
            if let Ok(mut m) = originators.lock() {
                m.remove(&session_id);
            }
            client.clear_in_flight_prompt(&session_id);
            return;
        }

        // Broadcast — any window pinned to the same session needs to
        // hear the complete to drop its "thinking" indicator and run
        // its post-completion actions. Per-window emit was correct
        // in the single-session world but leaves peer windows hung
        // when two chat windows share a session.
        //
        // Include both `sessionId` (the active session, possibly
        // post-recovery) and `oldSessionId` (the session the send
        // was issued against) so peer windows can match either:
        //  - peers on the active session see sessionId == theirs
        //  - peers stuck on the pre-recovery session see
        //    oldSessionId == theirs and adopt the new id
        crate::event_targets::emit_streaming_audience(
            &app_for_send,
            events::MESSAGE_COMPLETE,
            &serde_json::json!({
                "sessionId": &active_session_id,
                "oldSessionId": &session_id,
            }),
        );

        // Evict this session's server-side streaming accumulator now that the
        // turn is done. The interactive frontend built its own copy from the
        // streamed chunk events and never reads this bucket back, so once
        // MESSAGE_COMPLETE is out it's dead weight. Without this, the final
        // response of every session the user touches stays resident (up to the
        // per-session cap) until the *next* prompt on that session — which for
        // a backgrounded session may be never. Auto-steering below re-resets
        // the bucket before its own send, so clearing here is safe.
        client_arc.reset_session_accumulator(&active_session_id);
        client_arc.clear_in_flight_prompt(&active_session_id);
        if active_session_id != session_id {
            client_arc.reset_session_accumulator(&session_id);
            client_arc.clear_in_flight_prompt(&session_id);
        }

        // Refresh the window title now that there's a user message
        // available — fresh sessions get their first real title here.
        // Invalidate the session cache first so the JSONL re-extract
        // sees the new content rather than the cached "New Chat".
        crate::commands::sessions::invalidate_session_cache(&session_cache_for_send);
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

        // Background AI title summarisation. No-op if the session
        // already has a Manual or Ai title cached. Runs first because
        // it's quick and user-visible (window/sidebar titles flip);
        // auto-steering takes longer and runs less often.
        crate::commands::sessions::maybe_generate_ai_title(
            app_for_send.clone(),
            client_arc.clone(),
            session_cache_for_send.clone(),
            active_session_id.clone(),
        );

        // Trigger auto-steering generation periodically on the session
        // the message just landed on (post-recovery if applicable).
        crate::auto_steering::maybe_generate_steering(client_arc, config_arc, active_session_id);
    });

    Ok(())
}

#[tauri::command]
pub async fn check_connection(acp: State<'_, AcpHandles>) -> Result<bool, AppError> {
    Ok(acp.client.is_connected())
}

/// What `reconnect_acp` did.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconnectOutcome {
    /// The agent was restarted and answered `initialize`.
    pub reconnected: bool,
    /// The window's session after the reconnect, if it has one.
    pub session_id: Option<String>,
    /// That session was loaded on the restarted agent. False: the next
    /// prompt's recovery ladder reloads or replaces it.
    pub session_reloaded: bool,
}

/// Reconnect: restart the agent (even one that looks alive: a live but
/// wedged agent is exactly when the user clicks this), then reload the
/// window's session on it. The window resends its failed prompt itself.
/// Refused while a live agent has work in flight (any window's turn, a
/// background call, an automation).
#[tauri::command]
pub async fn reconnect_acp<R: tauri::Runtime>(
    session_id: Option<String>,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    ui: State<'_, UiState>,
    window: WebviewWindow<R>,
    app: tauri::AppHandle<R>,
) -> Result<ReconnectOutcome, AppError> {
    let client = acp.client.clone();
    let available_models = acp.available_models.clone();
    let config = features.config.clone();
    let session_cache = features.session_cache.clone();
    let window_sessions = ui.window_sessions.clone();
    let label = window.label().to_string();

    // The restart is a synchronous respawn + handshake (Remote mode: up to
    // 6 dial attempts × 5 s plus sleeps) — keep it off the Tokio workers.
    async_runtime::spawn_blocking(move || -> Result<ReconnectOutcome, AppError> {
        // A stale Reconnect (an old error still on screen) must not kill a
        // live agent mid-turn: the killed turn would lose its connection
        // and the recovery ladder would resend it, repeating its tools.
        // The check and the teardown are one step: a prompt another window
        // sends meanwhile waits for the new agent instead of being cut off.
        info!("Reconnect requested by {}", label);
        match client.restart_if_idle() {
            Ok(crate::acp_client::RestartIfIdle::Restarted) => {
                info!("Reconnect for {}: agent restarted", label);
                client.clear_unreachable_latch();
            }
            Ok(crate::acp_client::RestartIfIdle::Busy) => {
                info!(
                    "Reconnect requested by {}: not restarting, the agent has work in flight",
                    label
                );
                return Err(AppError::keyed(
                    ErrorKind::ConnectionLost,
                    "errors.connection.reconnect_busy",
                    &[],
                ));
            }
            Err(e) => {
                // The user asked for a fresh start: the next failing prompt
                // may restart the agent again.
                client.clear_unreachable_latch();
                return Err(session_setup_error(&e, &config, "Reconnect", |e| {
                    AppError::keyed(
                        ErrorKind::ConnectionLost,
                        "errors.connection.reconnect_failed",
                        &[("reason", &e.to_string())],
                    )
                }));
            }
        }

        let session_id = session_id
            .filter(|s| !s.trim().is_empty())
            .or_else(|| window_sessions.lock_or_recover().get(&label).cloned());
        let Some(id) = session_id else {
            info!(
                "Reconnect for {}: agent restarted, no session to reload",
                label
            );
            return Ok(ReconnectOutcome {
                reconnected: true,
                session_id: None,
                session_reloaded: false,
            });
        };
        match crate::commands::sessions::switch_acp_session_blocking(
            Some(id.clone()),
            client,
            available_models,
            config,
            session_cache,
            window_sessions,
            label.clone(),
            app,
        ) {
            Ok(loaded) => {
                info!("Reconnect for {}: reloaded session {}", label, loaded);
                Ok(ReconnectOutcome {
                    reconnected: true,
                    session_id: Some(loaded),
                    session_reloaded: true,
                })
            }
            Err(e) if matches!(e.kind, ErrorKind::AuthRequired) => Err(e),
            Err(e) => {
                warn!(
                    "Reconnect for {}: agent restarted but session {} did not reload: {}",
                    label, id, e
                );
                Ok(ReconnectOutcome {
                    reconnected: true,
                    session_id: Some(id),
                    session_reloaded: false,
                })
            }
        }
    })
    .await
    .map_err(|e| {
        AppError::keyed(
            ErrorKind::ConnectionLost,
            "errors.connection.reconnect_failed",
            &[("reason", &e.to_string())],
        )
    })?
}

/// Whether the active connection is a Local Kiro agent, so a sign-in hint
/// can name its command. A Remote connection's agent runs on a machine Kage
/// can't name.
fn is_local_kiro(cfg: &crate::config::Config) -> bool {
    crate::agent_presets::explicit_kind(cfg) == Some(crate::agent_presets::AgentKind::Kiro)
        && matches!(cfg.acp.active_mode(), crate::config::AcpMode::Local { .. })
}

/// The command that signs in to Kiro.
const KIRO_LOGIN_COMMAND: &str = "kiro-cli login";

/// "Your agent needs you to sign in", naming `kiro-cli login` for a Local
/// Kiro connection.
pub(crate) fn auth_required_error(cfg: &crate::config::Config) -> AppError {
    if is_local_kiro(cfg) {
        AppError::keyed(
            ErrorKind::AuthRequired,
            "errors.agent.auth_required_kiro",
            &[("command", KIRO_LOGIN_COMMAND)],
        )
    } else {
        AppError::keyed(ErrorKind::AuthRequired, "errors.agent.auth_required", &[])
    }
}

/// The sign-in error for `e` when it is the agent's ACP "authentication
/// required" (`-32000`) answer, to `initialize`, `session/new`,
/// `session/load` or a prompt, with the agent's own text; `None` for any
/// other error. The one place that rule lives: prompt failures
/// (`prompt_error_for_user`) and the commands that set up a session
/// (`session_setup_error`) both use it.
pub(crate) fn auth_required_for_user(
    e: &anyhow::Error,
    cfg: &crate::config::Config,
) -> Option<(AppError, String)> {
    crate::acp_client::types::is_auth_required(e).then(|| (auth_required_error(cfg), e.to_string()))
}

/// What a command that sets up a session for a window (create, load, the
/// connect or restart before either) returns for `e`: the sign-in error when
/// the agent wants one, so the window can offer Sign in; else
/// `otherwise(e)`, as before. `AppError` has no details field, so the
/// agent's own text is logged (English) instead: the localized message
/// stands on its own.
pub(crate) fn session_setup_error(
    e: &anyhow::Error,
    config: &std::sync::Mutex<crate::config::Config>,
    what: &str,
    otherwise: impl FnOnce(&anyhow::Error) -> AppError,
) -> AppError {
    match auth_required_for_user(e, &config.lock_or_recover()) {
        Some((err, detail)) => {
            warn!("{} refused: the agent wants a sign-in: {}", what, detail);
            err
        }
        None => otherwise(e),
    }
}

/// What to tell the user about a failed prompt, when the recovery ladder
/// gave it a meaning: the agent wants a sign-in (ACP `-32000`, from the
/// prompt or from the restart's `initialize` / `session/*`), or it still
/// failed after an automatic restart. The second value is the agent's raw
/// text, shown as a details line. `None`: show the error as it is.
fn prompt_error_for_user(
    e: &anyhow::Error,
    cfg: &crate::config::Config,
) -> Option<(AppError, String)> {
    use crate::acp_client::types::PromptRecoveryError;
    if let Some(PromptRecoveryError::AgentUnreachable { detail }) =
        e.downcast_ref::<PromptRecoveryError>()
    {
        let err = if is_local_kiro(cfg) {
            AppError::keyed(
                ErrorKind::AgentUnreachable,
                "errors.agent.unreachable_after_restart_kiro",
                &[("command", KIRO_LOGIN_COMMAND)],
            )
        } else {
            AppError::keyed(
                ErrorKind::AgentUnreachable,
                "errors.agent.unreachable_after_restart",
                &[],
            )
        };
        return Some((err, detail.clone()));
    }
    auth_required_for_user(e, cfg)
}

/// The MESSAGE_ERROR payload for a user prompt that failed:
/// `{ kind, key, message, detail, sessionId, resendable, turnId }`,
/// `message` in the active locale.
///
/// - `sessionId`: the session the failed turn last ran on (recovery may
///   have moved it; windows adopted that id from `session_migrated`).
/// - `resendable`: whether a window may send the prompt again as it is
///   (Reconnect does): true only when every attempt is known to have left
///   the agent untouched (answered with an error, or never sent; never
///   after a timeout or a connection lost after the write) and none of
///   them streamed anything, ran or updated a tool, asked for permission
///   or made an `fs/*` / `terminal/*` request, was rate limited or stopped
///   (`PromptReport::resendable`). Absent means false.
/// - `turnId`: the id the window sent the prompt with (null if it gave
///   none). A window attributes the error, and any resend, to the turn of
///   that id only; one naming no turn is never resent.
///
/// `for_user` is what `prompt_error_for_user` made of the error, if
/// anything (`detail` is then the agent's raw text); otherwise the message
/// is the error's own text, as the plain-string payload used to carry it.
/// MESSAGE_ERRORs that aren't a failed prompt stay plain strings.
fn failed_prompt_payload(
    for_user: Option<(AppError, String)>,
    error_text: &str,
    session_id: &str,
    resendable: bool,
    turn_id: Option<&str>,
) -> serde_json::Value {
    let (err, detail) = match for_user {
        Some((err, detail)) => (err, Some(detail)),
        None => (
            AppError::raw(
                plain_prompt_error_kind(error_text),
                format!("Failed to send: {}", error_text),
            ),
            None,
        ),
    };
    serde_json::json!({
        "kind": err.kind,
        "key": err.key,
        "message": err.localised_message(),
        "detail": detail,
        "sessionId": session_id,
        "resendable": resendable,
        "turnId": turn_id,
    })
}

/// The MESSAGE_ERROR payload for a user prompt that failed before it was
/// sent (no connection, no session to send it on): the same shape as
/// `failed_prompt_payload`, never resendable.
fn unsent_prompt_payload(
    kind: ErrorKind,
    message: String,
    session_id: Option<&str>,
    turn_id: Option<&str>,
) -> serde_json::Value {
    unsent_error_payload(AppError::raw(kind, message), session_id, turn_id)
}

/// `unsent_prompt_payload` for an error that already has its kind and
/// message (a signed-out agent's `auth_required`, which gets Sign in).
fn unsent_error_payload(
    err: AppError,
    session_id: Option<&str>,
    turn_id: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "kind": err.kind,
        "key": err.key,
        "message": err.localised_message(),
        "detail": serde_json::Value::Null,
        "sessionId": session_id,
        "resendable": false,
        "turnId": turn_id,
    })
}

/// `unsent_prompt_payload` for a connect that failed. The message keeps
/// its "Unable to connect: " text, which older windows still look for.
fn connect_failed_payload(
    reason: &str,
    session_id: Option<&str>,
    turn_id: Option<&str>,
) -> serde_json::Value {
    unsent_prompt_payload(
        ErrorKind::ConnectionLost,
        format!("Unable to connect: {}", reason),
        session_id,
        turn_id,
    )
}

/// The `kind` of a failed prompt's error that the recovery ladder gave no
/// meaning to, from its text.
fn plain_prompt_error_kind(text: &str) -> ErrorKind {
    let lower = text.to_lowercase();
    if [
        "rate limit",
        "ratelimit",
        "too many requests",
        "429",
        "throttl",
    ]
    .iter()
    .any(|t| lower.contains(t))
    {
        ErrorKind::RateLimited
    } else if lower.contains("timeout") {
        ErrorKind::Timeout
    } else if [
        "connection lost",
        "broken pipe",
        "no write handle",
        "no reader thread",
    ]
    .iter()
    .any(|t| lower.contains(t))
    {
        ErrorKind::ConnectionLost
    } else {
        ErrorKind::Internal
    }
}

#[tauri::command]
pub async fn cancel_generation(
    session_id: String,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    procs: State<'_, crate::state::ChildProcesses>,
) -> Result<(), AppError> {
    // Signal automation plan loop to stop
    features
        .automation_plan_cancelled
        .store(true, std::sync::atomic::Ordering::Relaxed);
    // A command this turn asked to run outside its sandbox that is still
    // waiting for the user: the agent won't read its result now.
    procs.host_commands.cancel_session(&session_id);

    acp.client.cancel_session(&session_id).map_err(|e| {
        AppError::keyed(
            ErrorKind::ConnectionLost,
            "errors.cancel.failed",
            &[("reason", &e.to_string())],
        )
    })?;

    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri commands take state via parameters; can't be condensed.
pub async fn open_chat_with_message<R: tauri::Runtime>(
    session_id: Option<String>,
    message: String,
    // The caller's id for this prompt (see `send_message_streaming`); one
    // is minted when it gives none. The main window learns it from
    // `initial_message` and matches the prompt's MESSAGE_ERROR by it.
    turn_id: Option<String>,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    ui: State<'_, UiState>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    if let Some(floating) = app.get_webview_window(window_labels::FLOATING) {
        let _ = floating.hide();
    }
    if let Some(main) = app.get_webview_window(window_labels::MAIN) {
        // Center on the active monitor
        crate::commands::window::center_window_on_active_monitor(&main);
        let _ = main.show();
        let _ = main.set_focus();
        let turn_id = turn_id
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        // Targeted emit: send to the main window only. Plain
        // `main.emit(...)` looks targeted but actually broadcasts to
        // every webview that has a listener — `emit_to` with a
        // WebviewWindow target is the one that actually scopes the
        // dispatch. `turnId` names the turn main records for it.
        let _ = app.emit_to(
            tauri::EventTarget::webview_window(window_labels::MAIN),
            "initial_message",
            serde_json::json!({ "message": &message, "turnId": &turn_id }),
        );

        // Nothing to send (e.g. the inline-assist icon clicked with no
        // selection) — opening the window was the whole job. Sending would
        // create a session and an empty prompt nobody is waiting on.
        if message.trim().is_empty() {
            return Ok(());
        }

        // Main had no pinned session when the caller passed none; the one
        // created below must be pinned and announced or main drops every
        // chunk of the reply as another session's background activity.
        let needs_pin = !matches!(session_id.as_deref(), Some(s) if !s.trim().is_empty());
        let client = acp.client.clone();
        let window = main.clone();
        let originators = ui.pending_prompt_originators.clone();
        let window_sessions = ui.window_sessions.clone();
        let config = features.config.clone();
        let session_cache = features.session_cache.clone();
        let tool_steering = acp.tool_steering.clone();

        async_runtime::spawn_blocking(move || {
            if !client.is_connected() {
                if let Err(e) = client.connect() {
                    crate::event_targets::emit_to_self(
                        &window,
                        events::MESSAGE_ERROR,
                        &connect_failed_payload(
                            &e.to_string(),
                            session_id.as_deref().filter(|s| !s.trim().is_empty()),
                            Some(&turn_id),
                        ),
                    );
                    return;
                }
            }
            // Resolve a real session — the main window may not have pinned
            // one yet (inline-assist's "inform" mode can fire before main
            // was ever opened). Must happen after connect so create works.
            let session_id = match resolve_or_create_session(&client, &config, session_id) {
                Ok(id) => id,
                Err(e) => {
                    // A signed-out agent: the error as it is, so main
                    // offers Sign in.
                    let payload = if matches!(e.kind, ErrorKind::AuthRequired) {
                        unsent_error_payload(e, None, Some(&turn_id))
                    } else {
                        unsent_prompt_payload(
                            plain_prompt_error_kind(&e.to_string()),
                            format!("Failed to send: {}", e),
                            None,
                            Some(&turn_id),
                        )
                    };
                    crate::event_targets::emit_to_self(&window, events::MESSAGE_ERROR, &payload);
                    return;
                }
            };
            if needs_pin {
                // Same pin `set_window_session` does, then tell main to adopt
                // the id. Emitted before the prompt goes out, so it lands
                // ahead of the first chunk.
                window_sessions
                    .lock_or_recover()
                    .insert(window_labels::MAIN.to_string(), session_id.clone());
                crate::commands::sessions::update_window_title(
                    window.app_handle(),
                    &config,
                    &session_cache,
                    window_labels::MAIN,
                    &session_id,
                );
                crate::event_targets::emit_to_self(
                    &window,
                    "initial_message_session",
                    &serde_json::json!({ "sessionId": &session_id }),
                );
            }
            // Tag the in-flight prompt so permission notifications route to
            // the main chat window, not the (now hidden) floating one.
            if let Ok(mut m) = originators.lock() {
                m.insert(session_id.clone(), window_labels::MAIN.to_string());
            }
            // Same mid-stream switch-in support as send_message_streaming.
            client.set_in_flight_prompt(&session_id, &message);
            let crate::acp_client::PromptReport {
                result,
                session_id: failed_on,
                resendable,
                outcome,
            } = send_user_prompt_report(
                &client,
                &tool_steering,
                window_labels::MAIN,
                &session_id,
                message,
                None,
            );
            let active_session_id = match &result {
                Ok(id) => id.clone(),
                Err(_) => session_id.clone(),
            };
            // Deliver the batched tail before the terminal event; see the
            // matching flush in send_message_streaming.
            let app = window.app_handle();
            super::notifications::flush_session_chunks(app, &client, &active_session_id);
            if active_session_id != session_id {
                super::notifications::flush_session_chunks(app, &client, &session_id);
            }
            if let Err(e) = result {
                info!(
                    "Prompt on {} failed (outcome {:?}, resendable {})",
                    failed_on, outcome, resendable
                );
                let for_user = prompt_error_for_user(&e, &config.lock_or_recover());
                crate::event_targets::emit_to_self(
                    &window,
                    events::MESSAGE_ERROR,
                    &failed_prompt_payload(
                        for_user,
                        &e.to_string(),
                        &failed_on,
                        resendable,
                        Some(&turn_id),
                    ),
                );
                if let Ok(mut m) = originators.lock() {
                    m.remove(&session_id);
                }
                client.clear_in_flight_prompt(&session_id);
                return;
            }
            // Streaming-audience emit — peer windows pinned to the same
            // session need to drop their thinking indicator. See the
            // parallel emit in send_message_streaming for the reasoning.
            crate::event_targets::emit_streaming_audience(
                app,
                events::MESSAGE_COMPLETE,
                &serde_json::json!({
                    "sessionId": &active_session_id,
                    "oldSessionId": &session_id,
                }),
            );
            if let Ok(mut m) = originators.lock() {
                m.remove(&active_session_id);
                if active_session_id != session_id {
                    m.remove(&session_id);
                }
            }
            client.clear_in_flight_prompt(&active_session_id);
            if active_session_id != session_id {
                client.clear_in_flight_prompt(&session_id);
            }
        });
    }
    Ok(())
}

/// Send a user prompt from `window` on `session_id`, with recovery (see
/// `AcpClient::send_chat_streaming_with_recovery`). Returns the session the
/// prompt went out on. Blocking.
///
/// The window's extension tool steering rides on it, as a hidden block ahead
/// of the user's text, when the session it goes to doesn't have that block
/// yet: the window's current tool list, or a "tools withdrawn" notice for a
/// session that may hold tools ([`ToolSteeringPrefix`]). Not on a slash
/// command (`/usage`), which the agent needs as typed: the block stays
/// pending for the next prompt. Nor do `commands/execute` slash commands
/// (`execute_slash_command`) touch it, as they aren't prompts.
fn send_user_prompt_report(
    client: &crate::acp_client::AcpClient,
    tool_steering: &std::sync::Mutex<crate::state::ToolSteeringState>,
    window: &str,
    session_id: &str,
    message: String,
    attachments: Option<Vec<serde_json::Value>>,
) -> crate::acp_client::PromptReport {
    let steering = ToolSteeringPrefix {
        state: tool_steering,
        window,
        attempt: std::cell::RefCell::new(None),
    };
    client.send_chat_streaming_with_recovery_report(
        session_id.to_string(),
        message,
        attachments,
        Some(&steering),
    )
}

/// `send_user_prompt_report`'s result alone.
#[cfg(test)]
fn send_user_prompt(
    client: &crate::acp_client::AcpClient,
    tool_steering: &std::sync::Mutex<crate::state::ToolSteeringState>,
    window: &str,
    session_id: &str,
    message: String,
    attachments: Option<Vec<serde_json::Value>>,
) -> anyhow::Result<String> {
    send_user_prompt_report(
        client,
        tool_steering,
        window,
        session_id,
        message,
        attachments,
    )
    .result
}

/// A user prompt's extension tool steering, decided per send attempt (the
/// recovery ladder may resend on a reloaded or fresh session), under the
/// session's prompt lock. The block is noted when the prompt carrying it is
/// written and counts as delivered once the agent finishes that turn; a send
/// that fails or is stopped leaves it pending, so the next prompt carries it
/// again.
struct ToolSteeringPrefix<'a> {
    state: &'a std::sync::Mutex<crate::state::ToolSteeringState>,
    window: &'a str,
    /// This attempt's session, the steering it carries, and its write once
    /// it's on the wire.
    attempt: std::cell::RefCell<Option<(String, crate::state::PromptSteering, Option<u64>)>>,
}

impl crate::acp_client::HiddenPrefix for ToolSteeringPrefix<'_> {
    fn text_for(&self, session_id: &str, created_this_run: bool) -> Option<String> {
        let steering =
            self.state
                .lock_or_recover()
                .for_prompt(self.window, session_id, created_this_run);
        let text = steering.as_ref().map(|s| s.text.clone());
        if let Some(text) = &text {
            info!(
                "Extension tool steering rides on this prompt on {} ({} chars)",
                session_id,
                text.len()
            );
        }
        *self.attempt.borrow_mut() = steering.map(|s| (session_id.to_string(), s, None));
        text
    }

    fn written(&self, session_id: &str) {
        if let Some((sid, _, write)) = self.attempt.borrow_mut().as_mut() {
            if sid == session_id {
                *write = Some(self.state.lock_or_recover().note_written(session_id));
            }
        }
    }

    fn completed(&self, session_id: &str) {
        if let Some((sid, steering, Some(write))) = self.attempt.borrow().as_ref() {
            if sid == session_id {
                self.state
                    .lock_or_recover()
                    .note_delivered(session_id, steering, *write);
            }
        }
    }
}

#[cfg(test)]
mod prompt_error_tests {
    use super::{
        auth_required_error, connect_failed_payload, failed_prompt_payload, prompt_error_for_user,
        session_setup_error, unsent_error_payload, unsent_prompt_payload,
    };
    use crate::acp_client::types::{AcpError, AgentRpcError, PromptRecoveryError};
    use crate::config::{AcpMode, AgentConnection, Config};

    fn config(preset_id: Option<&str>, mode: AcpMode) -> Config {
        let mut cfg = Config::default();
        cfg.acp.connections = vec![AgentConnection {
            id: "c".to_string(),
            name: "C".to_string(),
            preset_id: preset_id.map(str::to_string),
            mode,
            sessions_directory: None,
            ollama_settings: None,
        }];
        cfg.acp.active_connection_id = "c".to_string();
        cfg
    }

    fn local(cmd: &str) -> AcpMode {
        AcpMode::Local {
            spawn_command: cmd.to_string(),
        }
    }

    fn kiro_local() -> Config {
        config(Some("kiro"), local("kiro-cli acp"))
    }

    fn unreachable(detail: &str) -> anyhow::Error {
        anyhow::Error::new(PromptRecoveryError::AgentUnreachable {
            detail: detail.to_string(),
        })
    }

    fn auth_required() -> anyhow::Error {
        let reply = AcpError {
            code: -32000,
            message: "Authentication required".to_string(),
            data: None,
        };
        anyhow::Error::new(AgentRpcError::from_reply(
            "session/prompt",
            &reply,
            "ACP error: Authentication required".to_string(),
        ))
    }

    const RAW: &str = "ACP error: Internal error — Encountered an error in the response stream: An unknown error occurred: dispatch failure";

    #[test]
    fn unreachable_on_local_kiro_names_the_login_command() {
        crate::i18n::init(Some("en"));
        let (err, detail) = prompt_error_for_user(&unreachable(RAW), &kiro_local()).unwrap();
        assert_eq!(err.key, "errors.agent.unreachable_after_restart_kiro");
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "agent_unreachable");
        assert!(err.localised_message().contains("kiro-cli login"));
        assert_eq!(detail, RAW);
    }

    #[test]
    fn unreachable_on_other_agents_is_generic() {
        crate::i18n::init(Some("en"));
        let codex = config(Some("codex"), local("npx @zed-industries/codex-acp"));
        let (err, _) = prompt_error_for_user(&unreachable(RAW), &codex).unwrap();
        assert_eq!(err.key, "errors.agent.unreachable_after_restart");
        assert!(!err.localised_message().contains("kiro-cli"));
        // A Remote Kiro: Kage can't tell which machine needs the login.
        let remote_kiro = config(
            Some("kiro"),
            AcpMode::Remote {
                host: "10.0.0.2".to_string(),
                port: 8765,
                timeout_ms: 30000,
            },
        );
        let (err, _) = prompt_error_for_user(&unreachable(RAW), &remote_kiro).unwrap();
        assert_eq!(err.key, "errors.agent.unreachable_after_restart");
    }

    #[test]
    fn auth_required_asks_for_a_sign_in() {
        crate::i18n::init(Some("en"));
        let (err, detail) = prompt_error_for_user(&auth_required(), &kiro_local()).unwrap();
        assert_eq!(err.key, "errors.agent.auth_required_kiro");
        assert_eq!(serde_json::to_value(&err).unwrap()["kind"], "auth_required");
        assert!(err.localised_message().contains("kiro-cli login"));
        assert_eq!(detail, "ACP error: Authentication required");
        let claude = config(Some("claude-code"), local("claude-agent-acp"));
        let (err, _) = prompt_error_for_user(&auth_required(), &claude).unwrap();
        assert_eq!(err.key, "errors.agent.auth_required");
    }

    #[test]
    fn other_errors_are_left_alone() {
        crate::i18n::init(Some("en"));
        assert!(prompt_error_for_user(&anyhow::anyhow!("Timeout"), &kiro_local()).is_none());
        let internal = AcpError {
            code: -32603,
            message: "Internal error".to_string(),
            data: None,
        };
        let typed = anyhow::Error::new(AgentRpcError::from_reply(
            "session/prompt",
            &internal,
            "ACP error: Internal error".to_string(),
        ));
        assert!(prompt_error_for_user(&typed, &kiro_local()).is_none());
    }

    #[test]
    fn payload_carries_kind_key_message_detail_and_session() {
        crate::i18n::init(Some("en"));
        let for_user = prompt_error_for_user(&unreachable(RAW), &kiro_local());
        let json = failed_prompt_payload(for_user, RAW, "s1", true, Some("turn-7"));
        assert_eq!(json["kind"], "agent_unreachable");
        assert_eq!(json["key"], "errors.agent.unreachable_after_restart_kiro");
        assert!(json["message"].as_str().unwrap().contains("kiro-cli login"));
        assert_eq!(json["detail"], RAW);
        assert_eq!(json["sessionId"], "s1");
        assert_eq!(json["resendable"], true);
        assert_eq!(json["turnId"], "turn-7");
    }

    #[test]
    fn a_plain_prompt_failure_is_an_object_with_its_session_and_resendability() {
        // Was the bare string "Failed to send: …": no session, and nothing
        // to say a permission-approved action ran before the error.
        crate::i18n::init(Some("en"));
        let text = "ACP error: Internal error — dispatch failure";
        let json = failed_prompt_payload(None, text, "s9", false, None);
        assert_eq!(json["kind"], "internal");
        assert_eq!(json["message"], format!("Failed to send: {text}"));
        assert_eq!(json["detail"], serde_json::Value::Null);
        assert_eq!(json["sessionId"], "s9");
        assert_eq!(json["resendable"], false);
        assert_eq!(json["turnId"], serde_json::Value::Null);

        let json = failed_prompt_payload(None, "Rate limit exceeded", "s9", false, None);
        assert_eq!(json["kind"], "rate_limited");
        let json = failed_prompt_payload(None, "Timeout waiting for response", "s9", true, None);
        assert_eq!(json["kind"], "timeout");
        assert_eq!(json["resendable"], true);
    }

    #[test]
    fn a_connect_failure_is_an_object_naming_its_turn_and_never_resendable() {
        crate::i18n::init(Some("en"));
        let json = connect_failed_payload("refused", Some("s3"), Some("turn-1"));
        assert_eq!(json["kind"], "connection_lost");
        assert_eq!(json["message"], "Unable to connect: refused");
        assert_eq!(json["sessionId"], "s3");
        assert_eq!(json["turnId"], "turn-1");
        assert_eq!(json["resendable"], false);
        let json = unsent_prompt_payload(
            crate::error::ErrorKind::ConnectionLost,
            "Failed to send: no session".to_string(),
            None,
            None,
        );
        assert_eq!(json["sessionId"], serde_json::Value::Null);
        assert_eq!(json["turnId"], serde_json::Value::Null);
        assert_eq!(json["resendable"], false);
    }

    fn session_new_auth_required() -> anyhow::Error {
        let reply = AcpError {
            code: -32000,
            message: "Authentication required. Not logged in.".to_string(),
            data: None,
        };
        anyhow::Error::new(AgentRpcError::from_reply(
            "session/new",
            &reply,
            "Session creation failed: Authentication required. Not logged in. (code: -32000)"
                .to_string(),
        ))
    }

    #[test]
    fn a_session_setup_refused_for_a_sign_in_is_auth_required_and_names_kiros_command() {
        crate::i18n::init(Some("en"));
        let cfg = std::sync::Mutex::new(kiro_local());
        let err = session_setup_error(
            &session_new_auth_required(),
            &cfg,
            "Session creation",
            |_| panic!("a sign-in demand fell through to the plain error"),
        );
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "auth_required");
        assert_eq!(json["key"], "errors.agent.auth_required_kiro");
        assert!(json["message"].as_str().unwrap().contains("kiro-cli login"));
        // The same rule as a refused prompt's.
        let (prompt_err, detail) =
            prompt_error_for_user(&session_new_auth_required(), &kiro_local()).unwrap();
        assert_eq!(prompt_err.key, err.key);
        assert!(detail.contains("Not logged in"));
    }

    #[test]
    fn any_other_session_setup_failure_keeps_its_own_error() {
        let cfg = std::sync::Mutex::new(kiro_local());
        let err = session_setup_error(
            &anyhow::anyhow!("connection refused"),
            &cfg,
            "Session creation",
            |e| crate::error::AppError::from(format!("Failed to create session: {}", e)),
        );
        assert_eq!(err.key, "errors.passthrough");
        assert!(err
            .to_string()
            .contains("Failed to create session: connection refused"));
    }

    #[test]
    fn an_unsent_prompt_whose_session_was_refused_for_a_sign_in_carries_auth_required() {
        crate::i18n::init(Some("en"));
        let json = unsent_error_payload(auth_required_error(&kiro_local()), None, Some("turn-1"));
        assert_eq!(json["kind"], "auth_required");
        assert_eq!(json["key"], "errors.agent.auth_required_kiro");
        assert_eq!(json["turnId"], "turn-1");
        assert_eq!(json["resendable"], false);
    }
}

#[cfg(test)]
mod tests {
    use super::send_user_prompt;
    use crate::acp_client::{AcpClient, AcpConnectionMode};
    use crate::state::{ToolSteeringState, TOOLS_WITHDRAWN_BLOCK, TOOL_STEERING_TAG};
    use std::io::{BufRead, BufReader, Write};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    type Prompts = Arc<Mutex<Vec<Vec<String>>>>;

    /// A fake agent on loopback (ephemeral port, test-only). It ends every
    /// prompt at once (as cancelled if its text is "stop me", after 300 ms
    /// if it's "slow") and records each one's text blocks, timestamps left
    /// out, in the order they arrive. `session/new` creates "NEW"; any
    /// other request gets an empty result.
    fn fake_agent() -> (u16, Prompts) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let log = prompts.clone();
        std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut writer = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { return };
                let msg: serde_json::Value = serde_json::from_str(&line).unwrap();
                let result = match msg["method"].as_str() {
                    Some("session/prompt") => {
                        let texts: Vec<String> = msg["params"]["prompt"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter_map(|b| b["text"].as_str())
                            .filter(|t| !t.starts_with("[Current time:"))
                            .map(String::from)
                            .collect();
                        let last = texts.last().cloned().unwrap_or_default();
                        log.lock().unwrap().push(texts);
                        if last == "slow" {
                            std::thread::sleep(Duration::from_millis(300));
                        }
                        let stop_reason = if last == "stop me" {
                            "cancelled"
                        } else {
                            "end_turn"
                        };
                        serde_json::json!({ "stopReason": stop_reason })
                    }
                    Some("session/new") => serde_json::json!({ "sessionId": "NEW" }),
                    _ => serde_json::json!({}),
                };
                let reply =
                    serde_json::json!({ "jsonrpc": "2.0", "id": msg["id"], "result": result });
                if writeln!(writer, "{reply}").is_err() {
                    return;
                }
            }
        });
        (port, prompts)
    }

    fn connected_client(port: u16) -> AcpClient {
        let client = AcpClient::new(AcpConnectionMode::Remote {
            host: "127.0.0.1".to_string(),
            port,
        });
        client.connect().unwrap();
        client
    }

    /// Wait until the fake agent has received `n` prompts.
    fn wait_for_prompts(prompts: &Prompts, n: usize) {
        for _ in 0..500 {
            if prompts.lock().unwrap().len() >= n {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the agent never got {n} prompts");
    }

    fn tools(name: &str) -> String {
        format!("<extension_tools>\n{name}\n</extension_tools>")
    }

    /// The tool steering in effect for the agent after each prompt it
    /// received: that prompt's block, or the one before.
    fn effective_steering(prompts: &[Vec<String>]) -> Vec<Option<String>> {
        let mut current = None;
        prompts
            .iter()
            .map(|p| {
                if let Some(block) = p.first().filter(|t| t.starts_with(TOOL_STEERING_TAG)) {
                    current = Some(block.clone());
                }
                current.clone()
            })
            .collect()
    }

    #[test]
    fn tool_steering_rides_on_the_next_user_prompt_once() {
        let (port, prompts) = fake_agent();
        let client = connected_client(port);
        let steering = Mutex::new(ToolSteeringState::default());
        let (id, _) = client.create_session(None).unwrap();
        let send = |text: &str| {
            send_user_prompt(&client, &steering, "main", &id, text.to_string(), None).unwrap()
        };

        steering.lock().unwrap().publish("main", &tools("quotes"));
        assert_eq!(send("first"), id);
        send("second");
        steering.lock().unwrap().publish("main", "");
        send("third");
        send("fourth");

        let prompts = prompts.lock().unwrap().clone();
        assert_eq!(prompts.len(), 4);
        assert_eq!(prompts[0].len(), 2, "{:?}", prompts[0]);
        assert!(prompts[0][0].starts_with(TOOL_STEERING_TAG));
        assert!(prompts[0][0].contains("quotes"));
        assert_eq!(prompts[0][1], "first");
        assert_eq!(prompts[1], vec!["second"], "delivered once");
        assert!(prompts[2][0].contains("No local extension tools"));
        assert_eq!(prompts[2][1], "third");
        assert_eq!(prompts[3], vec!["fourth"]);
    }

    #[test]
    fn tool_steering_on_a_stopped_turn_is_sent_again() {
        // A turn cut short by Stop may not make it into the agent's history.
        let (port, prompts) = fake_agent();
        let client = connected_client(port);
        let steering = Mutex::new(ToolSteeringState::default());
        let send = |text: &str| {
            send_user_prompt(&client, &steering, "main", "S", text.to_string(), None).unwrap()
        };
        steering.lock().unwrap().publish("main", &tools("quotes"));
        send("stop me");
        send("again");
        send("later");

        let prompts = prompts.lock().unwrap().clone();
        assert!(prompts[0][0].starts_with(TOOL_STEERING_TAG));
        assert!(
            prompts[1][0].starts_with(TOOL_STEERING_TAG),
            "{:?}",
            prompts[1]
        );
        assert_eq!(prompts[1][1], "again");
        assert_eq!(prompts[2], vec!["later"]);
    }

    #[test]
    fn tool_steering_identical_prompts_racing_attach_it_once() {
        let (port, prompts) = fake_agent();
        let client = connected_client(port);
        let steering = Mutex::new(ToolSteeringState::default());
        steering.lock().unwrap().publish("main", &tools("quotes"));
        let send = |text: &str| {
            send_user_prompt(&client, &steering, "main", "S", text.to_string(), None).unwrap()
        };
        std::thread::scope(|scope| {
            let first = scope.spawn(|| send("slow"));
            // The second starts while the first, carrying the block, is
            // still in flight.
            wait_for_prompts(&prompts, 1);
            let second = scope.spawn(|| send("second"));
            first.join().unwrap();
            second.join().unwrap();
        });
        let prompts = prompts.lock().unwrap().clone();
        assert!(prompts[0][0].starts_with(TOOL_STEERING_TAG));
        assert_eq!(prompts[1], vec!["second"], "attached once");
    }

    #[test]
    fn tool_steering_queued_prompt_carries_its_own_windows_tools() {
        // The session holds A's tools. While another prompt holds the
        // session, window B (changed tools) and window A both send. Whichever
        // goes first, each prompt must reach the agent with its own window's
        // tools in effect: A's prompt queued behind B's can't go out bare.
        let (port, prompts) = fake_agent();
        let client = connected_client(port);
        let steering = Mutex::new(ToolSteeringState::default());
        steering.lock().unwrap().publish("a", &tools("A tools"));
        steering.lock().unwrap().publish("b", &tools("B tools"));
        let send = |window: &str, text: &str| {
            send_user_prompt(&client, &steering, window, "S", text.to_string(), None).unwrap()
        };
        send("a", "prime");
        std::thread::scope(|scope| {
            let holder = scope.spawn(|| client.send_chat_streaming("S", "slow", None).unwrap());
            wait_for_prompts(&prompts, 2);
            let from_b = scope.spawn(|| send("b", "from b"));
            let from_a = scope.spawn(|| send("a", "from a"));
            holder.join().unwrap();
            from_b.join().unwrap();
            from_a.join().unwrap();
        });
        let prompts = prompts.lock().unwrap().clone();
        assert_eq!(prompts.len(), 4, "{prompts:?}");
        let effective = effective_steering(&prompts);
        for (prompt, steering) in prompts.iter().zip(&effective) {
            let text = prompt.last().unwrap();
            let want = match text.as_str() {
                "from a" | "prime" | "slow" => "A tools",
                "from b" => "B tools",
                other => panic!("unexpected prompt {other}"),
            };
            assert!(
                steering.as_deref().is_some_and(|s| s.contains(want)),
                "{text:?} went out with {steering:?}: {prompts:?}"
            );
        }
    }

    #[test]
    fn tool_steering_skips_slash_commands_and_stays_pending() {
        let (port, prompts) = fake_agent();
        let client = connected_client(port);
        let steering = Mutex::new(ToolSteeringState::default());
        steering.lock().unwrap().publish("main", &tools("quotes"));
        let send = |text: &str| {
            send_user_prompt(&client, &steering, "main", "S", text.to_string(), None).unwrap()
        };
        // A command dispatched through `commands/execute` isn't a prompt
        // and leaves it pending too.
        client
            .send_request(
                &client.vendor_method("commands/execute"),
                serde_json::json!({ "sessionId": "S", "command": { "command": "context" } }),
            )
            .unwrap();
        send("/usage");
        send("/mcp:server:prompt arg");
        send("hello");
        send("again");

        let prompts = prompts.lock().unwrap().clone();
        assert_eq!(prompts[0], vec!["/usage"], "the command as typed, alone");
        assert_eq!(prompts[1], vec!["/mcp:server:prompt arg"]);
        assert!(
            prompts[2][0].starts_with(TOOL_STEERING_TAG),
            "{:?}",
            prompts[2]
        );
        assert_eq!(prompts[2][1], "hello");
        assert_eq!(prompts[3], vec!["again"]);
    }

    #[test]
    fn slash_command_prompt_is_a_single_text_block() {
        // Not even a due time stamp goes with it (`/usage` needs one block).
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(None));
        let log = seen.clone();
        std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut writer = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { return };
                let msg: serde_json::Value = serde_json::from_str(&line).unwrap();
                *log.lock().unwrap() = Some(msg["params"]["prompt"].clone());
                let reply = serde_json::json!({
                    "jsonrpc": "2.0", "id": msg["id"], "result": { "stopReason": "end_turn" }
                });
                if writeln!(writer, "{reply}").is_err() {
                    return;
                }
            }
        });
        let client = connected_client(port);
        let steering = Mutex::new(ToolSteeringState::default());
        steering.lock().unwrap().publish("main", &tools("quotes"));
        send_user_prompt(&client, &steering, "main", "S", "/usage".into(), None).unwrap();
        assert_eq!(
            seen.lock().unwrap().clone().unwrap(),
            serde_json::json!([{ "type": "text", "text": "/usage" }])
        );
    }

    #[test]
    fn tool_steering_restored_session_gets_a_withdrawal_once() {
        // "OLD" wasn't created by this run: an earlier run may have sent it
        // tools, so with none left its first prompt says so, once. A
        // session this run created never had any.
        let (port, prompts) = fake_agent();
        let client = connected_client(port);
        let steering = Mutex::new(ToolSteeringState::default());
        steering.lock().unwrap().publish("main", "");
        let (new_id, _) = client.create_session(None).unwrap();
        let send = |id: &str, text: &str| {
            send_user_prompt(&client, &steering, "main", id, text.to_string(), None).unwrap()
        };
        client.load_existing_session("OLD", None).unwrap();
        send("OLD", "resumed");
        send("OLD", "again");
        send(&new_id, "fresh");

        let prompts = prompts.lock().unwrap().clone();
        assert!(
            prompts[0][0].contains(TOOLS_WITHDRAWN_BLOCK),
            "{:?}",
            prompts[0]
        );
        assert_eq!(prompts[0][1], "resumed");
        assert_eq!(prompts[1], vec!["again"], "once");
        assert_eq!(prompts[2], vec!["fresh"]);
    }

    #[test]
    fn tool_steering_restored_session_gets_the_current_list_once() {
        let (port, prompts) = fake_agent();
        let client = connected_client(port);
        let steering = Mutex::new(ToolSteeringState::default());
        steering.lock().unwrap().publish("main", &tools("quotes"));
        let send = |text: &str| {
            send_user_prompt(&client, &steering, "main", "OLD", text.to_string(), None).unwrap()
        };
        send("resumed");
        send("again");
        let prompts = prompts.lock().unwrap().clone();
        assert!(prompts[0][0].contains("quotes"));
        assert_eq!(prompts[1], vec!["again"]);
    }

    #[test]
    fn tool_steering_window_that_has_not_published_sends_nothing() {
        // chat-1's extensions are still loading. It doesn't borrow main's
        // tools (it may not have them) and leaves the session alone.
        let (port, prompts) = fake_agent();
        let client = connected_client(port);
        let steering = Mutex::new(ToolSteeringState::default());
        steering
            .lock()
            .unwrap()
            .publish("main", &tools("main tools"));
        let send = |window: &str, text: &str| {
            send_user_prompt(&client, &steering, window, "S", text.to_string(), None).unwrap()
        };
        send("chat-1", "loading");
        send("main", "from main");
        send("chat-1", "still loading");
        send("main", "main again");
        steering
            .lock()
            .unwrap()
            .publish("chat-1", &tools("chat tools"));
        send("chat-1", "loaded");

        let prompts = prompts.lock().unwrap().clone();
        assert_eq!(prompts[0], vec!["loading"]);
        assert!(prompts[1][0].contains("main tools"));
        assert_eq!(prompts[2], vec!["still loading"]);
        assert_eq!(prompts[3], vec!["main again"], "main's delivery stands");
        assert!(prompts[4][0].contains("chat tools"));
    }
}
