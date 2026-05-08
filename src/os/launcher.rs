// Cross-platform application launcher scanning

use anyhow::Result;
use std::path::PathBuf;

/// Represents a discovered application
#[derive(Debug, Clone)]
pub struct AppInfo {
    pub name: String,
    pub path: PathBuf,
    pub icon_path: Option<String>,
    /// Emoji icon for built-in/system apps that don't have extractable icons
    pub emoji_icon: Option<String>,
    /// Pre-computed icon data URI (e.g., data:image/svg+xml;base64,...)
    pub icon_data: Option<String>,
}

/// Scan the system for installed applications
pub fn scan_applications() -> Result<Vec<AppInfo>> {
    crate::os::platform::launcher::scan_applications_impl()
}

/// Launch an application at the given path
pub fn launch_application(path: &PathBuf) -> Result<()> {
    crate::os::platform::launcher::launch_application_impl(path)
}

/// Launch an application by name or URI via the platform's shell. Name may be:
///   - display name ("Calculator", "Safari")
///   - executable basename with args ("winword /w")
///   - URI ("https://...", "x-apple.systempreferences:...")
///   - full path
///
/// Each platform uses its native name-resolution mechanism:
///   - Windows: `ShellExecuteW` — resolves via PATH + App Paths registry
///   - macOS:   `open -a <name>` for bare names, `open <uri|path>` otherwise
///   - Linux:   `xdg-open` for URIs, best-effort `Command::new(name)` for names
///
/// Distinct from `launch_application` (which takes a `PathBuf`) and from the
/// Tauri `launch_app_by_name` command (which fuzzy-matches against the
/// scanned app list for the floating-window launcher UI). This one is used
/// by the MCP `launch_app` and `launch_and_get_tree` tools, which receive
/// a free-form name from the agent.
#[allow(dead_code)] // consumed by src/bin/computer_control_mcp.rs, not by the main kage binary
pub fn shell_launch(name: &str) -> Result<()> {
    crate::os::platform::launcher::shell_launch_impl(name)
}
