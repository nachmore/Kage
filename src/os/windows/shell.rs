// Windows shell operations

use anyhow::{Context, Result};
use std::process::Command;

use super::process::spawn_detached_impl;

/// Open `url` in the default browser, started by Explorer rather than by
/// Kage.
///
/// Kage runs inside a `KILL_ON_JOB_CLOSE` Job Object
/// (`install_kill_on_exit_job_impl`), and a process Kage starts joins it
/// unless it breaks away. `ShellExecuteExW` has no way to ask for that: it
/// runs `CreateProcess` in our process, so a browser that wasn't already
/// running would become Kage's child, join the job and be killed, with
/// every tab, when Kage quits or crashes. So the URL goes to the desktop's
/// own shell (Explorer's `IShellDispatch2::ShellExecute`, reached through
/// the desktop window in `IShellWindows`): Explorer runs the association
/// and the browser is Explorer's child, outside our job, just as if the
/// user had opened the link from the Start menu. It also runs on
/// Explorer's thread, which pumps messages, so a browser's `ddeexec`
/// handler gets its one DDE conversation (see `open_url_in_process`).
///
/// Only when there's no Explorer desktop to ask (another shell, a service
/// session) does Kage open the URL itself, accepting that a browser started
/// that way shares our job.
///
/// Not `rundll32 url.dll,FileProtocolHandler` with
/// `CREATE_BREAKAWAY_FROM_JOB`: endpoint protection flags that command line
/// as a known way to run code (and it runs `ShellExecute` with no message
/// pump, so the double-tab below comes back). Not `explorer.exe <url>`:
/// Explorer's command line splits on commas.
pub fn open_url_impl(url: &str) -> Result<()> {
    let shown = crate::os::shell::url_for_log(url);
    match open_url_via_desktop_shell(url) {
        Ok(()) => {
            log::info!("[open_url] handed to the desktop shell: {shown}");
            Ok(())
        }
        Err(e) => {
            log::warn!(
                "[open_url] the desktop shell could not take {shown} ({e:#}); \
                 opening it from Kage, so a browser this starts shares Kage's Job Object"
            );
            open_url_in_process(url)
        }
    }
}

/// Ask Explorer's desktop to run `url` through its shell. COM is used on a
/// thread of its own, in a single-threaded apartment, so the caller's
/// apartment (a Tokio worker's, often none) doesn't matter.
fn open_url_via_desktop_shell(url: &str) -> Result<()> {
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };

    // The browser is no longer started by Kage, the foreground process, so
    // it wouldn't be allowed to come to the front. Pass our right on (it
    // lapses at the next input) or the page opens behind Kage.
    let _ = unsafe {
        windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
            windows::Win32::UI::WindowsAndMessaging::ASFW_ANY,
        )
    };

    let url = url.to_owned();
    std::thread::Builder::new()
        .name("open-url-desktop-shell".into())
        .spawn(move || {
            // SAFETY: balanced below when it succeeds; every COM object
            // `desktop_shell_execute` creates is dropped before it returns.
            let init =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
            if init.is_err() {
                return Err(anyhow::anyhow!("CoInitializeEx failed: {init:?}"));
            }
            let result = unsafe { desktop_shell_execute(&url) };
            unsafe { CoUninitialize() };
            result.context("desktop shell")
        })
        .context("could not start the desktop shell thread")?
        .join()
        .map_err(|_| anyhow::anyhow!("the desktop shell thread panicked"))?
}

/// Raymond Chen's "launch a program as the desktop's child" sequence:
/// desktop window → top-level browser → its active view → the view's
/// background `IShellFolderViewDual` → `Application` (`IShellDispatch2`)
/// → `ShellExecute`, which runs in Explorer's process.
///
/// # Safety
/// The calling thread must have initialised COM.
unsafe fn desktop_shell_execute(url: &str) -> windows::core::Result<()> {
    use std::mem::ManuallyDrop;
    use windows::core::{Interface, BSTR};
    use windows::Win32::System::Com::{CoCreateInstance, IServiceProvider, CLSCTX_ALL};
    use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VT_I4};
    use windows::Win32::UI::Shell::{
        IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellView, IShellWindows,
        SID_STopLevelBrowser, ShellWindows, CSIDL_DESKTOP, SVGIO_BACKGROUND, SWC_DESKTOP,
        SWFO_NEEDDISPATCH,
    };

    unsafe {
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL)?;

        let mut desktop = VARIANT::default();
        (*desktop.Anonymous.Anonymous).vt = VT_I4;
        (*desktop.Anonymous.Anonymous).Anonymous.lVal = CSIDL_DESKTOP as i32;
        let empty = VARIANT::default();
        let mut hwnd = 0i32;
        let found =
            windows.FindWindowSW(&desktop, &empty, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)?;

        let provider: IServiceProvider = found.cast()?;
        let browser: IShellBrowser = provider.QueryService(&SID_STopLevelBrowser)?;
        let view: IShellView = browser.QueryActiveShellView()?;
        let folder_view: IShellFolderViewDual = view.GetItemObject(SVGIO_BACKGROUND)?;
        let shell: IShellDispatch2 = folder_view.Application()?.cast()?;

        // No arguments: an empty BSTR is a null pointer, so nothing to free.
        let mut no_args = VARIANT::default();
        (*no_args.Anonymous.Anonymous).vt = VT_BSTR;
        (*no_args.Anonymous.Anonymous).Anonymous.bstrVal = ManuallyDrop::new(BSTR::new());

        shell.ShellExecute(&BSTR::from(url), &no_args, &empty, &empty, &empty)
    }
}

/// `ShellExecuteExW` from Kage's own process: the fallback for when there's
/// no Explorer desktop. A browser it starts is Kage's child and joins the
/// Job Object.
fn open_url_in_process(url: &str) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    // The OS owns browser selection, we don't hand-roll detection.
    //
    // The catch: when the default browser registers a `ddeexec` handler (e.g.
    // Firefox: `firefox.exe -osint -url "%1"` + a `shell\open\ddeexec` key)
    // and the shell call comes from a thread WITHOUT a running message pump
    // (our tokio worker — same for cmd/ShellExecuteW), the shell starts a DDE
    // conversation but doesn't pump messages to complete it, times out, and
    // ALSO runs the fallback `open` command. The browser receives the URL
    // twice → two tabs. This is why both plain ShellExecuteW and `cmd /c
    // start` doubled: both consult the association and trigger the ddeexec
    // race.
    //
    // SEE_MASK_NOASYNC (aka SEE_MASK_FLAG_DDEWAIT) tells the shell to finish
    // the DDE conversation before returning instead of racing it with the
    // fallback launch — the documented flag for callers that don't run a
    // message loop. ShellExecuteExW is required because plain ShellExecuteW
    // exposes no fMask.
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

/// Alternatives to `system_command_impl`'s program, tried in order when it
/// isn't installed. Windows Terminal (`wt.exe`) is missing from many
/// Windows 10 installs, so the terminal falls back to a PowerShell console.
/// `start` gives it a console window of its own: system commands are
/// spawned with CREATE_NO_WINDOW (`spawn_detached_impl`).
pub fn system_command_fallbacks_impl(cmd: &str) -> Vec<(&'static str, Vec<&'static str>)> {
    match cmd {
        "terminal" => vec![("cmd", vec!["/C", "start", "", "powershell.exe"])],
        _ => Vec::new(),
    }
}

/// Windows Terminal (`wt.exe new-tab …`) re-parses `;` in its command line
/// as its own separator, and a console window can't be handed a command to
/// type, so there is no safe way to run one there (see
/// `os::shell::run_in_terminal`): the command is copied instead.
pub fn run_in_terminal_impl(
    _command: &str,
    _cwd: &std::path::Path,
) -> Result<(), crate::os::shell::TerminalRunError> {
    Err(crate::os::shell::TerminalRunError::Unsupported)
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
