use crate::os;
use crate::state::AppState;
use log::{error, info};
use tauri::{Manager, State};

/// Check if input is a URL (must be the entire input, not embedded in a sentence)
fn is_url(input: &str) -> bool {
    let trimmed = input.trim();
    // If there are spaces before the protocol, it's a sentence, not a bare URL
    trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("ftp://")
        || trimmed.starts_with("file://")
        || (trimmed.starts_with("www.") && trimmed.contains('.'))
}

/// Check if input is a file or folder path.
/// Only matches if the input *starts* with a path-like pattern.
/// Natural language queries that happen to contain paths mid-sentence are not matched.
fn is_path(input: &str) -> Option<String> {
    let trimmed = input.trim();

    // Reject if it looks like a natural language query (starts with common words)
    // A path input should start directly with the path characters.
    
    // Windows paths — must start with drive letter or UNC prefix
    if cfg!(target_os = "windows") {
        // Drive letter: C:\ or C:/ or just C: (bare drive root)
        if trimmed.len() >= 2
            && trimmed.as_bytes()[0].is_ascii_alphabetic()
            && trimmed.chars().nth(1) == Some(':')
        {
            // Accept "C:", "C:\", "C:\Users\...", "C:/Users/..."
            if trimmed.len() == 2
                || trimmed.chars().nth(2) == Some('\\')
                || trimmed.chars().nth(2) == Some('/')
            {
                return Some(trimmed.to_string());
            }
        }
        // UNC path: \\server\share
        if trimmed.starts_with("\\\\") {
            return Some(trimmed.to_string());
        }
        // Don't match trimmed.contains('\\') — that catches paths mid-sentence
    }

    // Unix-like paths — must start with / or ~
    if cfg!(target_os = "linux") || cfg!(target_os = "macos") {
        if trimmed.starts_with('/') {
            return Some(trimmed.to_string());
        }
        if trimmed.starts_with('~') {
            return Some(trimmed.to_string());
        }
        // Don't match trimmed.contains('/') — that catches paths mid-sentence
    }

    None
}

/// Resolve well-known directory names to their actual paths.
/// Supports prefix matching — "down" matches "downloads".
/// Only matches if the input is a single word (no spaces).
fn resolve_well_known_dir(input: &str) -> Option<String> {
    let lower = input.to_lowercase();
    if lower.contains(' ') || lower.is_empty() {
        return None;
    }

    let candidates: &[(&[&str], fn() -> Option<std::path::PathBuf>)] = &[
        (&["downloads", "download"], dirs::download_dir),
        (&["documents", "docs"], dirs::document_dir),
        (&["pictures", "photos"], dirs::picture_dir),
        (&["videos", "video", "movies"], dirs::video_dir),
        (&["music", "audio"], dirs::audio_dir),
        (&["desktop"], dirs::desktop_dir),
        (&["home"], dirs::home_dir),
    ];

    // Exact match first
    for (names, resolver) in candidates {
        if names.iter().any(|n| *n == lower.as_str()) {
            return resolver().map(|p| p.to_string_lossy().to_string());
        }
    }

    // Prefix match — return the first match
    for (names, resolver) in candidates {
        if names.iter().any(|n| n.starts_with(lower.as_str())) {
            return resolver().map(|p| p.to_string_lossy().to_string());
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

    // Check for well-known directory names (single word only)
    if let Some(path) = resolve_well_known_dir(trimmed_input) {
        info!("Detected well-known directory: {} → {}", trimmed_input, path);
        return Ok(format!("path:folder:{}", path));
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
