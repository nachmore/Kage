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

/// Every way to run a well-known system command, in order of preference:
/// `system_command`'s program first, then the platform's alternatives for
/// when that one isn't installed. Only the terminal has alternatives: no
/// single terminal launcher exists on every Linux distribution, and
/// Windows Terminal is missing from many Windows 10 installs.
pub fn system_command_candidates(cmd: &str) -> Vec<(&'static str, Vec<&'static str>)> {
    let mut candidates = vec![system_command(cmd)];
    candidates.extend(crate::os::platform::shell::system_command_fallbacks_impl(
        cmd,
    ));
    candidates
}

#[cfg(test)]
mod tests {
    use super::{system_command, system_command_candidates};

    #[test]
    fn candidates_start_with_the_primary_program() {
        for cmd in ["terminal", "lock", "filemanager", "no-such-command"] {
            let candidates = system_command_candidates(cmd);
            assert_eq!(candidates.first(), Some(&system_command(cmd)), "{cmd}");
        }
    }

    #[test]
    fn candidates_never_repeat_a_program() {
        let candidates = system_command_candidates("terminal");
        for (i, (program, _)) in candidates.iter().enumerate() {
            assert!(
                candidates[i + 1..].iter().all(|(p, _)| p != program),
                "{program} listed twice"
            );
        }
    }

    #[test]
    fn only_the_terminal_has_alternatives() {
        for cmd in ["lock", "sleep", "filemanager", "restart", "no-such-command"] {
            assert_eq!(system_command_candidates(cmd).len(), 1, "{cmd}");
        }
    }
}
