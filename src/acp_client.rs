use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
}

impl AcpClient {
    pub fn new(host: String, port: u16) -> Self {
        Self {
            host,
            port,
            connection: Arc::new(Mutex::new(None)),
        }
    }

    pub fn connect(&self) -> Result<()> {
        let addr = format!("{}:{}", self.host, self.port);
        let stream = TcpStream::connect_timeout(
            &addr.parse().context("Invalid address")?,
            Duration::from_secs(5),
        )
        .context("Failed to connect to kiro-cli")?;

        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .context("Failed to set read timeout")?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .context("Failed to set write timeout")?;

        let mut conn = self.connection.lock().unwrap();
        *conn = Some(stream);
        Ok(())
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

        // Serialize and send the request
        let request_json = serde_json::to_string(&message)?;
        writeln!(stream, "{}", request_json).context("Failed to send message")?;
        stream.flush().context("Failed to flush stream")?;

        // Read the response
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut response_line = String::new();
        reader
            .read_line(&mut response_line)
            .context("Failed to read response")?;

        // Parse the response
        let response: AcpResponse =
            serde_json::from_str(&response_line).context("Failed to parse response")?;

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

        let mut conn_guard = self.connection.lock().unwrap();
        let stream = conn_guard
            .as_mut()
            .context("Not connected to kiro-cli")?;

        // Serialize and send the request
        let request_json = serde_json::to_string(&request)?;
        writeln!(stream, "{}", request_json).context("Failed to send message")?;
        stream.flush().context("Failed to flush stream")?;

        // Read streaming responses
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut full_response = String::new();
        
        loop {
            let mut response_line = String::new();
            reader
                .read_line(&mut response_line)
                .context("Failed to read response")?;

            if response_line.trim().is_empty() {
                break;
            }

            // Parse the response
            let response: AcpResponse =
                serde_json::from_str(&response_line).context("Failed to parse response")?;

            if let Some(error) = response.error {
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
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub fn disconnect(&self) {
        let mut conn = self.connection.lock().unwrap();
        *conn = None;
    }
}
