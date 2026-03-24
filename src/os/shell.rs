// Cross-platform shell operations for opening URLs and paths

use anyhow::Result;

/// Open a URL in the default browser
pub fn open_url(url: &str) -> Result<()> {
    crate::os::platform::shell::open_url_impl(url)
}

/// Open a file or directory path with the default application
pub fn open_path(path: &str) -> Result<()> {
    crate::os::platform::shell::open_path_impl(path)
}

/// Reveal a file in the native file manager, selecting it
pub fn reveal_in_file_manager(path: &str) -> Result<()> {
    crate::os::platform::shell::reveal_in_file_manager_impl(path)
}

/// Open a file in the default editor
pub fn open_in_editor(path: &str) -> Result<()> {
    crate::os::platform::shell::open_in_editor_impl(path)
}

/// Spawn a process with elevated (admin/root) privileges.
/// Windows: ShellExecuteW with "runas" verb.
/// macOS/Linux: pkexec wrapper.
pub fn spawn_elevated(program: &str, args: &[&str]) -> std::io::Result<std::process::Child> {
    crate::os::platform::shell::spawn_elevated_impl(program, args)
}

/// Get the program and arguments for a well-known system command.
/// Returns (program, args) appropriate for the current platform.
pub fn system_command(cmd: &str) -> (&'static str, Vec<&'static str>) {
    crate::os::platform::shell::system_command_impl(cmd)
}
