// Linux shell operations

use anyhow::{Context, Result};
use std::process::Command;

pub fn open_url_impl(url: &str) -> Result<()> {
    Command::new("xdg-open")
        .arg(url)
        .spawn()
        .context("Failed to open URL")?;
    Ok(())
}

pub fn open_path_impl(path: &str) -> Result<()> {
    Command::new("xdg-open")
        .arg(path)
        .spawn()
        .context("Failed to open path")?;
    Ok(())
}

/// Reveal a file in the default file manager.
/// Linux doesn't have a universal "select file" command, so we open the parent directory.
pub fn reveal_in_file_manager_impl(path: &str) -> Result<()> {
    let parent = std::path::Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    Command::new("xdg-open")
        .arg(&parent)
        .spawn()
        .context("Failed to open file manager")?;
    Ok(())
}

/// Open a file in the default editor
pub fn open_in_editor_impl(path: &str) -> Result<()> {
    Command::new("xdg-open")
        .arg(path)
        .spawn()
        .context("Failed to open in editor")?;
    Ok(())
}


/// Get the program and arguments for a well-known system command on Linux.
pub fn system_command_impl(cmd: &str) -> (&'static str, Vec<&'static str>) {
    match cmd {
        "lock" => ("loginctl", vec!["lock-session"]),
        "sleep" => ("systemctl", vec!["suspend"]),
        "screenshot" => ("gnome-screenshot", vec!["-c"]),
        "mute" => ("amixer", vec!["set", "Master", "mute"]),
        "unmute" => ("amixer", vec!["set", "Master", "unmute"]),
        "emoji" => ("ibus", vec!["emoji"]),
        "trash" => ("xdg-open", vec!["trash:///"]),
        "taskmanager" | "taskmgr" => ("gnome-system-monitor", vec![]),
        "terminal" => ("x-terminal-emulator", vec![]),
        "filemanager" => ("xdg-open", vec!["."]),
        "settings" => ("xdg-open", vec!["gnome-control-center"]),
        "display" => ("xdg-open", vec!["gnome-control-center", "display"]),
        "sound" => ("xdg-open", vec!["gnome-control-center", "sound"]),
        "wifi" | "network" => ("xdg-open", vec!["gnome-control-center", "network"]),
        "bluetooth" => ("xdg-open", vec!["gnome-control-center", "bluetooth"]),
        "apps" => ("xdg-open", vec!["/usr/share/applications"]),
        "updates" => ("xdg-open", vec!["gnome-control-center", "info-overview"]),
        "devicemanager" | "devmgr" => ("lshw", vec!["-short"]),
        "restart" => ("systemctl", vec!["reboot"]),
        "shutdown" => ("systemctl", vec!["poweroff"]),
        "signout" => ("loginctl", vec!["terminate-user", ""]),
        _ => ("echo", vec!["Unknown command"]),
    }
}

/// Spawn a process with elevated privileges using pkexec.
pub fn spawn_elevated_impl(program: &str, args: &[&str]) -> std::io::Result<std::process::Child> {
    let mut cmd_args: Vec<&str> = vec![program];
    cmd_args.extend(args);
    Command::new("pkexec").args(&cmd_args).spawn()
}
