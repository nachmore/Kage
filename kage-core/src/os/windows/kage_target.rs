// Windows side of `os::kage_target`.

use std::collections::HashMap;

use windows::Win32::Foundation::{CloseHandle, HWND, POINT};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetForegroundWindow, GetWindowThreadProcessId, WindowFromPoint, GA_ROOT,
};

/// The image name WebView2 content runs under. Its UI Automation elements
/// report this process, not the app hosting the web view.
pub(crate) const WEBVIEW2_PROCESS: &str = "msedgewebview2.exe";

pub fn is_kage_process_impl(pid: u32) -> bool {
    super::process::get_process_name_impl(pid)
        .is_some_and(|name| crate::os::kage_target::is_kage_executable(&name))
}

fn is_webview2(pid: u32) -> bool {
    super::process::get_process_name_impl(pid).as_deref() == Some(WEBVIEW2_PROCESS)
}

/// Every process's parent, from one snapshot.
fn parent_pids() -> HashMap<u32, u32> {
    let mut parents = HashMap::new();
    // SAFETY: a snapshot handle closed below; `entry` is a local whose
    // dwSize says how big it is.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return parents;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    parents
}

/// Whether `pid` is a WebView2 process of Kage's: msedgewebview2.exe
/// started by Kage (its browser process) or by such a process (a
/// renderer). UI Automation reports a web view's content under that
/// process wherever its window is — in front, behind another window, or
/// off screen — so this, unlike the window under a point, still holds
/// when Kage's window is covered.
pub fn is_kage_webview_impl(pid: u32) -> bool {
    if !is_webview2(pid) {
        return false;
    }
    let parents = parent_pids();
    let mut current = pid;
    for _ in 0..3 {
        let Some(&parent) = parents.get(&current) else {
            return false;
        };
        if parent == 0 || parent == current {
            return false;
        }
        if is_kage_process_impl(parent) {
            return true;
        }
        if !is_webview2(parent) {
            return false;
        }
        current = parent;
    }
    false
}

fn window_pid(hwnd: HWND) -> Option<u32> {
    if hwnd.is_invalid() {
        return None;
    }
    let mut pid = 0u32;
    // SAFETY: a window handle and an out-pointer to a local.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32)) };
    (pid != 0).then_some(pid)
}

pub fn keyboard_target_pids_impl() -> Vec<u32> {
    // SAFETY: no arguments.
    window_pid(unsafe { GetForegroundWindow() })
        .into_iter()
        .collect()
}

/// The window under the point and its top-level window (a WebView2 child
/// window belongs to msedgewebview2.exe; its root belongs to the app).
pub fn window_owners_at_impl(x: i32, y: i32) -> Vec<u32> {
    // SAFETY: plain window queries.
    unsafe {
        let hwnd = WindowFromPoint(POINT { x, y });
        let root = GetAncestor(hwnd, GA_ROOT);
        [window_pid(hwnd), window_pid(root)]
            .into_iter()
            .flatten()
            .collect()
    }
}
