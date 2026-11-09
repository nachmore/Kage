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
        // gnome-control-center takes the panel as its own argument —
        // xdg-open accepts exactly one file/URL, so routing these through
        // it failed (silently: the caller only checks that spawn worked).
        "settings" => ("gnome-control-center", vec![]),
        "display" => ("gnome-control-center", vec!["display"]),
        "sound" => ("gnome-control-center", vec!["sound"]),
        "wifi" | "network" => ("gnome-control-center", vec!["network"]),
        "bluetooth" => ("gnome-control-center", vec!["bluetooth"]),
        "apps" => ("xdg-open", vec!["/usr/share/applications"]),
        "updates" => ("gnome-software", vec!["--mode=updates"]),
        "devicemanager" | "devmgr" => ("lshw", vec!["-short"]),
        "restart" => ("systemctl", vec!["reboot"]),
        "shutdown" => ("systemctl", vec!["poweroff"]),
        // No shell here, so no $XDG_SESSION_ID / $USER expansion — use the
        // session manager's own logout.
        "signout" => ("gnome-session-quit", vec!["--logout"]),
        _ => ("echo", vec!["Unknown command"]),
    }
}

/// Alternatives to `system_command_impl`'s program, tried in order when it
/// isn't installed. `x-terminal-emulator` only exists on Debian-family
/// distributions, so the terminal falls back to the freedesktop launcher
/// (`xdg-terminal-exec`), then the common desktops' own terminals, then
/// xterm. Each opens a new window when run with no arguments.
pub fn system_command_fallbacks_impl(cmd: &str) -> Vec<(&'static str, Vec<&'static str>)> {
    match cmd {
        "terminal" => [
            "xdg-terminal-exec",
            "ptyxis",
            "gnome-terminal",
            "kgx",
            "konsole",
            "xfce4-terminal",
            "mate-terminal",
            "lxterminal",
            "tilix",
            "alacritty",
            "kitty",
            "foot",
            "xterm",
        ]
        .into_iter()
        .map(|program| (program, Vec::new()))
        .collect(),
        _ => Vec::new(),
    }
}

/// Spawn a process with elevated privileges using pkexec.
pub fn spawn_elevated_impl(program: &str, args: &[&str]) -> std::io::Result<std::process::Child> {
    let mut cmd_args: Vec<&str> = vec![program];
    cmd_args.extend(args);
    Command::new("pkexec").args(cmd_args).spawn()
}

/// Every Linux terminal takes its command differently (`-e`, `--`, `-x`,
/// one string or an argv), so there is no one safe way to hand it one (see
/// `os::shell::run_in_terminal`): the command is copied instead.
pub fn run_in_terminal_impl(
    _command: &str,
    _cwd: &std::path::Path,
) -> Result<(), crate::os::shell::TerminalRunError> {
    Err(crate::os::shell::TerminalRunError::Unsupported)
}
