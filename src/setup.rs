//! Tauri setup helpers extracted from main()'s `.setup(...)` closure.
//!
//! Each function here runs once during Tauri application setup and
//! owns one concern (window configuration, hotkey hot-reload, watchdog,
//! etc.). Moving them out of main.rs keeps the closure readable and
//! gives each stage a place to grow.
//!
//! These can't be unit-tested without spinning up a Tauri app, so the
//! trade is: small, self-explanatory functions with doc comments for
//! each concern, verified by building the binary and exercising it
//! manually.

use crate::lock_ext::LockExt;
use crate::state::AppState;
use log::{error, info, warn};
use std::sync::Arc;
use tauri::{App, AppHandle, Listener, Manager};

/// Configure the three transparent Tauri windows created by the app
/// config (floating, context-menu, inline-assist). Missing windows
/// are logged but not fatal — if e.g. the floating window failed to
/// register we want to know about it, not crash setup.
pub fn configure_transparent_windows(app: &App) {
    if let Some(floating_window) = app.get_webview_window("floating") {
        let _ = floating_window.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        #[cfg(target_os = "windows")]
        let _ = floating_window.set_shadow(false);
    } else {
        error!("Floating window not found during setup — UI will be limited");
    }

    if let Some(ctx_menu) = app.get_webview_window("context-menu") {
        let _ = ctx_menu.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        #[cfg(target_os = "windows")]
        let _ = ctx_menu.set_shadow(false);
    }

    if let Some(ia_win) = app.get_webview_window("inline-assist") {
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
    let hotkey_app = app.handle().clone();
    let hotkey_config = app.state::<AppState>().config.clone();
    let last_hotkey_snapshot: Arc<std::sync::Mutex<(String, Option<String>, Option<String>)>> = {
        let main = initial_config.get_hotkey_string();
        let cb = initial_config.get_clipboard_hotkey_string();
        let ia = initial_config.get_inline_assist_hotkey_string();
        Arc::new(std::sync::Mutex::new((main, cb, ia)))
    };
    app.listen("config_updated", move |_| {
        let (new_main, new_cb, new_ia) = match hotkey_config.try_lock() {
            Ok(config) => (
                config.get_hotkey_string(),
                config.get_clipboard_hotkey_string(),
                config.get_inline_assist_hotkey_string(),
            ),
            Err(_) => return,
        };

        let mut snapshot = last_hotkey_snapshot.lock_or_recover();
        if snapshot.0 == new_main && snapshot.1 == new_cb && snapshot.2 == new_ia {
            return;
        }

        info!("Hotkeys changed — re-registering all");
        crate::commands::system::register_all_hotkeys(&hotkey_app);
        *snapshot = (new_main, new_cb, new_ia);
    });
}

/// If the frontend doesn't signal ready within 15 seconds the webview
/// has almost certainly failed to load (typically because another
/// process still holds the WebView2 user data directory lock). We
/// exit with code 1 rather than run headless — a UI-less app is
/// worse than a clean restart.
pub fn spawn_frontend_watchdog(app: &App) {
    let ready_flag = app.state::<AppState>().frontend_ready.clone();
    let app_handle = app.handle().clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(15));
        if !ready_flag.load(std::sync::atomic::Ordering::Acquire) {
            error!("❌ Frontend did not become ready within 15 seconds — webview may have failed to load.");
            error!("   This usually means another process is holding the WebView2 user data folder.");
            error!("   Try closing other instances or killing stale WebView2 processes.");
            app_handle.exit(1);
        }
    });
}

/// Route `show-sessions` events (fired by the single-instance IPC
/// listener when a second launch tries to open) into the chat window.
pub fn install_show_sessions_listener(app: &App) {
    let app_handle = app.handle().clone();
    app.listen("show-sessions", move |_| {
        info!("show-sessions event received, opening chat window");
        let handle = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = crate::commands::window::open_chat_window(handle).await {
                log::error!("Failed to open chat window from IPC signal: {}", e);
            }
        });
    });
}

/// Boot the automation scheduler in the background and stash its
/// signal sender in AppState so emit_automation_signal can find it.
pub fn spawn_automation_scheduler(app: &App) {
    let state: tauri::State<'_, AppState> = app.state();
    let config_arc = state.config.clone();
    let signal_tx_arc = state.automation_signal_tx.clone();
    let app_handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        let (scheduler, signal_rx) = crate::automation::AutomationScheduler::new(config_arc);
        *signal_tx_arc.lock_or_recover() = Some(scheduler.signal_sender());
        scheduler.run(signal_rx, app_handle).await;
    });
}

/// If Pocket TTS is configured to auto-start, spawn its Python server
/// in the background and stash the child handle in AppState so we can
/// shut it down later.
pub fn maybe_autostart_pocket_tts(app: &App, config: &crate::config::Config) {
    if !(config.pocket_tts.enabled && config.pocket_tts.auto_start && config.pocket_tts.installed) {
        return;
    }
    info!("Pocket TTS auto-start enabled, spawning server in background");
    let state: tauri::State<'_, AppState> = app.state();
    let config_arc = state.config.clone();
    let tts_proc = state.pocket_tts_process.clone();
    tauri::async_runtime::spawn(async move {
        let (port, voice, temp, eos_threshold, python) = {
            let config = config_arc.lock_or_recover();
            (
                config.pocket_tts.port,
                config.pocket_tts.voice.clone(),
                config.pocket_tts.temp,
                config.pocket_tts.eos_threshold,
                config.pocket_tts.python_path.clone()
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
    let state: tauri::State<'_, AppState> = app.state();
    let launcher = state.app_launcher.clone();
    tauri::async_runtime::spawn(async move {
        crate::os::set_current_thread_name("app-launcher");

        match tauri::async_runtime::spawn_blocking(crate::app_launcher::AppLauncher::build_registry).await {
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
            match tauri::async_runtime::spawn_blocking(crate::app_launcher::AppLauncher::build_registry).await {
                Ok(Ok(registry)) => {
                    launcher.lock().await.apply_registry(registry);
                }
                Ok(Err(e)) => log::error!("Periodic app scan failed: {}", e),
                Err(e) => log::error!("Periodic app scan task failed: {}", e),
            }
        }
    });
}

/// Window close-requested handler: hide rather than close, so the app
/// persists in the tray. Logs (rather than panics) if hide fails.
pub fn handle_window_close(window: &tauri::Window, api: &tauri::CloseRequestApi) {
    if let Err(e) = window.hide() {
        log::warn!("Failed to hide window on close: {}", e);
    }
    api.prevent_close();
}

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


/// Spawn the start-of-day session bootstrap in the background when the
/// user has opted into `start_session_on_launch`. The flow:
///   1. Connect the ACP client.
///   2. Create a default session (capturing the available models).
///   3. Apply the default model if configured.
///   4. Send the steering message as the first hidden message.
///
/// Any failure here is logged but not propagated — the app stays
/// usable even if the agent backend is down at launch.
pub fn maybe_spawn_default_session(app: &App, config: &crate::config::Config) {
    if !config.acp.agent.start_session_on_launch {
        return;
    }
    info!("start_session_on_launch enabled, spawning background session init");

    let state: tauri::State<'_, AppState> = app.state();
    let acp_client = state.acp_client.clone();
    let floating_session = state.floating_session_id.clone();
    let config_arc = state.config.clone();
    let models_arc = state.available_models.clone();

    tauri::async_runtime::spawn(async move {
        let client = acp_client.lock().await;
        info!("Connecting ACP client on launch...");
        if let Err(e) = client.connect() {
            error!("Failed to connect on launch: {}", e);
            return;
        }

        info!("Creating default session on launch...");
        let cwd = {
            let cfg = config_arc.lock_or_recover();
            cfg.acp.agent.working_directory.clone()
        };

        let (session_id, models_json) = match client.create_session(cwd) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to create default session on launch: {}", e);
                return;
            }
        };
        info!("Default session created on launch: {}", session_id);

        if let Ok(mut fs) = floating_session.lock() {
            *fs = Some(session_id.clone());
        }

        store_available_models(models_json, &models_arc);
        apply_default_model_if_any(&client, &config_arc, &session_id);
        send_startup_steering(&client, &config_arc);
    });
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
    let request = crate::acp_client::AcpRequest {
        jsonrpc: "2.0".to_string(),
        id: serde_json::json!(4),
        method: "_kage.dev/commands/execute".to_string(),
        params: serde_json::json!({
            "sessionId": session_id,
            "command": { "command": "model", "args": { "modelName": model } }
        }),
    };
    match client.send_request(&request) {
        Ok(_) => info!("Default model applied: {}", model),
        Err(e) => error!("Failed to apply default model: {}", e),
    }
}

fn send_startup_steering(
    client: &crate::acp_client::AcpClient,
    config_arc: &Arc<std::sync::Mutex<crate::config::Config>>,
) {
    let steering_msg = {
        let cfg = config_arc.lock_or_recover();
        crate::commands::system::format_steering_message(
            &crate::commands::system::assemble_steering_parts(&cfg),
        )
    };
    info!("Sending steering message ({} chars)", steering_msg.len());
    if let Err(e) = client.send_chat_streaming(&steering_msg, None) {
        error!("Failed to send steering message: {}", e);
    }
}

/// Kick off the auto-update background loop.
pub fn start_updater(app: &App) {
    let state: tauri::State<'_, AppState> = app.state();
    crate::updater::start_update_loop(
        state.updater.clone(),
        state.config.clone(),
        app.handle().clone(),
        state.floating_session_id.clone(),
        state.acp_client.clone(),
    );
}

/// Watch the sessions directory for external changes (e.g., kage-cli
/// creating sessions outside of this process).
pub fn start_session_watcher(app: &App) {
    let state: tauri::State<'_, AppState> = app.state();
    crate::commands::sessions::start_session_watcher(
        state.session_cache.clone(),
        app.handle().clone(),
    );
}
