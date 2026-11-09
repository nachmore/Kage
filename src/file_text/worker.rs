//! Running the `kage-file-text` worker (docs/design/EXTENSION_FILE_TEXT.md
//! §5.5, §5.6).
//!
//! One short-lived process per file. It gets only the bytes, never a path.
//! It is found next to the running executable only, never on `PATH`, so a
//! planted binary can't get the user's files. It starts with an empty
//! environment (`SystemRoot` kept on Windows) in the temp folder, and no
//! console window on Windows. It is Kage's own binary, so the login-shell
//! environment isn't applied.
//!
//! The caller spawns and waits on the same thread: on Linux the worker's
//! `PR_SET_PDEATHSIG` fires when the thread that spawned it exits.
//!
//! Outcomes: exit 0 with a valid answer is the answer. Killed at the
//! deadline is `timeout`, killed for a cancel is `cancelled`. Any other
//! exit (abort, signal, stack overflow, out of memory) is `too_complex`.
//! Output over the cap, output that isn't an answer, or a protocol error
//! is `worker_failed`.

use super::Code;
use crate::error::AppError;
use kage_core::file_text_wire::{
    self as wire, Format, Pages, Warning, WorkerErrorCode, WorkerRequest, WorkerResponse,
};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How often the wait checks the worker, the deadline and the flags.
const POLL: Duration = Duration::from_millis(25);

/// The worker's text, when it read the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extraction {
    pub format: Format,
    pub text: String,
    pub chars: u32,
    pub truncated: bool,
    pub total_chars: Option<u32>,
    pub title: Option<String>,
    pub pages: Option<Pages>,
    pub page_starts: Option<Vec<u32>>,
    pub warnings: Vec<Warning>,
}

/// Why there is no text. `detail` is English, for the log only, and never
/// holds anything from the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerFailure {
    /// The worker's own answer.
    Refused(WorkerErrorCode),
    /// Killed at the deadline.
    Timeout,
    /// Killed because the request was cancelled.
    Cancelled,
    /// Ended without an answer: abort, signal, out of memory.
    Crashed(String),
    /// Couldn't start, said too much, or said something that isn't an
    /// answer.
    Failed(&'static str),
}

impl WorkerFailure {
    /// The error the extension gets. `name` is the cleaned file name,
    /// `extension` the file's, `formats` the requested ones.
    pub fn to_error(&self, name: &str, extension: &str, formats: &[Format]) -> AppError {
        match self {
            WorkerFailure::Refused(WorkerErrorCode::Unsupported) => {
                super::read::unsupported(extension, formats)
            }
            WorkerFailure::Refused(WorkerErrorCode::WrongContent) => {
                Code::WrongContent.error(&[("name", name), ("ext", extension)])
            }
            WorkerFailure::Refused(code) => Code::from_worker(*code).named(name),
            WorkerFailure::Timeout => Code::Timeout.named(name),
            WorkerFailure::Cancelled => Code::Cancelled.named(name),
            WorkerFailure::Crashed(_) => Code::TooComplex.named(name),
            WorkerFailure::Failed(_) => Code::WorkerFailed.error(&[]),
        }
    }

    /// One English phrase for the log.
    pub fn detail(&self) -> String {
        match self {
            WorkerFailure::Refused(code) => format!("refused: {}", code.as_str()),
            WorkerFailure::Timeout => "killed at the deadline".into(),
            WorkerFailure::Cancelled => "killed for a cancel".into(),
            WorkerFailure::Crashed(status) => format!("ended without an answer ({status})"),
            WorkerFailure::Failed(why) => (*why).into(),
        }
    }
}

/// What turns bytes into text: the worker process in the app, a stand-in
/// in unit tests.
pub trait Extractor: Send + Sync {
    fn extract(
        &self,
        request: &WorkerRequest,
        bytes: &[u8],
        cancel: &AtomicBool,
    ) -> Result<Extraction, WorkerFailure>;
}

/// The worker binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worker {
    path: PathBuf,
    deadline: Duration,
}

impl Worker {
    /// The worker next to the running executable, if it is there.
    pub fn locate() -> Option<Worker> {
        let exe = std::env::current_exe().ok()?;
        let path = exe.parent()?.join(format!(
            "{}{}",
            wire::WORKER_BIN_NAME,
            std::env::consts::EXE_SUFFIX
        ));
        path.is_file().then(|| Worker::at(path))
    }

    /// The worker at `path`, with the usual deadline.
    pub fn at(path: PathBuf) -> Worker {
        Worker {
            path,
            deadline: wire::WORKER_DEADLINE,
        }
    }
}

impl Extractor for Worker {
    fn extract(
        &self,
        request: &WorkerRequest,
        bytes: &[u8],
        cancel: &AtomicBool,
    ) -> Result<Extraction, WorkerFailure> {
        run_worker(
            &self.path,
            wire::WORKER_FLAG,
            request,
            bytes,
            self.deadline,
            cancel,
        )
    }
}

/// How the wait ended.
enum End {
    Exited(ExitStatus),
    Timeout,
    Cancelled,
    Overflow,
    Lost,
}

/// Spawns the worker with `flag` (always `--v1` in the app; the tests'
/// debug flags otherwise), sends one request frame, and waits for the
/// answer, the deadline or the cancel flag.
pub fn run_worker(
    path: &Path,
    flag: &str,
    request: &WorkerRequest,
    bytes: &[u8],
    deadline: Duration,
    cancel: &AtomicBool,
) -> Result<Extraction, WorkerFailure> {
    let mut cmd = Command::new(path);
    cmd.arg(flag)
        .env_clear()
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(root) = std::env::var_os("SystemRoot") {
        cmd.env("SystemRoot", root);
    }
    // No console window on Windows.
    crate::os::configure_process_spawn(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|_| WorkerFailure::Failed("the worker didn't start"))?;
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        stop(&mut child);
        return Err(WorkerFailure::Failed("the worker's pipes didn't open"));
    };

    let cap = wire::worker_stdout_cap(request.max_chars);
    let overflow = AtomicBool::new(false);
    let (end, out, err) = std::thread::scope(|s| {
        s.spawn(move || {
            let mut stdin = std::io::BufWriter::with_capacity(1 << 16, stdin);
            // A worker that stops reading closes the pipe; that error is
            // the worker's outcome, not ours.
            let _ = wire::write_request(&mut stdin, request, bytes);
            let _ = stdin.flush();
        });
        let reader = s.spawn(|| {
            let mut out = Vec::new();
            let _ = stdout.take(cap + 1).read_to_end(&mut out);
            if out.len() as u64 > cap {
                overflow.store(true, Ordering::SeqCst);
            }
            out
        });
        let errors = s.spawn(move || drain_stderr(stderr));
        let end = wait(&mut child, deadline, cancel, &overflow);
        (
            end,
            reader.join().unwrap_or_default(),
            errors.join().unwrap_or_default(),
        )
    });

    if !err.is_empty() {
        log::debug!(
            "File text worker stderr: {}",
            String::from_utf8_lossy(&err).trim()
        );
    }
    // The reader has joined, so its flag is final. A worker whose output
    // went over the cap may have exited on its own once the pipe closed.
    if overflow.load(Ordering::SeqCst) && !matches!(end, End::Cancelled) {
        return Err(WorkerFailure::Failed(
            "the worker's output was over the cap",
        ));
    }
    match end {
        End::Timeout => Err(WorkerFailure::Timeout),
        End::Cancelled => Err(WorkerFailure::Cancelled),
        End::Overflow => Err(WorkerFailure::Failed(
            "the worker's output was over the cap",
        )),
        End::Lost => Err(WorkerFailure::Failed("the worker couldn't be waited on")),
        End::Exited(status) if !status.success() => Err(WorkerFailure::Crashed(status.to_string())),
        End::Exited(_) => parse(&out),
    }
}

/// Keeps the first `WORKER_STDERR_CAP_BYTES` and throws the rest away, so
/// a chatty worker never blocks on a full pipe.
fn drain_stderr(mut stderr: impl Read) -> Vec<u8> {
    let mut kept = Vec::new();
    let _ = (&mut stderr)
        .take(wire::WORKER_STDERR_CAP_BYTES)
        .read_to_end(&mut kept);
    let _ = std::io::copy(&mut stderr, &mut std::io::sink());
    kept
}

fn wait(child: &mut Child, deadline: Duration, cancel: &AtomicBool, overflow: &AtomicBool) -> End {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return End::Exited(status),
            Ok(None) => {}
            Err(_) => {
                stop(child);
                return End::Lost;
            }
        }
        let end = if cancel.load(Ordering::SeqCst) {
            Some(End::Cancelled)
        } else if overflow.load(Ordering::SeqCst) {
            Some(End::Overflow)
        } else if started.elapsed() >= deadline {
            Some(End::Timeout)
        } else {
            None
        };
        if let Some(end) = end {
            stop(child);
            return end;
        }
        std::thread::sleep(POLL);
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// The worker's stdout as an answer.
fn parse(out: &[u8]) -> Result<Extraction, WorkerFailure> {
    let response: WorkerResponse = serde_json::from_slice(out)
        .map_err(|_| WorkerFailure::Failed("the worker's output wasn't an answer"))?;
    match response {
        WorkerResponse::Ok {
            format,
            text,
            chars,
            truncated,
            total_chars,
            title,
            pages,
            page_starts,
            warnings,
        } => Ok(Extraction {
            format,
            text,
            chars,
            truncated,
            total_chars,
            title,
            pages,
            page_starts,
            warnings,
        }),
        WorkerResponse::Err { code, detail } if is_protocol_error(code, &detail) => {
            log::warn!("File text worker refused the request frame: {detail}");
            Err(WorkerFailure::Failed(
                "the worker refused the request frame",
            ))
        }
        WorkerResponse::Err { code, detail } => {
            log::debug!("File text worker: {} ({detail})", code.as_str());
            Err(WorkerFailure::Refused(code))
        }
    }
}

/// The worker's answers to a bad frame or an unknown wire version
/// (`kage-file-text/src/main.rs`, `lib.rs`). They mean a broken install,
/// not a broken file, so they become `worker_failed` (§5.6).
fn is_protocol_error(code: WorkerErrorCode, detail: &str) -> bool {
    code == WorkerErrorCode::Malformed
        && (detail == "bad request frame" || detail.starts_with("wire version "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_parse_and_protocol_errors_are_worker_failures() {
        let ok = serde_json::to_vec(&WorkerResponse::Ok {
            format: Format::Text,
            text: "hi".into(),
            chars: 2,
            truncated: false,
            total_chars: Some(2),
            title: None,
            pages: None,
            page_starts: None,
            warnings: vec![Warning::LegacyEncoding],
        })
        .unwrap();
        let got = parse(&ok).unwrap();
        assert_eq!(got.text, "hi");
        assert_eq!(got.warnings, vec![Warning::LegacyEncoding]);

        let refused = |code, detail: &str| {
            serde_json::to_vec(&WorkerResponse::Err {
                code,
                detail: detail.into(),
            })
            .unwrap()
        };
        assert_eq!(
            parse(&refused(WorkerErrorCode::NotText, "control characters")),
            Err(WorkerFailure::Refused(WorkerErrorCode::NotText))
        );
        assert!(matches!(
            parse(&refused(
                WorkerErrorCode::Malformed,
                "wire version 9 not supported"
            )),
            Err(WorkerFailure::Failed(_))
        ));
        assert!(matches!(
            parse(&refused(WorkerErrorCode::Malformed, "bad request frame")),
            Err(WorkerFailure::Failed(_))
        ));
        assert_eq!(
            parse(&refused(WorkerErrorCode::Malformed, "broken xref")),
            Err(WorkerFailure::Refused(WorkerErrorCode::Malformed))
        );
        assert!(matches!(parse(b"xxxx"), Err(WorkerFailure::Failed(_))));
        assert!(matches!(parse(b""), Err(WorkerFailure::Failed(_))));
    }

    #[test]
    fn every_failure_maps_to_its_key() {
        crate::i18n::init(Some("en"));
        let formats = [Format::Text, Format::Pdf];
        let key = |f: WorkerFailure| f.to_error("Plan.pdf", "pdf", &formats).key;
        let cases = [
            (WorkerErrorCode::Unsupported, "unsupported"),
            (WorkerErrorCode::WrongContent, "wrong_content"),
            (WorkerErrorCode::Encrypted, "encrypted"),
            (WorkerErrorCode::LegacyFormat, "legacy_format"),
            (WorkerErrorCode::NotText, "not_text"),
            (WorkerErrorCode::Malformed, "malformed"),
            (WorkerErrorCode::TooComplex, "too_complex"),
        ];
        for (code, last) in cases {
            assert_eq!(code.as_str(), last);
            assert_eq!(
                key(WorkerFailure::Refused(code)),
                format!("errors.file_text.{last}")
            );
        }
        assert_eq!(key(WorkerFailure::Timeout), "errors.file_text.timeout");
        assert_eq!(key(WorkerFailure::Cancelled), "errors.file_text.cancelled");
        assert_eq!(
            key(WorkerFailure::Crashed("signal 6".into())),
            "errors.file_text.too_complex"
        );
        assert_eq!(
            key(WorkerFailure::Failed("x")),
            "errors.file_text.worker_failed"
        );

        let e = WorkerFailure::Refused(WorkerErrorCode::WrongContent)
            .to_error("Plan.pdf", "pdf", &formats);
        assert_eq!(
            e.to_string(),
            "Plan.pdf doesn't contain what its .pdf name says."
        );
        let e = WorkerFailure::Refused(WorkerErrorCode::Unsupported)
            .to_error("Plan.pdf", "pdf", &formats);
        assert_eq!(
            e.to_string(),
            "Kage can't read .pdf files here. It reads: .txt, .pdf."
        );
    }

    #[test]
    fn a_missing_binary_doesnt_start() {
        let dir = tempfile::tempdir().unwrap();
        let worker = Worker::at(dir.path().join("kage-file-text-missing"));
        let req = WorkerRequest::new(vec![Format::Text], "txt", 10);
        let got = worker.extract(&req, b"hi", &AtomicBool::new(false));
        assert_eq!(got, Err(WorkerFailure::Failed("the worker didn't start")));
    }
}
