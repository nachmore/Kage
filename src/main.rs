mod acp_client;
mod app_launcher;
mod config;
mod logger;
mod os;
mod process_manager;

use acp_client::AcpClient;
use app_launcher::AppLauncher;
use config::Config;
use log::{error, info, warn};
use process_manager::ProcessManager;
use std::sync::Arc;
use tauri::{
    async_runtime,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, State, WebviewWindow,
};
use tokio::sync::Mutex;

// Platform-specific cursor position detection
fn get_cursor_position() -> Option<(i32, i32)> {
    os::get_cursor_position()
}

/// Find which monitor contains the given point
fn find_monitor_at_position(window: &WebviewWindow, x: i32, y: i32) -> Option<tauri::Monitor> {
    if let Ok(monitors) = window.available_monitors() {
        for monitor in monitors {
            let pos = monitor.position();
            let size = monitor.size();

            if x >= pos.x
                && x < pos.x + size.width as i32
                && y >= pos.y
                && y < pos.y + size.height as i32
            {
                return Some(monitor);
            }
        }
    }
    None
}

/// Get the active monitor (where cursor is) or fall back to primary
fn get_active_monitor(window: &WebviewWindow) -> Option<tauri::Monitor> {
    if let Some((cursor_x, cursor_y)) = get_cursor_position() {
        println!("     Cursor position: ({}, {})", cursor_x, cursor_y);

        if let Some(monitor) = find_monitor_at_position(window, cursor_x, cursor_y) {
            println!("     Found active monitor at cursor position");
            return Some(monitor);
        }
    }

    println!("     Falling back to primary monitor");
    window.primary_monitor().ok().flatten()
}

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
async fn handle_floating_input(
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
async fn launch_app_by_name(
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

struct AppState {
    acp_client: Arc<Mutex<AcpClient>>,
    config: Arc<Mutex<Config>>,
    app_launcher: Arc<Mutex<AppLauncher>>,
    pipe_stdin: Arc<std::sync::Mutex<Option<Arc<std::sync::Mutex<std::process::ChildStdin>>>>>,
    tcp_writer: Arc<std::sync::Mutex<Option<std::net::TcpStream>>>,
    dev_mode: bool,
}

#[tauri::command]
async fn open_url(url: String, app: tauri::AppHandle) -> Result<(), String> {
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
async fn open_path(path: String, app: tauri::AppHandle) -> Result<(), String> {
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
async fn execute_shortcut(
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
async fn send_message_streaming(
    message: String,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<(), String> {
    info!("Sending message: {}", message);
    let client = state.acp_client.clone();
    let config = state.config.clone();
    let pipe_stdin_handle = state.pipe_stdin.clone();
    let tcp_writer_handle = state.tcp_writer.clone();
    let window_clone = window.clone();

    async_runtime::spawn_blocking(move || {
        let client = async_runtime::block_on(client.lock());

        if !client.is_connected() {
            info!("Not connected, attempting to connect...");
            if let Err(e) = client.connect() {
                error!("Connection failed: {}", e);
                let error_msg = format!(
                    "Unable to connect to Kiro CLI. Please ensure kiro-cli is running.\n\nError: {}",
                    e
                );
                let _ = window.emit("message_error", error_msg);
                return;
            }
        }

        // Create permission callback
        let window_for_permission = window_clone.clone();
        let config_for_permission = config.clone();
        let pipe_stdin_for_perm = pipe_stdin_handle.clone();
        let tcp_writer_for_perm = tcp_writer_handle.clone();
        let permission_callback = Box::new(move |notification: serde_json::Value| {
            let mut config_guard = async_runtime::block_on(config_for_permission.lock());

            let tool_title = notification
                .get("params")
                .and_then(|p| p.get("toolCall"))
                .and_then(|tc| tc.get("title"))
                .and_then(|t| t.as_str())
                .unwrap_or("unknown");

            let timestamp = chrono::Utc::now().to_rfc3339();
            let existing = config_guard
                .tool_permissions
                .tools
                .iter_mut()
                .find(|t| t.title == tool_title);
            if let Some(tool) = existing {
                tool.last_seen = timestamp;
            } else {
                config_guard
                    .tool_permissions
                    .tools
                    .push(crate::config::ToolPolicy {
                        title: tool_title.to_string(),
                        policy: "ask".to_string(),
                        last_seen: timestamp,
                    });
            }
            let _ = config_guard.save();

            let policy = if config_guard.tool_permissions.trust_all {
                "allow".to_string()
            } else {
                config_guard
                    .tool_permissions
                    .tools
                    .iter()
                    .find(|t| t.title == tool_title)
                    .map(|t| t.policy.clone())
                    .unwrap_or_else(|| "ask".to_string())
            };

            drop(config_guard);

            let send_response = |option_id: &str| {
                if let Some(request_id) = notification.get("id") {
                    let response = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "result": { "outcome": { "outcome": "selected", "optionId": option_id } }
                    });
                    if let Ok(response_json) = serde_json::to_string(&response) {
                        use std::io::Write;
                        info!(
                            "📤 Auto-responding permission ({}): {}",
                            option_id, response_json
                        );

                        if let Ok(guard) = pipe_stdin_for_perm.lock() {
                            if let Some(ref stdin_arc) = *guard {
                                let stdin_clone = stdin_arc.clone();
                                drop(guard);
                                if let Ok(mut stdin) = stdin_clone.lock() {
                                    let _ = write!(stdin, "{}\n", response_json);
                                    let _ = stdin.flush();
                                    info!("✅ Auto-response sent via Pipe");
                                    return;
                                };
                            }
                        }
                        if let Ok(guard) = tcp_writer_for_perm.lock() {
                            if let Some(ref stream) = *guard {
                                if let Ok(mut ws) = stream.try_clone() {
                                    drop(guard);
                                    let _ = write!(ws, "{}\n", response_json);
                                    let _ = ws.flush();
                                    info!("✅ Auto-response sent via TCP");
                                    return;
                                }
                            }
                        }
                        error!("❌ Failed to send auto-response: no write handle");
                    }
                }
            };

            match policy.as_str() {
                "allow" => {
                    info!("🔓 Policy=allow for tool: {}", tool_title);
                    send_response("allow_once");
                }
                "deny" => {
                    info!("🚫 Policy=deny for tool: {}", tool_title);
                    send_response("reject_once");
                }
                _ => {
                    info!("❓ Policy=ask for tool: {}", tool_title);
                    let _ = window_for_permission.emit(
                        "permission_request",
                        serde_json::json!({
                            "notification": notification,
                            "auto_approve": false
                        }),
                    );
                }
            }
        });

        // Create notification callback for tool_call updates
        let window_for_notif = window.clone();
        let notification_callback = Box::new(move |notification: serde_json::Value| {
            let _ = window_for_notif.emit("tool_call_update", notification);
        });

        if let Err(e) = client.send_chat_streaming(
            message,
            |chunk| {
                let _ = window.emit("message_chunk", chunk);
            },
            Some(permission_callback),
            Some(notification_callback),
        ) {
            error!("Send error: {}", e);
            let error_msg = format!(
                "Failed to send message. The connection may have been lost.\n\nError: {}",
                e
            );
            let _ = window.emit("message_error", error_msg);
            return;
        }

        let _ = window.emit("message_complete", ());
    });

    Ok(())
}

#[tauri::command]
async fn send_permission_response(
    request_id: serde_json::Value,
    option_id: String,
    tool_title: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    info!(
        "Sending permission response: option_id={}, tool_title={}",
        option_id, tool_title
    );

    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": {
            "outcome": {
                "outcome": "selected",
                "optionId": option_id
            }
        }
    });
    let response_json =
        serde_json::to_string(&response).map_err(|e| format!("Failed to serialize: {}", e))?;

    info!("📤 Permission response JSON: {}", response_json);

    use std::io::Write;

    let sent = {
        let pipe_guard = state
            .pipe_stdin
            .lock()
            .map_err(|e| format!("Failed to lock pipe_stdin: {}", e))?;
        if let Some(ref stdin_arc) = *pipe_guard {
            let stdin_clone = stdin_arc.clone();
            drop(pipe_guard);
            let mut stdin = stdin_clone
                .lock()
                .map_err(|e| format!("Failed to lock stdin: {}", e))?;
            write!(stdin, "{}\n", response_json)
                .map_err(|e| format!("Failed to write: {}", e))?;
            stdin
                .flush()
                .map_err(|e| format!("Failed to flush: {}", e))?;
            info!("✅ Permission response sent via Pipe");
            true
        } else {
            drop(pipe_guard);
            let tcp_guard = state
                .tcp_writer
                .lock()
                .map_err(|e| format!("Failed to lock tcp_writer: {}", e))?;
            if let Some(ref stream) = *tcp_guard {
                let mut write_stream = stream
                    .try_clone()
                    .map_err(|e| format!("Failed to clone stream: {}", e))?;
                drop(tcp_guard);
                write!(write_stream, "{}\n", response_json)
                    .map_err(|e| format!("Failed to write: {}", e))?;
                write_stream
                    .flush()
                    .map_err(|e| format!("Failed to flush: {}", e))?;
                info!("✅ Permission response sent via TCP");
                true
            } else {
                drop(tcp_guard);
                false
            }
        }
    };

    if !sent {
        return Err("Not connected - no write handle available".to_string());
    }

    if option_id == "allow_always" {
        let mut config = state.config.lock().await;
        if let Some(tool) = config
            .tool_permissions
            .tools
            .iter_mut()
            .find(|t| t.title == tool_title)
        {
            tool.policy = "allow".to_string();
        }
        config
            .save()
            .map_err(|e| format!("Failed to save config: {}", e))?;
    }

    Ok(())
}

#[tauri::command]
async fn update_tool_policy(
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
async fn remove_tool_permission(
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
async fn is_dev_mode(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.dev_mode)
}

#[tauri::command]
async fn open_devtools(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("floating") {
        window.open_devtools();
    }
    Ok(())
}

#[tauri::command]
async fn read_clipboard() -> Result<String, String> {
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

#[tauri::command]
async fn show_context_menu(
    x: i32,
    y: i32,
    _state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("context-menu") {
        // Reposition and show the cached window
        window
            .set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }))
            .map_err(|e| format!("Failed to position context menu: {}", e))?;
        window
            .show()
            .map_err(|e| format!("Failed to show context menu: {}", e))?;
        window
            .set_focus()
            .map_err(|e| format!("Failed to focus context menu: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
async fn check_connection(state: State<'_, AppState>) -> Result<bool, String> {
    let client = state.acp_client.lock().await;
    let is_connected = client.is_connected();
    info!(
        "Connection check: {}",
        if is_connected {
            "connected"
        } else {
            "disconnected"
        }
    );
    Ok(is_connected)
}

#[tauri::command]
async fn open_chat_with_message(
    message: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    info!("Opening chat with message: {}", message);

    if let Some(floating_window) = app.get_webview_window("floating") {
        let _ = floating_window.hide();
    }

    if let Some(main_window) = app.get_webview_window("main") {
        let _ = main_window.show();
        let _ = main_window.set_focus();

        let _ = main_window.emit("initial_message", message.clone());

        let client = state.acp_client.clone();
        let window = main_window.clone();

        async_runtime::spawn_blocking(move || {
            let client = async_runtime::block_on(client.lock());

            if !client.is_connected() {
                info!("Not connected, attempting to connect...");
                if let Err(e) = client.connect() {
                    error!("Connection failed: {}", e);
                    let error_msg = format!(
                        "Unable to connect to Kiro CLI. Please ensure kiro-cli is running.\n\nError: {}",
                        e
                    );
                    let _ = window.emit("message_error", error_msg);
                    return;
                }
            }

            if let Err(e) = client.send_chat_streaming(
                message,
                |chunk| {
                    let _ = window.emit("message_chunk", chunk);
                },
                None,
                None,
            ) {
                error!("Send error: {}", e);
                let error_msg = format!(
                    "Failed to send message. The connection may have been lost.\n\nError: {}",
                    e
                );
                let _ = window.emit("message_error", error_msg);
                return;
            }

            let _ = window.emit("message_complete", ());
        });
    }

    Ok(())
}

#[tauri::command]
async fn get_config(state: State<'_, AppState>) -> Result<Config, String> {
    let config = state.config.lock().await;
    Ok(config.clone())
}

#[tauri::command]
async fn save_config(
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

    // In Tauri v2, emit() sends to all listeners by default
    if let Err(e) = app.emit("config_updated", ()) {
        error!("Failed to emit config_updated event: {}", e);
    }

    Ok(())
}

#[tauri::command]
async fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    info!("Opening settings window");
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.set_focus();
    }
    Ok(())
}

#[tauri::command]
async fn reconnect_acp(state: State<'_, AppState>) -> Result<bool, String> {
    info!("Manual reconnection requested");
    let client = state.acp_client.lock().await;

    match client.connect() {
        Ok(_) => {
            info!("Reconnection successful");
            Ok(true)
        }
        Err(e) => {
            error!("Reconnection failed: {}", e);
            Err(format!("Failed to reconnect: {}", e))
        }
    }
}

#[tauri::command]
async fn test_floating_window(app: tauri::AppHandle) -> Result<String, String> {
    info!("Testing floating window visibility");
    println!("🧪 Testing floating window...");

    if let Some(window) = app.get_webview_window("floating") {
        let is_visible = window.is_visible().unwrap_or(false);
        println!(
            "   Current state: {}",
            if is_visible { "VISIBLE" } else { "HIDDEN" }
        );

        if is_visible {
            println!("   Action: Hiding window");
            window
                .hide()
                .map_err(|e| format!("Failed to hide: {}", e))?;
            println!("   ✅ Window hidden");
            Ok("Window was visible, now hidden".to_string())
        } else {
            println!("   Action: Showing window");
            window.show().map_err(|e| {
                println!("   ❌ Failed to show: {}", e);
                format!("Failed to show: {}", e)
            })?;
            println!("   ✅ Window shown");

            println!("   Action: Setting focus");
            window.set_focus().map_err(|e| {
                println!("   ⚠️  Failed to focus: {}", e);
                format!("Failed to focus: {}", e)
            })?;
            println!("   ✅ Window focused");

            if let Some(monitor) = get_active_monitor(&window) {
                let pos = monitor.position();
                let size = monitor.size();
                println!(
                    "   Monitor position: ({}, {}), size: {}x{}",
                    pos.x, pos.y, size.width, size.height
                );

                let window_size = window
                    .inner_size()
                    .unwrap_or(tauri::PhysicalSize {
                        width: 500,
                        height: 60,
                    });
                let x = pos.x + (size.width as i32 - window_size.width as i32) / 2;
                let y = pos.y + size.height as i32 / 3;

                println!(
                    "   Window size: {}x{}",
                    window_size.width, window_size.height
                );
                println!("   Positioning at: ({}, {})", x, y);
                window
                    .set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }))
                    .map_err(|e| {
                        println!("   ⚠️  Failed to position: {}", e);
                        format!("Failed to position: {}", e)
                    })?;
                println!("   ✅ Window positioned");
            }

            Ok("Window was hidden, now visible and positioned".to_string())
        }
    } else {
        println!("   ❌ Floating window not found!");
        Err("Floating window not found".to_string())
    }
}

#[tauri::command]
async fn start_drag_window(window: WebviewWindow) -> Result<(), String> {
    info!("Starting window drag");
    window.start_dragging().map_err(|e| {
        error!("Failed to start dragging: {}", e);
        e.to_string()
    })
}

#[tauri::command]
async fn open_chat_window(app: tauri::AppHandle) -> Result<(), String> {
    info!("Opening chat window");

    if let Some(floating_window) = app.get_webview_window("floating") {
        let _ = floating_window.hide();
    }

    if let Some(window) = app.get_webview_window("main") {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    } else {
        warn!("Main window not found, this shouldn't happen");
    }

    Ok(())
}

#[tauri::command]
async fn resize_floating_window(
    window: WebviewWindow,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<(), String> {
    let current_size = window.inner_size().map_err(|e| {
        error!("Failed to get current window size: {}", e);
        e.to_string()
    })?;

    let target_width = width.unwrap_or(current_size.width);
    let target_height = height.unwrap_or(current_size.height);

    info!(
        "Resizing floating window to {}x{}",
        target_width, target_height
    );

    let current_height = current_size.height;

    if (current_height as i32 - target_height as i32).abs() < 20 {
        return window
            .set_size(tauri::Size::Physical(tauri::PhysicalSize {
                width: target_width,
                height: target_height,
            }))
            .map_err(|e| {
                error!("Failed to resize window: {}", e);
                e.to_string()
            });
    }

    let steps = 10;
    let height_diff = target_height as i32 - current_height as i32;
    let step_size = height_diff as f32 / steps as f32;

    for i in 1..=steps {
        let new_height = (current_height as f32 + step_size * i as f32) as u32;

        if let Err(e) = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
            width: target_width,
            height: new_height,
        })) {
            error!("Failed to resize window during animation: {}", e);
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(15)).await;
    }

    window
        .set_size(tauri::Size::Physical(tauri::PhysicalSize {
            width: target_width,
            height: target_height,
        }))
        .map_err(|e| {
            error!("Failed to resize window: {}", e);
            e.to_string()
        })
}

/// Helper to toggle the floating window visibility and position it
fn toggle_floating_window(window: &WebviewWindow) {
    match window.is_visible() {
        Ok(is_visible) => {
            println!("   Window visible state: {}", is_visible);
            if is_visible {
                println!("  → Hiding floating window");
                match window.hide() {
                    Ok(_) => println!("     ✅ Window hidden successfully"),
                    Err(e) => println!("     ❌ Failed to hide: {}", e),
                }
            } else {
                println!("  → Showing floating window");
                match window.show() {
                    Ok(_) => {
                        println!("     ✅ Window shown successfully");
                        match window.set_focus() {
                            Ok(_) => println!("     ✅ Window focused successfully"),
                            Err(e) => println!("     ⚠️  Failed to focus: {}", e),
                        }
                        if let Some(monitor) = get_active_monitor(window) {
                            let pos = monitor.position();
                            let size = monitor.size();
                            println!(
                                "     Monitor position: ({}, {}), size: {}x{}",
                                pos.x, pos.y, size.width, size.height
                            );

                            let window_size = window.inner_size().unwrap_or(tauri::PhysicalSize {
                                width: 500,
                                height: 60,
                            });
                            let x =
                                pos.x + (size.width as i32 - window_size.width as i32) / 2;
                            let y = pos.y + size.height as i32 / 3;

                            println!(
                                "     Window size: {}x{}",
                                window_size.width, window_size.height
                            );
                            println!("     Positioning at: ({}, {})", x, y);
                            if let Err(e) = window.set_position(tauri::Position::Physical(
                                tauri::PhysicalPosition { x, y },
                            )) {
                                println!("     ⚠️  Failed to position: {}", e);
                            }
                        }
                    }
                    Err(e) => println!("     ❌ Failed to show: {}", e),
                }
            }
        }
        Err(e) => {
            println!("     ❌ Failed to check visibility: {}", e);
        }
    }
}

fn main() {
    // Initialize logger first
    if let Err(e) = logger::init_logger() {
        eprintln!("Failed to initialize logger: {}", e);
        eprintln!("Continuing without file logging...");
    }

    info!("=== Kiro Assistant Starting ===");

    let args: Vec<String> = std::env::args().collect();
    let dev_mode = args.iter().any(|arg| arg == "/dev" || arg == "--dev");
    let debug_mode = args.iter().any(|arg| arg == "/debug" || arg == "--debug");

    if debug_mode {
        println!("🐛 DEBUG MODE ENABLED - Detailed ACP logs will be printed to console");
        info!("🐛 DEBUG MODE ENABLED via command line argument");
        logger::enable_console_logging();
    }

    info!("Checking for orphaned processes...");
    if let Err(e) = ProcessManager::cleanup_orphaned_processes() {
        warn!("Failed to cleanup orphaned processes: {}", e);
    }

    let mut config = Config::load().unwrap_or_else(|e| {
        error!("Failed to load config, using defaults: {}", e);
        eprintln!("Failed to load config, using defaults: {}", e);
        Config::default()
    });

    if debug_mode {
        config.debug_mode = true;
    }

    info!("Configuration loaded");

    let acp_client = match &config.acp.mode {
        crate::config::AcpMode::Local { spawn_command } => {
            info!("ACP Mode: Local with spawn command: {}", spawn_command);
            AcpClient::new(acp_client::AcpConnectionMode::Local {
                spawn_command: spawn_command.clone(),
            })
        }
        crate::config::AcpMode::Remote {
            host,
            port,
            timeout_ms,
        } => {
            info!(
                "ACP Mode: Remote at {}:{} (timeout: {}ms)",
                host, port, timeout_ms
            );
            AcpClient::new(acp_client::AcpConnectionMode::Remote {
                host: host.clone(),
                port: *port,
            })
        }
    };

    acp_client.set_debug_mode(config.debug_mode);

    let process_manager = acp_client.get_process_manager();
    process_manager::install_signal_handlers(process_manager);

    let app_launcher = AppLauncher::new().unwrap_or_else(|e| {
        error!("Failed to initialize app launcher: {}", e);
        eprintln!("Failed to initialize app launcher: {}", e);
        AppLauncher::new().unwrap()
    });
    info!("App launcher initialized");

    let pipe_stdin_handle = acp_client.get_pipe_stdin();
    let tcp_writer_handle = acp_client.get_tcp_writer();

    let config_for_setup = config.clone();
    let dev_mode_for_setup = dev_mode;

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_global_shortcut::Builder::default().build())
        .manage(AppState {
            acp_client: Arc::new(Mutex::new(acp_client)),
            config: Arc::new(Mutex::new(config)),
            app_launcher: Arc::new(Mutex::new(app_launcher)),
            pipe_stdin: pipe_stdin_handle,
            tcp_writer: tcp_writer_handle,
            dev_mode,
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                window.hide().unwrap();
                api.prevent_close();
            }
        })
        .setup(move |app| {
            info!("Setting up application");
            println!("=== KIRO ASSISTANT SETUP ===");

            let config = config_for_setup;
            let dev_mode = dev_mode_for_setup;

            // Build tray menu
            let show = MenuItemBuilder::with_id("show", "Show").build(app)?;
            let settings = MenuItemBuilder::with_id("settings", "Settings").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

            let menu = if dev_mode {
                info!("Dev mode enabled - adding developer menu items");
                println!("🔧 Dev mode enabled - adding developer menu items");
                let inspect = MenuItemBuilder::with_id("inspect", "Inspect").build(app)?;
                let reload = MenuItemBuilder::with_id("reload", "Reload UX").build(app)?;
                MenuBuilder::new(app)
                    .items(&[&show, &settings])
                    .separator()
                    .items(&[&inspect, &reload])
                    .separator()
                    .item(&quit)
                    .build()?
            } else {
                MenuBuilder::new(app)
                    .items(&[&show, &settings])
                    .separator()
                    .item(&quit)
                    .build()?
            };

            // Build tray icon
            let app_handle = app.handle().clone();
            TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().unwrap())
                .menu(&menu)
                .on_menu_event(move |app_handle_inner, event| {
                    info!("System tray menu item clicked: {}", event.id().as_ref());
                    match event.id().as_ref() {
                        "show" => {
                            if let Some(window) = app_handle_inner.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "settings" => {
                            if let Some(window) = app_handle_inner.get_webview_window("settings") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "inspect" => {
                            info!("Opening inspector");
                            #[cfg(debug_assertions)]
                            if let Some(window) = app_handle_inner.get_webview_window("main") {
                                window.open_devtools();
                            }
                        }
                        "reload" => {
                            info!("Reloading UX");
                            if let Some(window) = app_handle_inner.get_webview_window("main") {
                                let _ = window.eval("window.location.reload()");
                            }
                            if let Some(window) = app_handle_inner.get_webview_window("floating") {
                                let _ = window.eval("window.location.reload()");
                            }
                            if let Some(window) = app_handle_inner.get_webview_window("settings") {
                                let _ = window.eval("window.location.reload()");
                            }
                        }
                        "quit" => {
                            info!("Application quit requested");
                            if let Some(state) = app_handle_inner.try_state::<AppState>() {
                                if let Ok(client) = state.acp_client.try_lock() {
                                    client.disconnect();
                                }
                            }
                            std::process::exit(0);
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
                        if let Some(window) = app_handle.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            // Register global hotkey
            let floating_window = app.get_webview_window("floating").unwrap();

            // Make the webview background fully transparent (removes the white border on Windows)
            let _ = floating_window.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
            // Remove the DWM window shadow/border on Windows
            #[cfg(target_os = "windows")]
            let _ = floating_window.set_shadow(false);

            // Apply same transparency fixes to the cached context-menu window
            if let Some(ctx_menu) = app.get_webview_window("context-menu") {
                let _ = ctx_menu.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
                #[cfg(target_os = "windows")]
                let _ = ctx_menu.set_shadow(false);
            }

            let hotkey_string = config.get_hotkey_string();

            info!(
                "Attempting to register global hotkey: {}",
                hotkey_string
            );
            println!(
                "Attempting to register global hotkey: {}",
                hotkey_string
            );

            use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

            let window_for_primary = floating_window.clone();
            let hotkey_str = hotkey_string.clone();
            let registration_result = app.global_shortcut().on_shortcut(
                hotkey_string.as_str(),
                move |_app, _shortcut, event| {
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    println!(
                        "🔥 HOTKEY TRIGGERED: {}",
                        chrono::Local::now().format("%H:%M:%S%.3f")
                    );
                    info!("Hotkey triggered");
                    toggle_floating_window(&window_for_primary);
                },
            );

            let hotkey = match registration_result {
                Ok(_) => {
                    info!(
                        "✅ Successfully registered global hotkey: {}",
                        hotkey_str
                    );
                    println!(
                        "✅ Successfully registered global hotkey: {}",
                        hotkey_str
                    );
                    println!("   Press {} to toggle the floating window", hotkey_str);
                    hotkey_str
                }
                Err(e) => {
                    warn!("❌ Failed to register {}: {}", hotkey_str, e);
                    eprintln!("❌ Failed to register {}: {}", hotkey_str, e);
                    eprintln!("   Trying Alt+K instead...");

                    let window_for_fallback = floating_window.clone();
                    match app.global_shortcut().on_shortcut(
                        "Alt+K",
                        move |_app, _shortcut, event| {
                            if event.state != ShortcutState::Pressed {
                                return;
                            }
                            println!(
                                "🔥 HOTKEY TRIGGERED (Alt+K): {}",
                                chrono::Local::now().format("%H:%M:%S%.3f")
                            );
                            info!("Hotkey triggered (Alt+K)");
                            toggle_floating_window(&window_for_fallback);
                        },
                    ) {
                        Ok(_) => {
                            info!("✅ Successfully registered fallback hotkey: Alt+K");
                            println!("✅ Successfully registered fallback hotkey: Alt+K");
                            "Alt+K".to_string()
                        }
                        Err(e2) => {
                            error!("❌ Failed to register fallback hotkey: {}", e2);
                            eprintln!("❌ Failed to register any hotkey: {}", e2);
                            "None".to_string()
                        }
                    }
                }
            };

            info!("Active hotkey: {}", hotkey);
            println!("=== SETUP COMPLETE ===");
            println!("Active hotkey: {}", hotkey);
            println!("Floating window initial state: hidden");
            println!();

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            send_message_streaming,
            check_connection,
            open_chat_with_message,
            get_config,
            save_config,
            open_settings_window,
            reconnect_acp,
            handle_floating_input,
            launch_app_by_name,
            open_url,
            open_path,
            execute_shortcut,
            test_floating_window,
            start_drag_window,
            open_chat_window,
            resize_floating_window,
            send_permission_response,
            remove_tool_permission,
            update_tool_policy,
            is_dev_mode,
            open_devtools,
            read_clipboard,
            show_context_menu
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    info!("Application shutting down");
}
