// Windows process management

use anyhow::Result;
use log::{info, warn};
use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x08000000;

// get_process_name_impl / spawn_detached_impl moved to kage-core (the
// sidecar's accessibility + launcher stack needs them); re-exported so
// `super::process::*` callers here keep resolving.
pub use kage_core::os::windows::process::{get_process_name_impl, spawn_detached_impl};

pub fn kill_process_impl(pid: u32) -> bool {
    match Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(output) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                info!("taskkill PID {} failed: {}", pid, stderr.trim());
            }
            output.status.success()
        }
        Err(e) => {
            warn!("Failed to run taskkill for PID {}: {}", pid, e);
            false
        }
    }
}

pub fn configure_spawn_impl(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
    info!("Windows: Setting CREATE_NO_WINDOW flag");
}

pub fn install_signal_handlers_impl<F>(cleanup_fn: F) -> Result<()>
where
    F: Fn() + Send + 'static,
{
    ctrlc::set_handler(move || {
        info!("Received Ctrl+C signal");
        cleanup_fn();
        std::process::exit(0);
    })?;

    info!("✅ Ctrl+C handler installed");
    Ok(())
}

/// Kill the WebView2 processes pinned to our user data folder whose
/// host Kage is gone. Returns the number of process trees killed.
///
/// Used by `startup::ensure_webview_directory_writable` (via the
/// platform-agnostic `os::cleanup_stale_processes`). WebView2 shares
/// one browser process per user data folder: a new Kage asks the
/// runtime for a webview and the runtime hands the request to the
/// browser process already serving that folder. When a previous Kage
/// died and left its browser process behind (hung, or kept alive after
/// an update released the kill-on-close job), every webview the new
/// Kage creates fails with `0x800700AA` ("The requested resource is in
/// use"): no window ever appears, while the tray and hotkeys still
/// register.
///
/// # What counts as an orphan
///
/// A browser process (no `--type=` argument) is the one WebView2
/// launches for its host app, so its parent is the host. It's orphaned
/// when that parent is gone: not running, a different program than the
/// `--webview-exe-name=` it was launched for, or started after the
/// browser process (the PID was reused). A child process (`--type=`
/// renderer, GPU, crashpad, …) is orphaned when its browser process is
/// no longer running; the children of an orphaned browser go with it.
///
/// A webview of a Kage that's still running is never touched, so a
/// second launch (which runs this before the single-instance plugin
/// hands off to the first) leaves the first one alone.
///
/// # How it reads command lines
///
/// `CreateToolhelp32Snapshot` lists every process with its parent; each
/// `msedgewebview2.exe`'s command line is read from its PEB
/// (`NtQueryInformationProcess` → PEB → ProcessParameters →
/// CommandLine). The PEB layout is documented as reserved but stable
/// and is what Process Explorer relies on; if it ever changes, the read
/// fails and nothing is killed. Every failure (access denied, a process
/// that died mid-scan) just skips that process.
pub fn cleanup_stale_processes_impl(user_data_dir: &std::path::Path) -> usize {
    let procs = match snapshot_processes() {
        Ok(v) => v,
        Err(e) => {
            warn!(
                "Failed to enumerate processes while looking for orphan WebView2: {}",
                e
            );
            return 0;
        }
    };
    let by_pid: std::collections::HashMap<u32, &ProcessEntry> =
        procs.iter().map(|p| (p.pid, p)).collect();

    let webviews: Vec<WebviewProcess> = procs
        .iter()
        .filter(|p| p.exe.eq_ignore_ascii_case("msedgewebview2.exe"))
        .filter_map(|p| {
            let cmdline = read_process_command_line(p.pid).ok()?;
            cmdline_matches_kage_webview(&cmdline, user_data_dir)
                .then(|| WebviewProcess::from_cmdline(p.pid, p.parent, &cmdline))
        })
        .collect();
    if webviews.is_empty() {
        return 0;
    }

    let roots = orphaned_webview_roots(&webviews, |browser| host_is_alive(browser, &by_pid));
    if roots.is_empty() {
        return 0;
    }

    warn!(
        "Found {} orphan Kage WebView2 process tree(s) (roots {:?}) whose host is gone — killing",
        roots.len(),
        roots
    );
    let mut killed = 0;
    for &pid in &roots {
        // `taskkill /T` takes the renderer/GPU children with it.
        if kill_process_impl(pid) {
            killed += 1;
        }
    }
    // The runtime only starts a fresh browser process for the folder
    // once the old one is fully gone.
    for &pid in &roots {
        wait_for_process_exit(pid, 2_000);
    }
    killed
}

/// One row of a process snapshot.
struct ProcessEntry {
    pid: u32,
    parent: u32,
    exe: String,
}

/// A `msedgewebview2.exe` pinned to our user data folder.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WebviewProcess {
    pid: u32,
    parent: u32,
    /// No `--type=`: the browser process, whose parent is the host app.
    is_browser: bool,
    /// `--webview-exe-name=`: the host app's image name.
    host_exe: Option<String>,
}

impl WebviewProcess {
    fn from_cmdline(pid: u32, parent: u32, cmdline: &str) -> Self {
        let host_exe = cmdline
            .split_whitespace()
            .find_map(|arg| arg.strip_prefix("--webview-exe-name="))
            .map(|s| s.trim_matches('"').to_string())
            .filter(|s| !s.is_empty());
        Self {
            pid,
            parent,
            is_browser: !cmdline.contains("--type="),
            host_exe,
        }
    }
}

/// The PIDs to tree-kill so every orphaned webview process goes:
/// browsers whose host is gone, plus children whose browser is no
/// longer running. Children of an orphaned browser aren't listed — the
/// tree kill of their browser takes them.
fn orphaned_webview_roots(
    procs: &[WebviewProcess],
    host_alive: impl Fn(&WebviewProcess) -> bool,
) -> Vec<u32> {
    let pids: std::collections::HashSet<u32> = procs.iter().map(|p| p.pid).collect();
    procs
        .iter()
        .filter(|p| {
            if p.is_browser {
                !host_alive(p)
            } else {
                !pids.contains(&p.parent)
            }
        })
        .map(|p| p.pid)
        .collect()
}

/// Is the browser process's parent still the app that launched it?
fn host_is_alive(
    browser: &WebviewProcess,
    by_pid: &std::collections::HashMap<u32, &ProcessEntry>,
) -> bool {
    let Some(host) = by_pid.get(&browser.parent) else {
        return false;
    };
    if let Some(expected) = &browser.host_exe {
        if !host.exe.eq_ignore_ascii_case(expected) {
            return false;
        }
    }
    // A reused PID belongs to a process started after the browser.
    match (
        process_creation_time(host.pid),
        process_creation_time(browser.pid),
    ) {
        (Some(host_t), Some(browser_t)) => host_t <= browser_t,
        // Can't tell: treat it as alive and never kill on a guess.
        _ => true,
    }
}

/// The process's creation time in FILETIME ticks, if readable.
fn process_creation_time(pid: u32) -> Option<u64> {
    use windows::Win32::Foundation::{CloseHandle, FILETIME};
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: a handle we open and close here; the out-params are locals.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let ok = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user);
        let _ = CloseHandle(handle);
        ok.ok()?;
        Some(((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64)
    }
}

/// Wait up to `timeout_ms` for `pid` to exit. Returns at once when it
/// already has (or can't be opened).
fn wait_for_process_exit(pid: u32, timeout_ms: u32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };
    // SAFETY: a handle we open and close here.
    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(handle, timeout_ms);
            let _ = CloseHandle(handle);
        }
    }
}

/// Every running process with its parent PID and image name.
fn snapshot_processes() -> Result<Vec<ProcessEntry>> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut procs: Vec<ProcessEntry> = Vec::new();

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .map_err(|e| anyhow::anyhow!("CreateToolhelp32Snapshot failed: {}", e))?;

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                // szExeFile is a fixed-size [u16; 260] null-terminated UTF-16 string.
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                procs.push(ProcessEntry {
                    pid: entry.th32ProcessID,
                    parent: entry.th32ParentProcessID,
                    exe: String::from_utf16_lossy(&entry.szExeFile[..len]),
                });
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }

    Ok(procs)
}

/// Read the command line of a foreign process via PEB walk.
///
/// PEB layout the code relies on (stable across Windows versions):
///
/// ```text
/// PEB
///   + 0x20  ProcessParameters: *mut RTL_USER_PROCESS_PARAMETERS  (x64)
/// RTL_USER_PROCESS_PARAMETERS
///   + 0x70  CommandLine: UNICODE_STRING { Length, MaxLength, Buffer }  (x64)
/// ```
///
/// We use the typed `windows` crate bindings so the offsets come from
/// the canonical headers rather than hardcoded numbers.
fn read_process_command_line(pid: u32) -> Result<String> {
    use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
    use windows::Win32::Foundation::{CloseHandle, UNICODE_STRING};
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::Threading::{
        OpenProcess, PEB, PROCESS_BASIC_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_VM_READ, RTL_USER_PROCESS_PARAMETERS,
    };

    unsafe {
        let handle = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
            false,
            pid,
        )
        .map_err(|e| anyhow::anyhow!("OpenProcess({}) failed: {}", pid, e))?;

        // Always close the handle on every exit path. Wrapped in a
        // closure so the early-return arms don't need to repeat the
        // CloseHandle call.
        let result = (|| -> Result<String> {
            // 1. PEB base address via NtQueryInformationProcess.
            let mut pbi: PROCESS_BASIC_INFORMATION = std::mem::zeroed();
            let mut returned: u32 = 0;
            let status = NtQueryInformationProcess(
                handle,
                ProcessBasicInformation,
                &mut pbi as *mut _ as *mut _,
                std::mem::size_of::<PROCESS_BASIC_INFORMATION>() as u32,
                &mut returned,
            );
            if status.0 < 0 {
                return Err(anyhow::anyhow!(
                    "NtQueryInformationProcess({}) status=0x{:x}",
                    pid,
                    status.0 as u32
                ));
            }
            if pbi.PebBaseAddress.is_null() {
                return Err(anyhow::anyhow!("PEB base address is null for pid {}", pid));
            }

            // 2. Read the PEB to get ProcessParameters pointer.
            let mut peb: PEB = std::mem::zeroed();
            ReadProcessMemory(
                handle,
                pbi.PebBaseAddress as *const _,
                &mut peb as *mut _ as *mut _,
                std::mem::size_of::<PEB>(),
                None,
            )
            .map_err(|e| anyhow::anyhow!("ReadProcessMemory(PEB pid={}): {}", pid, e))?;

            if peb.ProcessParameters.is_null() {
                return Err(anyhow::anyhow!("ProcessParameters is null for pid {}", pid));
            }

            // 3. Read RTL_USER_PROCESS_PARAMETERS for the CommandLine
            //    UNICODE_STRING. The struct's tail is variable-length on
            //    paper but the binding only exposes up through CommandLine,
            //    which is what we want anyway.
            let mut params: RTL_USER_PROCESS_PARAMETERS = std::mem::zeroed();
            ReadProcessMemory(
                handle,
                peb.ProcessParameters as *const _,
                &mut params as *mut _ as *mut _,
                std::mem::size_of::<RTL_USER_PROCESS_PARAMETERS>(),
                None,
            )
            .map_err(|e| anyhow::anyhow!("ReadProcessMemory(params pid={}): {}", pid, e))?;

            // 4. Read the command-line buffer itself. Length is in BYTES
            //    (per Win32 UNICODE_STRING contract — not characters).
            let cmdline_unicode: UNICODE_STRING = params.CommandLine;
            if cmdline_unicode.Buffer.is_null() || cmdline_unicode.Length == 0 {
                return Err(anyhow::anyhow!("Empty command line for pid {}", pid));
            }
            let byte_len = cmdline_unicode.Length as usize;
            // Defensive cap: a sane command line is well under 32 KB.
            // Anything larger means a bad read, not a real cmdline.
            if byte_len > 64 * 1024 {
                return Err(anyhow::anyhow!(
                    "Command line for pid {} unexpectedly large: {} bytes",
                    pid,
                    byte_len
                ));
            }
            let wchar_count = byte_len / 2;
            let mut buf: Vec<u16> = vec![0u16; wchar_count];
            ReadProcessMemory(
                handle,
                cmdline_unicode.Buffer.as_ptr() as *const _,
                buf.as_mut_ptr() as *mut _,
                byte_len,
                None,
            )
            .map_err(|e| anyhow::anyhow!("ReadProcessMemory(cmdline pid={}): {}", pid, e))?;

            Ok(String::from_utf16_lossy(&buf))
        })();

        let _ = CloseHandle(handle);
        result
    }
}

/// Set the current thread's description (name) via Win32 SetThreadDescription.
/// This makes the thread identifiable in debuggers and in our thread dump command.
pub fn set_thread_name(name: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadDescription};

    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = SetThreadDescription(GetCurrentThread(), PCWSTR(wide.as_ptr()));
    }
}

/// Create a Windows Job Object with KILL_ON_JOB_CLOSE and assign this
/// process to it. Any child process we spawn (TTS server, ACP CLI,
/// etc.) is implicitly added to the same job and gets killed when
/// this process exits — even on crash, force-kill from Task Manager,
/// or debugger detach. Without this, children can outlive their
/// parent and show up as orphaned processes.
///
/// The returned handle must stay alive for the lifetime of the
/// process. HANDLE is Copy with no Drop impl so it doesn't auto-close
/// when the local goes out of scope — which is what we want. The OS
/// holds the Job Object open (and its kill-on-close policy active)
/// as long as at least one handle remains unclosed.
///
/// Errors are logged but never propagated: if we can't create the
/// job, the app still runs; we just lose the orphan-cleanup guarantee.
/// Saved Job Object handle so `release_kill_on_exit_job_impl` can flip
/// the kill-on-close flag back off when we're about to hand off to the
/// updater installer. Without this, the spawned installer dies with us
/// (kill-on-close kicks in the moment our last handle to the job
/// closes, which the OS does on process exit).
static JOB_HANDLE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

pub fn install_kill_on_exit_job_impl() {
    use windows::core::PCWSTR;
    use windows::Win32::System::JobObjects::*;
    use windows::Win32::System::Threading::GetCurrentProcess;

    unsafe {
        let job = match CreateJobObjectW(None, PCWSTR::null()) {
            Ok(j) => j,
            Err(e) => {
                warn!("Failed to create Job Object: {}", e);
                return;
            }
        };

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;

        let set_ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if set_ok.is_err() {
            warn!("Failed to configure Job Object");
            return;
        }

        let current = GetCurrentProcess();
        match AssignProcessToJobObject(job, current) {
            Ok(_) => info!("✅ Job Object created — child processes will be killed on exit"),
            Err(e) => warn!("Failed to assign process to Job Object: {}", e),
        }
        // Save the handle for `release_kill_on_exit_job_impl`. The
        // handle is also deliberately leaked from this function's
        // perspective — the OS keeps the job alive as long as anyone
        // holds it open, and on normal exit the OS closes it for us.
        let _ = JOB_HANDLE.set(job.0 as usize);
    }
}

/// Disable kill-on-close on the Job Object so the next process we
/// exit doesn't take its children with it. Used right before the
/// updater plugin's `process::exit(0)` so the installer survives our
/// shutdown.
///
/// Effect: clears `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` in the job's
/// extended limit info. The job itself stays in place (and any future
/// children we spawn would still inherit it for `BREAKAWAY_OK`
/// purposes), but the OS-level kill-on-last-handle-close behaviour
/// is gone until `rearm_kill_on_exit_job_impl` restores it (the
/// updater does so when the install fails and we keep running).
pub fn release_kill_on_exit_job_impl() {
    use windows::Win32::System::JobObjects::JOB_OBJECT_LIMIT_BREAKAWAY_OK;

    // Keep BREAKAWAY_OK; drop KILL_ON_JOB_CLOSE.
    if set_job_limit_flags(JOB_OBJECT_LIMIT_BREAKAWAY_OK) {
        info!("Job Object kill-on-close released — child processes will survive exit");
    } else {
        warn!("Failed to release Job Object kill-on-close — installer may be killed with us");
    }
}

/// Undo `release_kill_on_exit_job_impl`: restore the startup limits
/// (`KILL_ON_JOB_CLOSE | BREAKAWAY_OK`) so children spawned after an
/// aborted exit (e.g. a failed update install) die with us again.
pub fn rearm_kill_on_exit_job_impl() {
    use windows::Win32::System::JobObjects::{
        JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    if set_job_limit_flags(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK) {
        info!("Job Object kill-on-close re-armed — child processes will be killed on exit");
    } else {
        warn!("Failed to re-arm Job Object kill-on-close — children may outlive a crash");
    }
}

/// Replace the saved job's basic limit flags. Returns false when no job
/// was created at startup or the update was rejected.
fn set_job_limit_flags(flags: windows::Win32::System::JobObjects::JOB_OBJECT_LIMIT) -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::*;

    let Some(&handle_usize) = JOB_HANDLE.get() else {
        warn!("Job Object handle not stored — cannot change kill-on-close");
        return false;
    };
    let job = HANDLE(handle_usize as *mut _);

    unsafe {
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = flags;

        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .is_ok()
    }
}

/// Test whether a `msedgewebview2.exe` command line belongs to a
/// previous kage instance — i.e. its `--user-data-dir=` argument
/// resolves to our EBWebView folder. Returns false for unrelated
/// WebView2 processes (VS Code, Slack, Teams, etc. — they all use
/// WebView2 and there can be dozens running).
///
/// Match strategy: WebView2 spawns its host process with the
/// `--user-data-dir=<path>` flag in the command line, sometimes
/// quoted, sometimes not. We look for the path string anywhere in
/// the command line — case-insensitively because Windows paths can
/// appear in any casing depending on how the spawning code referenced
/// them — and the `--user-data-dir=` substring may appear with or
/// without quotes depending on the WebView2 spawn flavour.
///
/// Pure logic kept private to the Windows process module so the
/// substring contract has a single home and a single test surface.
fn cmdline_matches_kage_webview(cmdline: &str, user_data_dir: &std::path::Path) -> bool {
    let dir_str = match user_data_dir.to_str() {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };
    // Normalize for case-insensitive substring match. Windows paths
    // are canonically mixed case but processes can pass them in any
    // casing; doing a tolower-style compare avoids false negatives.
    let cmd_lower = cmdline.to_ascii_lowercase();
    let dir_lower = dir_str.to_ascii_lowercase();
    cmd_lower.contains(&dir_lower)
}

#[cfg(test)]
mod tests {
    //! The native PEB-walking code in `cleanup_stale_processes_impl`
    //! delegates the match decision to `cmdline_matches_kage_webview`
    //! so the substring contract stays testable without spinning up
    //! Windows processes.

    use super::cmdline_matches_kage_webview;

    use super::{orphaned_webview_roots, WebviewProcess};

    const BROWSER: &str = r#""C:\EdgeWebView\msedgewebview2.exe" --embedded-browser-webview=1 --webview-exe-name=kage.exe --user-data-dir="C:\x\EBWebView" --noerrdialogs"#;
    const RENDERER: &str =
        r#""C:\EdgeWebView\msedgewebview2.exe" --type=renderer --user-data-dir="C:\x\EBWebView""#;

    #[test]
    fn webview_process_reads_role_and_host_from_its_command_line() {
        let b = WebviewProcess::from_cmdline(10, 1, BROWSER);
        assert!(b.is_browser);
        assert_eq!(b.host_exe.as_deref(), Some("kage.exe"));
        let r = WebviewProcess::from_cmdline(11, 10, RENDERER);
        assert!(!r.is_browser);
        assert_eq!(r.host_exe, None);
    }

    #[test]
    fn a_live_hosts_webviews_are_left_alone() {
        let procs = vec![
            WebviewProcess::from_cmdline(10, 1, BROWSER),
            WebviewProcess::from_cmdline(11, 10, RENDERER),
        ];
        assert!(orphaned_webview_roots(&procs, |_| true).is_empty());
    }

    #[test]
    fn a_dead_hosts_browser_is_the_only_root_to_kill() {
        // The renderer goes with its browser's tree kill.
        let procs = vec![
            WebviewProcess::from_cmdline(10, 1, BROWSER),
            WebviewProcess::from_cmdline(11, 10, RENDERER),
        ];
        assert_eq!(orphaned_webview_roots(&procs, |_| false), vec![10]);
    }

    #[test]
    fn a_child_whose_browser_is_gone_is_killed() {
        let procs = vec![WebviewProcess::from_cmdline(11, 10, RENDERER)];
        assert_eq!(orphaned_webview_roots(&procs, |_| true), vec![11]);
    }

    #[test]
    fn only_the_dead_hosts_tree_goes_when_two_hosts_share_the_folder() {
        let procs = vec![
            WebviewProcess::from_cmdline(10, 1, BROWSER),
            WebviewProcess::from_cmdline(11, 10, RENDERER),
            WebviewProcess::from_cmdline(20, 2, BROWSER),
            WebviewProcess::from_cmdline(21, 20, RENDERER),
        ];
        assert_eq!(orphaned_webview_roots(&procs, |b| b.parent == 1), vec![20]);
    }
    use std::path::PathBuf;

    #[test]
    fn cmdline_match_hits_when_user_data_dir_appears_verbatim() {
        let dir = PathBuf::from(r"C:\Users\foo\AppData\Local\kage\EBWebView");
        let cmd = format!(
            r#""C:\Program Files (x86)\Microsoft\EdgeWebView\msedgewebview2.exe" \
               --embedded-browser-webview=1 --user-data-dir="{}" --gpu-preferences=..."#,
            dir.display()
        );
        assert!(cmdline_matches_kage_webview(&cmd, &dir));
    }

    #[test]
    fn cmdline_match_is_case_insensitive() {
        // Process Explorer often shows path components in mixed case
        // depending on how the spawning code referenced them. We must
        // match regardless.
        let dir = PathBuf::from(r"C:\Users\foo\AppData\Local\kage\EBWebView");
        let cmd = r#"... --user-data-dir="C:\USERS\FOO\APPDATA\LOCAL\KAGE\EBWEBVIEW" ..."#;
        assert!(cmdline_matches_kage_webview(cmd, &dir));
    }

    #[test]
    fn cmdline_match_misses_when_user_data_dir_belongs_to_another_app() {
        // Many other apps also use WebView2 — VS Code, Slack, Teams, etc.
        // We must not kill those.
        let dir = PathBuf::from(r"C:\Users\foo\AppData\Local\kage\EBWebView");
        let other =
            r#"... --user-data-dir="C:\Users\foo\AppData\Local\Microsoft\VSCode\EBWebView" ..."#;
        assert!(!cmdline_matches_kage_webview(other, &dir));
    }

    #[test]
    fn cmdline_match_misses_when_path_is_a_partial_substring_of_kage() {
        // Kage isn't unique enough as a substring on its own. The match
        // must use the full user-data-dir path, not just "kage", to avoid
        // matching e.g. an unrelated tool that happens to have "kage" in
        // its data path.
        let dir = PathBuf::from(r"C:\Users\foo\AppData\Local\kage\EBWebView");
        let other =
            r#"... --user-data-dir="C:\Users\foo\AppData\Local\kage-old-cli\EBWebView" ..."#;
        assert!(!cmdline_matches_kage_webview(other, &dir));
    }

    #[test]
    fn cmdline_match_returns_false_for_empty_user_data_dir() {
        // Defensive: a Path::to_str() failure (e.g. non-UTF-8 path)
        // returns None inside the helper. We never want a falsy match
        // to silently kill arbitrary processes.
        let dir = PathBuf::new();
        let cmd = "anything --user-data-dir=foo";
        assert!(!cmdline_matches_kage_webview(cmd, &dir));
    }
}
