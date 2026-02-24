use crate::config::Config;
use crate::os;
use crate::state::AppState;
use log::{error, info};
use std::fs;
use tauri::{Emitter, Manager, State};

/// Prefix used to mark steering messages that should be hidden in the UI.
/// Only the very first message in a conversation with this prefix is hidden.
pub const STEERING_MSG_PREFIX: &str = "[KIRO_STEERING_IGNORE]";

/// Built-in steering document embedded at compile time.
pub const BUILTIN_STEERING: &str = include_str!("../builtin_steering.md");

#[tauri::command]
pub async fn get_config(state: State<'_, AppState>) -> Result<Config, String> {
    let config = state.config.lock().await;
    Ok(config.clone())
}

#[tauri::command]
pub async fn save_config(
    config: Config,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    info!("Saving configuration");
    config.save().map_err(|e| {
        error!("Failed to save config: {}", e);
        format!("Failed to save configuration: {}", e)
    })?;

    let mut state_config = state.config.lock().await;
    *state_config = config.clone();

    info!("Configuration saved successfully");

    if let Err(e) = app.emit("config_updated", ()) {
        error!("Failed to emit config_updated event: {}", e);
    }

    Ok(())
}

#[tauri::command]
pub async fn update_tool_policy(
    tool_title: String,
    policy: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    info!("Updating tool policy: {} -> {}", tool_title, policy);
    let mut config = state.config.lock().await;
    if let Some(tool) = config
        .tool_permissions
        .tools
        .iter_mut()
        .find(|t| t.title == tool_title)
    {
        tool.policy = policy;
    }
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn remove_tool_permission(
    tool_title: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().await;
    config
        .tool_permissions
        .tools
        .retain(|t| t.title != tool_title);
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn is_dev_mode(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.dev_mode)
}

#[tauri::command]
pub async fn open_welcome_window(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::WebviewWindowBuilder;
    // If window exists and is valid, just focus it
    if let Some(w) = app.get_webview_window("welcome") {
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }
    // Create fresh window (previous one was closed/destroyed)
    let w = WebviewWindowBuilder::new(&app, "welcome", tauri::WebviewUrl::App("welcome.html".into()))
        .title("Welcome to Kiro Assistant")
        .inner_size(520.0, 480.0)
        .resizable(false)
        .center()
        .visible(false) // Hidden until content loads
        .build()
        .map_err(|e| format!("Failed to open welcome window: {}", e))?;
    // Set dark background to prevent white flash
    let _ = w.set_background_color(Some(tauri::window::Color(30, 26, 36, 255)));
    // When closed, destroy so it can be recreated
    let w2 = w.clone();
    w.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { .. } = event {
            let _ = w2.destroy();
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn complete_first_run(
    state: State<'_, AppState>,
    launch_at_startup: bool,
) -> Result<(), String> {
    let mut config = state.config.lock().await;
    config.first_run_completed = true;
    let _ = config.save();

    // Set or remove Windows startup registry entry
    #[cfg(target_os = "windows")]
    {
        let exe = std::env::current_exe().unwrap_or_default();
        let key_path = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
        let app_name = "Kiro Assistant";
        if launch_at_startup {
            if let Ok(hkcu) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
                .open_subkey_with_flags(key_path, winreg::enums::KEY_WRITE)
            {
                let _ = hkcu.set_value(app_name, &exe.to_string_lossy().to_string());
            }
        } else {
            if let Ok(hkcu) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
                .open_subkey_with_flags(key_path, winreg::enums::KEY_WRITE)
            {
                let _ = hkcu.delete_value(app_name);
            }
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn is_first_run(state: State<'_, AppState>) -> Result<bool, String> {
    let config = state.config.lock().await;
    Ok(!config.first_run_completed)
}

#[tauri::command]
pub async fn get_app_info() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "authors": env!("CARGO_PKG_AUTHORS"),
        "description": env!("CARGO_PKG_DESCRIPTION"),
        "license": env!("CARGO_PKG_LICENSE"),
        "repository": env!("CARGO_PKG_REPOSITORY"),
        "homepage": env!("CARGO_PKG_HOMEPAGE"),
        "name": env!("CARGO_PKG_NAME"),
    }))
}

#[tauri::command]
pub async fn try_register_hotkey(
    app: tauri::AppHandle,
    modifiers: Vec<String>,
    key: String,
) -> Result<bool, String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let hotkey_str = if modifiers.is_empty() {
        key.clone()
    } else {
        format!("{}+{}", modifiers.join("+"), key)
    };
    info!("Trying to register hotkey: {}", hotkey_str);

    // Unregister all existing shortcuts first
    let _ = app.global_shortcut().unregister_all();

    // Try to register the new one
    let floating = app.get_webview_window("floating");
    match app.global_shortcut().on_shortcut(
        hotkey_str.as_str(),
        move |_app, _shortcut, event| {
            if event.state != ShortcutState::Pressed { return; }
            if let Some(ref w) = floating {
                crate::commands::window::toggle_floating_window(w);
            }
        },
    ) {
        Ok(_) => {
            info!("✅ Hotkey registered: {}", hotkey_str);
            Ok(true)
        }
        Err(e) => {
            let msg = format!("{}", e);
            info!("❌ Hotkey registration failed: {}", msg);
            // Try to re-register the old hotkey from config
            let state: tauri::State<'_, AppState> = app.state();
            let config = state.config.lock().await;
            let old_hotkey = config.get_hotkey_string();
            drop(config);
            if let Some(floating) = app.get_webview_window("floating") {
                let _ = app.global_shortcut().on_shortcut(
                    old_hotkey.as_str(),
                    move |_app, _shortcut, event| {
                        if event.state != ShortcutState::Pressed { return; }
                        crate::commands::window::toggle_floating_window(&floating);
                    },
                );
            }
            Err(msg)
        }
    }
}

#[tauri::command]
pub async fn capture_hotkey_combo(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    #[cfg(target_os = "windows")]
    {
        // Temporarily unregister global hotkeys so they don't intercept during capture
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        let _ = app.global_shortcut().unregister_all();

        let result = tauri::async_runtime::spawn_blocking(|| {
            // Use helper process to work around WebView2 blocking WH_KEYBOARD_LL
            crate::os::windows::hotkey_capture::capture_hotkey_via_helper(10000)
        }).await.map_err(|e| format!("Task error: {}", e))?;

        // Re-register the global hotkey from config
        let state: tauri::State<'_, AppState> = app.state();
        let config = state.config.lock().await;
        let hotkey_string = config.get_hotkey_string();
        drop(config);
        if let Some(floating) = app.get_webview_window("floating") {
            use tauri_plugin_global_shortcut::ShortcutState;
            let _ = app.global_shortcut().on_shortcut(
                hotkey_string.as_str(),
                move |_app, _shortcut, event| {
                    if event.state != ShortcutState::Pressed { return; }
                    crate::commands::window::toggle_floating_window(&floating);
                },
            );
        }

        match result {
            Some(captured) => Ok(serde_json::json!({
                "modifiers": captured.modifiers,
                "key": captured.key,
                "display": captured.display,
            })),
            None => Ok(serde_json::json!(null)),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        Err("Hotkey capture not supported on this platform".to_string())
    }
}

#[tauri::command]
pub async fn cancel_hotkey_capture() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::hotkey_capture::cancel_capture();
    }
    Ok(())
}

#[tauri::command]
pub async fn open_devtools(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(debug_assertions)]
    if let Some(window) = app.get_webview_window("floating") {
        let window: tauri::WebviewWindow = window;
        window.open_devtools();
    }
    #[cfg(not(debug_assertions))]
    { let _ = app; }
    Ok(())
}

#[tauri::command]
pub async fn restart_app(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<(), String> {
    info!("Restart requested via > command");

    // Collect current exe and args before we start tearing down
    let exe = std::env::current_exe().map_err(|e| format!("Failed to get exe path: {}", e))?;
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Hide all windows for instant feedback
    for label in &["floating", "main", "settings", "context-menu"] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.hide();
        }
    }
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_visible(false);
    }

    let acp_client = state.acp_client.clone();
    let config = state.config.clone();

    tauri::async_runtime::spawn(async move {
        // Generate auto-steering and disconnect
        if let Ok(client) = acp_client.try_lock() {
            if let Ok(config) = config.try_lock() {
                crate::auto_steering::generate_steering_on_quit(&client, &config);
            }
            client.disconnect();
        }

        // Spawn new instance with same args
        info!("Restarting: {:?} {:?}", exe, args);
        let _ = std::process::Command::new(&exe)
            .args(&args)
            .current_dir(std::env::current_dir().unwrap_or_default())
            .spawn();

        std::process::exit(0);
    });

    Ok(())
}

#[tauri::command]
pub async fn quit_app(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<(), String> {
    info!("Quit requested via > command");

    // Immediately hide all windows and tray so the user sees instant feedback
    for label in &["floating", "main", "settings", "context-menu"] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.hide();
        }
    }
    // Hide the tray icon
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_visible(false);
    }

    // Generate auto-steering document in background, then exit
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

    Ok(())
}

#[tauri::command]
pub async fn read_clipboard() -> Result<String, String> {
    use std::process::Command;
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("powershell")
            .args(["-NoProfile", "-Command", "Get-Clipboard"])
            .output()
            .map_err(|e| format!("Failed to read clipboard: {}", e))?;
        let text = String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string();
        return Ok(text);
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("pbpaste")
            .output()
            .map_err(|e| format!("Failed to read clipboard: {}", e))?;
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        return Ok(text);
    }
    #[cfg(target_os = "linux")]
    {
        let output = Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()
            .map_err(|e| format!("Failed to read clipboard: {}", e))?;
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        return Ok(text);
    }
}

#[derive(serde::Serialize)]
pub struct UserInfo {
    pub display_name: String,
    pub initials: String,
    pub avatar_path: Option<String>,
    pub avatar_base64: Option<String>,
}

#[tauri::command]
pub async fn get_user_info() -> Result<UserInfo, String> {
    let profile = os::get_user_profile();

    // Build initials from display name, falling back to username
    let name_for_initials = if profile.display_name == profile.username {
        &profile.username
    } else {
        &profile.display_name
    };

    let initials = name_for_initials
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();

    let initials = if initials.is_empty() {
        profile
            .username
            .chars()
            .next()
            .unwrap_or('U')
            .to_uppercase()
            .to_string()
    } else {
        initials
    };

    // Read avatar file as base64 for direct use in img src
    let avatar_base64 = profile.avatar_path.as_ref().and_then(|path| {
        use base64::Engine;
        let bytes = std::fs::read(path).ok()?;
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png");
        let mime = match ext {
            "jpg" | "jpeg" => "image/jpeg",
            "bmp" => "image/bmp",
            _ => "image/png",
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Some(format!("data:{};base64,{}", mime, b64))
    });

    Ok(UserInfo {
        display_name: profile.display_name,
        initials,
        avatar_path: profile.avatar_path,
        avatar_base64,
    })
}


/// Build the combined steering content from user and auto-generated docs.
/// User steering takes precedence (placed first).
/// Returns None if no steering content is available.
#[tauri::command]
pub async fn get_steering_content(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let config = state.config.lock().await;
    let assistant = &config.acp.assistant;

    let mut parts: Vec<String> = Vec::new();

    // Built-in steering document (always included first)
    parts.push(BUILTIN_STEERING.to_string());

    // User-written steering doc takes precedence (loaded first)
    if let Some(ref path) = assistant.user_steering_path {
        if !path.is_empty() {
            match fs::read_to_string(path) {
                Ok(content) if !content.trim().is_empty() => {
                    info!("Loaded user steering doc from: {}", path);
                    parts.push(content);
                }
                Ok(_) => info!("User steering doc is empty: {}", path),
                Err(e) => error!("Failed to read user steering doc {}: {}", path, e),
            }
        }
    }

    // Auto-generated steering doc
    if assistant.auto_steering_enabled {
        match Config::get_auto_steering_path() {
            Ok(auto_path) => {
                if auto_path.exists() {
                    match fs::read_to_string(&auto_path) {
                        Ok(content) if !content.trim().is_empty() => {
                            info!("Loaded auto steering doc from: {:?}", auto_path);
                            parts.push(content);
                        }
                        Ok(_) => info!("Auto steering doc is empty"),
                        Err(e) => error!("Failed to read auto steering doc: {}", e),
                    }
                }
            }
            Err(e) => error!("Failed to get auto steering path: {}", e),
        }
    }

    Ok(Some(format!("{} {}\n\n---\n\nPlease respond with only \"ack\" to confirm you've received this context.", STEERING_MSG_PREFIX, parts.join("\n\n---\n\n"))))
}

/// Open the auto-generated steering document in the default editor.
/// Creates the file with a header comment if it doesn't exist yet.
#[tauri::command]
pub async fn open_auto_steering_file() -> Result<String, String> {
    let auto_path = Config::get_auto_steering_path()
        .map_err(|e| format!("Failed to get auto steering path: {}", e))?;

    // Create with header if it doesn't exist
    if !auto_path.exists() {
        if let Some(parent) = auto_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        }
        let header = "<!-- AUTO-GENERATED STEERING DOCUMENT\n     Any manual changes will be overridden the next time this document is generated.\n     To add your own persistent instructions, use a User Steering Document instead. -->\n\n";
        fs::write(&auto_path, header)
            .map_err(|e| format!("Failed to create auto steering file: {}", e))?;
    }

    let path_str = auto_path
        .to_str()
        .ok_or_else(|| "Invalid path encoding".to_string())?
        .to_string();

    // Open in default editor
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &path_str])
            .spawn()
            .map_err(|e| format!("Failed to open file: {}", e))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path_str)
            .spawn()
            .map_err(|e| format!("Failed to open file: {}", e))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&path_str)
            .spawn()
            .map_err(|e| format!("Failed to open file: {}", e))?;
    }

    Ok(path_str)
}

/// Get the path to the auto-generated steering document
#[tauri::command]
pub async fn get_auto_steering_path() -> Result<String, String> {
    Config::get_auto_steering_path()
        .map_err(|e| format!("Failed to get path: {}", e))
        .and_then(|p| {
            p.to_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "Invalid path encoding".to_string())
        })
}
