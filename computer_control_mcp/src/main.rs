//! Computer Control MCP Server — standalone binary.
//!
//! Speaks MCP (JSON-RPC over stdio) and provides accessibility-based
//! desktop automation tools. Spawned by the agent backend (e.g.
//! kiro-cli) as an MCP server.

use std::io::{self, BufRead, Read, Write};

mod handlers;
mod input_tools;
mod run_outside_sandbox;
mod tool_definitions;

/// `run_outside_sandbox` calls in flight at once. Each waits for the user
/// (up to five minutes) and then the run itself, so they run on threads of
/// their own while the stdio loop keeps answering everything else.
const MAX_BRIDGE_CALLS: usize = 4;

static BRIDGE_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Whether `request` is a `tools/call` of `run_outside_sandbox`.
fn is_bridge_call(request: &mcp_json_rpc::JsonRpcRequest) -> bool {
    request.method == "tools/call"
        && request.params.get("name").and_then(|n| n.as_str())
            == Some(run_outside_sandbox::TOOL_NAME)
}

/// Answer a `run_outside_sandbox` call on its own thread (the reply is
/// written when Kage answers), or at once with an error when too many are
/// already waiting.
fn spawn_bridge_call(request: mcp_json_rpc::JsonRpcRequest) {
    use std::sync::atomic::Ordering;
    if BRIDGE_CALLS.fetch_add(1, Ordering::SeqCst) >= MAX_BRIDGE_CALLS {
        BRIDGE_CALLS.fetch_sub(1, Ordering::SeqCst);
        let busy = mcp_json_rpc::tool_result_text(
            &request.id,
            "Too many commands are already waiting for Kage; wait for them to finish.",
            true,
        );
        write_response(&io::stdout(), &busy);
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("run-outside-sandbox".into())
        .spawn(move || {
            let response = handlers::handle_tool_call(&request.id, &request.params);
            write_response(&io::stdout(), &response);
            BRIDGE_CALLS.fetch_sub(1, Ordering::SeqCst);
        });
    if let Err(e) = spawned {
        BRIDGE_CALLS.fetch_sub(1, Ordering::SeqCst);
        log::warn!("Couldn't start a thread for run_outside_sandbox: {e}");
    }
}

/// The sidecar log is opened in append mode and nothing else prunes it.
/// Each sidecar lives for one agent session, so checking at startup is
/// enough to bound growth.
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

/// Max bytes of content per JSON-RPC message, so a malicious or buggy host
/// cannot OOM us with a single gigantic line.
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

fn main() {
    // Log to file only — stdout/stderr are reserved for JSON-RPC
    // Store alongside the main kage log in %LOCALAPPDATA%/kage/logs/
    let log_dir = dirs::data_local_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from(".")))
        .join("kage")
        .join("logs");
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("Failed to create log dir {:?}: {}", log_dir, e);
    }
    let log_file = log_dir.join("kage-computer-control-mcp.log");
    rotate_log_if_large(&log_file);
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
    {
        Ok(file) => {
            // LineWriter ensures each log line is flushed immediately
            let writer = std::io::LineWriter::new(file);
            match env_logger::Builder::new()
                .target(env_logger::Target::Pipe(Box::new(writer)))
                .filter_level(log::LevelFilter::Debug)
                .format_timestamp_millis()
                .try_init()
            {
                Ok(_) => {}
                Err(e) => eprintln!("Failed to init logger: {}", e),
            }
        }
        Err(e) => eprintln!("Failed to open log file {:?}: {}", log_file, e),
    }

    log::info!(
        "Computer Control MCP server starting (pid={})",
        std::process::id()
    );

    let stdin = io::stdin();
    let stdout = io::stdout();

    // Send initialize response capabilities
    // The MCP host will send an initialize request first

    let mut reader = std::io::BufReader::new(stdin.lock());
    let mut line_buf: Vec<u8> = Vec::new();

    loop {
        match read_bounded_line(&mut reader, &mut line_buf, MAX_LINE_BYTES) {
            Ok(LineRead::Eof) => break,
            Ok(LineRead::Line) => {}
            Ok(LineRead::Oversized) => {
                write_response(&stdout, &mcp_json_rpc::oversized_error());
                continue;
            }
            Err(e) => {
                log::warn!("stdin read error: {}", e);
                break;
            }
        }

        // Decode ourselves rather than via read_line: invalid UTF-8 is a
        // per-message parse error, not a reason to tear down the server.
        let line = match std::str::from_utf8(&line_buf) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Non-UTF-8 JSON-RPC line dropped: {}", e);
                write_response(
                    &stdout,
                    &mcp_json_rpc::error(
                        &serde_json::Value::Null,
                        mcp_json_rpc::ErrorCode::ParseError,
                        "Parse error: invalid UTF-8",
                    ),
                );
                continue;
            }
        };

        let request = match mcp_json_rpc::parse_request(line) {
            mcp_json_rpc::ParseOutcome::Empty => continue,
            mcp_json_rpc::ParseOutcome::Ok(req) => req,
            mcp_json_rpc::ParseOutcome::ParseError(resp) => {
                log::warn!("Invalid JSON-RPC line dropped");
                write_response(&stdout, &resp);
                continue;
            }
        };

        // JSON-RPC 2.0 forbids replying to notifications (initialized,
        // cancelled, roots/list_changed, ...).
        if is_notification_method(&request.method) {
            continue;
        }

        if is_bridge_call(&request) {
            spawn_bridge_call(request);
            continue;
        }

        let response = match request.method.as_str() {
            "initialize" => handlers::handle_initialize(&request.id),
            "tools/list" => tool_definitions::handle_tools_list(&request.id),
            "tools/call" => handlers::handle_tool_call(&request.id, &request.params),
            "ping" => mcp_json_rpc::success(&request.id, serde_json::json!({})),
            other => mcp_json_rpc::error(
                &request.id,
                mcp_json_rpc::ErrorCode::MethodNotFound,
                &format!("Method not found: {}", other),
            ),
        };

        write_response(&stdout, &response);
    }

    log::info!("Computer Control MCP server exiting");
}

fn write_response(stdout: &io::Stdout, response: &str) {
    let mut out = stdout.lock();
    let _ = writeln!(out, "{}", response);
    let _ = out.flush();
}

/// Match on the MCP `notifications/` prefix rather than a null id, so a
/// request that (against spec advice) carries `id: null` still gets an
/// answer.
fn is_notification_method(method: &str) -> bool {
    method.starts_with("notifications/")
}

/// Move an oversized log to `<name>.old` (replacing any previous one) so
/// the next open starts fresh.
fn rotate_log_if_large(log_file: &std::path::Path) {
    let too_big = std::fs::metadata(log_file)
        .map(|m| m.len() > MAX_LOG_BYTES)
        .unwrap_or(false);
    if !too_big {
        return;
    }
    let mut old_name = log_file.as_os_str().to_owned();
    old_name.push(".old");
    // std::fs::rename replaces an existing destination on every platform.
    if let Err(e) = std::fs::rename(log_file, &old_name) {
        eprintln!("Failed to rotate log file {:?}: {}", log_file, e);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum LineRead {
    Eof,
    /// `buf` holds one complete line (or the final unterminated one).
    Line,
    /// The line exceeded the cap; it has been consumed and discarded.
    Oversized,
}

/// Read one newline-terminated line of at most `max` content bytes into
/// `buf` as raw bytes.
fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max: usize,
) -> io::Result<LineRead> {
    buf.clear();
    // +1 so a line of exactly `max` content bytes still fits with its '\n'.
    let n = reader
        .by_ref()
        .take(max as u64 + 1)
        .read_until(b'\n', buf)?;
    if n == 0 {
        return Ok(LineRead::Eof);
    }
    if n <= max || buf.last() == Some(&b'\n') {
        return Ok(LineRead::Line);
    }
    // Cap hit before a newline: skip the rest without buffering it, then
    // resync on the next line.
    buf.clear();
    skip_past_newline(reader)?;
    Ok(LineRead::Oversized)
}

/// Discard bytes up to and including the next '\n' (or EOF) without
/// storing them.
fn skip_past_newline<R: BufRead>(reader: &mut R) -> io::Result<()> {
    loop {
        let (used, done) = {
            let available = match reader.fill_buf() {
                Ok(b) => b,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if available.is_empty() {
                return Ok(());
            }
            match available.iter().position(|&b| b == b'\n') {
                Some(i) => (i + 1, true),
                None => (available.len(), false),
            }
        };
        reader.consume(used);
        if done {
            return Ok(());
        }
    }
}

// JSON-RPC framing lives in `kage_core::mcp_json_rpc` so it's testable
// without pulling in the whole binary.
use kage_core::mcp_json_rpc;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    /// Small BufReader capacity so lines span several fill_buf calls.
    fn reader(data: &[u8]) -> BufReader<Cursor<Vec<u8>>> {
        BufReader::with_capacity(4, Cursor::new(data.to_vec()))
    }

    #[test]
    fn reads_lines_then_eof() {
        let mut r = reader(b"abc\ndef");
        let mut buf = Vec::new();
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 16).unwrap(),
            LineRead::Line
        );
        assert_eq!(buf, b"abc\n");
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 16).unwrap(),
            LineRead::Line
        );
        assert_eq!(buf, b"def");
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 16).unwrap(),
            LineRead::Eof
        );
    }

    #[test]
    fn line_of_exactly_max_bytes_is_accepted_and_next_line_survives() {
        let mut r = reader(b"12345678\nping\n");
        let mut buf = Vec::new();
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 8).unwrap(),
            LineRead::Line
        );
        assert_eq!(buf, b"12345678\n");
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 8).unwrap(),
            LineRead::Line
        );
        assert_eq!(buf, b"ping\n");
    }

    #[test]
    fn oversized_line_is_skipped_and_next_line_survives() {
        let mut r = reader(b"0123456789abcdef\nping\n");
        let mut buf = Vec::new();
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 8).unwrap(),
            LineRead::Oversized
        );
        assert!(buf.is_empty());
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 8).unwrap(),
            LineRead::Line
        );
        assert_eq!(buf, b"ping\n");
    }

    #[test]
    fn oversized_line_split_mid_codepoint_is_reported_not_fatal() {
        // 3-byte chars with a cap of 4 bytes: the cap lands mid-codepoint.
        let mut data = "日日日日".as_bytes().to_vec();
        data.extend_from_slice(b"\nping\n");
        let mut r = reader(&data);
        let mut buf = Vec::new();
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 4).unwrap(),
            LineRead::Oversized
        );
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 4).unwrap(),
            LineRead::Line
        );
        assert_eq!(buf, b"ping\n");
    }

    #[test]
    fn invalid_utf8_line_is_returned_as_bytes() {
        let mut r = reader(b"\xff\xfe\nping\n");
        let mut buf = Vec::new();
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 16).unwrap(),
            LineRead::Line
        );
        assert!(std::str::from_utf8(&buf).is_err());
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 16).unwrap(),
            LineRead::Line
        );
        assert_eq!(buf, b"ping\n");
    }

    #[test]
    fn oversized_final_line_without_newline_hits_eof_cleanly() {
        let mut r = reader(b"0123456789");
        let mut buf = Vec::new();
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 4).unwrap(),
            LineRead::Oversized
        );
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 4).unwrap(),
            LineRead::Eof
        );
    }

    #[test]
    fn notifications_are_recognised_by_prefix() {
        assert!(is_notification_method("notifications/initialized"));
        assert!(is_notification_method("notifications/cancelled"));
        assert!(is_notification_method("notifications/roots/list_changed"));
        assert!(!is_notification_method("ping"));
        assert!(!is_notification_method("tools/call"));
    }
}
