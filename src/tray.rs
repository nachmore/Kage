use log::info;
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

/// Build and configure the system tray icon with menu
pub fn setup_tray(app: &mut tauri::App, dev_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItemBuilder::with_id("show", "Show").build(app)?;
    let new_chat_window =
        MenuItemBuilder::with_id("new-chat-window", "New Chat Window").build(app)?;
    let settings = MenuItemBuilder::with_id("settings", "Settings").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

    let menu = if dev_mode {
        info!("Dev mode enabled - adding developer menu items");
        let inspect = MenuItemBuilder::with_id("inspect", "Inspect Chat").build(app)?;
        let inspect_floating =
            MenuItemBuilder::with_id("inspect-floating", "Inspect Floating").build(app)?;
        let reload = MenuItemBuilder::with_id("reload", "Reload UX").build(app)?;
        let test_banner =
            MenuItemBuilder::with_id("test-welcome-banner", "Test Welcome Banner").build(app)?;
        let test_update =
            MenuItemBuilder::with_id("test-update-banner", "Test Update Banner").build(app)?;
        let test_update_avail =
            MenuItemBuilder::with_id("test-update-available", "Test Update Available")
                .build(app)?;
        let test_first_run =
            MenuItemBuilder::with_id("test-first-run", "Show First Run").build(app)?;
        let dump_threads = MenuItemBuilder::with_id("dump-threads", "Dump Threads").build(app)?;
        MenuBuilder::new(app)
            .items(&[&show, &new_chat_window, &settings])
            .separator()
            .items(&[
                &inspect,
                &inspect_floating,
                &reload,
                &test_banner,
                &test_update,
                &test_update_avail,
                &test_first_run,
                &dump_threads,
            ])
            .separator()
            .item(&quit)
            .build()?
    } else {
        MenuBuilder::new(app)
            .items(&[&show, &new_chat_window, &settings])
            .separator()
            .item(&quit)
            .build()?
    };

    // Load tray icon — use 128px source so Windows can downscale crisply at any DPI.
    // If decoding fails for some reason (corrupted asset), fall back to the window
    // icon, or as a last-ditch skip setting an icon rather than panicking.
    let icon_bytes = include_bytes!("../icons/128x128.png");
    let icon = tauri::image::Image::from_bytes(icon_bytes)
        .ok()
        .or_else(|| app.default_window_icon().cloned());
    if icon.is_none() {
        log::warn!("No tray icon available — using system default");
    }

    let app_handle = app.handle().clone();
    let mut tray_builder = TrayIconBuilder::with_id("main-tray");
    if let Some(icon) = icon {
        tray_builder = tray_builder.icon(icon);
    }
    tray_builder
        .menu(&menu)
        .on_menu_event(move |app_handle_inner, event| {
            info!("System tray menu item clicked: {}", event.id().as_ref());
            match event.id().as_ref() {
                "show" => {
                    if let Some(window) = app_handle_inner.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                    crate::setup::update_activation_policy(app_handle_inner);
                }
                "new-chat-window" => {
                    let app_clone = app_handle_inner.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) =
                            crate::commands::window::open_new_chat_window(None, app_clone).await
                        {
                            log::warn!("Tray: open_new_chat_window failed: {:?}", e);
                        }
                    });
                }
                "settings" => {
                    // Delegate to open_settings_window so the
                    // create-on-demand path lives in one place.
                    let app_clone = app_handle_inner.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) =
                            crate::commands::window::open_settings_window(app_clone, None, None)
                                .await
                        {
                            log::warn!("Tray: open_settings_window failed: {}", e);
                        }
                    });
                }
                "inspect" => {
                    info!("Opening chat inspector");
                    #[cfg(debug_assertions)]
                    if let Some(window) = app_handle_inner.get_webview_window("main") {
                        window.open_devtools();
                    }
                }
                "inspect-floating" => {
                    info!("Opening floating inspector");
                    #[cfg(debug_assertions)]
                    if let Some(window) = app_handle_inner.get_webview_window("floating") {
                        let _ = window.show();
                        window.open_devtools();
                    }
                }
                "reload" => {
                    info!("Reloading UX");
                    if let Some(window) = app_handle_inner.get_webview_window("main") {
                        let _ = window.eval("window.location.reload()");
                    }
                    if let Some(window) = app_handle_inner.get_webview_window("floating") {
                        let _ = window.eval("window.location.reload()");
                    }
                    if let Some(window) = app_handle_inner.get_webview_window("settings") {
                        let _ = window.eval("window.location.reload()");
                    }
                }
                "test-welcome-banner" => {
                    info!("Testing welcome banner");
                    crate::commands::system::show_welcome_banner(app_handle_inner);
                }
                "test-update-banner" => {
                    info!("Testing update banner");
                    crate::commands::system::simulate_update_complete(app_handle_inner);
                }
                "test-first-run" => {
                    info!("Showing first run experience");
                    let app_for_welcome = app_handle_inner.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = crate::commands::system::open_welcome_window(app_for_welcome).await;
                    });
                }
                "test-update-available" => {
                    use tauri::Emitter;
                    info!("Testing update available banner");
                    if let Some(floating) = app_handle_inner.get_webview_window("floating") {
                        let _ = floating.show();
                        let _ = floating.set_focus();
                    }
                    let _ = app_handle_inner.emit("update_available", "99.0.0");
                }
                "dump-threads" => {
                    info!("Dumping thread info...");
                    tauri::async_runtime::spawn(async {
                        match crate::commands::system::dump_thread_info().await {
                            Ok(output) => info!("Thread dump complete:\n{}", output),
                            Err(e) => log::error!("Thread dump failed: {}", e),
                        }
                    });
                }
                "quit" => {
                    info!("Application quit requested");
                    crate::commands::system::graceful_shutdown(app_handle_inner);

                    let app_for_exit = app_handle_inner.clone();
                    tauri::async_runtime::spawn(async move {
                        crate::commands::system::shutdown_and_exit(&app_for_exit).await;
                    });
                }
                _ => {}
            }
        })
        .on_tray_icon_event(move |_tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                info!("System tray left clicked");
                if let Some(window) = app_handle.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                crate::setup::update_activation_policy(&app_handle);
            }
        })
        .build(app)?;

    Ok(())
}
