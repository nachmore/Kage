use anyhow::{Context, Result};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::thread;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRequest {
    pub jsonrpc: String,
    pub id: String,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpResponse {
    pub jsonrpc: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AcpError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

pub struct AcpClient {
    host: String,
    port: u16,
    connection: Arc<Mutex<Option<TcpStream>>>,
    max_retries: u32,
    initial_retry_delay_ms: u64,
}

impl AcpClient {
    pub fn new(host: String, port: u16) -> Self {
        Self {
            host,
            port,
            connection: Arc::new(Mutex::new(None)),
            max_retries: 5,
            initial_retry_delay_ms: 100,
        }
    }

    pub fn connect(&self) -> Result<()> {
        self.connect_with_retry(0)
    }
    
    fn connect_with_retry(&self, attempt: u32) -> Result<()> {
        let addr = format!("{}:{}", self.host, self.port);
        
        info!("Attempting to connect to kiro-cli at {} (attempt {}/{})", 
              addr, attempt + 1, self.max_retries + 1);
        
        match TcpStream::connect_timeout(
            &addr.parse().context("Invalid address")?,
            Duration::from_secs(5),
        ) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(30)))
                    .context("Failed to set read timeout")?;
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .context("Failed to set write timeout")?;

                let mut conn = self.connection.lock().unwrap();
                *conn = Some(stream);
                
                info!("Successfully connected to kiro-cli at {}", addr);
                Ok(())
            }
            Err(e) => {
                warn!("Connection attempt {} failed: {}", attempt + 1, e);
                
                if attempt < self.max_retries {
                    // Exponential backoff: 100ms, 200ms, 400ms, 800ms, 1600ms
                    let delay_ms = self.initial_retry_delay_ms * 2_u64.pow(attempt);
                    let delay_ms = delay_ms.min(30000); // Cap at 30 seconds
                    
                    info!("Retrying in {}ms...", delay_ms);
                    thread::sleep(Duration::from_millis(delay_ms));
                    
                    self.connect_with_retry(attempt + 1)
                } else {
                    error!("Failed to connect to kiro-cli after {} attempts", self.max_retries + 1);
                    Err(e).context(format!(
                        "Failed to connect to kiro-cli at {} after {} attempts. Please ensure kiro-cli is running.",
                        addr, self.max_retries + 1
                    ))
                }
            }
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connection.lock().unwrap().is_some()
    }

    #[allow(dead_code)]
    pub fn send_message(&self, message: AcpRequest) -> Result<AcpResponse> {
        let mut conn_guard = self.connection.lock().unwrap();
        let stream = conn_guard
            .as_mut()
            .context("Not connected to kiro-cli")?;

        info!("Sending ACP message: method={}, id={}", message.method, message.id);

        // Serialize and send the request
        let request_json = serde_json::to_string(&message)?;
        match writeln!(stream, "{}", request_json) {
            Ok(_) => {}
            Err(e) => {
                error!("Failed to send message: {}", e);
                // Connection lost, clear it
                *conn_guard = None;
                return Err(e).context("Failed to send message - connection lost");
            }
        }
        
        if let Err(e) = stream.flush() {
            error!("Failed to flush stream: {}", e);
            *conn_guard = None;
            return Err(e).context("Failed to flush stream - connection lost");
        }

        // Read the response
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut response_line = String::new();
        match reader.read_line(&mut response_line) {
            Ok(_) => {}
            Err(e) => {
                error!("Failed to read response: {}", e);
                *conn_guard = None;
                return Err(e).context("Failed to read response - connection lost");
            }
        }

        // Parse the response
        let response: AcpResponse =
            serde_json::from_str(&response_line).context("Failed to parse response")?;

        if let Some(ref error) = response.error {
            warn!("ACP error response: {} (code: {})", error.message, error.code);
        } else {
            info!("Received ACP response: id={}", response.id);
        }

        Ok(response)
    }

    pub fn send_chat_streaming<F>(&self, content: String, mut callback: F) -> Result<()>
    where
        F: FnMut(String),
    {
        let request = AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            method: "chat".to_string(),
            params: serde_json::json!({
                "message": content
            }),
        };

        info!("Sending chat message: id={}", request.id);

        let mut conn_guard = self.connection.lock().unwrap();
        let stream = conn_guard
            .as_mut()
            .context("Not connected to kiro-cli")?;

        // Serialize and send the request
        let request_json = serde_json::to_string(&request)?;
        match writeln!(stream, "{}", request_json) {
            Ok(_) => {}
            Err(e) => {
                error!("Failed to send chat message: {}", e);
                *conn_guard = None;
                return Err(e).context("Failed to send message - connection lost");
            }
        }
        
        if let Err(e) = stream.flush() {
            error!("Failed to flush stream: {}", e);
            *conn_guard = None;
            return Err(e).context("Failed to flush stream - connection lost");
        }

        // Read streaming responses
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut full_response = String::new();
        
        loop {
            let mut response_line = String::new();
            match reader.read_line(&mut response_line) {
                Ok(_) => {}
                Err(e) => {
                    error!("Failed to read streaming response: {}", e);
                    *conn_guard = None;
                    return Err(e).context("Failed to read response - connection lost");
                }
            }

            if response_line.trim().is_empty() {
                break;
            }

            // Parse the response
            let response: AcpResponse = match serde_json::from_str(&response_line) {
                Ok(r) => r,
                Err(e) => {
                    error!("Failed to parse streaming response: {}", e);
                    return Err(e).context("Failed to parse response");
                }
            };

            if let Some(error) = response.error {
                error!("ACP error in streaming response: {} (code: {})", error.message, error.code);
                anyhow::bail!("ACP error: {} (code: {})", error.message, error.code);
            }

            if let Some(result) = response.result {
                if let Some(content) = result.get("content").and_then(|v| v.as_str()) {
                    full_response.push_str(content);
                    callback(full_response.clone());
                }
                
                // Check if this is the final message
                if let Some(done) = result.get("done").and_then(|v| v.as_bool()) {
                    if done {
                        info!("Chat streaming completed: id={}", response.id);
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub fn disconnect(&self) {
        info!("Disconnecting from kiro-cli");
        let mut conn = self.connection.lock().unwrap();
        *conn = None;
    }
}
