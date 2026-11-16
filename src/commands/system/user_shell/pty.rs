//! A program run in a pseudo-terminal the user types into: the sign-in
//! terminal (`commands::system::agent_sign_in`), where `kiro-cli login`'s
//! arrow-key menu needs a real TTY. `portable-pty` gives openpty on
//! macOS/Linux and ConPTY on Windows.
//!
//! The shape matches [`super::runner::run`]: one blocking loop that streams
//! output, polls the child, the cancel flag and the clock, and kills the
//! process group on Stop or at the time limit. Input goes the other way
//! through [`TerminalIo`], which never blocks its caller. No Tauri types,
//! so all of it is unit-testable with `/bin/sh`.
//!
//! **Output is flow-controlled and sent in small messages.** Each message
//! is JSON shorter than [`CHANNEL_DIRECT_JSON_LIMIT`], so Tauri delivers it
//! by `eval` into the window that owns the channel and never parks it in
//! its app-wide channel cache, which any window could fetch from and which
//! nothing bounds. At most [`MAX_UNACKED_BYTES`] of messages are out before
//! the window acknowledges them ([`TerminalIo::ack`], after xterm has
//! written them); past that the loop stops reading the terminal, so a
//! window that stalls makes the program block on its writes instead of
//! making Kage buffer.
//!
//! **Teardown never blocks a caller and leaves nothing behind.** On Unix,
//! once the program has exited (or been stopped) and its output drained,
//! what is left of its process group is hung up and then killed, so no
//! descendant keeps the terminal (the sign-in starts no browser in it:
//! see [`end_process_group`]); the reader and writer use their own
//! descriptors and give up within 50 ms of the run ending. On
//! Windows the pseudo console is closed on a thread of its own while the
//! reader keeps draining its output pipe to the end: before Windows 11
//! 24H2, `ClosePseudoConsole` waits for that pipe to be read, so closing it
//! from a lock holder with nobody reading would hang forever.
//!
//! Nothing that passes through the terminal is logged: it can hold device
//! codes, sign-in URLs and whatever else the user types.

use super::runner::{RunControl, RunOutcome, Utf8Stream};
use crate::lock_ext::LockExt;
use log::warn;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Output is sent at most this often: fast enough that typing echoes
/// without a visible lag.
pub const TERMINAL_EMIT_INTERVAL: Duration = Duration::from_millis(30);

/// Tauri 2.10.3 hands a channel message to the webview that owns the
/// channel by `eval` only while its JSON is shorter than this
/// (`MAX_JSON_DIRECT_EXECUTE_THRESHOLD`, tauri `src/ipc/channel.rs:37`,
/// compared at `:156`). A longer one is stored in the app-wide
/// `ChannelDataIpcQueue` under a sequential id (`:169-180`) that
/// `plugin:__TAURI_CHANNEL__|fetch` hands to whichever window asks first,
/// with no check of who asked (`:318-337`), and that keeps it until then.
pub const CHANNEL_DIRECT_JSON_LIMIT: usize = 8192;

/// Longest output message, as JSON: strictly under the limit.
pub const MAX_MESSAGE_JSON_BYTES: usize = CHANNEL_DIRECT_JSON_LIMIT - 1;

/// Room kept in a message for `{"seq":…,"data":""}` (38 bytes at most).
const MESSAGE_OVERHEAD: usize = 64;

/// The most text a message carries, measured as escaped JSON.
const MESSAGE_TEXT_BUDGET: usize = MAX_MESSAGE_JSON_BYTES - MESSAGE_OVERHEAD;

/// Output messages sent to the window and not yet acknowledged, in JSON
/// bytes. A message is only sent if it fits whole, so this is a hard
/// bound (eight full messages).
pub const MAX_UNACKED_BYTES: usize = 64 * 1024;

/// Decoded output waiting to be sent. The loop takes no more from the
/// terminal once this much is waiting (one read can go over it).
pub const TERMINAL_PENDING_BYTES: usize = 64 * 1024;

/// Longest single write into the terminal (a paste, usually).
pub const MAX_INPUT_BYTES: usize = 4096;

/// Writes queued for the terminal before [`TerminalIo::write`] reports
/// [`InputError::Full`] (the program isn't reading its input).
pub const INPUT_QUEUE_CHUNKS: usize = 64;

/// After the program exits, how long to keep reading what's left in the
/// terminal (a background job it left can hold it open).
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// The same after Stop or the time limit killed it: only what was in
/// flight.
const STOP_DRAIN_GRACE: Duration = Duration::from_millis(250);

/// Between hanging up what's left of the process group and killing it
/// (Unix).
#[cfg(unix)]
const HANGUP_GRACE: Duration = Duration::from_millis(100);

/// How long the end of a run waits for the pseudo console to close and the
/// reader and writer threads to finish; past it they are left to finish
/// on their own (logged).
const TEARDOWN_LIMIT: Duration = Duration::from_secs(2);

/// How often the loop looks at the child, the cancel flag and the clock
/// when no output arrives.
const POLL: Duration = Duration::from_millis(15);

/// How often a waiting reader or writer looks at the stop flag (Unix).
#[cfg(unix)]
const IO_POLL: Duration = Duration::from_millis(50);

/// How often a reader holding output the loop has no room for tries again.
const SEND_RETRY: Duration = Duration::from_millis(5);

const READ_CHUNK: usize = 8 * 1024;

/// Chunks buffered between the reader thread and the loop: small, since
/// a full channel is what stops the reader.
const CHANNEL_CHUNKS: usize = 4;

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
/// only (for the sign-in: the resolved agent binary and its fixed
/// arguments).
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
    tx: Mutex<Option<SyncSender<Vec<u8>>>>,
    closed: AtomicBool,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl InputQueue {
    /// Start the writer thread. It ends when the queue shuts down (or is
    /// dropped) or a write fails, dropping `writer`.
    pub fn new(mut writer: Box<dyn Write + Send>) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(INPUT_QUEUE_CHUNKS);
        let work = move || {
            while let Ok(chunk) = rx.recv() {
                if writer
                    .write_all(&chunk)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        };
        let thread = std::thread::Builder::new()
            .name("kage-pty-write".into())
            .spawn(work)
            .ok();
        let closed = thread.is_none();
        Self {
            tx: Mutex::new(Some(tx)),
            closed: AtomicBool::new(closed),
            thread: Mutex::new(thread),
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
        let tx = self.tx.lock_or_recover();
        let Some(tx) = tx.as_ref() else {
            return Err(InputError::Closed);
        };
        match tx.try_send(data.to_vec()) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(InputError::Full),
            Err(TrySendError::Disconnected(_)) => Err(InputError::Closed),
        }
    }

    /// Refuse every later write (the program exited).
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    /// [`Self::close`], and let the writer thread end once it has nothing
    /// in hand: returns it to wait on.
    fn shutdown(&self) -> Option<JoinHandle<()>> {
        self.close();
        self.tx.lock_or_recover().take();
        self.thread.lock_or_recover().take()
    }
}

/// Output messages out to the window and not yet acknowledged.
#[derive(Default)]
struct FlowControl(Mutex<Unacked>);

#[derive(Default)]
struct Unacked {
    /// `(seq, json bytes)`, oldest first.
    sent: VecDeque<(u64, usize)>,
    bytes: usize,
}

impl FlowControl {
    /// Whether a full-size message still fits under [`MAX_UNACKED_BYTES`].
    fn has_room(&self) -> bool {
        self.0.lock_or_recover().bytes + MAX_MESSAGE_JSON_BYTES <= MAX_UNACKED_BYTES
    }

    fn record(&self, seq: u64, bytes: usize) {
        let mut u = self.0.lock_or_recover();
        u.sent.push_back((seq, bytes));
        u.bytes += bytes;
    }

    /// Message `seq` and everything before it were shown. An id that was
    /// never sent frees nothing it doesn't cover.
    fn ack(&self, seq: u64) {
        let mut u = self.0.lock_or_recover();
        while u.sent.front().is_some_and(|(s, _)| *s <= seq) {
            if let Some((_, bytes)) = u.sent.pop_front() {
                u.bytes -= bytes;
            }
        }
    }

    #[cfg(test)]
    fn unacked_bytes(&self) -> usize {
        self.0.lock_or_recover().bytes
    }
}

/// The live terminal's window side: what the window types, its size, and
/// its acknowledgements of the output it was sent. The run itself closes
/// the terminal when it ends; after that every call here is refused. If
/// the last handle goes first (a panic), the terminal is closed on a
/// thread of its own, never by whoever dropped it.
pub struct TerminalIo {
    input: InputQueue,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    flow: FlowControl,
}

impl TerminalIo {
    fn new(writer: Box<dyn Write + Send>, master: Box<dyn MasterPty + Send>) -> Self {
        Self {
            input: InputQueue::new(writer),
            master: Mutex::new(Some(master)),
            flow: FlowControl::default(),
        }
    }

    /// Queue `data` for the program. Never blocks.
    pub fn write(&self, data: &[u8]) -> Result<(), InputError> {
        self.input.write(data)
    }

    /// Tell the program the terminal changed size (clamped).
    pub fn resize(&self, size: TerminalSize) -> io::Result<()> {
        let master = self.master.lock_or_recover();
        let Some(master) = master.as_ref() else {
            return Err(io::Error::other("terminal closed"));
        };
        master.resize(size.pty_size()).map_err(io::Error::other)
    }

    /// The window has written output message `seq` and every one before
    /// it: room for more.
    pub fn ack(&self, seq: u64) {
        self.flow.ack(seq);
    }

    /// Output bytes (as JSON) the window hasn't acknowledged.
    #[cfg(test)]
    pub fn unacked_bytes(&self) -> usize {
        self.flow.unacked_bytes()
    }

    fn close(&self) {
        self.input.close();
    }

    /// End the window side: no more input, the writer thread let go, and
    /// the pseudo-terminal handed over to be closed.
    fn detach(&self) -> (Option<Box<dyn MasterPty + Send>>, Option<JoinHandle<()>>) {
        let writer = self.input.shutdown();
        (self.master.lock_or_recover().take(), writer)
    }

    /// A terminal with nothing running in it, for registry tests.
    #[cfg(test)]
    pub fn idle_for_tests() -> Arc<Self> {
        let pair = native_pty_system()
            .openpty(TerminalSize { cols: 80, rows: 24 }.pty_size())
            .expect("openpty");
        let writer = pair.master.take_writer().expect("writer");
        Arc::new(Self::new(writer, pair.master))
    }

    /// A terminal around `master` (a fake), for teardown tests.
    #[cfg(test)]
    pub fn with_master_for_tests(master: Box<dyn MasterPty + Send>) -> Arc<Self> {
        Arc::new(Self::new(Box::new(io::sink()), master))
    }
}

impl Drop for TerminalIo {
    fn drop(&mut self) {
        let master = match self.master.get_mut() {
            Ok(m) => m.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(master) = master {
            // Never close the pseudo console on the dropping thread: it can
            // hold registry locks, and ConPTY's close can wait.
            let _ = close_in_background(master);
        }
    }
}

/// Close `master` on a thread of its own (on Windows that is
/// `ClosePseudoConsole`). Returns the thread, or `None` when it closed
/// inline because no thread could be started.
fn close_in_background(master: Box<dyn MasterPty + Send>) -> Option<JoinHandle<()>> {
    let slot = Arc::new(Mutex::new(Some(master)));
    let theirs = slot.clone();
    match std::thread::Builder::new()
        .name("kage-pty-close".into())
        .spawn(move || drop(theirs.lock_or_recover().take()))
    {
        Ok(handle) => Some(handle),
        Err(_) => {
            drop(slot.lock_or_recover().take());
            None
        }
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
    /// Everything the program printed that Kage read, in bytes.
    pub output_bytes: u64,
}

/// One output message, as sent: `seq` counts from 0 within the run (the
/// window acknowledges by it), `data` is text for the terminal.
#[derive(Debug, Serialize)]
struct TerminalOutput<'a> {
    seq: u64,
    data: &'a str,
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

/// `c`'s length inside a JSON string as serde_json writes it.
fn json_char_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// Split off the longest run of whole characters at the front of
/// `pending` whose JSON form takes at most `budget` bytes.
fn take_message_text(pending: &mut String, budget: usize) -> String {
    let mut used = 0;
    let mut end = 0;
    for (i, c) in pending.char_indices() {
        let len = json_char_len(c);
        if used + len > budget {
            break;
        }
        used += len;
        end = i + c.len_utf8();
    }
    let rest = pending.split_off(end);
    std::mem::replace(pending, rest)
}

/// The next message from `pending` as JSON: under
/// [`CHANNEL_DIRECT_JSON_LIMIT`], however the text escapes. `None` when
/// nothing is pending.
fn next_message(pending: &mut String, seq: u64) -> Option<String> {
    let mut budget = MESSAGE_TEXT_BUDGET;
    while !pending.is_empty() {
        let data = take_message_text(pending, budget);
        let json = serde_json::to_string(&TerminalOutput { seq, data: &data })
            .unwrap_or_else(|_| String::from("{}"));
        if json.len() <= MAX_MESSAGE_JSON_BYTES {
            return Some(json);
        }
        // Can't happen with the exact lengths above; never send it anyway.
        pending.insert_str(0, &data);
        budget /= 2;
    }
    None
}

/// Moves output from the reader to the window: decodes it, holds at most
/// [`TERMINAL_PENDING_BYTES`], and sends it in messages while the window
/// has room ([`FlowControl`]).
struct Pump<'a, F: FnMut(String)> {
    rx: Receiver<Vec<u8>>,
    reader_done: bool,
    decoder: Utf8Stream,
    pending: String,
    total: u64,
    seq: u64,
    last_emit: Instant,
    flow: &'a FlowControl,
    on_output: F,
}

impl<'a, F: FnMut(String)> Pump<'a, F> {
    fn new(rx: Receiver<Vec<u8>>, flow: &'a FlowControl, on_output: F) -> Self {
        Self {
            rx,
            reader_done: false,
            decoder: Utf8Stream::default(),
            pending: String::new(),
            total: 0,
            seq: 0,
            last_emit: Instant::now(),
            flow,
            on_output,
        }
    }

    fn has_room(&self) -> bool {
        self.pending.len() < TERMINAL_PENDING_BYTES
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len() as u64;
        let text = self.decoder.push(bytes);
        self.pending.push_str(&text);
    }

    /// Take output for up to `wait` (only while there's room for it: a
    /// full pump leaves the reader blocked, and so the program), then send
    /// what the window has room for.
    fn step(&mut self, wait: Duration) {
        if self.reader_done || !self.has_room() {
            std::thread::sleep(wait);
        } else {
            match self.rx.recv_timeout(wait) {
                Ok(bytes) => {
                    self.push(&bytes);
                    while self.has_room() {
                        match self.rx.try_recv() {
                            Ok(bytes) => self.push(&bytes),
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                self.reader_done = true;
                                break;
                            }
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => self.reader_done = true,
            }
        }
        self.emit(false);
    }

    /// Take output for up to `wait` and drop it: the run is being stopped
    /// and the program must be able to finish writing to exit. What was
    /// already pending is still sent, as far as the window has room.
    fn step_discarding(&mut self, wait: Duration) {
        if !self.reader_done {
            match self.rx.recv_timeout(wait) {
                Ok(bytes) => self.total += bytes.len() as u64,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => self.reader_done = true,
            }
            while let Ok(bytes) = self.rx.try_recv() {
                self.total += bytes.len() as u64;
            }
        } else {
            std::thread::sleep(wait);
        }
        self.emit(true);
    }

    fn emit(&mut self, force: bool) {
        if self.pending.is_empty() || (!force && self.last_emit.elapsed() < TERMINAL_EMIT_INTERVAL)
        {
            return;
        }
        let mut sent = false;
        while self.flow.has_room() {
            let Some(json) = next_message(&mut self.pending, self.seq) else {
                break;
            };
            self.flow.record(self.seq, json.len());
            self.seq += 1;
            (self.on_output)(json);
            sent = true;
        }
        if sent {
            self.last_emit = Instant::now();
        }
    }

    /// The decoder's last partial character, then a send regardless of
    /// the interval.
    fn finish(&mut self) {
        let rest = self.decoder.finish();
        self.pending.push_str(&rest);
        self.emit(true);
    }

    fn drained(&self) -> bool {
        self.reader_done && self.pending.is_empty()
    }
}

/// What the reader does once the run is over (`stop` set).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterStop {
    /// Return (Unix: its descriptor closes with it).
    #[cfg_attr(not(unix), allow(dead_code))]
    Exit,
    /// Keep reading, discarding, until the end of the output (Windows:
    /// `ClosePseudoConsole` waits for the output pipe to be read).
    #[cfg_attr(unix, allow(dead_code))]
    DrainToEof,
}

/// Read the terminal into `tx` until it ends. While the loop has no room
/// the reader waits with its chunk in hand (so the program blocks on its
/// writes); once `stop` is set nothing more is delivered.
fn read_loop(
    mut reader: Box<dyn Read + Send>,
    tx: SyncSender<Vec<u8>>,
    stop: &AtomicBool,
    after_stop: AfterStop,
) {
    let mut buf = vec![0u8; READ_CHUNK];
    let mut detached = false;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            // EIO on Linux once the program's end closes; on Unix also the
            // stop flag (`unix_io::PtyEnd`).
            Err(_) => break,
        };
        let over = || detached || stop.load(Ordering::SeqCst);
        if over() {
            match after_stop {
                AfterStop::Exit => break,
                AfterStop::DrainToEof => continue,
            }
        }
        let mut chunk = buf[..n].to_vec();
        loop {
            match tx.try_send(chunk) {
                Ok(()) => break,
                Err(TrySendError::Full(back)) => {
                    if over() {
                        break;
                    }
                    chunk = back;
                    std::thread::sleep(SEND_RETRY);
                }
                Err(TrySendError::Disconnected(_)) => {
                    detached = true;
                    break;
                }
            }
        }
    }
}

fn spawn_reader(
    reader: Box<dyn Read + Send>,
    tx: SyncSender<Vec<u8>>,
    stop: Arc<AtomicBool>,
    after_stop: AfterStop,
) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("kage-pty-read".into())
        .spawn(move || read_loop(reader, tx, &stop, after_stop))
}

/// The terminal's output and input ends, and what the reader does after
/// the run.
type Ends = (Box<dyn Read + Send>, Box<dyn Write + Send>, AfterStop);

/// Unix: copies of the master descriptor of our own, which give up once
/// the run is over, so neither thread can outlive it blocked in `read` or
/// `write` on a terminal a stray process still holds.
#[cfg(unix)]
fn open_ends(master: &dyn MasterPty, stop: &Arc<AtomicBool>) -> io::Result<Ends> {
    let fd = master
        .as_raw_fd()
        .ok_or_else(|| io::Error::other("terminal has no descriptor"))?;
    let reader = unix_io::PtyEnd::dup(fd, stop.clone())?;
    let writer = unix_io::PtyEnd::dup(fd, stop.clone())?;
    Ok((Box::new(reader), Box::new(writer), AfterStop::Exit))
}

/// Windows: ConPTY's pipes. The reader returns at the end of the output,
/// which comes once the pseudo console is closed.
#[cfg(not(unix))]
fn open_ends(master: &dyn MasterPty, _stop: &Arc<AtomicBool>) -> io::Result<Ends> {
    let reader = master.try_clone_reader().map_err(io::Error::other)?;
    let writer = master.take_writer().map_err(io::Error::other)?;
    Ok((reader, writer, AfterStop::DrainToEof))
}

#[cfg(unix)]
mod unix_io {
    use super::IO_POLL;
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// A non-blocking copy of the terminal's master descriptor that waits
    /// in `poll` with a timeout, so it can give up when `stop` is set.
    pub struct PtyEnd {
        fd: OwnedFd,
        stop: Arc<AtomicBool>,
    }

    fn closed() -> io::Error {
        io::Error::new(io::ErrorKind::BrokenPipe, "terminal closed")
    }

    impl PtyEnd {
        pub fn dup(fd: RawFd, stop: Arc<AtomicBool>) -> io::Result<Self> {
            // SAFETY: F_DUPFD_CLOEXEC only reads `fd` and returns a new
            // descriptor (or -1), which we then own.
            let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
            if copy < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `copy` is a fresh descriptor nothing else owns.
            let fd = unsafe { OwnedFd::from_raw_fd(copy) };
            // O_NONBLOCK is on the open file, shared with the master (only
            // ever resized through ioctl), not with the program's end.
            // SAFETY: plain flag calls on a descriptor we own.
            let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
            if flags < 0
                || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                    < 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { fd, stop })
        }

        /// Wait up to [`IO_POLL`] for `events`; `Ok(true)` once ready (or
        /// hung up: the next call says how).
        fn ready(&self, events: libc::c_short) -> io::Result<bool> {
            if self.stop.load(Ordering::SeqCst) {
                return Err(closed());
            }
            let mut p = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events,
                revents: 0,
            };
            let ms = IO_POLL.as_millis() as libc::c_int;
            // SAFETY: one valid pollfd for the duration of the call.
            let r = unsafe { libc::poll(&mut p, 1, ms) };
            if r < 0 {
                let e = io::Error::last_os_error();
                return if e.kind() == io::ErrorKind::Interrupted {
                    Ok(false)
                } else {
                    Err(e)
                };
            }
            Ok(r > 0)
        }
    }

    fn retry(e: &io::Error) -> bool {
        matches!(
            e.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
        )
    }

    impl Read for PtyEnd {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            loop {
                if !self.ready(libc::POLLIN)? {
                    continue;
                }
                // SAFETY: `buf` is valid for `buf.len()` bytes.
                let n =
                    unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                if n >= 0 {
                    return Ok(n as usize);
                }
                let e = io::Error::last_os_error();
                if !retry(&e) {
                    return Err(e);
                }
            }
        }
    }

    impl Write for PtyEnd {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            loop {
                if !self.ready(libc::POLLOUT)? {
                    continue;
                }
                // SAFETY: `buf` is valid for `buf.len()` bytes.
                let n = unsafe { libc::write(self.fd.as_raw_fd(), buf.as_ptr().cast(), buf.len()) };
                if n >= 0 {
                    return Ok(n as usize);
                }
                let e = io::Error::last_os_error();
                if !retry(&e) {
                    return Err(e);
                }
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

type ChildBox = Box<dyn portable_pty::Child + Send + Sync>;

/// Kill what the program started: it leads its own session and process
/// group (portable-pty's `setsid`), so a group kill (SIGTERM, then
/// SIGKILL) reaches its children; on Windows `taskkill /T`. Only called
/// while the child is unreaped, so its pid can't have been reused. The
/// sign-in leaves no browser in either (`agent_sign_in::sign_in_env_for`:
/// on Windows kiro-cli would otherwise start it with `cmd /c start`, as
/// its descendant).
///
/// On Unix the fallback is a plain SIGKILL, not portable-pty's `kill`,
/// which reaps the child: it must stay unreaped until
/// [`end_process_group`]. (macOS's `killpg` answers EPERM once only the
/// zombie leader is left, so `kill_process` can report failure for a
/// group that is in fact gone.)
fn kill_tree(child: &mut (dyn portable_pty::Child + Send + Sync)) {
    let pid = child.process_id();
    if pid.is_some_and(crate::os::process::kill_process) {
        return;
    }
    #[cfg(unix)]
    if let Some(raw) = pid.and_then(|p| i32::try_from(p).ok()).filter(|p| *p > 1) {
        // SAFETY: plain kill(2) of our own unreaped child.
        unsafe { libc::kill(raw, libc::SIGKILL) };
        return;
    }
    let _ = child.kill();
}

/// Whether the child has exited. On Unix it is left unreaped (a zombie),
/// so its pid, which is also the id of its process group, can't be reused
/// before [`end_process_group`]; `wait` reaps it.
#[cfg(unix)]
fn has_exited(child: &mut ChildBox) -> io::Result<bool> {
    let Some(pid) = child.process_id() else {
        return child.try_wait().map(|s| s.is_some());
    };
    loop {
        // SAFETY: an all-zero siginfo_t is a valid out-parameter.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let flags = libc::WEXITED | libc::WNOWAIT | libc::WNOHANG;
        // SAFETY: `info` outlives the call; WNOWAIT leaves the child be.
        let r = unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, flags) };
        if r == 0 {
            // SAFETY: waitid filled `info` (si_pid 0: still running).
            return Ok(unsafe { info.si_pid() } != 0);
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINTR) => continue,
            // Already reaped through `child` itself, which keeps the status.
            Some(libc::ECHILD) => return child.try_wait().map(|s| s.is_some()),
            _ => return Err(e),
        }
    }
}

#[cfg(not(unix))]
fn has_exited(child: &mut ChildBox) -> io::Result<bool> {
    child.try_wait().map(|s| s.is_some())
}

/// End what is left of the program's process group once it has exited
/// and its output drained: SIGHUP (what a closing terminal sends), then
/// SIGKILL for anything that ignored it, so nothing keeps the terminal
/// (and the reader) going. `pgid` is the program's pid (portable-pty runs
/// it under `setsid`), still unreaped, so the group id can't name another.
///
/// Nothing the user needs is in that group, on any platform: on Linux
/// (and Windows) the sign-in runs `kiro-cli login` with
/// `KIRO_FAKE_IS_REMOTE` set (`agent_sign_in::sign_in_env_for`), so
/// kiro-cli takes the device flow and only prints the URL and code, never
/// running `xdg-open`; the user opens the link through Kage's own process
/// (`open_user_terminal_link`), outside this session. On macOS kiro-cli
/// opens URLs through LaunchServices (`NSWorkspace openURL`), so launchd
/// starts the browser, never as the program's descendant.
#[cfg(unix)]
fn end_process_group(pgid: u32) {
    use nix::sys::signal::{killpg, Signal};
    let Ok(raw) = i32::try_from(pgid) else {
        return;
    };
    if raw <= 1 {
        return;
    }
    let group = nix::unistd::Pid::from_raw(raw);
    // EPERM (not ESRCH) on macOS when only the zombie leader is left: go on.
    if killpg(group, Signal::SIGHUP) == Err(nix::errno::Errno::ESRCH) {
        return;
    }
    std::thread::sleep(HANGUP_GRACE);
    let _ = killpg(group, Signal::SIGKILL);
}

fn report_parts(status: &portable_pty::ExitStatus) -> (Option<i32>, Option<String>) {
    match status.signal() {
        Some(sig) => (None, Some(sig.chars().take(64).collect())),
        None => (Some(status.exit_code() as i32), None),
    }
}

/// How the end of a run went, for tests.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Teardown {
    /// The pseudo console closed and the reader and writer threads ended
    /// within [`TEARDOWN_LIMIT`].
    pub finished: bool,
}

/// End the terminal: nothing more is delivered, the window side is
/// detached, the pseudo console is closed on its own thread while the
/// reader drains (Windows) or gives up (Unix), and all of it is waited on
/// for at most [`TEARDOWN_LIMIT`].
fn teardown(io: &TerminalIo, stop: &AtomicBool, reader: Option<JoinHandle<()>>) -> Teardown {
    stop.store(true, Ordering::SeqCst);
    let (master, writer) = io.detach();
    let closer = master.and_then(close_in_background);
    let threads: Vec<JoinHandle<()>> = [reader, writer, closer].into_iter().flatten().collect();
    let deadline = Instant::now() + TEARDOWN_LIMIT;
    while threads.iter().any(|t| !t.is_finished()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let finished = threads.iter().all(JoinHandle::is_finished);
    for t in threads {
        // Unfinished ones are left to end on their own.
        if t.is_finished() {
            let _ = t.join();
        }
    }
    Teardown { finished }
}

/// Run `spec` in a new pseudo-terminal until it exits, `control.cancel` is
/// set, or `control.timeout` passes (the last two kill its process
/// group). `on_started` gets the terminal's window side once the program
/// is running; `on_output(json)` gets what it prints as messages
/// (`{"seq":N,"data":"…"}`, each under [`CHANNEL_DIRECT_JSON_LIMIT`]
/// bytes), at most [`MAX_UNACKED_BYTES`] of them ahead of
/// [`TerminalIo::ack`]. Returns once the terminal is closed and its
/// threads are done (or [`TEARDOWN_LIMIT`] passed, logged). Blocks: call
/// from a blocking thread.
pub fn run_terminal(
    spec: TerminalSpec,
    control: &RunControl,
    on_started: impl FnOnce(Arc<TerminalIo>),
    on_output: impl FnMut(String),
) -> io::Result<TerminalReport> {
    let (report, teardown) = run_terminal_inner(spec, control, on_started, on_output)?;
    if !teardown.finished {
        warn!(
            "Terminal run: the pseudo console was still closing after {} ms; left to finish in the background",
            TEARDOWN_LIMIT.as_millis()
        );
    }
    Ok(report)
}

pub(crate) fn run_terminal_inner(
    spec: TerminalSpec,
    control: &RunControl,
    on_started: impl FnOnce(Arc<TerminalIo>),
    on_output: impl FnMut(String),
) -> io::Result<(TerminalReport, Teardown)> {
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

    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(CHANNEL_CHUNKS);
    let started_io = open_ends(&*pair.master, &stop).and_then(|(reader, writer, after)| {
        let thread = spawn_reader(reader, tx, stop.clone(), after)?;
        Ok((thread, writer))
    });
    let (reader_thread, writer) = match started_io {
        Ok(parts) => parts,
        Err(e) => {
            kill_tree(&mut *child);
            let _ = child.wait();
            stop.store(true, Ordering::SeqCst);
            drop(close_in_background(pair.master));
            return Err(e);
        }
    };
    let io = Arc::new(TerminalIo::new(writer, pair.master));
    on_started(io.clone());

    let mut pump = Pump::new(rx, &io.flow, on_output);
    let outcome = loop {
        pump.step(POLL);
        match has_exited(&mut child) {
            Ok(true) => break Ok(RunOutcome::Exited),
            Ok(false) => {}
            Err(e) => {
                kill_tree(&mut *child);
                break Err(e);
            }
        }
        let stop_now = if control.cancel.load(Ordering::SeqCst) {
            Some(RunOutcome::Cancelled)
        } else if started.elapsed() >= control.timeout {
            Some(RunOutcome::TimedOut)
        } else {
            None
        };
        if let Some(outcome) = stop_now {
            io.close();
            kill_tree(&mut *child);
            // Keep reading while it goes: a program can't finish exiting
            // while its output waits to be read (macOS drains a terminal's
            // output when its last descriptor closes), and nobody is
            // waiting for that output any more.
            break loop {
                match has_exited(&mut child) {
                    Ok(true) => break Ok(outcome),
                    Ok(false) => pump.step_discarding(POLL),
                    Err(e) => break Err(e),
                }
            };
        }
    };
    io.close();

    // What's still in the terminal (or in flight from a killed group),
    // for as long as the window takes it.
    let grace = match outcome {
        Ok(RunOutcome::Exited) => DRAIN_GRACE,
        _ => STOP_DRAIN_GRACE,
    };
    let deadline = Instant::now() + grace;
    while !pump.drained() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        pump.step(left.min(POLL));
    }
    pump.finish();
    let output_bytes = pump.total;

    #[cfg(unix)]
    if let Some(pid) = child.process_id() {
        end_process_group(pid);
    }
    let status = child.wait();
    let ended = teardown(&io, &stop, Some(reader_thread));
    drop(pump);

    let outcome = outcome?;
    let (exit_code, signal) = report_parts(&status?);
    Ok((
        TerminalReport {
            outcome,
            exit_code,
            signal,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            output_bytes,
        },
        ended,
    ))
}

#[cfg(test)]
mod tests;
