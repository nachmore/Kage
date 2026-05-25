// Windows process management

use anyhow::Result;
use log::{info, warn};
use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Check if a PID belongs to a kage-related process (kage-cli, node, etc.)
/// Returns the process name if found, None if the process doesn't exist or isn't ours.
pub fn get_process_name_impl(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 260];
        let mut size = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            windows::Win32::System::Threading::PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        if result.is_ok() && size > 0 {
            let path = String::from_utf16_lossy(&buf[..size as usize]);
            // Extract just the filename from the full path
            let name = path.rsplit('\\').next().unwrap_or(&path).to_lowercase();
            Some(name)
        } else {
            None
        }
    }
}

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

const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;

/// Spawn a process that is detached from our Job Object so it survives
/// when Kage exits. Use this for user-facing launches (apps,
/// URLs, explorer, system commands) — NOT for internal child processes
/// like kage-cli or TTS servers that should die with us.
pub fn spawn_detached_impl(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    cmd.creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW);
    cmd.spawn()
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

/// Find `msedgewebview2.exe` processes whose command line points at the
/// given user data folder, and kill them. Returns the number killed.
///
/// Used by `startup::ensure_webview_directory_writable` (via the
/// platform-agnostic `os::cleanup_stale_processes`) to clean up
/// orphan WebView2 host processes left behind by a previous kage that
/// didn't shut down cleanly. WebView2 enforces single-writer semantics
/// on its user data directory; an orphan child holding the lock makes
/// the next launch fail to render the floating window at all.
///
/// # How it finds them
///
/// 1. `CreateToolhelp32Snapshot` enumerates every process by name.
/// 2. For each `msedgewebview2.exe`, `OpenProcess(PROCESS_QUERY_LIMITED_
///    INFORMATION | PROCESS_VM_READ)` gets a handle.
/// 3. `NtQueryInformationProcess(ProcessBasicInformation)` returns the
///    PEB base address.
/// 4. `ReadProcessMemory` walks PEB → ProcessParameters → CommandLine
///    (a UNICODE_STRING with a buffer pointer + length in the remote
///    address space).
/// 5. A second `ReadProcessMemory` pulls the command-line bytes.
/// 6. We compare the lowercased command line against the lowercased
///    user-data-dir path. Any match gets `kill_process_impl`'d.
///
/// # Safety / fallibility
///
/// `NtQueryInformationProcess` and the PEB layout are documented as
/// "reserved" by Microsoft, but the offsets we use (PEB
/// → ProcessParameters → CommandLine) are stable across every
/// supported Windows version and are what Process Explorer / sysinfo /
/// htop-equivalent tools also rely on. If a future Windows ever
/// changes the layout, our match silently returns false (no spurious
/// kills) and the caller surfaces the "frontend not ready" warning as
/// before.
///
/// We deliberately tolerate every kind of failure (handle open denied,
/// remote read truncated, processes that died mid-enumeration, etc.) —
/// this is a best-effort cleanup and any error just means "skip this
/// PID and move on."
pub fn cleanup_stale_processes_impl(user_data_dir: &std::path::Path) -> usize {
    let pids = match enumerate_processes_by_name("msedgewebview2.exe") {
        Ok(v) => v,
        Err(e) => {
            warn!(
                "Failed to enumerate processes while looking for orphan WebView2: {}",
                e
            );
            return 0;
        }
    };

    let mut matches: Vec<u32> = Vec::new();
    for pid in pids {
        match read_process_command_line(pid) {
            Ok(cmdline) => {
                if cmdline_matches_kage_webview(&cmdline, user_data_dir) {
                    matches.push(pid);
                }
            }
            Err(_) => {
                // Common: PID died between enumeration and OpenProcess,
                // or we don't have rights. Skip silently — we're only
                // interested in processes we CAN read, and orphans we
                // spawned ourselves are by definition readable by us.
            }
        }
    }

    if matches.is_empty() {
        return 0;
    }

    log::warn!(
        "Found {} orphan kage-WebView2 process(es): {:?} — killing",
        matches.len(),
        matches
    );

    let mut killed = 0;
    for pid in matches {
        if kill_process_impl(pid) {
            killed += 1;
        }
    }
    killed
}

/// Enumerate every process whose image name (case-insensitive)
/// matches `name`. Returns the list of PIDs. Used by
/// `cleanup_stale_processes_impl`.
fn enumerate_processes_by_name(name: &str) -> Result<Vec<u32>> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let target = name.to_ascii_lowercase();
    let mut pids: Vec<u32> = Vec::new();

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .map_err(|e| anyhow::anyhow!("CreateToolhelp32Snapshot failed: {}", e))?;

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                // szExeFile is a fixed-size [u16; 260] null-terminated UTF-16
                // string. Read until first null, lowercase, compare.
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let exe = String::from_utf16_lossy(&entry.szExeFile[..len]).to_ascii_lowercase();
                if exe == target {
                    pids.push(entry.th32ProcessID);
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }

    Ok(pids)
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
/// is gone.
///
/// We don't restore the flag — by the time this is called we're
/// committed to exiting in a moment, and any orphan-cleanup
/// guarantees we lose are already moot.
pub fn release_kill_on_exit_job_impl() {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::*;

    let Some(&handle_usize) = JOB_HANDLE.get() else {
        warn!("Job Object handle not stored — cannot release kill-on-close");
        return;
    };
    let job = HANDLE(handle_usize as *mut _);

    unsafe {
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        // Keep BREAKAWAY_OK; drop KILL_ON_JOB_CLOSE.
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_BREAKAWAY_OK;

        let set_ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if set_ok.is_err() {
            warn!("Failed to release Job Object kill-on-close — installer may be killed with us");
        } else {
            info!("Job Object kill-on-close released — child processes will survive exit");
        }
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
