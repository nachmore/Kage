//! The worker as a process: the protocol, the limits it puts on itself
//! before reading input, and the debug-only failure modes the app's
//! tests rely on (EXTENSION_FILE_TEXT §5.5, §12).

use kage_core::file_text_wire::{
    write_request, Format, Warning, WorkerErrorCode, WorkerRequest, WorkerResponse,
    MAX_HEADER_BYTES, READS_P1_FORMATS, WORKER_ALLOC_CAP_BYTES, WORKER_FLAG,
};
use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const WORKER: &str = env!("CARGO_BIN_EXE_kage-file-text");

/// The worker as the app starts it: an empty environment (but
/// `SystemRoot` on Windows) and no per-user folders, so these tests touch
/// none of the user's files.
fn worker_command() -> Command {
    let mut cmd = Command::new(WORKER);
    cmd.env_clear();
    if let Some(root) = std::env::var_os("SystemRoot") {
        cmd.env("SystemRoot", root);
    }
    cmd
}

fn spawn(flag: &str) -> Child {
    worker_command()
        .arg(flag)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the worker")
}

fn frame(req: &WorkerRequest, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    write_request(&mut out, req, payload).unwrap();
    out
}

/// Writes `input`, closes stdin, waits up to `limit`. Kills on timeout.
fn run(flag: &str, input: &[u8], limit: Duration) -> (ExitStatus, Vec<u8>) {
    let mut child = spawn(flag);
    let mut stdin = child.stdin.take().unwrap();
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let status = wait_for(&mut child, limit).unwrap_or_else(|| {
        let _ = child.kill();
        panic!("the worker didn't finish within {limit:?}");
    });
    writer.join().unwrap();
    (status, reader.join().unwrap())
}

fn wait_for(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn respond(flag: &str, input: &[u8]) -> WorkerResponse {
    let (status, out) = run(flag, input, Duration::from_secs(30));
    assert!(status.success(), "exit {status:?}");
    serde_json::from_slice(&out).expect("one JSON response")
}

fn request(extension: &str, max_chars: u32) -> WorkerRequest {
    WorkerRequest::new(vec![Format::Text, Format::Markdown], extension, max_chars)
}

fn err_code(resp: &WorkerResponse) -> WorkerErrorCode {
    match resp {
        WorkerResponse::Err { code, .. } => *code,
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn reads_a_text_file() {
    let input = frame(&request("TXT", 1000), b"Hello  \r\nworld\r\n");
    match respond(WORKER_FLAG, &input) {
        WorkerResponse::Ok {
            format,
            text,
            chars,
            truncated,
            warnings,
            ..
        } => {
            assert_eq!(format, Format::Text);
            assert_eq!(text, "Hello\nworld");
            assert_eq!(chars, 11);
            assert!(!truncated);
            assert!(warnings.is_empty());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn reads_markdown_with_its_title() {
    let input = frame(&request("md", 1000), "# Plan \u{202e}A\n\nBody".as_bytes());
    match respond(WORKER_FLAG, &input) {
        WorkerResponse::Ok { format, title, .. } => {
            assert_eq!(format, Format::Markdown);
            assert_eq!(title.as_deref(), Some("Plan A"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_large_file_is_capped() {
    let full = "Lorem ipsum dolor sit amet. ".repeat(80_000);
    let input = frame(&request("txt", 100_000), full.as_bytes());
    match respond(WORKER_FLAG, &input) {
        WorkerResponse::Ok {
            chars,
            truncated,
            total_chars,
            text,
            ..
        } => {
            assert!(truncated);
            assert!(chars <= 100_000 && chars > 95_000);
            assert_eq!(text.chars().count() as u32, chars);
            assert!(text.ends_with("amet."), "cut at a sentence end");
            // The whole text, less its trailing space.
            assert_eq!(total_chars, Some(28 * 80_000 - 1));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn refuses_what_it_wasnt_asked_for() {
    let pdf_only = WorkerRequest::new(vec![Format::Pdf], "txt", 100);
    assert_eq!(
        err_code(&respond(WORKER_FLAG, &frame(&pdf_only, b"x"))),
        WorkerErrorCode::Unsupported
    );
    let unknown = request("exe", 100);
    assert_eq!(
        err_code(&respond(WORKER_FLAG, &frame(&unknown, b"x"))),
        WorkerErrorCode::Unsupported
    );
}

#[test]
fn reads_html_and_rtf_with_the_p1_switch() {
    let both = vec![Format::Html, Format::Rtf];
    let page = b"<html><head><title>Plan</title><script>x()</script></head>\
        <body><h1>Q3</h1><p hidden>left out</p><ul><li>Ship it</li></ul></body></html>";
    let doc = b"{\\rtf1\\ansi\\ansicpg1252{\\fonttbl{\\f0 Helvetica;}}\
        {\\info{\\title Notes}}\\pard\\f0 caf\\'e9\\par Second}";
    let html = frame(&WorkerRequest::new(both.clone(), "HTM", 1000), page);
    let rtf = frame(&WorkerRequest::new(both, "rtf", 1000), doc);
    if !READS_P1_FORMATS {
        for input in [html, rtf] {
            assert_eq!(
                err_code(&respond(WORKER_FLAG, &input)),
                WorkerErrorCode::Unsupported
            );
        }
        return;
    }
    match respond(WORKER_FLAG, &html) {
        WorkerResponse::Ok {
            format,
            text,
            title,
            warnings,
            ..
        } => {
            assert_eq!(format, Format::Html);
            assert_eq!(text, "# Q3\n\n- Ship it");
            assert_eq!(title.as_deref(), Some("Plan"));
            assert_eq!(warnings, [Warning::HiddenTextSkipped]);
        }
        other => panic!("{other:?}"),
    }
    match respond(WORKER_FLAG, &rtf) {
        WorkerResponse::Ok {
            format,
            text,
            title,
            ..
        } => {
            assert_eq!(format, Format::Rtf);
            assert_eq!(text, "caf\u{e9}\n\nSecond");
            assert_eq!(title.as_deref(), Some("Notes"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn detects_wrong_content() {
    let input = frame(&request("txt", 100), b"%PDF-1.7\n1 0 obj");
    assert_eq!(
        err_code(&respond(WORKER_FLAG, &input)),
        WorkerErrorCode::WrongContent
    );
    let binary: Vec<u8> = (0..=255u8).cycle().take(9000).collect();
    let input = frame(&request("log", 100), &binary);
    assert_eq!(
        err_code(&respond(WORKER_FLAG, &input)),
        WorkerErrorCode::NotText
    );
}

#[test]
fn an_unknown_version_is_malformed() {
    let mut req = request("txt", 100);
    req.v = 2;
    assert_eq!(
        err_code(&respond(WORKER_FLAG, &frame(&req, b"x"))),
        WorkerErrorCode::Malformed
    );
}

#[test]
fn a_bad_frame_is_malformed() {
    let too_big_header = (MAX_HEADER_BYTES + 1).to_le_bytes();
    assert_eq!(
        err_code(&respond(WORKER_FLAG, &too_big_header)),
        WorkerErrorCode::Malformed
    );
    let mut cut = frame(&request("txt", 100), b"0123456789");
    cut.truncate(cut.len() - 4);
    assert_eq!(
        err_code(&respond(WORKER_FLAG, &cut)),
        WorkerErrorCode::Malformed
    );
    assert_eq!(
        err_code(&respond(WORKER_FLAG, b"")),
        WorkerErrorCode::Malformed
    );
}

#[test]
fn refuses_other_arguments() {
    for args in [
        &[][..],
        &["--v2"][..],
        &[WORKER_FLAG, "extra"][..],
        &["/etc/passwd"][..],
    ] {
        let status = worker_command()
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(2), "{args:?}");
    }
}

#[cfg(debug_assertions)]
mod failure_modes {
    use super::*;

    fn abnormal(status: ExitStatus) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            status.signal() == Some(libc_sigabrt())
        }
        #[cfg(not(unix))]
        {
            !status.success()
        }
    }

    #[cfg(unix)]
    fn libc_sigabrt() -> i32 {
        6
    }

    fn any_input() -> Vec<u8> {
        frame(&request("txt", 100), b"hello")
    }

    /// The limits are in place before stdin is read: the report comes
    /// back while stdin stays open and empty.
    #[test]
    fn limits_are_applied_before_input_is_read() {
        let mut child = spawn("--v1-test-limits");
        let _stdin = child.stdin.take();
        let status = wait_for(&mut child, Duration::from_secs(10)).expect("report without input");
        assert!(status.success());
        let mut out = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        let report: serde_json::Value = serde_json::from_str(&out).unwrap();

        assert_eq!(report["alloc_cap"], WORKER_ALLOC_CAP_BYTES);
        assert!(
            report["alloc_live"].as_u64().unwrap() > 0,
            "the counting allocator is global"
        );
        #[cfg(unix)]
        {
            let cpu = report["cpu"].as_array().unwrap();
            assert!(cpu[0].as_u64().unwrap() <= 30 && cpu[1].as_u64().unwrap() <= 31);
            assert_eq!(report["core"], serde_json::json!([0, 0]));
        }
        #[cfg(target_os = "linux")]
        {
            let two_gib = 2u64 << 30;
            let space = report["address_space"].as_array().unwrap();
            assert!(space[0].as_u64().unwrap() <= two_gib && space[1].as_u64().unwrap() <= two_gib);
            assert_eq!(report["no_new_privs"], 1);
            assert_eq!(report["death_signal"], 9);
        }
        #[cfg(windows)]
        {
            let job = &report["job"];
            assert_eq!(job["in_job"], true);
            assert_eq!(job["process_memory"], 1u64 << 30);
            assert_eq!(job["active_processes"], 1);
            assert_eq!(job["die_on_unhandled_exception"], true);
        }
    }

    #[test]
    fn the_allocator_cap_aborts() {
        let (status, out) = run("--v1-test-oom", b"", Duration::from_secs(30));
        assert!(abnormal(status), "{status:?}");
        assert!(out.is_empty());
    }

    #[test]
    fn an_abort_is_an_abnormal_exit() {
        let (status, out) = run("--v1-test-abort", &any_input(), Duration::from_secs(30));
        assert!(abnormal(status), "{status:?}");
        assert!(out.is_empty());
    }

    #[test]
    fn a_parser_panic_is_malformed() {
        let mut child = spawn("--v1-test-panic");
        child.stdin.take().unwrap().write_all(&any_input()).unwrap();
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        let mut out = Vec::new();
        child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
        assert!(child.wait().unwrap().success());
        let resp: WorkerResponse = serde_json::from_slice(&out).unwrap();
        assert_eq!(err_code(&resp), WorkerErrorCode::Malformed);
        assert!(stderr.contains("panicked at"));
        assert!(
            !stderr.contains("test panic"),
            "the message isn't printed: {stderr}"
        );
    }

    #[test]
    fn deep_but_legal_recursion_survives_on_the_parser_stack() {
        match respond("--v1-test-deep", &any_input()) {
            WorkerResponse::Ok { text, .. } => {
                let used: usize = text.split(' ').next().unwrap().parse().unwrap();
                assert!(used >= 24 << 20, "{text}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_hanging_worker_runs_until_killed() {
        let mut child = spawn("--v1-test-hang");
        child.stdin.take().unwrap().write_all(&any_input()).unwrap();
        assert!(wait_for(&mut child, Duration::from_millis(300)).is_none());
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());
    }

    #[test]
    fn a_flooding_worker_writes_past_any_cap() {
        let mut child = spawn("--v1-test-flood");
        child.stdin.take().unwrap().write_all(&any_input()).unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let mut buf = vec![0u8; 1 << 16];
        let mut total = 0usize;
        while total < 4 << 20 {
            let n = stdout.read(&mut buf).unwrap();
            assert!(n > 0, "flood stopped after {total} bytes");
            total += n;
        }
        child.kill().unwrap();
        let _ = child.wait();
    }
}
