mod acp_client;
mod config;
mod logger;

use acp_client::AcpClient;
use config::Config;
use log::{error, info, warn};
use std::sync::Arc;
use tauri::{
    async_runtime, CustomMenuItem, GlobalShortcutManager, Manager, State, SystemTray,
    SystemTrayEvent, SystemTrayMenu, SystemTrayMenuItem, Window,
};
use tokio::sync::Mutex;

struct AppState {
    acp_client: Arc<Mutex<AcpClient>>,
    config: Arc<Mutex<Config>>,
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

fn main() {
    // Initialize logger first
    if let Err(e) = logger::init_logger() {
        eprintln!("Failed to initialize logger: {}", e);
        eprintln!("Continuing without file logging...");
    }
    
    info!("=== Kiro Assistant Starting ===");
    
    // Load configuration
    let config = Config::load().unwrap_or_else(|e| {
        error!("Failed to load config, using defaults: {}", e);
        eprintln!("Failed to load config, using defaults: {}", e);
        Config::default()
    });
    
    info!("Configuration loaded: ACP host={}:{}", config.acp.host, config.acp.port);
    
    let acp_client = AcpClient::new(config.acp.host.clone(), config.acp.port);
    
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
            
            // Register global hotkey from config
            let mut shortcut_manager = app.global_shortcut_manager();
            let floating_window = app.get_window("floating").unwrap();
            
            // Get hotkey from config
            let hotkey_string = config.get_hotkey_string();
            
            info!("Attempting to register global hotkey: {}", hotkey_string);
            
            // Try to register the configured hotkey
            let hotkey = if shortcut_manager.register(&hotkey_string, {
                let window = floating_window.clone();
                move || {
                    if window.is_visible().unwrap_or(false) {
                        let _ = window.hide();
                    } else {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }).is_ok() {
                info!("Successfully registered global hotkey: {}", hotkey_string);
                println!("Successfully registered global hotkey {}", hotkey_string);
                hotkey_string
            } else {
                warn!("Failed to register {}, trying Alt+K instead...", hotkey_string);
                eprintln!("Failed to register {}, trying Alt+K instead...", hotkey_string);
                match shortcut_manager.register("Alt+K", move || {
                    if floating_window.is_visible().unwrap_or(false) {
                        let _ = floating_window.hide();
                    } else {
                        let _ = floating_window.show();
                        let _ = floating_window.set_focus();
                    }
                }) {
                    Ok(_) => {
                        info!("Successfully registered fallback hotkey: Alt+K");
                        println!("Successfully registered global hotkey Alt+K");
                        "Alt+K".to_string()
                    }
                    Err(e) => {
                        error!("Failed to register global hotkey: {}", e);
                        eprintln!("Failed to register global hotkey: {}", e);
                        eprintln!("You can still use the system tray to show/hide the window.");
                        "None".to_string()
                    }
                }
            };
            
            info!("Active hotkey: {}", hotkey);
            println!("Active hotkey: {}", hotkey);
            
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            send_message_streaming, 
            check_connection, 
            open_chat_with_message,
            get_config,
            save_config,
            open_settings_window,
            reconnect_acp
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
    
    info!("Application shutting down");
}