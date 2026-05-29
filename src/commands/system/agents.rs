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

            // npm-installed Windows binaries land as a pair: a Unix
            // script with no extension *and* a `.cmd` shim in the same
            // folder. `where` returns both, so without dedup the user
            // sees two "Claude Code" entries pointing at the same
            // install. Collapse same-(dir, stem) candidates and prefer
            // the executable extension (.exe > .cmd > .bat > no-ext).
            let candidates = dedupe_shim_candidates(candidates);

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
                //
                // CLIs vary wildly in --version output:
                //   `kiro-cli --version` → "kiro-cli-chat 0.0.0-dev"
                //   `claude --version`   → multi-line, version on its
                //                          own line "2.1.128 (Claude Code)"
                // Whitespace-splitting the first line picked up
                // "kiro-cli-chat" as the version. We extract the first
                // token that looks like a version (digit-led, contains
                // a dot) instead. Some CLIs print to stderr, so merge
                // both streams before parsing.
                let version = if hint.version_args.is_empty() {
                    None
                } else {
                    let mut version_cmd = std::process::Command::new(&path);
                    version_cmd.args(hint.version_args);
                    // CREATE_NO_WINDOW: see comment above on the
                    // where/which call.
                    crate::os::configure_no_window(&mut version_cmd);
                    version_cmd.output().ok().and_then(|o| {
                        if !o.status.success() {
                            return None;
                        }
                        let mut combined = String::from_utf8_lossy(&o.stdout).into_owned();
                        combined.push('\n');
                        combined.push_str(&String::from_utf8_lossy(&o.stderr));
                        extract_version(&combined)
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

/// Collapse candidate paths that point at the same logical binary
/// because of Windows shim pairs. npm `-g` drops both `claude-code-acp`
/// and `claude-code-acp.cmd` in the same directory; `where` returns
/// both. We keep one entry per (parent_dir, file_stem) and prefer the
/// most-executable extension so the version probe and downstream
/// `Command::new` get a binary that actually runs.
///
/// Order is preserved beyond the dedup so the rest of the detector's
/// candidate ordering (well-known dirs first, PATH last) keeps
/// determining display order.
fn dedupe_shim_candidates(paths: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    use std::collections::HashMap;

    fn ext_priority(p: &std::path::Path) -> u8 {
        // Higher = preferred. .exe wins over .cmd/.bat (which are
        // shell shims) which win over no extension. Anything else
        // ranks lowest so unexpected extensions can still appear if
        // they're the only option.
        match p
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            Some("exe") => 4,
            Some("cmd") => 3,
            Some("bat") => 2,
            None => 1,
            _ => 0,
        }
    }

    // Index of (dir, stem) -> position in `out`. Lets us replace an
    // already-kept entry when a higher-priority sibling shows up
    // without rebuilding the vector.
    let mut keep: HashMap<(std::path::PathBuf, String), usize> = HashMap::new();
    let mut out: Vec<std::path::PathBuf> = Vec::with_capacity(paths.len());

    for path in paths {
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        let key = (dir, stem);

        match keep.get(&key) {
            Some(&idx) => {
                if ext_priority(&path) > ext_priority(&out[idx]) {
                    out[idx] = path;
                }
            }
            None => {
                keep.insert(key, out.len());
                out.push(path);
            }
        }
    }
    out
}

/// Pull the first version-shaped token out of a CLI's `--version`
/// output. A token qualifies if it starts with a digit and contains a
/// dot (so `2.1.128`, `0.0.0-dev`, `v1.2` all match) — we skip leading
/// `v`/`V` so the returned string is the version proper. Returns the
/// raw token (e.g. `2.1.128` or `0.0.0-dev`) or `None` if nothing
/// matched, in which case the UI shows no version badge.
///
/// Real-world inputs we've seen:
///   `kiro-cli-chat 0.0.0-dev`
///   `claude: info: builder-mcp setup: stamp_exists\n2.1.128 (Claude Code)`
fn extract_version(output: &str) -> Option<String> {
    for line in output.lines() {
        for raw in line.split_whitespace() {
            let token = raw.trim_start_matches(['v', 'V']);
            if token.contains('.') && token.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return Some(token.to_string());
            }
        }
    }
    None
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
    use super::{dedupe_shim_candidates, extract_version};
    use crate::agent_presets::{detection_hints, AgentKind, ALLOWED_WRAPPER_NPM_PACKAGES};
    use std::path::PathBuf;

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

    #[test]
    fn extract_version_picks_first_dotted_digit_token() {
        // The bug we're fixing: whitespace-splitting the first line of
        // `kiro-cli --version` returned "kiro-cli-chat" as the version.
        assert_eq!(
            extract_version("kiro-cli-chat 0.0.0-dev"),
            Some("0.0.0-dev".to_string()),
        );
        // Claude prints diagnostics to a leading line and the version
        // is on its own line, with the actual number followed by a
        // human label in parens. We want only the version.
        assert_eq!(
            extract_version("claude: info: builder-mcp setup: stamp_exists\n2.1.128 (Claude Code)"),
            Some("2.1.128".to_string()),
        );
        // `v`-prefixed versions are common (semver tooling, Go, …) —
        // strip the prefix so the badge shows the version proper.
        assert_eq!(extract_version("foo v1.2.3"), Some("1.2.3".to_string()));
        // Bare digit-only strings without a dot aren't semver-shaped
        // and are usually exit codes or year-stamps — skip.
        assert_eq!(extract_version("build 12345"), None);
        assert_eq!(extract_version(""), None);
    }

    #[test]
    fn dedupe_shim_candidates_collapses_npm_pair_keeping_cmd() {
        // npm `-g` on Windows installs a Unix script (no extension)
        // alongside a `.cmd` shim in the same directory. `where`
        // returns both, so without dedup the user sees two cards.
        // Both files exist, but `.cmd` is the one Windows knows how
        // to run via `Command::new`.
        let inputs = vec![
            PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\claude-code-acp"),
            PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\claude-code-acp.cmd"),
        ];
        let out = dedupe_shim_candidates(inputs);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0],
            PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\claude-code-acp.cmd"),
            ".cmd should win over the no-extension shim"
        );
    }

    #[test]
    fn dedupe_shim_candidates_keeps_distinct_installs() {
        // Two genuinely different installs must NOT collapse — same
        // stem in different directories is a real "user has two
        // copies" case (Toolbox vs. local install). Preserve both.
        let inputs = vec![
            PathBuf::from(r"C:\Users\me\AppData\Local\Toolbox\bin\kiro-cli.exe"),
            PathBuf::from(r"C:\Users\me\AppData\Local\kiro-cli\kiro-cli.exe"),
        ];
        let out = dedupe_shim_candidates(inputs.clone());
        assert_eq!(out, inputs);
    }

    #[test]
    fn dedupe_shim_candidates_prefers_exe_over_cmd() {
        let inputs = vec![
            PathBuf::from(r"C:\foo\agent.cmd"),
            PathBuf::from(r"C:\foo\agent.exe"),
        ];
        let out = dedupe_shim_candidates(inputs);
        assert_eq!(out, vec![PathBuf::from(r"C:\foo\agent.exe")]);
    }
}
