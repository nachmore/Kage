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

use crate::lock_ext::LockExt;
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
    /// Write handle for TCP.
    ///
    /// The inner `Arc<Mutex<TcpStream>>` mirrors the `pipe_stdin` layout: the
    /// outer mutex guards replacement of the handle (connect/disconnect), the
    /// inner mutex serializes the actual write+flush. Nesting them this way
    /// lets writers briefly hold the outer lock, clone the inner `Arc`, drop
    /// the outer guard, then write through the inner mutex — without calling
    /// `TcpStream::try_clone()` on every send (which dup's an OS handle).
    pub tcp_writer: Arc<Mutex<Option<Arc<Mutex<TcpStream>>>>>,
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
        *self.connected.lock_or_recover()
    }

    /// Set the notification handler. Called by the background reader for all notifications.
    pub fn set_notification_handler<F: Fn(serde_json::Value) + Send + Sync + 'static>(&self, handler: F) {
        let mut h = self.notification_handler.lock_or_recover();
        *h = Some(Arc::new(handler));
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
                self.spawn_backend_process(spawn_command)?;
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
                *self.tcp_writer.lock_or_recover() = Some(Arc::new(Mutex::new(write_clone)));

                let read_clone = stream.try_clone()?;
                self.start_reader_thread(ReaderSource::Tcp(BufReader::new(read_clone)));

                *self.connected.lock_or_recover() = true;
                info!("Connected to kage-cli at {}", addr);
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

    fn spawn_backend_process(&self, command_str: &str) -> Result<()> {
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
            let mut pm = self.process_manager.lock_or_recover();
            if let Err(e) = pm.store_process(child) {
                warn!("Failed to store spawned process: {}", e);
            }
        }

        let stdin_arc = Arc::new(Mutex::new(stdin));
        *self.pipe_stdin.lock_or_recover() = Some(stdin_arc);

        self.start_reader_thread(ReaderSource::Pipe(BufReader::new(stdout)));

        *self.connected.lock_or_recover() = true;
        info!("Local process spawned, reader thread started");
        Ok(())
    }

    // --- Background Reader Thread ---

    fn start_reader_thread(&self, source: ReaderSource) {
        let (response_tx, response_rx) = mpsc::channel();
        *self.response_rx.lock_or_recover() = Some(response_rx);

        let notification_handler = self.notification_handler.clone();
        let debug_mode = self.debug_mode.clone();
        let connected = self.connected.clone();
        let last_activity = self.last_activity.clone();

        thread::Builder::new().name("acp-reader".into()).spawn(move || {
            let mut reader: Box<dyn BufRead + Send> = match source {
                ReaderSource::Pipe(r) => Box::new(r),
                ReaderSource::Tcp(r) => Box::new(r),
            };

            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        warn!("Reader: stream closed (EOF)");
                        *connected.lock_or_recover() = false;
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
                        *connected.lock_or_recover() = false;
                        break;
                    }
                }

                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let debug = *debug_mode.lock_or_recover();
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
                    *last_activity.lock_or_recover() = now;
                }

                let has_method = val.get("method").and_then(|m| m.as_str()).is_some();
                let has_id = val.get("id").is_some_and(|id| !id.is_null());

                if has_id && !has_method {
                    if let Ok(response) = serde_json::from_value::<AcpResponse>(val) {
                        if debug {
                            info!("[READER] Response id={:?}", response.id);
                        }
                        let _ = response_tx.send(response);
                    }
                } else if has_method {
                    if debug {
                        info!(
                            "[READER] Notification: {}",
                            val.get("method")
                                .and_then(|m| m.as_str())
                                .unwrap_or("<unknown>")
                        );
                    }
                    // Clone the handler Arc out and drop the mutex guard BEFORE
                    // invoking. Holding the lock across the callback would cause
                    // deadlocks whenever the callback re-acquires a lock that
                    // the main thread holds while waiting in send_request.
                    let handler_arc = match notification_handler.lock() {
                        Ok(guard) => guard.as_ref().cloned(),
                        Err(poisoned) => {
                            log::warn!("Notification handler mutex poisoned — recovering");
                            poisoned.into_inner().as_ref().cloned()
                        }
                    };
                    if let Some(cb) = handler_arc {
                        cb(val);
                    }
                }
            }

            info!("Reader thread exiting");
        }).expect("Failed to spawn acp-reader thread");
    }

    // --- I/O ---

    /// Send a JSON-RPC request and wait for the response.
    pub fn send_request(&self, request: &super::types::AcpRequest) -> Result<AcpResponse> {
        let request_json = serde_json::to_string(request)?;
        let debug = *self.debug_mode.lock_or_recover();

        if debug {
            info!("[SEND] {} id={:?}", request.method, request.id);
            info!("[SEND] {}", request_json);
        } else {
            info!("Sending: {} id={:?}", request.method, request.id);
        }

        self.write_line(&request_json)?;

        let rx_guard = self.response_rx.lock_or_recover();
        let rx = rx_guard.as_ref().context("No reader thread (not connected)")?;

        let idle_timeout = Duration::from_secs(60);
        let poll_interval = Duration::from_secs(5);

        let start_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        *self.last_activity.lock_or_recover() = start_ms;

        loop {
            match rx.recv_timeout(poll_interval) {
                Ok(response) => {
                    if debug {
                        info!("[RECV] Response id={:?}", response.id);
                    }
                    return Ok(response);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let last = *self.last_activity.lock_or_recover();
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
                    *self.connected.lock_or_recover() = false;
                    anyhow::bail!("Connection lost while waiting for response")
                }
            }
        }
    }

    /// Write a line to the ACP server (pipe stdin or TCP).
    ///
    /// For both transports we clone the inner `Arc` out under a brief outer-lock
    /// scope, drop the outer guard, then hold only the inner write mutex for the
    /// `writeln!` + `flush`. This keeps the outer lock contention-free for
    /// readers (connect/disconnect) and avoids the per-write `TcpStream::try_clone`
    /// that was previously burning a kernel handle allocation on every message.
    pub fn write_line(&self, line: &str) -> Result<()> {
        // Pipe first
        let stdin_arc = {
            let guard = self.pipe_stdin.lock_or_recover();
            guard.as_ref().cloned()
        };
        if let Some(stdin_arc) = stdin_arc {
            let mut stdin = stdin_arc.lock_or_recover();
            writeln!(stdin, "{}", line)?;
            stdin.flush()?;
            return Ok(());
        }

        // Fall back to TCP
        let tcp_arc = {
            let guard = self.tcp_writer.lock_or_recover();
            guard.as_ref().cloned()
        };
        if let Some(tcp_arc) = tcp_arc {
            let mut writer = tcp_arc.lock_or_recover();
            writeln!(writer, "{}", line)?;
            writer.flush()?;
            return Ok(());
        }

        anyhow::bail!("No write handle available")
    }

    /// Disconnect from the ACP server.
    pub fn disconnect(&self) {
        info!("Disconnecting from ACP server");
        *self.connected.lock_or_recover() = false;
        *self.pipe_stdin.lock_or_recover() = None;
        *self.tcp_writer.lock_or_recover() = None;
        let mut pm = self.process_manager.lock_or_recover();
        pm.terminate();
    }

    /// Full teardown: disconnect, kill process, reset response channel.
    pub fn force_disconnect(&self) {
        info!("Force-disconnecting ACP (full teardown)");
        *self.connected.lock_or_recover() = false;
        *self.response_rx.lock_or_recover() = None;
        *self.pipe_stdin.lock_or_recover() = None;
        *self.tcp_writer.lock_or_recover() = None;
        let mut pm = self.process_manager.lock_or_recover();
        pm.terminate();
    }
}
