use super::default_true;
use serde::{Deserialize, Serialize};

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            start_session_on_launch: true,
            auto_steering_enabled: false,
            user_steering_path: None,
            default_model: None,
            working_directory: None,
            auto_compact_threshold: 90,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyConfig {
    #[serde(default)]
    pub modifiers: Vec<String>,
    #[serde(default)]
    pub key: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        // The primary launcher hotkey: Alt+Space.
        Self {
            modifiers: vec!["Alt".to_string()],
            key: "Space".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpConfig {
    /// Saved agent connections. Always contains at least one entry
    /// after the first launch (the welcome flow seeds one).
    #[serde(default)]
    pub connections: Vec<AgentConnection>,
    /// The id of the active connection in `connections`. The runtime
    /// transport reads only the active connection's mode.
    #[serde(default)]
    pub active_connection_id: String,
    #[serde(default)]
    pub agent: AgentConfig,
}

impl Default for AcpConfig {
    fn default() -> Self {
        // Seed one Remote connection (the welcome flow overwrites it).
        Self {
            connections: vec![AgentConnection {
                id: "default".to_string(),
                name: "Default".to_string(),
                preset_id: None,
                mode: AcpMode::Remote {
                    host: "127.0.0.1".to_string(),
                    port: 8765,
                    timeout_ms: 30000,
                },
                sessions_directory: None,
                ollama_settings: None,
            }],
            active_connection_id: "default".to_string(),
            agent: AgentConfig::default(),
        }
    }
}

impl AcpConfig {
    /// The currently selected connection. Falls back to the first
    /// entry if the active id no longer matches anything (which can
    /// happen if a connection was deleted out-of-band).
    pub fn active_connection(&self) -> Option<&AgentConnection> {
        self.connections
            .iter()
            .find(|c| c.id == self.active_connection_id)
            .or_else(|| self.connections.first())
    }

    /// Mutable variant of [`active_connection`]. Reserved for future
    /// callers that need to edit a connection in place; today the JS
    /// side replaces the whole config via `save_config`.
    #[allow(dead_code)]
    pub fn active_connection_mut(&mut self) -> Option<&mut AgentConnection> {
        let id = self.active_connection_id.clone();
        let idx = self.connections.iter().position(|c| c.id == id).or({
            if self.connections.is_empty() {
                None
            } else {
                Some(0)
            }
        })?;
        self.connections.get_mut(idx)
    }

    /// The active connection's mode. Returns a sensible default
    /// (Remote 127.0.0.1:8765) when no connection is configured yet —
    /// callers that care about the difference should check
    /// `active_connection()` directly.
    pub fn active_mode(&self) -> AcpMode {
        self.active_connection()
            .map(|c| c.mode.clone())
            .unwrap_or_else(|| AcpMode::Remote {
                host: "127.0.0.1".to_string(),
                port: 8765,
                timeout_ms: 30000,
            })
    }
}

/// A saved agent connection. Multiple of these live in
/// `AcpConfig::connections`; the user picks one as active via
/// `active_connection_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConnection {
    /// Stable id (uuid). Persisted across renames so the active pointer
    /// doesn't need to chase the display name.
    /// An empty id is an inert entry: `active_connection()` matches by
    /// exact id and falls back to the list head, so it can still be
    /// selected, renamed, or deleted from the settings UI.
    #[serde(default)]
    pub id: String,
    /// User-facing name (defaults to the preset display name).
    #[serde(default)]
    pub name: String,
    /// Optional preset id (e.g. "kiro", "claude-code", "codex").
    /// `None` means the connection was hand-rolled by the user.
    #[serde(default)]
    pub preset_id: Option<String>,
    /// Connection mode (Local spawn vs. Remote TCP).
    #[serde(default)]
    pub mode: AcpMode,
    /// Custom sessions directory for this agent. If unset, uses the
    /// preset's well-known path (e.g. `~/.kiro/sessions/cli` for Kiro).
    /// Stored per-connection because different agents lay out sessions
    /// in different places.
    #[serde(default)]
    pub sessions_directory: Option<String>,
    /// Ollama-specific settings, when this connection points at a
    /// local model running through codex-acp's OpenAI-compatible
    /// endpoint. Optional — only set when `preset_id == "ollama"`.
    /// Stored alongside the spawn command so the Edit flow can
    /// reopen the Ollama wizard pre-filled instead of dumping the
    /// user into raw env-var-prefixed shell syntax.
    #[serde(default)]
    pub ollama_settings: Option<OllamaConnectionSettings>,
}

/// Ollama-specific knobs persisted on an Ollama-shaped agent
/// connection. The connection's `mode.spawn_command` is the only
/// thing that actually runs at startup; this struct is the
/// editable source of truth the wizard reads + writes so a user
/// can change models or base URL without reverse-engineering the
/// shell incantation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaConnectionSettings {
    /// HTTP base URL of the Ollama daemon — scheme, host, and port.
    /// The wizard appends `/v1` when building the `OPENAI_BASE_URL`
    /// env var. Defaults to the local install.
    #[serde(default = "default_ollama_base_url")]
    pub base_url: String,
    /// Tag-form model name (e.g. `llama3:8b`). Plumbed into the
    /// codex-acp adapter via `OPENAI_MODEL`. Empty means "not chosen
    /// yet" — the wizard treats it as unset and shows the picker.
    #[serde(default)]
    pub model: String,
    /// Show a small "🦙 <model> · <size>" status widget in the
    /// floating window. Off by default — the widget polls
    /// `/api/tags` every ~30s and that adds chatter on the LAN. Users
    /// who want at-a-glance reassurance that the local model is
    /// alive can enable it from the Ollama wizard.
    #[serde(default)]
    pub show_status_widget: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_true")]
    pub start_session_on_launch: bool,
    #[serde(default)]
    pub auto_steering_enabled: bool,
    #[serde(default)]
    pub user_steering_path: Option<String>,
    /// Default model ID to select when creating a new session
    #[serde(default)]
    pub default_model: Option<String>,
    /// Working directory for the agent — it will have access to files under this path
    #[serde(default)]
    pub working_directory: Option<String>,
    /// Auto-compact threshold (0-100). When context usage >= this %, auto-send /compact. 0 = disabled.
    #[serde(default = "default_auto_compact_threshold")]
    pub auto_compact_threshold: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AcpMode {
    Local {
        spawn_command: String,
    },
    Remote {
        host: String,
        port: u16,
        timeout_ms: u64,
    },
}

/// Default for `AcpMode` when an old/hand-edited config entry omits
/// `mode` entirely: an empty Local spawn command. It is deliberately
/// NOT a runnable default — `spawn_backend_process` rejects an empty
/// command with a clear "Empty spawn command" error, which surfaces in
/// the connection UI where the user can fix the entry. That beats both
/// alternatives: failing the whole Config::load (silently resets every
/// setting) or guessing a Remote endpoint that may not exist.
impl Default for AcpMode {
    fn default() -> Self {
        AcpMode::Local {
            spawn_command: String::new(),
        }
    }
}

fn default_ollama_base_url() -> String {
    crate::ollama::DEFAULT_BASE_URL.to_string()
}

fn default_auto_compact_threshold() -> u32 {
    90
}
