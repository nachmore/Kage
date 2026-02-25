use crate::state::AppState;
use log::info;
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

/// Build and configure the system tray icon with menu
pub fn setup_tray(app: &mut tauri::App, dev_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItemBuilder::with_id("show", "Show").build(app)?;
    let settings = MenuItemBuilder::with_id("settings", "Settings").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

    let menu = if dev_mode {
        info!("Dev mode enabled - adding developer menu items");
        println!("🔧 Dev mode enabled - adding developer menu items");
        let inspect = MenuItemBuilder::with_id("inspect", "Inspect").build(app)?;
        let reload = MenuItemBuilder::with_id("reload", "Reload UX").build(app)?;
        let test_banner = MenuItemBuilder::with_id("test-welcome-banner", "Test Welcome Banner").build(app)?;
        let test_update = MenuItemBuilder::with_id("test-update-banner", "Test Update Banner").build(app)?;
        MenuBuilder::new(app)
            .items(&[&show, &settings])
            .separator()
            .items(&[&inspect, &reload, &test_banner, &test_update])
            .separator()
            .item(&quit)
            .build()?
    } else {
        MenuBuilder::new(app)
            .items(&[&show, &settings])
            .separator()
            .item(&quit)
            .build()?
    };

    // Load tray icon from embedded PNG
    let icon_bytes = include_bytes!("../ui/assets/kiro-assistant-icon.png");
    let icon = tauri::image::Image::from_bytes(icon_bytes)
        .unwrap_or_else(|_| app.default_window_icon().cloned().unwrap());

    let app_handle = app.handle().clone();
    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .menu(&menu)
        .on_menu_event(move |app_handle_inner, event| {
            info!("System tray menu item clicked: {}", event.id().as_ref());
            match event.id().as_ref() {
                "show" => {
                    if let Some(window) = app_handle_inner.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                "settings" => {
                    if let Some(window) = app_handle_inner.get_webview_window("settings") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                "inspect" => {
                    info!("Opening inspector");
                    #[cfg(debug_assertions)]
                    if let Some(window) = app_handle_inner.get_webview_window("main") {
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
                "quit" => {
                    info!("Application quit requested");

                    // Immediately hide all windows and tray
                    for label in &["floating", "main", "settings", "context-menu"] {
                        if let Some(window) = app_handle_inner.get_webview_window(label) {
                            let _ = window.hide();
                        }
                    }
                    if let Some(tray) = app_handle_inner.tray_by_id("main-tray") {
                        let _ = tray.set_visible(false);
                    }

                    // Generate steering doc in background, then exit
                    if let Some(state) = app_handle_inner.try_state::<AppState>() {
                        let acp_client = state.acp_client.clone();
                        let config = state.config.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Ok(client) = acp_client.try_lock() {
                                if let Ok(config) = config.try_lock() {
                                    crate::auto_steering::generate_steering_on_quit(&client, &config);
                                }
                                client.disconnect();
                            }
                            std::process::exit(0);
                        });
                    } else {
                        std::process::exit(0);
                    }
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
            }
        })
        .build(app)?;

    Ok(())
}
