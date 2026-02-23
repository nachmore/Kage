mod acp_client;
mod app_launcher;
mod auto_steering;
mod commands;
mod config;
mod logger;
mod os;
mod process_manager;
mod state;
mod tray;

use acp_client::AcpClient;
use app_launcher::AppLauncher;
use commands::window::toggle_floating_window;
use config::Config;
use log::{error, info, warn};
use process_manager::ProcessManager;
use state::AppState;
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::Mutex;

fn main() {
    // Initialize logger first
    if let Err(e) = logger::init_logger() {
        eprintln!("Failed to initialize logger: {}", e);
        eprintln!("Continuing without file logging...");
    }

    info!("=== Kiro Assistant Starting ===");

    let args: Vec<String> = std::env::args().collect();
    let dev_mode = args.iter().any(|arg| arg == "/dev" || arg == "--dev");
    let debug_mode = args.iter().any(|arg| arg == "/debug" || arg == "--debug");

    if debug_mode {
        println!("🐛 DEBUG MODE ENABLED - Detailed ACP logs will be printed to console");
        info!("🐛 DEBUG MODE ENABLED via command line argument");
        logger::enable_console_logging();
    }

    info!("Checking for orphaned processes...");
    if let Err(e) = ProcessManager::cleanup_orphaned_processes() {
        warn!("Failed to cleanup orphaned processes: {}", e);
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
    info!("App launcher initialized");

    let pipe_stdin_handle = acp_client.get_pipe_stdin();
    let tcp_writer_handle = acp_client.get_tcp_writer();

    let pipe_stdin_for_handler = pipe_stdin_handle.clone();
    let tcp_writer_for_handler = tcp_writer_handle.clone();
    let config_for_setup = config.clone();
    let dev_mode_for_setup = dev_mode;

    let acp_client_arc = Arc::new(Mutex::new(acp_client));
    let config_arc = Arc::new(Mutex::new(config));
    let slash_commands_arc = Arc::new(std::sync::Mutex::new(Vec::new()));
    let pending_permission_arc = Arc::new(std::sync::Mutex::new(None));
    let available_models_arc = Arc::new(std::sync::Mutex::new(Vec::<crate::state::AcpModel>::new()));

    // Clone Arcs for the notification handler setup
    let config_for_handler = config_arc.clone();
    let slash_cmds_for_handler = slash_commands_arc.clone();
    let pending_perm_for_handler = pending_permission_arc.clone();
    let acp_for_handler = acp_client_arc.clone();
    let models_for_handler = available_models_arc.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_global_shortcut::Builder::default().build())
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
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                window.hide().unwrap();
                api.prevent_close();
            }
        })
        .setup(move |app| {
            info!("Setting up application");
            println!("=== KIRO ASSISTANT SETUP ===");

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
            println!("Attempting to register global hotkey: {}", hotkey_string);

            use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

            let window_for_primary = floating_window.clone();
            let hotkey_str = hotkey_string.clone();
            let registration_result = app.global_shortcut().on_shortcut(
                hotkey_string.as_str(),
                move |_app, _shortcut, event| {
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    println!(
                        "🔥 HOTKEY TRIGGERED: {}",
                        chrono::Local::now().format("%H:%M:%S%.3f")
                    );
                    info!("Hotkey triggered");
                    toggle_floating_window(&window_for_primary);
                },
            );

            let hotkey = match registration_result {
                Ok(_) => {
                    info!("✅ Successfully registered global hotkey: {}", hotkey_str);
                    println!("✅ Successfully registered global hotkey: {}", hotkey_str);
                    println!("   Press {} to toggle the floating window", hotkey_str);
                    hotkey_str
                }
                Err(e) => {
                    warn!("❌ Failed to register {}: {}", hotkey_str, e);
                    eprintln!("❌ Failed to register {}: {}", hotkey_str, e);
                    eprintln!("   Trying Alt+K instead...");

                    let window_for_fallback = floating_window.clone();
                    match app.global_shortcut().on_shortcut(
                        "Alt+K",
                        move |_app, _shortcut, event| {
                            if event.state != ShortcutState::Pressed {
                                return;
                            }
                            println!(
                                "🔥 HOTKEY TRIGGERED (Alt+K): {}",
                                chrono::Local::now().format("%H:%M:%S%.3f")
                            );
                            info!("Hotkey triggered (Alt+K)");
                            toggle_floating_window(&window_for_fallback);
                        },
                    ) {
                        Ok(_) => {
                            info!("✅ Successfully registered fallback hotkey: Alt+K");
                            println!("✅ Successfully registered fallback hotkey: Alt+K");
                            "Alt+K".to_string()
                        }
                        Err(e2) => {
                            error!("❌ Failed to register fallback hotkey: {}", e2);
                            eprintln!("❌ Failed to register any hotkey: {}", e2);
                            "None".to_string()
                        }
                    }
                }
            };

            info!("Active hotkey: {}", hotkey);
            println!("=== SETUP COMPLETE ===");
            println!("Active hotkey: {}", hotkey);
            println!("Floating window initial state: hidden");
            println!();

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
                        let cfg = config_arc.lock().await;
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

                            // Send steering content as the first hidden message
                            let cfg = config_arc.lock().await;
                            let assistant = &cfg.acp.assistant;
                            let mut steering_parts: Vec<String> = Vec::new();

                            // Built-in steering (always first)
                            steering_parts.push(crate::commands::system::BUILTIN_STEERING.to_string());

                            // User steering (precedence)
                            if let Some(ref path) = assistant.user_steering_path {
                                if !path.is_empty() {
                                    if let Ok(content) = std::fs::read_to_string(path) {
                                        if !content.trim().is_empty() {
                                            steering_parts.push(content);
                                        }
                                    }
                                }
                            }
                            // Auto steering
                            if assistant.auto_steering_enabled {
                                if let Ok(auto_path) = crate::config::Config::get_auto_steering_path() {
                                    if auto_path.exists() {
                                        if let Ok(content) = std::fs::read_to_string(&auto_path) {
                                            if !content.trim().is_empty() {
                                                steering_parts.push(content);
                                            }
                                        }
                                    }
                                }
                            }
                            drop(cfg);

                            {
                                let steering_msg = format!(
                                    "{} {}",
                                    crate::commands::system::STEERING_MSG_PREFIX,
                                    steering_parts.join("\n\n---\n\n")
                                );
                                info!("Sending steering message ({} chars)", steering_msg.len());
                                if let Err(e) = client.send_chat_streaming(steering_msg, None) {
                                    error!("Failed to send steering message: {}", e);
                                }
                            }

                            // Apply default model if configured
                            let cfg = config_arc.lock().await;
                            if let Some(ref default_model) = cfg.acp.assistant.default_model {
                                if !default_model.is_empty() {
                                    info!("Applying default model: {}", default_model);
                                    let request = crate::acp_client::AcpRequest {
                                        jsonrpc: "2.0".to_string(),
                                        id: serde_json::json!(4),
                                        method: "_kiro.dev/commands/execute".to_string(),
                                        params: serde_json::json!({
                                            "sessionId": session_id,
                                            "command": { "command": "model", "args": { "modelName": default_model } }
                                        }),
                                    };
                                    match client.send_request(&request) {
                                        Ok(_) => info!("Default model applied: {}", default_model),
                                        Err(e) => error!("Failed to apply default model: {}", e),
                                    }
                                }
                            }
                            drop(cfg);
                        }
                        Err(e) => error!("Failed to create default session on launch: {}", e),
                    }
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
            commands::quit_app,
            commands::read_clipboard,
            commands::show_context_menu,
            commands::set_floating_opacity,
            commands::apply_chat_window_size,
            commands::save_window_position,
            commands::get_user_info,
            commands::list_sessions,
            commands::load_session,
            commands::switch_acp_session,
            commands::rename_session,
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
            commands::get_available_models
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    info!("Application shutting down");
}
