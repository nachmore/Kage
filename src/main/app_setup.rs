use crate::acp_client::AcpClient;
use crate::commands;
use crate::config::Config;
#[cfg(target_os = "macos")]
use crate::window_labels;
use crate::{os, process_manager::ProcessManager, setup, tray, webview_recovery};
use log::{info, warn};
use std::sync::{Arc, Mutex};
#[cfg(target_os = "macos")]
use tauri::Manager;

type SessionWatcher = Arc<Mutex<Option<commands::sessions::SessionWatcherHandle>>>;

#[allow(clippy::too_many_arguments)]
pub fn configure(
    app: &mut tauri::App,
    dev_mode: bool,
    main_args: &[String],
    config: &Config,
    acp_client: Arc<AcpClient>,
    config_for_handler: Arc<Mutex<Config>>,
    slash_commands: Arc<Mutex<Vec<crate::state::SlashCommand>>>,
    pending_permissions: Arc<
        Mutex<std::collections::HashMap<String, crate::state::PendingPermission>>,
    >,
    session_watcher: &SessionWatcher,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Setting up application");
    info!("=== Kage Setup ===");
    // First thing: probe the user's login-shell environment on a
    // background thread, so the launch-time agent spawn (and the MCP
    // servers it starts) rarely has to wait for it. Setup only runs in the
    // primary instance, so a forwarded second launch never probes. The
    // Settings toggle goes in first: off means no probe at all.
    os::login_env::set_enabled(config.acp.agent.import_login_shell_env);
    os::login_env::prewarm();

    #[cfg(target_os = "macos")]
    configure_macos_menu(app)?;

    #[cfg(target_os = "macos")]
    {
        use tauri::ActivationPolicy;
        if let Err(e) = app
            .handle()
            .set_activation_policy(ActivationPolicy::Accessory)
        {
            warn!("Failed to set initial activation policy: {}", e);
        } else {
            info!("Set initial activation policy to Accessory");
        }
    }

    let resume_session_id = crate::paths::config_dir()
        .map(|d| d.join("kage"))
        .and_then(|cfg_dir| crate::startup::resolve_resume_session_id(main_args, &cfg_dir));
    if let Some(ref id) = resume_session_id {
        info!("Resume marker present: will attempt to load session {}", id);
    }

    info!("Checking for orphaned processes...");
    std::thread::spawn(|| {
        if let Err(e) = ProcessManager::cleanup_orphaned_processes() {
            warn!("Failed to cleanup orphaned processes: {}", e);
        }
    });
    // Off the main thread: spawns schtasks on Windows (no-op unless a
    // legacy Run-key autostart needs upgrading to the Scheduled Task).
    std::thread::spawn(os::migrate_startup_mechanism);
    os::install_kill_on_exit_job();
    webview_recovery::set_app_handle(app.handle().clone());

    if let Err(e) = tray::setup_tray(app, dev_mode) {
        log::error!(
            "Failed to set up system tray (continuing without it): {}",
            e
        );
    }
    commands::messaging::setup_notification_handler(
        acp_client,
        app.handle(),
        config_for_handler,
        slash_commands,
        pending_permissions,
    );
    // The host-command bridge: agents' `run_outside_sandbox` calls reach
    // Kage through it (`host_commands`). State is managed on the Builder;
    // binding happens on its own thread so startup never waits for it.
    {
        use tauri::Manager as _;
        if let Some(procs) = app.try_state::<crate::state::ChildProcesses>() {
            let host_commands = procs.host_commands.clone();
            let handle = app.handle().clone();
            std::thread::spawn(move || host_commands.start(handle));
        }
    }
    setup::configure_transparent_windows(app);
    crate::overlay_windows::configure_all(app);
    crate::commands::window::preload_context_menu(app.handle());
    commands::system::register_all_hotkeys(app.handle());
    setup::install_hotkey_hot_reload(app, config);
    info!("=== Setup Complete ===");

    setup::install_show_sessions_listener(app);
    setup::install_deep_link_handler(app);
    setup::install_main_focus_tracker(app);
    setup::spawn_automation_scheduler(app);
    setup::maybe_autostart_pocket_tts(app, config);

    if let Some(handle) = setup::start_session_watcher(app) {
        if let Ok(mut slot) = session_watcher.lock() {
            *slot = Some(handle);
        }
    }

    setup::refresh_mcp_registration_if_enabled();
    setup::spawn_app_registry_scan(app);
    setup::spawn_changelog_cache_refresh(app);
    setup::maybe_spawn_default_session(app, config, resume_session_id);
    setup::start_updater(app);
    setup::maybe_show_welcome_window(app.handle(), config.first_run_completed);
    setup::maybe_show_floating_after_interactive_install(app);
    Ok(())
}

#[cfg(target_os = "macos")]
fn configure_macos_menu(app: &mut tauri::App) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};

    let hide_item = MenuItemBuilder::with_id("macos-hide", "Hide Kage")
        .accelerator("CmdOrCtrl+Q")
        .build(app)?;
    let hide_others = MenuItemBuilder::with_id("macos-hide-others", "Hide Others")
        .accelerator("CmdOrCtrl+Alt+H")
        .build(app)?;
    let show_all = MenuItemBuilder::with_id("macos-show-all", "Show All").build(app)?;
    let quit_item = MenuItemBuilder::with_id("macos-quit", "Quit Kage")
        .accelerator("CmdOrCtrl+Shift+Q")
        .build(app)?;
    let app_submenu = SubmenuBuilder::new(app, "Kage")
        .items(&[&hide_item, &hide_others, &show_all])
        .separator()
        .item(&quit_item)
        .build()?;
    // WKWebView's ⌘C/⌘V/⌘X/⌘A/⌘Z come from the Edit menu's key
    // equivalents (copy:, paste:, …). Replacing Tauri's default menu with
    // only the Kage submenu silently disabled them in every input — they
    // still work this way while an overlay panel is key and Kage isn't
    // the active app.
    let edit_submenu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    app.set_menu(
        MenuBuilder::new(app)
            .item(&app_submenu)
            .item(&edit_submenu)
            .build()?,
    )?;

    let app_handle = app.handle().clone();
    app.on_menu_event(move |app, event| match event.id().as_ref() {
        "macos-hide" => {
            info!("Cmd+Q: hiding all windows (use tray Quit to exit)");
            for (_, window) in app.webview_windows() {
                let _ = window.hide();
            }
            setup::hide_macos_app();
            setup::update_activation_policy(app);
        }
        "macos-hide-others" => {
            for (_, window) in app.webview_windows() {
                let _ = window.hide();
            }
            setup::hide_macos_app();
            setup::update_activation_policy(app);
        }
        "macos-show-all" => {
            if let Some(window) = app.get_webview_window(window_labels::FLOATING) {
                let _ = window.show();
            }
        }
        "macos-quit" => {
            info!("Cmd+Shift+Q: quitting application");
            crate::commands::system::graceful_shutdown(&app_handle);
            let app_for_exit = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                crate::commands::system::shutdown_and_exit(&app_for_exit).await;
            });
        }
        _ => {}
    });
    Ok(())
}
