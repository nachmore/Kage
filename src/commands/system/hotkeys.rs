//! Global hotkey registration and capture flow.
//!
//! `register_all_hotkeys` is the single source of truth — every config
//! save / app startup / capture-completion calls it. `try_register_hotkey`
//! probes a candidate combo without persisting; `capture_hotkey_combo`
//! drives the OS-level capture (Windows uses a low-level keyboard hook).

use crate::error::AppError;
use crate::events;
use crate::hotkey_norm::normalize_hotkey;
use crate::lock_ext::LockExt;
use crate::state::FeatureServices;
use crate::window_labels;
use log::{error, info, warn};
use tauri::{Emitter, Manager};

/// Register all global hotkeys from config. Unregisters everything first.
/// This is the single source of truth for hotkey registration — called from:
/// - App startup (main.rs)
/// - Config changes (config_updated listener)
/// - After hotkey capture (capture_hotkey_combo)
/// - After hotkey test (try_register_hotkey)
pub fn register_all_hotkeys(app: &tauri::AppHandle) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    info!("Registering all hotkeys...");
    let _ = app.global_shortcut().unregister_all();

    let features: tauri::State<'_, FeatureServices> = app.state();
    let config = features.config.lock_or_recover();
    let main_hk = config.get_hotkey_string();
    let cb_hk = config.get_clipboard_hotkey_string();
    let ia_hk = config.get_inline_assist_hotkey_string();
    let voice_hk = config.get_voice_hotkey_string();
    drop(config);

    // --- Main hotkey: toggle floating window (unique behavior) ---
    if let Some(floating) = app.get_webview_window(window_labels::FLOATING) {
        match app
            .global_shortcut()
            .on_shortcut(main_hk.as_str(), move |_app, _shortcut, event| {
                if event.state != ShortcutState::Pressed {
                    return;
                }
                info!("Hotkey triggered: main ({})", _shortcut);
                crate::commands::window::toggle_floating_window(&floating);
            }) {
            Ok(_) => info!("✅ Registered main hotkey: {}", main_hk),
            Err(e) => error!("❌ Failed to register main hotkey {}: {}", main_hk, e),
        }
    }

    // --- Inline assist hotkey: capture selection + show assist (unique behavior) ---
    if let Some(ref ia) = ia_hk {
        let ia_handle = app.clone();
        match app
            .global_shortcut()
            .on_shortcut(ia.as_str(), move |_app, _shortcut, event| {
                if event.state != ShortcutState::Pressed {
                    return;
                }
                info!("Hotkey triggered: inline-assist ({})", _shortcut);
                let source_info = crate::os::window_list::get_foreground_window_info();
                let features: tauri::State<'_, FeatureServices> = ia_handle.state();
                let blocklist = features
                    .config
                    .lock_or_recover()
                    .system
                    .capture_selection_blocklist
                    .clone();
                let fg_process = source_info
                    .as_ref()
                    .map(|(_, proc)| proc.as_str())
                    .unwrap_or("");
                let selection =
                    if crate::os::clipboard::is_process_blocklisted(fg_process, &blocklist) {
                        info!(
                            "[inline-assist] Skipping capture — foreground app '{}' is blocklisted",
                            fg_process
                        );
                        None
                    } else {
                        let capture_token = crate::os::clipboard::begin_selection_capture();
                        crate::os::clipboard::finish_selection_capture(capture_token)
                    };
                let cursor_pos = crate::os::cursor::get_cursor_position().unwrap_or((500, 500));
                let handle = ia_handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::commands::window::show_inline_assist_with_context(
                        handle,
                        source_info,
                        selection,
                        cursor_pos,
                    )
                    .await
                    {
                        error!("Failed to show inline assist: {}", e);
                    }
                });
            }) {
            Ok(_) => info!("✅ Registered inline-assist hotkey: {}", ia),
            Err(e) => warn!("❌ Failed to register inline-assist hotkey {}: {}", ia, e),
        }
    } else {
        info!("ℹ️ No inline-assist hotkey configured");
    }

    // --- Event-based hotkeys: show floating at mouse, then emit a frontend event ---
    // To add a new hotkey of this type: add a config getter and an entry here.
    let event_hotkeys: Vec<(&str, Option<String>, &str, u64)> = vec![
        // (name,        hotkey_string, event_name,              delay_ms)
        ("clipboard", cb_hk, events::CLIPBOARD_HISTORY_MODE, 150),
        ("voice", voice_hk, events::VOICE_MODE, 200),
    ];

    for (name, hk_opt, event_name, delay_ms) in event_hotkeys {
        match hk_opt {
            Some(ref hk) => {
                if let Some(floating) = app.get_webview_window(window_labels::FLOATING) {
                    let app_handle = app.clone();
                    let evt = event_name.to_string();
                    let label = name.to_string();
                    match app.global_shortcut().on_shortcut(
                        hk.as_str(),
                        move |_app, _shortcut, event| {
                            if event.state != ShortcutState::Pressed {
                                return;
                            }
                            info!("Hotkey triggered: {} ({})", label, _shortcut);
                            crate::commands::window::show_floating_at_mouse(&floating);
                            let handle = app_handle.clone();
                            let evt = evt.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                                let _ = handle.emit(&evt, ());
                            });
                        },
                    ) {
                        Ok(_) => info!("✅ Registered {} hotkey: {}", name, hk),
                        Err(e) => warn!("❌ Failed to register {} hotkey {}: {}", name, hk, e),
                    }
                }
            }
            None => info!("ℹ️ No {} hotkey configured", name),
        }
    }
}

#[tauri::command]
pub async fn try_register_hotkey(
    app: tauri::AppHandle,
    modifiers: Vec<String>,
    key: String,
    slot: Option<String>,
) -> Result<bool, AppError> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let hotkey_str = if modifiers.is_empty() {
        key.clone()
    } else {
        format!("{}+{}", modifiers.join("+"), key)
    };
    info!(
        "Trying to register hotkey: {} (slot: {:?})",
        hotkey_str, slot
    );

    // Check for conflicts with other hotkey slots
    {
        let features: tauri::State<'_, FeatureServices> = app.state();
        let config = features.config.lock_or_recover();
        let main_hk = config.get_hotkey_string();
        let cb_hk = config.get_clipboard_hotkey_string();
        let ia_hk = config.get_inline_assist_hotkey_string();
        let slot_name = slot.as_deref().unwrap_or("main");

        let new_norm = normalize_hotkey(&hotkey_str);

        // Check all other slots for conflicts
        let all_hotkeys: Vec<(&str, String)> = [
            ("main", Some(main_hk)),
            ("clipboard", cb_hk),
            ("inline-assist", ia_hk),
        ]
        .into_iter()
        .filter(|(name, _)| *name != slot_name)
        .filter_map(|(name, hk)| hk.map(|h| (name, normalize_hotkey(&h))))
        .collect();

        for (name, norm) in &all_hotkeys {
            if new_norm == *norm {
                return Err(format!("This shortcut is already used as the {} hotkey", name).into());
            }
        }

        // If it's the same as the current value for this slot, no change needed
        let current_for_slot = match slot_name {
            "main" => Some(normalize_hotkey(&config.get_hotkey_string())),
            "clipboard" => config
                .get_clipboard_hotkey_string()
                .map(|s| normalize_hotkey(&s)),
            "inline-assist" => config
                .get_inline_assist_hotkey_string()
                .map(|s| normalize_hotkey(&s)),
            _ => None,
        };
        if current_for_slot.as_deref() == Some(new_norm.as_str()) {
            return Ok(true);
        }
    }

    // Test that the hotkey can be registered
    let _ = app.global_shortcut().unregister_all();
    match app
        .global_shortcut()
        .on_shortcut(hotkey_str.as_str(), |_app, _shortcut, _event| {})
    {
        Ok(_) => {
            info!("✅ Hotkey test passed: {}", hotkey_str);
            // Re-register all hotkeys (the config hasn't been saved yet,
            // but the frontend will save it and trigger config_updated)
            register_all_hotkeys(&app);
            Ok(true)
        }
        Err(e) => {
            let msg = format!("{}", e);
            info!("❌ Hotkey registration failed: {}", msg);
            // Restore all hotkeys from config
            register_all_hotkeys(&app);
            Err(msg.into())
        }
    }
}

#[tauri::command]
pub async fn capture_hotkey_combo(app: tauri::AppHandle) -> Result<serde_json::Value, AppError> {
    // Temporarily unregister global hotkeys so they don't intercept during capture
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let _ = app.global_shortcut().unregister_all();

    let result = tauri::async_runtime::spawn_blocking(|| crate::os::capture_hotkey(10000))
        .await
        .map_err(|e| format!("Task error: {}", e))?;

    // Re-register all global hotkeys from config
    register_all_hotkeys(&app);

    match result {
        Some(captured) => Ok(serde_json::json!({
            "modifiers": captured.modifiers,
            "key": captured.key,
            "display": captured.display,
        })),
        None => Ok(serde_json::json!(null)),
    }
}

#[tauri::command]
pub async fn cancel_hotkey_capture() -> Result<(), AppError> {
    crate::os::cancel_hotkey_capture();
    Ok(())
}
