//! Checking and reading the file the user picked
//! (docs/design/EXTENSION_FILE_TEXT.md §5.4). Runs on a blocking thread.
//!
//! Nothing is read before the checks pass: a regular file (links
//! followed), an extension of a requested format (the Linux GTK dialog
//! takes typed paths, so the filter alone isn't enough), and at most
//! `MAX_INPUT_BYTES`. The open and the read run on their own thread with
//! a timeout, for files a cloud drive is still downloading; on timeout
//! that thread is left to finish and its bytes are dropped.
//!
//! The path stays in local variables here and in the `file_ref` input.
//! Only the cleaned file name leaves this module.

use super::{extension_list, format_size, post, Code};
use crate::error::AppError;
use crate::os::file_probe::{self, Opened};
use kage_core::file_text_wire::{Format, MAX_INPUT_BYTES, MAX_NAME_CHARS};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

/// How often a waiting read checks its deadline and the cancel flag.
const POLL: Duration = Duration::from_millis(25);

/// What the picked path's name says, before anything is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    /// The final path component, cleaned (§5.10), at most 255 characters.
    pub name: String,
    /// Lowercase, without the dot; empty when the name has none.
    pub extension: String,
    /// The format the extension belongs to, if any.
    pub format: Option<Format>,
}

/// The name and extension of `path`. Reads nothing.
pub fn name_of(path: &Path) -> Named {
    let raw = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    Named {
        name: post::clean_line(&raw, MAX_NAME_CHARS),
        format: Format::from_extension(&extension),
        extension,
    }
}

/// The picked file, once its checks passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// The format its extension names, one of the requested ones.
    pub format: Format,
    pub size: u64,
    /// Milliseconds since the epoch, when the OS reports it.
    pub modified_at: Option<i64>,
    /// Reading it waits for a cloud download.
    pub placeholder: bool,
}

/// §5.4 steps 1–3 and 7: a regular file, a requested extension, the size
/// cap. `named` is [`name_of`] for `path`.
pub fn check(path: &Path, named: &Named, formats: &[Format]) -> Result<Checked, AppError> {
    let meta = std::fs::metadata(path).map_err(|_| Code::ReadFailed.named(&named.name))?;
    if !meta.is_file() {
        return Err(Code::NotAFile.named(&named.name));
    }
    let Some(format) = named.format.filter(|f| formats.contains(f)) else {
        return Err(unsupported(&named.extension, formats));
    };
    if meta.len() > MAX_INPUT_BYTES {
        return Err(too_large(&named.name, meta.len()));
    }
    Ok(Checked {
        format,
        size: meta.len(),
        modified_at: meta.modified().ok().and_then(epoch_ms),
        placeholder: file_probe::is_placeholder(path, &meta),
    })
}

/// `unsupported` for a file's extension against the requested formats.
pub fn unsupported(extension: &str, formats: &[Format]) -> AppError {
    Code::Unsupported.error(&[("ext", extension), ("formats", &extension_list(formats))])
}

fn too_large(name: &str, size: u64) -> AppError {
    Code::TooLarge.error(&[
        ("name", name),
        ("size", &format_size(size)),
        ("limit", &format_size(MAX_INPUT_BYTES)),
    ])
}

fn epoch_ms(t: SystemTime) -> Option<i64> {
    let ms = t.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis();
    i64::try_from(ms).ok()
}

/// What the read thread found. An OS error is only `Failed`: its message
/// could quote the path.
enum Got {
    Bytes(Vec<u8>),
    NotRegular,
    Failed,
}

/// §5.4 steps 4–6: open without blocking on a FIFO, check again, read at
/// most `MAX_INPUT_BYTES + 1` bytes, all within `timeout`. A set `cancel`
/// stops the wait (`cancelled`); the thread's bytes are dropped.
pub fn read_bytes(
    path: &Path,
    name: &str,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, AppError> {
    let owned: PathBuf = path.to_path_buf();
    read_with(move || read_capped(&owned), name, timeout, cancel)
}

fn read_capped(path: &Path) -> Got {
    let file = match file_probe::open_regular(path) {
        Ok(Opened::File(file)) => file,
        Ok(Opened::NotRegular) => return Got::NotRegular,
        Err(_) => return Got::Failed,
    };
    let mut bytes = Vec::new();
    match file.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes) {
        Ok(_) => Got::Bytes(bytes),
        Err(_) => Got::Failed,
    }
}

/// Runs `read` on its own thread and waits for it, its deadline or the
/// cancel flag, whichever comes first.
fn read_with(
    read: impl FnOnce() -> Got + Send + 'static,
    name: &str,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, AppError> {
    let (tx, rx) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("file-text-read".into())
        .spawn(move || {
            let _ = tx.send(read());
        });
    if spawned.is_err() {
        return Err(Code::ReadFailed.named(name));
    }
    let deadline = Instant::now() + timeout;
    let outcome = loop {
        match rx.recv_timeout(POLL) {
            Ok(outcome) => break outcome,
            Err(mpsc::RecvTimeoutError::Disconnected) => break Got::Failed,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if cancel.load(Ordering::SeqCst) {
            return Err(Code::Cancelled.named(name));
        }
        if Instant::now() >= deadline {
            return Err(Code::NotDownloaded.named(name));
        }
    };
    match outcome {
        Got::Bytes(bytes) if bytes.len() as u64 > MAX_INPUT_BYTES => {
            Err(too_large(name, bytes.len() as u64))
        }
        Got::Bytes(bytes) => Ok(bytes),
        Got::NotRegular => Err(Code::NotAFile.named(name)),
        Got::Failed => Err(Code::ReadFailed.named(name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_text::outcome_of;

    fn no_cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn names_are_cleaned_and_extensions_lowercased() {
        let n = name_of(Path::new("/some/where/Q3\u{202E} plan.PDF"));
        assert_eq!(n.name, "Q3 plan.PDF");
        assert_eq!(n.extension, "pdf");
        assert_eq!(n.format, Some(Format::Pdf));
        let none = name_of(Path::new("/x/README"));
        assert_eq!(none.extension, "");
        assert_eq!(none.format, None);
        // NFD from HFS+ comes back NFC.
        assert_eq!(name_of(Path::new("cafe\u{301}.txt")).name, "caf\u{e9}.txt");
    }

    #[test]
    fn checks_refuse_folders_wrong_extensions_and_big_files_without_reading() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("pages.rtfd");
        std::fs::create_dir(&folder).unwrap();
        let e = check(&folder, &name_of(&folder), &[Format::Text]).unwrap_err();
        assert_eq!(outcome_of(&e), "not_a_file");

        let md = dir.path().join("a.md");
        std::fs::write(&md, "# hi").unwrap();
        let e = check(&md, &name_of(&md), &[Format::Text, Format::Pdf]).unwrap_err();
        assert_eq!(outcome_of(&e), "unsupported");
        assert_eq!(e.params["ext"], "md");
        assert_eq!(e.params["formats"], ".txt, .pdf");

        // A sparse file over the cap: refused from its size, never read.
        let big = dir.path().join("big.txt");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_INPUT_BYTES + 1).unwrap();
        let e = check(&big, &name_of(&big), &[Format::Text]).unwrap_err();
        assert_eq!(outcome_of(&e), "too_large");
        assert_eq!(e.params["limit"], "32 MB");
        assert_eq!(e.params["name"], "big.txt");

        let missing = dir.path().join("gone.txt");
        let e = check(&missing, &name_of(&missing), &[Format::Text]).unwrap_err();
        assert_eq!(outcome_of(&e), "read_failed");

        let ok = check(&md, &name_of(&md), &[Format::Markdown]).unwrap();
        assert_eq!(ok.format, Format::Markdown);
        assert_eq!(ok.size, 4);
        assert!(ok.modified_at.is_some_and(|ms| ms > 0));
        assert!(!ok.placeholder);
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_not_a_file_and_is_never_opened_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe.txt");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let e = check(&fifo, &name_of(&fifo), &[Format::Text]).unwrap_err();
        assert_eq!(outcome_of(&e), "not_a_file");
        // Swapped in after the checks: the open itself refuses it.
        let e = read_bytes(&fifo, "pipe.txt", Duration::from_secs(5), &no_cancel()).unwrap_err();
        assert_eq!(outcome_of(&e), "not_a_file");
    }

    #[test]
    fn reads_return_the_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"hello").unwrap();
        let bytes = read_bytes(&path, "a.txt", Duration::from_secs(5), &no_cancel()).unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn a_slow_read_times_out_as_not_downloaded_and_a_cancel_stops_the_wait() {
        let slow = || {
            std::thread::sleep(Duration::from_secs(2));
            Got::Bytes(b"late".to_vec())
        };
        let started = Instant::now();
        let e = read_with(slow, "cloud.txt", Duration::from_millis(100), &no_cancel()).unwrap_err();
        assert_eq!(outcome_of(&e), "not_downloaded");
        assert_eq!(e.params["name"], "cloud.txt");
        assert!(started.elapsed() < Duration::from_secs(1));

        let cancel = AtomicBool::new(true);
        let e = read_with(slow, "cloud.txt", Duration::from_secs(30), &cancel).unwrap_err();
        assert_eq!(outcome_of(&e), "cancelled");

        let grown = || Got::Bytes(vec![0; MAX_INPUT_BYTES as usize + 1]);
        let e = read_with(grown, "grew.txt", Duration::from_secs(5), &no_cancel()).unwrap_err();
        assert_eq!(outcome_of(&e), "too_large");
        let failed = || Got::Failed;
        let e = read_with(failed, "x.txt", Duration::from_secs(5), &no_cancel()).unwrap_err();
        assert_eq!(outcome_of(&e), "read_failed");
    }
}
