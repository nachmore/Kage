//! A program run in a pseudo-terminal the user types into: the sign-in
//! terminal (`commands::system::agent_sign_in`), where `kiro-cli login`'s
//! arrow-key menu needs a real TTY. `portable-pty` gives openpty on
//! macOS/Linux and ConPTY on Windows, so nothing here is platform-gated.
//!
//! The shape matches [`super::runner::run`]: one blocking loop that streams
//! output in batches, polls the child, the cancel flag and the clock, and
//! kills the process group on Stop or at the time limit. Input goes the
//! other way through [`TerminalIo`], which never blocks its caller. No
//! Tauri types, so all of it is unit-testable with `/bin/sh`.
//!
//! Nothing that passes through the terminal is logged: it can hold device
//! codes, sign-in URLs and whatever else the user types.

use super::runner::{OutputTail, RunControl, RunOutcome, Utf8Stream};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Output is handed to `on_output` at most this often: fast enough that
/// typing echoes without a visible lag.
pub const TERMINAL_EMIT_INTERVAL: Duration = Duration::from_millis(30);

/// Output waiting for the next batch is capped at this; anything older is
/// dropped and reported as skipped.
pub const TERMINAL_PENDING_BYTES: usize = 64 * 1024;

/// Longest single write into the terminal (a paste, usually).
pub const MAX_INPUT_BYTES: usize = 4096;

/// Writes queued for the terminal before [`TerminalIo::write`] reports
/// [`InputError::Full`] (the program isn't reading its input).
pub const INPUT_QUEUE_CHUNKS: usize = 64;

/// After the program exits, how long to keep reading what's left in the
/// terminal (a background job it left can hold it open).
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// How often the loop looks at the child, the cancel flag and the clock
/// when no output arrives.
const POLL: Duration = Duration::from_millis(15);

const READ_CHUNK: usize = 8 * 1024;

/// Chunks buffered between the reader thread and the loop.
const CHANNEL_CHUNKS: usize = 64;

pub const MIN_COLS: u16 = 20;
pub const MAX_COLS: u16 = 500;
pub const MIN_ROWS: u16 = 4;
pub const MAX_ROWS: u16 = 200;

/// The terminal's size in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

impl TerminalSize {
    /// Within [`MIN_COLS`]..=[`MAX_COLS`] and [`MIN_ROWS`]..=[`MAX_ROWS`].
    pub fn clamp(self) -> Self {
        Self {
            cols: self.cols.clamp(MIN_COLS, MAX_COLS),
            rows: self.rows.clamp(MIN_ROWS, MAX_ROWS),
        }
    }

    fn pty_size(self) -> PtySize {
        let size = self.clamp();
        PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// What to run in the terminal. Built by the caller from trusted values
/// only (for the sign-in: the resolved agent binary and `login`).
#[derive(Debug, Clone)]
pub struct TerminalSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Set on top of Kage's own environment.
    pub env: Vec<(String, String)>,
    pub size: TerminalSize,
}

/// Why a write into the terminal was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    /// Over [`MAX_INPUT_BYTES`].
    TooLong,
    /// [`INPUT_QUEUE_CHUNKS`] writes are already waiting.
    Full,
    /// The program has exited (or the terminal closed).
    Closed,
}

/// A bounded queue in front of a writer, drained by its own thread, so a
/// program that stops reading its input never blocks the caller.
pub struct InputQueue {
    tx: SyncSender<Vec<u8>>,
    closed: AtomicBool,
}

impl InputQueue {
    /// Start the writer thread. It ends when the queue is dropped or a
    /// write fails, dropping `writer`.
    pub fn new(mut writer: Box<dyn Write + Send>) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(INPUT_QUEUE_CHUNKS);
        std::thread::spawn(move || {
            while let Ok(chunk) = rx.recv() {
                if writer
                    .write_all(&chunk)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            tx,
            closed: AtomicBool::new(false),
        }
    }

    pub fn write(&self, data: &[u8]) -> Result<(), InputError> {
        if data.len() > MAX_INPUT_BYTES {
            return Err(InputError::TooLong);
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err(InputError::Closed);
        }
        if data.is_empty() {
            return Ok(());
        }
        match self.tx.try_send(data.to_vec()) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(InputError::Full),
            Err(TrySendError::Disconnected(_)) => Err(InputError::Closed),
        }
    }

    /// Refuse every later write (the program exited).
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// The live terminal's input side: what the window types and its size.
/// Dropping the last handle closes the terminal (on Windows that ends the
/// pseudoconsole, so the reader returns; on Unix anything left holding it
/// gets SIGHUP).
pub struct TerminalIo {
    input: InputQueue,
    master: Mutex<Box<dyn MasterPty + Send>>,
}

impl TerminalIo {
    /// Queue `data` for the program. Never blocks.
    pub fn write(&self, data: &[u8]) -> Result<(), InputError> {
        self.input.write(data)
    }

    /// Tell the program the terminal changed size (clamped).
    pub fn resize(&self, size: TerminalSize) -> io::Result<()> {
        let master = self
            .master
            .lock()
            .map_err(|_| io::Error::other("terminal lock poisoned"))?;
        master.resize(size.pty_size()).map_err(io::Error::other)
    }

    fn close(&self) {
        self.input.close();
    }

    /// A terminal with nothing running in it, for registry tests.
    #[cfg(test)]
    pub fn idle_for_tests() -> Arc<Self> {
        let pair = native_pty_system()
            .openpty(TerminalSize { cols: 80, rows: 24 }.pty_size())
            .expect("openpty");
        let writer = pair.master.take_writer().expect("writer");
        Arc::new(Self {
            input: InputQueue::new(writer),
            master: Mutex::new(pair.master),
        })
    }
}

/// How a terminal run ended. Carries no output: that only ever goes to
/// the window, through `on_output`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalReport {
    pub outcome: RunOutcome,
    /// The exit code; `None` when a signal ended it.
    pub exit_code: Option<i32>,
    /// The signal that ended it, by name (Unix).
    pub signal: Option<String>,
    pub duration_ms: u64,
    /// Everything the program printed, in bytes.
    pub output_bytes: u64,
}

/// The environment a terminal program gets on top of Kage's own: the
/// login-shell overlay (`os::login_env::overlay`, which must have been
/// asked to skip `TERM` and `COLORTERM`), then what an xterm.js view
/// renders.
pub fn terminal_env(overlay: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = overlay
        .into_iter()
        .filter(|(k, _)| k != "TERM" && k != "COLORTERM")
        .collect();
    env.push(("TERM".into(), "xterm-256color".into()));
    env.push(("COLORTERM".into(), "truecolor".into()));
    env
}

/// Kill what the program started: it leads its own session and process
/// group (portable-pty's `setsid`), so a group kill (SIGTERM, then
/// SIGKILL) reaches its children; on Windows `taskkill /T`. Only called
/// while the child is unreaped, so its pid can't have been reused.
fn kill_tree(child: &mut (dyn portable_pty::Child + Send + Sync)) {
    let killed = child
        .process_id()
        .is_some_and(crate::os::process::kill_process);
    if !killed {
        let _ = child.kill();
    }
}

fn report_parts(status: &portable_pty::ExitStatus) -> (Option<i32>, Option<String>) {
    match status.signal() {
        Some(sig) => (None, Some(sig.chars().take(64).collect())),
        None => (Some(status.exit_code() as i32), None),
    }
}

/// Run `spec` in a new pseudo-terminal until it exits, `control.cancel` is
/// set, or `control.timeout` passes (the last two kill its process
/// group). `on_started` gets the terminal's input side once the program
/// is running; `on_output(text, skipped_bytes)` gets what it prints at
/// most every [`TERMINAL_EMIT_INTERVAL`] (`skipped_bytes`: output that
/// didn't fit in one [`TERMINAL_PENDING_BYTES`] batch). Blocks: call from
/// a blocking thread.
pub fn run_terminal(
    spec: TerminalSpec,
    control: &RunControl,
    on_started: impl FnOnce(Arc<TerminalIo>),
    mut on_output: impl FnMut(&str, u64),
) -> io::Result<TerminalReport> {
    let pair = native_pty_system()
        .openpty(spec.size.pty_size())
        .map_err(io::Error::other)?;
    let mut cmd = CommandBuilder::new(&spec.program);
    cmd.args(&spec.args);
    cmd.cwd(&spec.cwd);
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }

    let started = Instant::now();
    let mut child = pair.slave.spawn_command(cmd).map_err(io::Error::other)?;
    // Our copy of the program's end: without dropping it the reader would
    // never see the terminal close when the program exits.
    drop(pair.slave);

    let reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            kill_tree(&mut *child);
            let _ = child.wait();
            return Err(io::Error::other(e));
        }
    };
    let writer = match pair.master.take_writer() {
        Ok(w) => w,
        Err(e) => {
            kill_tree(&mut *child);
            let _ = child.wait();
            return Err(io::Error::other(e));
        }
    };
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(CHANNEL_CHUNKS);
    spawn_reader(reader, tx);

    let io = Arc::new(TerminalIo {
        input: InputQueue::new(writer),
        master: Mutex::new(pair.master),
    });
    on_started(io.clone());

    let mut decoder = Utf8Stream::default();
    let mut pending = OutputTail::new(TERMINAL_PENDING_BYTES);
    let mut total: u64 = 0;
    let mut last_emit = Instant::now();
    let mut push = |bytes: &[u8], pending: &mut OutputTail| {
        total += bytes.len() as u64;
        let text = decoder.push(bytes);
        if !text.is_empty() {
            pending.push(&text);
        }
    };
    let mut emit = |pending: &mut OutputTail, last_emit: &mut Instant, force: bool| {
        if pending.is_empty() || (!force && last_emit.elapsed() < TERMINAL_EMIT_INTERVAL) {
            return;
        }
        let (text, skipped) = pending.take();
        on_output(&text, skipped);
        *last_emit = Instant::now();
    };

    let mut reader_done = false;
    let (outcome, status) = loop {
        if reader_done {
            std::thread::sleep(POLL);
        } else {
            match rx.recv_timeout(POLL) {
                Ok(bytes) => {
                    push(&bytes, &mut pending);
                    while let Ok(bytes) = rx.try_recv() {
                        push(&bytes, &mut pending);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => reader_done = true,
            }
        }
        emit(&mut pending, &mut last_emit, false);
        match child.try_wait() {
            Ok(Some(status)) => break (RunOutcome::Exited, status),
            Ok(None) => {}
            Err(e) => {
                io.close();
                kill_tree(&mut *child);
                let _ = child.wait();
                return Err(e);
            }
        }
        let stop = if control.cancel.load(Ordering::SeqCst) {
            Some(RunOutcome::Cancelled)
        } else if started.elapsed() >= control.timeout {
            Some(RunOutcome::TimedOut)
        } else {
            None
        };
        if let Some(outcome) = stop {
            io.close();
            kill_tree(&mut *child);
            break (outcome, child.wait()?);
        }
    };
    io.close();

    // What's still in the terminal (or in flight from a killed group).
    let deadline = Instant::now() + DRAIN_GRACE;
    while !reader_done {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok(bytes) => push(&bytes, &mut pending),
            Err(_) => reader_done = true,
        }
    }
    let rest = decoder.finish();
    if !rest.is_empty() {
        pending.push(&rest);
    }
    emit(&mut pending, &mut last_emit, true);

    let (exit_code, signal) = report_parts(&status);
    Ok(TerminalReport {
        outcome,
        exit_code,
        signal,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        output_bytes: total,
    })
}

fn spawn_reader(mut reader: Box<dyn Read + Send>, tx: SyncSender<Vec<u8>>) {
    std::thread::spawn(move || {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                // EIO on Linux once the program's end closes.
                Err(_) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests;
