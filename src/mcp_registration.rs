//! Auto-registration of the computer-control MCP server in mcp.json.
//!
//! Uses a "ka-" prefix on the server name to avoid clashing with user-defined
//! MCP servers. Only touches its own entry — never overwrites other entries.

use log::{info, warn};
use std::path::PathBuf;

/// Key used in mcp.json for our managed MCP server.
const MCP_SERVER_KEY: &str = "kage-computer-control";

/// Outcome of `upsert_server_entry` — lets the caller skip a write when nothing changed.
#[derive(Debug, PartialEq, Eq)]
pub enum UpsertOutcome {
    /// Entry already present with the same `command` value — no mutation performed.
    Unchanged,
    /// Entry was inserted (didn't exist before).
    Inserted,
    /// Entry existed but `command` differed; updated to the new path.
    PathUpdated,
}

/// Insert or update the managed server entry in an mcp.json document, preserving
/// every other entry. Returns the outcome so the caller can skip the disk write
/// when nothing changed.
///
/// Defensive against malformed input: if `mcpServers` is missing or not an
/// object, it's replaced with a fresh object and the entry inserted there.
pub fn upsert_server_entry(
    config: &mut serde_json::Value,
    server_key: &str,
    command_path: &str,
) -> UpsertOutcome {
    if config
        .get("mcpServers")
        .map(|v| !v.is_object())
        .unwrap_or(true)
    {
        config["mcpServers"] = serde_json::json!({});
    }
    let servers = config["mcpServers"]
        .as_object_mut()
        .expect("mcpServers just normalized to object");

    let outcome = match servers.get(server_key) {
        Some(existing) => {
            let existing_cmd = existing
                .get("command")
                .and_then(|c| c.as_str())
                .unwrap_or("");
            if existing_cmd == command_path {
                return UpsertOutcome::Unchanged;
            }
            UpsertOutcome::PathUpdated
        }
        None => UpsertOutcome::Inserted,
    };

    servers.insert(
        server_key.to_string(),
        serde_json::json!({
            "command": command_path,
            "args": [],
            "disabled": false,
            "autoApprove": []
        }),
    );
    outcome
}

/// Remove the managed server entry from an mcp.json document. Returns true if
/// an entry was present and removed (caller should write back), false otherwise.
pub fn remove_server_entry(config: &mut serde_json::Value, server_key: &str) -> bool {
    let Some(servers) = config.get_mut("mcpServers").and_then(|s| s.as_object_mut()) else {
        return false;
    };
    servers.remove(server_key).is_some()
}

/// Whether a config document carries the managed server entry.
pub fn has_server_entry(config: &serde_json::Value, server_key: &str) -> bool {
    config
        .get("mcpServers")
        .and_then(|s| s.get(server_key))
        .is_some()
}

/// Get the path to the computer-control-mcp binary (sibling of the current exe).
pub fn get_mcp_binary_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(windows) {
        "kage-computer-control-mcp.exe"
    } else {
        "kage-computer-control-mcp"
    };

    // Check next to the main exe (dev builds, post-install)
    let sibling = dir.join(name);
    if sibling.exists() {
        return Some(sibling);
    }

    // Check in resources/ subdirectory (Tauri bundle before NSIS hook runs)
    let resource = dir.join("resources").join(name);
    if resource.exists() {
        return Some(resource);
    }

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
            None => {
                warn!("Cannot determine settings dir for mcp.json");
                return;
            }
        },
        None => {
            warn!("Cannot determine home directory for mcp.json");
            return;
        }
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

    match upsert_server_entry(&mut config, MCP_SERVER_KEY, &path_str) {
        UpsertOutcome::Unchanged => return,
        UpsertOutcome::Inserted => {
            info!("Registering {} MCP server at: {}", MCP_SERVER_KEY, path_str);
        }
        UpsertOutcome::PathUpdated => {
            info!(
                "Updating {} MCP server path to: {}",
                MCP_SERVER_KEY, path_str
            );
        }
    }

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
    if !mcp_json_path.exists() {
        return false;
    }

    let config: serde_json::Value = std::fs::read_to_string(&mcp_json_path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or(serde_json::json!({}));

    has_server_entry(&config, MCP_SERVER_KEY)
}

/// Remove the computer-control MCP server from mcp.json.
/// Preserves all other entries.
pub fn unregister() {
    let mcp_json_path = match crate::agent_presets::default_mcp_json_path() {
        Some(p) => p,
        None => return,
    };
    if !mcp_json_path.exists() {
        return;
    }

    let mut config: serde_json::Value = match std::fs::read_to_string(&mcp_json_path) {
        Ok(c) => serde_json::from_str(&c).unwrap_or(serde_json::json!({})),
        Err(_) => return,
    };

    if remove_server_entry(&mut config, MCP_SERVER_KEY) {
        info!("Unregistered {} MCP server", MCP_SERVER_KEY);
        if let Ok(json) = serde_json::to_string_pretty(&config) {
            if let Err(e) = std::fs::write(&mcp_json_path, json) {
                warn!("Failed to write mcp.json: {}", e);
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn upsert_inserts_into_empty_config() {
        let mut cfg = json!({});
        let outcome = upsert_server_entry(&mut cfg, "kage-cc", "/usr/bin/kage-cc");
        assert_eq!(outcome, UpsertOutcome::Inserted);
        assert_eq!(cfg["mcpServers"]["kage-cc"]["command"], "/usr/bin/kage-cc");
        assert_eq!(cfg["mcpServers"]["kage-cc"]["disabled"], false);
        assert_eq!(cfg["mcpServers"]["kage-cc"]["args"], json!([]));
        assert_eq!(cfg["mcpServers"]["kage-cc"]["autoApprove"], json!([]));
    }

    #[test]
    fn upsert_with_same_path_is_unchanged() {
        let mut cfg = json!({
            "mcpServers": {
                "kage-cc": { "command": "/usr/bin/kage-cc", "args": [], "disabled": false, "autoApprove": [] }
            }
        });
        let snapshot = cfg.clone();
        let outcome = upsert_server_entry(&mut cfg, "kage-cc", "/usr/bin/kage-cc");
        assert_eq!(outcome, UpsertOutcome::Unchanged);
        assert_eq!(cfg, snapshot, "no mutation when path matches");
    }

    #[test]
    fn upsert_updates_path_when_command_differs() {
        let mut cfg = json!({
            "mcpServers": {
                "kage-cc": { "command": "/old/kage-cc", "args": [], "disabled": false, "autoApprove": [] }
            }
        });
        let outcome = upsert_server_entry(&mut cfg, "kage-cc", "/new/kage-cc");
        assert_eq!(outcome, UpsertOutcome::PathUpdated);
        assert_eq!(cfg["mcpServers"]["kage-cc"]["command"], "/new/kage-cc");
    }

    #[test]
    fn upsert_preserves_unrelated_entries() {
        // The whole point of this module: never clobber user-defined servers.
        let mut cfg = json!({
            "mcpServers": {
                "user-server": { "command": "/usr/bin/user-thing", "args": ["--flag"], "disabled": true },
                "another": { "command": "/elsewhere", "args": [] }
            },
            "someOtherKey": { "preserve": "me" }
        });
        upsert_server_entry(&mut cfg, "kage-cc", "/usr/bin/kage-cc");
        assert_eq!(
            cfg["mcpServers"]["user-server"]["command"],
            "/usr/bin/user-thing"
        );
        assert_eq!(cfg["mcpServers"]["user-server"]["args"], json!(["--flag"]));
        assert_eq!(cfg["mcpServers"]["user-server"]["disabled"], true);
        assert_eq!(cfg["mcpServers"]["another"]["command"], "/elsewhere");
        assert_eq!(cfg["someOtherKey"]["preserve"], "me");
        assert_eq!(cfg["mcpServers"]["kage-cc"]["command"], "/usr/bin/kage-cc");
    }

    #[test]
    fn upsert_normalizes_non_object_mcpservers() {
        // User hand-edits mcp.json to "mcpServers": null or "" — we must
        // not panic; replace with empty object and insert.
        for bogus in [json!(null), json!("oops"), json!([])] {
            let mut cfg = json!({ "mcpServers": bogus });
            let outcome = upsert_server_entry(&mut cfg, "kage-cc", "/p");
            assert_eq!(outcome, UpsertOutcome::Inserted);
            assert!(cfg["mcpServers"].is_object());
            assert_eq!(cfg["mcpServers"]["kage-cc"]["command"], "/p");
        }
    }

    #[test]
    fn remove_returns_false_when_no_entry() {
        let mut cfg = json!({ "mcpServers": { "other": { "command": "/x" } } });
        let snapshot = cfg.clone();
        assert!(!remove_server_entry(&mut cfg, "kage-cc"));
        assert_eq!(cfg, snapshot, "no mutation when target absent");
    }

    #[test]
    fn remove_preserves_other_entries() {
        let mut cfg = json!({
            "mcpServers": {
                "kage-cc": { "command": "/p" },
                "other": { "command": "/x" }
            }
        });
        assert!(remove_server_entry(&mut cfg, "kage-cc"));
        assert!(cfg["mcpServers"].get("kage-cc").is_none());
        assert_eq!(cfg["mcpServers"]["other"]["command"], "/x");
    }

    #[test]
    fn remove_handles_missing_or_non_object_mcpservers() {
        let mut cfg = json!({});
        assert!(!remove_server_entry(&mut cfg, "kage-cc"));

        let mut cfg = json!({ "mcpServers": "garbage" });
        assert!(!remove_server_entry(&mut cfg, "kage-cc"));
    }

    #[test]
    fn has_server_entry_detects_presence() {
        let cfg = json!({ "mcpServers": { "kage-cc": { "command": "/p" } } });
        assert!(has_server_entry(&cfg, "kage-cc"));
        assert!(!has_server_entry(&cfg, "kage-other"));

        let empty = json!({});
        assert!(!has_server_entry(&empty, "kage-cc"));
    }

    #[test]
    fn upsert_then_remove_round_trip() {
        let mut cfg = json!({});
        upsert_server_entry(&mut cfg, "kage-cc", "/p");
        assert!(has_server_entry(&cfg, "kage-cc"));
        assert!(remove_server_entry(&mut cfg, "kage-cc"));
        assert!(!has_server_entry(&cfg, "kage-cc"));
        // The mcpServers object itself is left in place — empty but valid.
        assert!(cfg["mcpServers"].is_object());
        assert_eq!(cfg["mcpServers"].as_object().unwrap().len(), 0);
    }
}
