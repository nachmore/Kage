// Windows shell operations

use anyhow::{Context, Result};
use std::process::Command;

use super::process::spawn_detached_impl;

pub fn open_url_impl(url: &str) -> Result<()> {
    // Open via `cmd /c start "" "<url>"` rather than `ShellExecuteW("open", …)`.
    //
    // ShellExecuteW's "open" verb goes through the shell's per-protocol
    // association, which for `http(s)` uses the browser's DDE ("open URL in
    // existing window") channel. On some systems the shell BOTH launches the
    // browser process AND fires the DDE command, so a single call opens the
    // URL in two tabs. We confirmed via logs that our whole stack (JS handler
    // → open_url command → this fn) runs exactly once per click and
    // ShellExecuteW still returned success while two tabs opened — i.e. the
    // duplication was inside the shell dispatch, not our code.
    //
    // `cmd /c start` performs a single CreateProcess-style launch with no DDE,
    // so it dispatches exactly once. The URL is wrapped in quotes so `cmd`
    // doesn't split it on `&` (the breakage that motivated the original switch
    // to ShellExecuteW); the empty `""` first argument is `start`'s title
    // parameter, required so a quoted URL isn't mistaken for the window title.
    log::info!("[open_url] start: {}", url);

    // `raw_arg` passes the token to cmd verbatim (no C-runtime requoting).
    // cmd.exe treats `&`, `|`, `^`, etc. as metacharacters UNLESS they're
    // inside double quotes, so we wrap the URL in quotes ourselves — this is
    // exactly how the `open` crate does it. `""` is start's (empty) window-
    // title argument, required so a quoted URL isn't consumed as the title.
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("cmd");
    cmd.arg("/c")
        .arg("start")
        .raw_arg("\"\"")
        .raw_arg(format!("\"{}\"", url));
    spawn_detached_impl(&mut cmd).context("Failed to open URL")?;
    Ok(())
}

pub fn open_path_impl(path: &str) -> Result<()> {
    spawn_detached_impl(Command::new("explorer").arg(path)).context("Failed to open path")?;
    Ok(())
}

/// Reveal a file in Explorer, selecting it
pub fn reveal_in_file_manager_impl(path: &str) -> Result<()> {
    spawn_detached_impl(Command::new("explorer").args(["/select,", path]))
        .context("Failed to reveal in Explorer")?;
    Ok(())
}

/// Open a file in the default editor
pub fn open_in_editor_impl(path: &str) -> Result<()> {
    spawn_detached_impl(Command::new("cmd").args(["/C", "start", "", path]))
        .context("Failed to open in editor")?;
    Ok(())
}

/// Spawn a process with elevated privileges via ShellExecuteW "runas".
pub fn spawn_elevated_impl(program: &str, args: &[&str]) -> std::io::Result<std::process::Child> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;

    let args_str = args.join(" ");
    let verb: Vec<u16> = std::ffi::OsStr::new("runas")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let file: Vec<u16> = std::ffi::OsStr::new(program)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let params: Vec<u16> = std::ffi::OsStr::new(&args_str)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(if args_str.is_empty() {
                std::ptr::null()
            } else {
                params.as_ptr()
            }),
            PCWSTR(std::ptr::null()),
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        )
    };

    if result.0 as usize > 32 {
        // ShellExecuteW doesn't give us a process handle — return a dummy child
        Command::new("cmd").args(["/C", "rem"]).spawn()
    } else {
        Err(std::io::Error::other(format!(
            "ShellExecuteW failed with code {}",
            result.0 as usize
        )))
    }
}

/// Get the program and arguments for a well-known system command on Windows.
pub fn system_command_impl(cmd: &str) -> (&'static str, Vec<&'static str>) {
    match cmd {
        "lock" => ("rundll32.exe", vec!["user32.dll,LockWorkStation"]),
        "sleep" => (
            "rundll32.exe",
            vec!["powrprof.dll,SetSuspendState", "0,1,0"],
        ),
        "screenshot" => ("snippingtool", vec![]),
        "mute" => (
            "powershell",
            vec![
                "-NoProfile",
                "-Command",
                "(New-Object -ComObject WScript.Shell).SendKeys([char]173)",
            ],
        ),
        "unmute" => (
            "powershell",
            vec![
                "-NoProfile",
                "-Command",
                "(New-Object -ComObject WScript.Shell).SendKeys([char]173)",
            ],
        ),
        "emoji" => ("cmd", vec!["/C", "start", "ms-inputapp:///emojiandmore"]),
        "trash" => ("explorer.exe", vec!["shell:RecycleBinFolder"]),
        "taskmanager" | "taskmgr" => ("taskmgr.exe", vec![]),
        "terminal" => ("wt.exe", vec![]),
        "filemanager" => ("explorer.exe", vec![]),
        "settings" => ("ms-settings:", vec![]),
        "display" => ("ms-settings:display", vec![]),
        "sound" => ("ms-settings:sound", vec![]),
        "wifi" | "network" => ("ms-settings:network-wifi", vec![]),
        "bluetooth" => ("ms-settings:bluetooth", vec![]),
        "apps" => ("ms-settings:appsfeatures", vec![]),
        "updates" => ("ms-settings:windowsupdate", vec![]),
        "devicemanager" | "devmgr" => ("devmgmt.msc", vec![]),
        "restart" => ("shutdown", vec!["/r", "/t", "0"]),
        "shutdown" => ("shutdown", vec!["/s", "/t", "0"]),
        "signout" => ("shutdown", vec!["/l"]),
        _ => ("cmd", vec!["/C", "echo", "Unknown command"]),
    }
}
