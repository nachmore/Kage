mod acp_client;

use acp_client::AcpClient;
use std::sync::Arc;
use tauri::{async_runtime, State, Window};
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
    
    tauri::Builder::default()
        .manage(AppState {
            acp_client: Arc::new(Mutex::new(acp_client)),
        })
        .invoke_handler(tauri::generate_handler![send_message_streaming, check_connection])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}