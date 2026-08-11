// Windows shell operations

use anyhow::{Context, Result};
use std::process::Command;

use super::process::spawn_detached_impl;

pub fn open_url_impl(url: &str) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    // Open the URL through the shell's default-browser association — the OS
    // owns browser selection, we don't hand-roll detection.
    //
    // The catch: when the default browser registers a `ddeexec` handler (e.g.
    // Firefox: `firefox.exe -osint -url "%1"` + a `shell\open\ddeexec` key)
    // and the shell call comes from a thread WITHOUT a running message pump
    // (our tokio worker — same for cmd/ShellExecuteW), the shell starts a DDE
    // conversation but doesn't pump messages to complete it, times out, and
    // ALSO runs the fallback `open` command. The browser receives the URL
    // twice → two tabs. This is why both plain ShellExecuteW and `cmd /c
    // start` doubled: both consult the association and trigger the ddeexec
    // race. Our logs proved every layer of our own code fired exactly once.
    //
    // SEE_MASK_NOASYNC (aka SEE_MASK_FLAG_DDEWAIT) tells the shell to finish
    // the DDE conversation before returning instead of racing it with the
    // fallback launch — the documented flag for callers that don't run a
    // message loop. That collapses the double-open back to a single tab while
    // leaving browser choice entirely to the OS. ShellExecuteExW is required
    // because plain ShellExecuteW exposes no fMask.
    log::info!("[open_url] ShellExecuteExW (NOASYNC): {}", url);

    let mut verb: Vec<u16> = "open".encode_utf16().collect();
    verb.push(0);
    let mut file: Vec<u16> = std::ffi::OsStr::new(url).encode_wide().collect();
    file.push(0);

    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..unsafe { std::mem::zeroed() }
    };

    unsafe { ShellExecuteExW(&mut info) }.context("ShellExecuteExW failed to open URL")?;
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
