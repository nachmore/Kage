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
        (&["templates", "template"], dirs::template_dir),
        (&["public"], dirs::public_dir),
        (&["fonts", "font"], dirs::font_dir),
        (&["cache"], dirs::cache_dir),
        (&["config", "configuration"], dirs::config_dir),
        (&["data"], dirs::data_dir),
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
        (&["sleep"], "sleep", "😴 Sleep", false, "all"),
        (&["screenshot"], "screenshot", "📸 Screenshot", false, "all"),
        (&["mute"], "mute", "🔇 Mute Audio", false, "all"),
        (&["unmute"], "unmute", "🔊 Unmute Audio", false, "all"),
        (&["emoji"], "emoji", "😀 Emoji Picker", false, "all"),
        (&["trash", "recycle"], "trash", "🗑️ Open Recycle Bin", false, "windows"),
        (&["trash"], "trash", "🗑️ Open Trash", false, "macos"),
        (&["trash"], "trash", "🗑️ Open Trash", false, "linux"),
        (&["taskmanager", "taskmgr"], "taskmanager", "📊 Task Manager", false, "windows"),
        (&["activitymonitor", "taskmanager", "taskmgr"], "taskmanager", "📊 Activity Monitor", false, "macos"),
        (&["taskmanager", "taskmgr", "systemmonitor"], "taskmanager", "📊 System Monitor", false, "linux"),
        (&["terminal", "cmd", "powershell"], "terminal", "💻 Terminal", false, "windows"),
        (&["terminal"], "terminal", "💻 Terminal", false, "macos"),
        (&["terminal"], "terminal", "💻 Terminal", false, "linux"),
        (&["explorer"], "filemanager", "📁 File Explorer", false, "windows"),
        (&["finder"], "filemanager", "📁 Finder", false, "macos"),
        (&["files", "nautilus"], "filemanager", "📁 Files", false, "linux"),
        // With confirmation
        (&["restart", "reboot"], "restart", "🔄 Restart Computer", true, "all"),
        (&["shutdown"], "shutdown", "⏻ Shut Down", true, "all"),
        (&["signout", "logout", "logoff"], "signout", "🚪 Sign Out", true, "all"),
    ];

    let platform = if cfg!(target_os = "windows") { "windows" }
        else if cfg!(target_os = "macos") { "macos" }
        else { "linux" };

    // Exact match first
    for &(aliases, id, label, confirm, plat) in commands {
        if plat != "all" && plat != platform { continue; }
        if aliases.iter().any(|a| *a == lower.as_str()) {
            return Some((id, label, confirm));
        }
    }
    // Prefix match
    for &(aliases, id, label, confirm, plat) in commands {
        if plat != "all" && plat != platform { continue; }
        if aliases.iter().any(|a| a.starts_with(lower.as_str())) {
            return Some((id, label, confirm));
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

    // Check for system commands (single word, exact or prefix match)
    if let Some((cmd_id, cmd_label, needs_confirm)) = match_system_command(trimmed_input) {
        info!("Detected system command: {} ({})", cmd_id, cmd_label);
        return Ok(format!("system:{}:{}:{}", cmd_id, cmd_label, if needs_confirm { "confirm" } else { "immediate" }));
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

#[tauri::command]
pub async fn execute_system_command(
    command_id: String,
    elevated: Option<bool>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let elevated = elevated.unwrap_or(false);
    info!("Executing system command: {}{}", command_id, if elevated { " (elevated)" } else { "" });

    // Hide the floating window first
    if let Some(floating) = app.get_webview_window("floating") {
        let _ = floating.hide();
    }

    let result = if elevated {
        spawn_elevated(&command_id)
    } else {
        std::process::Command::new(get_system_command_program(&command_id))
            .args(get_system_command_args(&command_id))
            .spawn()
    };

    match result {
        Ok(_) => {
            info!("System command '{}' executed", command_id);
            Ok(())
        }
        Err(e) => {
            error!("Failed to execute system command '{}': {}", command_id, e);
            Err(format!("Failed to execute: {}", e))
        }
    }
}

/// Spawn a process with elevated (admin) privileges.
#[cfg(target_os = "windows")]
fn spawn_elevated(command_id: &str) -> std::io::Result<std::process::Child> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::core::PCWSTR;

    let program = get_system_command_program(command_id);
    let args = get_system_command_args(command_id).join(" ");

    let verb: Vec<u16> = std::ffi::OsStr::new("runas").encode_wide().chain(std::iter::once(0)).collect();
    let file: Vec<u16> = std::ffi::OsStr::new(program).encode_wide().chain(std::iter::once(0)).collect();
    let params: Vec<u16> = std::ffi::OsStr::new(&args).encode_wide().chain(std::iter::once(0)).collect();

    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(if args.is_empty() { std::ptr::null() } else { params.as_ptr() }),
            PCWSTR(std::ptr::null()),
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        )
    };

    // ShellExecuteW returns an HINSTANCE; values > 32 mean success
    if result.0 as usize > 32 {
        // Return a dummy child — ShellExecuteW doesn't give us a process handle
        std::process::Command::new("cmd").args(&["/C", "rem"]).spawn()
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::Other,
            format!("ShellExecuteW failed with code {}", result.0 as usize)))
    }
}

#[cfg(not(target_os = "windows"))]
fn spawn_elevated(command_id: &str) -> std::io::Result<std::process::Child> {
    // On Unix, use pkexec for graphical sudo
    let program = get_system_command_program(command_id);
    let mut args = vec![program];
    args.extend(get_system_command_args(command_id));
    std::process::Command::new("pkexec")
        .args(&args)
        .spawn()
}

#[cfg(target_os = "windows")]
fn get_system_command_program(cmd: &str) -> &'static str {
    match cmd {
        "lock" => "rundll32.exe",
        "sleep" => "rundll32.exe",
        "screenshot" => "snippingtool",
        "mute" | "unmute" => "powershell",
        "emoji" => "cmd",
        "trash" => "explorer.exe",
        "taskmanager" => "taskmgr.exe",
        "terminal" => "wt.exe",
        "filemanager" => "explorer.exe",
        "restart" => "shutdown",
        "shutdown" => "shutdown",
        "signout" => "shutdown",
        _ => "cmd",
    }
}

#[cfg(target_os = "windows")]
fn get_system_command_args(cmd: &str) -> Vec<&'static str> {
    match cmd {
        "lock" => vec!["user32.dll,LockWorkStation"],
        "sleep" => vec!["powrprof.dll,SetSuspendState", "0,1,0"],
        "screenshot" => vec![],
        "mute" => vec!["-NoProfile", "-Command",
            "(New-Object -ComObject WScript.Shell).SendKeys([char]173)"],
        "unmute" => vec!["-NoProfile", "-Command",
            "(New-Object -ComObject WScript.Shell).SendKeys([char]173)"],
        "emoji" => vec!["/C", "start", "ms-inputapp:///emojiandmore"],
        "trash" => vec!["shell:RecycleBinFolder"],
        "taskmanager" => vec![],
        "terminal" => vec![],
        "filemanager" => vec![],
        "restart" => vec!["/r", "/t", "0"],
        "shutdown" => vec!["/s", "/t", "0"],
        "signout" => vec!["/l"],
        _ => vec![],
    }
}

#[cfg(target_os = "macos")]
fn get_system_command_program(cmd: &str) -> &'static str {
    match cmd {
        "emoji" => "osascript",
        "taskmanager" => "open",
        "terminal" => "open",
        "filemanager" => "open",
        "trash" => "open",
        _ => "osascript",
    }
}

#[cfg(target_os = "macos")]
fn get_system_command_args(cmd: &str) -> Vec<&'static str> {
    match cmd {
        "lock" => vec!["-e", "tell application \"System Events\" to keystroke \"q\" using {command down, control down}"],
        "sleep" => vec!["-e", "tell application \"System Events\" to sleep"],
        "screenshot" => vec!["-e", "do shell script \"screencapture -ic\""],
        "mute" => vec!["-e", "set volume with output muted"],
        "unmute" => vec!["-e", "set volume without output muted"],
        "emoji" => vec!["-e", "tell application \"System Events\" to keystroke \" \" using {command down, control down}"],
        "trash" => vec!["-a", "Finder", "/Users"],  // Opens Finder, user navigates to trash
        "taskmanager" => vec!["-a", "Activity Monitor"],
        "terminal" => vec!["-a", "Terminal"],
        "filemanager" => vec!["-a", "Finder"],
        "restart" => vec!["-e", "tell application \"System Events\" to restart"],
        "shutdown" => vec!["-e", "tell application \"System Events\" to shut down"],
        "signout" => vec!["-e", "tell application \"System Events\" to log out"],
        _ => vec![],
    }
}

#[cfg(target_os = "linux")]
fn get_system_command_program(cmd: &str) -> &'static str {
    match cmd {
        "lock" => "loginctl",
        "sleep" => "systemctl",
        "screenshot" => "gnome-screenshot",
        "mute" => "amixer",
        "unmute" => "amixer",
        "emoji" => "ibus",
        "taskmanager" => "gnome-system-monitor",
        "terminal" => "x-terminal-emulator",
        "filemanager" => "xdg-open",
        "trash" => "xdg-open",
        "restart" => "systemctl",
        "shutdown" => "systemctl",
        "signout" => "loginctl",
        _ => "true",
    }
}

#[cfg(target_os = "linux")]
fn get_system_command_args(cmd: &str) -> Vec<&'static str> {
    match cmd {
        "lock" => vec!["lock-session"],
        "sleep" => vec!["suspend"],
        "screenshot" => vec!["-c"],  // to clipboard
        "mute" => vec!["set", "Master", "mute"],
        "unmute" => vec!["set", "Master", "unmute"],
        "emoji" => vec!["emoji"],
        "taskmanager" => vec![],
        "terminal" => vec![],
        "filemanager" => vec!["."],
        "trash" => vec!["trash:///"],
        "restart" => vec!["reboot"],
        "shutdown" => vec!["poweroff"],
        "signout" => vec!["terminate-user", ""],
        _ => vec![],
    }
}
