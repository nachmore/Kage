//! Agent presets — the registry of known ACP-compatible agents.
//!
//! Each preset describes:
//!   - Display name and stable id (used in config to remember which preset
//!     a saved connection was created from).
//!   - Binary name(s) per OS, search paths, and the args passed to invoke
//!     it as an ACP server.
//!   - Optional `dot_dir` for agents that have an on-disk home directory
//!     layout we read from (sessions, mcp.json). Currently only Kiro.
//!   - Install URL surfaced in the UI when a preset is selected but no
//!     binary is found.
//!
//! Two consumers:
//!   1. `detect_agents` (commands/system.rs) — scans presets for installed
//!      binaries and returns a list the UI can render as "found agents".
//!   2. The settings + welcome UIs — render preset metadata (install URL,
//!      auth notes, etc.) when the user picks a preset.

use std::path::PathBuf;

/// Stable identifiers for known agents. Persisted in config as
/// `connection.preset_id`, so be careful not to rename existing variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    /// Kiro CLI — uses ~/.kiro/
    Kiro,
    /// Anthropic's Claude Code via the @agentclientprotocol/claude-agent-acp
    /// adapter (npm).
    ClaudeCode,
    /// OpenAI Codex via the @zed-industries/codex-acp adapter.
    Codex,
    /// Local model running on Ollama, routed through the Codex
    /// adapter's OpenAI-compatible endpoint. Distinct from `Codex`
    /// proper because the UI's edit experience is the Ollama wizard
    /// (test connection, model dropdown) — not the raw spawn-command
    /// form. Detection-wise we don't probe binaries for this preset:
    /// it's only ever set when the user runs the Ollama wizard.
    Ollama,
}

/// Static preset metadata. Returned by `preset()` and used by both
/// detection (which binaries to look for) and the UI (install copy,
/// preset id strings).
pub struct AgentPreset {
    /// Stable id used in config.
    pub id: &'static str,
    /// Human-friendly name shown in the UI.
    pub display_name: &'static str,
    /// Short description shown next to the name.
    pub description: &'static str,
    /// Short URL for the install / docs page.
    pub install_url: &'static str,
    /// True when this preset typically requires a user-provided API key
    /// or login. UI surfaces this as a setup hint.
    pub requires_auth: bool,
    /// Hint about how auth is provided (env var name etc.).
    pub auth_hint: Option<&'static str>,
}

impl AgentKind {
    /// All known kinds in detect-probe order. Ollama is included so
    /// `from_id("ollama")` resolves, but the binary-probe path
    /// (`detection_hints`) skips it — Ollama connections are only
    /// created via the wizard, never auto-detected as a binary.
    pub fn all() -> &'static [AgentKind] {
        &[
            AgentKind::Kiro,
            AgentKind::ClaudeCode,
            AgentKind::Codex,
            AgentKind::Ollama,
        ]
    }

    /// Resolve the static preset metadata.
    pub fn preset(&self) -> AgentPreset {
        match self {
            AgentKind::Kiro => AgentPreset {
                id: "kiro",
                display_name: "Kiro CLI",
                description: "Local Kiro agent backend.",
                install_url: "https://kiro.dev/",
                requires_auth: false,
                auth_hint: None,
            },
            AgentKind::ClaudeCode => AgentPreset {
                id: "claude-code",
                display_name: "Claude Code",
                description: "Anthropic Claude via the official ACP adapter.",
                install_url: "https://github.com/agentclientprotocol/claude-agent-acp",
                requires_auth: true,
                auth_hint: Some("Sign in via `claude` CLI, or set ANTHROPIC_API_KEY."),
            },
            AgentKind::Codex => AgentPreset {
                id: "codex",
                display_name: "OpenAI Codex",
                description: "OpenAI Codex via the Zed ACP adapter.",
                install_url: "https://github.com/zed-industries/codex-acp",
                requires_auth: true,
                auth_hint: Some("Set OPENAI_API_KEY (or CODEX_API_KEY)."),
            },
            AgentKind::Ollama => AgentPreset {
                id: "ollama",
                display_name: "Ollama (local model)",
                description: "Local model via Ollama, routed through the Codex ACP adapter.",
                install_url: "https://ollama.com/download",
                requires_auth: false,
                auth_hint: None,
            },
        }
    }

    /// Look up a preset from its stable string id (as stored in config).
    /// Unknown ids return `None` so callers can degrade to a "custom"
    /// connection with no preset metadata.
    pub fn from_id(id: &str) -> Option<AgentKind> {
        Self::all().iter().copied().find(|k| k.preset().id == id)
    }

    /// The home-relative dot-directory for this agent, if it has one.
    /// Used by the MCP / sessions path resolvers for agents that store
    /// data under a known home subdirectory (e.g. `~/.kiro/`).
    pub fn dot_dir(&self) -> Option<&'static str> {
        match self {
            AgentKind::Kiro => Some(".kiro"),
            // ClaudeCode and Codex talk to their own backends and don't
            // expose a session-store layout we read from. Ollama runs
            // through the Codex adapter, same story.
            AgentKind::ClaudeCode | AgentKind::Codex | AgentKind::Ollama => None,
        }
    }

    /// MCP settings path: ~/<dot_dir>/settings/mcp.json (only if the
    /// agent has a dot-dir).
    pub fn mcp_json_path(&self) -> Option<PathBuf> {
        let dot = self.dot_dir()?;
        dirs::home_dir().map(|h| h.join(dot).join("settings").join("mcp.json"))
    }

    /// Sessions directory: ~/<dot_dir>/sessions/cli/ (only if the agent
    /// has a dot-dir).
    pub fn sessions_dir(&self) -> Option<PathBuf> {
        let dot = self.dot_dir()?;
        dirs::home_dir().map(|h| h.join(dot).join("sessions").join("cli"))
    }
}

/// Per-OS detection hint for an agent. Returned by [`detection_hints`]
/// and consumed by `commands::system::detect_agents_sync`.
///
/// `binary_names` is a list because a preset may ship under several
/// names (e.g. `claude-code-acp` and the npx-vended package). The
/// detector tries each in turn, in well-known install locations and on
/// PATH.
pub struct DetectionHint {
    pub kind: AgentKind,
    /// Candidate binary names without OS extension (the detector adds
    /// `.exe` on Windows). The first name wins for display purposes.
    pub binary_names: &'static [&'static str],
    /// Args passed to the binary when invoked as an ACP server. Empty
    /// means "no args".
    pub acp_args: &'static [&'static str],
    /// Args used to print a version string. Empty means "skip the
    /// version probe".
    pub version_args: &'static [&'static str],
    /// When set, finding this binary doesn't yield a directly-usable
    /// agent — it indicates the underlying CLI is installed but needs
    /// an ACP wrapper from npm before Kage can talk to it. The detector
    /// will surface a "needs wrapper" entry pointing at this package.
    /// `None` for hints whose binaries are already ACP servers.
    pub wrapper_npm_package: Option<&'static str>,
}

/// Detection hints for all known presets. Keep in sync with
/// `AgentKind::all()`.
pub fn detection_hints() -> &'static [DetectionHint] {
    // `static` slice so callers can iterate without allocation.
    &[
        DetectionHint {
            kind: AgentKind::Kiro,
            binary_names: &["kiro-cli"],
            acp_args: &["acp"],
            version_args: &["--version"],
            wrapper_npm_package: None,
        },
        DetectionHint {
            kind: AgentKind::ClaudeCode,
            // The npm package installs the binary as `claude-code-acp`;
            // some users may have it as `claude-agent-acp`. Detect both.
            binary_names: &["claude-code-acp", "claude-agent-acp"],
            acp_args: &[],
            version_args: &["--version"],
            wrapper_npm_package: None,
        },
        // Bare Anthropic `claude` CLI — not ACP itself. We surface this
        // so the UI can offer to install the wrapper that makes it
        // usable (the binary above). The detector tags hits from this
        // hint with `wrapper_npm_package` so the UI shows an "Install
        // wrapper" affordance instead of "Use this agent".
        DetectionHint {
            kind: AgentKind::ClaudeCode,
            binary_names: &["claude"],
            acp_args: &[],
            // Skip version probe — `claude --version` would work but we
            // don't display it for wrapper-needed entries (the message
            // is "install the wrapper", not "you have v1.2.3").
            version_args: &[],
            wrapper_npm_package: Some("@zed-industries/claude-code-acp"),
        },
        DetectionHint {
            kind: AgentKind::Codex,
            binary_names: &["codex-acp"],
            acp_args: &[],
            version_args: &["--version"],
            wrapper_npm_package: None,
        },
    ]
}

/// npm package names the [`install_acp_wrapper`] command is allowed to
/// install globally. Anything outside this list is rejected — the
/// command is exposed to the frontend, so a strict allowlist keeps the
/// IPC surface from becoming an arbitrary `npm install` runner.
pub const ALLOWED_WRAPPER_NPM_PACKAGES: &[&str] = &["@zed-industries/claude-code-acp"];

/// Detect the agent kind from a spawn command string. Looks for known
/// binary names anywhere in the command (handles `/path/to/codex-acp`
/// and `npx @zed-industries/codex-acp` alike).
pub fn detect_from_command(spawn_command: &str) -> Option<AgentKind> {
    let lower = spawn_command.to_lowercase();
    // Order matters when names overlap — most specific first.
    if lower.contains("codex-acp") || lower.contains("codex") {
        return Some(AgentKind::Codex);
    }
    if lower.contains("claude-code-acp")
        || lower.contains("claude-agent-acp")
        || lower.contains("claude-agent")
    {
        return Some(AgentKind::ClaudeCode);
    }
    if lower.contains("kiro") {
        return Some(AgentKind::Kiro);
    }
    None
}

/// Detect the agent kind from the app config — looks at the *active*
/// connection. Returns Kiro as the safe default (it owns the dot-dir
/// path resolvers below).
pub fn detect(config: &crate::config::Config) -> AgentKind {
    // Prefer an explicit preset id stored on the active connection.
    if let Some(conn) = config.acp.active_connection() {
        if let Some(ref preset_id) = conn.preset_id {
            if let Some(kind) = AgentKind::from_id(preset_id) {
                return kind;
            }
        }
        if let crate::config::AcpMode::Local { ref spawn_command } = conn.mode {
            if let Some(kind) = detect_from_command(spawn_command) {
                return kind;
            }
        }
    }
    AgentKind::Kiro
}

/// Resolve the MCP json path, respecting the user's explicit override.
/// If `mcp_config_path` is set in config, use that. Otherwise use the
/// detected agent preset; falls back to Kiro if the active agent has
/// no dot-dir layout.
#[allow(dead_code)] // Will be used when settings UI is wired to agent presets
pub fn resolve_mcp_json_path(config: &crate::config::Config) -> Option<PathBuf> {
    if let Some(ref custom) = config.mcp_config_path {
        if !custom.is_empty() {
            let p = PathBuf::from(custom);
            if p.is_absolute() {
                return Some(p);
            }
            return dirs::home_dir().map(|h| h.join(custom));
        }
    }
    detect(config)
        .mcp_json_path()
        .or_else(|| AgentKind::Kiro.mcp_json_path())
}

/// Resolve the sessions directory, respecting the user's explicit override.
pub fn resolve_sessions_dir(config: &crate::config::Config) -> Option<PathBuf> {
    let home = dirs::home_dir()?;

    // 1) Explicit override on the active connection
    if let Some(ref dir) = config
        .acp
        .active_connection()
        .and_then(|c| c.sessions_directory.clone())
    {
        let p = PathBuf::from(dir);
        if p.is_absolute() {
            return Some(p);
        }
        return Some(home.join(dir));
    }

    // 2) Active agent preset directory
    let kind = detect(config);
    if let Some(preset_dir) = kind.sessions_dir() {
        if preset_dir.exists() {
            return Some(preset_dir);
        }
    }

    // 3) Probe all known agent paths
    for agent in AgentKind::all() {
        if let Some(dir) = agent.sessions_dir() {
            if dir.exists() {
                return Some(dir);
            }
        }
    }

    // 4) Fall back to Kiro's path (the only agent with a dot-dir today).
    AgentKind::Kiro.sessions_dir()
}

/// Default MCP json path without config context — probes known paths.
pub fn default_mcp_json_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    for agent in AgentKind::all() {
        if let Some(dot) = agent.dot_dir() {
            let p = home.join(dot).join("settings").join("mcp.json");
            if p.exists() {
                return Some(p);
            }
        }
    }
    AgentKind::Kiro.mcp_json_path()
}

/// Default sessions dir without config context — probes known paths.
pub fn default_sessions_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    for agent in AgentKind::all() {
        if let Some(dot) = agent.dot_dir() {
            let dir = home.join(dot).join("sessions").join("cli");
            if dir.exists() {
                return Some(dir);
            }
        }
    }
    AgentKind::Kiro.sessions_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_from_command_matches_known_agents() {
        assert_eq!(
            detect_from_command("/usr/local/bin/kiro-cli acp"),
            Some(AgentKind::Kiro)
        );
        assert_eq!(
            detect_from_command("npx @zed-industries/codex-acp"),
            Some(AgentKind::Codex)
        );
        assert_eq!(
            detect_from_command("claude-code-acp"),
            Some(AgentKind::ClaudeCode)
        );
        assert_eq!(detect_from_command("some-other-thing"), None);
    }

    #[test]
    fn from_id_roundtrip() {
        for kind in AgentKind::all() {
            let id = kind.preset().id;
            assert_eq!(AgentKind::from_id(id), Some(*kind));
        }
        assert_eq!(AgentKind::from_id("nope"), None);
    }

    #[test]
    fn dot_dir_only_for_kiro() {
        assert_eq!(AgentKind::Kiro.dot_dir(), Some(".kiro"));
        assert_eq!(AgentKind::ClaudeCode.dot_dir(), None);
        assert_eq!(AgentKind::Codex.dot_dir(), None);
        assert_eq!(AgentKind::Ollama.dot_dir(), None);
    }

    #[test]
    fn ollama_preset_resolves_but_has_no_detection_hint() {
        // Ollama is intentionally NOT in `detection_hints()` — there's
        // no binary to probe — but `from_id("ollama")` must still
        // resolve so a saved connection can round-trip.
        assert_eq!(AgentKind::from_id("ollama"), Some(AgentKind::Ollama));
        let hint_kinds: Vec<AgentKind> = detection_hints().iter().map(|h| h.kind).collect();
        assert!(!hint_kinds.contains(&AgentKind::Ollama));
    }
}
