//! Computer Control MCP Server — standalone binary.
//!
//! Speaks MCP (JSON-RPC over stdio) and provides accessibility-based
//! desktop automation tools. Spawned by kage-cli as an MCP server.

use std::io::{self, BufRead, Read, Write};

use kage::os::accessibility;

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
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_WHEEL, MOUSE_EVENT_FLAGS, MOUSEINPUT,
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
    match std::fs::OpenOptions::new().create(true).append(true).open(&log_file) {
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

    log::info!("Computer Control MCP server starting (pid={})", std::process::id());

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
            "initialize" => handle_initialize(&request.id),
            "tools/list" => handle_tools_list(&request.id),
            "tools/call" => handle_tool_call(&request.id, &request.params),
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

fn json_rpc_result(id: &serde_json::Value, result: serde_json::Value) -> String {
    mcp_json_rpc::success(id, result)
}

fn tool_result_text(id: &serde_json::Value, text: &str, is_error: bool) -> String {
    mcp_json_rpc::tool_result_text(id, text, is_error)
}

// ---------------------------------------------------------------------------
// MCP protocol handlers
// ---------------------------------------------------------------------------
fn handle_initialize(id: &serde_json::Value) -> String {
    json_rpc_result(id, serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "computer-control",
            "version": env!("CARGO_PKG_VERSION"),
        }
    }))
}

fn handle_tools_list(id: &serde_json::Value) -> String {
    let tools = serde_json::json!({ "tools": [
        tool_def("get_ui_tree", "Get the accessibility tree for a window.", serde_json::json!({
            "type": "object",
            "properties": {
                "window_title": { "type": "string", "description": "Substring match on window title. Uses focused window if omitted." },
                "max_depth": { "type": "integer", "default": 3, "description": "How deep to walk the tree." },
                "include_invisible": { "type": "boolean", "default": false }
            }
        })),
        tool_def("find_elements", "Search for UI elements matching criteria.", serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }, "role": { "type": "string" },
                "automation_id": { "type": "string" }, "value": { "type": "string" },
                "window_title": { "type": "string" }
            }
        })),
        tool_def("get_focused_element", "Get the currently focused UI element.", serde_json::json!({ "type": "object", "properties": {} })),
        tool_def("list_windows", "List visible top-level windows.", serde_json::json!({
            "type": "object",
            "properties": { "title_filter": { "type": "string" } }
        })),
        tool_def("click_element", "Click/invoke a UI element by ID.", serde_json::json!({
            "type": "object", "properties": { "element_id": { "type": "string" } }, "required": ["element_id"]
        })),
        tool_def("focus_element", "Set keyboard focus to a UI element without moving the mouse. Prefer this over click_element when you need to type into an element.", serde_json::json!({
            "type": "object", "properties": { "element_id": { "type": "string" } }, "required": ["element_id"]
        })),
        tool_def("set_value", "Set the value of a text field.", serde_json::json!({
            "type": "object", "properties": { "element_id": { "type": "string" }, "value": { "type": "string" } }, "required": ["element_id", "value"]
        })),
        tool_def("toggle_element", "Toggle a checkbox or switch.", serde_json::json!({
            "type": "object", "properties": { "element_id": { "type": "string" } }, "required": ["element_id"]
        })),
        tool_def("select_element", "Select an item in a list/combo/tab.", serde_json::json!({
            "type": "object", "properties": { "element_id": { "type": "string" } }, "required": ["element_id"]
        })),
        tool_def("expand_element", "Expand a tree node or dropdown.", serde_json::json!({
            "type": "object", "properties": { "element_id": { "type": "string" } }, "required": ["element_id"]
        })),
        tool_def("collapse_element", "Collapse a tree node or dropdown.", serde_json::json!({
            "type": "object", "properties": { "element_id": { "type": "string" } }, "required": ["element_id"]
        })),
        tool_def("scroll_element", "Scroll within a scrollable container.", serde_json::json!({
            "type": "object",
            "properties": {
                "element_id": { "type": "string" },
                "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                "amount": { "type": "number", "default": 0.2 }
            },
            "required": ["element_id"]
        })),
        tool_def("get_element_text", "Read text content from a text element.", serde_json::json!({
            "type": "object", "properties": { "element_id": { "type": "string" } }, "required": ["element_id"]
        })),
        tool_def("get_element_children", "Drill into a specific element's subtree.", serde_json::json!({
            "type": "object",
            "properties": { "element_id": { "type": "string" }, "max_depth": { "type": "integer", "default": 2 } },
            "required": ["element_id"]
        })),
        // Compound tools
        tool_def("launch_and_get_tree", "Launch an app, wait for it to load, and return its UI tree.", serde_json::json!({
            "type": "object",
            "properties": {
                "app_name": { "type": "string", "description": "Application name or path to launch" },
                "max_depth": { "type": "integer", "default": 3 },
                "wait_ms": { "type": "integer", "default": 2000, "description": "Milliseconds to wait after launch" }
            },
            "required": ["app_name"]
        })),
        tool_def("click_and_get_tree", "Click an element and return the updated UI tree.", serde_json::json!({
            "type": "object",
            "properties": { "element_id": { "type": "string" }, "window_title": { "type": "string" }, "max_depth": { "type": "integer", "default": 3 } },
            "required": ["element_id"]
        })),
        tool_def("click_and_read_result", "Click an element and read a specific result element.", serde_json::json!({
            "type": "object",
            "properties": { "element_id": { "type": "string" }, "result_name": { "type": "string", "description": "Name of the element to read after clicking" }, "window_title": { "type": "string" } },
            "required": ["element_id", "result_name"]
        })),
        tool_def("type_and_get_tree", "Type text into an element and return the updated UI tree.", serde_json::json!({
            "type": "object",
            "properties": { "element_id": { "type": "string" }, "text": { "type": "string" }, "window_title": { "type": "string" }, "max_depth": { "type": "integer", "default": 3 } },
            "required": ["element_id", "text"]
        })),
        // App steering
        tool_def("get_app_steering", "Get app-specific automation tips.", serde_json::json!({
            "type": "object",
            "properties": { "task": { "type": "string" }, "details": { "type": "string" } },
            "required": ["task"]
        })),
        // Fallback tools
        tool_def("launch_app", "Launch an application by name.", serde_json::json!({
            "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"]
        })),
        tool_def("list_all_windows", "List all windows including minimized ones.", serde_json::json!({
            "type": "object", "properties": { "title_filter": { "type": "string" } }
        })),
        tool_def("type_text", "Type text using keyboard simulation.", serde_json::json!({
            "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"]
        })),
        tool_def("key_press", "Press key combinations (e.g. 'ctrl+c', 'enter').", serde_json::json!({
            "type": "object", "properties": { "keys": { "type": "string" } }, "required": ["keys"]
        })),
        tool_def("key_press_confirmed", "Execute a dangerous key combination after confirmation.", serde_json::json!({
            "type": "object", "properties": { "keys": { "type": "string" }, "confirm": { "type": "boolean" } }, "required": ["keys"]
        })),
        tool_def("click", "Click at screen coordinates. FALLBACK — prefer click_element().", serde_json::json!({
            "type": "object",
            "properties": {
                "x": { "type": "integer" }, "y": { "type": "integer" },
                "button": { "type": "string", "enum": ["left", "right", "middle"], "default": "left" },
                "count": { "type": "integer", "default": 1 }
            }
        })),
        tool_def("drag", "Click and drag between two points.", serde_json::json!({
            "type": "object",
            "properties": {
                "from_x": { "type": "integer" }, "from_y": { "type": "integer" },
                "to_x": { "type": "integer" }, "to_y": { "type": "integer" },
                "duration": { "type": "number", "default": 0.5 },
                "button": { "type": "string", "default": "left" }
            },
            "required": ["from_x", "from_y", "to_x", "to_y"]
        })),
        tool_def("scroll", "Scroll the mouse wheel at coordinates. FALLBACK — prefer scroll_element().", serde_json::json!({
            "type": "object",
            "properties": {
                "direction": { "type": "string", "enum": ["up", "down"], "default": "down" },
                "amount": { "type": "integer", "default": 3 },
                "x": { "type": "integer" }, "y": { "type": "integer" }
            }
        })),
        tool_def("move_mouse", "Move the mouse cursor to an absolute position.", serde_json::json!({
            "type": "object", "properties": { "x": { "type": "integer" }, "y": { "type": "integer" } }, "required": ["x", "y"]
        })),
        tool_def("wait", "Wait for a specified number of milliseconds.", serde_json::json!({
            "type": "object", "properties": { "milliseconds": { "type": "integer", "default": 500 } }
        })),
        tool_def("get_cursor_position", "Get the current mouse cursor position.", serde_json::json!({ "type": "object", "properties": {} })),
        tool_def("get_screen_size", "Get the screen dimensions.", serde_json::json!({ "type": "object", "properties": {} })),
        // Folder tools
        tool_def("get_common_folders", "Get a map of well-known folder names (downloads, documents, pictures, desktop, etc.) to their absolute paths on this system. Use this to resolve folder names the user mentions.", serde_json::json!({
            "type": "object", "properties": {}
        })),
        tool_def("pick_folder", "Open a native folder picker dialog so the user can select a folder. Returns the chosen path or null if cancelled.", serde_json::json!({
            "type": "object", "properties": {}
        })),
        tool_def("scan_folder", "Scan a folder recursively and return a manifest of all files and directories with sizes, dates, and content hashes for duplicate detection.", serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path to the folder to scan" },
                "max_depth": { "type": "integer", "default": 10, "description": "Maximum recursion depth" },
                "compute_hashes": { "type": "boolean", "default": true, "description": "Compute content hashes for duplicate detection" }
            },
            "required": ["path"]
        })),
        tool_def("execute_folder_plan", "Execute a folder organization plan: move, rename, or delete files. Deletes are safe — files go to a _kage_trash subfolder. Returns success/failure counts.", serde_json::json!({
            "type": "object",
            "properties": {
                "root": { "type": "string", "description": "Absolute path to the root folder" },
                "operations": {
                    "type": "array",
                    "description": "Array of operations. Each: {action: 'move'|'rename'|'delete', from: 'relative/path', to: 'dest/path', reason: 'why'}",
                    "items": { "type": "object" }
                }
            },
            "required": ["root", "operations"]
        })),
    ]});
    json_rpc_result(id, tools)
}

fn tool_def(name: &str, description: &str, input_schema: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "name": name, "description": description, "inputSchema": input_schema })
}

/// Launch an app using ShellExecuteW — the proper Win32 API.
/// Handles program names with args (e.g. "winword /w"), paths, and URIs.
/// Launch an app using ShellExecuteW — the proper Win32 API.
/// Handles program names with args (e.g. "winword /w"), paths, and URIs.
fn shell_launch(name: &str) -> Result<(), String> {
    use windows::core::HSTRING;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    // Detect if this is a file path (contains backslash, or drive letter like C:/)
    let is_path = name.contains('\\') || (name.len() >= 3 && name.as_bytes().get(1) == Some(&b':'));
    let (file_str, params_str) = if name.contains(' ') && !is_path {
        let mut parts = name.splitn(2, ' ');
        let prog = parts.next().unwrap_or(name);
        let args = parts.next().unwrap_or("");
        (prog, args)
    } else {
        (name, "")
    };

    let op = HSTRING::from("open");
    let file = HSTRING::from(file_str);

    log::info!("[shell_launch] file='{}' params='{}'", file_str, params_str);

    let result = unsafe {
        if params_str.is_empty() {
            ShellExecuteW(None, &op, &file, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
        } else {
            let params = HSTRING::from(params_str);
            ShellExecuteW(None, &op, &file, &params, PCWSTR::null(), SW_SHOWNORMAL)
        }
    };

    if result.0 as usize > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed with code {} for '{}'", result.0 as usize, name))
    }
}


// ---------------------------------------------------------------------------
// Tool call dispatch
// ---------------------------------------------------------------------------
fn handle_tool_call(id: &serde_json::Value, params: &serde_json::Value) -> String {
    let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));
    log::info!("[tool_call] {} args={}", tool_name, args);

    match tool_name {
        "get_ui_tree" => {
            let title = args.get("window_title").and_then(|v| v.as_str());
            let depth = args.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
            let invisible = args.get("include_invisible").and_then(|v| v.as_bool()).unwrap_or(false);
            match accessibility::get_ui_tree(title, depth, invisible) {
                Ok(elem) => {
                    let mut text = String::new();
                    if !elem.meta.is_empty() { text.push_str(&elem.meta); text.push('\n'); }
                    text.push_str(&elem.to_text(0, depth));
                    tool_result_text(id, &text, false)
                }
                Err(e) => tool_result_text(id, &e, true),
            }
        }
        "find_elements" => {
            let params = accessibility::FindElementsParams {
                name: args.get("name").and_then(|v| v.as_str()).map(String::from),
                role: args.get("role").and_then(|v| v.as_str()).map(String::from),
                automation_id: args.get("automation_id").and_then(|v| v.as_str()).map(String::from),
                value: args.get("value").and_then(|v| v.as_str()).map(String::from),
                window_title: args.get("window_title").and_then(|v| v.as_str()).map(String::from),
            };
            match accessibility::find_elements(&params) {
                Ok(elems) => {
                    let text: String = if elems.is_empty() {
                        "No matching elements found.".to_string()
                    } else {
                        elems.iter().map(|e| e.to_text(0, 0)).collect::<Vec<_>>().join("\n")
                    };
                    tool_result_text(id, &text, false)
                }
                Err(e) => tool_result_text(id, &e, true),
            }
        }
        "get_focused_element" => {
            match accessibility::get_focused_element() {
                Ok(Some(elem)) => tool_result_text(id, &elem.to_text(0, 0), false),
                Ok(None) => tool_result_text(id, "No focused element found.", false),
                Err(e) => tool_result_text(id, &e, true),
            }
        }
        "list_windows" => {
            let filter = args.get("title_filter").and_then(|v| v.as_str());
            match accessibility::list_accessible_windows(filter) {
                Ok(wins) => {
                    let text = if wins.is_empty() {
                        "No visible windows found.".to_string()
                    } else {
                        wins.iter().map(|w| {
                            let b = w.bounds.map(|(x,y,ww,h)| format!(" ({}x{}@{},{})", ww, h, x, y)).unwrap_or_default();
                            format!("[window] \"{}\" pid={} process={}{}", w.title, w.process_id, w.process_name, b)
                        }).collect::<Vec<_>>().join("\n")
                    };
                    tool_result_text(id, &text, false)
                }
                Err(e) => tool_result_text(id, &e, true),
            }
        }
        "click_element" => dispatch_element_action(id, &args, accessibility::click_element),
        "focus_element" => dispatch_element_action(id, &args, accessibility::focus_element),
        "set_value" => {
            let eid = args.get("element_id").and_then(|v| v.as_str()).unwrap_or("");
            let val = args.get("value").and_then(|v| v.as_str()).unwrap_or("");
            match accessibility::set_element_value(eid, val) {
                Ok(msg) => tool_result_text(id, &msg, false),
                Err(e) => tool_result_text(id, &e, true),
            }
        }
        "toggle_element" => dispatch_element_action(id, &args, accessibility::toggle_element),
        "select_element" => dispatch_element_action(id, &args, accessibility::select_element),
        "expand_element" => dispatch_element_action(id, &args, accessibility::expand_element),
        "collapse_element" => dispatch_element_action(id, &args, accessibility::collapse_element),
        "scroll_element" => {
            let eid = args.get("element_id").and_then(|v| v.as_str()).unwrap_or("");
            let dir = args.get("direction").and_then(|v| v.as_str()).unwrap_or("down");
            let amt = args.get("amount").and_then(|v| v.as_f64()).unwrap_or(0.2);
            match accessibility::scroll_element(eid, dir, amt) {
                Ok(msg) => tool_result_text(id, &msg, false),
                Err(e) => tool_result_text(id, &e, true),
            }
        }
        "get_element_text" => dispatch_element_action(id, &args, accessibility::get_element_text),
        "get_element_children" => {
            let eid = args.get("element_id").and_then(|v| v.as_str()).unwrap_or("");
            let depth = args.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(2) as usize;
            match accessibility::get_element_children(eid, depth) {
                Ok(elem) => tool_result_text(id, &elem.to_text(0, depth), false),
                Err(e) => tool_result_text(id, &e, true),
            }
        }
        // Compound tools
        "launch_and_get_tree" => {
            let app = args.get("app_name").and_then(|v| v.as_str()).unwrap_or("");
            let depth = args.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
            let wait = args.get("wait_ms").and_then(|v| v.as_u64()).unwrap_or(2000);
            log::info!("[launch_and_get_tree] Launching: '{}' (wait={}ms, depth={})", app, wait, depth);
            let launch_result = shell_launch(app);
            match launch_result {
                Ok(_) => {
                    std::thread::sleep(std::time::Duration::from_millis(wait));
                    // Try to find the window and get its tree
                    match accessibility::get_ui_tree(None, depth, false) {
                        Ok(elem) => {
                            let mut text = format!("Launched '{}'. UI tree:\n", app);
                            if !elem.meta.is_empty() { text.push_str(&elem.meta); text.push('\n'); }
                            text.push_str(&elem.to_text(0, depth));
                            tool_result_text(id, &text, false)
                        }
                        Err(e) => tool_result_text(id, &format!("Launched '{}' but failed to get tree: {}", app, e), true),
                    }
                }
                Err(e) => tool_result_text(id, &format!("Failed to launch '{}': {}", app, e), true),
            }
        }
        "click_and_get_tree" => {
            let eid = args.get("element_id").and_then(|v| v.as_str()).unwrap_or("");
            let title = args.get("window_title").and_then(|v| v.as_str());
            let depth = args.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
            let click_msg = accessibility::click_element(eid);
            std::thread::sleep(std::time::Duration::from_millis(300));
            match accessibility::get_ui_tree(title, depth, false) {
                Ok(elem) => {
                    let click_str = click_msg.unwrap_or_else(|e| format!("Click failed: {}", e));
                    let mut text = format!("{}\n\nUpdated tree:\n", click_str);
                    text.push_str(&elem.to_text(0, depth));
                    tool_result_text(id, &text, false)
                }
                Err(e) => tool_result_text(id, &format!("Click succeeded but tree failed: {}", e), true),
            }
        }
        "click_and_read_result" => {
            let eid = args.get("element_id").and_then(|v| v.as_str()).unwrap_or("");
            let result_name = args.get("result_name").and_then(|v| v.as_str()).unwrap_or("");
            let title = args.get("window_title").and_then(|v| v.as_str());
            let click_msg = accessibility::click_element(eid);
            std::thread::sleep(std::time::Duration::from_millis(300));
            let find_params = accessibility::FindElementsParams {
                name: Some(result_name.to_string()),
                role: None, automation_id: None, value: None,
                window_title: title.map(String::from),
            };
            match accessibility::find_elements(&find_params) {
                Ok(elems) => {
                    let click_str = click_msg.unwrap_or_else(|e| format!("Click failed: {}", e));
                    let result_text = elems.first().map(|e| {
                        if !e.value.is_empty() { e.value.clone() } else { e.name.clone() }
                    }).unwrap_or_else(|| format!("Element '{}' not found", result_name));
                    tool_result_text(id, &format!("{}\nResult: {}", click_str, result_text), false)
                }
                Err(e) => tool_result_text(id, &format!("Click succeeded but find failed: {}", e), true),
            }
        }
        "type_and_get_tree" => {
            let eid = args.get("element_id").and_then(|v| v.as_str()).unwrap_or("");
            let text_val = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let title = args.get("window_title").and_then(|v| v.as_str());
            let depth = args.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
            let set_msg = accessibility::set_element_value(eid, text_val);
            std::thread::sleep(std::time::Duration::from_millis(300));
            match accessibility::get_ui_tree(title, depth, false) {
                Ok(elem) => {
                    let set_str = set_msg.unwrap_or_else(|e| format!("Type failed: {}", e));
                    let mut text = format!("{}\n\nUpdated tree:\n", set_str);
                    text.push_str(&elem.to_text(0, depth));
                    tool_result_text(id, &text, false)
                }
                Err(e) => tool_result_text(id, &format!("Type succeeded but tree failed: {}", e), true),
            }
        }
        "get_app_steering" => {
            let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("");
            let details = args.get("details").and_then(|v| v.as_str()).unwrap_or("");
            let combined = format!("{} {}", task, details).to_lowercase();
            // Embedded app steering files
            const STEERING: &[(&str, &str)] = &[
                ("calculator", include_str!("../computer_control/app_steering/calculator.md")),
                ("microsoft_office", include_str!("../computer_control/app_steering/microsoft_office.md")),
                ("notepad", include_str!("../computer_control/app_steering/notepad.md")),
            ];
            const APP_PATTERNS: &[(&str, &[&str])] = &[
                ("microsoft_office", &["word", "winword", "excel", "powerpnt", "powerpoint", "outlook", "onenote", "access", "publisher", "visio"]),
                ("notepad", &["notepad"]),
                ("calculator", &["calc", "calculator"]),
            ];
            let mut result = String::from("No app-specific steering found for this task.");
            'outer: for (key, patterns) in APP_PATTERNS {
                for pattern in *patterns {
                    if combined.contains(pattern) {
                        if let Some((_, content)) = STEERING.iter().find(|(k, _)| k == key) {
                            result = content.to_string();
                            break 'outer;
                        }
                    }
                }
            }
            tool_result_text(id, &result, false)
        }
        // Utility tools
        "launch_app" => {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
            log::info!("[launch_app] Attempting to launch: '{}'", name);
            match shell_launch(name) {
                Ok(_) => tool_result_text(id, &format!("Launched '{}'", name), false),
                Err(e) => {
                    log::info!("[launch_app] Failed: {}", e);
                    tool_result_text(id, &format!("Failed to launch '{}': {}", name, e), true)
                },
            }
        }
        "list_all_windows" => {
            let filter = args.get("title_filter").and_then(|v| v.as_str());
            match accessibility::list_accessible_windows(filter) {
                Ok(wins) => {
                    let text = if wins.is_empty() { "No windows found.".to_string() } else {
                        wins.iter().map(|w| {
                            let b = w.bounds.map(|(x,y,ww,h)| format!(" ({}x{}@{},{})", ww, h, x, y)).unwrap_or_default();
                            format!("[window] \"{}\" pid={} process={}{}", w.title, w.process_id, w.process_name, b)
                        }).collect::<Vec<_>>().join("\n")
                    };
                    tool_result_text(id, &text, false)
                }
                Err(e) => tool_result_text(id, &e, true),
            }
        }
        "type_text" => {
            let text_val = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            log::info!("[type_text] Typing {} chars: {:?}", text_val.len(), text_val);
            #[cfg(target_os = "windows")]
            {
                let kb = uiautomation::inputs::Keyboard::new();
                // Handle newlines: split text on \n and send Enter between lines
                let lines: Vec<&str> = text_val.split('\n').collect();
                for (i, line) in lines.iter().enumerate() {
                    if !line.is_empty() {
                        if let Err(e) = kb.send_text(line) {
                            return tool_result_text(id, &format!("Failed to type line {}: {}", i + 1, e), true);
                        }
                    }
                    // Send Enter between lines (not after the last one)
                    if i < lines.len() - 1 {
                        if let Err(e) = kb.send_keys("{Enter}") {
                            return tool_result_text(id, &format!("Failed to send Enter: {}", e), true);
                        }
                    }
                }
                tool_result_text(id, &format!("Typed {} characters ({} lines)", text_val.len(), lines.len()), false)
            }
            #[cfg(not(target_os = "windows"))]
            { tool_result_text(id, "type_text not available on this platform", true) }
        }
        "key_press" => {
            let keys = args.get("keys").and_then(|v| v.as_str()).unwrap_or("");
            let dangerous = ["alt+f4", "ctrl+w", "ctrl+q", "alt+f4"];
            let normalized = keys.trim().to_lowercase().replace(" ", "");
            if dangerous.iter().any(|&d| normalized == d) {
                tool_result_text(id, &format!("⚠️ DANGEROUS: '{}' — call key_press_confirmed(keys='{}', confirm=true) to proceed.", keys, keys), false)
            } else {
                #[cfg(target_os = "windows")]
                {
                    let kb = uiautomation::inputs::Keyboard::new();
                    // Convert "ctrl+s" format to uiautomation "{Ctrl}s" format
                    let uia_keys = convert_key_combo(keys);
                    match kb.send_keys(&uia_keys) {
                        Ok(_) => tool_result_text(id, &format!("Pressed: {}", keys), false),
                        Err(e) => tool_result_text(id, &format!("Failed to press '{}': {}", keys, e), true),
                    }
                }
                #[cfg(not(target_os = "windows"))]
                { tool_result_text(id, "key_press not available on this platform", true) }
            }
        }
        "key_press_confirmed" => {
            let keys = args.get("keys").and_then(|v| v.as_str()).unwrap_or("");
            let confirm = args.get("confirm").and_then(|v| v.as_bool()).unwrap_or(false);
            if !confirm {
                tool_result_text(id, "Cancelled — confirm must be true.", false)
            } else {
                #[cfg(target_os = "windows")]
                {
                    let kb = uiautomation::inputs::Keyboard::new();
                    let uia_keys = convert_key_combo(keys);
                    match kb.send_keys(&uia_keys) {
                        Ok(_) => tool_result_text(id, &format!("Executed: {}", keys), false),
                        Err(e) => tool_result_text(id, &format!("Failed: {}", e), true),
                    }
                }
                #[cfg(not(target_os = "windows"))]
                { tool_result_text(id, "key_press not available on this platform", true) }
            }
        }
        "click" => {
            let x = args.get("x").and_then(|v| v.as_i64());
            let y = args.get("y").and_then(|v| v.as_i64());
            let button = args.get("button").and_then(|v| v.as_str()).unwrap_or("left");
            let count = args.get("count").and_then(|v| v.as_u64()).unwrap_or(1);
            #[cfg(target_os = "windows")]
            {
                let mouse = uiautomation::inputs::Mouse::new().auto_move(true).move_time(50);
                if let (Some(px), Some(py)) = (x, y) {
                    let pt = uiautomation::types::Point::new(px as i32, py as i32);
                    let result = match (button, count) {
                        ("right", _) => mouse.right_click(&pt),
                        (_, 2) => mouse.double_click(&pt),
                        _ => mouse.click(&pt),
                    };
                    match result {
                        Ok(_) => tool_result_text(id, &format!("Clicked {} at ({}, {})", button, px, py), false),
                        Err(e) => tool_result_text(id, &format!("Click failed: {}", e), true),
                    }
                } else {
                    let pos = uiautomation::inputs::Mouse::get_cursor_pos().unwrap_or(uiautomation::types::Point::new(0, 0));
                    tool_result_text(id, &format!("Clicked {} at ({}, {})", button, pos.get_x(), pos.get_y()), false)
                }
            }
            #[cfg(not(target_os = "windows"))]
            { tool_result_text(id, "click not available on this platform", true) }
        }
        "drag" => {
            let from_x = args.get("from_x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let from_y = args.get("from_y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let to_x = args.get("to_x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let to_y = args.get("to_y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let duration = args.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.5);
            #[cfg(target_os = "windows")]
            {
                let _ = uiautomation::inputs::Mouse::set_cursor_pos(
                    &uiautomation::types::Point::new(from_x, from_y)
                );
                std::thread::sleep(std::time::Duration::from_millis(50));
                // Press, move in steps, release
                win32_mouse_event(MOUSEEVENTF_LEFTDOWN, 0);
                let steps = (duration * 60.0).max(10.0) as i32;
                let dx = (to_x - from_x) as f64 / steps as f64;
                let dy = (to_y - from_y) as f64 / steps as f64;
                for i in 1..=steps {
                    let _ = uiautomation::inputs::Mouse::set_cursor_pos(
                        &uiautomation::types::Point::new(from_x + (dx * i as f64) as i32, from_y + (dy * i as f64) as i32)
                    );
                    std::thread::sleep(std::time::Duration::from_secs_f64(duration / steps as f64));
                }
                win32_mouse_event(MOUSEEVENTF_LEFTUP, 0);
                tool_result_text(id, &format!("Dragged from ({},{}) to ({},{})", from_x, from_y, to_x, to_y), false)
            }
            #[cfg(not(target_os = "windows"))]
            { tool_result_text(id, "drag not available on this platform", true) }
        }
        "scroll" => {
            let direction = args.get("direction").and_then(|v| v.as_str()).unwrap_or("down");
            let amount = args.get("amount").and_then(|v| v.as_i64()).unwrap_or(3) as i32;
            let x = args.get("x").and_then(|v| v.as_i64());
            let y = args.get("y").and_then(|v| v.as_i64());
            #[cfg(target_os = "windows")]
            {
                if let (Some(px), Some(py)) = (x, y) {
                    let _ = uiautomation::inputs::Mouse::set_cursor_pos(
                        &uiautomation::types::Point::new(px as i32, py as i32)
                    );
                }
                let wheel_delta = if direction == "up" { amount * 120 } else { -amount * 120 };
                win32_mouse_event(MOUSEEVENTF_WHEEL, wheel_delta);
                tool_result_text(id, &format!("Scrolled {} by {}", direction, amount), false)
            }
            #[cfg(not(target_os = "windows"))]
            { tool_result_text(id, "scroll not available on this platform", true) }
        }
        "move_mouse" => {
            let x = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let y = args.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            #[cfg(target_os = "windows")]
            {
                match uiautomation::inputs::Mouse::set_cursor_pos(&uiautomation::types::Point::new(x, y)) {
                    Ok(_) => tool_result_text(id, &format!("Mouse moved to ({}, {})", x, y), false),
                    Err(e) => tool_result_text(id, &format!("Failed to move mouse: {}", e), true),
                }
            }
            #[cfg(not(target_os = "windows"))]
            { tool_result_text(id, "move_mouse not available on this platform", true) }
        }
        "wait" => {
            let ms = args.get("milliseconds").and_then(|v| v.as_u64()).unwrap_or(500);
            std::thread::sleep(std::time::Duration::from_millis(ms));
            tool_result_text(id, &format!("Waited {}ms", ms), false)
        }
        "get_cursor_position" => {
            #[cfg(target_os = "windows")]
            {
                match uiautomation::inputs::Mouse::get_cursor_pos() {
                    Ok(pos) => tool_result_text(id, &format!("{{\"x\": {}, \"y\": {}}}", pos.get_x(), pos.get_y()), false),
                    Err(e) => tool_result_text(id, &format!("Failed: {}", e), true),
                }
            }
            #[cfg(not(target_os = "windows"))]
            { tool_result_text(id, "Not available on this platform", true) }
        }
        "get_screen_size" => {
            #[cfg(target_os = "windows")]
            {
                match uiautomation::inputs::get_screen_size() {
                    Ok((w, h)) => tool_result_text(id, &format!("{{\"width\": {}, \"height\": {}}}", w, h), false),
                    Err(e) => tool_result_text(id, &format!("Failed: {}", e), true),
                }
            }
            #[cfg(not(target_os = "windows"))]
            { tool_result_text(id, "Not available on this platform", true) }
        }
        // Folder tools
        "get_common_folders" => {
            let folders = kage::commands::folder_tools::get_common_folders();
            let text = serde_json::to_string_pretty(&folders).unwrap_or_default();
            tool_result_text(id, &text, false)
        }
        "pick_folder" => {
            // The previous Windows path hand-rolled IFileOpenDialog and did a
            // manual CoTaskMemFree on a PWSTR that the windows crate already
            // frees on drop — a guaranteed double-free that produced random
            // crashes the second time the dialog was invoked. rfd uses the
            // same IFileOpenDialog under the hood and gets the lifecycle
            // right (CoInit, RAII for the COM interfaces, no manual frees).
            // We spawn a fresh thread so the COM apartment doesn't leak into
            // the stdio JSON-RPC loop.
            //
            // The old code passed GetForegroundWindow as the dialog parent
            // to enforce z-order over the floating window. rfd doesn't expose
            // a clean parent-by-HWND API in 0.15 without pulling in
            // raw-window-handle plumbing, and an unowned dialog already
            // steals focus on activation — if z-order issues surface in
            // practice, a parent wrapper can be added back as a follow-up.
            let result = std::thread::spawn(|| rfd::FileDialog::new().pick_folder())
                .join()
                .unwrap_or(None);
            match result {
                Some(path) => {
                    let path_str = path.to_string_lossy().replace('\\', "\\\\");
                    tool_result_text(id, &format!("{{\"path\": \"{}\"}}", path_str), false)
                }
                None => tool_result_text(id, "{\"path\": null, \"message\": \"User cancelled the folder picker\"}", false),
            }
        }
        "scan_folder" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if path.is_empty() {
                return tool_result_text(id, "Missing required parameter: path", true);
            }
            let max_depth = args.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let compute_hashes = args.get("compute_hashes").and_then(|v| v.as_bool()).unwrap_or(true);
            let root = std::path::Path::new(path);
            if !root.is_dir() {
                return tool_result_text(id, &format!("Not a directory: {}", path), true);
            }
            let result = kage::commands::folder_tools::scan_directory(root, max_depth, compute_hashes);
            let text = serde_json::to_string_pretty(&result).unwrap_or_default();
            tool_result_text(id, &text, false)
        }
        "execute_folder_plan" => {
            let root_str = args.get("root").and_then(|v| v.as_str()).unwrap_or("");
            if root_str.is_empty() {
                return tool_result_text(id, "Missing required parameter: root", true);
            }
            let ops: Vec<kage::commands::folder_tools::FolderOperation> = match args.get("operations") {
                Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
                None => return tool_result_text(id, "Missing required parameter: operations", true),
            };
            if ops.is_empty() {
                return tool_result_text(id, "Operations array is empty", true);
            }
            let root = std::path::Path::new(root_str);
            if !root.is_dir() {
                return tool_result_text(id, &format!("Not a directory: {}", root_str), true);
            }
            let result = kage::commands::folder_tools::execute_plan(root, &ops);
            let text = serde_json::to_string_pretty(&result).unwrap_or_default();
            tool_result_text(id, &text, false)
        }
        _ => mcp_json_rpc::error(
            id,
            mcp_json_rpc::ErrorCode::MethodNotFound,
            &format!("Unknown tool: {}", tool_name),
        ),
    }
}

fn dispatch_element_action(
    id: &serde_json::Value,
    args: &serde_json::Value,
    action: impl Fn(&str) -> Result<String, String>,
) -> String {
    let eid = args.get("element_id").and_then(|v| v.as_str()).unwrap_or("");
    match action(eid) {
        Ok(msg) => tool_result_text(id, &msg, false),
        Err(e) => tool_result_text(id, &e, true),
    }
}

/// Convert "ctrl+shift+s" format to uiautomation "{Ctrl}{Shift}s" format.
fn convert_key_combo(keys: &str) -> String {
    let parts: Vec<&str> = keys.split('+').map(|s| s.trim()).collect();
    let mut result = String::new();
    for part in &parts {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => result.push_str("{Ctrl}"),
            "alt" => result.push_str("{Alt}"),
            "shift" => result.push_str("{Shift}"),
            "win" | "windows" | "meta" | "super" => result.push_str("{Win}"),
            "enter" | "return" => result.push_str("{Enter}"),
            "tab" => result.push_str("{Tab}"),
            "escape" | "esc" => result.push_str("{Esc}"),
            "backspace" | "back" => result.push_str("{Backspace}"),
            "delete" | "del" => result.push_str("{Delete}"),
            "space" => result.push_str("{Space}"),
            "up" => result.push_str("{Up}"),
            "down" => result.push_str("{Down}"),
            "left" => result.push_str("{Left}"),
            "right" => result.push_str("{Right}"),
            "home" => result.push_str("{Home}"),
            "end" => result.push_str("{End}"),
            "pageup" | "pgup" => result.push_str("{PageUp}"),
            "pagedown" | "pgdn" => result.push_str("{PageDown}"),
            "insert" | "ins" => result.push_str("{Insert}"),
            k if k.starts_with('f') && k[1..].parse::<u32>().is_ok() => {
                result.push_str(&format!("{{{}}}", part));
            }
            _ => result.push_str(part),
        }
    }
    result
}
