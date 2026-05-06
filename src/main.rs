// Hide console window on Windows
#![cfg_attr(windows, windows_subsystem = "windows")]

mod acp_client;
mod activity_tracker;
mod agent_presets;
mod app_launcher;
mod app_log;
mod auto_steering;
mod automation;
mod commands;
#[allow(dead_code)] // Consumed by the kage-computer-control-mcp binary, not this one
mod computer_control;
mod config;
mod config_migrations;
mod error;
mod extensions;
mod lock_ext;
mod logger;
mod mcp_registration;
mod os;
mod panic_handler;
mod permission_audit;
mod process_manager;
mod setup;
mod single_instance;
mod startup;
mod state;
mod tray;
mod updater;

use acp_client::AcpClient;
use app_launcher::AppLauncher;
use config::Config;
use log::{error, info, warn};
use process_manager::ProcessManager;
use state::AppState;
use std::sync::Arc;
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
        if let Some(timeout) = startup::detect_capture_hotkey_subcommand(&args) {
            os::windows::hotkey_capture::run_capture_helper(timeout);
            return;
        }
    }

    #[cfg(all(windows, debug_assertions))]
    attach_parent_console();

    // Install the panic hook as early as possible so any panic during startup
    // still gets captured into crash.log.
    panic_handler::install();

    // Initialize logger first
    if let Err(e) = logger::init_logger() {
        eprintln!("Failed to initialize logger: {}", e);
        eprintln!("Continuing without file logging...");
    }

    info!("=== Kage Starting ===");
    let startup_t0 = std::time::Instant::now();

    let args: Vec<String> = std::env::args().collect();
    let flags = startup::CliFlags::parse(&args);

    // Enforce single instance across all builds (debug + release)
    let _instance_lock = match single_instance::try_acquire(flags.is_restart) {
        Ok(lock) => lock,
        Err(e) => {
            // Another instance is running — signal it to show the sessions UI
            info!("Another instance detected, signaling it to show sessions UI");
            single_instance::signal_running_instance();
            // Small delay to ensure the TCP send completes before we exit
            std::thread::sleep(std::time::Duration::from_millis(200));
            info!("{}", e);
            std::process::exit(0);
        }
    };

    // On restart, wait for the old process to fully release WebView2/Tauri resources
    startup::wait_for_previous_instance_if_restart(flags.is_restart);

    let dev_mode = flags.dev_mode;
    let debug_mode = flags.debug_mode;

    // In dev mode, enable Rust backtraces on panic unless the user has
    // already set RUST_BACKTRACE explicitly (e.g. to "full"). This means
    // `cargo tauri dev -- /dev` always produces useful panic traces.
    // RUST_LIB_BACKTRACE controls backtraces captured by std::backtrace
    // on Error types (e.g. anyhow), as opposed to panics.
    if dev_mode {
        // SAFETY: called before any threads are spawned that read these vars.
        if std::env::var_os("RUST_BACKTRACE").is_none() {
            std::env::set_var("RUST_BACKTRACE", "1");
        }
        if std::env::var_os("RUST_LIB_BACKTRACE").is_none() {
            std::env::set_var("RUST_LIB_BACKTRACE", "1");
        }
    }

    // Check for session resume after update — clean up last-session.txt
    let _resume_session_id: Option<String> = dirs::config_dir()
        .map(|d| d.join("kage"))
        .and_then(|cfg_dir| startup::resolve_resume_session_id(&args, &cfg_dir));

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
    os::windows::process::install_kill_on_exit_job();

    let config = startup::load_config_with_overrides(debug_mode, Config::load);

    info!("Configuration loaded");
    if dev_mode { info!("⏱ Config loaded at +{}ms", startup_t0.elapsed().as_millis()); }

    // Initialize the app log ring buffer
    if let Err(e) = app_log::init(config.system.log_buffer_size) {
        warn!("Failed to initialize app log: {}", e);
    } else {
        app_log::log("info", "system", "App log initialized");
    }

    let (acp_connection_mode, acp_mode_desc) = startup::acp_mode_for(&config.acp.mode);
    info!("{}", acp_mode_desc);
    let acp_client = AcpClient::new(acp_connection_mode);

    acp_client.set_debug_mode(config.debug_mode);

    let process_manager = acp_client.get_process_manager();
    process_manager::install_signal_handlers(process_manager);

    // AppLauncher::new() is currently infallible (returns Ok(empty)) but we
    // handle the Err path defensively: if it ever becomes fallible, we fall
    // back to an empty launcher rather than crashing the whole app. The
    // background scan later populates the registry regardless.
    let app_launcher = AppLauncher::new().unwrap_or_else(|e| {
        error!("Failed to initialize app launcher: {}", e);
        eprintln!("Failed to initialize app launcher: {}", e);
        AppLauncher::new().unwrap_or_else(|e2| {
            error!("AppLauncher fallback also failed: {} — continuing without app launcher registry", e2);
            // If even the fallback fails, build a zero-initialized launcher
            // by reusing the no-op path. This can't actually happen with the
            // current implementation (new() just returns an empty HashMap)
            // but we refuse to panic here either way.
            AppLauncher::empty()
        })
    });
    info!("App launcher initialized (scan deferred to background)");
    if dev_mode { info!("⏱ App launcher ready at +{}ms", startup_t0.elapsed().as_millis()); }

    let pipe_stdin_handle = acp_client.get_pipe_stdin();
    let tcp_writer_handle = acp_client.get_tcp_writer();

    let pipe_stdin_for_handler = pipe_stdin_handle.clone();
    let tcp_writer_for_handler = tcp_writer_handle.clone();
    let config_for_setup = config.clone();
    let dev_mode_for_setup = dev_mode;

    let acp_client_arc = Arc::new(acp_client);
    let config_arc = Arc::new(std::sync::Mutex::new(config));
    let slash_commands_arc = Arc::new(std::sync::Mutex::new(Vec::new()));
    let pending_permission_arc = Arc::new(std::sync::Mutex::new(None));
    let available_models_arc = Arc::new(std::sync::Mutex::new(Vec::<crate::state::AcpModel>::new()));

    // Clone Arcs for the notification handler setup
    let config_for_handler = config_arc.clone();
    let slash_cmds_for_handler = slash_commands_arc.clone();
    let pending_perm_for_handler = pending_permission_arc.clone();
    let acp_for_handler = acp_client_arc.clone();

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
            last_selection: Arc::new(std::sync::Mutex::new(None)),
            source_window: Arc::new(std::sync::Mutex::new(None)),
            notification_source: Arc::new(std::sync::Mutex::new("floating".to_string())),
            updater: Arc::new(updater::UpdaterState::new()),
            user_info_cache: Arc::new(std::sync::Mutex::new(None)),
            session_cache: Arc::new(std::sync::Mutex::new(None)),
            pocket_tts_process: Arc::new(std::sync::Mutex::new(None)),
            pocket_tts_install_process: Arc::new(std::sync::Mutex::new(None)),
            automation_plan_cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_tool_steering_hash: Arc::new(std::sync::Mutex::new(0)),
            frontend_ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            activity_tracker: Arc::new(crate::activity_tracker::ActivityTrackerState::new()),
            kage_desktop_cache: Arc::new(crate::commands::kage_desktop::KageDesktopCache::new()),
            automation_signal_tx: Arc::new(std::sync::Mutex::new(None)),
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                setup::handle_window_close(window, api);
            }
        })
        .setup(move |app| {
            info!("Setting up application");
            info!("=== Kage Setup ===");

            let config = config_for_setup;
            let dev_mode = dev_mode_for_setup;

            // Build system tray
            tray::setup_tray(app, dev_mode)?;

            // Set up the ACP notification handler. acp_for_handler is now a
            // bare Arc<AcpClient> — no lock acquisition, no block_on inside
            // .setup() that previously danced around tokio's runtime lifecycle.
            {
                commands::messaging::setup_notification_handler(
                    &acp_for_handler,
                    app.handle(),
                    config_for_handler,
                    pipe_stdin_for_handler,
                    tcp_writer_for_handler,
                    slash_cmds_for_handler,
                    pending_perm_for_handler,
                );
            }

            // Configure floating / context-menu / inline-assist windows for transparency.
            setup::configure_transparent_windows(app);

            // Register all global hotkeys from config
            commands::system::register_all_hotkeys(app.handle());

            // Hot-reload hotkeys when config changes
            setup::install_hotkey_hot_reload(app, &config);

            info!("=== Setup Complete ===");

            // Start IPC listener for second-instance signals
            single_instance::start_ipc_listener(app.handle().clone());

            // Listen for show-sessions event (triggered by second instance)
            setup::install_show_sessions_listener(app);

            // Start automation scheduler
            setup::spawn_automation_scheduler(app);

            // Watchdog: exit early if the frontend doesn't become ready.
            setup::spawn_frontend_watchdog(app);

            // Auto-start Pocket TTS server if configured
            setup::maybe_autostart_pocket_tts(app, &config);

            // Watch the sessions directory for external changes (e.g., kage-cli creating sessions)
            setup::start_session_watcher(app);

            // Background app registry scan (deferred from startup for speed)
            // and periodic refresh every hour so the list stays current.
            setup::spawn_app_registry_scan(app);

            // Start default session on launch if configured
            setup::maybe_spawn_default_session(app, &config);

            // Start the auto-update background loop
            setup::start_updater(app);

            // Show welcome window on first run
            setup::maybe_show_welcome_window(app.handle(), config.first_run_completed);

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
            commands::get_permission_audit_log,
            commands::clear_permission_audit_log,
            commands::get_permission_audit_log_path,
            commands::is_dev_mode,
            commands::is_terminator_mode,
            commands::open_devtools,
            commands::capture_hotkey_combo,
            commands::cancel_hotkey_capture,
            commands::try_register_hotkey,
            commands::get_app_info,
            commands::get_os_dark_mode,
            commands::open_welcome_window,
            commands::complete_first_run,
            commands::is_first_run,
            commands::detect_agents,
            commands::get_startup_enabled,
            commands::set_startup_enabled,
            commands::get_computer_control_enabled,
            commands::set_computer_control_enabled,
            commands::get_mcp_json_path,
            commands::get_mcp_config,
            commands::save_mcp_config,
            commands::kage_desktop_available,
            commands::kage_desktop_workspaces,
            commands::kage_desktop_sessions,
            commands::kage_desktop_load_session,
            commands::kage_desktop_load_chat_file,
            commands::kage_desktop_chat_sessions,
            commands::kage_desktop_delete_session,
            commands::kage_desktop_open_folder,
            commands::kage_cli_available,
            commands::kage_cli_sessions,
            commands::kage_cli_load_session,
            commands::kage_cli_check_updated,
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
            commands::remove_extension_grant,
            commands::commit_extension_install,
            commands::open_store_window,
            commands::store_get_catalog,
            commands::store_get_detail,
            commands::store_install,
            commands::install_bundled_package,
            commands::check_extension_updates,
            commands::read_extension_file,
            commands::save_store_url,
            commands::save_extension_data,
            commands::load_extension_data,
            commands::delete_extension_data,
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
            commands::get_process_name,
            commands::focus_open_window,
            crate::automation::emit_automation_signal,
            crate::automation::get_power_status,
            crate::automation::list_automation_signals,
            commands::start_activity_tracker,
            commands::stop_activity_tracker,
            commands::get_activity_report,
            commands::is_activity_tracker_running,
            commands::get_app_icon,
            commands::get_source_window,
            commands::get_screen_context,
            commands::show_inline_assist,
            commands::inline_assist_apply,
            commands::send_inline_assist,
            commands::execute_macro,
            commands::app_log_write,
            commands::app_log_get_entries,
            commands::app_log_clear,
            commands::app_log_get_dir,
            commands::dump_thread_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    info!("Application shutting down");
}
