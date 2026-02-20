mod acp_client;
mod app_launcher;
mod config;
mod logger;
mod process_manager;

use acp_client::AcpClient;
use app_launcher::AppLauncher;
use config::Config;
use log::{error, info, warn};
use process_manager::ProcessManager;
use std::sync::Arc;
use tauri::{
    async_runtime, CustomMenuItem, GlobalShortcutManager, Manager, State, SystemTray,
    SystemTrayEvent, SystemTrayMenu, SystemTrayMenuItem, Window,
};
use tokio::sync::Mutex;

/// Check if input is a URL
fn is_url(input: &str) -> bool {
    let trimmed = input.trim();
    // Check for common URL patterns
    trimmed.starts_with("http://") 
        || trimmed.starts_with("https://") 
        || trimmed.starts_with("ftp://")
        || trimmed.starts_with("file://")
        // Also match common patterns like www.example.com
        || (trimmed.starts_with("www.") && trimmed.contains('.'))
}

/// Check if input is a file or folder path
fn is_path(input: &str) -> Option<String> {
    let trimmed = input.trim();
    
    // Windows paths
    if cfg!(target_os = "windows") {
        // Absolute paths: C:\, D:\, \\network\share
        if trimmed.len() >= 3 && trimmed.chars().nth(1) == Some(':') && trimmed.chars().nth(2) == Some('\\') {
            return Some(trimmed.to_string());
        }
        // UNC paths
        if trimmed.starts_with("\\\\") {
            return Some(trimmed.to_string());
        }
        // Relative paths with backslash
        if trimmed.contains('\\') {
            return Some(trimmed.to_string());
        }
    }
    
    // Unix-like paths (Linux, macOS)
    if cfg!(target_os = "linux") || cfg!(target_os = "macos") {
        // Absolute paths starting with /
        if trimmed.starts_with('/') {
            return Some(trimmed.to_string());
        }
        // Home directory paths
        if trimmed.starts_with('~') {
            return Some(trimmed.to_string());
        }
        // Relative paths with forward slash
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
    
    // Pattern 1: Check if input is a URL
    if is_url(trimmed_input) {
        info!("Detected URL pattern: {}", trimmed_input);
        return Ok(format!("url:{}", trimmed_input));
    }
    
    // Pattern 2: Check if input is a file/folder path
    if let Some(path) = is_path(trimmed_input) {
        info!("Detected path pattern: {}", path);
        // Determine if it's likely a file or folder
        let is_file = path.contains('.') && !path.ends_with('\\') && !path.ends_with('/');
        return Ok(format!("path:{}:{}", if is_file { "file" } else { "folder" }, path));
    }
    
    // Pattern 3: Check if input matches an application
    let launcher = state.app_launcher.lock().await;
    let matches = launcher.find_app(trimmed_input);
    
    if !matches.is_empty() {
        info!("Found {} matching application(s)", matches.len());
        
        // Serialize matches to JSON
        let json = serde_json::to_string(&matches).map_err(|e| e.to_string())?;
        
        // Return matches for display, don't auto-launch
        if matches.len() == 1 {
            return Ok(format!("launched:{}", json));
        } else {
            return Ok(format!("multiple:{}", json));
        }
    }
    
    // No pattern match - open chat mode
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
        
        // Hide floating window
        if let Some(floating_window) = app.get_window("floating") {
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
}

#[tauri::command]
async fn open_url(url: String, app: tauri::AppHandle) -> Result<(), String> {
    info!("Opening URL: {}", url);
    
    // Ensure URL has a protocol
    let full_url = if url.starts_with("www.") {
        format!("https://{}", url)
    } else {
        url.clone()
    };
    
    // Use the OS default browser to open the URL
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(&["/C", "start", &full_url])
            .spawn()
            .map_err(|e| format!("Failed to open URL: {}", e))?;
    }
    
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&full_url)
            .spawn()
            .map_err(|e| format!("Failed to open URL: {}", e))?;
    }
    
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&full_url)
            .spawn()
            .map_err(|e| format!("Failed to open URL: {}", e))?;
    }
    
    // Hide floating window
    if let Some(floating_window) = app.get_window("floating") {
        let _ = floating_window.hide();
    }
    
    Ok(())
}

#[tauri::command]
async fn open_path(path: String, app: tauri::AppHandle) -> Result<(), String> {
    info!("Opening path: {}", path);
    
    // Expand home directory if needed
    let expanded_path = if path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            path.replacen('~', &home.to_string_lossy(), 1)
        } else {
            path.clone()
        }
    } else {
        path.clone()
    };
    
    // Check if path exists
    let path_obj = std::path::Path::new(&expanded_path);
    if !path_obj.exists() {
        return Err(format!("Path does not exist: {}", expanded_path));
    }
    
    // Open with OS default file explorer/application
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&expanded_path)
            .spawn()
            .map_err(|e| format!("Failed to open path: {}", e))?;
    }
    
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&expanded_path)
            .spawn()
            .map_err(|e| format!("Failed to open path: {}", e))?;
    }
    
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&expanded_path)
            .spawn()
            .map_err(|e| format!("Failed to open path: {}", e))?;
    }
    
    // Hide floating window
    if let Some(floating_window) = app.get_window("floating") {
        let _ = floating_window.hide();
    }
    
    Ok(())
}

#[tauri::command]
async fn send_message_streaming(
    message: String,
    state: State<'_, AppState>,
    window: Window,
) -> Result<(), String> {
    info!("Sending message: {}", message);
    let client = state.acp_client.clone();
    
    // Spawn a blocking task to handle the streaming
    async_runtime::spawn_blocking(move || {
        let client = async_runtime::block_on(client.lock());
        
        // Try to connect if not connected
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
        
        // Send the message and stream the response
        if let Err(e) = client.send_chat_streaming(message, |chunk| {
            // Emit each chunk to the frontend
            let _ = window.emit("message_chunk", chunk);
        }) {
            error!("Send error: {}", e);
            let error_msg = format!(
                "Failed to send message. The connection may have been lost.\n\nError: {}",
                e
            );
            let _ = window.emit("message_error", error_msg);
            return;
        }
        
        // Emit completion event
        let _ = window.emit("message_complete", ());
    });
    
    Ok(())
}

#[tauri::command]
async fn check_connection(state: State<'_, AppState>) -> Result<bool, String> {
    let client = state.acp_client.lock().await;
    let is_connected = client.is_connected();
    info!("Connection check: {}", if is_connected { "connected" } else { "disconnected" });
    Ok(is_connected)
}

#[tauri::command]
async fn open_chat_with_message(
    message: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    info!("Opening chat with message: {}", message);
    
    // Hide floating window
    if let Some(floating_window) = app.get_window("floating") {
        let _ = floating_window.hide();
    }
    
    // Show main chat window
    if let Some(main_window) = app.get_window("main") {
        let _ = main_window.show();
        let _ = main_window.set_focus();
        
        // Send the message to the chat window
        let _ = main_window.emit("initial_message", message.clone());
        
        // Also send it to the ACP client
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
            
            if let Err(e) = client.send_chat_streaming(message, |chunk| {
                let _ = window.emit("message_chunk", chunk);
            }) {
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
async fn save_config(config: Config, state: State<'_, AppState>) -> Result<(), String> {
    info!("Saving configuration");
    config.save().map_err(|e| {
        error!("Failed to save config: {}", e);
        format!("Failed to save configuration: {}", e)
    })?;
    
    let mut state_config = state.config.lock().await;
    *state_config = config;
    
    info!("Configuration saved successfully");
    Ok(())
}

#[tauri::command]
async fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    info!("Opening settings window");
    if let Some(window) = app.get_window("settings") {
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
    
    if let Some(window) = app.get_window("floating") {
        let is_visible = window.is_visible().unwrap_or(false);
        println!("   Current state: {}", if is_visible { "VISIBLE" } else { "HIDDEN" });
        
        if is_visible {
            println!("   Action: Hiding window");
            window.hide().map_err(|e| format!("Failed to hide: {}", e))?;
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
            
            // Position at 1/3 from top
            if let Ok(monitor) = window.current_monitor() {
                if let Some(monitor) = monitor {
                    let size = monitor.size();
                    println!("   Monitor size: {}x{}", size.width, size.height);
                    let x = (size.width as i32 - 500) / 2; // 500px window width
                    let y = size.height as i32 / 3; // 1/3 from top
                    println!("   Positioning at: ({}, {})", x, y);
                    window.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }))
                        .map_err(|e| {
                            println!("   ⚠️  Failed to position: {}", e);
                            format!("Failed to position: {}", e)
                        })?;
                    println!("   ✅ Window positioned");
                }
            }
            
            Ok("Window was hidden, now visible and positioned".to_string())
        }
    } else {
        println!("   ❌ Floating window not found!");
        Err("Floating window not found".to_string())
    }
}

#[tauri::command]
async fn start_drag_window(window: Window) -> Result<(), String> {
    info!("Starting window drag");
    window.start_dragging().map_err(|e| {
        error!("Failed to start dragging: {}", e);
        e.to_string()
    })
}

#[tauri::command]
async fn open_chat_window(app: tauri::AppHandle) -> Result<(), String> {
    info!("Opening chat window");
    
    // Hide floating window
    if let Some(floating_window) = app.get_window("floating") {
        let _ = floating_window.hide();
    }
    
    // Get or show main chat window
    if let Some(window) = app.get_window("main") {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    } else {
        // Create the main window if it doesn't exist (shouldn't happen normally)
        warn!("Main window not found, this shouldn't happen");
    }
    
    Ok(())
}

#[tauri::command]
async fn resize_floating_window(window: Window, width: u32, height: u32) -> Result<(), String> {
    info!("Resizing floating window to {}x{}", width, height);
    
    // Get current size
    let current_size = window.inner_size().map_err(|e| {
        error!("Failed to get current window size: {}", e);
        e.to_string()
    })?;
    
    let current_height = current_size.height;
    let target_height = height;
    
    // If the height difference is small, just resize directly
    if (current_height as i32 - target_height as i32).abs() < 20 {
        return window.set_size(tauri::Size::Physical(tauri::PhysicalSize { width, height }))
            .map_err(|e| {
                error!("Failed to resize window: {}", e);
                e.to_string()
            });
    }
    
    // Animate the resize for larger changes
    let steps = 10;
    let height_diff = target_height as i32 - current_height as i32;
    let step_size = height_diff as f32 / steps as f32;
    
    for i in 1..=steps {
        let new_height = (current_height as f32 + step_size * i as f32) as u32;
        
        if let Err(e) = window.set_size(tauri::Size::Physical(tauri::PhysicalSize { 
            width, 
            height: new_height 
        })) {
            error!("Failed to resize window during animation: {}", e);
            // Continue anyway, don't fail the whole operation
        }
        
        // Small delay between steps for smooth animation
        tokio::time::sleep(tokio::time::Duration::from_millis(15)).await;
    }
    
    // Ensure we end at exactly the target size
    window.set_size(tauri::Size::Physical(tauri::PhysicalSize { width, height }))
        .map_err(|e| {
            error!("Failed to resize window: {}", e);
            e.to_string()
        })
}

fn main() {
    // Initialize logger first
    if let Err(e) = logger::init_logger() {
        eprintln!("Failed to initialize logger: {}", e);
        eprintln!("Continuing without file logging...");
    }
    
    info!("=== Kiro Assistant Starting ===");
    
    // Clean up any orphaned processes from previous runs
    info!("Checking for orphaned processes...");
    if let Err(e) = ProcessManager::cleanup_orphaned_processes() {
        warn!("Failed to cleanup orphaned processes: {}", e);
    }
    
    // Load configuration
    let config = Config::load().unwrap_or_else(|e| {
        error!("Failed to load config, using defaults: {}", e);
        eprintln!("Failed to load config, using defaults: {}", e);
        Config::default()
    });
    
    info!("Configuration loaded");
    
    let acp_client = match &config.acp.mode {
        crate::config::AcpMode::Local { spawn_command } => {
            info!("ACP Mode: Local with spawn command: {}", spawn_command);
            AcpClient::new(acp_client::AcpConnectionMode::Local {
                spawn_command: spawn_command.clone(),
            })
        }
        crate::config::AcpMode::Remote { host, port, timeout_ms } => {
            info!("ACP Mode: Remote at {}:{} (timeout: {}ms)", host, port, timeout_ms);
            AcpClient::new(acp_client::AcpConnectionMode::Remote {
                host: host.clone(),
                port: *port,
            })
        }
    };
    
    // Install signal handlers for graceful shutdown
    let process_manager = acp_client.get_process_manager();
    process_manager::install_signal_handlers(process_manager);
    
    // Initialize app launcher
    let app_launcher = AppLauncher::new().unwrap_or_else(|e| {
        error!("Failed to initialize app launcher: {}", e);
        eprintln!("Failed to initialize app launcher: {}", e);
        // Create an empty launcher as fallback
        AppLauncher::new().unwrap()
    });
    info!("App launcher initialized");
    
    // Create system tray menu
    let show = CustomMenuItem::new("show".to_string(), "Show");
    let settings = CustomMenuItem::new("settings".to_string(), "Settings");
    let quit = CustomMenuItem::new("quit".to_string(), "Quit");
    let tray_menu = SystemTrayMenu::new()
        .add_item(show)
        .add_item(settings)
        .add_native_item(SystemTrayMenuItem::Separator)
        .add_item(quit);
    
    let system_tray = SystemTray::new().with_menu(tray_menu);
    
    tauri::Builder::default()
        .manage(AppState {
            acp_client: Arc::new(Mutex::new(acp_client)),
            config: Arc::new(Mutex::new(config.clone())),
            app_launcher: Arc::new(Mutex::new(app_launcher)),
        })
        .system_tray(system_tray)
        .on_system_tray_event(|app, event| match event {
            SystemTrayEvent::MenuItemClick { id, .. } => {
                info!("System tray menu item clicked: {}", id);
                match id.as_str() {
                    "show" => {
                        if let Some(window) = app.get_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "settings" => {
                        if let Some(window) = app.get_window("settings") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        info!("Application quit requested");
                        
                        // Get the ACP client and disconnect (which will cleanup the process)
                        if let Some(state) = app.try_state::<AppState>() {
                            if let Ok(client) = state.acp_client.try_lock() {
                                client.disconnect();
                            }
                        }
                        
                        std::process::exit(0);
                    }
                    _ => {}
                }
            }
            SystemTrayEvent::LeftClick { .. } => {
                info!("System tray left clicked");
                if let Some(window) = app.get_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            _ => {}
        })
        .on_window_event(|event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event.event() {
                // Prevent the window from closing, hide it instead
                event.window().hide().unwrap();
                api.prevent_close();
            }
        })
        .setup(move |app| {
            info!("Setting up application");
            println!("=== KIRO ASSISTANT SETUP ===");
            
            // Register global hotkey from config
            let mut shortcut_manager = app.global_shortcut_manager();
            let floating_window = app.get_window("floating").unwrap();
            
            // Get hotkey from config
            let hotkey_string = config.get_hotkey_string();
            
            info!("Attempting to register global hotkey: {}", hotkey_string);
            println!("Attempting to register global hotkey: {}", hotkey_string);
            
            // Clone window for closures
            let window_for_primary = floating_window.clone();
            let window_for_fallback = floating_window.clone();
            
            // Try to register the configured hotkey
            let registration_result = shortcut_manager.register(&hotkey_string, move || {
                println!("🔥 HOTKEY TRIGGERED: {}", chrono::Local::now().format("%H:%M:%S%.3f"));
                info!("Hotkey triggered");
                
                match window_for_primary.is_visible() {
                    Ok(is_visible) => {
                        println!("   Window visible state: {}", is_visible);
                        if is_visible {
                            println!("  → Hiding floating window");
                            match window_for_primary.hide() {
                                Ok(_) => println!("     ✅ Window hidden successfully"),
                                Err(e) => println!("     ❌ Failed to hide: {}", e),
                            }
                        } else {
                            println!("  → Showing floating window");
                            match window_for_primary.show() {
                                Ok(_) => {
                                    println!("     ✅ Window shown successfully");
                                    match window_for_primary.set_focus() {
                                        Ok(_) => println!("     ✅ Window focused successfully"),
                                        Err(e) => println!("     ⚠️  Failed to focus: {}", e),
                                    }
                                    // Position at 1/3 from top
                                    if let Ok(monitor) = window_for_primary.current_monitor() {
                                        if let Some(monitor) = monitor {
                                            let size = monitor.size();
                                            println!("     Monitor size: {}x{}", size.width, size.height);
                                            let x = (size.width as i32 - 500) / 2; // 500px window width
                                            let y = size.height as i32 / 3; // 1/3 from top
                                            println!("     Positioning at: ({}, {})", x, y);
                                            if let Err(e) = window_for_primary.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y })) {
                                                println!("     ⚠️  Failed to position: {}", e);
                                            }
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
            });
            
            let hotkey = match registration_result {
                Ok(_) => {
                    info!("✅ Successfully registered global hotkey: {}", hotkey_string);
                    println!("✅ Successfully registered global hotkey: {}", hotkey_string);
                    println!("   Press {} to toggle the floating window", hotkey_string);
                    hotkey_string
                }
                Err(e) => {
                    warn!("❌ Failed to register {}: {}", hotkey_string, e);
                    eprintln!("❌ Failed to register {}: {}", hotkey_string, e);
                    eprintln!("   Trying Alt+K instead...");
                    
                    match shortcut_manager.register("Alt+K", move || {
                        println!("🔥 HOTKEY TRIGGERED (Alt+K): {}", chrono::Local::now().format("%H:%M:%S%.3f"));
                        info!("Hotkey triggered (Alt+K)");
                        
                        match window_for_fallback.is_visible() {
                            Ok(is_visible) => {
                                println!("   Window visible state: {}", is_visible);
                                if is_visible {
                                    println!("  → Hiding floating window");
                                    match window_for_fallback.hide() {
                                        Ok(_) => println!("     ✅ Window hidden successfully"),
                                        Err(e) => println!("     ❌ Failed to hide: {}", e),
                                    }
                                } else {
                                    println!("  → Showing floating window");
                                    match window_for_fallback.show() {
                                        Ok(_) => {
                                            println!("     ✅ Window shown successfully");
                                            match window_for_fallback.set_focus() {
                                                Ok(_) => println!("     ✅ Window focused successfully"),
                                                Err(e) => println!("     ⚠️  Failed to focus: {}", e),
                                            }
                                            // Position at 1/3 from top
                                            if let Ok(monitor) = window_for_fallback.current_monitor() {
                                                if let Some(monitor) = monitor {
                                                    let size = monitor.size();
                                                    println!("     Monitor size: {}x{}", size.width, size.height);
                                                    let x = (size.width as i32 - 500) / 2; // 500px window width
                                                    let y = size.height as i32 / 3; // 1/3 from top
                                                    println!("     Positioning at: ({}, {})", x, y);
                                                    if let Err(e) = window_for_fallback.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y })) {
                                                        println!("     ⚠️  Failed to position: {}", e);
                                                    }
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
                    }) {
                        Ok(_) => {
                            info!("✅ Successfully registered fallback hotkey: Alt+K");
                            println!("✅ Successfully registered fallback hotkey: Alt+K");
                            println!("   Press Alt+K to toggle the floating window");
                            "Alt+K".to_string()
                        }
                        Err(e) => {
                            error!("❌ Failed to register global hotkey: {}", e);
                            eprintln!("❌ Failed to register global hotkey: {}", e);
                            eprintln!("   You can still use the system tray to show/hide the window.");
                            "None".to_string()
                        }
                    }
                }
            };
            
            info!("Active hotkey: {}", hotkey);
            println!("=== SETUP COMPLETE ===");
            println!("Active hotkey: {}", hotkey);
            println!("Floating window initial state: hidden");
            println!("");
            
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
            test_floating_window,
            start_drag_window,
            open_chat_window,
            resize_floating_window
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
    
    info!("Application shutting down");
}