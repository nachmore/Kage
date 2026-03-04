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
pub async fn save_frecency(data: String) -> Result<(), String> {
    let path = dirs::config_dir()
        .ok_or("No config dir")?
        .join("kiro-assistant")
        .join("search-frecency.json");
    std::fs::write(&path, &data).map_err(|e| format!("Failed to save frecency: {}", e))
}

#[tauri::command]
pub async fn load_frecency() -> Result<String, String> {
    let path = dirs::config_dir()
        .ok_or("No config dir")?
        .join("kiro-assistant")
        .join("search-frecency.json");
    match std::fs::read_to_string(&path) {
        Ok(data) => Ok(data),
        Err(_) => Ok("{}".to_string()),
    }
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
        .inner_size(520.0, 540.0)
        .resizable(false)
        .decorations(false)
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
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    launch_at_startup: bool,
    auto_update: bool,
) -> Result<(), String> {
    let mut config = state.config.lock().await;
    let is_true_first_run = !config.first_run_completed;
    config.first_run_completed = true;
    if auto_update {
        config.updates.auto_check = true;
        config.updates.silent_update = true;
    }
    let _ = config.save();
    drop(config);

    set_startup_enabled_impl(launch_at_startup);

    // On true first run (or dev mode), show the floating window with a welcome banner
    if is_true_first_run || state.dev_mode {
        show_welcome_banner(&app);
    }

    Ok(())
}

/// Show the floating window with a welcome banner displaying the configured hotkey.
/// Called from first-run completion and the dev tray menu.
pub fn show_welcome_banner(app: &tauri::AppHandle) {
    let hotkey_str = app.try_state::<crate::state::AppState>()
        .and_then(|state| {
            state.config.try_lock().ok().map(|c| c.get_hotkey_string())
        })
        .unwrap_or_else(|| "Alt+Space".to_string());
    let keycaps: String = hotkey_str.split('+')
        .map(|k| format!("<span class=\"keycap\">{}</span>", k))
        .collect::<Vec<_>>()
        .join("<span class=\"keycap-sep\">+</span>");
    let text = format!("<b>Welcome to the Assistant!</b><br/>&nbsp;<br>Press {} anytime to summon me.", keycaps);

    if let Some(floating) = app.get_webview_window("floating") {
        let _ = floating.show();
        let _ = floating.set_focus();
    }
    let _ = app.emit("show_floating_banner", serde_json::json!({
        "icon": "👋",
        "text": text,
        "action_label": "",
        "action_type": "dismiss",
        "action_data": ""
    }));
}

#[tauri::command]
pub async fn is_first_run(state: State<'_, AppState>) -> Result<bool, String> {
    let config = state.config.lock().await;
    Ok(!config.first_run_completed)
}

#[tauri::command]
pub async fn get_startup_enabled() -> Result<bool, String> {
    Ok(get_startup_enabled_impl())
}

#[tauri::command]
pub async fn set_startup_enabled(enabled: bool) -> Result<(), String> {
    set_startup_enabled_impl(enabled);
    Ok(())
}

fn get_startup_enabled_impl() -> bool {
    crate::os::get_startup_enabled()
}

fn set_startup_enabled_impl(enabled: bool) {
    crate::os::set_startup_enabled(enabled);
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
    // Temporarily unregister global hotkeys so they don't intercept during capture
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let _ = app.global_shortcut().unregister_all();

    let result = tauri::async_runtime::spawn_blocking(|| {
        crate::os::capture_hotkey(10000)
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

#[tauri::command]
pub async fn cancel_hotkey_capture() -> Result<(), String> {
    crate::os::cancel_hotkey_capture();
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

    // Kill pocket-tts server if running
    {
        let mut tts_proc = state.pocket_tts_process.lock().unwrap();
        if let Some(mut child) = tts_proc.take() {
            info!("Stopping pocket-tts server on quit");
            let _ = child.kill();
            let _ = child.wait();
        }
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
    Ok(crate::os::read_clipboard().unwrap_or_default())
}

#[derive(serde::Serialize, Clone)]
pub struct UserInfo {
    pub display_name: String,
    pub initials: String,
    pub avatar_path: Option<String>,
    pub avatar_base64: Option<String>,
    pub home: Option<String>,
}

#[tauri::command]
pub async fn get_user_info(state: State<'_, AppState>) -> Result<UserInfo, String> {
    // Return cached user info if available
    {
        let cached = state.user_info_cache.lock().unwrap();
        if let Some(ref info) = *cached {
            return Ok(info.clone());
        }
    }

    // Compute and cache
    let info = compute_user_info();
    {
        let mut cached = state.user_info_cache.lock().unwrap();
        *cached = Some(info.clone());
    }
    Ok(info)
}

/// Compute user info (expensive — spawns whoami subprocess on Windows).
/// Called once and cached in AppState.
pub fn compute_user_info() -> UserInfo {
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

    UserInfo {
        display_name: profile.display_name.clone(),
        initials,
        avatar_path: profile.avatar_path.clone(),
        avatar_base64,
        home: dirs::home_dir().and_then(|p| p.to_str().map(|s| s.to_string())),
    }
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

    Ok(Some(format!("{} {}\n\n---\n\n<instructions>Respond with only \"ack\" to confirm receipt. Do not summarize or comment on the content above.</instructions>", STEERING_MSG_PREFIX, parts.join("\n\n---\n\n"))))
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
    crate::os::open_in_editor(&path_str)
        .map_err(|e| format!("Failed to open file: {}", e))?;

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

// --- Update commands ---

#[tauri::command]
pub async fn check_for_update(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let result = tauri::async_runtime::spawn_blocking(crate::updater::check_for_update)
        .await
        .map_err(|e| format!("Task error: {}", e))?
        .map_err(|e| format!("Check failed: {}", e))?;

    // Emit event so the floating window can show a banner too
    if let Some(ref version) = result {
        let _ = app.emit("update_available", version);
    }

    Ok(serde_json::json!({
        "current_version": crate::updater::CURRENT_VERSION,
        "available_version": result,
    }))
}

#[tauri::command]
pub async fn fetch_changelog() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(crate::updater::fetch_changelog)
        .await
        .map_err(|e| format!("Task error: {}", e))?
        .map_err(|e| format!("Fetch failed: {}", e))
}

#[tauri::command]
pub async fn get_update_urls() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "version_url": crate::updater::VERSION_URL,
        "installer_url": crate::updater::INSTALLER_URL,
        "changelog_url": crate::updater::CHANGELOG_URL,
    }))
}

#[tauri::command]
pub async fn download_and_install_update(
    state: State<'_, AppState>,
) -> Result<(), String> {
    let session_id = state.floating_session_id.lock()
        .ok()
        .and_then(|s| s.clone());

    tauri::async_runtime::spawn_blocking(move || {
        let path = crate::updater::download_installer()
            .map_err(|e| e.to_string())?;
        crate::updater::run_installer_and_exit(&path, session_id.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn was_just_updated(state: State<'_, AppState>) -> Result<bool, String> {
    let config = state.config.lock().await;
    Ok(crate::updater::was_just_updated(&config))
}

#[tauri::command]
pub async fn clear_update_flag(state: State<'_, AppState>) -> Result<(), String> {
    let mut config = state.config.lock().await;
    crate::updater::clear_update_flag(&mut config);
    config.save().map_err(|e| format!("Failed to save: {}", e))
}

#[tauri::command]
pub async fn touch_floating_activity(state: State<'_, AppState>) -> Result<(), String> {
    state.updater.touch_activity();
    Ok(())
}

/// Simulate a completed update by showing the update banner on the floating window.
pub fn simulate_update_complete(app: &tauri::AppHandle) {
    show_update_banner(app);
}

/// Show the floating window with an update celebration banner.
pub fn show_update_banner(app: &tauri::AppHandle) {
    if let Some(floating) = app.get_webview_window("floating") {
        let _ = floating.show();
        let _ = floating.set_focus();
    }
    let _ = app.emit("show_floating_banner", serde_json::json!({
        "icon": "🎉",
        "text": "Kiro Assistant has been updated!",
        "action_label": "View changelog →",
        "action_type": "settings",
        "action_data": "updates"
    }));
}
