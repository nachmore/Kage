//! A local corpus run (EXTENSION_FILE_TEXT §12). Skipped unless
//! `KAGE_FILE_TEXT_CORPUS` names a folder:
//!
//!   KAGE_FILE_TEXT_CORPUS=/path/to/files cargo test -p kage-file-text --test corpus -- --nocapture
//!
//! Every file with an extension this build reads goes through the worker
//! binary, one process each, as the app would run it. Each must finish
//! within the deadline, and a successful answer must be within its caps.
//! A crashed worker is counted, not a test failure: the app reports it as
//! `too_complex`. No files are checked in.

use kage_core::file_text_wire::{
    readable_formats, write_request, Format, WorkerRequest, WorkerResponse, DEFAULT_MAX_CHARS,
    MAX_INPUT_BYTES, WORKER_DEADLINE, WORKER_FLAG,
};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const WORKER: &str = env!("CARGO_BIN_EXE_kage-file-text");

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => files_under(&path, out),
            Ok(t) if t.is_file() => out.push(path),
            _ => {}
        }
    }
}

/// Runs one file; returns the outcome name.
fn run_one(path: &Path, format: Format, extension: &str) -> String {
    let bytes = std::fs::read(path).unwrap();
    let request = WorkerRequest::new(readable_formats().collect(), extension, DEFAULT_MAX_CHARS);
    let mut frame = Vec::new();
    write_request(&mut frame, &request, &bytes).unwrap();
    let mut cmd = Command::new(WORKER);
    cmd.env_clear();
    if let Some(root) = std::env::var_os("SystemRoot") {
        cmd.env("SystemRoot", root);
    }
    let mut child = cmd
        .arg(WORKER_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&frame);
    });
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            start.elapsed() < WORKER_DEADLINE + Duration::from_secs(5),
            "{} ran past the deadline",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    let _ = writer.join();
    let out = reader.join().unwrap();
    if !status.success() {
        return "crashed (too_complex)".into();
    }
    match serde_json::from_slice::<WorkerResponse>(&out).expect("one JSON response") {
        WorkerResponse::Ok {
            format: got,
            chars,
            text,
            ..
        } => {
            assert_eq!(got, format, "{}", path.display());
            assert!(chars <= DEFAULT_MAX_CHARS);
            assert_eq!(text.chars().count() as u32, chars);
            "ok".into()
        }
        WorkerResponse::Err { code, .. } => code.as_str().into(),
    }
}

#[test]
fn every_file_finishes_within_its_limits() {
    let Some(root) = std::env::var_os("KAGE_FILE_TEXT_CORPUS") else {
        eprintln!("KAGE_FILE_TEXT_CORPUS isn't set; skipping the corpus run");
        return;
    };
    let mut files = Vec::new();
    files_under(Path::new(&root), &mut files);
    files.sort();
    let mut outcomes: BTreeMap<(&str, String), usize> = BTreeMap::new();
    for path in files {
        let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let extension = extension.to_ascii_lowercase();
        let Some(format) = Format::from_extension(&extension).filter(|f| f.is_readable()) else {
            continue;
        };
        if std::fs::metadata(&path).map_or(true, |m| m.len() > MAX_INPUT_BYTES) {
            continue;
        }
        let outcome = run_one(&path, format, &extension);
        *outcomes.entry((format.id(), outcome)).or_default() += 1;
    }
    for ((format, outcome), n) in &outcomes {
        eprintln!("{format:>8} {outcome:<24} {n}");
    }
}
