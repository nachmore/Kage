//! Startup session initialization and post-install window display.

use crate::lock_ext::LockExt;
use crate::state::{AcpHandles, FeatureServices, UiState};
use crate::window_labels;
use log::{error, info, warn};
use std::sync::Arc;
use tauri::{App, Manager};

/// Consume the install-source marker (if any) and show the floating
/// window when the previous run was a *user-initiated* install. The
/// idle-install path leaves the floating window hidden — the user will
/// see the celebration banner the next time they summon it themselves.
///
/// We delete the marker as part of consuming it (see
/// `updater::consume_install_source`) so a stale marker can never
/// re-trigger this behaviour on a future launch.
///
/// Why we eval a suppression flag BEFORE `show()`: at this point the
/// other preloaded webviews (chat, inline-assist) are still painting
/// for the first time. Whichever paints LAST after we show the
/// floating window steals focus and triggers our `tauri://blur`
/// handler, which would normally hide the window. The blur handler
/// in `floating/app.js` checks `_suppressBlurHideUntil`, but that
/// flag was previously only set INSIDE `checkForUpdateBanner` —
/// which runs on the first `tauri://focus`, well after the early
/// focus-thrashing storm has already torn the window down. Setting
/// the flag from Rust here, BEFORE `show()`, races the focus events
/// to the JS engine and wins because eval runs synchronously in the
/// webview's main thread before the next paint. 5 seconds is
/// generous: chat-window first-paint is ~500ms cold, but we'd
/// rather over-suppress than have the banner vanish on a slower
/// machine.
pub fn maybe_show_floating_after_interactive_install(app: &App) {
    use crate::updater::{consume_install_source, InstallSource};
    let Some(source) = consume_install_source() else {
        return;
    };
    info!("Install source marker: {:?}", source);
    if source != InstallSource::Interactive {
        return;
    }

    // Warm the session before showing. Post-update, the resume path runs a
    // `session/load` (full conversation-history replay) that's much slower
    // than the `session/new` a normal launch does — so if we show the
    // floating window the instant the app starts, the user catches the
    // "Spinning up agent…" placeholder while that load is still in flight.
    // Wait (bounded) for `maybe_spawn_default_session` to pin the floating
    // session before showing, so the window appears already warm. The wait
    // is capped so a slow or dead agent still gets the window + celebration
    // banner — we never trade the banner for a hang.
    let app_handle = app.handle().clone();
    let ui: tauri::State<'_, UiState> = app.state();
    let features: tauri::State<'_, FeatureServices> = app.state();
    let window_sessions = ui.window_sessions.clone();
    // Only warm-wait when a session is actually being pre-spun. With
    // `start_session_on_launch = false`, `maybe_spawn_default_session`
    // returns early and never pins — waiting would just burn the full
    // timeout before showing. In that case show immediately.
    let preload = features
        .config
        .lock_or_recover()
        .acp
        .agent
        .start_session_on_launch;

    tauri::async_runtime::spawn(async move {
        // Cap at ~8s. session/new is sub-second; a long-history session/load
        // can take several seconds. Past this we show regardless rather than
        // leave the user staring at nothing after their install click.
        const MAX_WAIT_MS: u64 = 8000;
        const POLL_MS: u64 = 100;
        // Skip the warm-wait entirely when nothing will pin the session
        // (start_session_on_launch=false) — start already at the cap.
        let mut waited = if preload { 0u64 } else { MAX_WAIT_MS };
        let mut warmed = false;
        while waited < MAX_WAIT_MS {
            let pinned = window_sessions
                .lock()
                .ok()
                .map(|ws| ws.contains_key(window_labels::FLOATING))
                .unwrap_or(false);
            if pinned {
                warmed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
            waited += POLL_MS;
        }
        info!(
            "Post-update floating show: session {} after {}ms",
            if warmed {
                "warmed"
            } else {
                "NOT warmed (timeout)"
            },
            waited
        );

        if let Some(floating) = app_handle.get_webview_window(window_labels::FLOATING) {
            // Pre-arm a window-scoped suppression flag the JS blur
            // handler honours. Eval is queued onto the webview's main
            // thread; the property assignment runs before the next paint
            // and before the JS bootstrap has a chance to observe the
            // first focus event. The matching read in floating/app.js is
            // a guard at the top of the `tauri://blur` handler. 5s is
            // generous: chat-window first-paint is ~500ms cold, but
            // over-suppressing is much less bad than losing the banner.
            let _ = floating.eval(
                "window._kagePostUpdateSuppressUntil = Date.now() + 5000; \
                 console.log('[floating] post-update blur-hide suppression armed by Rust');",
            );
            crate::commands::window::center_floating_on_active_monitor(&floating);
            let _ = floating.show();
            let _ = floating.set_focus();
        }
    });
}

/// Spawn the start-of-day session bootstrap in the background.
///
/// `resume_session_id` is set when the user is launching a fresh process
/// after auto-update (or used `--resume-session <id>`). When present we
/// take the resume path: load that session via `session/load` and skip
/// the model/steering bootstrap, since the loaded session already has
/// its own model selection and steering history.
///
/// When `resume_session_id` is None we follow the original flow:
///   1. Connect the ACP client.
///   2. Create a fresh session (capturing the available models).
///   3. Apply the default model if configured.
///   4. Send the steering message as the first hidden message.
///
/// If `start_session_on_launch` is disabled we skip both paths. The
/// resume marker has already been consumed at startup either way, so
/// turning the setting off doesn't leave the file lying around to ghost
/// the next launch.
///
/// Any failure here is logged but not propagated — the app stays
/// usable even if the agent backend is down at launch.
pub fn maybe_spawn_default_session(
    app: &App,
    config: &crate::config::Config,
    resume_session_id: Option<String>,
) {
    if !config.acp.agent.start_session_on_launch {
        if resume_session_id.is_some() {
            warn!("Resume marker present but start_session_on_launch is disabled — ignoring (marker already consumed)");
        }
        return;
    }
    info!("start_session_on_launch enabled, spawning background session init");

    let acp: tauri::State<'_, AcpHandles> = app.state();
    let features: tauri::State<'_, FeatureServices> = app.state();
    let ui: tauri::State<'_, UiState> = app.state();
    let acp_client = acp.client.clone();
    let window_sessions = ui.window_sessions.clone();
    let config_arc = features.config.clone();
    let session_cache_arc = features.session_cache.clone();
    let models_arc = acp.available_models.clone();
    let app_handle = app.handle().clone();

    tauri::async_runtime::spawn(async move {
        info!("Connecting ACP client on launch...");
        if let Err(e) = acp_client.connect() {
            error!("Failed to connect on launch: {}", e);
            emit_session_pin_failed(
                &app_handle,
                window_labels::FLOATING,
                &format!("connect failed: {}", e),
            );
            return;
        }

        let cwd = {
            let cfg = config_arc.lock_or_recover();
            cfg.acp.agent.working_directory.clone()
        };

        let session_id = if let Some(resume_id) = resume_session_id {
            info!("Resuming session on launch: {}", resume_id);
            match acp_client.load_existing_session(&resume_id, cwd) {
                Ok((id, models_json)) => {
                    info!("Resumed session on launch: {}", id);
                    pin_session_to_floating(
                        &app_handle,
                        &window_sessions,
                        &config_arc,
                        &session_cache_arc,
                        &id,
                    );
                    // Source: was this a post-update relaunch, or a
                    // user picking up where they left off? Reading
                    // last_updated_version under a brief lock here
                    // distinguishes them — the welcome banner consumes
                    // the same field a moment later.
                    let source = {
                        let cfg = config_arc.lock_or_recover();
                        if crate::updater::was_just_updated(&cfg) {
                            "update-resume"
                        } else {
                            "floating-launch"
                        }
                    };
                    crate::telemetry::track(
                        &app_handle,
                        "session_resumed",
                        Some(serde_json::json!({ "source": source })),
                    );
                    // Loaded session already has its model + steering history;
                    // don't re-apply either or we'd duplicate the steering
                    // message and stomp the model the user actually picked.
                    // We DO populate the model dropdown if the agent
                    // included availableModels in the load response —
                    // otherwise the toolbar reads "No models" until a new
                    // session is created.
                    store_available_models(models_json, &models_arc);
                    return;
                }
                Err(e) => {
                    error!(
                        "Failed to resume session {}, falling back to fresh session: {}",
                        resume_id, e
                    );
                    // Recompute cwd because we moved it into load_existing_session
                    let cwd = {
                        let cfg = config_arc.lock_or_recover();
                        cfg.acp.agent.working_directory.clone()
                    };
                    match acp_client.create_session(cwd) {
                        Ok((sid, models_json)) => {
                            store_available_models(models_json, &models_arc);
                            crate::telemetry::track(
                                &app_handle,
                                "session_created",
                                Some(serde_json::json!({ "source": "resume-fallback" })),
                            );
                            sid
                        }
                        Err(e) => {
                            error!("Fallback session creation also failed: {}", e);
                            emit_session_pin_failed(
                                &app_handle,
                                window_labels::FLOATING,
                                &format!("fallback session/new failed: {}", e),
                            );
                            return;
                        }
                    }
                }
            }
        } else {
            info!("Creating default session on launch...");
            match acp_client.create_session(cwd) {
                Ok((sid, models_json)) => {
                    info!("Default session created on launch: {}", sid);
                    store_available_models(models_json, &models_arc);
                    crate::telemetry::track(
                        &app_handle,
                        "session_created",
                        Some(serde_json::json!({ "source": "launch" })),
                    );
                    sid
                }
                Err(e) => {
                    error!("Failed to create default session on launch: {}", e);
                    emit_session_pin_failed(
                        &app_handle,
                        window_labels::FLOATING,
                        &format!("session/new failed: {}", e),
                    );
                    return;
                }
            }
        };

        pin_session_to_floating(
            &app_handle,
            &window_sessions,
            &config_arc,
            &session_cache_arc,
            &session_id,
        );

        apply_default_model_if_any(&acp_client, &config_arc, &session_id);
        send_startup_steering(&acp_client, &config_arc, &session_id);
    });
}

/// Pin a launch-created session to the floating window, update its
/// title, and broadcast `session_pinned` so the floating frontend can
/// adopt it without polling. Main and chat-* peers don't get a pin
/// here — they default to floating's session lazily when opened, or
/// create their own when the user clicks "New Chat".
/// Tell the floating frontend the launch sequence failed and it
/// should stop waiting for `session_pinned`. Without this, floating
/// would hang on its `_adoptFloatingSession` await indefinitely
/// (since we removed the timeout) — the user types and sees a
/// "Spinning up agent…" placeholder forever.
fn emit_session_pin_failed(app: &tauri::AppHandle, label: &str, reason: &str) {
    log::warn!("session_pin_failed for {}: {}", label, reason);
    crate::event_targets::emit_to_floating(
        app,
        "session_pin_failed",
        &serde_json::json!({
            "label": label,
            "reason": reason,
        }),
    );
}

fn pin_session_to_floating(
    app: &tauri::AppHandle,
    window_sessions: &Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
    config_arc: &Arc<std::sync::Mutex<crate::config::Config>>,
    session_cache_arc: &Arc<std::sync::Mutex<Option<crate::commands::sessions::SessionCache>>>,
    session_id: &str,
) {
    if let Ok(mut ws) = window_sessions.lock() {
        ws.insert(window_labels::FLOATING.to_string(), session_id.to_string());
    }
    crate::commands::sessions::update_window_title(
        app,
        config_arc,
        session_cache_arc,
        window_labels::FLOATING,
        session_id,
    );
    // Tell the floating webview to adopt this id without racing
    // against the launch sequence. The frontend listens for
    // `session_pinned { label: "floating", sessionId }` during init
    // and falls back to creating its own session if the event hasn't
    // arrived within a short timeout.
    crate::event_targets::emit_to_floating(
        app,
        "session_pinned",
        &serde_json::json!({
            "label": window_labels::FLOATING,
            "sessionId": session_id,
        }),
    );
}

fn store_available_models(
    models_json: Vec<serde_json::Value>,
    models_arc: &Arc<std::sync::Mutex<Vec<crate::state::AcpModel>>>,
) {
    let models_value = serde_json::Value::Array(models_json);
    match serde_json::from_value::<Vec<crate::state::AcpModel>>(models_value.clone()) {
        Ok(parsed) => {
            info!("Storing {} models from session", parsed.len());
            if let Ok(mut m) = models_arc.lock() {
                *m = parsed;
            }
        }
        Err(e) => error!("Failed to parse models: {}. Raw: {}", e, models_value),
    }
}

fn apply_default_model_if_any(
    client: &crate::acp_client::AcpClient,
    config_arc: &Arc<std::sync::Mutex<crate::config::Config>>,
    session_id: &str,
) {
    let default_model = {
        let cfg = config_arc.lock_or_recover();
        cfg.acp.agent.default_model.clone()
    };
    let Some(model) = default_model.filter(|m| !m.is_empty()) else {
        return;
    };
    info!("Applying default model: {}", model);
    let result = client.send_request(
        &client.vendor_method("commands/execute"),
        serde_json::json!({
            "sessionId": session_id,
            "command": { "command": "model", "args": { "modelName": model } }
        }),
    );
    match result {
        Ok(_) => info!("Default model applied: {}", model),
        Err(e) => error!("Failed to apply default model: {}", e),
    }
}

fn send_startup_steering(
    client: &crate::acp_client::AcpClient,
    config_arc: &Arc<std::sync::Mutex<crate::config::Config>>,
    session_id: &str,
) {
    let steering_msg = {
        let inputs =
            crate::commands::system::SteeringInputs::from_config(&config_arc.lock_or_recover());
        crate::commands::system::format_steering_message(
            &crate::commands::system::assemble_steering_parts(&inputs),
        )
    };
    info!("Sending steering message ({} chars)", steering_msg.len());
    if let Err(e) = client.send_chat_streaming(session_id, &steering_msg, None) {
        error!("Failed to send steering message: {}", e);
    }
}
