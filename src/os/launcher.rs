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
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::launcher::scan_applications_impl()
    }
    
    #[cfg(target_os = "macos")]
    {
        crate::os::macos::launcher::scan_applications_impl()
    }
    
    #[cfg(target_os = "linux")]
    {
        crate::os::linux::launcher::scan_applications_impl()
    }
}

/// Launch an application at the given path
pub fn launch_application(path: &PathBuf) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::launcher::launch_application_impl(path)
    }
    
    #[cfg(target_os = "macos")]
    {
        crate::os::macos::launcher::launch_application_impl(path)
    }
    
    #[cfg(target_os = "linux")]
    {
        crate::os::linux::launcher::launch_application_impl(path)
    }
}
