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

    // Collect (slot, hotkey) pairs the OS refused so we can surface them to
    // the user once at the end, rather than leaving the failure buried in the
    // log. The usual cause is another app already owning the combo.
    let mut failures: Vec<(&str, String)> = Vec::new();

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
            Err(e) => {
                error!("❌ Failed to register main hotkey {}: {}", main_hk, e);
                failures.push(("main", main_hk.clone()));
            }
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
            Err(e) => {
                warn!("❌ Failed to register inline-assist hotkey {}: {}", ia, e);
                failures.push(("inline-assist", ia.clone()));
            }
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
                                // CLIPBOARD_HISTORY_MODE / VOICE_MODE both
                                // target the floating launcher; the only
                                // listener is in `floating/clipboard-history.js`.
                                crate::event_targets::emit_to_floating(&handle, &evt, &());
                            });
                        },
                    ) {
                        Ok(_) => info!("✅ Registered {} hotkey: {}", name, hk),
                        Err(e) => {
                            warn!("❌ Failed to register {} hotkey {}: {}", name, hk, e);
                            failures.push((name, hk.clone()));
                        }
                    }
                }
            }
            None => info!("ℹ️ No {} hotkey configured", name),
        }
    }

    // Stash the failures in state so Settings → Hotkeys can read them on open
    // even if the event below fired before that window existed (startup), then
    // surface them once. The usual cause is another app owning the combo.
    {
        let ui: tauri::State<'_, crate::state::UiState> = app.state();
        *ui.hotkey_registration_failures.lock_or_recover() = failures
            .iter()
            .map(|(slot, hk)| (slot.to_string(), hk.clone()))
            .collect();
    }
    if !failures.is_empty() {
        let payload: Vec<serde_json::Value> = failures
            .iter()
            .map(|(slot, hk)| serde_json::json!({ "slot": slot, "hotkey": hk }))
            .collect();
        if let Err(e) = app.emit(events::HOTKEY_REGISTRATION_FAILED, payload) {
            warn!("Failed to emit hotkey-registration-failed event: {}", e);
        }
    }
}

/// Return the most recent global-hotkey registration failures as
/// `[{ slot, hotkey }]`. Empty when the last registration pass was clean.
/// Settings → Hotkeys calls this on open so a startup failure (which fired
/// its event before any window could listen) is still surfaced.
#[tauri::command]
pub async fn get_hotkey_registration_failures(
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<Vec<serde_json::Value>, AppError> {
    let failures = ui.hotkey_registration_failures.lock_or_recover();
    Ok(failures
        .iter()
        .map(|(slot, hk)| serde_json::json!({ "slot": slot, "hotkey": hk }))
        .collect())
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
        let voice_hk = config.get_voice_hotkey_string();
        let slot_name = slot.as_deref().unwrap_or("main");

        let new_norm = normalize_hotkey(&hotkey_str);

        // Check all other slots for conflicts. Voice is a real slot too (it's
        // registered in register_all_hotkeys), so it must participate — else a
        // user could bind main/clipboard/inline-assist to a combo already used
        // by voice and one would silently shadow the other at registration.
        let all_hotkeys: Vec<(&str, String)> = [
            ("main", Some(main_hk)),
            ("clipboard", cb_hk),
            ("inline-assist", ia_hk),
            ("voice", voice_hk),
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

    // Capture runs on the blocking pool. Whatever happens — success, timeout,
    // or a panicked capture thread — we MUST re-register the hotkeys we just
    // unregistered, or every global hotkey stays dead until the next config
    // change. So don't `?`-return the join error before re-registering.
    let join = tauri::async_runtime::spawn_blocking(|| crate::os::capture_hotkey(10000)).await;

    // Re-register all global hotkeys from config (unconditionally).
    register_all_hotkeys(&app);

    let result = join.map_err(|e| format!("Task error: {}", e))?;

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
