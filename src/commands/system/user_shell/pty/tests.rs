//! The PTY loop with harmless programs only (`/bin/sh -c` builtins,
//! `sleep`, `stty`, `head`; `cmd /c exit` on Windows). Nothing here
//! starts an agent or signs in to anything.

use super::*;
use std::sync::atomic::AtomicBool;

fn control(timeout: Duration) -> RunControl {
    RunControl {
        cancel: Arc::new(AtomicBool::new(false)),
        timeout,
    }
}

#[cfg(unix)]
fn sh(script: &str) -> TerminalSpec {
    TerminalSpec {
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".into(), script.into()],
        cwd: std::env::temp_dir(),
        env: terminal_env(Vec::new()),
        size: TerminalSize { cols: 80, rows: 24 },
    }
}

/// One output message, parsed: `(seq, data)`.
fn parse(json: &str) -> (u64, String) {
    let v: serde_json::Value = serde_json::from_str(json).expect("message is JSON");
    let seq = v["seq"].as_u64().expect("seq");
    let data = v["data"].as_str().expect("data").to_string();
    (seq, data)
}

/// Run `spec`, handing the terminal to `started` and acknowledging every
/// message as it arrives (a window that keeps up); returns the report,
/// all the output, and the terminal (to poke at after the run).
fn run_collect(
    spec: TerminalSpec,
    control: &RunControl,
    started: impl FnOnce(&Arc<TerminalIo>),
) -> (TerminalReport, String, Arc<TerminalIo>) {
    let mut out = String::new();
    let handle = std::cell::OnceCell::new();
    let report = run_terminal(
        spec,
        control,
        |io| {
            started(&io);
            let _ = handle.set(io);
        },
        |json| {
            let (seq, data) = parse(&json);
            out.push_str(&data);
            handle.get().expect("on_started ran first").ack(seq);
        },
    )
    .expect("terminal run");
    (report, out, handle.into_inner().expect("on_started ran"))
}

#[test]
fn terminal_size_clamped() {
    assert_eq!(
        TerminalSize { cols: 0, rows: 0 }.clamp(),
        TerminalSize { cols: 20, rows: 4 }
    );
    assert_eq!(
        TerminalSize {
            cols: 9999,
            rows: 9999
        }
        .clamp(),
        TerminalSize {
            cols: 500,
            rows: 200
        }
    );
    assert_eq!(
        TerminalSize { cols: 80, rows: 24 }.clamp(),
        TerminalSize { cols: 80, rows: 24 }
    );
}

#[test]
fn terminal_env_sets_term_and_drops_overlay_copies() {
    let env = terminal_env(vec![
        ("PATH".into(), "/x".into()),
        ("TERM".into(), "dumb".into()),
    ]);
    assert!(env.contains(&("PATH".into(), "/x".into())));
    assert!(env.contains(&("TERM".into(), "xterm-256color".into())));
    assert!(env.contains(&("COLORTERM".into(), "truecolor".into())));
    assert_eq!(env.iter().filter(|(k, _)| k == "TERM").count(), 1);
}

/// A writer that never finishes a write until it is dropped: a program
/// that stopped reading its input.
struct Stuck(mpsc::Receiver<()>);

impl Write for Stuck {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        let _ = self.0.recv();
        Err(io::Error::other("closed"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn input_limits() {
    let (_hold, rx) = mpsc::channel::<()>();
    let queue = InputQueue::new(Box::new(Stuck(rx)));
    assert_eq!(
        queue.write(&vec![b'a'; MAX_INPUT_BYTES + 1]),
        Err(InputError::TooLong)
    );
    assert_eq!(queue.write(&vec![b'a'; MAX_INPUT_BYTES]), Ok(()));
    // The writer thread holds at most one chunk; the queue the rest.
    let mut full = false;
    for _ in 0..(INPUT_QUEUE_CHUNKS + 2) {
        if queue.write(b"x") == Err(InputError::Full) {
            full = true;
            break;
        }
    }
    assert!(full, "a queue nobody drains fills up");
    queue.close();
    assert_eq!(queue.write(b"x"), Err(InputError::Closed));
}

#[test]
fn input_queue_reports_closed_once_the_writer_fails() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("gone"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let queue = InputQueue::new(Box::new(Broken));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match queue.write(b"x") {
            Err(InputError::Closed) => break,
            _ if Instant::now() > deadline => panic!("writer thread never ended"),
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[cfg(unix)]
#[test]
fn pty_reports_exit_code() {
    let (report, _, _) = run_collect(sh("exit 3"), &control(Duration::from_secs(10)), |_| {});
    assert_eq!(report.outcome, RunOutcome::Exited);
    assert_eq!(report.exit_code, Some(3));
    assert_eq!(report.signal, None);
}

#[cfg(unix)]
#[test]
fn pty_is_a_tty() {
    let (report, out, _) = run_collect(
        sh("[ -t 0 ] && [ -t 1 ] && echo is-a-tty"),
        &control(Duration::from_secs(10)),
        |_| {},
    );
    assert_eq!(report.exit_code, Some(0));
    assert!(out.contains("is-a-tty"), "{out:?}");
}

/// The smoke test: an interactive program reads what the window types.
#[cfg(unix)]
#[test]
fn pty_input_reaches_child() {
    let (report, out, io) = run_collect(
        sh("read x; echo got:$x"),
        &control(Duration::from_secs(10)),
        |io| io.write(b"hi\r").unwrap(),
    );
    assert_eq!(report.outcome, RunOutcome::Exited);
    assert_eq!(report.exit_code, Some(0));
    assert!(out.contains("got:hi"), "{out:?}");
    // The program is gone: typing now is refused, not queued.
    assert_eq!(io.write(b"x"), Err(InputError::Closed));
    assert_eq!(
        io.write(&vec![b'a'; MAX_INPUT_BYTES + 1]),
        Err(InputError::TooLong)
    );
}

#[cfg(unix)]
#[test]
fn pty_resize_seen_by_child() {
    let (_, out, _) = run_collect(
        sh("read _; stty size"),
        &control(Duration::from_secs(10)),
        |io| {
            io.resize(TerminalSize {
                cols: 100,
                rows: 30,
            })
            .unwrap();
            io.write(b"\r").unwrap();
        },
    );
    assert!(out.contains("30 100"), "{out:?}");
}

#[cfg(unix)]
#[test]
fn pty_env_sets_term() {
    let (_, out, _) = run_collect(
        sh("echo term=$TERM"),
        &control(Duration::from_secs(10)),
        |_| {},
    );
    assert!(out.contains("term=xterm-256color"), "{out:?}");
}

#[cfg(unix)]
#[test]
fn pty_cancel_kills_process_group() {
    let dir = std::env::temp_dir().join(format!("kage-pty-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let pid_file = dir.join("pid");
    let ctl = control(Duration::from_secs(60));
    let cancel = ctl.cancel.clone();
    let watch = pid_file.clone();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !watch.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(100));
        cancel.store(true, Ordering::SeqCst);
    });
    let script = format!("sleep 30 & echo $! > '{}'; wait", pid_file.display());
    let t0 = Instant::now();
    let (report, _, _) = run_collect(sh(&script), &ctl, |_| {});
    assert_eq!(report.outcome, RunOutcome::Cancelled);
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // The background `sleep` went with the group (allow for its reaping).
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        // SAFETY: signal 0 only probes for existence.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        if !alive {
            break;
        }
        assert!(Instant::now() < deadline, "background job survived Stop");
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Stop pressed while the run was still being prepared: nothing starts,
/// and the run ends as stopped, never as a clean exit (which would resend
/// the failed turn).
#[cfg(unix)]
#[test]
fn a_stop_latched_before_launch_starts_nothing() {
    let dir = std::env::temp_dir().join(format!("kage-pty-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let marker = dir.join("ran");
    let ctl = control(Duration::from_secs(60));
    ctl.cancel.store(true, Ordering::SeqCst);
    let mut started = false;
    let report = run_terminal(
        sh(&format!("touch '{}'", marker.display())),
        &ctl,
        |_| started = true,
        |_| {},
    )
    .expect("terminal run");
    assert_eq!(report.outcome, RunOutcome::Cancelled);
    assert_eq!(report.exit_code, None);
    assert!(!started, "no terminal was handed out");
    std::thread::sleep(Duration::from_millis(200));
    assert!(!marker.exists(), "the program ran despite the Stop");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Stop pressed while the program was already on its way out with 0 (here:
/// the moment the terminal starts, on a program that exits at once): the
/// loop sees the exit and the Stop together, and it is still a stop.
#[cfg(unix)]
#[test]
fn a_stop_that_races_a_clean_exit_is_still_a_stop() {
    let ctl = control(Duration::from_secs(60));
    let cancel = ctl.cancel.clone();
    let (report, _, _) = run_collect(sh("exit 0"), &ctl, |_| {
        cancel.store(true, Ordering::SeqCst);
    });
    assert_eq!(report.outcome, RunOutcome::Cancelled, "{report:?}");
}

#[cfg(unix)]
#[test]
fn pty_timeout() {
    let t0 = Instant::now();
    let (report, _, _) = run_collect(sh("sleep 5"), &control(Duration::from_millis(300)), |_| {});
    assert_eq!(report.outcome, RunOutcome::TimedOut);
    assert!(t0.elapsed() < Duration::from_secs(4), "{:?}", t0.elapsed());
}

// --- Output messages and flow control (S1, S4) --------------------------

/// Every message is JSON shorter than Tauri's direct-eval threshold, so it
/// goes by `eval` to the owning webview only, never to the shared channel
/// cache; and nothing is lost or reordered when the window keeps up.
#[cfg(unix)]
#[test]
fn pty_messages_stay_under_the_direct_eval_threshold() {
    let mut largest = 0usize;
    let mut next_seq = 0u64;
    let mut out_len = 0u64;
    let handle = std::cell::OnceCell::<Arc<TerminalIo>>::new();
    let report = run_terminal(
        sh("head -c 2000000 /dev/zero | tr '\\0' a"),
        &control(Duration::from_secs(60)),
        |io| {
            let _ = handle.set(io);
        },
        |json| {
            largest = largest.max(json.len());
            let (seq, data) = parse(&json);
            assert_eq!(seq, next_seq, "messages in order");
            next_seq += 1;
            out_len += data.len() as u64;
            handle.get().unwrap().ack(seq);
        },
    )
    .unwrap();
    assert_eq!(report.exit_code, Some(0));
    assert!(largest < CHANNEL_DIRECT_JSON_LIMIT, "{largest}");
    assert_eq!(report.output_bytes, 2_000_000);
    assert_eq!(out_len, 2_000_000);
}

/// Output that escapes to six JSON bytes per byte (ESC → `\u001b`) still
/// fits: messages are cut by their JSON length, not their text length.
#[cfg(unix)]
#[test]
fn pty_messages_fit_when_every_byte_escapes() {
    let mut largest = 0usize;
    let mut escs = 0usize;
    let handle = std::cell::OnceCell::<Arc<TerminalIo>>::new();
    run_terminal(
        sh("head -c 200000 /dev/zero | tr '\\0' '\\033'"),
        &control(Duration::from_secs(60)),
        |io| {
            let _ = handle.set(io);
        },
        |json| {
            largest = largest.max(json.len());
            let (seq, data) = parse(&json);
            escs += data.chars().filter(|c| *c == '\u{1b}').count();
            handle.get().unwrap().ack(seq);
        },
    )
    .unwrap();
    assert!(largest < CHANNEL_DIRECT_JSON_LIMIT, "{largest}");
    assert_eq!(escs, 200_000);
}

/// A window that stops acknowledging gets at most `MAX_UNACKED_BYTES`;
/// past that Kage stops reading the terminal (the program blocks on its
/// writes) instead of buffering output.
#[cfg(unix)]
#[test]
fn pty_unacknowledged_output_is_bounded() {
    let mut sent = 0usize;
    let handle = std::cell::OnceCell::<Arc<TerminalIo>>::new();
    let report = run_terminal(
        sh("head -c 20000000 /dev/zero | tr '\\0' a"),
        &control(Duration::from_secs(2)),
        |io| {
            let _ = handle.set(io);
        },
        |json| {
            assert!(json.len() < CHANNEL_DIRECT_JSON_LIMIT);
            sent += json.len();
        },
    )
    .unwrap();
    assert_eq!(report.outcome, RunOutcome::TimedOut);
    assert!(sent <= MAX_UNACKED_BYTES, "sent {sent} bytes with no ack");
    assert!(handle.get().unwrap().unacked_bytes() <= MAX_UNACKED_BYTES);
    // Read from the terminal: what was sent plus what was waiting, nowhere
    // near the 20 MB the program had to print.
    assert!(
        report.output_bytes <= 256 * 1024,
        "read {} bytes",
        report.output_bytes
    );
}

#[test]
fn message_text_is_cut_by_its_json_length() {
    let sample = "a\"\\\n\r\t\u{1}\u{1b}\u{7f}é€😀";
    for c in sample.chars() {
        let alone = serde_json::to_string(&c.to_string()).unwrap();
        assert_eq!(json_char_len(c), alone.len() - 2, "{c:?}");
    }
    let mut pending = sample.repeat(5000);
    let total_chars = pending.chars().count();
    let mut seq = 0;
    let mut chars = 0;
    while let Some(json) = next_message(&mut pending, seq) {
        assert!(json.len() <= MAX_MESSAGE_JSON_BYTES, "{}", json.len());
        let (s, data) = parse(&json);
        assert_eq!(s, seq);
        chars += data.chars().count();
        seq += 1;
    }
    assert_eq!(chars, total_chars);
    assert!(next_message(&mut String::new(), 0).is_none());
}

#[test]
fn acks_free_only_what_they_cover() {
    let flow = FlowControl::default();
    let full = MAX_UNACKED_BYTES / MAX_MESSAGE_JSON_BYTES;
    for seq in 0..full as u64 {
        assert!(flow.has_room());
        flow.record(seq, MAX_MESSAGE_JSON_BYTES);
    }
    assert!(!flow.has_room());
    flow.ack(0);
    assert!(flow.has_room());
    assert_eq!(flow.unacked_bytes(), (full - 1) * MAX_MESSAGE_JSON_BYTES);
    // Stale and repeated acks change nothing.
    flow.ack(0);
    assert_eq!(flow.unacked_bytes(), (full - 1) * MAX_MESSAGE_JSON_BYTES);
    flow.ack(u64::MAX);
    assert_eq!(flow.unacked_bytes(), 0);
}

// --- Teardown (S3, S5) ---------------------------------------------------

/// A reader the loop has let go of keeps reading to the end on Windows
/// (`ClosePseudoConsole` waits for the output pipe to drain) and stops on
/// Unix.
#[test]
fn reader_after_stop_drains_to_eof_or_exits() {
    use std::sync::atomic::AtomicUsize;
    struct Counting(Arc<AtomicUsize>, usize);
    impl Read for Counting {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.0.fetch_add(1, Ordering::SeqCst);
            if n >= self.1 {
                return Ok(0);
            }
            buf[0] = b'x';
            Ok(1)
        }
    }
    for (after, expect_all) in [(AfterStop::DrainToEof, true), (AfterStop::Exit, false)] {
        let reads = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(1);
        let stop = Arc::new(AtomicBool::new(true));
        drop(rx);
        let t = spawn_reader(Box::new(Counting(reads.clone(), 500)), tx, stop, after).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !t.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(t.is_finished(), "{after:?}: reader never ended");
        let n = reads.load(Ordering::SeqCst);
        if expect_all {
            assert!(n > 500, "{after:?}: stopped after {n} reads");
        } else {
            assert_eq!(n, 1, "{after:?}");
        }
    }
}

/// A reader whose output the loop has no room for waits with it (that is
/// the backpressure) until the run is over, then lets it go.
#[test]
fn reader_waits_for_room_until_stopped() {
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let t = spawn_reader(
        Box::new(io::repeat(b'x')),
        tx,
        stop.clone(),
        AfterStop::Exit,
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(!t.is_finished());
    // One chunk in the channel, one in hand: nothing else was read.
    assert!(rx.try_recv().is_ok());
    stop.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(2);
    while !t.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(t.is_finished());
}

/// Read a pid the script wrote, waiting for it.
#[cfg(unix)]
fn read_pid(path: &std::path::Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(s) = std::fs::read_to_string(path) {
            if let Ok(pid) = s.trim().parse() {
                return pid;
            }
        }
        assert!(Instant::now() < deadline, "no pid written");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether `pid` is gone within `within` (allowing for its reaping).
#[cfg(unix)]
fn gone_within(pid: i32, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        // SAFETY: signal 0 only probes for existence.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
fn scratch_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kage-pty-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A descendant that ignores SIGHUP and keeps the terminal open doesn't
/// outlive the run, and the run's reader and writer are done when it
/// returns (before: the slot was freed with both still running, and the
/// descendant kept the terminal and the reader forever).
#[cfg(unix)]
#[test]
fn pty_exit_ends_what_is_left_of_the_process_group() {
    let dir = scratch_dir();
    let pid_file = dir.join("pid");
    let script = format!(
        "trap '' HUP; sleep 30 & echo $! > '{}'; exit 0",
        pid_file.display()
    );
    let t0 = Instant::now();
    let (report, teardown) = run_terminal_inner(
        sh(&script),
        &control(Duration::from_secs(20)),
        |_| {},
        |_| {},
    )
    .unwrap();
    let pid = read_pid(&pid_file);
    let gone = gone_within(pid, Duration::from_secs(2));
    // SAFETY: cleanup of a process this test started.
    unsafe { libc::kill(pid, libc::SIGKILL) };
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(report.outcome, RunOutcome::Exited);
    assert_eq!(report.exit_code, Some(0));
    assert!(gone, "the HUP-ignoring descendant outlived the run");
    assert!(teardown.finished, "reader or writer still running");
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
}

/// Something that left the process group (job control puts `&` jobs in a
/// group of their own) but kept the terminal can't be reached by the group
/// kill; the run's threads still end and its descriptors close.
#[cfg(unix)]
#[test]
fn pty_threads_end_even_if_an_escaped_process_keeps_the_terminal() {
    let dir = scratch_dir();
    let pid_file = dir.join("pid");
    let script = format!(
        "set -m; trap '' HUP; (trap '' HUP; sleep 30) & echo $! > '{}'; exit 0",
        pid_file.display()
    );
    let t0 = Instant::now();
    let (report, teardown) = run_terminal_inner(
        sh(&script),
        &control(Duration::from_secs(20)),
        |_| {},
        |_| {},
    )
    .unwrap();
    let pid = read_pid(&pid_file);
    // SAFETY: cleanup of a process this test started.
    unsafe { libc::kill(pid, libc::SIGKILL) };
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(report.outcome, RunOutcome::Exited);
    assert!(teardown.finished, "reader or writer still running");
    assert!(t0.elapsed() < Duration::from_secs(6), "{:?}", t0.elapsed());
}

/// Stop: the group is killed, and the run's threads are done when it
/// returns.
#[cfg(unix)]
#[test]
fn pty_stop_tears_down_completely() {
    let dir = scratch_dir();
    let pid_file = dir.join("pid");
    let ctl = control(Duration::from_secs(60));
    let cancel = ctl.cancel.clone();
    let watch = pid_file.clone();
    std::thread::spawn(move || {
        read_pid(&watch);
        cancel.store(true, Ordering::SeqCst);
    });
    let script = format!(
        "trap '' HUP TERM; sleep 30 & echo $! > '{}'; wait",
        pid_file.display()
    );
    let (report, teardown) = run_terminal_inner(sh(&script), &ctl, |_| {}, |_| {}).unwrap();
    let pid = read_pid(&pid_file);
    let gone = gone_within(pid, Duration::from_secs(2));
    // SAFETY: cleanup of a process this test started.
    unsafe { libc::kill(pid, libc::SIGKILL) };
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(report.outcome, RunOutcome::Cancelled);
    assert!(gone, "background job survived Stop");
    assert!(teardown.finished);
}

/// Once the run has closed the terminal, the window side refuses
/// everything instead of reaching a closed pseudo console.
#[cfg(unix)]
#[test]
fn pty_window_side_is_inert_after_the_run() {
    let (_, _, io) = run_collect(sh("exit 0"), &control(Duration::from_secs(10)), |_| {});
    assert_eq!(io.write(b"x"), Err(InputError::Closed));
    assert!(io.resize(TerminalSize { cols: 80, rows: 24 }).is_err());
    io.ack(0);
}

#[cfg(windows)]
#[test]
fn pty_reports_exit_code_windows() {
    let spec = TerminalSpec {
        program: PathBuf::from("cmd.exe"),
        args: vec!["/c".into(), "exit 3".into()],
        cwd: std::env::temp_dir(),
        env: terminal_env(Vec::new()),
        size: TerminalSize { cols: 80, rows: 24 },
    };
    let t0 = Instant::now();
    let (report, _, io) = run_collect(spec, &control(Duration::from_secs(20)), |_| {});
    assert_eq!(report.outcome, RunOutcome::Exited);
    assert_eq!(report.exit_code, Some(3));
    // The loop doesn't wait on the pseudoconsole past its drain grace.
    assert!(t0.elapsed() < Duration::from_secs(10));
    drop(io);
}
