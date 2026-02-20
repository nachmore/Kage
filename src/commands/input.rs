use crate::os;
use crate::state::AppState;
use log::{error, info};
use tauri::{Manager, State};

/// Check if input is a URL
fn is_url(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("ftp://")
        || trimmed.starts_with("file://")
        || (trimmed.starts_with("www.") && trimmed.contains('.'))
}

/// Check if input is a file or folder path
fn is_path(input: &str) -> Option<String> {
    let trimmed = input.trim();

    // Windows paths
    if cfg!(target_os = "windows") {
        if trimmed.len() >= 3
            && trimmed.chars().nth(1) == Some(':')
            && trimmed.chars().nth(2) == Some('\\')
        {
            return Some(trimmed.to_string());
        }
        if trimmed.starts_with("\\\\") {
            return Some(trimmed.to_string());
        }
        if trimmed.contains('\\') {
            return Some(trimmed.to_string());
        }
    }

    // Unix-like paths
    if cfg!(target_os = "linux") || cfg!(target_os = "macos") {
        if trimmed.starts_with('/') {
            return Some(trimmed.to_string());
        }
        if trimmed.starts_with('~') {
            return Some(trimmed.to_string());
        }
        if trimmed.contains('/') && !trimmed.contains("://") {
            return Some(trimmed.to_string());
        }
    }

    None
}

#[tauri::command]
pub async fn handle_floating_input(
    input: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    info!("Handling floating input: {}", input);

    let trimmed_input = input.trim();

    if is_url(trimmed_input) {
        info!("Detected URL pattern: {}", trimmed_input);
        return Ok(format!("url:{}", trimmed_input));
    }

    if let Some(path) = is_path(trimmed_input) {
        info!("Detected path pattern: {}", path);
        let is_file = path.contains('.') && !path.ends_with('\\') && !path.ends_with('/');
        return Ok(format!(
            "path:{}:{}",
            if is_file { "file" } else { "folder" },
            path
        ));
    }

    let launcher = state.app_launcher.lock().await;
    let matches = launcher.find_app(trimmed_input);

    if !matches.is_empty() {
        info!("Found {} matching application(s)", matches.len());
        let json = serde_json::to_string(&matches).map_err(|e| e.to_string())?;
        if matches.len() == 1 {
            return Ok(format!("launched:{}", json));
        } else {
            return Ok(format!("multiple:{}", json));
        }
    }

    info!("No pattern match, opening chat mode");
    Ok("chat".to_string())
}

#[tauri::command]
pub async fn launch_app_by_name(
    app_name: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    info!("Launching app by name: {}", app_name);

    let launcher = state.app_launcher.lock().await;
    let matches = launcher.find_app(&app_name);

    if let Some(app_to_launch) = matches.first() {
        launcher.launch(app_to_launch).map_err(|e| {
            error!("Failed to launch {}: {}", app_name, e);
            format!("Failed to launch {}: {}", app_name, e)
        })?;

        if let Some(floating_window) = app.get_webview_window("floating") {
            let _ = floating_window.hide();
        }

        Ok(())
    } else {
        Err(format!("Application not found: {}", app_name))
    }
}

#[tauri::command]
pub async fn open_url(url: String, app: tauri::AppHandle) -> Result<(), String> {
    info!("Opening URL: {}", url);

    let full_url = if url.starts_with("www.") {
        format!("https://{}", url)
    } else {
        url.clone()
    };

    os::open_url(&full_url).map_err(|e| format!("Failed to open URL: {}", e))?;

    if let Some(floating_window) = app.get_webview_window("floating") {
        let _ = floating_window.hide();
    }

    Ok(())
}

#[tauri::command]
pub async fn open_path(path: String, app: tauri::AppHandle) -> Result<(), String> {
    info!("Opening path: {}", path);

    let expanded_path = if path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            path.replacen('~', &home.to_string_lossy(), 1)
        } else {
            path.clone()
        }
    } else {
        path.clone()
    };

    let path_obj = std::path::Path::new(&expanded_path);
    if !path_obj.exists() {
        return Err(format!("Path does not exist: {}", expanded_path));
    }

    os::open_path(&expanded_path).map_err(|e| format!("Failed to open path: {}", e))?;

    if let Some(floating_window) = app.get_webview_window("floating") {
        let _ = floating_window.hide();
    }

    Ok(())
}

#[tauri::command]
pub async fn execute_shortcut(
    path: String,
    args: Vec<String>,
    working_directory: Option<String>,
) -> Result<(), String> {
    info!("Executing shortcut: {} with args: {:?}", path, args);

    use std::process::Command;

    let expanded_path = if path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            path.replacen('~', &home.to_string_lossy(), 1)
        } else {
            path.clone()
        }
    } else {
        path.clone()
    };

    let expanded_work_dir = working_directory.as_ref().and_then(|wd| {
        if wd.starts_with('~') {
            dirs::home_dir().map(|home| wd.replacen('~', &home.to_string_lossy(), 1))
        } else {
            Some(wd.clone())
        }
    });

    let mut command = Command::new(&expanded_path);
    command.args(&args);

    if let Some(work_dir) = expanded_work_dir {
        command.current_dir(work_dir);
    }

    command
        .spawn()
        .map_err(|e| format!("Failed to execute shortcut: {}", e))?;

    info!("Shortcut executed successfully");
    Ok(())
}
