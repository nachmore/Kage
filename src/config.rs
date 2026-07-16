use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::config_migrations;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_config_version")]
    pub version: u32,
    #[serde(default)]
    pub hotkey: HotkeyConfig,
    #[serde(default)]
    pub acp: AcpConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub system: SystemConfig,
    #[serde(default)]
    pub shortcuts: Vec<ShortcutConfig>,
    #[serde(default)]
    pub debug_mode: bool,
    #[serde(default)]
    pub tool_permissions: ToolPermissionsConfig,
    #[serde(default)]
    pub first_run_completed: bool,
    #[serde(default)]
    pub updates: UpdateConfig,
    #[serde(default)]
    pub quick_actions: QuickActionsConfig,
    /// Extension configs keyed by extension ID. Each extension owns its own JSON object.
    #[serde(default)]
    pub extensions: HashMap<String, serde_json::Value>,
    /// Enable/disable state for extensions, themes, and command packs keyed by ID.
    #[serde(default)]
    pub extension_states: HashMap<String, bool>,
    /// Capabilities granted by the user to each installed extension. Missing
    /// entry means "no grant recorded" and the extension gets zero
    /// capabilities — it can run but every invoke() will be rejected.
    /// See ui/js/shared/extension-permissions.js for the capability list.
    #[serde(default)]
    pub extension_grants: HashMap<String, ExtensionGrant>,
    /// Pocket TTS configuration (local neural TTS via kyutai-labs/pocket-tts)
    #[serde(default)]
    pub pocket_tts: PocketTtsConfig,
    /// Optional hotkey for clipboard history (e.g. Alt+Shift+V)
    #[serde(default)]
    pub clipboard_hotkey: Option<HotkeyConfig>,
    /// Optional hotkey for inline assist (default: Ctrl+Shift+Space)
    #[serde(default = "default_inline_assist_hotkey")]
    pub inline_assist_hotkey: Option<HotkeyConfig>,
    /// Optional hotkey for voice input (show floating + start speech)
    #[serde(default)]
    pub voice_hotkey: Option<HotkeyConfig>,
    /// Custom store URL (advanced). If empty, uses the default store.
    #[serde(default)]
    pub store_url: Option<String>,
    /// Additional store sources (name + URL pairs). Merged with the primary store.
    #[serde(default)]
    pub store_sources: Vec<StoreSource>,
    /// Custom path to mcp.json. If empty, uses the agent preset path (e.g. ~/.kiro/settings/mcp.json).
    #[serde(default)]
    pub mcp_config_path: Option<String>,
    /// Automatically update installed extensions from the store
    #[serde(default)]
    pub auto_update_extensions: bool,
    /// ISO 8601 timestamp of the last extension update check
    #[serde(default)]
    pub last_extension_update_check: Option<String>,
    /// Macros/Automations — named sequences of AI transformation steps with triggers
    #[serde(default)]
    pub macros: Vec<MacroConfig>,
    /// Power/battery settings for automations
    #[serde(default)]
    pub automation_power: AutomationPowerConfig,
    /// Anonymous product analytics settings. See docs/PRIVACY.md for what
    /// is and isn't collected.
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    /// Per-app context rules ("App Modes"). When the foreground app
    /// matches a rule's `executable` at summon time, the rule's
    /// `steering` is appended to the outgoing prompt as a small
    /// `<_kage_app_steering>` tag. See `src/context_rules.rs`.
    ///
    /// Fresh installs are seeded with `default_context_rules()` (a
    /// curated starter set). Existing users upgrading from a build
    /// that didn't have this field stay empty — `#[serde(default)]`
    /// fills the missing field with `Vec::new()`, NOT with the
    /// struct's default, so the seeding only fires on first install.
    /// Users who delete every rule in the UI persist `[]` to disk and
    /// also stay empty across launches and reinstalls.
    #[serde(default)]
    pub context_rules: Vec<crate::context_rules::ContextRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolPermissionsConfig {
    #[serde(default)]
    pub trust_all: bool,
    #[serde(default)]
    pub tools: Vec<ToolPolicy>,
    /// Terminator mode: auto-approve all tool requests without any prompts
    #[serde(default)]
    pub terminator_mode: bool,
}

impl ToolPermissionsConfig {
    /// Resolve the effective policy for a tool by title.
    ///
    /// An explicit per-tool policy is consulted FIRST and always wins — in
    /// particular an explicit `Deny` is honoured even under `trust_all` /
    /// `terminator_mode`. The blanket-allow modes only upgrade a tool that
    /// would otherwise be `Ask` (or has no recorded policy). This matches the
    /// contract in docs/TOOL_PERMISSIONS.md: "allow everything except explicit
    /// deny" — not "allow everything, period".
    pub fn resolve_policy(&self, tool_title: &str) -> PolicyKind {
        let explicit = self
            .tools
            .iter()
            .find(|t| t.title == tool_title)
            .map(|t| t.effective_policy());
        let blanket_allow = self.terminator_mode || self.trust_all;
        match explicit {
            Some(PolicyKind::Deny) => PolicyKind::Deny,
            Some(PolicyKind::Allow) => PolicyKind::Allow,
            _ if blanket_allow => PolicyKind::Allow,
            Some(p) => p,
            None => PolicyKind::Ask,
        }
    }
}

/// Per-tool permission policy. The frontend's UI exposes three states —
/// Always Ask, Allow, Deny — combined with a separate `grant_type` for
/// the duration of an Allow grant.
///
/// `#[serde(other)]` on `Ask` means an unknown wire value (e.g. a future
/// variant or a hand-edited config) collapses to "Ask" rather than
/// failing config load. Forward-compat without back-compat shims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PolicyKind {
    Allow,
    Deny,
    /// Default. Listed last so `#[serde(other)]` can land on it — that
    /// makes any unknown wire value (future variant, hand-edited
    /// config) collapse to "ask" and re-prompt the user, which is the
    /// safe-by-default behaviour.
    #[default]
    #[serde(other)]
    Ask,
}

impl PolicyKind {
    /// Wire-format string. Stable across releases.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

/// Duration of an Allow grant. `Hours24` serialises as `"24h"` because
/// that's the wire format the JS UI emits — `#[serde(rename)]` handles
/// the digit prefix that snake_case can't reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GrantType {
    /// One-shot grant — consumed after the next tool call.
    #[default]
    Once,
    /// Sliding 24-hour grant from `granted_at`.
    #[serde(rename = "24h")]
    Hours24,
    /// Persistent grant; re-prompts after 30 days of inactivity (see
    /// `effective_policy`'s staleness check).
    Always,
}

impl GrantType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Hours24 => "24h",
            Self::Always => "always",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPolicy {
    pub title: String,
    #[serde(default)]
    pub policy: PolicyKind,
    #[serde(default)]
    pub last_seen: String, // ISO 8601 — last time this tool was requested
    #[serde(default)]
    pub granted_at: String, // ISO 8601 — when the current grant was issued
    #[serde(default)]
    pub grant_type: GrantType,
}

fn default_config_version() -> u32 {
    config_migrations::CURRENT_VERSION
}

/// A user-approved capability grant for an installed extension.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExtensionGrant {
    /// Capabilities the user approved at install or upgrade time.
    /// See ui/js/shared/extension-permissions.js for the authoritative list.
    #[serde(default)]
    pub granted: Vec<String>,
    /// Version of the extension manifest at the time of approval. If the
    /// extension updates and requests a larger capability set, the runtime
    /// drops the new caps until the user re-approves.
    #[serde(default)]
    pub approved_version: String,
    /// ISO 8601 timestamp of the approval.
    #[serde(default)]
    pub approved_at: String,
}

impl ToolPolicy {
    /// Check if this tool's grant is still valid.
    /// Returns the effective policy considering expiry and staleness.
    pub fn effective_policy(&self) -> PolicyKind {
        match self.policy {
            PolicyKind::Deny => PolicyKind::Deny,
            PolicyKind::Ask => PolicyKind::Ask,
            PolicyKind::Allow => match self.grant_type {
                GrantType::Always => {
                    // Check 30-day staleness. If the stored timestamp is in the
                    // future (clock skew), treat the grant as suspicious and
                    // re-prompt rather than silently honouring it forever.
                    if let Ok(last) = chrono::DateTime::parse_from_rfc3339(&self.last_seen) {
                        let delta = chrono::Utc::now() - last.with_timezone(&chrono::Utc);
                        if delta < chrono::Duration::zero() || delta.num_days() > 30 {
                            return PolicyKind::Ask;
                        }
                    }
                    PolicyKind::Allow
                }
                GrantType::Hours24 => {
                    // Check if granted_at is within 24 hours AND not in the future.
                    // A negative delta would previously satisfy `hours < 24` and
                    // keep the permission indefinitely-granted whenever the clock
                    // was ever set forward and then corrected back.
                    if let Ok(granted) = chrono::DateTime::parse_from_rfc3339(&self.granted_at) {
                        let delta = chrono::Utc::now() - granted.with_timezone(&chrono::Utc);
                        if delta >= chrono::Duration::zero() && delta.num_hours() < 24 {
                            return PolicyKind::Allow;
                        }
                    }
                    PolicyKind::Ask // expired or future-dated
                }
                GrantType::Once => {
                    // "once" — already consumed, back to ask.
                    PolicyKind::Ask
                }
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreSource {
    pub name: String,
    pub url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Update channel. Resolved to a concrete endpoint URL by
/// `updater::endpoint_for_channel`. The `#[serde(other)]` fallback on
/// `Stable` means a stale / corrupted config or future-version variant
/// can't silently trap the user on a dead channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Beta,
    Dev,
    /// Default. Listed last so `#[serde(other)]` lands here — unknown
    /// wire values fall back to Stable rather than failing config load.
    #[serde(other)]
    Stable,
}

impl Default for Channel {
    fn default() -> Self {
        default_update_channel()
    }
}

impl Channel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Dev => "dev",
        }
    }

    /// Every defined channel, in display order. Surfaced to the
    /// settings UI so the dropdown is built from a single source.
    pub fn all() -> &'static [Channel] {
        &[Channel::Stable, Channel::Beta, Channel::Dev]
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateConfig {
    /// Automatically check for updates once per day
    #[serde(default)]
    pub auto_check: bool,
    /// Silently download and install updates when idle
    #[serde(default)]
    pub silent_update: bool,
    /// ISO 8601 timestamp of the last update check
    #[serde(default)]
    pub last_check_time: Option<String>,
    /// Version that was last installed via auto-update (to detect fresh updates)
    #[serde(default)]
    pub last_updated_version: Option<String>,
    /// Which release channel this install tracks.
    #[serde(default)]
    pub channel: Channel,
}

fn default_update_channel() -> Channel {
    // Dev builds embed "+dev." in the version (e.g. 0.9.202511171430+dev.abc1234),
    // beta builds embed "+beta.". Default new installs to the channel that
    // matches their build so the updater hits an endpoint that actually exists.
    let version = env!("CARGO_PKG_VERSION");
    if version.contains("+dev.") {
        Channel::Dev
    } else if version.contains("+beta.") {
        Channel::Beta
    } else {
        Channel::Stable
    }
}

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
    pub id: String,
    /// User-facing name (defaults to the preset display name).
    pub name: String,
    /// Optional preset id (e.g. "kiro", "claude-code", "codex").
    /// `None` means the connection was hand-rolled by the user.
    #[serde(default)]
    pub preset_id: Option<String>,
    /// Connection mode (Local spawn vs. Remote TCP).
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
    pub base_url: String,
    /// Tag-form model name (e.g. `llama3:8b`). Plumbed into the
    /// codex-acp adapter via `OPENAI_MODEL`.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_opacity")]
    pub floating_window_opacity: f32,
    #[serde(default = "default_chat_size")]
    pub chat_window_width: u32,
    #[serde(default = "default_chat_size")]
    pub chat_window_height: u32,
    #[serde(default)]
    pub chat_window_x: Option<i32>,
    #[serde(default)]
    pub chat_window_y: Option<i32>,
    #[serde(default = "default_true")]
    pub preserve_last_response: bool,
    #[serde(default = "default_window_start_position")]
    pub window_start_position: String,
    #[serde(default)]
    pub last_window_x: Option<i32>,
    #[serde(default)]
    pub last_window_y: Option<i32>,
    #[serde(default = "default_font_size")]
    pub font_size: u8,
    #[serde(default)]
    pub show_time: bool,
    #[serde(default)]
    pub show_date: bool,
    #[serde(default)]
    pub show_speech_button: bool,
    #[serde(default)]
    pub speech_read_back: bool,
    /// Show quick action chips on agent responses (translate, summarize, etc.)
    #[serde(default = "default_true")]
    pub show_response_actions: bool,
    /// Show attach file/image toolbar in the launcher
    #[serde(default)]
    pub show_floating_toolbar: bool,
    /// Remember the launcher window size after manual resize
    #[serde(default)]
    pub remember_launcher_size: bool,
    /// Saved launcher width (logical pixels)
    #[serde(default)]
    pub launcher_width: Option<u32>,
    /// Saved launcher height (logical pixels)
    #[serde(default)]
    pub launcher_height: Option<u32>,
    #[serde(default = "default_speech_silence_timeout")]
    pub speech_silence_timeout: f32,
    #[serde(default)]
    pub speech_voice: Option<String>,
    #[serde(default = "default_time_format")]
    pub time_format: String,
    #[serde(default = "default_date_format")]
    pub date_format: String,
    /// UI language code (e.g. "en", "ja", "ar"). When unset, falls back to
    /// the OS locale via `sys_locale::get_locale()`. The runtime catalog
    /// resolver then strips region tags ("en-GB" → "en") if no exact match
    /// is shipped. See `src/i18n.rs`.
    #[serde(default)]
    pub language: Option<String>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            floating_window_opacity: default_opacity(),
            chat_window_width: default_chat_size(),
            chat_window_height: default_chat_size(),
            chat_window_x: None,
            chat_window_y: None,
            preserve_last_response: true,
            window_start_position: default_window_start_position(),
            last_window_x: None,
            last_window_y: None,
            font_size: default_font_size(),
            show_time: false,
            show_date: false,
            show_speech_button: false,
            speech_read_back: false,
            show_response_actions: true,
            show_floating_toolbar: false,
            remember_launcher_size: false,
            launcher_width: None,
            launcher_height: None,
            speech_silence_timeout: default_speech_silence_timeout(),
            speech_voice: None,
            time_format: default_time_format(),
            date_format: default_date_format(),
            language: None,
        }
    }
}

fn default_theme() -> String {
    "system".to_string()
}

fn default_opacity() -> f32 {
    1.0
}

fn default_window_start_position() -> String {
    "center".to_string()
}

fn default_font_size() -> u8 {
    14
}

fn default_chat_size() -> u32 {
    0 // 0 means "use default / don't remember"
}

fn default_time_format() -> String {
    "HH:mm".to_string()
}

fn default_date_format() -> String {
    "ddd, MMM D".to_string()
}

fn default_true() -> bool {
    true
}

fn default_log_buffer_size() -> usize {
    1000
}

fn default_inline_assist_hotkey() -> Option<HotkeyConfig> {
    Some(HotkeyConfig {
        modifiers: vec!["Ctrl".to_string(), "Shift".to_string()],
        key: "Space".to_string(),
    })
}

fn default_speech_silence_timeout() -> f32 {
    2.0
}

fn default_auto_compact_threshold() -> u32 {
    90
}

/// Default blocklist of processes where auto-copy would be disruptive.
/// Terminals are the big one — Ctrl+C is overloaded with SIGINT, and even
/// Windows Terminal's "copy-if-selection-else-interrupt" mapping trips on
/// some edge cases. Users can extend/replace this list in settings.
fn default_capture_selection_blocklist() -> Vec<String> {
    vec![
        "cmd".to_string(),
        "powershell".to_string(),
        "pwsh".to_string(),
        "conhost".to_string(),
        "WindowsTerminal".to_string(),
        "wsl".to_string(),
        "bash".to_string(),
        "alacritty".to_string(),
        "wezterm-gui".to_string(),
        "Terminal".to_string(), // macOS Terminal.app
        "iTerm2".to_string(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemConfig {
    #[serde(default)]
    pub auto_start: bool,
    /// Capture selected text from the active window when the hotkey is pressed.
    #[serde(default = "default_true")]
    pub capture_selection: bool,
    /// Process names (no extension) to skip selection capture for. When the
    /// foreground window belongs to one of these, Kage won't inject the
    /// Ctrl+C / Cmd+C keystroke — matters most for terminals where Ctrl+C
    /// also means SIGINT and can cancel in-progress commands even when
    /// text is highlighted. Matching is case-insensitive; an optional
    /// trailing ".exe" on Windows is ignored.
    #[serde(default = "default_capture_selection_blocklist")]
    pub capture_selection_blocklist: Vec<String>,
    /// Show system notifications when responses complete while hidden.
    #[serde(default = "default_true")]
    pub show_notifications: bool,
    /// Include the source window context (app name, title) when sending messages.
    #[serde(default = "default_true")]
    pub screen_context: bool,
    /// Maximum number of app log entries to keep in the ring buffer.
    #[serde(default = "default_log_buffer_size")]
    pub log_buffer_size: usize,
    /// Mirror every frontend `console.log` / `console.debug` to the app log.
    /// Off by default — only `console.warn` / `console.error` are forwarded.
    /// Enable for verbose troubleshooting; the setting is heavy on IPC and
    /// disk I/O so it's not suitable for steady-state use.
    #[serde(default)]
    pub verbose_frontend_logging: bool,
    /// Log the full text of chat prompts (and other message content) to
    /// app.jsonl. OFF by default: app.jsonl is routinely attached to bug
    /// reports, so message content must never land there unless the user
    /// explicitly opts in. Only useful when developing/debugging Kage
    /// itself; the default path logs message length only.
    #[serde(default)]
    pub log_message_content: bool,
    /// Header timestamp of the most recent crash the user has been
    /// shown the recovery dialog for. Used by `crash_recovery` to
    /// suppress repeated dialogs for the same crash across launches.
    /// Stored as the literal `=== Kage crash report @ <ts>` value so
    /// string-equality is enough — no time-zone parsing.
    #[serde(default)]
    pub last_seen_crash_timestamp: Option<String>,
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            auto_start: false,
            capture_selection: true,
            capture_selection_blocklist: default_capture_selection_blocklist(),
            show_notifications: true,
            screen_context: true,
            log_buffer_size: default_log_buffer_size(),
            verbose_frontend_logging: false,
            log_message_content: false,
            last_seen_crash_timestamp: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickActionsConfig {
    /// Enable quick action chips when text is selected
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Default language for the Translate action (e.g., "English", "Spanish")
    #[serde(default)]
    pub translate_language: Option<String>,
    /// Custom actions (shown in addition to smart defaults)
    #[serde(default)]
    pub custom_actions: Vec<QuickAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickAction {
    /// Display label on the chip
    pub label: String,
    /// Emoji icon for the chip
    #[serde(default)]
    pub icon: String,
    /// Prompt template — {text} is replaced with the selected text
    pub prompt: String,
    /// Optional: only show for specific content types (code, prose, error, url, json, math)
    /// Empty means show for all types.
    #[serde(default)]
    pub content_types: Vec<String>,
}

impl Default for QuickActionsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            translate_language: None,
            custom_actions: vec![],
        }
    }
}

/// A macro/automation is a named sequence of transformation steps with an optional trigger.
/// Each step's output feeds into the next step's {input} placeholder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroConfig {
    /// Display name
    pub name: String,
    /// Emoji icon
    #[serde(default = "default_macro_icon")]
    pub icon: String,
    /// Ordered list of transformation steps
    pub steps: Vec<MacroStep>,
    /// What to do with the final output: "clipboard" or "replace" or "inform"
    #[serde(default = "default_macro_output")]
    pub output: String,
    /// How this automation is triggered (default: manual only)
    #[serde(default)]
    pub trigger: AutomationTrigger,
    /// Whether this automation is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// AI-generated summary of what this automation does
    #[serde(default)]
    pub summary: Option<String>,
}

/// How an automation is triggered.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AutomationTrigger {
    /// Only runs via inline assist / quick actions (current behavior)
    #[default]
    #[serde(rename = "manual")]
    Manual,
    /// Runs on a time-based schedule
    #[serde(rename = "schedule")]
    Schedule {
        /// Cron-like interval: "every_5m", "every_1h", "daily_09:00", "weekdays_09:00"
        #[serde(default)]
        interval: String,
        /// Last execution timestamp (ISO 8601)
        #[serde(default)]
        last_run: Option<String>,
    },
    /// Runs in response to a named signal from an extension or the system
    #[serde(rename = "signal")]
    Signal {
        /// Signal name, e.g. "calendar:meeting_starting", "todos:item_due", "system:clipboard_change"
        #[serde(default)]
        signal: String,
        /// Optional filter (extension-defined, e.g. subject contains "standup")
        #[serde(default)]
        filter: Option<String>,
    },
}

/// Power/battery awareness settings for automations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationPowerConfig {
    /// How to handle power: "auto" (detect battery), "full" (always run), "saving" (always throttle)
    #[serde(default = "default_power_mode")]
    pub mode: String,
    /// Multiplier for schedule intervals when on battery (e.g. 2.0 = run half as often)
    #[serde(default = "default_battery_multiplier")]
    pub battery_multiplier: f32,
    /// Multiplier when battery is low (< 20%)
    #[serde(default = "default_low_battery_multiplier")]
    pub low_battery_multiplier: f32,
    /// Disable signal-triggered automations entirely on low battery
    #[serde(default)]
    pub disable_signals_on_low_battery: bool,
}

impl Default for AutomationPowerConfig {
    fn default() -> Self {
        AutomationPowerConfig {
            mode: "auto".to_string(),
            battery_multiplier: 2.0,
            low_battery_multiplier: 4.0,
            disable_signals_on_low_battery: false,
        }
    }
}

fn default_power_mode() -> String {
    "auto".to_string()
}
fn default_battery_multiplier() -> f32 {
    2.0
}
fn default_low_battery_multiplier() -> f32 {
    4.0
}

/// What a macro step does. Exec'd by `execute_macro` (which works on
/// raw JSON for legacy reasons) and surfaced in the settings UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MacroStepKind {
    /// Run the prompt template through the agent, replacing `{input}`
    /// with the previous step's output. The default for new steps.
    #[default]
    AiPrompt,
    FindReplace,
    Transform,
    Condition,
    Script,
    /// Forward-compat: a future variant in the config maps to this so
    /// load doesn't fail. The settings UI shows a warning chip and the
    /// runtime treats unknown steps as no-ops.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroStep {
    #[serde(default)]
    pub step_type: MacroStepKind,
    /// Prompt template for ai_prompt — {input} is replaced with the previous step's output
    #[serde(default)]
    pub prompt: String,
    /// For find_replace: regex pattern to find
    #[serde(default)]
    pub find: String,
    /// For find_replace: replacement string
    #[serde(default)]
    pub replace: String,
    /// For transform: built-in transform name
    #[serde(default)]
    pub transform: String,
    /// For condition: text that must be present in the previous output to continue
    #[serde(default)]
    pub condition: String,
    /// For script: JS function body (receives `input` variable, must return a string)
    #[serde(default)]
    pub script: String,
}

fn default_macro_icon() -> String {
    "🔄".to_string()
}
fn default_macro_output() -> String {
    "clipboard".to_string()
}

/// What kind of action a user-defined shortcut performs.
/// Surfaced in Settings → Shortcuts; the frontend dispatches on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutActionKind {
    #[default]
    RunProgram,
    OpenUrl,
    Prompt,
    Text,
    Script,
    /// Forward-compat fallback for a future variant.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutConfig {
    pub name: String,
    pub shortcut: String,
    #[serde(default)]
    pub action_type: ShortcutActionKind,
    #[serde(default)]
    pub icon: Option<String>, // Emoji or base64 data URI (png/jpg)
    #[serde(default)]
    pub path: Option<String>, // For run_program
    #[serde(default)]
    pub url: Option<String>, // For open_url
    #[serde(default)]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>, // For prompt action type — template sent to agent
    #[serde(default)]
    pub script: Option<String>, // For script action type — JS function body
    #[serde(default)]
    pub script_action: Option<String>, // What to do with script result: "run_program", "open_url", "prompt", "text"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PocketTtsConfig {
    /// Enable pocket-tts as the TTS engine (instead of browser speechSynthesis)
    #[serde(default)]
    pub enabled: bool,
    /// Voice to use (built-in: alba, marius, javert, jean, fantine, cosette, eponine, azelma)
    #[serde(default = "default_pocket_tts_voice")]
    pub voice: String,
    /// Port for the pocket-tts HTTP server
    #[serde(default = "default_pocket_tts_port")]
    pub port: u16,
    /// Path to Python executable (auto-detected if empty)
    #[serde(default)]
    pub python_path: Option<String>,
    /// Whether pocket-tts pip package is installed
    #[serde(default)]
    pub installed: bool,
    /// Auto-start the TTS server when the app launches
    #[serde(default)]
    pub auto_start: bool,
    /// Sampling temperature (0.3=consistent, 0.7=default, 1.0=expressive)
    #[serde(default = "default_pocket_tts_temp")]
    pub temp: f32,
    /// End-of-sequence threshold (default: -4.0, lower = less likely to stop early)
    #[serde(default = "default_pocket_tts_eos_threshold")]
    pub eos_threshold: f32,
}

fn default_pocket_tts_voice() -> String {
    "alba".to_string()
}

fn default_pocket_tts_port() -> u16 {
    9877
}

fn default_pocket_tts_temp() -> f32 {
    0.7
}

fn default_pocket_tts_eos_threshold() -> f32 {
    -4.0
}

impl Default for PocketTtsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            voice: "alba".to_string(),
            port: 9877,
            python_path: None,
            installed: false,
            auto_start: false,
            temp: 0.7,
            eos_threshold: -4.0,
        }
    }
}

/// Anonymous product analytics configuration.
///
/// We collect minimum viable telemetry through Aptabase: a randomly-generated
/// install ID, app version, OS/locale, and feature-usage event names. No
/// prompts, file paths, clipboard contents, or PII. See docs/PRIVACY.md for
/// the full disclosure.
///
/// Defaults:
///  - `enabled`: `true`. Opt-out with clear disclosure on the welcome screen
///    and a toggle in Settings → Privacy. Kept simple for now — if the build
///    was produced without an APTABASE_KEY the plugin is a no-op anyway, so
///    this flag only matters for distribution builds.
///  - `install_id`: generated lazily on first use (not here) so resetting it
///    via Settings actually changes the ID sent to Aptabase.
///  - `consent_version`: bumped whenever the privacy policy materially
///    changes. The UI compares this to the current policy version and
///    re-prompts if it lags behind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    /// Whether to send anonymous usage events. Respected by every call site
    /// through `telemetry::track()`, which short-circuits when false.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Anonymous UUID generated on first consent. Not linked to any account
    /// or device fingerprint — the user can reset it from Settings at any
    /// time, which orphans all prior events for that install.
    #[serde(default)]
    pub install_id: Option<String>,
    /// Version of the privacy policy the user last consented to. If the
    /// current `PRIVACY_POLICY_VERSION` exceeds this, we re-prompt.
    #[serde(default)]
    pub consent_version: u32,
    /// ISO 8601 date (YYYY-MM-DD) of the last `app_daily_active` event. Used
    /// to throttle that event to once per UTC day per install so DAU counts
    /// aren't skewed by users who open/close the app many times.
    #[serde(default)]
    pub last_daily_ping: Option<String>,
    /// The app version that last fired `app_started`. Used to detect upgrades
    /// (fire `app_upgraded` when this differs from the current version) and
    /// first installs (fire `app_installed` when this is `None`).
    #[serde(default)]
    pub last_seen_version: Option<String>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            install_id: None,
            consent_version: 0,
            last_daily_ping: None,
            last_seen_version: None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: config_migrations::CURRENT_VERSION,
            hotkey: HotkeyConfig::default(),
            acp: AcpConfig::default(),
            ui: UiConfig::default(),
            system: SystemConfig::default(),
            shortcuts: vec![],
            debug_mode: false,
            tool_permissions: ToolPermissionsConfig::default(),
            first_run_completed: false,
            updates: UpdateConfig::default(),
            quick_actions: QuickActionsConfig::default(),
            extensions: HashMap::new(),
            extension_states: HashMap::new(),
            extension_grants: HashMap::new(),
            pocket_tts: PocketTtsConfig::default(),
            clipboard_hotkey: None,
            inline_assist_hotkey: Some(HotkeyConfig {
                modifiers: vec!["Ctrl".to_string(), "Shift".to_string()],
                key: "Space".to_string(),
            }),
            voice_hotkey: None,
            store_url: None,
            store_sources: Vec::new(),
            mcp_config_path: None,
            auto_update_extensions: false,
            last_extension_update_check: None,
            macros: vec![],
            automation_power: AutomationPowerConfig::default(),
            telemetry: TelemetryConfig::default(),
            context_rules: crate::context_rules::default_starter_rules(),
        }
    }
}

impl Config {
    /// Maximum config file size (1 MB). Anything larger is likely corrupted.
    const MAX_CONFIG_SIZE: u64 = 1024 * 1024;

    pub fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;

        if !config_path.exists() {
            let config = Self::default();
            config.save()?;
            return Ok(config);
        }

        let metadata = fs::metadata(&config_path).context("Failed to read config file metadata")?;
        if metadata.len() > Self::MAX_CONFIG_SIZE {
            // Too-large config is almost certainly corrupted (maybe a
            // truncated write that got padded, or a log file written to
            // the wrong place). Back it up and reset rather than
            // refusing to start — the user's session can continue.
            log::warn!(
                "Config file is {} bytes (max {}); treating as corrupt",
                metadata.len(),
                Self::MAX_CONFIG_SIZE
            );
            Self::backup_corrupt(&config_path, "oversized");
            let config = Self::default();
            config.save()?;
            return Ok(config);
        }

        let content = fs::read_to_string(&config_path).context("Failed to read config file")?;

        // Parse to a generic Value first so we can run migrations on the
        // JSON representation before it hits the strongly-typed struct.
        // This means a field rename or restructure in a migration doesn't
        // have to also pass through the current struct's shape.
        let raw: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                log::warn!(
                    "Config file is not valid JSON ({}); backing up and resetting",
                    e
                );
                Self::backup_corrupt(&config_path, "invalid-json");
                let config = Self::default();
                config.save()?;
                return Ok(config);
            }
        };

        let migrated = match config_migrations::migrate(raw) {
            Ok(v) => v,
            Err(e) => {
                // Two cases land here:
                //   1. Version is newer than we understand — preserve the
                //      file, start with defaults *without* overwriting.
                //   2. Version is too old to migrate — back up and reset.
                let msg = format!("{}", e);
                if msg.contains("newer") {
                    log::warn!(
                        "Config is from a newer build ({}); running with defaults without overwriting the file",
                        e
                    );
                    return Ok(Self::default());
                }
                log::warn!("Config migration failed ({}); backing up and resetting", e);
                Self::backup_corrupt(&config_path, "migration-failed");
                let config = Self::default();
                config.save()?;
                return Ok(config);
            }
        };

        let config: Config = match serde_json::from_value(migrated) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("Post-migration config did not match current schema ({}); backing up and resetting", e);
                Self::backup_corrupt(&config_path, "schema-mismatch");
                let config = Self::default();
                config.save()?;
                return Ok(config);
            }
        };

        // If migrations bumped the version, persist the upgrade so we
        // don't rerun them every launch.
        if config.version < config_migrations::CURRENT_VERSION {
            let mut upgraded = config.clone();
            upgraded.version = config_migrations::CURRENT_VERSION;
            let _ = upgraded.save();
            return Ok(upgraded);
        }

        Ok(config)
    }

    /// Copy a bad config file aside so the user can inspect it later.
    /// Best-effort: failure to back up does not block the reset path.
    fn backup_corrupt(path: &std::path::Path, reason: &str) {
        let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S");
        let backup = path.with_extension(format!("json.corrupt-{}-{}.bak", reason, ts));
        if let Err(e) = fs::copy(path, &backup) {
            log::warn!("Failed to back up corrupt config to {:?}: {}", backup, e);
        } else {
            log::info!("Backed up corrupt config to {:?}", backup);
        }
    }

    /// Persist the config atomically: write to a sibling temp file in the
    /// same directory, then rename over the destination. fs::rename is
    /// atomic on POSIX and uses MoveFileExW with REPLACE_EXISTING on Windows
    /// (effectively atomic for same-volume moves on NTFS), so a crash during
    /// the write leaves either the old config intact or the new one fully
    /// in place — never a half-written file. Tool permission policies,
    /// hotkeys, and grants live in this file; truncating it via plain
    /// fs::write meant a poorly-timed crash could lose all of them.
    pub fn save(&self) -> Result<()> {
        let config_path = Self::get_config_path()?;
        Self::save_to(self, &config_path)
    }

    /// Inner save — exposed so tests can drive the atomic-write logic
    /// against a temp path without depending on the user's config dir.
    pub fn save_to(&self, config_path: &std::path::Path) -> Result<()> {
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent).context("Failed to create config directory")?;
        }

        let content = serde_json::to_string_pretty(self).context("Failed to serialize config")?;

        // Sibling temp file so the rename is same-volume (cross-volume
        // renames degrade to copy+delete, which loses atomicity). Include
        // the PID so concurrent processes can't collide on the temp path.
        let tmp_path = config_path.with_extension(format!("json.tmp.{}", std::process::id()));

        // Write + flush, then close (drop) the file before renaming —
        // Windows refuses to rename over an open handle.
        {
            use std::io::Write;
            let mut f = fs::File::create(&tmp_path)
                .with_context(|| format!("Failed to create temp config at {:?}", tmp_path))?;
            f.write_all(content.as_bytes())
                .context("Failed to write temp config")?;
            f.sync_all()
                .context("Failed to flush temp config to disk")?;
        }

        if let Err(e) = fs::rename(&tmp_path, config_path) {
            // Best-effort cleanup so the temp file doesn't accumulate.
            let _ = fs::remove_file(&tmp_path);
            return Err(e).context("Failed to atomically replace config file");
        }

        Ok(())
    }

    pub fn get_config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir().context("Failed to get config directory")?;

        Ok(config_dir.join("kage").join("config.json"))
    }

    pub fn get_hotkey_string(&self) -> String {
        let mut parts = self.hotkey.modifiers.clone();
        parts.push(self.hotkey.key.clone());
        parts.join("+")
    }

    pub fn get_clipboard_hotkey_string(&self) -> Option<String> {
        self.clipboard_hotkey.as_ref().map(|hk| {
            let mut parts = hk.modifiers.clone();
            parts.push(hk.key.clone());
            parts.join("+")
        })
    }

    pub fn get_inline_assist_hotkey_string(&self) -> Option<String> {
        self.inline_assist_hotkey.as_ref().map(|hk| {
            let mut parts = hk.modifiers.clone();
            parts.push(hk.key.clone());
            parts.join("+")
        })
    }

    pub fn get_voice_hotkey_string(&self) -> Option<String> {
        self.voice_hotkey.as_ref().map(|hk| {
            let mut parts = hk.modifiers.clone();
            parts.push(hk.key.clone());
            parts.join("+")
        })
    }

    /// Get the path to the auto-generated steering document
    pub fn get_auto_steering_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir().context("Failed to get config directory")?;
        Ok(config_dir.join("kage").join("auto-steering.md"))
    }
}

#[cfg(test)]
mod enum_tests {
    //! Wire-format guarantees for the typed config enums. The values
    //! here are the contract with both saved configs on disk and the
    //! frontend — drift on either side silently breaks tool-permission
    //! display, update channel routing, etc.

    use super::*;

    #[test]
    fn policy_kind_serialises_as_snake_case() {
        // Wire format: "ask" | "allow" | "deny". Anything else and the
        // settings UI's `tool.policy === 'allow'` checks miss.
        assert_eq!(serde_json::to_string(&PolicyKind::Ask).unwrap(), "\"ask\"");
        assert_eq!(
            serde_json::to_string(&PolicyKind::Allow).unwrap(),
            "\"allow\""
        );
        assert_eq!(
            serde_json::to_string(&PolicyKind::Deny).unwrap(),
            "\"deny\""
        );
    }

    #[test]
    fn policy_kind_unknown_falls_back_to_ask() {
        // Forward-compat: a value the current build doesn't recognise
        // (a future variant, hand-edited config) collapses to Ask. That's
        // the safe default — we re-prompt the user rather than silently
        // honouring something we don't understand.
        let p: PolicyKind = serde_json::from_str("\"some_future_variant\"").unwrap();
        assert_eq!(p, PolicyKind::Ask);
    }

    #[test]
    fn grant_type_serialises_24h_correctly() {
        // The "24h" wire value can't come from snake_case alone — the
        // digit prefix needs an explicit serde rename. If this regresses,
        // settings → "Allow 24h" silently shows the wrong selection.
        assert_eq!(serde_json::to_string(&GrantType::Once).unwrap(), "\"once\"");
        assert_eq!(
            serde_json::to_string(&GrantType::Hours24).unwrap(),
            "\"24h\""
        );
        assert_eq!(
            serde_json::to_string(&GrantType::Always).unwrap(),
            "\"always\""
        );
    }

    #[test]
    fn grant_type_round_trips() {
        for &gt in &[GrantType::Once, GrantType::Hours24, GrantType::Always] {
            let s = serde_json::to_string(&gt).unwrap();
            let back: GrantType = serde_json::from_str(&s).unwrap();
            assert_eq!(back, gt);
        }
    }

    #[test]
    fn channel_unknown_falls_back_to_stable() {
        // A user editing config.json or upgrading from a build with a
        // since-removed channel must not get stuck. Matches the old
        // `normalize_channel` behaviour.
        let c: Channel = serde_json::from_str("\"experimental\"").unwrap();
        assert_eq!(c, Channel::Stable);
    }

    #[test]
    fn channel_known_values_round_trip() {
        for &c in &[Channel::Stable, Channel::Beta, Channel::Dev] {
            let s = serde_json::to_string(&c).unwrap();
            let back: Channel = serde_json::from_str(&s).unwrap();
            assert_eq!(back, c);
        }
    }

    #[test]
    fn channel_as_str_matches_wire_format() {
        // The integrations command exposes Channel::as_str() to JS via
        // get_app_info's `update_channels` array. The dropdown's value
        // attribute must equal the JSON serialisation.
        for &c in Channel::all() {
            let json = serde_json::to_string(&c).unwrap();
            // strip surrounding quotes from JSON string
            let stripped = json.trim_matches('"');
            assert_eq!(stripped, c.as_str(), "{:?}", c);
        }
    }

    #[test]
    fn macro_step_kind_unknown_falls_back_to_unknown() {
        // Future variants in saved configs must not block load. The
        // `Unknown` variant is the dedicated catch-all so the settings
        // UI can show a "this step type isn't supported in this build"
        // chip rather than silently dropping the step.
        let k: MacroStepKind = serde_json::from_str("\"future_step\"").unwrap();
        assert_eq!(k, MacroStepKind::Unknown);
        // Known variants still parse:
        let k: MacroStepKind = serde_json::from_str("\"ai_prompt\"").unwrap();
        assert_eq!(k, MacroStepKind::AiPrompt);
        let k: MacroStepKind = serde_json::from_str("\"find_replace\"").unwrap();
        assert_eq!(k, MacroStepKind::FindReplace);
    }

    #[test]
    fn shortcut_action_kind_unknown_falls_back_to_unknown() {
        let k: ShortcutActionKind = serde_json::from_str("\"future_action\"").unwrap();
        assert_eq!(k, ShortcutActionKind::Unknown);
        let k: ShortcutActionKind = serde_json::from_str("\"run_program\"").unwrap();
        assert_eq!(k, ShortcutActionKind::RunProgram);
    }

    #[test]
    fn tool_policy_loads_with_defaults_for_missing_fields() {
        // Old configs (or partial JSON from a buggy save) must round-trip:
        // missing `policy` / `grant_type` get the type's Default impl.
        let json = r#"{"title":"shell"}"#;
        let p: ToolPolicy = serde_json::from_str(json).unwrap();
        assert_eq!(p.policy, PolicyKind::Ask);
        assert_eq!(p.grant_type, GrantType::Once);
    }
}

#[cfg(test)]
mod resolve_policy_tests {
    use super::*;

    fn tool(title: &str, policy: PolicyKind) -> ToolPolicy {
        ToolPolicy {
            title: title.to_string(),
            policy,
            // Fresh timestamp + Always so an Allow resolves to Allow (a Once
            // grant is "already consumed" → Ask, which would muddy these tests).
            last_seen: chrono::Utc::now().to_rfc3339(),
            granted_at: chrono::Utc::now().to_rfc3339(),
            grant_type: GrantType::Always,
        }
    }

    #[test]
    fn explicit_deny_wins_over_trust_all() {
        // The whole point of the fix: a user who trusts everything but
        // explicitly denied one dangerous tool must still have it denied.
        let cfg = ToolPermissionsConfig {
            trust_all: true,
            terminator_mode: false,
            tools: vec![tool("rm_rf", PolicyKind::Deny)],
        };
        assert_eq!(cfg.resolve_policy("rm_rf"), PolicyKind::Deny);
    }

    #[test]
    fn explicit_deny_wins_over_terminator_mode() {
        let cfg = ToolPermissionsConfig {
            trust_all: false,
            terminator_mode: true,
            tools: vec![tool("rm_rf", PolicyKind::Deny)],
        };
        assert_eq!(cfg.resolve_policy("rm_rf"), PolicyKind::Deny);
    }

    #[test]
    fn trust_all_upgrades_ask_and_unknown_tools() {
        let cfg = ToolPermissionsConfig {
            trust_all: true,
            terminator_mode: false,
            tools: vec![tool("known", PolicyKind::Ask)],
        };
        assert_eq!(cfg.resolve_policy("known"), PolicyKind::Allow);
        assert_eq!(cfg.resolve_policy("never_seen"), PolicyKind::Allow);
    }

    #[test]
    fn without_blanket_modes_policy_is_per_tool() {
        let cfg = ToolPermissionsConfig {
            trust_all: false,
            terminator_mode: false,
            tools: vec![tool("a", PolicyKind::Allow), tool("d", PolicyKind::Deny)],
        };
        assert_eq!(cfg.resolve_policy("a"), PolicyKind::Allow);
        assert_eq!(cfg.resolve_policy("d"), PolicyKind::Deny);
        // Unknown tool with no blanket mode → Ask.
        assert_eq!(cfg.resolve_policy("unknown"), PolicyKind::Ask);
    }
}

#[cfg(test)]
mod partial_config_tests {
    //! A config file missing top-level sections must still deserialize —
    //! every top-level field carries `#[serde(default)]`. Without it, an
    //! old or partially-written config that omitted (say) `hotkey` failed
    //! deserialization and triggered the full backup-and-reset path in
    //! `Config::load`, wiping tool grants, hotkeys, and extension state for
    //! want of one section.
    use super::*;

    #[test]
    fn config_missing_top_level_sections_uses_defaults() {
        // Only `version` present — every other section absent.
        let cfg: Config = serde_json::from_str(r#"{ "version": 1 }"#)
            .expect("a config missing hotkey/acp/ui/system must still deserialize");
        // Defaults must match Config::default(), not the zero-value derive.
        assert_eq!(cfg.hotkey.modifiers, vec!["Alt".to_string()]);
        assert_eq!(cfg.hotkey.key, "Space");
        assert_eq!(cfg.ui.theme, "system");
        assert_eq!(cfg.ui.floating_window_opacity, 1.0);
        assert!(cfg.system.capture_selection);
        assert!(!cfg.system.auto_start);
        assert!(!cfg.acp.connections.is_empty());
    }

    #[test]
    fn empty_object_config_deserializes() {
        // The most degenerate case: `{}`. Should be equivalent to defaults.
        let cfg: Config =
            serde_json::from_str("{}").expect("an empty-object config must deserialize");
        assert_eq!(cfg.hotkey.key, "Space");
        assert_eq!(cfg.ui.font_size, 14);
    }
}
