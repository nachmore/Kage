//! Process lifecycle: shutdown, restart, quit, dev-tools, first-run/welcome,
//! and post-update celebration banner.
//!
//! `graceful_shutdown` + `shutdown_and_exit_*` are the canonical exit path.
//! Tray-quit, the `quit_app` command, the restart command, and the auto-update
//! finish callback all funnel through here so we never miss steering generation
//! or child-process cleanup.

use crate::error::AppError;
use crate::lock_ext::LockExt;
use crate::state::{AcpHandles, ChildProcesses, FeatureServices};
use crate::window_labels;
use log::{error, info, warn};
use tauri::{Manager, State};

/// Consolidated shutdown: hide UI, kill TTS, generate steering, disconnect ACP.
/// Called from tray quit, quit_app, and restart_app to avoid duplicated cleanup.
pub fn graceful_shutdown(app: &tauri::AppHandle) {
    // Hide all windows and tray for instant visual feedback
    for label in &[
        window_labels::FLOATING,
        window_labels::MAIN,
        window_labels::SETTINGS,
        window_labels::CONTEXT_MENU,
    ] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.hide();
        }
    }
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_visible(false);
    }

    // Kill pocket-tts server and any in-flight pip install if running.
    // The Job Object reaps both on Windows when we exit, but macOS/Linux
    // have no equivalent — without an explicit kill here, a Cmd+Q during
    // a Pocket TTS install leaves the install running headless.
    if let Some(procs) = app.try_state::<ChildProcesses>() {
        let mut tts_proc = procs.pocket_tts.lock_or_recover();
        if let Some(mut child) = tts_proc.take() {
            info!("Stopping pocket-tts server on shutdown");
            let _ = child.kill();
            let _ = child.wait();
        }
        let mut install_proc = procs.pocket_tts_install.lock_or_recover();
        if let Some(mut child) = install_proc.take() {
            info!("Cancelling in-flight pocket-tts install on shutdown");
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    // Flush app log to disk
    crate::app_log::flush();
}

/// Run the async portion of shutdown (steering + disconnect) then exit.
/// Must be called from an async context after `graceful_shutdown`.
pub async fn shutdown_and_exit(app: &tauri::AppHandle) {
    shutdown_and_exit_inner(app, None).await;
}

/// Shutdown with optional restart: spawns a new process right before exit.
pub async fn shutdown_and_exit_with_restart(
    app: &tauri::AppHandle,
    exe: std::path::PathBuf,
    args: Vec<String>,
) {
    shutdown_and_exit_inner(app, Some((exe, args))).await;
}

async fn shutdown_and_exit_inner(
    app: &tauri::AppHandle,
    restart: Option<(std::path::PathBuf, Vec<String>)>,
) {
    if let (Some(acp), Some(features)) = (
        app.try_state::<AcpHandles>(),
        app.try_state::<FeatureServices>(),
    ) {
        let acp_client = acp.client.clone();
        let config = features.config.clone();

        // Quit-time steering operates on main's pinned session — that's
        // the conventional "primary" conversation and the one users
        // expect to be analysed. Floating sessions are typically short
        // and transactional; chat-<uuid> windows aren't yet ubiquitous
        // enough to be worth picking between. If main has no pinned
        // session, skip steering entirely.
        let main_session_id = app.try_state::<crate::state::UiState>().and_then(|ui| {
            ui.window_sessions
                .lock()
                .ok()
                .and_then(|m| m.get(window_labels::MAIN).cloned())
        });

        // Snapshot whether auto-steering will actually run before we pay any
        // cancel-and-wait cost. If there's nothing to steer, quit can skip
        // straight to disconnect.
        let will_run_steering = main_session_id.is_some()
            && if let Ok(cfg) = config.try_lock() {
                cfg.acp.agent.auto_steering_enabled
                    && acp_client.is_connected()
                    && crate::auto_steering::has_pending_messages()
            } else {
                false
            };

        if will_run_steering {
            let session_id = main_session_id.expect("checked above");
            // Cancel any in-flight prompt on this session before
            // issuing the steering prompt. The agent (kiro-cli) holds
            // an internal "prompt in progress" lock per session that
            // rejects any subsequent session/prompt until the active
            // prompt finishes or is cancelled. Without this, on-quit
            // steering races with the user's last prompt and the
            // agent replies with "Prompt already in progress" instead
            // of generating the doc.
            if let Err(e) = acp_client.cancel_session(&session_id) {
                warn!("Failed to send session/cancel on quit: {}", e);
            } else {
                // Brief pause so the agent can release its prompt
                // lock before auto_steering issues a new prompt.
                // 150ms is plenty for a local stdio agent; kept
                // small so quit still feels instant.
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            }

            if let Ok(config) = config.try_lock() {
                crate::auto_steering::generate_steering_on_quit(&acp_client, &config, &session_id);
            }
        }

        acp_client.disconnect();
    }

    // Spawn new instance right before exit (if restarting). On Windows we
    // pass CREATE_BREAKAWAY_FROM_JOB via `os::configure_breakaway_from_job`
    // so the new instance isn't tied to the dying parent's Job Object — see
    // `os::install_kill_on_exit_job`. The helper is a no-op on macOS/Linux,
    // where parent-exit reaping is handled by init/launchd.
    if let Some((exe, args)) = restart {
        info!("Spawning restart: {:?} {:?}", exe, args);
        let mut restart_cmd = std::process::Command::new(&exe);
        restart_cmd
            .args(&args)
            .current_dir(std::env::current_dir().unwrap_or_default());
        crate::os::configure_breakaway_from_job(&mut restart_cmd);
        match restart_cmd.spawn() {
            Ok(child) => info!("Restart process spawned (PID: {})", child.id()),
            Err(e) => error!("Failed to spawn restart process: {}", e),
        }
    }

    std::process::exit(0);
}

#[tauri::command]
pub async fn restart_app(app: tauri::AppHandle) -> Result<(), AppError> {
    info!("Restart requested via > command");

    let exe = std::env::current_exe().map_err(|e| format!("Failed to get exe path: {}", e))?;
    // Filter args: only keep our app flags, not cargo flags (--no-default-features, --color, etc.)
    let mut args: Vec<String> = Vec::new();
    let mut skip_next = false;
    for arg in std::env::args().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--" {
            break;
        } // Stop at cargo separator
        if arg.starts_with("--no-default") || arg.starts_with("--color") {
            if arg == "--color" {
                skip_next = true;
            } // --color has a value arg
            continue;
        }
        args.push(arg);
    }
    if !args.iter().any(|a| a == "--restart" || a == "/restart") {
        args.push(
            if cfg!(windows) {
                "/restart"
            } else {
                "--restart"
            }
            .to_string(),
        );
    }

    graceful_shutdown(&app);

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        shutdown_and_exit_with_restart(&app_handle, exe, args).await;
    });

    Ok(())
}

#[tauri::command]
pub async fn quit_app(app: tauri::AppHandle) -> Result<(), AppError> {
    info!("Quit requested via > command");
    graceful_shutdown(&app);

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        shutdown_and_exit(&app_handle).await;
    });

    Ok(())
}

#[tauri::command]
pub async fn open_devtools(app: tauri::AppHandle) -> Result<(), AppError> {
    #[cfg(debug_assertions)]
    if let Some(window) = app.get_webview_window(window_labels::FLOATING) {
        let window: tauri::WebviewWindow = window;
        window.open_devtools();
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
    }
    Ok(())
}

#[tauri::command]
pub async fn open_welcome_window(app: tauri::AppHandle) -> Result<(), AppError> {
    use tauri::WebviewWindowBuilder;
    // If window exists and is valid, just focus it
    if let Some(w) = app.get_webview_window(window_labels::WELCOME) {
        let _ = w.show();
        let _ = w.set_focus();
        crate::setup::update_activation_policy(&app);
        return Ok(());
    }
    // Create fresh window (previous one was closed/destroyed)
    let w = WebviewWindowBuilder::new(
        &app,
        window_labels::WELCOME,
        tauri::WebviewUrl::App("welcome.html".into()),
    )
    .title("Welcome to Kage")
    .inner_size(580.0, 640.0)
    .resizable(false)
    .decorations(false)
    .center()
    .visible(false) // Hidden until content loads
    .build()
    .map_err(|e| format!("Failed to open welcome window: {}", e))?;
    // Set dark background to prevent white flash
    let _ = w.set_background_color(Some(tauri::window::Color(30, 26, 36, 255)));
    // When closed, destroy so it can be recreated
    let w2 = w.clone();
    let app2 = app.clone();
    w.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { .. } = event {
            let _ = w2.destroy();
            crate::setup::update_activation_policy(&app2);
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn complete_first_run(
    app: tauri::AppHandle,
    features: State<'_, FeatureServices>,
    launch_at_startup: bool,
    auto_update: bool,
    enable_computer_control: bool,
    enable_personalization: bool,
    enable_telemetry: bool,
) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();
    config.first_run_completed = true;
    if auto_update {
        config.updates.auto_check = true;
        config.updates.silent_update = true;
    }
    config.acp.agent.auto_steering_enabled = enable_personalization;
    let _ = config.save();
    drop(config);

    // Apply the telemetry decision. `set_consent` generates a fresh
    // install_id on first opt-in and records the privacy policy version
    // the user saw — the UI re-prompts if the version ever changes.
    crate::telemetry::set_consent(&features.config, enable_telemetry);

    crate::os::set_startup_enabled(launch_at_startup);

    // Register the computer-control MCP server if the user opted in
    if enable_computer_control {
        crate::mcp_registration::ensure_registered();
    }

    // Track the first-run outcome. This runs *after* set_consent so it
    // respects the user's choice — if they opted out, no event is sent.
    crate::telemetry::track(
        &app,
        "first_run_completed",
        Some(serde_json::json!({
            "telemetry": if enable_telemetry { "opted_in" } else { "opted_out" },
            "startup": if launch_at_startup { "on" } else { "off" },
            "auto_update": if auto_update { "on" } else { "off" },
            "computer_control": if enable_computer_control { "on" } else { "off" },
            "personalization": if enable_personalization { "on" } else { "off" },
        })),
    );

    // NOTE: the welcome banner / floating window display used to be
    // triggered from here. That's moved to the `trigger_welcome_banner`
    // command so the welcome UI can pick the precise moment — part of
    // a fade-in choreography where the floating window appears midway
    // through the welcome window's fade-out.

    Ok(())
}

/// Show the floating window and welcome banner. Called from the welcome
/// UI's Finish-flow choreography (not automatically from
/// `complete_first_run` — we want precise control over when the
/// floating UI appears relative to the welcome window's fade-out).
#[tauri::command]
pub async fn trigger_welcome_banner(app: tauri::AppHandle) -> Result<(), AppError> {
    show_welcome_banner(&app);
    Ok(())
}

/// Show the floating window with a welcome banner displaying the configured hotkey.
/// Called from first-run completion and the dev tray menu.
pub fn show_welcome_banner(app: &tauri::AppHandle) {
    let hotkey_str = app
        .try_state::<FeatureServices>()
        .and_then(|features| {
            features
                .config
                .try_lock()
                .ok()
                .map(|c| c.get_hotkey_string())
        })
        .unwrap_or_else(|| "Alt+Space".to_string());
    let keycaps: String = hotkey_str
        .split('+')
        .map(|k| format!("<span class=\"keycap\">{}</span>", k))
        .collect::<Vec<_>>()
        .join("<span class=\"keycap-sep\">+</span>");
    let text = format!(
        "<b>Welcome to Kage!</b><br/>&nbsp;<br>Press {} anytime to summon me.",
        keycaps
    );

    if let Some(floating) = app.get_webview_window(window_labels::FLOATING) {
        crate::commands::window::center_floating_on_active_monitor(&floating);
        let _ = floating.show();
        let _ = floating.set_focus();
    }
    crate::event_targets::emit_to_floating(
        app,
        crate::events::SHOW_FLOATING_BANNER,
        &serde_json::json!({
            "icon": "👋",
            "text": text,
            "action_label": "",
            "action_type": "dismiss",
            "action_data": ""
        }),
    );
}

#[tauri::command]
pub async fn is_first_run(features: State<'_, FeatureServices>) -> Result<bool, AppError> {
    let config = features.config.lock_or_recover();
    Ok(!config.first_run_completed)
}

/// Simulate a completed update by showing the update banner on the floating window.
pub fn simulate_update_complete(app: &tauri::AppHandle) {
    show_update_banner(app);
}

/// Show the floating window with an update celebration banner.
pub fn show_update_banner(app: &tauri::AppHandle) {
    if let Some(floating) = app.get_webview_window(window_labels::FLOATING) {
        let _ = floating.show();
        let _ = floating.set_focus();
    }
    crate::event_targets::emit_to_floating(
        app,
        crate::events::SHOW_FLOATING_BANNER,
        &serde_json::json!({
            "icon": "🎉",
            "text": "Kage has been updated!",
            "action_label": "View changelog →",
            "action_type": "settings",
            "action_data": "updates"
        }),
    );
}
