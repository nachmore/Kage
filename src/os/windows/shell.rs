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

/// PowerShell that sets (not toggles) the default playback device's mute
/// state via Core Audio `IAudioEndpointVolume::SetMute`. The previous
/// VK_VOLUME_MUTE keystroke only toggles, so "unmute" muted an unmuted system.
/// `system_command_impl` must return a spawnable (program, args) pair, hence
/// PowerShell + Add-Type rather than calling Core Audio in-process.
///
/// The unnamed methods (`f`..`p`) are placeholder vtable slots: 11 methods
/// precede SetMute in IAudioEndpointVolume, 1 precedes GetDefaultAudioEndpoint
/// in IMMDeviceEnumerator. Only the slot count matters for unused ones.
///
/// The script contains no double quotes or backslashes (the C# quotes are
/// spelled `~` and swapped in by PowerShell) so it survives both Rust's argv
/// quoting and the space-joined parameter string of the elevated path.
macro_rules! set_mute_ps {
    ($mute:literal) => {
        concat!(
            "$ErrorActionPreference = 'Stop'\n",
            "$src = '\n",
            "using System;\n",
            "using System.Runtime.InteropServices;\n",
            "[Guid(~5CDF2C82-841E-4546-9722-0CF74078229A~), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]\n",
            "interface IAudioEndpointVolume {\n",
            "  int f(); int g(); int h(); int i(); int j(); int k();\n",
            "  int l(); int m(); int n(); int o(); int p();\n",
            "  int SetMute([MarshalAs(UnmanagedType.Bool)] bool bMute, IntPtr eventContext);\n",
            "}\n",
            "[Guid(~D666063F-1587-4E43-81F1-B948E807363F~), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]\n",
            "interface IMMDevice {\n",
            "  int Activate(ref Guid iid, int clsCtx, IntPtr activationParams, out IAudioEndpointVolume endpoint);\n",
            "}\n",
            "[Guid(~A95664D2-9614-4F35-A746-DE8DB63617E6~), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]\n",
            "interface IMMDeviceEnumerator {\n",
            "  int f();\n",
            "  int GetDefaultAudioEndpoint(int dataFlow, int role, out IMMDevice device);\n",
            "}\n",
            "[ComImport, Guid(~BCDE0395-E52F-467C-8E3D-C4579291692E~)] class MMDeviceEnumeratorComObject { }\n",
            "public static class KageAudio {\n",
            "  public static void SetMute(bool mute) {\n",
            "    var enumerator = (IMMDeviceEnumerator)(new MMDeviceEnumeratorComObject());\n",
            "    IMMDevice device;\n",
            // eRender = 0, eMultimedia = 1
            "    Marshal.ThrowExceptionForHR(enumerator.GetDefaultAudioEndpoint(0, 1, out device));\n",
            "    IAudioEndpointVolume volume;\n",
            "    var iid = typeof(IAudioEndpointVolume).GUID;\n",
            // CLSCTX_ALL = 23
            "    Marshal.ThrowExceptionForHR(device.Activate(ref iid, 23, IntPtr.Zero, out volume));\n",
            "    Marshal.ThrowExceptionForHR(volume.SetMute(mute, IntPtr.Zero));\n",
            "  }\n",
            "}\n",
            "'.Replace('~', [string][char]34)\n",
            "Add-Type -TypeDefinition $src\n",
            "[KageAudio]::SetMute($",
            $mute,
            ")\n",
        )
    };
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
                "-NonInteractive",
                "-Command",
                set_mute_ps!("true"),
            ],
        ),
        "unmute" => (
            "powershell",
            vec![
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                set_mute_ps!("false"),
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

#[cfg(test)]
mod tests {
    use super::system_command_impl;

    #[test]
    fn mute_and_unmute_set_explicit_state() {
        // Both used to send the same toggle keystroke.
        let (_, mute) = system_command_impl("mute");
        let (_, unmute) = system_command_impl("unmute");
        let mute_script = mute.last().unwrap();
        let unmute_script = unmute.last().unwrap();
        assert!(mute_script.ends_with("[KageAudio]::SetMute($true)\n"));
        assert!(unmute_script.ends_with("[KageAudio]::SetMute($false)\n"));
        // The C# sits in a single-quoted PowerShell string, and nothing may
        // need escaping on the command line (see `set_mute_ps!`).
        let csharp_start = mute_script.find("$src = '").unwrap() + 8;
        let csharp_end = mute_script.rfind("'.Replace(").unwrap();
        assert!(!mute_script[csharp_start..csharp_end].contains('\''));
        assert!(!mute_script.contains('"') && !mute_script.contains('\\'));
    }
}
