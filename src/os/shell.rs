// Cross-platform shell operations for opening URLs and paths

use anyhow::Result;

/// Open a URL in the default browser
pub fn open_url(url: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::shell::open_url_impl(url)
    }
    
    #[cfg(target_os = "macos")]
    {
        crate::os::macos::shell::open_url_impl(url)
    }
    
    #[cfg(target_os = "linux")]
    {
        crate::os::linux::shell::open_url_impl(url)
    }
}

/// Open a file or directory path with the default application
pub fn open_path(path: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::shell::open_path_impl(path)
    }
    
    #[cfg(target_os = "macos")]
    {
        crate::os::macos::shell::open_path_impl(path)
    }
    
    #[cfg(target_os = "linux")]
    {
        crate::os::linux::shell::open_path_impl(path)
    }
}
