use kage_core::mcp_json_rpc;
use std::sync::LazyLock;

/// The tool list is static — only the echoed request `id` varies per
/// response. Agent backends re-issue tools/list on every reconnect /
/// session refresh, so build the ~20-tool schema tree once and splice
/// the id per request instead of re-allocating the whole tree.
static TOOLS: LazyLock<serde_json::Value> = LazyLock::new(build_tools);

pub(crate) fn handle_tools_list(id: &serde_json::Value) -> String {
    mcp_json_rpc::success(id, TOOLS.clone())
}

fn build_tools() -> serde_json::Value {
    serde_json::json!({ "tools": [
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
        // Kage self-knowledge
        tool_def("get_kage_changelog", "Get Kage's release notes (changelog) and current version. Use when the user asks what changed in the last Kage update, what's new in Kage, or which Kage version is installed. Reads a locally cached copy refreshed by the app — works offline.", serde_json::json!({
            "type": "object", "properties": {}
        })),
        // Fallback tools
        tool_def("launch_app", "Launch an application by name.", serde_json::json!({
            "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"]
        })),
        tool_def("list_installed_apps", "List applications installed on this system (name + launch path). Use this to discover what software is available — e.g. to answer \"which app can I use for video editing\" — instead of guessing app names or shelling out to query the OS. Pass this to launch_app by name.", serde_json::json!({
            "type": "object",
            "properties": {
                "filter": { "type": "string", "description": "Optional case-insensitive substring; only apps whose name contains it are returned. Omit to list everything." }
            }
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
        tool_def("run_script", "Run a PowerShell (Windows) or bash (macOS/Linux) script to query or control the OS. PREFER THIS over inlining a multi-line script into a single shell command — pass the script as-is in the `script` argument (no escaping needed; it's written to a temp file and executed). Returns exit_code, stdout, stderr. Use this for things like enumerating installed software, reading system info, or file operations that aren't covered by a dedicated tool.", serde_json::json!({
            "type": "object",
            "properties": {
                "lang": { "type": "string", "enum": ["powershell", "bash"], "description": "Script language. Use 'powershell' on Windows, 'bash' on macOS/Linux." },
                "script": { "type": "string", "description": "The full script source, verbatim. Multi-line is fine; no quoting/escaping required." },
                "timeout_ms": { "type": "integer", "default": 30000, "description": "Kill the script after this many milliseconds (max 600000)." }
            },
            "required": ["lang", "script"]
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
    ]})
}

fn tool_def(name: &str, description: &str, input_schema: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "name": name, "description": description, "inputSchema": input_schema })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consecutive_tools_list_responses_are_identical_and_echo_id() {
        let a = handle_tools_list(&serde_json::json!(1));
        let b = handle_tools_list(&serde_json::json!(1));
        assert_eq!(a, b);

        let c = handle_tools_list(&serde_json::json!("other-id"));
        let parsed: serde_json::Value = serde_json::from_str(&c).unwrap();
        assert_eq!(parsed["id"], serde_json::json!("other-id"));
        // Same tool list regardless of id.
        let parsed_a: serde_json::Value = serde_json::from_str(&a).unwrap();
        assert_eq!(parsed["result"], parsed_a["result"]);
        assert!(parsed["result"]["tools"].as_array().unwrap().len() > 10);
    }
}
