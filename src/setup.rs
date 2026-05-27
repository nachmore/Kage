//! Tauri setup helpers extracted from main()'s `.setup(...)` closure.
//!
//! Each function here runs once during Tauri application setup and
//! owns one concern (window configuration, hotkey hot-reload, etc.).
//! Moving them out of main.rs keeps the closure readable and
//! gives each stage a place to grow.
//!
//! These can't be unit-tested without spinning up a Tauri app, so the
//! trade is: small, self-explanatory functions with doc comments for
//! each concern, verified by building the binary and exercising it
//! manually.

use crate::events;
use crate::lock_ext::LockExt;
use crate::state::{AcpHandles, ChildProcesses, FeatureServices, UiState};
use crate::window_labels::{self, is_session_host_label};
use log::{error, info, warn};
use std::sync::Arc;
use tauri::{App, AppHandle, Listener, Manager};

/// Configure the three transparent Tauri windows created by the app
/// config (floating, context-menu, inline-assist). Missing windows
/// are logged but not fatal — if e.g. the floating window failed to
/// register we want to know about it, not crash setup.
pub fn configure_transparent_windows(app: &App) {
    if let Some(floating_window) = app.get_webview_window(window_labels::FLOATING) {
        let _ = floating_window.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        #[cfg(target_os = "windows")]
        let _ = floating_window.set_shadow(false);
    } else {
        error!("Floating window not found during setup — UI will be limited");
    }

    if let Some(ctx_menu) = app.get_webview_window(window_labels::CONTEXT_MENU) {
        let _ = ctx_menu.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        #[cfg(target_os = "windows")]
        let _ = ctx_menu.set_shadow(false);
    }

    if let Some(ia_win) = app.get_webview_window(window_labels::INLINE_ASSIST) {
        let _ = ia_win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        #[cfg(target_os = "windows")]
        let _ = ia_win.set_shadow(false);
    }
}

/// Listen on `config_updated` and re-register all global hotkeys when
/// any of the three hotkey fields (main, clipboard, inline-assist)
/// actually changes. Snapshots the prior values so unrelated config
/// saves don't churn the registration.
pub fn install_hotkey_hot_reload(app: &App, initial_config: &crate::config::Config) {
    /// Snapshot of the three hotkey strings (main, clipboard, inline-assist).
    /// Aliased so the type doesn't bloat the local declaration.
    type HotkeySnapshot = (String, Option<String>, Option<String>);

    let hotkey_app = app.handle().clone();
    let hotkey_config = app.state::<FeatureServices>().config.clone();
    let last_hotkey_snapshot: Arc<std::sync::Mutex<HotkeySnapshot>> = {
        let main = initial_config.get_hotkey_string();
        let cb = initial_config.get_clipboard_hotkey_string();
        let ia = initial_config.get_inline_assist_hotkey_string();
        Arc::new(std::sync::Mutex::new((main, cb, ia)))
    };
    app.listen(events::CONFIG_UPDATED, move |_| {
        // Read the new hotkey strings out under a brief lock, then drop the
        // guard before doing anything else. Using lock() (via lock_or_recover)
        // instead of try_lock means we wait briefly under contention rather
        // than silently dropping the change. Pre-fix this listener used
        // try_lock and a single concurrent save of any config field would
        // make the user's hotkey edit go nowhere with no log line.
        let (new_main, new_cb, new_ia) = {
            let config = hotkey_config.lock_or_recover();
            (
                config.get_hotkey_string(),
                config.get_clipboard_hotkey_string(),
                config.get_inline_assist_hotkey_string(),
            )
        };

        let snapshot = last_hotkey_snapshot.lock_or_recover();
        if snapshot.0 == new_main && snapshot.1 == new_cb && snapshot.2 == new_ia {
            return;
        }

        info!("Hotkeys changed — re-registering all");
        // Drop the snapshot guard before calling register_all_hotkeys — that
        // path takes its own config lock and we don't want to hold an
        // unrelated mutex across it.
        let to_store = (new_main, new_cb, new_ia);
        drop(snapshot);
        crate::commands::system::register_all_hotkeys(&hotkey_app);
        *last_hotkey_snapshot.lock_or_recover() = to_store;
    });
}

/// Route `show-sessions` events (fired by the single-instance IPC
/// listener when a second launch tries to open) into the most-recently
/// focused chat window. Falls back to `main` if no chat window has been
/// focused this session — a fresh install or one where the user only
/// ever uses the floating widget.
pub fn install_show_sessions_listener(app: &App) {
    let app_handle = app.handle().clone();
    app.listen(events::SHOW_SESSIONS, move |_| {
        let handle = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            let target_label = handle
                .try_state::<UiState>()
                .and_then(|ui| ui.last_focused_chat.lock().ok().and_then(|s| s.clone()))
                .filter(|label| handle.get_webview_window(label).is_some())
                .unwrap_or_else(|| window_labels::MAIN.to_string());
            info!(
                "show_sessions event received, surfacing window: {}",
                target_label
            );
            if target_label == window_labels::MAIN {
                if let Err(e) = crate::commands::window::open_chat_window(handle.clone()).await {
                    log::error!("Failed to open chat window from IPC signal: {}", e);
                }
            } else if let Some(window) = handle.get_webview_window(&target_label) {
                let _ = window.show();
                let _ = window.set_focus();
                crate::setup::update_activation_policy(&handle);
            }
        });
    });
}

/// Install a focus listener on `main` so the
/// `UiState.last_focused_chat` tracker stays accurate without each
/// chat-* peer having to coordinate with main. `chat-<uuid>` peers
/// install their own listener in `open_new_chat_window`.
pub fn install_main_focus_tracker(app: &App) {
    if let Some(window) = app.get_webview_window(window_labels::MAIN) {
        let app_handle = app.handle().clone();
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Focused(true)) {
                crate::commands::window::mark_focused_chat(&app_handle, window_labels::MAIN);
            }
        });
    }
}

/// Boot the automation scheduler in the background and stash its
/// signal sender in FeatureServices so emit_automation_signal can find it.
pub fn spawn_automation_scheduler(app: &App) {
    let features: tauri::State<'_, FeatureServices> = app.state();
    let config_arc = features.config.clone();
    let signal_tx_arc = features.automation_signal_tx.clone();
    let app_handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        let (scheduler, signal_rx) = crate::automation::AutomationScheduler::new(config_arc);
        *signal_tx_arc.lock_or_recover() = Some(scheduler.signal_sender());
        scheduler.run(signal_rx, app_handle).await;
    });
}

/// If Pocket TTS is configured to auto-start, spawn its Python server
/// in the background and stash the child handle in ChildProcesses so we
/// can shut it down later.
pub fn maybe_autostart_pocket_tts(app: &App, config: &crate::config::Config) {
    if !(config.pocket_tts.enabled && config.pocket_tts.auto_start && config.pocket_tts.installed) {
        return;
    }
    info!("Pocket TTS auto-start enabled, spawning server in background");
    let features: tauri::State<'_, FeatureServices> = app.state();
    let procs: tauri::State<'_, ChildProcesses> = app.state();
    let config_arc = features.config.clone();
    let tts_proc = procs.pocket_tts.clone();
    tauri::async_runtime::spawn(async move {
        let (port, voice, temp, eos_threshold, python) = {
            let config = config_arc.lock_or_recover();
            (
                config.pocket_tts.port,
                config.pocket_tts.voice.clone(),
                config.pocket_tts.temp,
                config.pocket_tts.eos_threshold,
                config
                    .pocket_tts
                    .python_path
                    .clone()
                    .unwrap_or_else(|| "python".to_string()),
            )
        };

        let script_path = crate::commands::pocket_tts::get_server_script_path();
        if !script_path.exists() {
            warn!("Pocket TTS server script not found, skipping auto-start");
            return;
        }

        let mut cmd = std::process::Command::new(&python);
        cmd.arg(script_path.to_str().unwrap_or(""))
            .args(["--port", &port.to_string()])
            .args(["--voice", &voice])
            .args(["--temp", &temp.to_string()])
            .args(["--eos-threshold", &eos_threshold.to_string()])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        crate::commands::pocket_tts::configure_no_window(&mut cmd);

        match cmd.spawn() {
            Ok(child) => {
                info!("Pocket TTS server auto-started (PID: {})", child.id());
                let mut proc = tts_proc.lock_or_recover();
                *proc = Some(child);
            }
            Err(e) => warn!("Failed to auto-start Pocket TTS server: {}", e),
        }
    });
}

/// Kick off the background app-registry scan: one scan now, then a
/// periodic refresh every hour so discovered apps stay fresh. Both
/// scans run on blocking threads so the async runtime isn't tied up
/// during Windows registry walks.
pub fn spawn_app_registry_scan(app: &App) {
    let features: tauri::State<'_, FeatureServices> = app.state();
    let launcher = features.app_launcher.clone();
    tauri::async_runtime::spawn(async move {
        crate::os::set_current_thread_name("app-launcher");

        match tauri::async_runtime::spawn_blocking(crate::app_launcher::AppLauncher::build_registry)
            .await
        {
            Ok(Ok(registry)) => {
                launcher.lock().await.apply_registry(registry);
            }
            Ok(Err(e)) => log::error!("Background app scan failed: {}", e),
            Err(e) => log::error!("Background app scan task failed: {}", e),
        }

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        interval.tick().await; // skip immediate first tick
        loop {
            interval.tick().await;
            log::info!("Periodic app registry refresh");
            match tauri::async_runtime::spawn_blocking(
                crate::app_launcher::AppLauncher::build_registry,
            )
            .await
            {
                Ok(Ok(registry)) => {
                    launcher.lock().await.apply_registry(registry);
                }
                Ok(Err(e)) => log::error!("Periodic app scan failed: {}", e),
                Err(e) => log::error!("Periodic app scan task failed: {}", e),
            }
        }
    });
}

/// Self-heal the computer-control MCP registration on every launch.
///
/// Why this exists: `mcp_registration::ensure_registered()` only ran on
/// first-run opt-in or explicit toggle. After an update, three things
/// can drift the entry out of sync silently:
///   1. The exe install path changed (per-user reinstall, manual move).
///   2. The agent backend changed (e.g. user switched from Kiro to
///      Claude Code) so we'd want to write to a different mcp.json.
///   3. The bundle stopped shipping the MCP binary (build regression).
///
/// In all three cases the toggle in Settings says "on" but the agent
/// doesn't actually spawn the server, and there's no surface telling
/// the user why. This re-runs the registration on every launch so the
/// path stays fresh, and emits a loud log line if the binary isn't
/// where we expect it.
///
/// Cheap: a no-op when the existing entry already matches the current
/// path, which is the common case.
pub fn refresh_mcp_registration_if_enabled() {
    if !crate::mcp_registration::is_registered() {
        // User never opted in (or explicitly toggled off). Nothing to
        // refresh; leave their mcp.json alone.
        return;
    }

    match crate::mcp_registration::get_mcp_binary_path() {
        Some(path) => {
            info!(
                "computer-control MCP binary at {} — refreshing registration",
                path.display()
            );
            crate::mcp_registration::ensure_registered();
        }
        None => {
            // Toggle says on but we can't find the binary next to the
            // exe. Most likely a botched install/update; surface it
            // loudly so the user can see why the agent isn't getting
            // computer-control tools.
            warn!(
                "computer-control MCP is enabled in mcp.json but \
                 kage-computer-control-mcp binary is missing next to the \
                 main exe. The agent will fail to spawn it. Try toggling \
                 the switch in Settings → MCP Servers, or reinstall."
            );
        }
    }
}

/// Window close-requested handler: hide rather than close, so the app
/// persists in the tray. Logs (rather than panics) if hide fails.
/// On macOS, also hides the app to return focus to the previous application.
pub fn handle_window_close(window: &tauri::Window, api: &tauri::CloseRequestApi) {
    if let Err(e) = window.hide() {
        log::warn!("Failed to hide window on close: {}", e);
    }
    api.prevent_close();

    // Hidden chat window counts as closed for the agent-shutdown
    // decision. Schedule the check; if the user reopens within the
    // grace window (e.g. by clicking the tray) we cancel.
    let label = window.label();
    if is_session_host_label(label) {
        crate::commands::window::schedule_chat_shutdown_check_public(window.app_handle());
    }

    // On macOS: update activation policy (exclude the closing window since
    // is_visible() may not reflect the hide yet), then hide the app to
    // deactivate and return focus to the previous application.
    #[cfg(target_os = "macos")]
    {
        let closing_label = window.label().to_string();
        update_activation_policy_excluding(window.app_handle(), Some(&closing_label));
        hide_macos_app();
    }
}

/// Hide the macOS app (NSApp.hide), returning focus to the previous application.
/// This is the equivalent of Cmd+H — the app stays running but yields focus.
#[cfg(target_os = "macos")]
pub fn hide_macos_app() {
    use objc2::rc::autoreleasepool;
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    autoreleasepool(|_pool| {
        // Safe: this is always called from the main thread (UI event handlers).
        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            app.hide(None);
        }
    });
}

/// Update the macOS activation policy based on whether any "real" window
/// (chat, settings, store, welcome) is visible. When at least one is
/// visible, switch to Regular (shows in Cmd+Tab and Dock). When none are
/// visible, switch to Accessory (hidden from Cmd+Tab and Dock).
///
/// The floating window is excluded — it's a transient overlay, not
/// something the user Cmd+Tabs to.
///
/// Uses Tauri's built-in `set_activation_policy` which handles main-thread
/// dispatch internally.
#[cfg(target_os = "macos")]
pub fn update_activation_policy(app_handle: &AppHandle) {
    update_activation_policy_excluding(app_handle, None);
}

/// Same as `update_activation_policy` but allows excluding a window label
/// from the visibility check (used when a window is being hidden but
/// `is_visible()` hasn't caught up yet).
#[cfg(target_os = "macos")]
pub fn update_activation_policy_excluding(app_handle: &AppHandle, exclude: Option<&str>) {
    use tauri::ActivationPolicy;

    // Windows that count as "real" for Cmd+Tab purposes
    let real_windows = [
        window_labels::MAIN,
        window_labels::SETTINGS,
        window_labels::STORE,
        window_labels::WELCOME,
    ];

    let any_visible = real_windows.iter().any(|label| {
        if exclude == Some(*label) {
            return false;
        }
        app_handle
            .get_webview_window(label)
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(false)
    });

    let desired = if any_visible {
        ActivationPolicy::Regular
    } else {
        ActivationPolicy::Accessory
    };

    log::debug!(
        "update_activation_policy: any_visible={}, setting {}",
        any_visible,
        if any_visible { "Regular" } else { "Accessory" }
    );

    if let Err(e) = app_handle.set_activation_policy(desired) {
        log::warn!("Failed to set activation policy: {}", e);
    } else {
        log::debug!(
            "Activation policy set → {}",
            if any_visible { "Regular" } else { "Accessory" }
        );
        // macOS quirk: switching from Accessory → Regular doesn't update
        // the Cmd+Tab list until the app goes through an activation cycle.
        // We must explicitly activate the app after the policy change.
        if any_visible {
            let _ = app_handle.run_on_main_thread(|| {
                use objc2::MainThreadMarker;
                use objc2_app_kit::NSApplication;

                let mtm = unsafe { MainThreadMarker::new_unchecked() };
                let ns_app = NSApplication::sharedApplication(mtm);
                #[allow(deprecated)]
                ns_app.activateIgnoringOtherApps(true);
            });
        }
    }
}

/// No-op on non-macOS platforms.
#[cfg(not(target_os = "macos"))]
pub fn update_activation_policy(_app_handle: &AppHandle) {}

// `update_activation_policy_excluding` has no non-macOS stub on purpose:
// its only caller is the `#[cfg(target_os = "macos")]` block in
// `handle_window_close`, so a cross-platform stub would be dead code.

/// Show the welcome window on first run. Small delay so the floating
/// window has finished initializing before the welcome stacks on top.
pub fn maybe_show_welcome_window(app_handle: &AppHandle, first_run_completed: bool) {
    if first_run_completed {
        return;
    }
    info!("First run detected, showing welcome window");
    let app_handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let _ = crate::commands::system::open_welcome_window(app_handle).await;
    });
}

/// Consume the install-source marker (if any) and show the floating
/// window when the previous run was a *user-initiated* install. The
/// idle-install path leaves the floating window hidden — the user will
/// see the celebration banner the next time they summon it themselves.
///
/// We delete the marker as part of consuming it (see
/// `updater::consume_install_source`) so a stale marker can never
/// re-trigger this behaviour on a future launch.
///
/// Why deferred: at this point in setup the floating window's webview
/// JS may still be initialising, but `show()` is fine to call
/// regardless — the window appears as soon as its frontend is
/// painted, and `checkForUpdateBanner` (in floating/app.js) will
/// already have queued the banner DOM by the time the user looks at
/// it.
pub fn maybe_show_floating_after_interactive_install(app_handle: &AppHandle) {
    use crate::updater::{consume_install_source, InstallSource};
    let Some(source) = consume_install_source() else {
        return;
    };
    info!("Install source marker: {:?}", source);
    if source != InstallSource::Interactive {
        return;
    }
    if let Some(floating) = app_handle.get_webview_window(window_labels::FLOATING) {
        crate::commands::window::center_floating_on_active_monitor(&floating);
        let _ = floating.show();
        let _ = floating.set_focus();
    }
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
    use tauri::Emitter;
    log::warn!("session_pin_failed for {}: {}", label, reason);
    let _ = app.emit(
        "session_pin_failed",
        serde_json::json!({
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
    use tauri::Emitter;
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
    // Broadcast so the floating webview can adopt this id without
    // racing against the launch sequence. The frontend listens for
    // `session_pinned { label: "floating", sessionId }` during init
    // and falls back to creating its own session if the event hasn't
    // arrived within a short timeout.
    let _ = app.emit(
        "session_pinned",
        serde_json::json!({
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
        let cfg = config_arc.lock_or_recover();
        crate::commands::system::format_steering_message(
            &crate::commands::system::assemble_steering_parts(&cfg),
        )
    };
    info!("Sending steering message ({} chars)", steering_msg.len());
    if let Err(e) = client.send_chat_streaming(session_id, &steering_msg, None) {
        error!("Failed to send steering message: {}", e);
    }
}

/// Kick off the auto-update background loop.
pub fn start_updater(app: &App) {
    let acp: tauri::State<'_, AcpHandles> = app.state();
    let features: tauri::State<'_, FeatureServices> = app.state();
    let ui: tauri::State<'_, UiState> = app.state();
    crate::updater::start_update_loop(
        features.updater.clone(),
        features.config.clone(),
        app.handle().clone(),
        ui.window_sessions.clone(),
        acp.client.clone(),
    );
}

/// Watch the sessions directory for external changes (e.g., the agent
/// backend creating sessions outside of this process).
pub fn start_session_watcher(app: &App) {
    let features: tauri::State<'_, FeatureServices> = app.state();
    crate::commands::sessions::start_session_watcher(
        features.session_cache.clone(),
        app.handle().clone(),
    );
}
