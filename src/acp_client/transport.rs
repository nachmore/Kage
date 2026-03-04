//! ACP transport layer: connection management, pipe/TCP I/O, and background reader thread.

use anyhow::{Context, Result};
use log::{error, info, warn};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::os;
use crate::process_manager::ProcessManager;
use super::types::{AcpConnectionMode, AcpResponse, NotificationHandler};

enum ReaderSource {
    Pipe(BufReader<ChildStdout>),
    Tcp(BufReader<TcpStream>),
}

/// Manages the low-level connection to the ACP server (pipe or TCP),
/// the background reader thread, and raw line-based I/O.
pub struct AcpTransport {
    mode: AcpConnectionMode,
    /// Write handle for pipe stdin
    pub pipe_stdin: Arc<Mutex<Option<Arc<Mutex<ChildStdin>>>>>,
    /// Write handle for TCP
    pub tcp_writer: Arc<Mutex<Option<TcpStream>>>,
    /// Channel to receive responses from the background reader
    response_rx: Arc<Mutex<Option<mpsc::Receiver<AcpResponse>>>>,
    /// Notification handler called by the background reader thread
    notification_handler: NotificationHandler,
    pub max_retries: u32,
    pub initial_retry_delay_ms: u64,
    pub process_manager: Arc<Mutex<ProcessManager>>,
    pub debug_mode: Arc<Mutex<bool>>,
    pub connected: Arc<Mutex<bool>>,
    /// Epoch millis of the last message from the server.
    pub last_activity: Arc<Mutex<u64>>,
}

impl AcpTransport {
    pub fn new(mode: AcpConnectionMode) -> Self {
        Self {
            mode,
            pipe_stdin: Arc::new(Mutex::new(None)),
            tcp_writer: Arc::new(Mutex::new(None)),
            response_rx: Arc::new(Mutex::new(None)),
            notification_handler: Arc::new(Mutex::new(None)),
            max_retries: 5,
            initial_retry_delay_ms: 100,
            process_manager: Arc::new(Mutex::new(ProcessManager::new())),
            debug_mode: Arc::new(Mutex::new(false)),
            connected: Arc::new(Mutex::new(false)),
            last_activity: Arc::new(Mutex::new(0)),
        }
    }

    pub fn is_connected(&self) -> bool {
        *self.connected.lock().unwrap()
    }

    /// Set the notification handler. Called by the background reader for all notifications.
    pub fn set_notification_handler<F: Fn(serde_json::Value) + Send + 'static>(&self, handler: F) {
        let mut h = self.notification_handler.lock().unwrap();
        *h = Some(Box::new(handler));
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

                let write_clone = stream.try_clone()?;
                *self.tcp_writer.lock().unwrap() = Some(write_clone);

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

        {
            let mut pm = self.process_manager.lock().unwrap();
            pm.store_process(child).ok();
        }

        let stdin_arc = Arc::new(Mutex::new(stdin));
        *self.pipe_stdin.lock().unwrap() = Some(stdin_arc);

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
        let last_activity = self.last_activity.clone();

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
                    let display: String = trimmed.chars().take(200).collect();
                    info!("[READER] {}", display);
                }

                let val: serde_json::Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("Reader: failed to parse JSON: {}", e);
                        continue;
                    }
                };

                {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    *last_activity.lock().unwrap() = now;
                }

                let has_method = val.get("method").and_then(|m| m.as_str()).is_some();
                let has_id = val.get("id").map_or(false, |id| !id.is_null());

                if has_id && !has_method {
                    if let Ok(response) = serde_json::from_value::<AcpResponse>(val) {
                        if debug {
                            info!("[READER] Response id={:?}", response.id);
                        }
                        let _ = response_tx.send(response);
                    }
                } else if has_method {
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

    // --- I/O ---

    /// Send a JSON-RPC request and wait for the response.
    pub fn send_request(&self, request: &super::types::AcpRequest) -> Result<AcpResponse> {
        let request_json = serde_json::to_string(request)?;
        let debug = *self.debug_mode.lock().unwrap();

        if debug {
            info!("[SEND] {} id={:?}", request.method, request.id);
            info!("[SEND] {}", request_json);
        } else {
            info!("Sending: {} id={:?}", request.method, request.id);
        }

        self.write_line(&request_json)?;

        let rx_guard = self.response_rx.lock().unwrap();
        let rx = rx_guard.as_ref().context("No reader thread (not connected)")?;

        let idle_timeout = Duration::from_secs(60);
        let poll_interval = Duration::from_secs(5);

        let start_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        *self.last_activity.lock().unwrap() = start_ms;

        loop {
            match rx.recv_timeout(poll_interval) {
                Ok(response) => {
                    if debug {
                        info!("[RECV] Response id={:?}", response.id);
                    }
                    return Ok(response);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let last = *self.last_activity.lock().unwrap();
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let idle_ms = now.saturating_sub(last);

                    if idle_ms > idle_timeout.as_millis() as u64 {
                        anyhow::bail!("Timeout waiting for response to {}", request.method);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    *self.connected.lock().unwrap() = false;
                    anyhow::bail!("Connection lost while waiting for response")
                }
            }
        }
    }

    /// Write a line to the ACP server (pipe stdin or TCP)
    pub fn write_line(&self, line: &str) -> Result<()> {
        if let Ok(guard) = self.pipe_stdin.lock() {
            if let Some(ref stdin_arc) = *guard {
                let mut stdin = stdin_arc.lock().unwrap();
                writeln!(stdin, "{}", line)?;
                stdin.flush()?;
                return Ok(());
            }
        }
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

    /// Disconnect from the ACP server.
    pub fn disconnect(&self) {
        info!("Disconnecting from ACP server");
        *self.connected.lock().unwrap() = false;
        *self.pipe_stdin.lock().unwrap() = None;
        *self.tcp_writer.lock().unwrap() = None;
        let mut pm = self.process_manager.lock().unwrap();
        pm.terminate();
    }

    /// Full teardown: disconnect, kill process, reset response channel.
    pub fn force_disconnect(&self) {
        info!("Force-disconnecting ACP (full teardown)");
        *self.connected.lock().unwrap() = false;
        *self.response_rx.lock().unwrap() = None;
        *self.pipe_stdin.lock().unwrap() = None;
        *self.tcp_writer.lock().unwrap() = None;
        let mut pm = self.process_manager.lock().unwrap();
        pm.terminate();
    }
}
