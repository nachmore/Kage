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
    { crate::os::windows::shell::open_path_impl(path) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::shell::open_path_impl(path) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::shell::open_path_impl(path) }
}

/// Reveal a file in the native file manager, selecting it
pub fn reveal_in_file_manager(path: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::shell::reveal_in_file_manager_impl(path) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::shell::reveal_in_file_manager_impl(path) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::shell::reveal_in_file_manager_impl(path) }
}

/// Open a file in the default editor
pub fn open_in_editor(path: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::shell::open_in_editor_impl(path) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::shell::open_in_editor_impl(path) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::shell::open_in_editor_impl(path) }
}


/// Spawn a process with elevated (admin/root) privileges.
/// Windows: ShellExecuteW with "runas" verb.
/// macOS/Linux: pkexec wrapper.
pub fn spawn_elevated(program: &str, args: &[&str]) -> std::io::Result<std::process::Child> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::shell::spawn_elevated_impl(program, args) }

    #[cfg(not(target_os = "windows"))]
    {
        let mut cmd_args: Vec<&str> = vec![program];
        cmd_args.extend(args);
        std::process::Command::new("pkexec")
            .args(&cmd_args)
            .spawn()
    }
}

/// Get the program and arguments for a well-known system command (e.g., "lock", "sleep", "settings").
/// Returns (program, args) appropriate for the current platform.
pub fn system_command(cmd: &str) -> (&'static str, Vec<&'static str>) {
    #[cfg(target_os = "windows")]
    { crate::os::windows::shell::system_command_impl(cmd) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::shell::system_command_impl(cmd) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::shell::system_command_impl(cmd) }
}
