// Windows side of `os::kage_target`.

use windows::Win32::Foundation::{HWND, POINT};
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
