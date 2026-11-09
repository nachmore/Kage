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

/// Why [`run_in_terminal`] didn't run the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalRunError {
    /// No safe way to hand the terminal a command on this platform.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Unsupported,
    /// macOS refused the Apple Event: the user said no to (or never saw)
    /// "Kage wants to control Terminal" (Privacy & Security → Automation).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    NotAuthorized,
    /// The terminal couldn't be reached; the detail is English, for logs.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Failed(String),
}

/// Open a new window of the user's terminal app and run `command` in it,
/// in `cwd`, as if they had typed it: their interactive shell, a real TTY
/// (so `sudo` can ask for a password), output in front of them. Blocks
/// until the terminal accepted it — on macOS that can include the
/// one-time "Kage wants to control Terminal" prompt — so call it from a
/// blocking thread.
///
/// macOS only (Terminal.app via Apple Events, the command passed as an
/// argument, never spliced into script source). Windows and Linux return
/// [`TerminalRunError::Unsupported`]: Windows Terminal re-parses `;` in its
/// command line and Linux terminals each quote differently, so there the
/// command is only copied for the user to paste.
pub fn run_in_terminal(command: &str, cwd: &std::path::Path) -> Result<(), TerminalRunError> {
    crate::os::platform::shell::run_in_terminal_impl(command, cwd)
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
