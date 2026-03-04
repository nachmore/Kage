use crate::acp_client::AcpClient;
use crate::app_launcher::AppLauncher;
use crate::config::Config;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct AppState {
    pub acp_client: Arc<Mutex<AcpClient>>,
    pub config: Arc<Mutex<Config>>,
    pub app_launcher: Arc<Mutex<AppLauncher>>,
    #[allow(dead_code)]
    pub pipe_stdin: Arc<std::sync::Mutex<Option<Arc<std::sync::Mutex<std::process::ChildStdin>>>>>,
    #[allow(dead_code)]
    pub tcp_writer: Arc<std::sync::Mutex<Option<std::net::TcpStream>>>,
    pub dev_mode: bool,
    /// The session ID used by the floating window (persists across session switches)
    pub floating_session_id: Arc<std::sync::Mutex<Option<String>>>,
    /// Pending permission request: (request_id, tool_title, session_id)
    /// Set when a permission_request notification arrives, cleared when responded to.
    pub pending_permission: Arc<std::sync::Mutex<Option<PendingPermission>>>,
    /// Slash commands received from the ACP server via _kiro.dev/commands/available
    pub slash_commands: Arc<std::sync::Mutex<Vec<SlashCommand>>>,
    /// Available models from the ACP session/new response
    #[allow(dead_code)]
    pub available_models: Arc<std::sync::Mutex<Vec<AcpModel>>>,
    /// Current model ID
    #[allow(dead_code)]
    pub current_model_id: Arc<std::sync::Mutex<Option<String>>>,
    /// Text that was selected in the previously active window when the hotkey was pressed
    pub last_selection: Arc<std::sync::Mutex<Option<String>>>,
    /// Which window sent the last notification ('floating' or 'main')
    pub notification_source: Arc<std::sync::Mutex<String>>,
    /// Updater state for auto-update system
    pub updater: Arc<crate::updater::UpdaterState>,
    /// Pocket TTS server child process
    pub pocket_tts_process: Arc<std::sync::Mutex<Option<std::process::Child>>>,
    /// Pocket TTS pip install child process (for cancellation)
    pub pocket_tts_install_process: Arc<std::sync::Mutex<Option<std::process::Child>>>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PendingPermission {
    pub request_id: serde_json::Value,
    pub tool_title: String,
    pub session_id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub meta: Option<SlashCommandMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SlashCommandMeta {
    #[serde(rename = "optionsMethod")]
    pub options_method: Option<String>,
    #[serde(rename = "inputType")]
    pub input_type: Option<String>,
    pub hint: Option<String>,
    pub local: Option<bool>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AcpModel {
    #[serde(rename = "modelId")]
    pub model_id: String,
    pub name: String,
    pub description: String,
}
