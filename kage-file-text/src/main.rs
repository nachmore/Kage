//! The `kage-file-text` worker (docs/design/EXTENSION_FILE_TEXT.md §5.5).
//!
//! Usage: `kage-file-text --v1`. The app writes one request frame
//! (`kage_core::file_text_wire::write_request`) to stdin and reads one
//! `WorkerResponse` as JSON from stdout. Exit status 0 means stdout holds
//! that response. Anything else (abort, signal, stack overflow, out of
//! memory) the app reports as `too_complex`.
//!
//! Before it reads a byte of input, the worker:
//! - runs under a counting allocator capped at `WORKER_ALLOC_CAP_BYTES`
//!   (from the first allocation on);
//! - puts its OS limits in place (`limits`);
//! - swaps the panic hook for one that prints only where a panic
//!   happened. The message could quote the file, and stderr ends up in
//!   the app's debug log.
//!
//! Then it parses on a thread with a `WORKER_STACK_BYTES` stack, inside
//! `catch_unwind`. A panic is `malformed`.
//!
//! Debug builds also take `--v1-test-<mode>` flags, which the tests use
//! to make the worker hang, crash or flood its output (`test_modes.rs`).
//! Release builds know only `--v1`.

use kage_core::file_text_wire::{self as wire, WORKER_ALLOC_CAP_BYTES, WORKER_STACK_BYTES};
use kage_file_text::alloc::CappedAlloc;
use kage_file_text::{limits, Failure, WorkerErrorCode, WorkerResponse};
use std::io::Write;
use std::panic::AssertUnwindSafe;

#[cfg(debug_assertions)]
mod test_modes;

#[global_allocator]
static ALLOC: CappedAlloc = CappedAlloc::new(WORKER_ALLOC_CAP_BYTES as usize);

const USAGE: &str = "usage: kage-file-text --v1  (a request frame on stdin, JSON on stdout)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Normal,
    #[cfg(debug_assertions)]
    Test(test_modes::TestMode),
}

/// Exactly one argument: `--v1`, or a test flag in debug builds.
fn parse_mode() -> Option<Mode> {
    let mut args = std::env::args_os().skip(1);
    let flag = args.next()?;
    if args.next().is_some() {
        return None;
    }
    let flag = flag.to_str()?;
    if flag == wire::WORKER_FLAG {
        return Some(Mode::Normal);
    }
    #[cfg(debug_assertions)]
    if let Some(test) = test_modes::TestMode::from_flag(flag) {
        return Some(Mode::Test(test));
    }
    None
}

fn main() {
    let Some(mode) = parse_mode() else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    for problem in limits::apply() {
        eprintln!("kage-file-text: {problem}");
    }
    install_panic_hook();

    #[cfg(debug_assertions)]
    if let Mode::Test(test) = mode {
        test_modes::before_input(test, &ALLOC);
    }

    let input = wire::read_request(&mut std::io::stdin().lock());

    #[cfg(debug_assertions)]
    if let Mode::Test(test) = mode {
        test_modes::after_input(test);
    }

    let response = match input {
        Ok((request, bytes)) => run_parser(move || {
            #[cfg(debug_assertions)]
            if let Mode::Test(test) = mode {
                if let Some(response) = test_modes::in_parser(test) {
                    return response;
                }
            }
            #[cfg(not(debug_assertions))]
            let _ = mode;
            kage_file_text::extract(&request, &bytes)
        }),
        Err(_) => Failure::new(WorkerErrorCode::Malformed, "bad request frame").into(),
    };
    write_response(&response);
}

/// Runs the parser on a big stack and turns a panic into `malformed`.
/// A stack overflow still aborts the process, which the app reports as
/// `too_complex`.
fn run_parser(parse: impl FnOnce() -> WorkerResponse + Send + 'static) -> WorkerResponse {
    let thread = std::thread::Builder::new()
        .name("parser".into())
        .stack_size(WORKER_STACK_BYTES as usize)
        .spawn(move || std::panic::catch_unwind(AssertUnwindSafe(parse)));
    let outcome = match thread {
        Ok(handle) => handle.join(),
        Err(_) => {
            return Failure::new(WorkerErrorCode::TooComplex, "parser thread didn't start").into()
        }
    };
    match outcome {
        Ok(Ok(response)) => response,
        _ => Failure::new(WorkerErrorCode::Malformed, "parser panicked").into(),
    }
}

fn write_response(response: &WorkerResponse) -> ! {
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::with_capacity(1 << 16, stdout.lock());
    let written = serde_json::to_writer(&mut out, response).is_ok() && out.flush().is_ok();
    std::process::exit(if written { 0 } else { 1 });
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let place = info.location().map_or_else(
            || "an unknown place".to_string(),
            |l| format!("{}:{}", short_path(l.file()), l.line()),
        );
        eprintln!("kage-file-text: a parser panicked at {place}");
    }));
}

/// The last three components of a source path: the crate and file, not
/// the folder of the machine that built it.
fn short_path(file: &str) -> &str {
    let mut cut = file.len();
    for _ in 0..3 {
        match file[..cut].rfind(['/', '\\']) {
            Some(i) => cut = i,
            None => return file,
        }
    }
    &file[cut + 1..]
}
