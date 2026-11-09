use crate::events;
use crate::lock_ext::LockExt;
use crate::sync::status::{SyncState, SyncStatus};
use crate::t;
use crate::window_labels;
use log::info;
use std::sync::{Mutex, OnceLock};
use tauri::{
    menu::{Menu, MenuBuilder, MenuItem, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

/// The tray's sync lines: a disabled status line and **Sync now**, shown
/// while sync is set up on this computer (docs/design/SYNC.md §13.8).
struct SyncLines {
    menu: Menu<tauri::Wry>,
    separator: PredefinedMenuItem<tauri::Wry>,
    status: MenuItem<tauri::Wry>,
    now: MenuItem<tauri::Wry>,
    shown: bool,
}

static SYNC_LINES: OnceLock<Mutex<SyncLines>> = OnceLock::new();

/// Where the sync lines go: after Show, New Chat Window and Settings.
const SYNC_LINES_AT: usize = 3;

/// The tray's sync status line for `status`, or `None` when sync isn't set
/// up here (then the tray has no sync lines).
pub fn sync_line(status: &SyncStatus) -> Option<String> {
    let key = match status.state {
        SyncState::Off | SyncState::Unavailable => return None,
        SyncState::UpToDate => "tray.sync_state.up_to_date",
        SyncState::Syncing | SyncState::SettingUp => "tray.sync_state.syncing",
        SyncState::Waiting | SyncState::Placeholders => "tray.sync_state.waiting",
        SyncState::Paused => "tray.sync_state.paused",
        _ => "tray.sync_state.attention",
    };
    Some(t!("tray.sync_status", "status" => &t!(key)))
}

/// Show the sync status in the tray menu, or take the sync lines out when
/// sync is off. Posted to the main thread rather than waited for: the
/// engine thread calls it, and shutdown may hold the main thread.
pub fn set_sync_status<R: tauri::Runtime>(app: &tauri::AppHandle<R>, status: &SyncStatus) {
    let line = sync_line(status);
    let posted = app.run_on_main_thread(move || {
        let Some(lines) = SYNC_LINES.get() else {
            return;
        };
        let mut lines = lines.lock_or_recover();
        let result = match line {
            Some(text) => lines.status.set_text(text).and_then(|()| {
                if lines.shown {
                    return Ok(());
                }
                lines.shown = true;
                lines.menu.insert_items(
                    &[&lines.separator, &lines.status, &lines.now],
                    SYNC_LINES_AT,
                )
            }),
            None if lines.shown => {
                lines.shown = false;
                [
                    &lines.separator as &dyn tauri::menu::IsMenuItem<_>,
                    &lines.status,
                    &lines.now,
                ]
                .into_iter()
                .try_for_each(|item| lines.menu.remove(item))
            }
            None => Ok(()),
        };
        if let Err(e) = result {
            log::debug!("Could not update the tray's sync lines: {e}");
        }
    });
    if let Err(e) = posted {
        log::debug!("Could not update the tray's sync lines: {e}");
    }
}

/// Name the extension in the tray tooltip while its background AI request
/// runs, and clear the tooltip when none runs
/// (docs/design/EXTENSION_AGENT_CALLS.md §4.4). Linux trays don't show
/// tooltips, and Tauri ignores the call there.
///
/// Posted to the main thread rather than waited for: the caller may be a
/// background call finishing while shutdown holds the main thread.
pub fn set_agent_call_tooltip<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    extension: Option<&str>,
) {
    let text = extension.map(|name| t!("tray.agent_call_tooltip", "extension" => name));
    let handle = app.clone();
    let posted = app.run_on_main_thread(move || {
        if let Some(tray) = handle.tray_by_id("main-tray") {
            if let Err(e) = tray.set_tooltip(text.as_deref()) {
                log::debug!("Could not set the tray tooltip: {e}");
            }
        }
    });
    if let Err(e) = posted {
        log::debug!("Could not update the tray tooltip: {e}");
    }
}

/// Build and configure the system tray icon with menu.
///
/// Production menu items are translated via the i18n catalog so a German user
/// gets "Anzeigen" / "Beenden" instead of "Show" / "Quit". Dev-only items
/// stay English — they're a debug surface, not a user-facing one.
pub fn setup_tray(app: &mut tauri::App, dev_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItemBuilder::with_id("show", t!("tray.show")).build(app)?;
    let new_chat_window =
        MenuItemBuilder::with_id("new-chat-window", t!("tray.new_chat")).build(app)?;
    let settings = MenuItemBuilder::with_id("settings", t!("tray.settings")).build(app)?;
    let quit = MenuItemBuilder::with_id("quit", t!("tray.quit")).build(app)?;

    let menu = if dev_mode {
        info!("Dev mode enabled - adding developer menu items");
        // Dev-only items stay English. They're surfaced behind /dev mode and
        // exist for engineers debugging the app, not for end users.
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

    // The sync lines go in once the engine reports that sync is set up.
    let sync_lines = SyncLines {
        menu: menu.clone(),
        separator: PredefinedMenuItem::separator(app)?,
        status: MenuItemBuilder::with_id("sync-status", "")
            .enabled(false)
            .build(app)?,
        now: MenuItemBuilder::with_id("sync-now", t!("tray.sync_now")).build(app)?,
        shown: false,
    };
    let _ = SYNC_LINES.set(Mutex::new(sync_lines));

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
                    if let Some(window) = app_handle_inner.get_webview_window(window_labels::MAIN) {
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
                "sync-now" => {
                    // An immediate tick; a wake never blocks.
                    if let Some(features) =
                        app_handle_inner.try_state::<crate::state::FeatureServices>()
                    {
                        features.sync.wake(crate::sync::engine::Wake::Manual);
                    }
                }
                "inspect" => {
                    info!("Opening chat inspector");
                    #[cfg(debug_assertions)]
                    if let Some(window) = app_handle_inner.get_webview_window(window_labels::MAIN) {
                        window.open_devtools();
                    }
                }
                "inspect-floating" => {
                    info!("Opening floating inspector");
                    #[cfg(debug_assertions)]
                    if let Some(window) =
                        app_handle_inner.get_webview_window(window_labels::FLOATING)
                    {
                        let _ = window.show();
                        window.open_devtools();
                    }
                }
                "reload" => {
                    info!("Reloading UX");
                    if let Some(window) = app_handle_inner.get_webview_window(window_labels::MAIN) {
                        let _ = window.eval("window.location.reload()");
                    }
                    if let Some(window) =
                        app_handle_inner.get_webview_window(window_labels::FLOATING)
                    {
                        let _ = window.eval("window.location.reload()");
                    }
                    if let Some(window) =
                        app_handle_inner.get_webview_window(window_labels::SETTINGS)
                    {
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
                    info!("Testing update available banner");
                    if let Some(floating) =
                        app_handle_inner.get_webview_window(window_labels::FLOATING)
                    {
                        let _ = crate::overlay_windows::present(&floating);
                    }
                    crate::event_targets::emit_update_audience(
                        app_handle_inner,
                        events::UPDATE_AVAILABLE,
                        &"99.0.0",
                    );
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
                if let Some(window) = app_handle.get_webview_window(window_labels::MAIN) {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                crate::setup::update_activation_policy(&app_handle);
            }
        })
        .build(app)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(state: SyncState) -> Option<String> {
        crate::i18n::init(Some("en"));
        sync_line(&SyncStatus {
            state,
            ..SyncStatus::default()
        })
    }

    #[test]
    fn the_sync_line_shows_only_while_sync_is_set_up() {
        assert_eq!(line(SyncState::Off), None);
        assert_eq!(line(SyncState::Unavailable), None);
        assert_eq!(
            line(SyncState::UpToDate).as_deref(),
            Some("Sync: Up to date")
        );
        assert_eq!(line(SyncState::Paused).as_deref(), Some("Sync: Paused"));
        for state in [
            SyncState::Locked,
            SyncState::FolderMissing,
            SyncState::Removed,
        ] {
            assert_eq!(line(state).as_deref(), Some("Sync: Needs your attention"));
        }
    }
}
