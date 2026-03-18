//! Computer Control MCP Server — standalone binary.
//!
//! Speaks MCP (JSON-RPC over stdio) and provides accessibility-based
//! desktop automation tools. Spawned by kiro-cli as an MCP server.

use std::io::{self, BufRead, Write};

use kiro_assistant::os::accessibility;

// ---------------------------------------------------------------------------
// Raw Win32 FFI for mouse events (avoids windows crate version conflicts)
// ---------------------------------------------------------------------------
#[cfg(target_os = "windows")]
const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
#[cfg(target_os = "windows")]
const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
#[cfg(target_os = "windows")]
const MOUSEEVENTF_WHEEL: u32 = 0x0800;

#[cfg(target_os = "windows")]
#[repr(C)]
struct MouseInput {
    dx: i32, dy: i32, mouse_data: u32, flags: u32, time: u32, extra: usize,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct RawInput {
    input_type: u32, // 0 = INPUT_MOUSE
    mi: MouseInput,
}

#[cfg(target_os = "windows")]
extern "system" {
    fn SendInput(count: u32, inputs: *const RawInput, size: i32) -> u32;
}

#[cfg(target_os = "windows")]
fn win32_mouse_event(flags: u32, data: i32) {
    let input = RawInput {
        input_type: 0, // INPUT_MOUSE
        mi: MouseInput { dx: 0, dy: 0, mouse_data: data as u32, flags, time: 0, extra: 0 },
    };
    unsafe { SendInput(1, &input, std::mem::size_of::<RawInput>() as i32); }
}

fn main() {
    // Log to file only — stdout/stderr are reserved for JSON-RPC
    let log_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".kiro")
        .join("logs");
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("Failed to create log dir {:?}: {}", log_dir, e);
    }
    let log_file = log_dir.join("computer-control-mcp.log");
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

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() { continue; }

        let request: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("Invalid JSON: {}", e);
                continue;
            }
        };

        let id = request.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(serde_json::json!({}));

        let response = match method {
            "initialize" => handle_initialize(&id),
            "tools/list" => handle_tools_list(&id),
            "tools/call" => handle_tool_call(&id, &params),
            "notifications/initialized" | "ping" => {
                // Notifications — no response needed (but ping gets a pong)
                if method == "ping" {
                    json_rpc_result(&id, serde_json::json!({}))
                } else {
                    continue;
                }
            }
            _ => json_rpc_error(&id, -32601, &format!("Method not found: {}", method)),
        };

        let mut out = stdout.lock();
        let _ = writeln!(out, "{}", response);
        let _ = out.flush();
    }

    log::info!("Computer Control MCP server exiting");
}

// ---------------------------------------------------------------------------
// JSON-RPC helpers
// ---------------------------------------------------------------------------
fn json_rpc_result(id: &serde_json::Value, result: serde_json::Value) -> String {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn json_rpc_error(id: &serde_json::Value, code: i32, message: &str) -> String {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

fn tool_result_text(id: &serde_json::Value, text: &str, is_error: bool) -> String {
    json_rpc_result(id, serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    }))
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
                    let text = if elems.is_empty() {
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
        "click_element" => dispatch_element_action(id, &args, |eid| accessibility::click_element(eid)),
        "set_value" => {
            let eid = args.get("element_id").and_then(|v| v.as_str()).unwrap_or("");
            let val = args.get("value").and_then(|v| v.as_str()).unwrap_or("");
            match accessibility::set_element_value(eid, val) {
                Ok(msg) => tool_result_text(id, &msg, false),
                Err(e) => tool_result_text(id, &e, true),
            }
        }
        "toggle_element" => dispatch_element_action(id, &args, |eid| accessibility::toggle_element(eid)),
        "select_element" => dispatch_element_action(id, &args, |eid| accessibility::select_element(eid)),
        "expand_element" => dispatch_element_action(id, &args, |eid| accessibility::expand_element(eid)),
        "collapse_element" => dispatch_element_action(id, &args, |eid| accessibility::collapse_element(eid)),
        "scroll_element" => {
            let eid = args.get("element_id").and_then(|v| v.as_str()).unwrap_or("");
            let dir = args.get("direction").and_then(|v| v.as_str()).unwrap_or("down");
            let amt = args.get("amount").and_then(|v| v.as_f64()).unwrap_or(0.2);
            match accessibility::scroll_element(eid, dir, amt) {
                Ok(msg) => tool_result_text(id, &msg, false),
                Err(e) => tool_result_text(id, &e, true),
            }
        }
        "get_element_text" => dispatch_element_action(id, &args, |eid| accessibility::get_element_text(eid)),
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
                    let text = if wins.is_empty() { "No windows found.".into() } else {
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
            #[cfg(target_os = "windows")]
            {
                let kb = uiautomation::inputs::Keyboard::new();
                match kb.send_text(text_val) {
                    Ok(_) => tool_result_text(id, &format!("Typed {} characters", text_val.len()), false),
                    Err(e) => tool_result_text(id, &format!("Failed to type: {}", e), true),
                }
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
        _ => json_rpc_error(id, -32601, &format!("Unknown tool: {}", tool_name)),
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
