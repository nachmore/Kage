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
        "trash" => ("open", vec!["-a", "Finder", "/Users"]),
        "taskmanager" | "taskmgr" => ("open", vec!["-a", "Activity Monitor"]),
        "terminal" => ("open", vec!["-a", "Terminal"]),
        "filemanager" => ("open", vec!["-a", "Finder"]),
        "settings" => ("open", vec!["-a", "System Preferences"]),
        "display" => ("open", vec!["-a", "System Preferences", "--args", "Displays"]),
        "sound" => ("open", vec!["-a", "System Preferences", "--args", "Sound"]),
        "wifi" | "network" => ("open", vec!["-a", "System Preferences", "--args", "Network"]),
        "bluetooth" => ("open", vec!["-a", "System Preferences", "--args", "Bluetooth"]),
        "apps" => ("open", vec!["/Applications"]),
        "updates" => ("open", vec!["-a", "System Preferences", "--args", "Software Update"]),
        "devicemanager" | "devmgr" => ("open", vec!["-a", "System Information"]),
        "restart" => ("osascript", vec!["-e", "tell application \"System Events\" to restart"]),
        "shutdown" => ("osascript", vec!["-e", "tell application \"System Events\" to shut down"]),
        "signout" => ("osascript", vec!["-e", "tell application \"System Events\" to log out"]),
        _ => ("echo", vec!["Unknown command"]),
    }
}

/// Spawn a process with elevated privileges using osascript (admin prompt).
pub fn spawn_elevated_impl(program: &str, args: &[&str]) -> std::io::Result<std::process::Child> {
    let mut cmd_args: Vec<&str> = vec![program];
    cmd_args.extend(args);
    // Use pkexec as a cross-Unix fallback; macOS could also use osascript with admin privileges
    Command::new("pkexec").args(&cmd_args).spawn()
}
