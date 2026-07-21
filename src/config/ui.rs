use serde::{Deserialize, Serialize};

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

fn default_speech_silence_timeout() -> f32 {
    2.0
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
