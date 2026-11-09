//! Debug-only ways to make the worker misbehave, for tests
//! (EXTENSION_FILE_TEXT §12). Release builds don't compile this module,
//! so their worker refuses these flags.
//!
//! | Flag | What the worker does |
//! |---|---|
//! | `--v1-test-limits` | After the limits and before reading stdin: prints them as JSON and exits 0 |
//! | `--v1-test-oom` | Before reading stdin: allocates past the allocator cap (aborts) |
//! | `--v1-test-hang` | Reads the request, then sleeps forever |
//! | `--v1-test-abort` | Reads the request, then aborts |
//! | `--v1-test-flood` | Reads the request, then writes to stdout without end |
//! | `--v1-test-panic` | Panics in the parser thread (answers `malformed`) |
//! | `--v1-test-deep` | Recurses 24 MiB deep in the parser thread, then answers `ok` |

use kage_file_text::alloc::CappedAlloc;
use kage_file_text::{limits, Format, WorkerResponse};
use std::io::Write;
use std::time::Duration;

/// Stack `--v1-test-deep` uses: well past an 8 MiB main thread stack,
/// well inside the parser's 64 MiB.
const DEEP_BYTES: usize = 24 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestMode {
    Limits,
    Oom,
    Hang,
    Abort,
    Flood,
    Panic,
    Deep,
}

impl TestMode {
    pub fn from_flag(flag: &str) -> Option<Self> {
        Some(match flag.strip_prefix("--v1-test-")? {
            "limits" => Self::Limits,
            "oom" => Self::Oom,
            "hang" => Self::Hang,
            "abort" => Self::Abort,
            "flood" => Self::Flood,
            "panic" => Self::Panic,
            "deep" => Self::Deep,
            _ => return None,
        })
    }
}

/// Runs where the worker is about to read stdin.
pub fn before_input(mode: TestMode, alloc: &CappedAlloc) {
    match mode {
        TestMode::Limits => {
            let mut report = limits::report();
            if !report.is_object() {
                report = serde_json::json!({});
            }
            report["alloc_cap"] = serde_json::json!(alloc.cap());
            report["alloc_live"] = serde_json::json!(alloc.live());
            println!("{report}");
            std::process::exit(0);
        }
        TestMode::Oom => {
            let big: Vec<u8> = Vec::with_capacity(alloc.cap() + 1);
            std::hint::black_box(&big);
            // Not reached: the allocator refuses and Rust aborts.
            std::process::exit(0);
        }
        _ => {}
    }
}

/// Runs right after the request was read.
pub fn after_input(mode: TestMode) {
    match mode {
        TestMode::Hang => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        TestMode::Abort => std::process::abort(),
        TestMode::Flood => {
            let chunk = [b'x'; 64 * 1024];
            let mut out = std::io::stdout().lock();
            while out.write_all(&chunk).is_ok() {}
            std::process::exit(1);
        }
        _ => {}
    }
}

/// Runs inside the parser thread instead of the parser.
pub fn in_parser(mode: TestMode) -> Option<WorkerResponse> {
    match mode {
        TestMode::Panic => panic!("test panic"),
        TestMode::Deep => {
            let top = std::hint::black_box(0u8);
            let used = recurse(&top as *const u8 as usize);
            let text = format!("{used} bytes deep");
            Some(WorkerResponse::Ok {
                format: Format::Text,
                chars: text.chars().count() as u32,
                text,
                truncated: false,
                total_chars: None,
                title: None,
                pages: None,
                page_starts: None,
                warnings: Vec::new(),
            })
        }
        _ => None,
    }
}

/// Recurses until the stack is `DEEP_BYTES` deep below `top`, and
/// returns how deep it got.
#[inline(never)]
fn recurse(top: usize) -> usize {
    let frame = std::hint::black_box([0u8; 1024]);
    let used = top.abs_diff(frame.as_ptr() as usize);
    if used >= DEEP_BYTES {
        return used;
    }
    recurse(top).max(usize::from(std::hint::black_box(frame)[0]))
}
