//! Auto-registration of the computer-control MCP server in mcp.json.

use log::{info, warn};
use std::path::PathBuf;

/// Get the path to the computer-control-mcp binary (sibling of the current exe).
pub fn get_mcp_binary_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(windows) { "computer-control-mcp.exe" } else { "computer-control-mcp" };

    // Check next to the main exe (dev builds, post-install)
    let sibling = dir.join(name);
    if sibling.exists() { return Some(sibling); }

    // Check in resources/ subdirectory (Tauri bundle before NSIS hook runs)
    let resource = dir.join("resources").join(name);
    if resource.exists() { return Some(resource); }

    None
}

/// Ensure the computer-control MCP server is registered in the user's mcp.json.
/// Creates the file if it doesn't exist. Only adds the entry if not already present.
pub fn ensure_registered() {
    let Some(mcp_path) = get_mcp_binary_path() else {
        warn!("computer-control-mcp binary not found next to main exe");
        return;
    };

    let config_dir = match dirs::home_dir() {
        Some(h) => h.join(".kiro").join("settings"),
        None => { warn!("Cannot determine home directory for mcp.json"); return; }
    };
    let _ = std::fs::create_dir_all(&config_dir);
    let mcp_json_path = config_dir.join("mcp.json");

    // Read existing config or start fresh
    let mut config: serde_json::Value = if mcp_json_path.exists() {
        match std::fs::read_to_string(&mcp_json_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or(serde_json::json!({})),
            Err(_) => serde_json::json!({}),
        }
    } else {
        serde_json::json!({})
    };

    // Ensure mcpServers object exists
    if !config.get("mcpServers").is_some() {
        config["mcpServers"] = serde_json::json!({});
    }

    let servers = config["mcpServers"].as_object_mut().unwrap();

    // Check if already registered
    if servers.contains_key("computer-control") {
        // Update the command path in case the install location changed
        let path_str = mcp_path.to_string_lossy().to_string();
        if let Some(existing) = servers.get("computer-control") {
            let existing_cmd = existing.get("command").and_then(|c| c.as_str()).unwrap_or("");
            if existing_cmd == path_str {
                return; // Already up to date
            }
        }
        info!("Updating computer-control MCP server path to: {}", path_str);
        servers.insert("computer-control".to_string(), serde_json::json!({
            "command": path_str,
            "args": [],
            "disabled": false,
            "autoApprove": []
        }));
    } else {
        let path_str = mcp_path.to_string_lossy().to_string();
        info!("Registering computer-control MCP server at: {}", path_str);
        servers.insert("computer-control".to_string(), serde_json::json!({
            "command": path_str,
            "args": [],
            "disabled": false,
            "autoApprove": []
        }));
    }

    // Write back
    match serde_json::to_string_pretty(&config) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&mcp_json_path, json) {
                warn!("Failed to write mcp.json: {}", e);
            }
        }
        Err(e) => warn!("Failed to serialize mcp.json: {}", e),
    }
}
