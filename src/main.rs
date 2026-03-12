// Hide console window on Windows
#![cfg_attr(windows, windows_subsystem = "windows")]

mod acp_client;
mod app_launcher;
mod auto_steering;
mod commands;
mod config;
mod error;
mod extensions;
mod logger;
mod os;
mod process_manager;
mod single_instance;
mod state;
mod tray;
mod updater;

use acp_client::AcpClient;
use app_launcher::AppLauncher;
use commands::window::toggle_floating_window;
use config::Config;
use log::{error, info, warn};
use process_manager::ProcessManager;
use state::AppState;
use std::sync::Arc;
use tauri::Manager;
use tauri::Listener;
use tauri::Emitter;
use tokio::sync::Mutex;

/// In debug builds on Windows, attach to the parent console (if any) so that
/// logs appear when launched from a terminal. If launched from Explorer/GUI,
/// AttachConsole fails silently and no console is shown.
#[cfg(all(windows, debug_assertions))]
fn attach_parent_console() {
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFFFFFF;
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS); }
}

fn main() {
    // Handle /capture-hotkey subcommand (helper process mode)
    #[cfg(target_os = "windows")]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.len() >= 2 && args[1] == "/capture-hotkey" {
            let timeout: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10000);
            os::windows::hotkey_capture::run_capture_helper(timeout);
            return;
        }
    }

    #[cfg(all(windows, debug_assertions))]
    attach_parent_console();

    // Initialize logger first
    if let Err(e) = logger::init_logger() {
        eprintln!("Failed to initialize logger: {}", e);
        eprintln!("Continuing without file logging...");
    }

    info!("=== Kiro Assistant Starting ===");
    let startup_t0 = std::time::Instant::now();

    // Enforce single instance across all builds (debug + release)
    let _instance_lock = match single_instance::try_acquire() {
        Ok(lock) => lock,
        Err(e) => {
            error!("{}", e);
            std::process::exit(0);
        }
    };

    let args: Vec<String> = std::env::args().collect();
    let dev_mode = args.iter().any(|arg| arg == "/dev" || arg == "--dev");
    let debug_mode = args.iter().any(|arg| arg == "/debug" || arg == "--debug");

    // Check for session resume after update — read from last-session.txt
    let _resume_session_id: Option<String> = args.iter()
        .position(|arg| arg == "/resume-session" || arg == "--resume-session")
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| {
            // Also check the last-session.txt file (written by the updater before exit)
            dirs::config_dir()
                .map(|d| d.join("kiro-assistant").join("last-session.txt"))
                .and_then(|p| std::fs::read_to_string(&p).ok())
                .map(|s| {
                    // Clean up the file after reading
                    let _ = std::fs::remove_file(
                        dirs::config_dir().unwrap().join("kiro-assistant").join("last-session.txt")
                    );
                    s.trim().to_string()
                })
                .filter(|s| !s.is_empty())
        });

    if debug_mode {
        println!("🐛 DEBUG MODE ENABLED - Detailed ACP logs will be printed to console");
        info!("🐛 DEBUG MODE ENABLED via command line argument");
        logger::enable_console_logging();
    }

    info!("Checking for orphaned processes...");
    std::thread::spawn(|| {
        if let Err(e) = ProcessManager::cleanup_orphaned_processes() {
            warn!("Failed to cleanup orphaned processes: {}", e);
        }
    });

    // On Windows, create a Job Object that auto-kills all child processes
    // when this process exits (even on crash). This prevents orphaned
    // TTS servers, ACP CLI processes, etc.
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::System::JobObjects::*;
        use windows::Win32::System::Threading::GetCurrentProcess;
        use windows::core::PCWSTR;

        unsafe {
            match CreateJobObjectW(None, PCWSTR::null()) {
                Ok(job) => {
                    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                        | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
                    let set_ok = SetInformationJobObject(
                        job,
                        JobObjectExtendedLimitInformation,
                        &info as *const _ as *const _,
                        std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    );
                    if set_ok.is_ok() {
                        let current = GetCurrentProcess();
                        match AssignProcessToJobObject(job, current) {
                            Ok(_) => info!("✅ Job Object created — child processes will be killed on exit"),
                            Err(e) => warn!("Failed to assign process to Job Object: {}", e),
                        }
                    } else {
                        warn!("Failed to configure Job Object");
                    }
                    // The job handle must stay open for the lifetime of the process.
                    // HANDLE is Copy with no Drop impl, so it won't be auto-closed
                    // when it goes out of scope — exactly what we want. The OS keeps
                    // the Job Object alive (and its KILL_ON_JOB_CLOSE policy active)
                    // as long as the handle isn't explicitly closed via CloseHandle.
                    let _ = job;
                }
                Err(e) => warn!("Failed to create Job Object: {}", e),
            }
        }
    }

    let mut config = Config::load().unwrap_or_else(|e| {
        error!("Failed to load config, using defaults: {}", e);
        eprintln!("Failed to load config, using defaults: {}", e);
        Config::default()
    });

    if debug_mode {
        config.debug_mode = true;
    }

    info!("Configuration loaded");
    if dev_mode { info!("⏱ Config loaded at +{}ms", startup_t0.elapsed().as_millis()); }

    let acp_client = match &config.acp.mode {
        crate::config::AcpMode::Local { spawn_command } => {
            info!("ACP Mode: Local with spawn command: {}", spawn_command);
            AcpClient::new(acp_client::AcpConnectionMode::Local {
                spawn_command: spawn_command.clone(),
            })
        }
        crate::config::AcpMode::Remote {
            host,
            port,
            timeout_ms,
        } => {
            info!(
                "ACP Mode: Remote at {}:{} (timeout: {}ms)",
                host, port, timeout_ms
            );
            AcpClient::new(acp_client::AcpConnectionMode::Remote {
                host: host.clone(),
                port: *port,
            })
        }
    };

    acp_client.set_debug_mode(config.debug_mode);

    let process_manager = acp_client.get_process_manager();
    process_manager::install_signal_handlers(process_manager);

    let app_launcher = AppLauncher::new().unwrap_or_else(|e| {
        error!("Failed to initialize app launcher: {}", e);
        eprintln!("Failed to initialize app launcher: {}", e);
        AppLauncher::new().unwrap()
    });
    info!("App launcher initialized (scan deferred to background)");
    if dev_mode { info!("⏱ App launcher ready at +{}ms", startup_t0.elapsed().as_millis()); }

    let pipe_stdin_handle = acp_client.get_pipe_stdin();
    let tcp_writer_handle = acp_client.get_tcp_writer();

    let pipe_stdin_for_handler = pipe_stdin_handle.clone();
    let tcp_writer_for_handler = tcp_writer_handle.clone();
    let config_for_setup = config.clone();
    let dev_mode_for_setup = dev_mode;

    let acp_client_arc = Arc::new(Mutex::new(acp_client));
    let config_arc = Arc::new(std::sync::Mutex::new(config));
    let slash_commands_arc = Arc::new(std::sync::Mutex::new(Vec::new()));
    let pending_permission_arc = Arc::new(std::sync::Mutex::new(None));
    let available_models_arc = Arc::new(std::sync::Mutex::new(Vec::<crate::state::AcpModel>::new()));

    // Clone Arcs for the notification handler setup
    let config_for_handler = config_arc.clone();
    let slash_cmds_for_handler = slash_commands_arc.clone();
    let pending_perm_for_handler = pending_permission_arc.clone();
    let acp_for_handler = acp_client_arc.clone();
    let models_for_handler = available_models_arc.clone();

    if dev_mode { info!("⏱ Tauri builder starting at +{}ms", startup_t0.elapsed().as_millis()); }
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_global_shortcut::Builder::default().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            acp_client: acp_client_arc,
            config: config_arc,
            app_launcher: Arc::new(Mutex::new(app_launcher)),
            pipe_stdin: pipe_stdin_handle,
            tcp_writer: tcp_writer_handle,
            dev_mode,
            floating_session_id: Arc::new(std::sync::Mutex::new(None)),
            pending_permission: pending_permission_arc,
            slash_commands: slash_commands_arc,
            available_models: available_models_arc,
            current_model_id: Arc::new(std::sync::Mutex::new(None)),
            last_selection: Arc::new(std::sync::Mutex::new(None)),
            notification_source: Arc::new(std::sync::Mutex::new("floating".to_string())),
            updater: Arc::new(updater::UpdaterState::new()),
            user_info_cache: Arc::new(std::sync::Mutex::new(None)),
            session_cache: Arc::new(std::sync::Mutex::new(None)),
            pocket_tts_process: Arc::new(std::sync::Mutex::new(None)),
            pocket_tts_install_process: Arc::new(std::sync::Mutex::new(None)),
            automation_plan_cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_tool_steering_hash: Arc::new(std::sync::Mutex::new(0)),
            frontend_ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                window.hide().unwrap();
                api.prevent_close();
            }
        })
        .setup(move |app| {
            info!("Setting up application");
            info!("=== Kiro Assistant Setup ===");

            let config = config_for_setup;
            let dev_mode = dev_mode_for_setup;

            // Build system tray
            tray::setup_tray(app, dev_mode)?;

            // Set up the ACP notification handler
            {
                let client = tauri::async_runtime::block_on(acp_for_handler.lock());
                commands::messaging::setup_notification_handler(
                    &client,
                    app.handle(),
                    config_for_handler,
                    pipe_stdin_for_handler,
                    tcp_writer_for_handler,
                    slash_cmds_for_handler,
                    pending_perm_for_handler,
                    models_for_handler,
                );
            }

            // Configure floating window
            let floating_window = app.get_webview_window("floating").unwrap();
            let _ = floating_window.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
            #[cfg(target_os = "windows")]
            let _ = floating_window.set_shadow(false);

            // Configure context-menu window
            if let Some(ctx_menu) = app.get_webview_window("context-menu") {
                let _ = ctx_menu.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
                #[cfg(target_os = "windows")]
                let _ = ctx_menu.set_shadow(false);
            }

            // Register global hotkey
            let hotkey_string = config.get_hotkey_string();
            info!("Attempting to register global hotkey: {}", hotkey_string);

            // Helper to register a hotkey
            fn register_hotkey(app: &tauri::App, hotkey: &str, window: tauri::WebviewWindow) -> Result<(), String> {
                use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
                app.global_shortcut().on_shortcut(
                    hotkey,
                    move |_app, _shortcut, event| {
                        if event.state != ShortcutState::Pressed { return; }
                        info!("Hotkey triggered");
                        toggle_floating_window(&window);
                    },
                ).map_err(|e| format!("{}", e))
            }

            let active_hotkey = match register_hotkey(app, &hotkey_string, floating_window.clone()) {
                Ok(_) => {
                    info!("✅ Registered global hotkey: {}", hotkey_string);
                    hotkey_string.clone()
                }
                Err(e) => {
                    warn!("❌ Failed to register {}: {}", hotkey_string, e);
                    match register_hotkey(app, "Alt+K", floating_window.clone()) {
                        Ok(_) => {
                            info!("✅ Registered fallback hotkey: Alt+K");
                            "Alt+K".to_string()
                        }
                        Err(e2) => {
                            error!("❌ Failed to register any hotkey: {}", e2);
                            "None".to_string()
                        }
                    }
                }
            };

            info!("Active hotkey: {}", active_hotkey);

            // Register clipboard history hotkey if configured
            if let Some(cb_hotkey) = config.get_clipboard_hotkey_string() {
                use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
                let cb_window = floating_window.clone();
                let cb_app_handle = app.handle().clone();
                match app.global_shortcut().on_shortcut(
                    cb_hotkey.as_str(),
                    move |_app, _shortcut, event| {
                        if event.state != ShortcutState::Pressed { return; }
                        info!("Clipboard hotkey triggered");
                        commands::window::show_floating_at_mouse(&cb_window);
                        // Delay event so the window has time to show and the JS listener is ready
                        let handle = cb_app_handle.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(150));
                            let _ = handle.emit("clipboard_history_mode", ());
                        });
                    },
                ) {
                    Ok(_) => info!("✅ Registered clipboard hotkey: {}", cb_hotkey),
                    Err(e) => warn!("❌ Failed to register clipboard hotkey {}: {}", cb_hotkey, e),
                }
            }

            // Hot-reload hotkeys when config changes
            let current_hotkey = Arc::new(std::sync::Mutex::new(active_hotkey));
            let current_cb_hotkey: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(
                config.get_clipboard_hotkey_string()
            ));
            let hotkey_app = app.handle().clone();
            let hotkey_window = floating_window.clone();
            let hotkey_current = current_hotkey.clone();
            let hotkey_cb_current = current_cb_hotkey.clone();
            let hotkey_config = app.state::<AppState>().config.clone();
            app.listen("config_updated", move |_| {
                use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

                let (new_hotkey, new_cb_hotkey) = match hotkey_config.try_lock() {
                    Ok(config) => (config.get_hotkey_string(), config.get_clipboard_hotkey_string()),
                    Err(_) => return,
                };

                let mut current = hotkey_current.lock().unwrap();
                let mut current_cb = hotkey_cb_current.lock().unwrap();
                let main_changed = *current != new_hotkey;
                let cb_changed = *current_cb != new_cb_hotkey;
                if !main_changed && !cb_changed { return; }

                info!("Hotkeys changed — main: {} → {}, clipboard: {:?} → {:?}",
                    *current, new_hotkey, *current_cb, new_cb_hotkey);

                // Unregister all and re-register both
                let _ = hotkey_app.global_shortcut().unregister_all();

                // Register the main hotkey
                let window_clone = hotkey_window.clone();
                match hotkey_app.global_shortcut().on_shortcut(
                    new_hotkey.as_str(),
                    move |_app, _shortcut, event| {
                        if event.state != ShortcutState::Pressed { return; }
                        info!("Hotkey triggered");
                        toggle_floating_window(&window_clone);
                    },
                ) {
                    Ok(_) => {
                        info!("✅ Registered main hotkey: {}", new_hotkey);
                        *current = new_hotkey;
                    }
                    Err(e) => {
                        error!("❌ Failed to register main hotkey {}: {}", new_hotkey, e);
                        let old = current.clone();
                        let window_clone2 = hotkey_window.clone();
                        let _ = hotkey_app.global_shortcut().on_shortcut(
                            old.as_str(),
                            move |_app, _shortcut, event| {
                                if event.state != ShortcutState::Pressed { return; }
                                toggle_floating_window(&window_clone2);
                            },
                        );
                    }
                }

                // Register clipboard hotkey if configured
                if let Some(ref cb_hk) = new_cb_hotkey {
                    let cb_win = hotkey_window.clone();
                    let cb_handle = hotkey_app.clone();
                    match hotkey_app.global_shortcut().on_shortcut(
                        cb_hk.as_str(),
                        move |_app, _shortcut, event| {
                            if event.state != ShortcutState::Pressed { return; }
                            info!("Clipboard hotkey triggered");
                            commands::window::show_floating_at_mouse(&cb_win);
                            let handle = cb_handle.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_millis(150));
                                let _ = handle.emit("clipboard_history_mode", ());
                            });
                        },
                    ) {
                        Ok(_) => info!("✅ Registered clipboard hotkey: {}", cb_hk),
                        Err(e) => warn!("❌ Failed to register clipboard hotkey {}: {}", cb_hk, e),
                    }
                }
                *current_cb = new_cb_hotkey;
            });

            info!("=== Setup Complete ===");

            // Auto-start Pocket TTS server if configured
            if config.pocket_tts.enabled && config.pocket_tts.auto_start && config.pocket_tts.installed {
                info!("Pocket TTS auto-start enabled, spawning server in background");
                let state: tauri::State<'_, AppState> = app.state();
                let config_arc = state.config.clone();
                let tts_proc = state.pocket_tts_process.clone();
                tauri::async_runtime::spawn(async move {
                    let (port, voice, temp, eos_threshold, python) = {
                        let config = config_arc.lock().unwrap();
                        (
                            config.pocket_tts.port,
                            config.pocket_tts.voice.clone(),
                            config.pocket_tts.temp,
                            config.pocket_tts.eos_threshold,
                            config.pocket_tts.python_path.clone()
                                .unwrap_or_else(|| "python".to_string()),
                        )
                    };

                    let script_path = commands::pocket_tts::get_server_script_path();
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
                    commands::pocket_tts::configure_no_window(&mut cmd);

                    match cmd.spawn() {
                        Ok(child) => {
                            info!("Pocket TTS server auto-started (PID: {})", child.id());
                            let mut proc = tts_proc.lock().unwrap();
                            *proc = Some(child);
                        }
                        Err(e) => {
                            warn!("Failed to auto-start Pocket TTS server: {}", e);
                        }
                    }
                });
            }

            // Background app registry scan (deferred from startup for speed)
            // and periodic refresh every hour so the list stays current.
            {
                let state: tauri::State<'_, AppState> = app.state();
                let launcher = state.app_launcher.clone();
                tauri::async_runtime::spawn(async move {
                    // Initial scan — do the heavy work outside the lock
                    match tauri::async_runtime::spawn_blocking(AppLauncher::build_registry).await {
                        Ok(Ok(registry)) => {
                            launcher.lock().await.apply_registry(registry);
                        }
                        Ok(Err(e)) => log::error!("Background app scan failed: {}", e),
                        Err(e) => log::error!("Background app scan task failed: {}", e),
                    }
                    // Periodic refresh every hour
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
                    interval.tick().await; // consume the immediate first tick
                    loop {
                        interval.tick().await;
                        log::info!("Periodic app registry refresh");
                        // Scan outside the lock so find_app calls aren't blocked
                        match tauri::async_runtime::spawn_blocking(AppLauncher::build_registry).await {
                            Ok(Ok(registry)) => {
                                launcher.lock().await.apply_registry(registry);
                            }
                            Ok(Err(e)) => log::error!("Periodic app scan failed: {}", e),
                            Err(e) => log::error!("Periodic app scan task failed: {}", e),
                        }
                    }
                });
            }

            // Start default session on launch if configured
            if config.acp.assistant.start_session_on_launch {
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
                        let cfg = config_arc.lock().unwrap();
                        cfg.acp.assistant.working_directory.clone()
                    };
                    match client.create_session(cwd) {
                        Ok((session_id, models_json)) => {
                            info!("Default session created on launch: {}", session_id);
                            if let Ok(mut fs) = floating_session.lock() {
                                *fs = Some(session_id.clone());
                            }

                            // Store available models
                            let models_value = serde_json::Value::Array(models_json);
                            match serde_json::from_value::<Vec<crate::state::AcpModel>>(models_value.clone()) {
                                Ok(parsed) => {
                                    info!("Storing {} models from session", parsed.len());
                                    if let Ok(mut m) = models_arc.lock() {
                                        *m = parsed;
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to parse models: {}. Raw: {}", e, models_value);
                                }
                            }

                            // Apply default model BEFORE steering (fast round-trip, don't block on LLM)
                            {
                                let default_model = {
                                    let cfg = config_arc.lock().unwrap();
                                    cfg.acp.assistant.default_model.clone()
                                };
                                if let Some(ref model) = default_model {
                                    if !model.is_empty() {
                                        info!("Applying default model: {}", model);
                                        let request = crate::acp_client::AcpRequest {
                                            jsonrpc: "2.0".to_string(),
                                            id: serde_json::json!(4),
                                            method: "_kiro.dev/commands/execute".to_string(),
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
                                }
                            }

                            // Send steering content as the first hidden message
                            let steering_msg = {
                                let cfg = config_arc.lock().unwrap();
                                crate::commands::system::format_steering_message(
                                    &crate::commands::system::assemble_steering_parts(&cfg)
                                )
                            };

                            {
                                info!("Sending steering message ({} chars)", steering_msg.len());
                                if let Err(e) = client.send_chat_streaming(&steering_msg, None) {
                                    error!("Failed to send steering message: {}", e);
                                }
                            }

                        }
                        Err(e) => error!("Failed to create default session on launch: {}", e),
                    }
                });
            }

            // Start the auto-update background loop
            {
                let state: tauri::State<'_, AppState> = app.state();
                updater::start_update_loop(
                    state.updater.clone(),
                    state.config.clone(),
                    app.handle().clone(),
                    state.floating_session_id.clone(),
                    state.acp_client.clone(),
                );
            }

            // Show welcome window on first run
            if !config.first_run_completed {
                info!("First run detected, showing welcome window");
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    // Small delay to let the app finish initializing
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    let _ = commands::system::open_welcome_window(app_handle).await;
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::send_message_streaming,
            commands::check_connection,
            commands::open_chat_with_message,
            commands::get_config,
            commands::save_config,
            commands::open_settings_window,
            commands::reconnect_acp,
            commands::handle_floating_input,
            commands::launch_app_by_name,
            commands::open_url,
            commands::fetch_link_metadata,
            commands::open_path,
            commands::execute_shortcut,
            commands::test_floating_window,
            commands::start_drag_window,
            commands::open_chat_window,
            commands::resize_floating_window,
            commands::send_permission_response,
            commands::remove_tool_permission,
            commands::update_tool_policy,
            commands::is_dev_mode,
            commands::open_devtools,
            commands::capture_hotkey_combo,
            commands::cancel_hotkey_capture,
            commands::try_register_hotkey,
            commands::get_app_info,
            commands::open_welcome_window,
            commands::complete_first_run,
            commands::is_first_run,
            commands::get_startup_enabled,
            commands::set_startup_enabled,
            commands::quit_app,
            commands::restart_app,
            commands::read_clipboard,
            commands::resolve_directories,
            commands::get_clipboard_history,
            commands::paste_clipboard_item,
            commands::fetch_favicon,
            commands::record_shortcut_usage,
            commands::get_shortcut_history,
            commands::search_files,
            commands::get_calendar_events,
            commands::get_calendar_events_for_date,
            commands::show_context_menu,
            commands::set_floating_opacity,
            commands::apply_chat_window_size,
            commands::save_window_position,
            commands::save_chat_window_geometry,
            commands::get_last_selection,
            commands::set_notification_source,
            commands::show_notification_source_window,
            commands::get_user_info,
            commands::list_sessions,
            commands::load_session,
            commands::switch_acp_session,
            commands::rename_session,
            commands::reveal_session_file,
            commands::get_sessions_directory,
            commands::delete_session,
            commands::get_current_session_id,
            commands::get_floating_session_id,
            commands::restore_floating_session,
            commands::get_steering_content,
            commands::open_auto_steering_file,
            commands::get_auto_steering_path,
            commands::send_steering_message,
            commands::dismiss_pending_permission,
            commands::has_pending_permission,
            commands::get_slash_commands,
            commands::execute_slash_command,
            commands::get_slash_command_options,
            commands::get_available_models,
            commands::check_for_update,
            commands::fetch_changelog,
            commands::get_update_urls,
            commands::download_and_install_update,
            commands::was_just_updated,
            commands::clear_update_flag,
            commands::touch_floating_activity,
            commands::execute_system_command,
            commands::cancel_generation,
            commands::save_frecency,
            commands::load_frecency,
            commands::list_extensions,
            commands::list_themes,
            commands::list_command_packs,
            commands::get_extension_config,
            commands::save_extension_config,
            commands::set_extension_enabled,
            commands::load_theme_colors,
            commands::install_extension_from_path,
            commands::uninstall_extension,
            commands::open_store_window,
            commands::store_get_catalog,
            commands::store_get_detail,
            commands::store_install,
            commands::check_extension_updates,
            commands::read_extension_file,
            commands::save_store_url,
            commands::pocket_tts_check_install,
            commands::pocket_tts_install,
            commands::pocket_tts_cancel_install,
            commands::pocket_tts_start,
            commands::pocket_tts_stop,
            commands::pocket_tts_voices,
            commands::pocket_tts_test,
            commands::execute_automation_plan,
            commands::extension_tool_response,
            commands::send_extension_tool_steering,
            commands::check_extension_tool_permission,
            commands::pick_folder,
            commands::scan_folder,
            commands::execute_folder_plan,
            commands::get_common_folders,
            commands::notify_frontend_ready,
            commands::list_open_windows,
            commands::focus_open_window,
            commands::get_app_icon,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    info!("Application shutting down");
}
