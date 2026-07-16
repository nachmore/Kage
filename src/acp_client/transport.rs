//! ACP transport layer: connection management, pipe/TCP I/O, and background reader thread.

use anyhow::{Context, Result};
use log::{error, info, warn};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::types::{AcpConnectionMode, AcpRequest, AcpResponse, NotificationHandler};
use crate::lock_ext::LockExt;
use crate::os;
use crate::process_manager::ProcessManager;

/// Per-request inbox: the reader thread routes responses here by id.
/// `sync_channel(1)` is enough — there's only ever one response per id, and
/// using std mpsc avoids dragging tokio into the sync transport layer.
type ResponseInbox = mpsc::SyncSender<AcpResponse>;

enum ReaderSource {
    Pipe(BufReader<ChildStdout>),
    Tcp(BufReader<TcpStream>),
}

/// Split a spawn command into program + args, honouring single and double
/// quotes so a path containing spaces survives intact.
///
/// `split_whitespace` broke any command whose program path had a space
/// (`C:\Program Files\agent\kiro-cli.exe` → spawn `C:\Program`). This is a
/// minimal shell-word splitter: whitespace separates tokens except inside a
/// quoted span, and a matching quote pair is stripped. For unquoted input it
/// is identical to `split_whitespace`, so existing commands (including the
/// `cmd /c set …&& …` / `env … codex-acp` Ollama forms) tokenize unchanged.
/// We intentionally do NOT interpret backslash escapes — on Windows the
/// backslash is a path separator, and escaping isn't needed once quotes group
/// the spaces.
fn tokenize_spawn_command(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;

    for c in command.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None; // closing quote — drop it, keep the token open
                } else {
                    current.push(c);
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                    in_token = true; // `""` alone is a valid empty argument
                } else if c.is_whitespace() {
                    if in_token {
                        tokens.push(std::mem::take(&mut current));
                        in_token = false;
                    }
                } else {
                    current.push(c);
                    in_token = true;
                }
            }
        }
    }
    if in_token {
        tokens.push(current);
    }
    tokens
}

/// Manages the low-level connection to the ACP server (pipe or TCP),
/// the background reader thread, and raw line-based I/O.
pub struct AcpTransport {
    mode: Arc<Mutex<AcpConnectionMode>>,
    /// Write handle for pipe stdin.
    pipe_stdin: Arc<Mutex<Option<Arc<Mutex<ChildStdin>>>>>,
    /// Write handle for TCP.
    ///
    /// The inner `Arc<Mutex<TcpStream>>` mirrors the `pipe_stdin` layout: the
    /// outer mutex guards replacement of the handle (connect/disconnect), the
    /// inner mutex serializes the actual write+flush. Nesting them this way
    /// lets writers briefly hold the outer lock, clone the inner `Arc`, drop
    /// the outer guard, then write through the inner mutex — without calling
    /// `TcpStream::try_clone()` on every send (which dup's an OS handle).
    tcp_writer: Arc<Mutex<Option<Arc<Mutex<TcpStream>>>>>,
    /// Monotonic request-id allocator. Every outbound request gets a fresh id
    /// from this counter so two callers can never collide. JSON-RPC ids are
    /// strings or numbers; we send them as numbers and the matching response
    /// echoes the same number back.
    next_id: Arc<AtomicU64>,
    /// Map from outstanding request id to the per-request response inbox.
    /// The reader thread looks up the entry on every `id`-bearing line and
    /// forwards the response into the matching channel. A timed-out caller
    /// removes its own entry on the way out, so a late response just finds
    /// nothing in the map and gets logged + dropped — no cross-request leak.
    pending: Arc<Mutex<HashMap<u64, ResponseInbox>>>,
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
            mode: Arc::new(Mutex::new(mode)),
            pipe_stdin: Arc::new(Mutex::new(None)),
            tcp_writer: Arc::new(Mutex::new(None)),
            // Start at 1 — id=0 used to be the initialize handshake's hardcoded
            // value, and avoiding it makes pre/post-fix log diffs easier to read.
            next_id: Arc::new(AtomicU64::new(1)),
            pending: Arc::new(Mutex::new(HashMap::new())),
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

    /// Stronger liveness check than `is_connected()`.
    ///
    /// `is_connected()` only reflects the `connected` flag, which flips false
    /// when the reader thread observes EOF — there's a window where the agent
    /// process has died but that flag is still true. In Local mode we
    /// cross-check the managed child's liveness so a zombie/exited agent reads
    /// as unhealthy immediately. In Remote (TCP) mode there's no child to
    /// probe, so this is equivalent to `is_connected()`.
    pub fn is_healthy(&self) -> bool {
        if !self.is_connected() {
            return false;
        }
        match self.process_manager.lock_or_recover().child_liveness() {
            // Child managed and confirmed dead — not healthy despite the flag.
            Some(false) => false,
            // Alive, or no managed child (TCP mode): trust `connected`.
            _ => true,
        }
    }

    /// Set the notification handler. Called by the background reader for all notifications.
    pub fn set_notification_handler<F: Fn(serde_json::Value) + Send + Sync + 'static>(
        &self,
        handler: F,
    ) {
        let mut h = self.notification_handler.lock_or_recover();
        *h = Some(Arc::new(handler));
    }

    /// Feed a synthetic notification through the registered handler, exactly
    /// as if the reader thread had received it off the wire. Lets internal
    /// code (e.g. the recovery ladder) reach the frontend via the same
    /// `method`-switch dispatch without threading an `AppHandle` down into the
    /// transport-agnostic session layer. No-op if no handler is set.
    ///
    /// Clones the handler `Arc` out and drops the guard before invoking, for
    /// the same deadlock-avoidance reason as the reader-thread call site.
    pub fn dispatch_synthetic_notification(&self, notification: serde_json::Value) {
        let handler_arc = self
            .notification_handler
            .lock_or_recover()
            .as_ref()
            .cloned();
        if let Some(cb) = handler_arc {
            cb(notification);
        }
    }

    // --- Connection ---

    pub fn connect(&self) -> Result<()> {
        let mode = self.mode.lock_or_recover().clone();
        match mode {
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

    /// Replace the connection mode. Disconnects the current transport first.
    pub fn set_mode(&self, mode: AcpConnectionMode) {
        self.disconnect();
        *self.mode.lock_or_recover() = mode;
    }

    fn connect_with_retry(&self, attempt: u32) -> Result<()> {
        let (host, port) = match &*self.mode.lock_or_recover() {
            AcpConnectionMode::Remote { host, port } => (host.clone(), *port),
            _ => anyhow::bail!("Cannot use TCP in local mode"),
        };

        let addr = format!("{}:{}", host, port);
        info!(
            "TCP connection attempt {}/{} to {}",
            attempt + 1,
            self.max_retries + 1,
            addr
        );

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
                info!("Connected to agent backend at {}", addr);
                Ok(())
            }
            Err(e) => {
                warn!("Connection attempt {} failed: {}", attempt + 1, e);
                if attempt < self.max_retries {
                    let delay = self.initial_retry_delay_ms * 2u64.pow(attempt);
                    thread::sleep(Duration::from_millis(delay.min(30000)));
                    self.connect_with_retry(attempt + 1)
                } else {
                    Err(e).context(format!(
                        "Failed to connect after {} attempts",
                        self.max_retries + 1
                    ))
                }
            }
        }
    }

    fn spawn_backend_process(&self, command_str: &str) -> Result<()> {
        info!("Spawning: {}", command_str);

        // Reap any previously-managed child before spawning a replacement.
        // This is the single chokepoint that guarantees the old agent
        // process is killed and `wait()`ed regardless of which `connect()`
        // caller we arrived from. The motivating leak: when the agent dies
        // on its own, the reader thread flips `connected=false` on EOF but
        // never calls `terminate()`, so the dead `Child` handle lingers in
        // the ProcessManager. A lazy reconnect would then overwrite that
        // handle in `store_process`, dropping it WITHOUT `wait()` — which
        // leaves a zombie (defunct) process on macOS/Linux until the app
        // exits (std `Child` neither kills nor reaps on drop). `connect()`
        // only reaches here when disconnected, so the old process is always
        // either already dead (EOF/error) or already terminated (an explicit
        // disconnect) — killing it here is a cheap no-op in both cases.
        self.process_manager.lock_or_recover().terminate();

        let parts = tokenize_spawn_command(command_str);
        if parts.is_empty() {
            anyhow::bail!("Empty spawn command");
        }

        let program = &parts[0];
        let args = &parts[1..];

        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        os::configure_process_spawn(&mut cmd);

        let mut child = cmd
            .spawn()
            .context(format!("Failed to spawn: {}", program))?;

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
        let notification_handler = self.notification_handler.clone();
        let debug_mode = self.debug_mode.clone();
        let connected = self.connected.clone();
        let last_activity = self.last_activity.clone();
        let pending = self.pending.clone();

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
                        // Drop every pending inbox sender. Each blocked
                        // send_request caller wakes with a Disconnected
                        // error within milliseconds rather than waiting
                        // for the per-request 60s timeout. Pre-fix, the
                        // reader silently flipped connected=false and
                        // every in-flight request continued blocking
                        // until the timer fired.
                        pending.lock_or_recover().clear();
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
                        // Same wakeup as the EOF branch above.
                        pending.lock_or_recover().clear();
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
                        // Route to the waiting caller. JSON-RPC allows ids to be
                        // strings or numbers; we send numbers and the agent
                        // echoes them back. If the agent echoes a string or a
                        // mismatched id, the entry just isn't found and the
                        // response is logged + dropped — far better than the
                        // pre-fix behavior of delivering it to the next caller.
                        let id_u64 = response.id.as_u64();
                        match id_u64.and_then(|id| pending.lock_or_recover().remove(&id)) {
                            Some(inbox) => {
                                let _ = inbox.send(response);
                            }
                            None => {
                                warn!(
                                    "Reader: orphaned response id={:?} (no matching pending request); dropping",
                                    response.id
                                );
                            }
                        }
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

    /// Flat per-request deadline for ordinary RPCs (handshake, session load,
    /// commands/* — anything that should reply promptly). A `session/prompt`
    /// uses the idle watchdog instead (see [`send_prompt_request`]) because a
    /// healthy turn can legitimately run for minutes.
    const RPC_TIMEOUT: Duration = Duration::from_secs(60);

    /// How long the connection may go *completely silent* during a
    /// `session/prompt` before we treat it as wedged. A working agent streams
    /// `session/update` notifications continuously (token deltas, tool calls),
    /// so any real progress resets the clock. Only a genuinely dead/hung
    /// backend — no response AND no notifications for this long — trips it,
    /// at which point the recovery ladder respawns. Generous by design: the
    /// user-facing escape hatch is the Stop button, not a wall-clock cap.
    const PROMPT_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

    /// Send a JSON-RPC request and wait for its matching response.
    ///
    /// The transport owns the request id — callers don't pass one. This makes
    /// id collisions impossible, and lets the reader thread route responses
    /// to the originating caller via a per-request inbox instead of a single
    /// shared channel. A response that arrives after this method has timed
    /// out is dropped (logged) instead of corrupting the next caller.
    pub fn send_request(&self, method: &str, params: serde_json::Value) -> Result<AcpResponse> {
        self.send_request_inner(method, params, false)
    }

    /// Send a `session/prompt` request and wait for the turn to end.
    ///
    /// Unlike [`send_request`], this does not impose a wall-clock deadline on
    /// the whole turn — a chat request may run as long as the agent keeps
    /// working. It watches inbound activity instead: as long as the backend is
    /// streaming updates it waits; only total silence for `PROMPT_IDLE_TIMEOUT`
    /// is treated as a wedged connection.
    pub fn send_prompt_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<AcpResponse> {
        self.send_request_inner(method, params, true)
    }

    fn send_request_inner(
        &self,
        method: &str,
        params: serde_json::Value,
        idle_watchdog: bool,
    ) -> Result<AcpResponse> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = AcpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::Value::Number(id.into()),
            method: method.to_string(),
            params,
        };
        let request_json = serde_json::to_string(&request)?;
        let debug = *self.debug_mode.lock_or_recover();

        if debug {
            info!("[SEND] {} id={}", method, id);
            info!("[SEND] {}", request_json);
        } else {
            info!("Sending: {} id={}", method, id);
        }

        // Register the inbox before writing the request line — otherwise a
        // very fast reply could land before we've inserted the entry and get
        // dropped as orphaned.
        let (tx, rx) = mpsc::sync_channel::<AcpResponse>(1);
        self.pending.lock_or_recover().insert(id, tx);

        if let Err(e) = self.write_line(&request_json) {
            // Write failed — pull our entry back out so we don't leak it.
            self.pending.lock_or_recover().remove(&id);
            return Err(e);
        }

        if idle_watchdog {
            self.recv_with_idle_watchdog(id, method, &rx, debug)
        } else {
            // Per-request timeout, not a global idle timer: an unrelated chatty
            // session/update stream on another id no longer extends this
            // caller's deadline.
            match rx.recv_timeout(Self::RPC_TIMEOUT) {
                Ok(response) => {
                    if debug {
                        info!("[RECV] Response id={}", id);
                    }
                    Ok(response)
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.pending.lock_or_recover().remove(&id);
                    anyhow::bail!("Timeout waiting for response to {}", method)
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.pending.lock_or_recover().remove(&id);
                    *self.connected.lock_or_recover() = false;
                    anyhow::bail!("Connection lost while waiting for response")
                }
            }
        }
    }

    /// Wait for a prompt response, extending the deadline for as long as the
    /// backend shows *any* inbound activity. We poll the response inbox in
    /// short slices; on each timeout slice we check whether `last_activity`
    /// (bumped by the reader thread on every inbound line, for any id) has
    /// advanced. If it has, the turn is progressing and we keep waiting. Only
    /// `PROMPT_IDLE_TIMEOUT` of total silence bails — the connection is wedged
    /// and the recovery ladder should respawn.
    fn recv_with_idle_watchdog(
        &self,
        id: u64,
        method: &str,
        rx: &mpsc::Receiver<AcpResponse>,
        debug: bool,
    ) -> Result<AcpResponse> {
        // Poll cadence: short enough that a wedged connection is detected
        // promptly after the idle window elapses, long enough not to spin.
        const POLL_SLICE: Duration = Duration::from_secs(1);

        let mut last_seen_activity = *self.last_activity.lock_or_recover();
        let mut idle = Duration::ZERO;

        loop {
            match rx.recv_timeout(POLL_SLICE) {
                Ok(response) => {
                    if debug {
                        info!("[RECV] Response id={}", id);
                    }
                    return Ok(response);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let now = *self.last_activity.lock_or_recover();
                    if now != last_seen_activity {
                        // The backend emitted something (a streaming update on
                        // this or any session) — the turn is alive. Reset.
                        last_seen_activity = now;
                        idle = Duration::ZERO;
                        continue;
                    }
                    idle += POLL_SLICE;
                    if idle >= Self::PROMPT_IDLE_TIMEOUT {
                        self.pending.lock_or_recover().remove(&id);
                        anyhow::bail!(
                            "Timeout waiting for response to {} (no activity for {}s)",
                            method,
                            Self::PROMPT_IDLE_TIMEOUT.as_secs()
                        )
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.pending.lock_or_recover().remove(&id);
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
        // Drop every pending inbox sender so blocked send_request
        // callers wake immediately with a Disconnected error rather
        // than timing out 60s later. Same semantics as the reader
        // thread's EOF cleanup.
        self.pending.lock_or_recover().clear();
        let mut pm = self.process_manager.lock_or_recover();
        pm.terminate();
    }

    /// Full teardown: disconnect, kill process, drop pending request inboxes.
    /// Dropping the inboxes wakes any blocked `send_request` callers with a
    /// `Disconnected` error rather than letting them sit on the 60s timeout.
    pub fn force_disconnect(&self) {
        info!("Force-disconnecting ACP (full teardown)");
        *self.connected.lock_or_recover() = false;
        self.pending.lock_or_recover().clear();
        *self.pipe_stdin.lock_or_recover() = None;
        *self.tcp_writer.lock_or_recover() = None;
        let mut pm = self.process_manager.lock_or_recover();
        pm.terminate();
    }
}

#[cfg(test)]
mod tokenize_tests {
    use super::tokenize_spawn_command;

    #[test]
    fn unquoted_matches_split_whitespace() {
        // The common case must behave exactly like the old split_whitespace,
        // including the Ollama env-prefixed forms.
        assert_eq!(
            tokenize_spawn_command("kiro-cli acp"),
            vec!["kiro-cli", "acp"]
        );
        assert_eq!(
            tokenize_spawn_command("env OPENAI_MODEL=llama3:8b codex-acp"),
            vec!["env", "OPENAI_MODEL=llama3:8b", "codex-acp"]
        );
        // Collapses runs of whitespace, like split_whitespace.
        assert_eq!(tokenize_spawn_command("  a   b\tc  "), vec!["a", "b", "c"]);
    }

    #[test]
    fn quoted_program_path_with_spaces_stays_one_token() {
        assert_eq!(
            tokenize_spawn_command(r#""C:\Program Files\agent\kiro-cli.exe" acp"#),
            vec![r"C:\Program Files\agent\kiro-cli.exe", "acp"]
        );
        assert_eq!(
            tokenize_spawn_command("'/Applications/Some App/bin/agent' --acp"),
            vec!["/Applications/Some App/bin/agent", "--acp"]
        );
    }

    #[test]
    fn empty_input_yields_no_tokens() {
        assert!(tokenize_spawn_command("").is_empty());
        assert!(tokenize_spawn_command("   ").is_empty());
    }

    #[test]
    fn empty_quoted_string_is_a_token() {
        assert_eq!(
            tokenize_spawn_command(r#"prog "" x"#),
            vec!["prog", "", "x"]
        );
    }
}
