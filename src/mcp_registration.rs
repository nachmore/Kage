//! Auto-registration of the computer-control MCP server in mcp.json.
//!
//! Uses a "ka-" prefix on the server name to avoid clashing with user-defined
//! MCP servers. Only touches its own entry — never overwrites other entries.

use log::{info, warn};
use std::path::PathBuf;

/// Key used in mcp.json for our managed MCP server.
const MCP_SERVER_KEY: &str = "kage-computer-control";

/// Get the path to the computer-control-mcp binary (sibling of the current exe).
pub fn get_mcp_binary_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(windows) { "kage-computer-control-mcp.exe" } else { "kage-computer-control-mcp" };

    // Check next to the main exe (dev builds, post-install)
    let sibling = dir.join(name);
    if sibling.exists() { return Some(sibling); }

    // Check in resources/ subdirectory (Tauri bundle before NSIS hook runs)
    let resource = dir.join("resources").join(name);
    if resource.exists() { return Some(resource); }

    None
}

/// Ensure the computer-control MCP server is registered in the user's mcp.json.
///
/// - Reads existing mcp.json and preserves all other entries untouched
/// - Only adds/updates the "kage-computer-control" entry
/// - Creates the file if it doesn't exist
/// - Updates the command path if the install location changed
pub fn ensure_registered() {
    let Some(mcp_path) = get_mcp_binary_path() else {
        warn!("kage-computer-control-mcp binary not found next to main exe");
        return;
    };

    let config_dir = match crate::agent_presets::default_mcp_json_path() {
        Some(p) => match p.parent() {
            Some(dir) => dir.to_path_buf(),
            None => { warn!("Cannot determine settings dir for mcp.json"); return; }
        },
        None => { warn!("Cannot determine home directory for mcp.json"); return; }
    };
    if let Err(e) = std::fs::create_dir_all(&config_dir) {
        warn!("Failed to create settings dir: {}", e);
        return;
    }
    let mcp_json_path = config_dir.join("mcp.json");
    let path_str = mcp_path.to_string_lossy().to_string();

    // Read existing config — preserve everything
    let mut config: serde_json::Value = if mcp_json_path.exists() {
        match std::fs::read_to_string(&mcp_json_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or(serde_json::json!({})),
            Err(e) => {
                warn!("Failed to read mcp.json: {}", e);
                serde_json::json!({})
            }
        }
    } else {
        serde_json::json!({})
    };

    // Ensure mcpServers object exists
    if config.get("mcpServers").is_none() {
        config["mcpServers"] = serde_json::json!({});
    }

    // Defensive: if mcpServers exists but isn't an object (e.g. a user
    // hand-edited mcp.json into invalid shape), replace it rather than panic.
    let servers = match config["mcpServers"].as_object_mut() {
        Some(obj) => obj,
        None => {
            warn!("mcpServers in mcp.json is not a JSON object — overwriting with empty object");
            config["mcpServers"] = serde_json::json!({});
            match config["mcpServers"].as_object_mut() {
                Some(obj) => obj,
                None => return, // Truly can't happen, but we refuse to panic
            }
        }
    };

    // Check if our entry already exists and is up to date
    if let Some(existing) = servers.get(MCP_SERVER_KEY) {
        let existing_cmd = existing.get("command").and_then(|c| c.as_str()).unwrap_or("");
        if existing_cmd == path_str {
            return; // Already registered with correct path
        }
        // Path changed (e.g. app was reinstalled to a different location) — update it
        info!("Updating {} MCP server path to: {}", MCP_SERVER_KEY, path_str);
    } else {
        info!("Registering {} MCP server at: {}", MCP_SERVER_KEY, path_str);
    }

    // Insert/update only our entry — all other entries are untouched
    servers.insert(MCP_SERVER_KEY.to_string(), serde_json::json!({
        "command": path_str,
        "args": [],
        "disabled": false,
        "autoApprove": []
    }));

    // Write back preserving all other content
    match serde_json::to_string_pretty(&config) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&mcp_json_path, json) {
                warn!("Failed to write mcp.json: {}", e);
            }
        }
        Err(e) => warn!("Failed to serialize mcp.json: {}", e),
    }
}

/// Check if the computer-control MCP server is currently registered.
pub fn is_registered() -> bool {
    let mcp_json_path = match crate::agent_presets::default_mcp_json_path() {
        Some(p) => p,
        None => return false,
    };
    if !mcp_json_path.exists() { return false; }

    let config: serde_json::Value = std::fs::read_to_string(&mcp_json_path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or(serde_json::json!({}));

    config.get("mcpServers")
        .and_then(|s| s.get(MCP_SERVER_KEY))
        .is_some()
}

/// Remove the computer-control MCP server from mcp.json.
/// Preserves all other entries.
pub fn unregister() {
    let mcp_json_path = match crate::agent_presets::default_mcp_json_path() {
        Some(p) => p,
        None => return,
    };
    if !mcp_json_path.exists() { return; }

    let mut config: serde_json::Value = match std::fs::read_to_string(&mcp_json_path) {
        Ok(c) => serde_json::from_str(&c).unwrap_or(serde_json::json!({})),
        Err(_) => return,
    };

    if let Some(servers) = config.get_mut("mcpServers").and_then(|s| s.as_object_mut()) {
        if servers.remove(MCP_SERVER_KEY).is_some() {
            info!("Unregistered {} MCP server", MCP_SERVER_KEY);
            if let Ok(json) = serde_json::to_string_pretty(&config) {
                if let Err(e) = std::fs::write(&mcp_json_path, json) {
                    warn!("Failed to write mcp.json: {}", e);
                }
            }
        }
    }
}

/// Get the default mcp.json path.
pub fn default_mcp_json_path() -> Option<PathBuf> {
    crate::agent_presets::default_mcp_json_path()
}

/// Read the full mcp.json content as a JSON value.
pub fn read_mcp_json(path: Option<&str>) -> serde_json::Value {
    let mcp_path = path
        .map(PathBuf::from)
        .or_else(default_mcp_json_path)
        .unwrap_or_default();
    if !mcp_path.exists() {
        return serde_json::json!({ "mcpServers": {} });
    }
    std::fs::read_to_string(&mcp_path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or(serde_json::json!({ "mcpServers": {} }))
}

/// Write the full mcp.json content.
pub fn write_mcp_json(path: Option<&str>, config: &serde_json::Value) -> Result<(), String> {
    let mcp_path = path
        .map(PathBuf::from)
        .or_else(default_mcp_json_path)
        .ok_or("Cannot determine mcp.json path")?;
    if let Some(parent) = mcp_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = serde_json::to_string_pretty(config).map_err(|e| format!("Serialize: {}", e))?;
    std::fs::write(&mcp_path, json).map_err(|e| format!("Write: {}", e))
}
