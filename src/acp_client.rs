use anyhow::{Context, Result};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::thread;

use crate::os;
use crate::process_manager::ProcessManager;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AcpError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

pub enum AcpConnectionMode {
    Local {
        spawn_command: String,
    },
    Remote {
        host: String,
        port: u16,
    },
}

enum Connection {
    Tcp(TcpStream),
    Pipe {
        stdin: Arc<Mutex<ChildStdin>>,
        stdout: BufReader<ChildStdout>,
    },
}

pub struct AcpClient {
    mode: AcpConnectionMode,
    connection: Arc<Mutex<Option<Connection>>>,
    /// Separate write handle for pipe stdin, accessible without the connection lock.
    /// This lets send_permission_response write while the streaming loop reads.
    pipe_stdin: Arc<Mutex<Option<Arc<Mutex<ChildStdin>>>>>,
    /// Cloned TcpStream for writing, accessible without the connection lock.
    tcp_writer: Arc<Mutex<Option<TcpStream>>>,
    max_retries: u32,
    initial_retry_delay_ms: u64,
    process_manager: Arc<Mutex<ProcessManager>>,
    session_id: Arc<Mutex<Option<String>>>,
    initialized: Arc<Mutex<bool>>,
    debug_mode: Arc<Mutex<bool>>,
}

impl AcpClient {
    pub fn new(mode: AcpConnectionMode) -> Self {
        Self {
            mode,
            connection: Arc::new(Mutex::new(None)),
            pipe_stdin: Arc::new(Mutex::new(None)),
            tcp_writer: Arc::new(Mutex::new(None)),
            max_retries: 5,
            initial_retry_delay_ms: 100,
            process_manager: Arc::new(Mutex::new(ProcessManager::new())),
            session_id: Arc::new(Mutex::new(None)),
            initialized: Arc::new(Mutex::new(false)),
            debug_mode: Arc::new(Mutex::new(false)),
        }
    }
    
    /// Set debug mode for detailed ACP logging
    pub fn set_debug_mode(&self, enabled: bool) {
        let mut debug = self.debug_mode.lock().unwrap();
        *debug = enabled;
        if enabled {
            info!("Ã°Å¸Ââ€º ACP Debug mode ENABLED - detailed logging active");
        } else {
            info!("Ã°Å¸Ââ€º ACP Debug mode DISABLED");
        }
    }
    
    /// Get the process manager for signal handler registration
    pub fn get_process_manager(&self) -> Arc<Mutex<ProcessManager>> {
        self.process_manager.clone()
    }

    /// Get the pipe stdin handle for writing permission responses without holding the connection lock
    pub fn get_pipe_stdin(&self) -> Arc<Mutex<Option<Arc<Mutex<ChildStdin>>>>> {
        self.pipe_stdin.clone()
    }

    /// Get the TCP writer handle for writing permission responses without holding the connection lock
    pub fn get_tcp_writer(&self) -> Arc<Mutex<Option<TcpStream>>> {
        self.tcp_writer.clone()
    }

    /// Get the current session ID, creating one if needed
    fn spawn_kiro_process(&self, command_str: &str) -> Result<()> {
        info!("Ã°Å¸Å¡â‚¬ Spawning Kiro process with command: {}", command_str);
        
        // Parse the command string into program and arguments
        let parts: Vec<&str> = command_str.split_whitespace().collect();
        if parts.is_empty() {
            anyhow::bail!("Empty spawn command");
        }
        
        let program = parts[0];
        let args = &parts[1..];
        
        info!("Ã°Å¸â€œÂ¦ Program: {}, Args: {:?}", program, args);
        
        // Create command with piped stdin/stdout for communication
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()); // Keep stderr for debugging
        
        // Configure platform-specific process spawning
        os::configure_process_spawn(&mut cmd);
        
        info!("Ã¢ÂÂ³ Spawning process...");
        let mut child = cmd.spawn()
            .context("Failed to spawn Kiro process")?;
        
        info!("Ã¢Å“â€¦ Process spawned successfully (PID: {:?})", child.id());
        
        // Take ownership of stdin and stdout
        let stdin = child.stdin.take()
            .context("Failed to get stdin handle")?;
        let stdout = child.stdout.take()
            .context("Failed to get stdout handle")?;
        
        info!("Ã°Å¸â€œÂ¡ Pipe handles acquired");
        
        // Store the connection
        let stdin_arc = Arc::new(Mutex::new(stdin));
        let mut conn_guard = self.connection.lock().unwrap();
        *conn_guard = Some(Connection::Pipe {
            stdin: stdin_arc.clone(),
            stdout: BufReader::new(stdout),
        });
        drop(conn_guard);
        
        // Store separate stdin handle for permission responses
        let mut pipe_stdin_guard = self.pipe_stdin.lock().unwrap();
        *pipe_stdin_guard = Some(stdin_arc);
        drop(pipe_stdin_guard);
        
        // Store the process in ProcessManager for cleanup
        let mut pm = self.process_manager.lock().unwrap();
        pm.store_process(child)
            .context("Failed to register process for cleanup")?;
        drop(pm);
        
        info!("Ã¢ÂÂ±Ã¯Â¸Â  Waiting 1 second for process to initialize...");
        thread::sleep(Duration::from_millis(1000));
        
        info!("Ã°Å¸Å½â€° Kiro process ready for communication");
        Ok(())
    }

    pub fn connect(&self) -> Result<()> {
        match &self.mode {
            AcpConnectionMode::Local { ref spawn_command } => {
                info!("Ã°Å¸â€Â§ Local mode: Checking if process needs to be spawned");
                let conn_guard = self.connection.lock().unwrap();
                if conn_guard.is_none() {
                    drop(conn_guard);
                    info!("Ã°Å¸â€œÂ No existing connection, spawning process");
                    self.spawn_kiro_process(spawn_command)?;
                    info!("Ã¢Å“â€¦ Local mode ready - using pipe communication");
                } else {
                    info!("Ã¢Å“â€¦ Local mode already connected via pipes");
                }
                Ok(())
            }
            AcpConnectionMode::Remote { .. } => {
                info!("Ã°Å¸Å’Â Remote mode: Establishing TCP connection");
                self.connect_with_retry(0)
            }
        }
    }
    
    fn connect_with_retry(&self, attempt: u32) -> Result<()> {
        let (host, port) = match &self.mode {
            AcpConnectionMode::Remote { host, port } => (host.clone(), *port),
            AcpConnectionMode::Local { .. } => {
                anyhow::bail!("Cannot use TCP connection in local mode");
            }
        };
        
        let addr = format!("{}:{}", host, port);
        
        info!("Ã°Å¸â€Å’ Attempting TCP connection to {} (attempt {}/{})", 
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

                // Store a cloned stream for writing permission responses
                let write_clone = stream.try_clone().context("Failed to clone TCP stream")?;
                let mut tw = self.tcp_writer.lock().unwrap();
                *tw = Some(write_clone);
                drop(tw);

                let mut conn = self.connection.lock().unwrap();
                *conn = Some(Connection::Tcp(stream));
                
                info!("Ã¢Å“â€¦ Successfully connected to kiro-cli at {}", addr);
                Ok(())
            }
            Err(e) => {
                warn!("Ã¢ÂÅ’ Connection attempt {} failed: {}", attempt + 1, e);
                
                if attempt < self.max_retries {
                    let delay_ms = self.initial_retry_delay_ms * 2_u64.pow(attempt);
                    let delay_ms = delay_ms.min(30000);
                    
                    info!("Ã¢ÂÂ³ Retrying in {}ms...", delay_ms);
                    thread::sleep(Duration::from_millis(delay_ms));
                    
                    self.connect_with_retry(attempt + 1)
                } else {
                    error!("Ã°Å¸â€™Â¥ Failed to connect after {} attempts", self.max_retries + 1);
                    Err(e).context(format!(
                        "Failed to connect to kiro-cli at {} after {} attempts",
                        addr, self.max_retries + 1
                    ))
                }
            }
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connection.lock().unwrap().is_some()
    }

    fn send_request(&self, request: &AcpRequest) -> Result<AcpResponse> {
        let request_json = serde_json::to_string(&request)?;
        let debug_enabled = *self.debug_mode.lock().unwrap();
        
        if debug_enabled {
            info!("Ã°Å¸Ââ€º [ACP DEBUG] Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â");
            info!("Ã°Å¸Ââ€º [ACP DEBUG] Ã°Å¸â€œÂ¤ SENDING REQUEST");
            info!("Ã°Å¸Ââ€º [ACP DEBUG] Method: {}", request.method);
            info!("Ã°Å¸Ââ€º [ACP DEBUG] ID: {:?}", request.id);
            info!("Ã°Å¸Ââ€º [ACP DEBUG] Full JSON: {}", request_json);
            info!("Ã°Å¸Ââ€º [ACP DEBUG] Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â");
        } else {
            info!("Ã°Å¸â€œÂ¤ Sending request: method={}, id={:?}", request.method, request.id);
        }

        let mut conn_guard = self.connection.lock().unwrap();
        let conn = conn_guard
            .as_mut()
            .context("Not connected to ACP server")?;

        // Send based on connection type
        match conn {
            Connection::Tcp(stream) => {
                writeln!(stream, "{}", request_json)?;
                stream.flush()?;
                
                // Read response
                let mut reader = BufReader::new(stream.try_clone()?);
                let mut response_line = String::new();
                reader.read_line(&mut response_line)?;
                
                if debug_enabled {
                    info!("Ã°Å¸Ââ€º [ACP DEBUG] Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â");
                    info!("Ã°Å¸Ââ€º [ACP DEBUG] Ã°Å¸â€œÂ¥ RECEIVED RESPONSE (TCP)");
                    info!("Ã°Å¸Ââ€º [ACP DEBUG] Raw: {}", response_line.trim());
                    info!("Ã°Å¸Ââ€º [ACP DEBUG] Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â");
                } else {
                    info!("Ã°Å¸â€œÂ¥ TCP response: {}", response_line.trim());
                }
                
                serde_json::from_str(&response_line).context("Failed to parse response")
            }
            Connection::Pipe { stdin, stdout } => {
                let mut stdin_guard = stdin.lock().unwrap();
                writeln!(stdin_guard, "{}", request_json)?;
                stdin_guard.flush()?;
                drop(stdin_guard);
                
                // Read response, skipping any notifications (lines with "method" but no "id")
                loop {
                    let mut response_line = String::new();
                    stdout.read_line(&mut response_line)?;
                    
                    if debug_enabled {
                        info!("[ACP DEBUG] RECEIVED (Pipe): {}", response_line.trim());
                    } else {
                        info!("Pipe response: {}", response_line.trim());
                    }
                    
                    if response_line.trim().is_empty() {
                        continue;
                    }
                    
                    // Check if this is a notification (has "method" key but no "id")
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&response_line) {
                        if val.get("method").is_some() && val.get("id").is_none() {
                            info!("Skipping notification in send_request: {:?}", val.get("method"));
                            continue;
                        }
                    }
                    
                    return serde_json::from_str(&response_line).context("Failed to parse response");
                }
            }
        }
    }

    pub fn initialize(&self) -> Result<()> {
        info!("Ã°Å¸â€Â§ Initializing ACP connection");
        
        let request = AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(0),
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": {
                        "readTextFile": true,
                        "writeTextFile": true
                    },
                    "terminal": true
                },
                "clientInfo": {
                    "name": "kiro-assistant",
                    "title": "Kiro Assistant",
                    "version": "0.1.0"
                }
            }),
        };

        let response = self.send_request(&request)?;
        
        if let Some(error) = response.error {
            anyhow::bail!("Initialize failed: {} (code: {})", error.message, error.code);
        }

        info!("Ã¢Å“â€¦ ACP initialized successfully");
        info!("Ã°Å¸â€œâ€¹ Agent info: {:?}", response.result);
        
        let mut initialized = self.initialized.lock().unwrap();
        *initialized = true;
        
        Ok(())
    }


    pub fn create_session(&self, cwd: Option<String>) -> Result<String> {
        info!("Ã°Å¸â€ â€¢ Creating new ACP session");
        
        // Ensure we're initialized
        {
            let initialized = self.initialized.lock().unwrap();
            if !*initialized {
                drop(initialized);
                self.initialize()?;
            }
        }
        
        let cwd = cwd.unwrap_or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "/".to_string())
        });
        
        let request = AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            method: "session/new".to_string(),
            params: serde_json::json!({
                "cwd": cwd,
                "mcpServers": []
            }),
        };

        let response = self.send_request(&request)?;
        
        if let Some(error) = response.error {
            anyhow::bail!("Session creation failed: {} (code: {})", error.message, error.code);
        }

        let session_id = response.result
            .and_then(|r| r.get("sessionId").cloned())
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .context("No sessionId in response")?;

        info!("Ã¢Å“â€¦ Session created: {}", session_id);
        
        let mut stored_session_id = self.session_id.lock().unwrap();
        *stored_session_id = Some(session_id.clone());
        
        Ok(session_id)
    }

    /// Load an existing session by ID via session/load
    pub fn load_existing_session(&self, session_id: &str, cwd: Option<String>) -> Result<String> {
        info!("Loading existing ACP session: {}", session_id);

        // Ensure we're initialized
        {
            let initialized = self.initialized.lock().unwrap();
            if !*initialized {
                drop(initialized);
                self.initialize()?;
            }
        }

        let cwd = cwd.unwrap_or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "/".to_string())
        });

        let request = AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            method: "session/load".to_string(),
            params: serde_json::json!({
                "sessionId": session_id,
                "cwd": cwd,
                "mcpServers": []
            }),
        };

        let response = self.send_request(&request)?;

        if let Some(error) = response.error {
            anyhow::bail!(
                "Session load failed: {} (code: {})",
                error.message,
                error.code
            );
        }

        info!("Session loaded: {}", session_id);

        let mut stored_session_id = self.session_id.lock().unwrap();
        *stored_session_id = Some(session_id.to_string());

        Ok(session_id.to_string())
    }

    /// Get the current active session ID
    pub fn get_session_id(&self) -> Option<String> {
        self.session_id.lock().unwrap().clone()
    }

    /// Directly set the active session ID (for switching without a load call)
    pub fn set_session_id(&self, session_id: Option<String>) {
        let mut stored = self.session_id.lock().unwrap();
        *stored = session_id;
    }

    /// Execute a Kiro slash command via _kiro.dev/commands/execute extension.
    /// Returns the AcpResponse result value.
    pub fn send_chat_streaming<F>(&self, content: String, mut callback: F, permission_callback: Option<Box<dyn Fn(serde_json::Value) + Send>>, notification_callback: Option<Box<dyn Fn(serde_json::Value) + Send>>) -> Result<()>
    where
        F: FnMut(String),
    {
        let debug_enabled = *self.debug_mode.lock().unwrap();
        
        if debug_enabled {
            info!("Ã°Å¸Ââ€º [ACP DEBUG] Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â");
            info!("Ã°Å¸Ââ€º [ACP DEBUG] Ã°Å¸â€™Â¬ SENDING CHAT MESSAGE");
            info!("Ã°Å¸Ââ€º [ACP DEBUG] Length: {} chars", content.len());
            info!("Ã°Å¸Ââ€º [ACP DEBUG] Content: {}", content);
            info!("Ã°Å¸Ââ€º [ACP DEBUG] Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â");
        } else {
            info!("Ã°Å¸â€™Â¬ Sending chat message (length: {})", content.len());
        }
        
        // Ensure we have a session
        let session_id = {
            let session_guard = self.session_id.lock().unwrap();
            if let Some(ref id) = *session_guard {
                id.clone()
            } else {
                drop(session_guard);
                self.create_session(None)?
            }
        };

        let request = AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(2),
            method: "session/prompt".to_string(),
            params: serde_json::json!({
                "sessionId": session_id,
                "prompt": [
                    {
                        "type": "text",
                        "text": content
                    }
                ]
            }),
        };

        let request_json = serde_json::to_string(&request)?;
        
        if debug_enabled {
            info!("Ã°Å¸Ââ€º [ACP DEBUG] Full request JSON: {}", request_json);
        } else {
            info!("Ã°Å¸â€œÂ¤ Sending session/prompt");
            info!("Ã°Å¸â€œÂ JSON: {}", request_json);
        }

        let mut conn_guard = self.connection.lock().unwrap();
        let conn = conn_guard
            .as_mut()
            .context("Not connected to ACP server")?;

        let mut full_response = String::new();
        
        // Extract what we need from the connection and drop the lock
        // so send_permission_response can write concurrently
        match conn {
            Connection::Tcp(stream) => {
                writeln!(stream, "{}", request_json)?;
                stream.flush()?;
                info!("Ã¢Å“â€¦ Request sent via TCP");
                
                let mut reader = BufReader::new(stream.try_clone()?);
                // Drop the connection lock so permission responses can write
                drop(conn_guard);
                
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) => {
                            warn!("Ã¢Å¡Â Ã¯Â¸Â  TCP stream closed");
                            break;
                        }
                        Ok(n) => info!("Ã°Å¸â€œÂ¨ Read {} bytes", n),
                        Err(e) => {
                            error!("Ã¢ÂÅ’ Read error: {}", e);
                            let mut cg = self.connection.lock().unwrap();
                            *cg = None;
                            return Err(e).context("Failed to read response");
                        }
                    }

                    if line.trim().is_empty() {
                        continue;
                    }

                    info!("Ã°Å¸â€œâ€ž Line: {}", line.trim());
                    
                    if let Ok(notification) = serde_json::from_str::<AcpNotification>(&line) {
                        info!("Ã°Å¸â€â€ Notification: method={}", notification.method);
                        
                        if notification.method == "session/request_permission" {
                            info!("Ã°Å¸â€Â Permission request received");
                            if let Some(ref perm_cb) = permission_callback {
                                let notification_value = serde_json::to_value(&notification)
                                    .unwrap_or(serde_json::json!({}));
                                perm_cb(notification_value);
                            }
                            continue;
                        }
                        
                        if notification.method == "session/update" {
                            if let Some(update) = notification.params.get("update") {
                                if let Some(session_update) = update.get("sessionUpdate").and_then(|v| v.as_str()) {
                                    if session_update == "agent_message_chunk" {
                                        if let Some(content_obj) = update.get("content") {
                                            if let Some(text) = content_obj.get("text").and_then(|v| v.as_str()) {
                                                full_response.push_str(text);
                                                info!("Ã°Å¸â€œÂ Accumulated: {} chars", full_response.len());
                                                callback(full_response.clone());
                                            }
                                        }
                                    }
                                    // Forward tool_call updates to notification callback
                                    if session_update == "tool_call" || session_update == "tool_call_update" {
                                        if let Some(ref notif_cb) = notification_callback {
                                            notif_cb(serde_json::to_value(&notification).unwrap_or_default());
                                        }
                                    }
                                }
                            }
                        }
                        continue;
                    }
                    
                    if let Ok(response) = serde_json::from_str::<AcpResponse>(&line) {
                        info!("Ã°Å¸â€œÂ¬ Response: id={:?}", response.id);
                        
                        if let Some(error) = response.error {
                            error!("Ã¢ÂÅ’ ACP error: {} (code: {})", error.message, error.code);
                            anyhow::bail!("ACP error: {}", error.message);
                        }
                        
                        info!("Ã¢Å“â€¦ Prompt completed");
                        break;
                    }
                }
            }
            Connection::Pipe { stdin, stdout } => {
                let mut stdin_guard = stdin.lock().unwrap();
                writeln!(stdin_guard, "{}", request_json)?;
                stdin_guard.flush()?;
                drop(stdin_guard);
                info!("Ã¢Å“â€¦ Request sent via pipe");
                
                // We need to keep reading from stdout, but drop the connection lock
                // so send_permission_response can access stdin via the Arc<Mutex<>>
                // 
                // Problem: stdout is behind &mut conn_guard, we can't drop conn_guard
                // while still borrowing stdout. Solution: read in a loop that temporarily
                // re-acquires the lock for each read.
                //
                // Actually, we need a different approach for pipes. Let's keep the lock
                // but have send_permission_response use the stdin Arc directly.
                
                loop {
                    let mut line = String::new();
                    match stdout.read_line(&mut line) {
                        Ok(0) => {
                            warn!("Ã¢Å¡Â Ã¯Â¸Â  Pipe closed");
                            break;
                        }
                        Ok(n) => info!("Ã°Å¸â€œÂ¨ Read {} bytes", n),
                        Err(e) => {
                            error!("Ã¢ÂÅ’ Read error: {}", e);
                            *conn_guard = None;
                            return Err(e).context("Failed to read response");
                        }
                    }

                    if line.trim().is_empty() {
                        continue;
                    }

                    info!("Ã°Å¸â€œâ€ž Line: {}", line.trim());
                    
                    if let Ok(notification) = serde_json::from_str::<AcpNotification>(&line) {
                        info!("Ã°Å¸â€â€ Notification: method={}", notification.method);
                        
                        if notification.method == "session/request_permission" {
                            info!("Ã°Å¸â€Â Permission request received");
                            if let Some(ref perm_cb) = permission_callback {
                                let notification_value = serde_json::to_value(&notification)
                                    .unwrap_or(serde_json::json!({}));
                                perm_cb(notification_value);
                            }
                            continue;
                        }
                        
                        if notification.method == "session/update" {
                            if let Some(update) = notification.params.get("update") {
                                if let Some(session_update) = update.get("sessionUpdate").and_then(|v| v.as_str()) {
                                    if session_update == "agent_message_chunk" {
                                        if let Some(content_obj) = update.get("content") {
                                            if let Some(text) = content_obj.get("text").and_then(|v| v.as_str()) {
                                                full_response.push_str(text);
                                                info!("Ã°Å¸â€œÂ Accumulated: {} chars", full_response.len());
                                                callback(full_response.clone());
                                            }
                                        }
                                    }
                                    // Forward tool_call updates to notification callback
                                    if session_update == "tool_call" || session_update == "tool_call_update" {
                                        if let Some(ref notif_cb) = notification_callback {
                                            notif_cb(serde_json::to_value(&notification).unwrap_or_default());
                                        }
                                    }
                                }
                            }
                        }
                        continue;
                    }
                    
                    if let Ok(response) = serde_json::from_str::<AcpResponse>(&line) {
                        info!("Ã°Å¸â€œÂ¬ Response: id={:?}", response.id);
                        
                        if let Some(error) = response.error {
                            error!("Ã¢ÂÅ’ ACP error: {} (code: {})", error.message, error.code);
                            anyhow::bail!("ACP error: {}", error.message);
                        }
                        
                        info!("Ã¢Å“â€¦ Prompt completed");
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub fn disconnect(&self) {
        info!("Disconnecting from ACP server");
        let mut conn = self.connection.lock().unwrap();
        *conn = None;
        
        // Terminate the spawned process using ProcessManager
        let mut pm = self.process_manager.lock().unwrap();
        pm.terminate();
    }
}
