use crate::acp_client::AcpClient;
use crate::app_launcher::AppLauncher;
use crate::config::Config;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct AppState {
    pub acp_client: Arc<Mutex<AcpClient>>,
    pub config: Arc<Mutex<Config>>,
    pub app_launcher: Arc<Mutex<AppLauncher>>,
    pub pipe_stdin: Arc<std::sync::Mutex<Option<Arc<std::sync::Mutex<std::process::ChildStdin>>>>>,
    pub tcp_writer: Arc<std::sync::Mutex<Option<std::net::TcpStream>>>,
    pub dev_mode: bool,
    /// The session ID used by the floating window (persists across session switches)
    pub floating_session_id: Arc<std::sync::Mutex<Option<String>>>,
    /// Pending permission request: (request_id, tool_title, session_id)
    /// Set when a permission_request notification arrives, cleared when responded to.
    pub pending_permission: Arc<std::sync::Mutex<Option<PendingPermission>>>,
}

#[derive(Debug, Clone)]
pub struct PendingPermission {
    pub request_id: serde_json::Value,
    pub tool_title: String,
    pub session_id: String,
}
