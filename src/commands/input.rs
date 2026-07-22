use crate::error::AppError;
use crate::os;
use crate::state::FeatureServices;
use crate::window_labels;
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

    #[allow(clippy::type_complexity)]
    let candidates: &[(&[&str], fn() -> Option<std::path::PathBuf>)] = &[
        (&["downloads", "download"], dirs::download_dir),
        (&["documents", "docs"], dirs::document_dir),
        (&["pictures", "photos"], dirs::picture_dir),
        (&["videos", "video", "movies"], dirs::video_dir),
        (&["music", "audio"], dirs::audio_dir),
        (&["desktop"], dirs::desktop_dir),
        (&["home", "user"], dirs::home_dir),
        (&["templates", "template"], dirs::template_dir),
        (&["temp", "tmp"], || -> Option<std::path::PathBuf> {
            Some(std::env::temp_dir())
        }),
        (&["public"], dirs::public_dir),
        (
            &["screenshots", "screenshot"],
            || -> Option<std::path::PathBuf> { dirs::picture_dir().map(|p| p.join("Screenshots")) },
        ),
        (&["fonts", "font"], dirs::font_dir),
        (&["cache"], dirs::cache_dir),
        (&["config", "configuration"], dirs::config_dir),
        (&["data"], dirs::data_dir),
    ];

    // Override "fonts"/"font" to use the system fonts directory
    {
        let font_names = ["fonts", "font"];
        if font_names.contains(&lower.as_str())
            || font_names.iter().any(|n| n.starts_with(lower.as_str()))
        {
            if let Some(font_dir) = crate::os::fonts_dir() {
                return Some(font_dir.to_string_lossy().to_string());
            }
        }
    }

    // Exact match first
    for (names, resolver) in candidates {
        if names.contains(&lower.as_str()) {
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

/// Match a system command by name (exact or prefix).
/// Returns (command_id, display_label, needs_confirmation).
fn match_system_command(input: &str) -> Option<(&'static str, &'static str, bool)> {
    let lower = input.to_lowercase();
    if lower.contains(' ') || lower.is_empty() {
        return None;
    }

    // (aliases, command_id, display_label, needs_confirm, platform filter)
    let commands: &[(&[&str], &str, &str, bool, &str)] = &[
        // Immediate
        (&["lock"], "lock", "🔒 Lock Screen", false, "all"),
        (&["sleep"], "sleep", "😴 Sleep", true, "all"),
        (&["screenshot"], "screenshot", "📸 Screenshot", false, "all"),
        (&["mute"], "mute", "🔇 Mute Audio", false, "all"),
        (&["unmute"], "unmute", "🔊 Unmute Audio", false, "all"),
        (&["emoji"], "emoji", "😀 Emoji Picker", false, "all"),
        (
            &["trash", "recycle"],
            "trash",
            "🗑️ Open Recycle Bin",
            false,
            "windows",
        ),
        (&["trash"], "trash", "🗑️ Open Trash", false, "macos"),
        (&["trash"], "trash", "🗑️ Open Trash", false, "linux"),
        (
            &["taskmanager", "taskmgr"],
            "taskmanager",
            "📊 Task Manager",
            false,
            "windows",
        ),
        (
            &["activitymonitor", "taskmanager", "taskmgr"],
            "taskmanager",
            "📊 Activity Monitor",
            false,
            "macos",
        ),
        (
            &["taskmanager", "taskmgr", "systemmonitor"],
            "taskmanager",
            "📊 System Monitor",
            false,
            "linux",
        ),
        (
            &["terminal", "cmd", "powershell"],
            "terminal",
            "💻 Terminal",
            false,
            "windows",
        ),
        (&["terminal"], "terminal", "💻 Terminal", false, "macos"),
        (&["terminal"], "terminal", "💻 Terminal", false, "linux"),
        (
            &["explorer"],
            "filemanager",
            "📁 File Explorer",
            false,
            "windows",
        ),
        (&["finder"], "filemanager", "📁 Finder", false, "macos"),
        (
            &["files", "nautilus"],
            "filemanager",
            "📁 Files",
            false,
            "linux",
        ),
        // With confirmation
        (
            &["restart", "reboot"],
            "restart",
            "🔄 Restart Computer",
            true,
            "all",
        ),
        (&["shutdown"], "shutdown", "⏻ Shut Down", true, "all"),
        (
            &["signout", "logout", "logoff"],
            "signout",
            "🚪 Sign Out",
            true,
            "all",
        ),
    ];

    let platform = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };

    // Exact match first
    for &(aliases, id, label, confirm, plat) in commands {
        if plat != "all" && plat != platform {
            continue;
        }
        if aliases.contains(&lower.as_str()) {
            return Some((id, label, confirm));
        }
    }
    // Prefix match
    for &(aliases, id, label, confirm, plat) in commands {
        if plat != "all" && plat != platform {
            continue;
        }
        if aliases.iter().any(|a| a.starts_with(lower.as_str())) {
            return Some((id, label, confirm));
        }
    }
    None
}

#[tauri::command]
pub async fn handle_floating_input(
    input: String,
    features: State<'_, FeatureServices>,
) -> Result<String, AppError> {
    let trimmed_input = input.trim();

    // Collect ALL matches from all sources (no early returns)
    let mut results: Vec<serde_json::Value> = Vec::new();

    // URL
    if is_url(trimmed_input) {
        results.push(serde_json::json!({ "type": "url", "value": trimmed_input, "score": 95 }));
    }

    // Path
    if let Some(path) = is_path(trimmed_input) {
        let is_file = path.contains('.') && !path.ends_with('\\') && !path.ends_with('/');
        results.push(serde_json::json!({
            "type": "path",
            "pathType": if is_file { "file" } else { "folder" },
            "value": path,
            "score": 90
        }));
    }

    // Well-known directory
    if let Some(path) = resolve_well_known_dir(trimmed_input) {
        results.push(serde_json::json!({
            "type": "path",
            "pathType": "folder",
            "value": path,
            "score": 87
        }));
    }

    // System command
    if let Some((cmd_id, cmd_label, needs_confirm)) = match_system_command(trimmed_input) {
        results.push(serde_json::json!({
            "type": "system",
            "cmdId": cmd_id,
            "cmdLabel": cmd_label,
            "needsConfirm": needs_confirm,
            "score": 86
        }));
    }

    // App search — use original input (preserving spaces) so "w " doesn't match "word"
    let launcher = features.app_launcher.lock().await;
    let app_matches = launcher.find_app(&input);
    for (i, app) in app_matches.iter().enumerate() {
        results.push(serde_json::json!({
            "type": "app",
            "name": app.name,
            "path": app.path,
            "icon_base64": app.icon_base64,
            "emoji_icon": app.emoji_icon,
            "score": 80 - i
        }));
    }

    // Sort by score descending
    results.sort_by(|a, b| {
        let sa = a.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        let sb = b.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        sb.cmp(&sa)
    });

    Ok(serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string()))
}

#[tauri::command]
pub async fn launch_app_by_name<R: tauri::Runtime>(
    app_name: String,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    info!("Launching app by name: {}", app_name);

    let launcher = features.app_launcher.lock().await;
    let matches = launcher.find_app(&app_name);

    if let Some(app_to_launch) = matches.first() {
        launcher.launch(app_to_launch).map_err(|e| {
            error!("Failed to launch {}: {}", app_name, e);
            format!("Failed to launch {}: {}", app_name, e)
        })?;

        if let Some(floating_window) = app.get_webview_window(window_labels::FLOATING) {
            let _ = floating_window.hide();
        }

        Ok(())
    } else {
        Err(format!("Application not found: {}", app_name).into())
    }
}

#[tauri::command]
pub async fn open_url<R: tauri::Runtime>(
    url: String,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    info!("Opening URL: {}", url);

    let full_url = if url.starts_with("www.") {
        format!("https://{}", url)
    } else {
        url.clone()
    };

    os::open_url(&full_url).map_err(|e| format!("Failed to open URL: {}", e))?;

    if let Some(floating_window) = app.get_webview_window(window_labels::FLOATING) {
        let _ = floating_window.hide();
    }

    Ok(())
}

#[tauri::command]
pub async fn open_path<R: tauri::Runtime>(
    path: String,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
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
        return Err(format!("Path does not exist: {}", expanded_path).into());
    }

    os::open_path(&expanded_path).map_err(|e| format!("Failed to open path: {}", e))?;

    if let Some(floating_window) = app.get_webview_window(window_labels::FLOATING) {
        let _ = floating_window.hide();
    }

    Ok(())
}

#[tauri::command]
pub async fn execute_shortcut<R: tauri::Runtime>(
    path: String,
    args: Vec<String>,
    working_directory: Option<String>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
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

    crate::os::configure_breakaway_from_job(&mut command);

    command
        .spawn()
        .map_err(|e| format!("Failed to execute shortcut: {}", e))?;

    // Anonymous shortcut-usage event. We deliberately don't send the
    // path or args — those can contain personal info like `~/Documents/...`.
    // The only property is the count of args so we can see "does anyone
    // actually use args?" in aggregate.
    crate::telemetry::track(
        &app,
        "shortcut_triggered",
        Some(serde_json::json!({ "arg_count": args.len() })),
    );

    info!("Shortcut executed successfully");
    Ok(())
}

#[tauri::command]
pub async fn execute_system_command<R: tauri::Runtime>(
    command_id: String,
    elevated: Option<bool>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    let elevated = elevated.unwrap_or(false);
    info!(
        "Executing system command: {}{}",
        command_id,
        if elevated { " (elevated)" } else { "" }
    );

    // Hide the floating window first
    if let Some(floating) = app.get_webview_window(window_labels::FLOATING) {
        let _ = floating.hide();
    }

    let result = if elevated {
        let (program, args) = crate::os::shell::system_command(command_id.as_str());
        crate::os::shell::spawn_elevated(program, &args)
    } else {
        let (program, args) = crate::os::shell::system_command(command_id.as_str());
        let mut cmd = std::process::Command::new(program);
        cmd.args(args);
        crate::os::process::spawn_detached(&mut cmd)
    };

    match result {
        Ok(_) => {
            info!("System command '{}' executed", command_id);
            Ok(())
        }
        Err(e) => {
            error!("Failed to execute system command '{}': {}", command_id, e);
            Err(format!("Failed to execute: {}", e).into())
        }
    }
}

pub mod link_metadata;
