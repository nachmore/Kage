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
