//! Argument validation, shell choice, output capping, and the run loop's
//! exit / timeout / cancel paths — with harmless commands (`echo`,
//! `sleep`) under `/bin/sh -c`, never the developer's login shell.

use super::runner::*;
use super::*;
use std::path::Path;

fn tempdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kage-user-shell-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// --- Who may call -------------------------------------------------------------

#[test]
fn only_kage_chat_and_floating_windows_may_run_commands() {
    for label in [
        window_labels::MAIN,
        window_labels::FLOATING,
        &window_labels::chat_label("0b3e"),
    ] {
        assert!(caller_allowed(label), "{label}");
        assert!(ensure_caller(label).is_ok());
    }
    for label in [
        window_labels::SETTINGS,
        window_labels::STORE,
        window_labels::WELCOME,
        window_labels::CONTEXT_MENU,
        window_labels::INLINE_ASSIST,
        "",
        "Main",
        "floating2",
    ] {
        assert!(!caller_allowed(label), "{label}");
        assert_eq!(
            ensure_caller(label).unwrap_err().key,
            "errors.user_shell.not_allowed"
        );
    }
}

// --- Arguments ----------------------------------------------------------------

#[test]
fn commands_must_be_non_blank_bounded_and_nul_free() {
    assert_eq!(validate_command(""), Err(InvalidRun::EmptyCommand));
    assert_eq!(validate_command("  \n\t "), Err(InvalidRun::EmptyCommand));
    assert_eq!(validate_command("echo a\0b"), Err(InvalidRun::NulInCommand));
    let at_limit = "x".repeat(MAX_COMMAND_BYTES);
    assert_eq!(validate_command(&at_limit), Ok(()));
    let over = "x".repeat(MAX_COMMAND_BYTES + 1);
    assert_eq!(validate_command(&over), Err(InvalidRun::CommandTooLong));
    // Bytes, not characters: 3-byte characters hit the limit sooner.
    let wide = "€".repeat(MAX_COMMAND_BYTES / 3 + 1);
    assert_eq!(validate_command(&wide), Err(InvalidRun::CommandTooLong));
    assert_eq!(validate_command(" brew install awscli\n"), Ok(()));
}

#[test]
fn invalid_arguments_map_to_keyed_errors() {
    assert_eq!(
        invalid(InvalidRun::EmptyCommand).key,
        "errors.user_shell.empty_command"
    );
    let too_long = invalid(InvalidRun::CommandTooLong);
    assert_eq!(too_long.key, "errors.user_shell.command_too_long");
    assert_eq!(
        too_long.params.get("max_kib").map(String::as_str),
        Some("32")
    );
    let missing = invalid(InvalidRun::CwdMissing("/nope".into()));
    assert_eq!(missing.key, "errors.user_shell.cwd_missing");
    assert_eq!(
        missing.params.get("path").map(String::as_str),
        Some("/nope")
    );
}

#[test]
fn run_ids_are_short_and_plain() {
    assert_eq!(validate_run_id("kage-run-12"), Ok(()));
    assert_eq!(validate_run_id("A_b-9"), Ok(()));
    for bad in ["", "a b", "x/../y", "run\n", "é", &"x".repeat(65)] {
        assert_eq!(validate_run_id(bad), Err(InvalidRun::BadRunId), "{bad:?}");
    }
}

#[test]
fn cwd_defaults_to_home_and_must_be_an_existing_directory() {
    let home = tempdir();
    std::fs::create_dir_all(home.join("proj")).unwrap();
    std::fs::write(home.join("file.txt"), "x").unwrap();

    assert_eq!(resolve_cwd(None, Some(&home)), Ok(home.clone()));
    assert_eq!(resolve_cwd(Some("  "), Some(&home)), Ok(home.clone()));
    assert_eq!(resolve_cwd(Some("~"), Some(&home)), Ok(home.clone()));
    assert_eq!(
        resolve_cwd(Some("~/proj"), Some(&home)),
        Ok(home.join("proj"))
    );
    let abs = home.join("proj");
    assert_eq!(
        resolve_cwd(abs.to_str(), Some(Path::new("/elsewhere"))),
        Ok(abs.clone())
    );

    assert_eq!(
        resolve_cwd(Some("proj"), Some(&home)),
        Err(InvalidRun::CwdNotAbsolute("proj".into()))
    );
    let missing = home.join("gone");
    assert_eq!(
        resolve_cwd(missing.to_str(), Some(&home)),
        Err(InvalidRun::CwdMissing(missing.to_string_lossy().into()))
    );
    let file = home.join("file.txt");
    assert_eq!(
        resolve_cwd(file.to_str(), Some(&home)),
        Err(InvalidRun::CwdNotDirectory(file.to_string_lossy().into()))
    );
    assert_eq!(resolve_cwd(None, None), Err(InvalidRun::NoHome));
    assert_eq!(resolve_cwd(Some("~/proj"), None), Err(InvalidRun::NoHome));
}

// --- Which shell --------------------------------------------------------------

#[test]
fn login_shell_is_shell_env_when_posix_else_zsh_else_sh() {
    use std::ffi::OsStr;
    let all = |_: &Path| true;
    assert_eq!(
        select_posix_shell(Some(OsStr::new("/opt/homebrew/bin/bash")), all),
        PathBuf::from("/opt/homebrew/bin/bash")
    );
    assert_eq!(
        select_posix_shell(Some(OsStr::new("/bin/zsh")), all),
        PathBuf::from("/bin/zsh")
    );
    // Not POSIX-family, relative, empty, missing: the fallback.
    for env in [
        "/opt/homebrew/bin/fish",
        "/usr/bin/nu",
        "/bin/tcsh",
        "zsh",
        "",
    ] {
        assert_eq!(
            select_posix_shell(Some(OsStr::new(env)), all),
            PathBuf::from("/bin/zsh"),
            "{env}"
        );
    }
    assert_eq!(select_posix_shell(None, all), PathBuf::from("/bin/zsh"));
    // $SHELL not executable; no zsh either.
    let only_sh = |p: &Path| p == Path::new("/bin/sh");
    assert_eq!(
        select_posix_shell(Some(OsStr::new("/bin/bash")), only_sh),
        PathBuf::from("/bin/sh")
    );
    assert_eq!(
        select_posix_shell(None, |_| false),
        PathBuf::from("/bin/sh")
    );
}

#[test]
fn posix_shells_get_the_command_unchanged_as_one_argument() {
    let cmd = "brew install awscli && echo \"done\" # it's fine\n";
    let inv = posix_invocation(Path::new("/bin/zsh"), cmd);
    assert_eq!(inv.program, PathBuf::from("/bin/zsh"));
    assert_eq!(inv.args, vec!["-l".to_string(), "-c".into(), cmd.into()]);
    assert_eq!(inv.raw_tail, None);
    assert_eq!(inv.label, "zsh");
}

#[test]
fn windows_runs_powershell_unless_the_agent_shell_was_cmd() {
    assert_eq!(WindowsShell::from_hint(None), WindowsShell::PowerShell);
    assert_eq!(
        WindowsShell::from_hint(Some("bash")),
        WindowsShell::PowerShell
    );
    assert_eq!(WindowsShell::from_hint(Some("CMD")), WindowsShell::Cmd);

    let ps = windows_invocation(WindowsShell::PowerShell, "Get-ChildItem \"C:\\x y\"");
    assert_eq!(ps.program, PathBuf::from("powershell.exe"));
    assert_eq!(
        ps.args,
        vec![
            "-NoLogo".to_string(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            "Get-ChildItem \"C:\\x y\"".into(),
        ]
    );
    assert_eq!(ps.raw_tail, None);

    let cmd = windows_invocation(WindowsShell::Cmd, "dir /b && echo \"x\"");
    assert_eq!(cmd.program, PathBuf::from("cmd.exe"));
    assert_eq!(cmd.args, vec!["/D".to_string()]);
    assert_eq!(
        cmd.raw_tail.as_deref(),
        Some("/S /C \"dir /b && echo \"x\"\"")
    );
    assert_eq!(cmd.label, "cmd");
}

// --- Output -------------------------------------------------------------------

#[test]
fn utf8_stream_reassembles_characters_split_across_reads() {
    let mut d = Utf8Stream::default();
    let bytes = "a€b😀".as_bytes();
    // Split inside "€" (3 bytes) and inside "😀" (4 bytes).
    let mut out = String::new();
    out += &d.push(&bytes[..2]);
    out += &d.push(&bytes[2..6]);
    out += &d.push(&bytes[6..]);
    out += &d.finish();
    assert_eq!(out, "a€b😀");

    let mut bad = Utf8Stream::default();
    assert_eq!(bad.push(b"ok\xffthen"), "ok\u{FFFD}then");
    assert_eq!(bad.push(b"\xe2\x82"), "");
    assert_eq!(bad.finish(), "\u{FFFD}");
    assert_eq!(bad.finish(), "");
}

#[test]
fn output_tail_keeps_the_last_bytes_on_a_character_boundary() {
    let mut tail = OutputTail::new(10);
    tail.push("hello ");
    tail.push("world");
    assert_eq!(tail.text(), "ello world");
    assert_eq!(tail.total_bytes(), 11);
    assert_eq!(tail.dropped_bytes(), 1);

    // One chunk bigger than the cap; the cut never splits "é".
    let mut tail = OutputTail::new(5);
    tail.push("abééé");
    assert!(tail.text().len() <= 5);
    assert_eq!(tail.text(), "éé");
    assert_eq!(tail.total_bytes(), 8);
    assert_eq!(tail.dropped_bytes(), 4);

    let mut small = OutputTail::new(64);
    small.push("x");
    assert_eq!(small.dropped_bytes(), 0);
    assert_eq!(small.take(), ("x".to_string(), 0));
    assert!(small.is_empty());
}

#[test]
fn outcome_buckets_and_program_labels_carry_no_command_text() {
    let report = |outcome, exit_code, signal| RunReport {
        outcome,
        exit_code,
        signal,
        duration_ms: 1,
        output: String::new(),
        truncated: false,
        output_bytes: 0,
    };
    assert_eq!(
        outcome_bucket(&report(RunOutcome::Exited, Some(0), None)),
        "0"
    );
    assert_eq!(
        outcome_bucket(&report(RunOutcome::Exited, Some(1), None)),
        "1"
    );
    assert_eq!(
        outcome_bucket(&report(RunOutcome::Exited, Some(42), None)),
        "2-125"
    );
    assert_eq!(
        outcome_bucket(&report(RunOutcome::Exited, Some(127), None)),
        "127"
    );
    assert_eq!(
        outcome_bucket(&report(RunOutcome::Exited, Some(255), None)),
        "other"
    );
    assert_eq!(
        outcome_bucket(&report(RunOutcome::Exited, None, Some(9))),
        "signal"
    );
    assert_eq!(
        outcome_bucket(&report(RunOutcome::Cancelled, None, Some(15))),
        "cancelled"
    );
    assert_eq!(
        outcome_bucket(&report(RunOutcome::TimedOut, None, Some(9))),
        "timeout"
    );

    assert_eq!(program_label("brew install awscli"), "brew");
    assert_eq!(
        program_label("TOKEN=s3cret /opt/homebrew/bin/brew up"),
        "brew"
    );
    assert_eq!(program_label("  "), "?");
    assert_eq!(program_label("API_KEY=abc"), "?");
    assert_eq!(
        program_label("C:\\Tools\\winget.exe install x"),
        "winget.exe"
    );
}

// --- One run per window -------------------------------------------------------

#[test]
fn one_run_per_window_and_only_its_own_window_can_stop_it() {
    let runs = Arc::new(UserShellRuns::new());
    let slot = runs.begin("main", "r1").expect("free");
    assert!(runs.is_running("main"));
    assert!(
        runs.begin("main", "r2").is_none(),
        "second run in the same window"
    );
    let other = runs
        .begin("floating", "r3")
        .expect("another window is free");

    assert!(!runs.cancel("main", "r2"), "not the running id");
    assert!(!runs.cancel("floating", "r1"), "another window's run");
    assert!(!slot.cancel_flag().load(Ordering::SeqCst));
    assert!(runs.cancel("main", "r1"));
    assert!(slot.cancel_flag().load(Ordering::SeqCst));

    drop(slot);
    assert!(!runs.is_running("main"));
    assert!(runs.begin("main", "r4").is_some());
    drop(other);
    assert!(!runs.is_running("floating"));
}

#[test]
fn shutdown_stops_every_run_and_waits_for_them() {
    let runs = Arc::new(UserShellRuns::new());
    // Nothing running: returns at once.
    let t = Instant::now();
    runs.cancel_all_and_wait(Duration::from_secs(5));
    assert!(t.elapsed() < Duration::from_secs(1));

    let slot = runs.begin("main", "r1").unwrap();
    let flag = slot.cancel_flag();
    let waiter = std::thread::spawn(move || {
        while !flag.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(slot); // the run ends once told to stop
    });
    let t = Instant::now();
    runs.cancel_all_and_wait(Duration::from_secs(5));
    assert!(t.elapsed() < Duration::from_secs(2));
    assert!(!runs.is_running("main"));
    waiter.join().unwrap();
}

// --- Running ------------------------------------------------------------------

#[cfg(unix)]
mod running {
    use super::*;

    fn sh(script: &str) -> ShellInvocation {
        ShellInvocation {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), script.into()],
            raw_tail: None,
            label: "sh".into(),
        }
    }

    fn control(timeout: Duration) -> RunControl {
        RunControl {
            cancel: Arc::new(AtomicBool::new(false)),
            timeout,
        }
    }

    fn run_collecting(inv: &ShellInvocation, cwd: &Path, ctl: &RunControl) -> (RunReport, String) {
        let mut streamed = String::new();
        let report = run(inv, cwd, ctl, |text, skipped| {
            assert_eq!(skipped, 0);
            streamed.push_str(text);
        })
        .expect("spawns");
        (report, streamed)
    }

    #[test]
    fn reports_exit_code_and_streams_stdout_and_stderr() {
        let dir = tempdir();
        let (report, streamed) = run_collecting(
            &sh("echo out; echo err >&2; exit 3"),
            &dir,
            &control(Duration::from_secs(30)),
        );
        assert_eq!(report.outcome, RunOutcome::Exited);
        assert_eq!(report.exit_code, Some(3));
        assert_eq!(report.signal, None);
        assert!(report.output.contains("out\n"), "{:?}", report.output);
        assert!(report.output.contains("err\n"), "{:?}", report.output);
        assert_eq!(streamed, report.output);
        assert!(!report.truncated);
        assert_eq!(report.output_bytes, 8);
    }

    #[test]
    fn runs_in_the_directory_with_stdin_closed() {
        let dir = tempdir();
        // `cat` would block forever on an open stdin.
        let (report, _) = run_collecting(
            &sh("pwd; cat; echo after-cat"),
            &dir,
            &control(Duration::from_secs(30)),
        );
        assert_eq!(report.exit_code, Some(0));
        let pwd = report.output.lines().next().unwrap();
        assert_eq!(
            std::fs::canonicalize(pwd).unwrap(),
            std::fs::canonicalize(&dir).unwrap()
        );
        assert!(report.output.ends_with("after-cat\n"));
    }

    #[test]
    fn keeps_only_the_last_64_kib_and_says_so() {
        let dir = tempdir();
        // ~121 KiB: 2000 lines of 62 bytes.
        let script = "i=0; while [ $i -lt 2000 ]; do \
                      printf 'line %05d %050d\\n' $i 0; i=$((i+1)); done";
        let mut chunks = 0;
        let report = run(
            &sh(script),
            &dir,
            &control(Duration::from_secs(60)),
            |_, _| {
                chunks += 1;
            },
        )
        .unwrap();
        assert_eq!(report.exit_code, Some(0));
        assert!(report.truncated);
        assert!(report.output.len() <= OUTPUT_TAIL_BYTES);
        assert!(report.output.len() > OUTPUT_TAIL_BYTES - 100);
        assert_eq!(report.output_bytes, 2000 * 62);
        assert!(report.output.ends_with(&format!("line 01999 {:050}\n", 0)));
        assert!(!report.output.contains("line 00000 "));
        assert!(chunks >= 1);
    }

    #[test]
    fn times_out_and_kills_the_process_group() {
        let dir = tempdir();
        let marker = dir.join("bg.pid");
        // A background job in the same group, which must die with it.
        let script = format!("sleep 30 & echo $! > '{}'; sleep 30", marker.display());
        let started = Instant::now();
        let (report, _) = run_collecting(&sh(&script), &dir, &control(Duration::from_millis(400)));
        assert_eq!(report.outcome, RunOutcome::TimedOut);
        assert!(report.exit_code.is_none());
        assert!(report.signal.is_some());
        assert!(started.elapsed() < Duration::from_secs(10));

        let pid: i32 = std::fs::read_to_string(&marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let gone = (0..100).any(|_| {
            let alive = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok();
            if alive {
                std::thread::sleep(Duration::from_millis(20));
            }
            !alive
        });
        assert!(gone, "background job {pid} survived the kill");
    }

    #[test]
    fn stop_cancels_a_running_command() {
        let dir = tempdir();
        let ctl = control(Duration::from_secs(60));
        let flag = ctl.cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let (report, streamed) = run_collecting(&sh("echo started; sleep 30"), &dir, &ctl);
        assert_eq!(report.outcome, RunOutcome::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(streamed, "started\n");
        assert_eq!(report.output, "started\n");
    }

    #[test]
    fn a_missing_shell_fails_to_spawn() {
        let dir = tempdir();
        let inv = ShellInvocation {
            program: PathBuf::from("/nonexistent/kage-test-shell"),
            args: vec!["-c".into(), "true".into()],
            raw_tail: None,
            label: "x".into(),
        };
        assert!(run(&inv, &dir, &control(Duration::from_secs(5)), |_, _| {}).is_err());
    }
}
