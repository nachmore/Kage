mod acp_client;

use acp_client::AcpClient;
use std::sync::Arc;
use tauri::{
    async_runtime, CustomMenuItem, GlobalShortcutManager, Manager, State, SystemTray,
    SystemTrayEvent, SystemTrayMenu, SystemTrayMenuItem, Window,
};
use tokio::sync::Mutex;

struct AppState {
    acp_client: Arc<Mutex<AcpClient>>,
}

#[tauri::command]
async fn send_message_streaming(
    message: String,
    state: State<'_, AppState>,
    window: Window,
) -> Result<(), String> {
    let client = state.acp_client.clone();
    
    // Spawn a blocking task to handle the streaming
    async_runtime::spawn_blocking(move || {
        let client = async_runtime::block_on(client.lock());
        
        // Try to connect if not connected
        if !client.is_connected() {
            if let Err(e) = client.connect() {
                let _ = window.emit("message_error", format!("Connection error: {}", e));
                return;
            }
        }
        
        // Send the message and stream the response
        if let Err(e) = client.send_chat_streaming(message, |chunk| {
            // Emit each chunk to the frontend
            let _ = window.emit("message_chunk", chunk);
        }) {
            let _ = window.emit("message_error", format!("Send error: {}", e));
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
    Ok(client.is_connected())
}

fn main() {
    let acp_client = AcpClient::new("127.0.0.1".to_string(), 8765);
    
    // Create system tray menu
    let show = CustomMenuItem::new("show".to_string(), "Show");
    let quit = CustomMenuItem::new("quit".to_string(), "Quit");
    let tray_menu = SystemTrayMenu::new()
        .add_item(show)
        .add_native_item(SystemTrayMenuItem::Separator)
        .add_item(quit);
    
    let system_tray = SystemTray::new().with_menu(tray_menu);
    
    tauri::Builder::default()
        .manage(AppState {
            acp_client: Arc::new(Mutex::new(acp_client)),
        })
        .system_tray(system_tray)
        .on_system_tray_event(|app, event| match event {
            SystemTrayEvent::MenuItemClick { id, .. } => match id.as_str() {
                "show" => {
                    if let Some(window) = app.get_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                "quit" => {
                    std::process::exit(0);
                }
                _ => {}
            },
            SystemTrayEvent::LeftClick { .. } => {
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
        .setup(|app| {
            // Register global hotkey Alt+K (Alt+Space may be in use on some systems)
            let mut shortcut_manager = app.global_shortcut_manager();
            let window = app.get_window("main").unwrap();
            
            // Try Alt+Space first, fall back to Alt+K if it fails
            let hotkey = if shortcut_manager.register("Alt+Space", {
                let window = window.clone();
                move || {
                    if window.is_visible().unwrap_or(false) {
                        let _ = window.hide();
                    } else {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }).is_ok() {
                println!("Successfully registered global hotkey Alt+Space");
                "Alt+Space"
            } else {
                eprintln!("Failed to register Alt+Space, trying Alt+K instead...");
                match shortcut_manager.register("Alt+K", move || {
                    if window.is_visible().unwrap_or(false) {
                        let _ = window.hide();
                    } else {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }) {
                    Ok(_) => {
                        println!("Successfully registered global hotkey Alt+K");
                        "Alt+K"
                    }
                    Err(e) => {
                        eprintln!("Failed to register global hotkey: {}", e);
                        eprintln!("You can still use the system tray to show/hide the window.");
                        "None"
                    }
                }
            };
            
            println!("Active hotkey: {}", hotkey);
            
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![send_message_streaming, check_connection])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}