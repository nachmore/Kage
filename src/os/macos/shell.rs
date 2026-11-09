// macOS shell operations

use anyhow::{Context, Result};
use std::process::Command;

pub fn open_url_impl(url: &str) -> Result<()> {
    Command::new("open")
        .arg(url)
        .spawn()
        .context("Failed to open URL")?;
    Ok(())
}

pub fn open_path_impl(path: &str) -> Result<()> {
    Command::new("open")
        .arg(path)
        .spawn()
        .context("Failed to open path")?;
    Ok(())
}

/// Reveal a file in Finder, selecting it
pub fn reveal_in_file_manager_impl(path: &str) -> Result<()> {
    Command::new("open")
        .args(["-R", path])
        .spawn()
        .context("Failed to reveal in Finder")?;
    Ok(())
}

/// Open a file in the default editor
pub fn open_in_editor_impl(path: &str) -> Result<()> {
    Command::new("open")
        .arg(path)
        .spawn()
        .context("Failed to open in editor")?;
    Ok(())
}

/// Get the program and arguments for a well-known system command on macOS.
pub fn system_command_impl(cmd: &str) -> (&'static str, Vec<&'static str>) {
    match cmd {
        "lock" => ("osascript", vec!["-e", "tell application \"System Events\" to keystroke \"q\" using {command down, control down}"]),
        "sleep" => ("osascript", vec!["-e", "tell application \"System Events\" to sleep"]),
        "screenshot" => ("osascript", vec!["-e", "do shell script \"screencapture -ic\""]),
        "mute" => ("osascript", vec!["-e", "set volume with output muted"]),
        "unmute" => ("osascript", vec!["-e", "set volume without output muted"]),
        "emoji" => ("osascript", vec!["-e", "tell application \"System Events\" to keystroke \" \" using {command down, control down}"]),
        "trash" => ("osascript", vec!["-e", "tell application \"Finder\" to open trash", "-e", "tell application \"Finder\" to activate"]),
        "taskmanager" | "taskmgr" => ("open", vec!["-a", "Activity Monitor"]),
        "terminal" => ("open", vec!["-a", "Terminal"]),
        "filemanager" => ("open", vec!["-a", "Finder"]),
        // Pane URLs rather than `open -a "System Preferences" --args <pane>`:
        // the app ignores --args (every entry opened the home pane) and is
        // named "System Settings" on macOS 13+. The legacy pane IDs are
        // still honoured by System Settings.
        "settings" => ("open", vec!["x-apple.systempreferences:"]),
        "display" => ("open", vec!["x-apple.systempreferences:com.apple.preference.displays"]),
        "sound" => ("open", vec!["x-apple.systempreferences:com.apple.preference.sound"]),
        "wifi" | "network" => ("open", vec!["x-apple.systempreferences:com.apple.preference.network"]),
        "bluetooth" => ("open", vec!["x-apple.systempreferences:com.apple.preferences.Bluetooth"]),
        "apps" => ("open", vec!["/Applications"]),
        "updates" => ("open", vec!["x-apple.systempreferences:com.apple.preferences.softwareupdate"]),
        "devicemanager" | "devmgr" => ("open", vec!["-a", "System Information"]),
        "restart" => ("osascript", vec!["-e", "tell application \"System Events\" to restart"]),
        "shutdown" => ("osascript", vec!["-e", "tell application \"System Events\" to shut down"]),
        "signout" => ("osascript", vec!["-e", "tell application \"System Events\" to log out"]),
        _ => ("echo", vec!["Unknown command"]),
    }
}

/// Alternatives to `system_command_impl`'s program for when it isn't
/// installed. None on macOS: every program it names ships with the OS.
pub fn system_command_fallbacks_impl(_cmd: &str) -> Vec<(&'static str, Vec<&'static str>)> {
    Vec::new()
}

/// Spawn a process with elevated privileges using osascript's `with administrator privileges`.
///
/// This triggers the native macOS admin authentication dialog (Touch ID / password).
/// The command and its arguments are shell-escaped and executed via `do shell script`.
pub fn spawn_elevated_impl(program: &str, args: &[&str]) -> std::io::Result<std::process::Child> {
    // Build the shell command string, quoting each component for safety
    let quoted_parts: Vec<String> = std::iter::once(program)
        .chain(args.iter().copied())
        .map(shell_quote)
        .collect();
    let shell_cmd = quoted_parts.join(" ");

    // osascript -e 'do shell script "..." with administrator privileges'
    // The inner command uses escaped double-quotes inside the AppleScript string.
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        shell_cmd.replace('\\', "\\\\").replace('"', "\\\"")
    );

    Command::new("osascript").args(["-e", &script]).spawn()
}

/// Quote a string for safe inclusion in a shell command.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    // If the string contains no special characters, return as-is
    if s.chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/')
    {
        return s.to_string();
    }
    // Wrap in single quotes, escaping any embedded single quotes
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The AppleScript behind `run_in_terminal_impl`. Constant source: the
/// directory and the command arrive as `argv` items, so no quoting of
/// either into AppleScript is ever needed. The directory reaches the shell
/// through `quoted form of` (POSIX single quotes); the command is typed as
/// is — the text the user approved — on its own line in the same tab.
const RUN_IN_TERMINAL_SCRIPT: &[&str] = &[
    "on run argv",
    "set dir to item 1 of argv",
    "set cmd to item 2 of argv",
    "tell application \"Terminal\"",
    "set t to do script (\"cd \" & quoted form of dir)",
    "do script cmd in t",
    "activate",
    "end tell",
    "end run",
];

/// How long to wait for Terminal to take the command, including the
/// one-time Automation prompt the user may be looking at.
const RUN_IN_TERMINAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// `osascript` arguments: the script as `-e` lines, then the directory and
/// the command. The directory (always absolute) comes first, so getopt has
/// stopped at it before the command — which could start with `-` — and
/// can't read the command as an option.
fn run_in_terminal_args(command: &str, cwd: &std::path::Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    for line in RUN_IN_TERMINAL_SCRIPT {
        args.push("-e".into());
        args.push((*line).into());
    }
    args.push(cwd.as_os_str().to_os_string());
    args.push(command.into());
    args
}

/// What an `osascript` failure means, from its stderr.
fn classify_run_in_terminal_error(stderr: &str) -> crate::os::shell::TerminalRunError {
    use crate::os::shell::TerminalRunError;
    // errAEEventNotPermitted: Privacy & Security → Automation said no.
    if stderr.contains("-1743") || stderr.contains("Not authorized to send Apple events") {
        return TerminalRunError::NotAuthorized;
    }
    let line = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let detail: String = line.trim().chars().take(300).collect();
    TerminalRunError::Failed(if detail.is_empty() {
        "osascript failed".into()
    } else {
        detail
    })
}

/// Run `command` in a new Terminal.app window, in `cwd` (see
/// `os::shell::run_in_terminal`). The first time, macOS asks the user
/// whether Kage may control Terminal; an ad-hoc-signed update can ask again.
pub fn run_in_terminal_impl(
    command: &str,
    cwd: &std::path::Path,
) -> std::result::Result<(), crate::os::shell::TerminalRunError> {
    use crate::os::shell::TerminalRunError;
    use std::io::Read;
    use std::process::Stdio;

    let mut child = Command::new("/usr/bin/osascript")
        .args(run_in_terminal_args(command, cwd))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| TerminalRunError::Failed(format!("could not start osascript: {e}")))?;
    let deadline = std::time::Instant::now() + RUN_IN_TERMINAL_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(TerminalRunError::Failed(
                    "Terminal didn't respond in time".into(),
                ));
            }
            Err(e) => return Err(TerminalRunError::Failed(e.to_string())),
        }
    };
    if status.success() {
        return Ok(());
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    Err(classify_run_in_terminal_error(&stderr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::shell::TerminalRunError;

    #[test]
    fn run_in_terminal_passes_dir_and_command_as_arguments() {
        let command = "echo \"it's\" `date` $HOME; rm -rf -- '\\\"'\nsecond line";
        let dir = std::path::Path::new("/Users/someone/My \"Projects\"");
        let args = run_in_terminal_args(command, dir);
        let n = RUN_IN_TERMINAL_SCRIPT.len();
        assert_eq!(args.len(), n * 2 + 2);
        for (i, line) in RUN_IN_TERMINAL_SCRIPT.iter().enumerate() {
            assert_eq!(args[i * 2], "-e");
            assert_eq!(args[i * 2 + 1], *line);
        }
        // Exactly the approved text, as one argument, after the directory.
        assert_eq!(args[n * 2], dir.as_os_str());
        assert_eq!(args[n * 2 + 1], command);
    }

    #[test]
    fn run_in_terminal_script_is_constant() {
        // Nothing but argv reaches the script: no line may be built from data.
        for line in RUN_IN_TERMINAL_SCRIPT {
            assert!(!line.contains('{'), "{line}");
        }
        assert!(RUN_IN_TERMINAL_SCRIPT.contains(&"set cmd to item 2 of argv"));
        assert!(RUN_IN_TERMINAL_SCRIPT
            .iter()
            .any(|l| l.contains("quoted form of dir")));
    }

    #[test]
    fn run_in_terminal_errors_are_classified() {
        assert_eq!(
            classify_run_in_terminal_error(
                "0:120: execution error: Not authorized to send Apple events to Terminal. (-1743)\n"
            ),
            TerminalRunError::NotAuthorized
        );
        assert_eq!(
            classify_run_in_terminal_error("\n0:5: execution error: Terminal got an error.\n"),
            TerminalRunError::Failed("0:5: execution error: Terminal got an error.".into())
        );
        assert_eq!(
            classify_run_in_terminal_error(""),
            TerminalRunError::Failed("osascript failed".into())
        );
    }
}
