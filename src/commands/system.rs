use crate::config::Config;
use crate::state::AppState;
use log::{error, info};
use tauri::{Emitter, Manager, State};

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
pub async fn open_devtools(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("floating") {
        let window: tauri::WebviewWindow = window;
        window.open_devtools();
    }
    Ok(())
}

#[tauri::command]
pub async fn quit_app(state: State<'_, AppState>) -> Result<(), String> {
    info!("Quit requested via > command");
    if let Ok(client) = state.acp_client.try_lock() {
        client.disconnect();
    }
    std::process::exit(0);
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
