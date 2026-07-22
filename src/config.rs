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

mod automations;
mod connections;
mod permissions;
mod speech;
mod ui;
mod updates;

pub use automations::*;
pub use connections::*;
pub use permissions::*;
pub use speech::*;
pub use ui::*;
pub use updates::*;

fn default_config_version() -> u32 {
    config_migrations::CURRENT_VERSION
}

/// Shared `#[serde(default = "...")]` helper for boolean fields that
/// default to on. Single copy — submodules import it via `use super::
/// default_true;` (serde resolves the string as a path in the field's
/// module scope).
pub(crate) fn default_true() -> bool {
    true
}

fn default_inline_assist_hotkey() -> Option<HotkeyConfig> {
    Some(HotkeyConfig {
        modifiers: vec!["Ctrl".to_string(), "Shift".to_string()],
        key: "Space".to_string(),
    })
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

    #[test]
    fn nested_config_entries_missing_fields_use_defaults() {
        // List-entry structs must ALSO tolerate `{}`: one hand-edited or
        // old-shape entry inside connections[] / store sources used to
        // fail the entire Config::load and reset the user to defaults.
        let conn: AgentConnection =
            serde_json::from_str("{}").expect("empty AgentConnection must deserialize");
        assert!(conn.id.is_empty());
        assert!(conn.name.is_empty());
        assert!(
            matches!(conn.mode, AcpMode::Local { ref spawn_command } if spawn_command.is_empty())
        );

        let ollama: OllamaConnectionSettings =
            serde_json::from_str("{}").expect("empty OllamaConnectionSettings must deserialize");
        assert_eq!(ollama.base_url, crate::ollama::DEFAULT_BASE_URL);
        assert!(ollama.model.is_empty());
        assert!(!ollama.show_status_widget);

        let source: StoreSource =
            serde_json::from_str("{}").expect("empty StoreSource must deserialize");
        assert!(source.name.is_empty());
        assert!(source.url.is_empty());
        assert!(source.enabled);
    }

    #[test]
    fn old_shape_connection_round_trips() {
        // A pre-ollama_settings, pre-sessions_directory connection entry
        // (the shape shipped before the connections split) must load and
        // re-serialize without losing the fields it does carry.
        let old = r#"{
            "id": "abc-123",
            "name": "Kiro",
            "mode": { "type": "local", "spawn_command": "kiro-cli acp" }
        }"#;
        let conn: AgentConnection =
            serde_json::from_str(old).expect("old-shape connection must deserialize");
        assert_eq!(conn.id, "abc-123");
        assert_eq!(conn.name, "Kiro");
        assert!(
            matches!(conn.mode, AcpMode::Local { ref spawn_command } if spawn_command == "kiro-cli acp")
        );
        assert!(conn.preset_id.is_none());
        assert!(conn.ollama_settings.is_none());

        let json = serde_json::to_string(&conn).expect("serialize");
        let back: AgentConnection = serde_json::from_str(&json).expect("round-trip");
        assert_eq!(back.id, conn.id);
        assert_eq!(back.name, conn.name);
        assert_eq!(back.mode, conn.mode);
    }
}
