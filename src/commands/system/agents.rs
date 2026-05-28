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
    /// For wrapper-needed entries, this is the spawn command that
    /// *would* be valid once the wrapper is installed — it points at
    /// the wrapper binary by name (resolved via PATH at spawn time),
    /// not at `path` (which is the bare CLI).
    pub spawn_command: String,
    /// Output of `<binary> --version` when it succeeded.
    pub version: Option<String>,
    /// When set, the detected binary is not ACP-capable on its own and
    /// requires this npm package as a wrapper before Kage can use it.
    /// The UI surfaces an "Install wrapper" button instead of "Use
    /// this agent". `None` for ready-to-use detections.
    pub needs_wrapper_npm_package: Option<String>,
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

                // For wrapper-needed hints (e.g. bare `claude`), the
                // saved spawn command should target the wrapper binary
                // — the bare CLI doesn't speak ACP. We use the bare
                // wrapper name (no absolute path) so it resolves via
                // PATH after `npm install -g`, which is where the
                // wrapper lands on every supported OS.
                let spawn_command = if let Some(_pkg) = hint.wrapper_npm_package {
                    "claude-code-acp".to_string()
                } else if hint.acp_args.is_empty() {
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
                    needs_wrapper_npm_package: hint.wrapper_npm_package.map(|s| s.to_string()),
                });
            }
        }
    }

    // Suppress wrapper-needed entries when a ready-to-use detection of
    // the same preset is already in the list — no point nagging the
    // user to install a wrapper they already have. We can't decide
    // this inside the hint loop because the wrapper binary and the
    // bare CLI are detected from different hints.
    let ready_preset_ids: std::collections::HashSet<String> = agents
        .iter()
        .filter(|a| a.needs_wrapper_npm_package.is_none())
        .map(|a| a.preset_id.clone())
        .collect();
    agents.retain(|a| {
        a.needs_wrapper_npm_package.is_none() || !ready_preset_ids.contains(&a.preset_id)
    });

    agents
}

/// Status of `npm` on the user's machine — informs whether the
/// "Install ACP wrapper" UI can attempt the install or has to fall
/// back to "install Node.js first".
#[derive(serde::Serialize, Clone, Default)]
pub struct NpmStatus {
    /// True when `npm` resolves on PATH and `npm --version` succeeded.
    pub available: bool,
    /// `npm --version` stdout when available.
    pub version: Option<String>,
    /// Resolved absolute path to the npm binary (mostly for diagnostics).
    pub path: Option<String>,
}

/// Check whether `npm` is available on PATH and probe its version.
/// Cheap; called from the welcome wizard and Settings → Connection
/// before showing the wrapper-install button.
#[tauri::command]
pub async fn check_npm_available() -> Result<NpmStatus, AppError> {
    Ok(
        tauri::async_runtime::spawn_blocking(check_npm_available_sync)
            .await
            .map_err(|e| format!("Task error: {}", e))?,
    )
}

fn check_npm_available_sync() -> NpmStatus {
    // On Windows npm is typically `npm.cmd`; on Unix it's `npm`. Try
    // the canonical name first, then `.cmd` as a fallback so we don't
    // spuriously report "missing" on systems where only the cmd
    // shim is on PATH.
    let candidates: &[&str] = if cfg!(windows) {
        &["npm.cmd", "npm"]
    } else {
        &["npm"]
    };

    for name in candidates {
        let resolved = resolve_binary_path(name);
        if let Some(ref p) = resolved {
            let mut version_cmd = std::process::Command::new(p);
            version_cmd.arg("--version");
            crate::os::configure_no_window(&mut version_cmd);
            let version = version_cmd.output().ok().and_then(|o| {
                if o.status.success() {
                    let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if v.is_empty() {
                        None
                    } else {
                        Some(v)
                    }
                } else {
                    None
                }
            });
            return NpmStatus {
                available: version.is_some(),
                version,
                path: resolved,
            };
        }
    }
    NpmStatus::default()
}

/// Install one of the allowlisted ACP wrapper packages globally via
/// `npm install -g`. Blocking — the UI shows a spinner and disables
/// the button while it runs. Returns the combined stdout/stderr so the
/// caller can show diagnostics on failure.
///
/// The package name is checked against
/// [`crate::agent_presets::ALLOWED_WRAPPER_NPM_PACKAGES`]. Anything
/// outside that list is rejected — the IPC surface should not be a
/// general-purpose `npm install` runner.
#[tauri::command]
pub async fn install_acp_wrapper(package: String) -> Result<String, AppError> {
    if !crate::agent_presets::ALLOWED_WRAPPER_NPM_PACKAGES
        .iter()
        .any(|allowed| *allowed == package)
    {
        return Err(AppError::from(format!(
            "package not allowlisted for install: {}",
            package
        )));
    }

    tauri::async_runtime::spawn_blocking(move || install_acp_wrapper_sync(&package))
        .await
        .map_err(|e| format!("Task error: {}", e))?
}

fn install_acp_wrapper_sync(package: &str) -> Result<String, AppError> {
    let npm_status = check_npm_available_sync();
    let npm_path = npm_status
        .path
        .as_deref()
        .ok_or_else(|| AppError::from("npm not found on PATH"))?;

    let mut cmd = std::process::Command::new(npm_path);
    cmd.arg("install").arg("-g").arg(package);
    crate::os::configure_no_window(&mut cmd);

    let out = cmd
        .output()
        .map_err(|e| AppError::from(format!("failed to spawn npm: {}", e)))?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{}{}", stdout, stderr);

    if !out.status.success() {
        return Err(AppError::from(format!(
            "npm install failed (exit {}): {}",
            out.status.code().unwrap_or(-1),
            combined.trim()
        )));
    }

    Ok(combined.trim().to_string())
}

#[cfg(test)]
mod tests {
    use crate::agent_presets::{detection_hints, AgentKind, ALLOWED_WRAPPER_NPM_PACKAGES};

    #[test]
    fn detection_hints_include_bare_claude_with_wrapper() {
        let hints = detection_hints();
        let bare = hints
            .iter()
            .find(|h| h.kind == AgentKind::ClaudeCode && h.binary_names.contains(&"claude"))
            .expect("bare-claude detection hint missing");
        assert_eq!(
            bare.wrapper_npm_package,
            Some("@zed-industries/claude-code-acp"),
            "bare-claude hint must point at the Zed wrapper package"
        );
    }

    #[test]
    fn ready_to_use_hints_have_no_wrapper() {
        for hint in detection_hints() {
            // Only the bare-claude hint declares a wrapper requirement.
            // A ready-to-use binary advertising one would mean the UI
            // shows an "install wrapper" button for an already-working
            // agent.
            let is_bare_claude =
                hint.kind == AgentKind::ClaudeCode && hint.binary_names == ["claude"];
            if !is_bare_claude {
                assert!(
                    hint.wrapper_npm_package.is_none(),
                    "hint for {:?} ({:?}) should not require a wrapper",
                    hint.kind,
                    hint.binary_names
                );
            }
        }
    }

    #[test]
    fn wrapper_install_rejects_unallowlisted_package() {
        // The `install_acp_wrapper` allowlist is the security boundary
        // for the IPC surface — drift here and we'd be exposing an
        // arbitrary `npm install -g` runner to the frontend.
        assert!(
            ALLOWED_WRAPPER_NPM_PACKAGES.contains(&"@zed-industries/claude-code-acp"),
            "the Claude wrapper must be in the install allowlist"
        );
        assert!(
            !ALLOWED_WRAPPER_NPM_PACKAGES.contains(&"left-pad"),
            "allowlist must reject arbitrary packages"
        );
        assert!(
            !ALLOWED_WRAPPER_NPM_PACKAGES.contains(&""),
            "allowlist must reject empty package names"
        );
    }
}
