use super::default_true;
use serde::{Deserialize, Serialize};

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
    /// Display label on the chip. Empty label/prompt render an inert
    /// chip that does nothing when clicked — harmless.
    #[serde(default)]
    pub label: String,
    /// Emoji icon for the chip
    #[serde(default)]
    pub icon: String,
    /// Prompt template — {text} is replaced with the selected text
    #[serde(default)]
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
    /// Display name. An unnamed macro with no steps is inert — it
    /// shows in the list and can be edited or deleted.
    #[serde(default)]
    pub name: String,
    /// Emoji icon
    #[serde(default = "default_macro_icon")]
    pub icon: String,
    /// Ordered list of transformation steps
    #[serde(default)]
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
    /// Empty name/shortcut entries are inert: no hotkey gets
    /// registered (the registrar skips unparseable accelerators) and
    /// the settings list renders them for the user to fix or delete.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
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
