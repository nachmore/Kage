//! Computer Control MCP Server — standalone binary.
//!
//! Speaks MCP (JSON-RPC over stdio) and provides accessibility-based
//! desktop automation tools. Spawned by the agent backend (e.g.
//! kiro-cli) as an MCP server.

use std::io::{self, BufRead, Read, Write};

mod handlers;
mod input_tools;
#[cfg(target_os = "macos")]
mod macos_input;
mod tool_definitions;

// ---------------------------------------------------------------------------
// Mouse SendInput helper. Uses the windows crate's INPUT/MOUSEINPUT — these
// types have correct layout on every supported architecture, unlike a hand-
// rolled MouseInput struct which would only work on x64 by accident of
// padding. The crate version was previously avoided here under a "version
// conflicts" comment that never quite held — both this binary and the lib
// link the same windows crate, so we just enable the matching feature
// (Win32_UI_Input_KeyboardAndMouse) and use what's there.
// ---------------------------------------------------------------------------
#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEINPUT, MOUSE_EVENT_FLAGS,
};

#[cfg(target_os = "windows")]
fn win32_mouse_event(flags: MOUSE_EVENT_FLAGS, data: i32) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

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

    // Read length-capped lines directly from a BufReader so a malicious or buggy
    // host cannot OOM us with a single gigantic line.
    const MAX_LINE_BYTES: usize = 4 * 1024 * 1024; // 4 MiB per JSON-RPC message
    let mut reader = std::io::BufReader::new(stdin.lock());
    let mut line_buf = String::new();

    loop {
        line_buf.clear();
        // Use take() on the underlying reader to bound how much we'll read for
        // a single line. If the cap is hit before a newline, we flush the
        // oversized data and emit an error response.
        let mut bounded = (&mut reader).take((MAX_LINE_BYTES + 1) as u64);
        let n = match bounded.read_line(&mut line_buf) {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(e) => {
                log::warn!("stdin read error: {}", e);
                break;
            }
        };

        if n > MAX_LINE_BYTES {
            // Drain the rest of the oversized line so we resync on the next newline.
            let mut discard = String::new();
            let _ = reader.read_line(&mut discard);
            let err = mcp_json_rpc::oversized_error();
            let mut out = stdout.lock();
            let _ = writeln!(out, "{}", err);
            let _ = out.flush();
            continue;
        }

        let request = match mcp_json_rpc::parse_request(&line_buf) {
            mcp_json_rpc::ParseOutcome::Empty => continue,
            mcp_json_rpc::ParseOutcome::Ok(req) => req,
            mcp_json_rpc::ParseOutcome::ParseError(resp) => {
                log::warn!("Invalid JSON-RPC line dropped");
                let mut out = stdout.lock();
                let _ = writeln!(out, "{}", resp);
                let _ = out.flush();
                continue;
            }
        };

        let response = match request.method.as_str() {
            "initialize" => handlers::handle_initialize(&request.id),
            "tools/list" => tool_definitions::handle_tools_list(&request.id),
            "tools/call" => handlers::handle_tool_call(&request.id, &request.params),
            "notifications/initialized" | "ping" => {
                // Notifications — no response needed (but ping gets a pong)
                if request.method == "ping" {
                    mcp_json_rpc::success(&request.id, serde_json::json!({}))
                } else {
                    continue;
                }
            }
            other => mcp_json_rpc::error(
                &request.id,
                mcp_json_rpc::ErrorCode::MethodNotFound,
                &format!("Method not found: {}", other),
            ),
        };

        let mut out = stdout.lock();
        let _ = writeln!(out, "{}", response);
        let _ = out.flush();
    }

    log::info!("Computer Control MCP server exiting");
}

// JSON-RPC framing lives in `kage::mcp_json_rpc` so it's testable without
// pulling in the whole binary. The thin local aliases below are kept for
// readability of the existing handler bodies — they desugar to the new
// typed builders.
use kage::mcp_json_rpc;

fn tool_result_text(id: &serde_json::Value, text: &str, is_error: bool) -> String {
    mcp_json_rpc::tool_result_text(id, text, is_error)
}
