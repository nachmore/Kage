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
}
