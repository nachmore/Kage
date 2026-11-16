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

/// Run `spec`, handing the terminal to `started`; returns the report, all
/// the output, and the terminal (to poke at after the run).
fn run_collect(
    spec: TerminalSpec,
    control: &RunControl,
    started: impl FnOnce(&Arc<TerminalIo>),
) -> (TerminalReport, String, Arc<TerminalIo>) {
    let mut out = String::new();
    let mut handle = None;
    let report = run_terminal(
        spec,
        control,
        |io| {
            started(&io);
            handle = Some(io);
        },
        |text, _| out.push_str(text),
    )
    .expect("terminal run");
    (report, out, handle.expect("on_started ran"))
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

#[cfg(unix)]
#[test]
fn pty_timeout() {
    let t0 = Instant::now();
    let (report, _, _) = run_collect(sh("sleep 5"), &control(Duration::from_millis(300)), |_| {});
    assert_eq!(report.outcome, RunOutcome::TimedOut);
    assert!(t0.elapsed() < Duration::from_secs(4), "{:?}", t0.elapsed());
}

#[cfg(unix)]
#[test]
fn pty_output_batches_bounded() {
    let mut emitted: u64 = 0;
    let mut largest = 0usize;
    let report = run_terminal(
        sh("head -c 2000000 /dev/zero | tr '\\0' a"),
        &control(Duration::from_secs(30)),
        |_| {},
        |text, skipped| {
            largest = largest.max(text.len());
            emitted += text.len() as u64 + skipped;
        },
    )
    .unwrap();
    assert_eq!(report.exit_code, Some(0));
    assert!(largest <= TERMINAL_PENDING_BYTES, "{largest}");
    assert_eq!(report.output_bytes, 2_000_000);
    assert_eq!(emitted, report.output_bytes);
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
