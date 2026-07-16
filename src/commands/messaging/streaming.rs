//! Prompt streaming: send/cancel a message, connection check/reconnect,
//! and opening a chat window pre-seeded with a message.

use super::*;

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
    let client = acp.client.clone();
    let config = features.config.clone();
    let config_for_title = features.config.clone();
    let session_cache_for_send = features.session_cache.clone();
    let app_for_send = app.clone();
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
                crate::event_targets::emit_to_self(
                    &window,
                    events::MESSAGE_ERROR,
                    &format!("Unable to connect: {}", e),
                );
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
                // An image error corrupts the conversation, so we always
                // want a *fresh* session — route through the same
                // restart-and-reset primitive the recovery ladder uses so
                // there's exactly one respawn path to reason about. The
                // originating window adopts the new id; other windows'
                // sessions are unaffected — they'll reconnect lazily on
                // their next send.
                info!("Image-related error detected — resetting ACP connection and session");
                let new_session_id = match client.restart_with_fresh_session() {
                    Ok(id) => Some(id),
                    Err(e) => {
                        error!("Failed to reset connection after image error: {}", e);
                        None
                    }
                };

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
                    &format!("Failed to send: {}", error_str),
                );
            }
            if let Ok(mut m) = originators.lock() {
                m.remove(&session_id);
            }
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
        if active_session_id != session_id {
            client_arc.reset_session_accumulator(&session_id);
        }

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

#[tauri::command]
pub async fn reconnect_acp(acp: State<'_, AcpHandles>) -> Result<bool, AppError> {
    // connect() is a synchronous handshake (Remote mode: up to 6 attempts
    // × 5s plus sleeps) — keep it off the Tokio workers.
    let client = acp.client.clone();
    async_runtime::spawn_blocking(move || client.connect())
        .await
        .map_err(|e| {
            AppError::keyed(
                ErrorKind::ConnectionLost,
                "errors.connection.reconnect_failed",
                &[("reason", &e.to_string())],
            )
        })?
        .map_err(|e| {
            AppError::keyed(
                ErrorKind::ConnectionLost,
                "errors.connection.reconnect_failed",
                &[("reason", &e.to_string())],
            )
        })?;
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
pub async fn open_chat_with_message(
    session_id: Option<String>,
    message: String,
    acp: State<'_, AcpHandles>,
    ui: State<'_, UiState>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    if let Some(floating) = app.get_webview_window(window_labels::FLOATING) {
        let _ = floating.hide();
    }
    if let Some(main) = app.get_webview_window(window_labels::MAIN) {
        // Center on the active monitor
        crate::commands::window::center_window_on_active_monitor(&main);
        let _ = main.show();
        let _ = main.set_focus();
        // Targeted emit: send to the main window only. Plain
        // `main.emit(...)` looks targeted but actually broadcasts to
        // every webview that has a listener — `emit_to` with a
        // WebviewWindow target is the one that actually scopes the
        // dispatch.
        let _ = app.emit_to(
            tauri::EventTarget::webview_window(window_labels::MAIN),
            "initial_message",
            message.clone(),
        );

        let client = acp.client.clone();
        let window = main.clone();
        let originators = ui.pending_prompt_originators.clone();

        async_runtime::spawn_blocking(move || {
            if !client.is_connected() {
                if let Err(e) = client.connect() {
                    crate::event_targets::emit_to_self(
                        &window,
                        events::MESSAGE_ERROR,
                        &format!("Unable to connect: {}", e),
                    );
                    return;
                }
            }
            // Resolve a real session — the main window may not have pinned
            // one yet (inline-assist's "inform" mode can fire before main
            // was ever opened). Must happen after connect so create works.
            let session_id = match resolve_or_create_session(&client, session_id) {
                Ok(id) => id,
                Err(e) => {
                    crate::event_targets::emit_to_self(
                        &window,
                        events::MESSAGE_ERROR,
                        &format!("Failed to send: {}", e),
                    );
                    return;
                }
            };
            // Tag the in-flight prompt so permission notifications route to
            // the main chat window, not the (now hidden) floating one.
            if let Ok(mut m) = originators.lock() {
                m.insert(session_id.clone(), window_labels::MAIN.to_string());
            }
            let result =
                client.send_chat_streaming_with_recovery(session_id.clone(), message, None);
            let active_session_id = match &result {
                Ok(id) => id.clone(),
                Err(_) => session_id.clone(),
            };
            if let Err(e) = result {
                crate::event_targets::emit_to_self(
                    &window,
                    events::MESSAGE_ERROR,
                    &format!("Failed to send: {}", e),
                );
                if let Ok(mut m) = originators.lock() {
                    m.remove(&session_id);
                }
                return;
            }
            // Streaming-audience emit — peer windows pinned to the same
            // session need to drop their thinking indicator. See the
            // parallel emit in send_message_streaming for the reasoning.
            crate::event_targets::emit_streaming_audience(
                window.app_handle(),
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
        });
    }
    Ok(())
}
