mod acp_client;
mod app_launcher;
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

    let config_for_setup = config.clone();
    let dev_mode_for_setup = dev_mode;

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_global_shortcut::Builder::default().build())
        .manage(AppState {
            acp_client: Arc::new(Mutex::new(acp_client)),
            config: Arc::new(Mutex::new(config)),
            app_launcher: Arc::new(Mutex::new(app_launcher)),
            pipe_stdin: pipe_stdin_handle,
            tcp_writer: tcp_writer_handle,
            dev_mode,
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
            commands::list_sessions,
            commands::load_session
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    info!("Application shutting down");
}
