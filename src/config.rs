use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub hotkey: HotkeyConfig,
    pub acp: AcpConfig,
    pub ui: UiConfig,
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
    /// Custom path to mcp.json. If empty, uses ~/.kage/settings/mcp.json.
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct ToolPermissionsConfig {
    #[serde(default)]
    pub trust_all: bool,
    #[serde(default)]
    pub tools: Vec<ToolPolicy>,
}

/// Per-tool permission policy: "ask", "allow", or "deny"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPolicy {
    pub title: String,
    #[serde(default = "default_policy")]
    pub policy: String, // "ask", "allow", "deny"
    #[serde(default)]
    pub last_seen: String, // ISO 8601 timestamp
}

fn default_policy() -> String {
    "ask".to_string()
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreSource {
    pub name: String,
    pub url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
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
            sessions_directory: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyConfig {
    pub modifiers: Vec<String>,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpConfig {
    pub mode: AcpMode,
    #[serde(default)]
    pub agent: AgentConfig,
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
    /// Custom sessions directory. If unset, auto-detected from spawn_command
    /// (kiro-cli → ~/.kiro/sessions/cli, kage-cli → ~/.kage/sessions/cli).
    #[serde(default)]
    pub sessions_directory: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

fn default_theme() -> String {
    "system".to_string()
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemConfig {
    pub auto_start: bool,
    /// Capture selected text from the active window when the hotkey is pressed.
    #[serde(default = "default_true")]
    pub capture_selection: bool,
    /// Show system notifications when responses complete while hidden.
    #[serde(default = "default_true")]
    pub show_notifications: bool,
    /// Include the source window context (app name, title) when sending messages.
    #[serde(default = "default_true")]
    pub screen_context: bool,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AutomationTrigger {
    /// Only runs via inline assist / quick actions (current behavior)
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

impl Default for AutomationTrigger {
    fn default() -> Self { AutomationTrigger::Manual }
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

fn default_power_mode() -> String { "auto".to_string() }
fn default_battery_multiplier() -> f32 { 2.0 }
fn default_low_battery_multiplier() -> f32 { 4.0 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroStep {
    /// Step type: "ai_prompt", "find_replace", "transform", "condition", "script"
    #[serde(default = "default_step_type")]
    pub step_type: String,
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

fn default_step_type() -> String { "ai_prompt".to_string() }

fn default_macro_icon() -> String { "🔄".to_string() }
fn default_macro_output() -> String { "clipboard".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutConfig {
    pub name: String,
    pub shortcut: String,
    #[serde(default = "default_action_type")]
    pub action_type: String, // "run_program", "open_url", "prompt", "text", "script"
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

fn default_action_type() -> String {
    "run_program".to_string()
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

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            hotkey: HotkeyConfig {
                modifiers: vec!["Alt".to_string()],
                key: "Space".to_string(),
            },
            acp: AcpConfig {
                mode: AcpMode::Remote {
                    host: "127.0.0.1".to_string(),
                    port: 8765,
                    timeout_ms: 30000,
                },
                agent: AgentConfig::default(),
            },
            ui: UiConfig {
                theme: "system".to_string(),
                floating_window_opacity: 1.0,
                chat_window_width: 0,
                chat_window_height: 0,
                chat_window_x: None,
                chat_window_y: None,
                preserve_last_response: true,
                window_start_position: "center".to_string(),
                last_window_x: None,
                last_window_y: None,
                font_size: 14,
                show_time: false,
                show_date: false,
                show_speech_button: false,
                speech_read_back: false,
                show_response_actions: true,
                show_floating_toolbar: false,
                remember_launcher_size: false,
                launcher_width: None,
                launcher_height: None,
                speech_silence_timeout: 2.0,
                speech_voice: None,
                time_format: "HH:mm".to_string(),
                date_format: "ddd, MMM D".to_string(),
            },
            system: SystemConfig {
                auto_start: false,
                capture_selection: true,
                show_notifications: true,
                screen_context: true,
            },
            shortcuts: vec![],
            debug_mode: false,
            tool_permissions: ToolPermissionsConfig::default(),
            first_run_completed: false,
            updates: UpdateConfig::default(),
            quick_actions: QuickActionsConfig::default(),
            extensions: HashMap::new(),
            extension_states: HashMap::new(),
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

        let metadata = fs::metadata(&config_path)
            .context("Failed to read config file metadata")?;
        if metadata.len() > Self::MAX_CONFIG_SIZE {
            anyhow::bail!(
                "Config file is too large ({} bytes, max {}). It may be corrupted.",
                metadata.len(),
                Self::MAX_CONFIG_SIZE
            );
        }
        
        let content = fs::read_to_string(&config_path)
            .context("Failed to read config file")?;
        
        let config: Config = serde_json::from_str(&content)
            .context("Failed to parse config file")?;
        
        Ok(config)
    }
    
    pub fn save(&self) -> Result<()> {
        let config_path = Self::get_config_path()?;
        
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)
                .context("Failed to create config directory")?;
        }
        
        let content = serde_json::to_string_pretty(self)
            .context("Failed to serialize config")?;
        
        fs::write(&config_path, content)
            .context("Failed to write config file")?;
        
        Ok(())
    }
    
    pub fn get_config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Failed to get config directory")?;
        
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
        let config_dir = dirs::config_dir()
            .context("Failed to get config directory")?;
        Ok(config_dir.join("kage").join("auto-steering.md"))
    }
}
