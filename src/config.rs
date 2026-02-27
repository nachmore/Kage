use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
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
    pub math: MathConfig,
    #[serde(default)]
    pub first_run_completed: bool,
    #[serde(default)]
    pub updates: UpdateConfig,
    #[serde(default)]
    pub quick_actions: QuickActionsConfig,
    #[serde(default)]
    pub color_picker: ColorPickerConfig,
    #[serde(default)]
    pub dev_tools: DevToolsConfig,
    #[serde(default)]
    pub timer: TimerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Default for ToolPermissionsConfig {
    fn default() -> Self {
        Self {
            trust_all: false,
            tools: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MathConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub precision: u8,
    #[serde(default = "default_true")]
    pub auto_copy: bool,
    #[serde(default)]
    pub thousands_separator: bool,
}

impl Default for MathConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            precision: 0,
            auto_copy: true,
            thousands_separator: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            auto_check: false,
            silent_update: false,
            last_check_time: None,
            last_updated_version: None,
        }
    }
}

impl Default for AssistantConfig {
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
    pub modifiers: Vec<String>,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpConfig {
    pub mode: AcpMode,
    #[serde(default)]
    pub assistant: AssistantConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantConfig {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorPickerConfig {
    /// Enable color detection and preview in the floating window
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Default format to copy: "hex", "rgb", "hsl", or "all"
    #[serde(default = "default_color_format")]
    pub copy_format: String,
}

fn default_color_format() -> String {
    "all".to_string()
}

impl Default for ColorPickerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            copy_format: "all".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevToolsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub uuid: bool,
    #[serde(default = "default_true")]
    pub base64: bool,
    #[serde(default = "default_true")]
    pub hash: bool,
    #[serde(default = "default_true")]
    pub epoch: bool,
    #[serde(default = "default_true")]
    pub json_format: bool,
}

impl Default for DevToolsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            uuid: true,
            base64: true,
            hash: true,
            epoch: true,
            json_format: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimerConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Show system notification when timer completes
    #[serde(default = "default_true")]
    pub notify_on_complete: bool,
    /// Play a sound when timer completes
    #[serde(default = "default_true")]
    pub sound_on_complete: bool,
    /// Auto-show the floating window when timer completes
    #[serde(default = "default_true")]
    pub show_window_on_complete: bool,
}

impl Default for TimerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            notify_on_complete: true,
            sound_on_complete: true,
            show_window_on_complete: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutConfig {
    pub name: String,
    pub shortcut: String,
    #[serde(default = "default_action_type")]
    pub action_type: String, // "run_program", "open_url", "prompt", "text", "script"
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
                assistant: AssistantConfig::default(),
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
                time_format: "HH:mm".to_string(),
                date_format: "ddd, MMM D".to_string(),
            },
            system: SystemConfig {
                auto_start: false,
                capture_selection: true,
                show_notifications: true,
            },
            shortcuts: vec![],
            debug_mode: false,
            tool_permissions: ToolPermissionsConfig::default(),
            math: MathConfig::default(),
            first_run_completed: false,
            updates: UpdateConfig::default(),
            quick_actions: QuickActionsConfig::default(),
            color_picker: ColorPickerConfig::default(),
            dev_tools: DevToolsConfig::default(),
            timer: TimerConfig::default(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;
        
        if !config_path.exists() {
            let config = Self::default();
            config.save()?;
            return Ok(config);
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
        
        Ok(config_dir.join("kiro-assistant").join("config.json"))
    }
    
    pub fn get_hotkey_string(&self) -> String {
        let mut parts = self.hotkey.modifiers.clone();
        parts.push(self.hotkey.key.clone());
        parts.join("+")
    }

    /// Get the path to the auto-generated steering document
    pub fn get_auto_steering_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Failed to get config directory")?;
        Ok(config_dir.join("kiro-assistant").join("auto-steering.md"))
    }
}
