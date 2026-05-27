//! ACP-compatible agent auto-detection and validation. Powers the
//! "+ New connection" dropdown and the connection-validation
//! affordances in Settings → Connections.

use crate::error::AppError;

#[derive(serde::Serialize, Clone)]
pub struct DetectedAgent {
    /// Display name from the preset.
    pub name: String,
    /// Stable preset id (e.g. "kiro", "claude-code", "codex").
    pub preset_id: String,
    /// Absolute path to the binary that was found.
    pub path: String,
    /// Full spawn command (path + ACP args) ready to drop into config.
    pub spawn_command: String,
    /// Output of `<binary> --version` when it succeeded.
    pub version: Option<String>,
}

/// Static metadata for a preset, surfaced to the UI so the settings page
/// can render install instructions, auth hints, etc. without duplicating
/// the registry in JS.
#[derive(serde::Serialize, Clone)]
pub struct AgentPresetInfo {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub install_url: String,
    pub requires_auth: bool,
    pub auth_hint: Option<String>,
}

/// Connection-validation result returned by [`validate_agent_connection`].
/// `ok=false` and `issues` populated means we found problems the user
/// should see in the UI.
#[derive(serde::Serialize, Clone, Default)]
pub struct ConnectionIssues {
    /// The binary the user pointed at (resolved against PATH where
    /// applicable). `None` for remote connections.
    pub resolved_path: Option<String>,
    /// True when no issues were found.
    pub ok: bool,
    /// Issue codes the UI maps to friendly copy. Examples: "empty",
    /// "binary-not-found", "not-executable", "host-empty", "port-invalid".
    pub issues: Vec<String>,
}

/// List the known agent presets the UI can render in a "+ New
/// connection" dropdown.
#[tauri::command]
pub async fn list_agent_presets() -> Result<Vec<AgentPresetInfo>, AppError> {
    use crate::agent_presets::AgentKind;
    Ok(AgentKind::all()
        .iter()
        .map(|k| {
            let p = k.preset();
            AgentPresetInfo {
                id: p.id.to_string(),
                display_name: p.display_name.to_string(),
                description: p.description.to_string(),
                install_url: p.install_url.to_string(),
                requires_auth: p.requires_auth,
                auth_hint: p.auth_hint.map(|s| s.to_string()),
            }
        })
        .collect())
}

/// Validate a saved connection. For local connections, parses the
/// spawn_command, resolves the binary against PATH, and checks that it
/// exists. For remote connections, sanity-checks host/port. Cheap (no
/// process spawn) so it's safe to call on every render of the settings
/// page.
#[tauri::command]
pub async fn validate_agent_connection(
    mode: crate::config::AcpMode,
) -> Result<ConnectionIssues, AppError> {
    let mut out = ConnectionIssues::default();
    match mode {
        crate::config::AcpMode::Local { spawn_command } => {
            let trimmed = spawn_command.trim();
            if trimmed.is_empty() {
                out.issues.push("empty".to_string());
                return Ok(out);
            }
            // First whitespace-separated token is the binary; this
            // matches the transport's own parsing, so what we validate
            // is what would be spawned.
            let first = trimmed.split_whitespace().next().unwrap_or("");
            let resolved = resolve_binary_path(first);
            out.resolved_path = resolved.clone();
            if resolved.is_none() {
                out.issues.push("binary-not-found".to_string());
            }
            out.ok = out.issues.is_empty();
        }
        crate::config::AcpMode::Remote { host, port, .. } => {
            if host.trim().is_empty() {
                out.issues.push("host-empty".to_string());
            }
            if port == 0 {
                out.issues.push("port-invalid".to_string());
            }
            out.ok = out.issues.is_empty();
        }
    }
    Ok(out)
}

/// Resolve a binary token to an absolute path, mirroring how the
/// transport's `Command::new` resolves names. Absolute paths are
/// validated by `Path::exists`; bare names go through `where`/`which`.
fn resolve_binary_path(token: &str) -> Option<String> {
    let p = std::path::Path::new(token);
    if p.is_absolute() {
        return p.exists().then(|| token.to_string());
    }
    let cmd = if cfg!(windows) { "where" } else { "which" };
    let mut command = std::process::Command::new(cmd);
    command.arg(token);
    // CREATE_NO_WINDOW: GUI subsystem processes spawning console
    // children inherit no console — Windows allocates a fresh one for
    // the child unless we suppress it, which flashes a DOS window.
    crate::os::configure_no_window(&mut command);
    let out = command.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let first = stdout.lines().next()?.trim();
    if first.is_empty() {
        None
    } else {
        Some(first.to_string())
    }
}

/// Search well-known locations for ACP-compatible agent binaries.
#[tauri::command]
pub async fn detect_agents() -> Result<Vec<DetectedAgent>, AppError> {
    Ok(tauri::async_runtime::spawn_blocking(detect_agents_sync)
        .await
        .map_err(|e| format!("Task error: {}", e))?)
}

pub(crate) fn detect_agents_sync() -> Vec<DetectedAgent> {
    use crate::agent_presets::detection_hints;

    let mut agents = Vec::new();
    let home = dirs::home_dir();

    for hint in detection_hints() {
        let preset = hint.kind.preset();
        for bin_name in hint.binary_names {
            let mut candidates: Vec<std::path::PathBuf> = Vec::new();

            // Windows-specific locations
            #[cfg(target_os = "windows")]
            {
                if let Ok(local) = std::env::var("LOCALAPPDATA") {
                    candidates.push(
                        std::path::PathBuf::from(&local)
                            .join("Toolbox")
                            .join("bin")
                            .join(format!("{}.exe", bin_name)),
                    );
                    candidates.push(
                        std::path::PathBuf::from(&local)
                            .join("Programs")
                            .join(format!("{}.exe", bin_name)),
                    );
                }
                if let Some(ref h) = home {
                    candidates.push(
                        h.join(".local")
                            .join("bin")
                            .join(format!("{}.exe", bin_name)),
                    );
                }
            }

            // macOS-specific locations
            #[cfg(target_os = "macos")]
            {
                if let Some(ref h) = home {
                    candidates.push(h.join(".local").join("bin").join(bin_name));
                    candidates.push(h.join(".toolbox").join("bin").join(bin_name));
                }
                candidates.push(std::path::PathBuf::from("/usr/local/bin").join(bin_name));
                candidates.push(std::path::PathBuf::from("/opt/homebrew/bin").join(bin_name));
            }

            // Linux-specific locations
            #[cfg(target_os = "linux")]
            {
                if let Some(ref h) = home {
                    candidates.push(h.join(".local").join("bin").join(bin_name));
                    candidates.push(h.join(".toolbox").join("bin").join(bin_name));
                    candidates.push(h.join("bin").join(bin_name));
                }
                candidates.push(std::path::PathBuf::from("/usr/local/bin").join(bin_name));
                candidates.push(std::path::PathBuf::from("/usr/bin").join(bin_name));
                candidates.push(std::path::PathBuf::from("/snap/bin").join(bin_name));
            }

            // Also check PATH via `which` / `where`. CREATE_NO_WINDOW
            // matters because the settings UI's Connection page calls
            // detect_agents during normal startup — without the flag
            // each `where` flashes a DOS window.
            let where_or_which = if cfg!(windows) { "where" } else { "which" };
            let mut where_cmd = std::process::Command::new(where_or_which);
            where_cmd.arg(bin_name);
            crate::os::configure_no_window(&mut where_cmd);
            if let Ok(output) = where_cmd.output() {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    for line in stdout.lines() {
                        let p = std::path::PathBuf::from(line.trim());
                        if !candidates.contains(&p) {
                            candidates.push(p);
                        }
                    }
                }
            }

            for path in candidates {
                if !path.exists() {
                    continue;
                }
                let path_str = path.to_string_lossy().to_string();

                // Skip duplicate detections of the same binary.
                if agents.iter().any(|a: &DetectedAgent| a.path == path_str) {
                    continue;
                }

                // Try to capture a version string. Skipped when the
                // preset declares no version_args (some adapters don't
                // implement --version).
                let version = if hint.version_args.is_empty() {
                    None
                } else {
                    let mut version_cmd = std::process::Command::new(&path);
                    version_cmd.args(hint.version_args);
                    // CREATE_NO_WINDOW: see comment above on the
                    // where/which call.
                    crate::os::configure_no_window(&mut version_cmd);
                    version_cmd.output().ok().and_then(|o| {
                        if o.status.success() {
                            let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
                            if !v.is_empty() {
                                Some(v)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                };

                let spawn_command = if hint.acp_args.is_empty() {
                    path_str.clone()
                } else {
                    format!("{} {}", path_str, hint.acp_args.join(" "))
                };

                agents.push(DetectedAgent {
                    name: preset.display_name.to_string(),
                    preset_id: preset.id.to_string(),
                    path: path_str,
                    spawn_command,
                    version,
                });
            }
        }
    }

    agents
}
