//! Agent presets — maps detected agent type to the correct directory layout.
//!
//! Different CLI agents store their data in different home-directory prefixes.
//! This module detects the agent from the spawn command and provides the right
//! paths for MCP config, sessions, etc.
//!
//! Currently only Kiro is supported. The architecture is ready for adding
//! Claude and other agents in the future.

use std::path::PathBuf;

/// Known agent types with their directory conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    /// Kiro agent — uses ~/.kiro/
    Kiro,
}

impl AgentKind {
    /// The home-relative dot-directory for this agent (e.g. ".kiro").
    pub fn dot_dir(&self) -> &'static str {
        match self {
            AgentKind::Kiro => ".kiro",
        }
    }

    /// MCP settings path: ~/<dot_dir>/settings/mcp.json
    pub fn mcp_json_path(&self) -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(self.dot_dir()).join("settings").join("mcp.json"))
    }

    /// Sessions directory: ~/<dot_dir>/sessions/cli/
    pub fn sessions_dir(&self) -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(self.dot_dir()).join("sessions").join("cli"))
    }

    /// Settings directory: ~/<dot_dir>/settings/
    #[allow(dead_code)] // Available for future use
    pub fn settings_dir(&self) -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(self.dot_dir()).join("settings"))
    }

    /// All known agent kinds, in probe order (preferred first).
    pub fn all() -> &'static [AgentKind] {
        &[AgentKind::Kiro]
    }
}

/// Detect the agent kind from a spawn command string.
/// Currently always returns Kiro. Future: match on "claude-cli", etc.
pub fn detect_from_command(_spawn_command: &str) -> AgentKind {
    // Future: inspect the command for agent-specific patterns
    // e.g. if cmd.contains("claude") { AgentKind::Claude }
    AgentKind::Kiro
}

/// Detect the agent kind from the app config.
pub fn detect(config: &crate::config::Config) -> AgentKind {
    match &config.acp.mode {
        crate::config::AcpMode::Local { spawn_command } => detect_from_command(spawn_command),
        _ => AgentKind::Kiro,
    }
}

/// Resolve the MCP json path, respecting the user's explicit override.
/// If `mcp_config_path` is set in config, use that. Otherwise use the agent preset.
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
    detect(config).mcp_json_path()
}

/// Resolve the sessions directory, respecting the user's explicit override.
pub fn resolve_sessions_dir(config: &crate::config::Config) -> Option<PathBuf> {
    let home = dirs::home_dir()?;

    // 1) Explicit override
    if let Some(ref dir) = config.acp.agent.sessions_directory {
        let p = PathBuf::from(dir);
        if p.is_absolute() {
            return Some(p);
        }
        return Some(home.join(dir));
    }

    // 2) Agent preset directory
    let kind = detect(config);
    let preset_dir = home.join(kind.dot_dir()).join("sessions").join("cli");
    if preset_dir.exists() {
        return Some(preset_dir);
    }

    // 3) Probe all known agent paths
    for agent in AgentKind::all() {
        let dir = home.join(agent.dot_dir()).join("sessions").join("cli");
        if dir.exists() {
            return Some(dir);
        }
    }

    // 4) Default to the detected agent's path
    Some(preset_dir)
}

/// Default MCP json path without config context — probes known paths.
pub fn default_mcp_json_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    for agent in AgentKind::all() {
        let p = home.join(agent.dot_dir()).join("settings").join("mcp.json");
        if p.exists() {
            return Some(p);
        }
    }
    AgentKind::Kiro.mcp_json_path()
}

/// Default sessions dir without config context — probes known paths.
pub fn default_sessions_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    for agent in AgentKind::all() {
        let dir = home.join(agent.dot_dir()).join("sessions").join("cli");
        if dir.exists() {
            return Some(dir);
        }
    }
    AgentKind::Kiro.sessions_dir()
}
