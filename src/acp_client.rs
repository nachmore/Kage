use anyhow::{Context, Result};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

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
#[allow(dead_code)]
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
    Local { spawn_command: String },
    Remote { host: String, port: u16 },
}

/// Callback type for handling notifications from the background reader
pub type NotificationHandler = Arc<Mutex<Option<Box<dyn Fn(serde_json::Value) + Send>>>>;

pub struct AcpClient {
    mode: AcpConnectionMode,
    /// Write handle for pipe stdin
    pipe_stdin: Arc<Mutex<Option<Arc<Mutex<ChildStdin>>>>>,
    /// Write handle for TCP
    tcp_writer: Arc<Mutex<Option<TcpStream>>>,
    /// Channel to receive responses from the background reader
    response_rx: Arc<Mutex<Option<mpsc::Receiver<AcpResponse>>>>,
    /// Notification handler called by the background reader thread
    notification_handler: NotificationHandler,
    /// Accumulated streaming text (reset per message)
    pub streaming_accumulator: Arc<Mutex<String>>,
    max_retries: u32,
    initial_retry_delay_ms: u64,
    process_manager: Arc<Mutex<ProcessManager>>,
    session_id: Arc<Mutex<Option<String>>>,
    initialized: Arc<Mutex<bool>>,
    debug_mode: Arc<Mutex<bool>>,
    connected: Arc<Mutex<bool>>,
}

impl AcpClient {
    pub fn new(mode: AcpConnectionMode) -> Self {
        Self {
            mode,
            pipe_stdin: Arc::new(Mutex::new(None)),
            tcp_writer: Arc::new(Mutex::new(None)),
            response_rx: Arc::new(Mutex::new(None)),
            notification_handler: Arc::new(Mutex::new(None)),
            streaming_accumulator: Arc::new(Mutex::new(String::new())),
            max_retries: 5,
            initial_retry_delay_ms: 100,
            process_manager: Arc::new(Mutex::new(ProcessManager::new())),
            session_id: Arc::new(Mutex::new(None)),
            initialized: Arc::new(Mutex::new(false)),
            debug_mode: Arc::new(Mutex::new(false)),
            connected: Arc::new(Mutex::new(false)),
        }
    }

    pub fn set_debug_mode(&self, enabled: bool) {
        let mut debug = self.debug_mode.lock().unwrap();
        *debug = enabled;
    }

    pub fn get_process_manager(&self) -> Arc<Mutex<ProcessManager>> {
        self.process_manager.clone()
    }

    pub fn get_pipe_stdin(&self) -> Arc<Mutex<Option<Arc<Mutex<ChildStdin>>>>> {
        self.pipe_stdin.clone()
    }

    pub fn get_tcp_writer(&self) -> Arc<Mutex<Option<TcpStream>>> {
        self.tcp_writer.clone()
    }

    /// Set the notification handler. Called by the background reader for all notifications.
    pub fn set_notification_handler<F: Fn(serde_json::Value) + Send + 'static>(&self, handler: F) {
        let mut h = self.notification_handler.lock().unwrap();
        *h = Some(Box::new(handler));
    }

    pub fn is_connected(&self) -> bool {
        *self.connected.lock().unwrap()
    }

    pub fn get_session_id(&self) -> Option<String> {
        self.session_id.lock().unwrap().clone()
    }

    pub fn set_session_id(&self, session_id: Option<String>) {
        let mut stored = self.session_id.lock().unwrap();
        *stored = session_id;
    }

    // --- Connection ---

    pub fn connect(&self) -> Result<()> {
        match &self.mode {
            AcpConnectionMode::Local { ref spawn_command } => {
                if self.is_connected() {
                    info!("Already connected via pipes");
                    return Ok(());
                }
                info!("Local mode: spawning process");
                self.spawn_kiro_process(spawn_command)?;
                Ok(())
            }
            AcpConnectionMode::Remote { .. } => {
                if self.is_connected() {
                    info!("Already connected via TCP");
                    return Ok(());
                }
                info!("Remote mode: establishing TCP connection");
                self.connect_with_retry(0)
            }
        }
    }

    fn connect_with_retry(&self, attempt: u32) -> Result<()> {
        let (host, port) = match &self.mode {
            AcpConnectionMode::Remote { host, port } => (host.clone(), *port),
            _ => anyhow::bail!("Cannot use TCP in local mode"),
        };

        let addr = format!("{}:{}", host, port);
        info!("TCP connection attempt {}/{} to {}", attempt + 1, self.max_retries + 1, addr);

        match TcpStream::connect_timeout(
            &addr.parse().context("Invalid address")?,
            Duration::from_secs(5),
        ) {
            Ok(stream) => {
                stream.set_read_timeout(Some(Duration::from_secs(30)))?;
                stream.set_write_timeout(Some(Duration::from_secs(5)))?;

                // Clone for writing
                let write_clone = stream.try_clone()?;
                *self.tcp_writer.lock().unwrap() = Some(write_clone);

                // Clone for reading — background reader thread
                let read_clone = stream.try_clone()?;
                self.start_reader_thread(ReaderSource::Tcp(BufReader::new(read_clone)));

                *self.connected.lock().unwrap() = true;
                info!("Connected to kiro-cli at {}", addr);
                Ok(())
            }
            Err(e) => {
                warn!("Connection attempt {} failed: {}", attempt + 1, e);
                if attempt < self.max_retries {
                    let delay = self.initial_retry_delay_ms * 2u64.pow(attempt);
                    thread::sleep(Duration::from_millis(delay.min(30000)));
                    self.connect_with_retry(attempt + 1)
                } else {
                    Err(e).context(format!("Failed to connect after {} attempts", self.max_retries + 1))
                }
            }
        }
    }

    fn spawn_kiro_process(&self, command_str: &str) -> Result<()> {
        info!("Spawning: {}", command_str);

        let parts: Vec<&str> = command_str.split_whitespace().collect();
        if parts.is_empty() {
            anyhow::bail!("Empty spawn command");
        }

        let program = parts[0];
        let args = &parts[1..];

        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        os::configure_process_spawn(&mut cmd);

        let mut child = cmd.spawn().context(format!("Failed to spawn: {}", program))?;

        let stdin = child.stdin.take().context("No stdin")?;
        let stdout = child.stdout.take().context("No stdout")?;

        // Store process for cleanup
        {
            let mut pm = self.process_manager.lock().unwrap();
            pm.store_process(child).ok();
        }

        // Store stdin for writing
        let stdin_arc = Arc::new(Mutex::new(stdin));
        *self.pipe_stdin.lock().unwrap() = Some(stdin_arc);

        // Start background reader on stdout
        self.start_reader_thread(ReaderSource::Pipe(BufReader::new(stdout)));

        *self.connected.lock().unwrap() = true;
        info!("Local process spawned, reader thread started");
        Ok(())
    }

    // --- Background Reader Thread ---

    fn start_reader_thread(&self, source: ReaderSource) {
        let (response_tx, response_rx) = mpsc::channel();
        *self.response_rx.lock().unwrap() = Some(response_rx);

        let notification_handler = self.notification_handler.clone();
        let debug_mode = self.debug_mode.clone();
        let connected = self.connected.clone();

        thread::spawn(move || {
            let mut reader: Box<dyn BufRead + Send> = match source {
                ReaderSource::Pipe(r) => Box::new(r),
                ReaderSource::Tcp(r) => Box::new(r),
            };

            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        warn!("Reader: stream closed (EOF)");
                        *connected.lock().unwrap() = false;
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        // Read timeout on TCP is normal when idle
                        if e.kind() == std::io::ErrorKind::TimedOut
                            || e.kind() == std::io::ErrorKind::WouldBlock
                        {
                            continue;
                        }
                        error!("Reader: error: {}", e);
                        *connected.lock().unwrap() = false;
                        break;
                    }
                }

                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let debug = *debug_mode.lock().unwrap();
                if debug {
                    info!("[READER] {}", &trimmed[..trimmed.len().min(200)]);
                }

                // Try to parse as a JSON value first to classify
                let val: serde_json::Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("Reader: failed to parse JSON: {}", e);
                        continue;
                    }
                };

                // Responses have an "id" field and no "method" field
                // Notifications have a "method" field and no "id" (or id is null)
                let has_method = val.get("method").and_then(|m| m.as_str()).is_some();
                let has_id = val.get("id").map_or(false, |id| !id.is_null());

                if has_id && !has_method {
                    // This is a response
                    if let Ok(response) = serde_json::from_value::<AcpResponse>(val) {
                        if debug {
                            info!("[READER] Response id={:?}", response.id);
                        }
                        let _ = response_tx.send(response);
                    }
                } else if has_method {
                    // This is a notification
                    if debug {
                        info!("[READER] Notification: {}", val.get("method").unwrap());
                    }
                    if let Ok(handler) = notification_handler.lock() {
                        if let Some(ref cb) = *handler {
                            cb(val);
                        }
                    }
                }
            }

            info!("Reader thread exiting");
        });
    }

    // --- Request/Response ---

    /// Send a JSON-RPC request and wait for the response.
    /// The background reader thread delivers responses via the channel.
    pub fn send_request(&self, request: &AcpRequest) -> Result<AcpResponse> {
        let request_json = serde_json::to_string(request)?;
        let debug = *self.debug_mode.lock().unwrap();

        if debug {
            info!("[SEND] {} id={:?}", request.method, request.id);
            info!("[SEND] {}", request_json);
        } else {
            info!("Sending: {} id={:?}", request.method, request.id);
        }

        // Write the request
        self.write_line(&request_json)?;

        // Wait for the response from the reader thread
        let rx_guard = self.response_rx.lock().unwrap();
        let rx = rx_guard.as_ref().context("No reader thread (not connected)")?;

        // Use a timeout to avoid hanging forever
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(response) => {
                if debug {
                    info!("[RECV] Response id={:?}", response.id);
                }
                Ok(response)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                anyhow::bail!("Timeout waiting for response to {}", request.method)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                *self.connected.lock().unwrap() = false;
                anyhow::bail!("Connection lost while waiting for response")
            }
        }
    }

    /// Write a line to the ACP server (pipe stdin or TCP)
    pub fn write_line(&self, line: &str) -> Result<()> {
        // Try pipe first
        if let Ok(guard) = self.pipe_stdin.lock() {
            if let Some(ref stdin_arc) = *guard {
                let mut stdin = stdin_arc.lock().unwrap();
                writeln!(stdin, "{}", line)?;
                stdin.flush()?;
                return Ok(());
            }
        }
        // Try TCP
        if let Ok(guard) = self.tcp_writer.lock() {
            if let Some(ref stream) = *guard {
                let mut writer = stream.try_clone()?;
                writeln!(writer, "{}", line)?;
                writer.flush()?;
                return Ok(());
            }
        }
        anyhow::bail!("No write handle available")
    }

    // --- Protocol Methods ---

    pub fn initialize(&self) -> Result<()> {
        info!("Initializing ACP connection");

        let request = AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(0),
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": true, "writeTextFile": true },
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

        info!("ACP initialized successfully");
        *self.initialized.lock().unwrap() = true;
        Ok(())
    }

    pub fn create_session(&self, cwd: Option<String>) -> Result<(String, Vec<serde_json::Value>)> {
        info!("Creating new ACP session");

        {
            let init = self.initialized.lock().unwrap();
            if !*init {
                drop(init);
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
            params: serde_json::json!({ "cwd": cwd, "mcpServers": [] }),
        };

        let response = self.send_request(&request)?;
        if let Some(error) = response.error {
            anyhow::bail!("Session creation failed: {} (code: {})", error.message, error.code);
        }

        let result = response.result.context("No result in session/new response")?;
        let session_id = result.get("sessionId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .context("No sessionId in response")?;

        let mut models_list = Vec::new();
        if let Some(models) = result.get("models").and_then(|m| m.get("availableModels")).and_then(|a| a.as_array()) {
            info!("Session has {} available models", models.len());
            models_list = models.clone();
        }

        info!("Session created: {}", session_id);
        *self.session_id.lock().unwrap() = Some(session_id.clone());
        Ok((session_id, models_list))
    }

    /// Send the built-in steering document on the current session.
    /// This should be called after creating a new session to give the agent
    /// its identity and behavior guidelines.
    pub fn send_builtin_steering(&self) {
        let session_id = match self.get_session_id() {
            Some(id) => id,
            None => return,
        };

        let steering_msg = format!(
            "{} {}",
            crate::commands::system::STEERING_MSG_PREFIX,
            crate::commands::system::BUILTIN_STEERING
        );

        // Reset accumulator so the steering response doesn't pollute the next message
        *self.streaming_accumulator.lock().unwrap() = String::new();

        let request = AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(98),
            method: "session/prompt".to_string(),
            params: serde_json::json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": steering_msg }]
            }),
        };

        match self.send_request(&request) {
            Ok(_) => info!("Built-in steering sent to session {}", session_id),
            Err(e) => warn!("Failed to send built-in steering: {}", e),
        }
    }

    pub fn load_existing_session(&self, session_id: &str, cwd: Option<String>) -> Result<String> {
        info!("Loading existing ACP session: {}", session_id);

        {
            let init = self.initialized.lock().unwrap();
            if !*init {
                drop(init);
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
            anyhow::bail!("Session load failed: {} (code: {})", error.message, error.code);
        }

        info!("Session loaded: {}", session_id);
        *self.session_id.lock().unwrap() = Some(session_id.to_string());
        Ok(session_id.to_string())
    }

    /// Send a chat message and stream the response.
    /// The background reader thread handles all incoming messages.
    /// Streaming chunks, permissions, and tool updates are delivered via
    /// the notification handler (set via set_notification_handler).
    /// This method blocks until the prompt response is received.
    /// Send a chat message with optional attachments.
    /// `content` is the text message.
    /// `attachments` is an optional list of content blocks (images, resource_links) to include.
    pub fn send_chat_streaming(&self, content: String, attachments: Option<Vec<serde_json::Value>>) -> Result<()> {
        let debug = *self.debug_mode.lock().unwrap();

        // Reset the streaming accumulator for the new message
        *self.streaming_accumulator.lock().unwrap() = String::new();

        if debug {
            info!("[CHAT] Sending message ({} chars): {}", content.len(), content);
        } else {
            info!("Sending chat message ({} chars)", content.len());
        }

        // Ensure we have a session
        let session_id = {
            let guard = self.session_id.lock().unwrap();
            if let Some(ref id) = *guard {
                id.clone()
            } else {
                drop(guard);
                let (id, _) = self.create_session(None)?;
                self.send_builtin_steering();
                id
            }
        };

        // Build the prompt content array
        let mut prompt: Vec<serde_json::Value> = Vec::new();

        // Add text block if non-empty
        if !content.is_empty() {
            prompt.push(serde_json::json!({ "type": "text", "text": content }));
        }

        // Append any attachments (images, resource_links)
        if let Some(att) = attachments {
            for block in att {
                prompt.push(block);
            }
        }

        // Ensure we have at least one content block
        if prompt.is_empty() {
            prompt.push(serde_json::json!({ "type": "text", "text": "" }));
        }

        let request = AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(2),
            method: "session/prompt".to_string(),
            params: serde_json::json!({
                "sessionId": session_id,
                "prompt": prompt
            }),
        };

        // send_request writes the request and waits for the response.
        // While waiting, the reader thread delivers all notifications
        // (streaming chunks, permissions, tool calls) via the notification handler.
        let response = self.send_request(&request)?;

        if let Some(error) = response.error {
            // Include the error data if available for richer error messages
            let detail = error.data
                .as_ref()
                .and_then(|d| d.as_str())
                .unwrap_or("");
            if detail.is_empty() {
                anyhow::bail!("ACP error: {}", error.message);
            } else {
                anyhow::bail!("ACP error: {} — {}", error.message, detail);
            }
        }

        info!("Prompt completed");
        Ok(())
    }

    pub fn disconnect(&self) {
        info!("Disconnecting from ACP server");
        *self.connected.lock().unwrap() = false;

        // Clear write handles
        *self.pipe_stdin.lock().unwrap() = None;
        *self.tcp_writer.lock().unwrap() = None;

        // Terminate the spawned process
        let mut pm = self.process_manager.lock().unwrap();
        pm.terminate();
    }

    /// Full teardown: disconnect, kill process, reset initialized state.
    /// After this, the next connect() + initialize() will start completely fresh.
    fn force_disconnect(&self) {
        info!("Force-disconnecting ACP (full teardown)");
        *self.connected.lock().unwrap() = false;
        *self.initialized.lock().unwrap() = false;

        // Drop the response channel so any blocked recv unblocks
        *self.response_rx.lock().unwrap() = None;

        // Clear write handles
        *self.pipe_stdin.lock().unwrap() = None;
        *self.tcp_writer.lock().unwrap() = None;

        // Kill the spawned process (Local mode) or just sever TCP
        let mut pm = self.process_manager.lock().unwrap();
        pm.terminate();
    }

    /// Tear down the current connection and establish a fresh one.
    /// Returns Ok(()) if reconnected and re-initialized successfully.
    fn restart_connection(&self) -> Result<()> {
        info!("Restarting ACP connection");
        self.force_disconnect();

        // Small delay to let the OS clean up sockets / process handles
        thread::sleep(Duration::from_millis(500));

        self.connect()?;
        self.initialize()?;
        Ok(())
    }

    /// Send a chat message with automatic recovery on timeout.
    ///
    /// Recovery strategy:
    /// 1. Send the prompt normally.
    /// 2. On timeout → restart the connection, try to reload the same session,
    ///    and resend the prompt.
    /// 3. If the reload+resend also times out → restart again with a brand-new
    ///    session and resend one last time.
    /// 4. If that still fails → give up and return the error.
    ///
    /// This means a single prompt will never cause more than one automatic
    /// restart cycle. But if the prompt eventually succeeds and a *later*
    /// prompt hangs, the restart budget resets.
    pub fn send_chat_streaming_with_recovery(
        &self,
        content: String,
        attachments: Option<Vec<serde_json::Value>>,
    ) -> Result<()> {
        // --- Attempt 1: normal send ---
        match self.send_chat_streaming(content.clone(), attachments.clone()) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let err_str = format!("{}", e);
                if !err_str.contains("Timeout") && !err_str.contains("Connection lost") {
                    // Not a timeout/disconnect — don't retry
                    return Err(e);
                }
                warn!("Prompt failed ({}), attempting recovery…", err_str);
            }
        }

        // --- Attempt 2: restart + reload session + resend ---
        let old_session_id = self.get_session_id();
        self.restart_connection()?;

        // Try to reload the previous session so the user keeps their history
        let mut session_restored = false;
        if let Some(ref sid) = old_session_id {
            info!("Attempting to reload session {} after restart", sid);
            match self.load_existing_session(sid, None) {
                Ok(_) => {
                    info!("Session {} reloaded successfully", sid);
                    session_restored = true;
                }
                Err(e) => {
                    warn!("Could not reload session {}: {}", sid, e);
                }
            }
        }

        if !session_restored {
            info!("Creating fresh session for retry");
            self.set_session_id(None);
            self.create_session(None)?;
            self.send_builtin_steering();
        }

        match self.send_chat_streaming(content.clone(), attachments.clone()) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let err_str = format!("{}", e);
                if !err_str.contains("Timeout") && !err_str.contains("Connection lost") {
                    return Err(e);
                }
                warn!("Prompt failed again after session reload ({}), trying fresh session…", err_str);
            }
        }

        // --- Attempt 3: restart + brand-new session + resend (last chance) ---
        self.restart_connection()?;
        self.set_session_id(None);
        self.create_session(None)?;
        self.send_builtin_steering();

        self.send_chat_streaming(content, attachments)
    }
}

enum ReaderSource {
    Pipe(BufReader<ChildStdout>),
    Tcp(BufReader<TcpStream>),
}
