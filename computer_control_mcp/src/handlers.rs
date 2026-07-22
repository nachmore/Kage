use super::input_tools;
use kage_core::mcp_json_rpc;
use kage_core::mcp_json_rpc::tool_result_text;
use kage_core::os::accessibility;

pub(crate) fn handle_initialize(id: &serde_json::Value) -> String {
    mcp_json_rpc::success(
        id,
        serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "computer-control",
                "version": env!("CARGO_PKG_VERSION"),
            }
        }),
    )
}

#[cfg_attr(not(windows), allow(unused_variables))]
pub(crate) fn handle_tool_call(id: &serde_json::Value, params: &serde_json::Value) -> String {
    let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    log::info!("[tool_call] {} args={}", tool_name, args);

    if let Some(response) = input_tools::dispatch(id, tool_name, &args) {
        return response;
    }

    match tool_name {
        "get_ui_tree" => {
            let title = args.get("window_title").and_then(|v| v.as_str());
            let depth = args.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
            let invisible = args
                .get("include_invisible")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            match accessibility::get_ui_tree(title, depth, invisible) {
                Ok(elem) => {
                    let mut text = String::new();
                    if !elem.meta.is_empty() {
                        text.push_str(&elem.meta);
                        text.push('\n');
                    }
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
                automation_id: args
                    .get("automation_id")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                value: args.get("value").and_then(|v| v.as_str()).map(String::from),
                window_title: args
                    .get("window_title")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            };
            match accessibility::find_elements(&params) {
                Ok(elems) => {
                    let text: String = if elems.is_empty() {
                        "No matching elements found.".to_string()
                    } else {
                        elems
                            .iter()
                            .map(|e| e.to_text(0, 0))
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    tool_result_text(id, &text, false)
                }
                Err(e) => tool_result_text(id, &e, true),
            }
        }
        "get_focused_element" => match accessibility::get_focused_element() {
            Ok(Some(elem)) => tool_result_text(id, &elem.to_text(0, 0), false),
            Ok(None) => tool_result_text(id, "No focused element found.", false),
            Err(e) => tool_result_text(id, &e, true),
        },
        "list_windows" => {
            let filter = args.get("title_filter").and_then(|v| v.as_str());
            match accessibility::list_accessible_windows(filter) {
                Ok(wins) => {
                    let text = if wins.is_empty() {
                        "No visible windows found.".to_string()
                    } else {
                        wins.iter()
                            .map(|w| {
                                let b = w
                                    .bounds
                                    .map(|(x, y, ww, h)| format!(" ({}x{}@{},{})", ww, h, x, y))
                                    .unwrap_or_default();
                                format!(
                                    "[window] \"{}\" pid={} process={}{}",
                                    w.title, w.process_id, w.process_name, b
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    tool_result_text(id, &text, false)
                }
                Err(e) => tool_result_text(id, &e, true),
            }
        }
        "click_element" => dispatch_element_action(id, &args, accessibility::click_element),
        "focus_element" => dispatch_element_action(id, &args, accessibility::focus_element),
        "set_value" => {
            let eid = args
                .get("element_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
            let eid = args
                .get("element_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let dir = args
                .get("direction")
                .and_then(|v| v.as_str())
                .unwrap_or("down");
            let amt = args.get("amount").and_then(|v| v.as_f64()).unwrap_or(0.2);
            match accessibility::scroll_element(eid, dir, amt) {
                Ok(msg) => tool_result_text(id, &msg, false),
                Err(e) => tool_result_text(id, &e, true),
            }
        }
        "get_element_text" => dispatch_element_action(id, &args, accessibility::get_element_text),
        "get_element_children" => {
            let eid = args
                .get("element_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
            log::info!(
                "[launch_and_get_tree] Launching: '{}' (wait={}ms, depth={})",
                app,
                wait,
                depth
            );
            let launch_result = kage_core::os::launcher::shell_launch(app);
            match launch_result {
                Ok(_) => {
                    std::thread::sleep(std::time::Duration::from_millis(wait));
                    // Try to find the window and get its tree
                    match accessibility::get_ui_tree(None, depth, false) {
                        Ok(elem) => {
                            let mut text = format!("Launched '{}'. UI tree:\n", app);
                            if !elem.meta.is_empty() {
                                text.push_str(&elem.meta);
                                text.push('\n');
                            }
                            text.push_str(&elem.to_text(0, depth));
                            tool_result_text(id, &text, false)
                        }
                        Err(e) => tool_result_text(
                            id,
                            &format!("Launched '{}' but failed to get tree: {}", app, e),
                            true,
                        ),
                    }
                }
                Err(e) => tool_result_text(id, &format!("Failed to launch '{}': {}", app, e), true),
            }
        }
        "click_and_get_tree" => {
            let eid = args
                .get("element_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
                Err(e) => {
                    tool_result_text(id, &format!("Click succeeded but tree failed: {}", e), true)
                }
            }
        }
        "click_and_read_result" => {
            let eid = args
                .get("element_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let result_name = args
                .get("result_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let title = args.get("window_title").and_then(|v| v.as_str());
            let click_msg = accessibility::click_element(eid);
            std::thread::sleep(std::time::Duration::from_millis(300));
            let find_params = accessibility::FindElementsParams {
                name: Some(result_name.to_string()),
                role: None,
                automation_id: None,
                value: None,
                window_title: title.map(String::from),
            };
            match accessibility::find_elements(&find_params) {
                Ok(elems) => {
                    let click_str = click_msg.unwrap_or_else(|e| format!("Click failed: {}", e));
                    let result_text = elems
                        .first()
                        .map(|e| {
                            if !e.value.is_empty() {
                                e.value.clone()
                            } else {
                                e.name.clone()
                            }
                        })
                        .unwrap_or_else(|| format!("Element '{}' not found", result_name));
                    tool_result_text(
                        id,
                        &format!("{}\nResult: {}", click_str, result_text),
                        false,
                    )
                }
                Err(e) => {
                    tool_result_text(id, &format!("Click succeeded but find failed: {}", e), true)
                }
            }
        }
        "type_and_get_tree" => {
            let eid = args
                .get("element_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
                Err(e) => {
                    tool_result_text(id, &format!("Type succeeded but tree failed: {}", e), true)
                }
            }
        }
        "get_app_steering" => {
            let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("");
            let details = args.get("details").and_then(|v| v.as_str()).unwrap_or("");
            let combined = format!("{} {}", task, details).to_lowercase();
            // Embedded app steering files
            const STEERING: &[(&str, &str)] = &[
                ("calculator", include_str!("app_steering/calculator.md")),
                (
                    "microsoft_office",
                    include_str!("app_steering/microsoft_office.md"),
                ),
                ("notepad", include_str!("app_steering/notepad.md")),
            ];
            const APP_PATTERNS: &[(&str, &[&str])] = &[
                (
                    "microsoft_office",
                    &[
                        "word",
                        "winword",
                        "excel",
                        "powerpnt",
                        "powerpoint",
                        "outlook",
                        "onenote",
                        "access",
                        "publisher",
                        "visio",
                    ],
                ),
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
            match kage_core::os::launcher::shell_launch(name) {
                Ok(_) => tool_result_text(id, &format!("Launched '{}'", name), false),
                Err(e) => {
                    log::info!("[launch_app] Failed: {}", e);
                    tool_result_text(id, &format!("Failed to launch '{}': {}", name, e), true)
                }
            }
        }
        "list_installed_apps" => {
            let filter = args
                .get("filter")
                .and_then(|v| v.as_str())
                .map(|s| s.to_lowercase());
            match kage_core::os::launcher::scan_applications() {
                Ok(apps) => {
                    let list: Vec<serde_json::Value> = apps
                        .into_iter()
                        .filter(|app| match filter.as_deref() {
                            Some(f) => app.name.to_lowercase().contains(f),
                            None => true,
                        })
                        .map(|app| {
                            serde_json::json!({
                                "name": app.name,
                                "path": app.path.to_string_lossy(),
                            })
                        })
                        .collect();
                    log::info!("[list_installed_apps] returning {} apps", list.len());
                    let text = serde_json::to_string_pretty(&serde_json::json!({
                        "count": list.len(),
                        "apps": list,
                    }))
                    .unwrap_or_default();
                    tool_result_text(id, &text, false)
                }
                Err(e) => {
                    tool_result_text(id, &format!("Failed to scan installed apps: {}", e), true)
                }
            }
        }
        "list_all_windows" => {
            let filter = args.get("title_filter").and_then(|v| v.as_str());
            match accessibility::list_accessible_windows(filter) {
                Ok(wins) => {
                    let text = if wins.is_empty() {
                        "No windows found.".to_string()
                    } else {
                        wins.iter()
                            .map(|w| {
                                let b = w
                                    .bounds
                                    .map(|(x, y, ww, h)| format!(" ({}x{}@{},{})", ww, h, x, y))
                                    .unwrap_or_default();
                                format!(
                                    "[window] \"{}\" pid={} process={}{}",
                                    w.title, w.process_id, w.process_name, b
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    tool_result_text(id, &text, false)
                }
                Err(e) => tool_result_text(id, &e, true),
            }
        }
        "get_common_folders" => {
            let folders = kage_core::folder_tools::get_common_folders();
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
                None => tool_result_text(
                    id,
                    "{\"path\": null, \"message\": \"User cancelled the folder picker\"}",
                    false,
                ),
            }
        }
        "scan_folder" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if path.is_empty() {
                return tool_result_text(id, "Missing required parameter: path", true);
            }
            let max_depth = args.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let compute_hashes = args
                .get("compute_hashes")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let root = std::path::Path::new(path);
            if !root.is_dir() {
                return tool_result_text(id, &format!("Not a directory: {}", path), true);
            }
            let result = kage_core::folder_tools::scan_directory(root, max_depth, compute_hashes);
            let text = serde_json::to_string_pretty(&result).unwrap_or_default();
            tool_result_text(id, &text, false)
        }
        "run_script" => {
            use kage_core::computer_control::script_runner::{run_script, RunOutcome, ScriptLang};
            let lang_str = args.get("lang").and_then(|v| v.as_str()).unwrap_or("");
            let Some(lang) = ScriptLang::parse(lang_str) else {
                return tool_result_text(
                    id,
                    &format!(
                        "Unsupported lang '{}'. Use 'powershell' or 'bash'.",
                        lang_str
                    ),
                    true,
                );
            };
            let script = args.get("script").and_then(|v| v.as_str()).unwrap_or("");
            if script.trim().is_empty() {
                return tool_result_text(id, "Missing required parameter: script", true);
            }
            let timeout_ms = args.get("timeout_ms").and_then(|v| v.as_u64());
            match run_script(lang, script, timeout_ms) {
                RunOutcome::Ran(result) => {
                    let text = serde_json::to_string_pretty(&result).unwrap_or_default();
                    // A non-zero exit or timeout is a tool error the model
                    // should react to, not a transport failure.
                    let is_err = result.timed_out || result.exit_code.unwrap_or(1) != 0;
                    tool_result_text(id, &text, is_err)
                }
                RunOutcome::Unsupported(msg) => tool_result_text(id, &msg, true),
            }
        }
        "execute_folder_plan" => {
            let root_str = args.get("root").and_then(|v| v.as_str()).unwrap_or("");
            if root_str.is_empty() {
                return tool_result_text(id, "Missing required parameter: root", true);
            }
            let ops: Vec<kage_core::folder_tools::FolderOperation> = match args.get("operations") {
                Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
                None => {
                    return tool_result_text(id, "Missing required parameter: operations", true)
                }
            };
            if ops.is_empty() {
                return tool_result_text(id, "Operations array is empty", true);
            }
            let root = std::path::Path::new(root_str);
            if !root.is_dir() {
                return tool_result_text(id, &format!("Not a directory: {}", root_str), true);
            }
            let result = kage_core::folder_tools::execute_plan(root, &ops);
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
    let eid = args
        .get("element_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match action(eid) {
        Ok(msg) => tool_result_text(id, &msg, false),
        Err(e) => tool_result_text(id, &e, true),
    }
}
