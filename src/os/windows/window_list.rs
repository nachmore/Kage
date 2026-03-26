// Windows window enumeration using Win32 API

use crate::os::window_list::WindowInfo;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

// Win32 FFI
#[allow(non_snake_case)]
extern "system" {
    fn EnumWindows(lpEnumFunc: extern "system" fn(isize, isize) -> i32, lParam: isize) -> i32;
    fn IsWindowVisible(hwnd: isize) -> i32;
    fn GetWindowTextW(hwnd: isize, lpString: *mut u16, nMaxCount: i32) -> i32;
    fn GetWindowTextLengthW(hwnd: isize) -> i32;
    fn GetWindowThreadProcessId(hwnd: isize, lpdwProcessId: *mut u32) -> u32;
    fn SetForegroundWindow(hwnd: isize) -> i32;
    fn ShowWindow(hwnd: isize, nCmdShow: i32) -> i32;
    fn IsIconic(hwnd: isize) -> i32;
    fn GetWindow(hwnd: isize, uCmd: u32) -> isize;
    fn GetWindowLongW(hwnd: isize, nIndex: i32) -> i32;
}

const GWL_EXSTYLE: i32 = -20;
const WS_EX_TOOLWINDOW: i32 = 0x00000080;
const GW_OWNER: u32 = 4;
const SW_RESTORE: i32 = 9;

fn get_window_title(hwnd: isize) -> Option<String> {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 { return None; }
        let mut buf = vec![0u16; (len + 1) as usize];
        let copied = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if copied <= 0 { return None; }
        let title = OsString::from_wide(&buf[..copied as usize])
            .to_string_lossy()
            .to_string();
        if title.trim().is_empty() { return None; }
        Some(title)
    }
}

fn get_process_info(pid: u32) -> (String, String) {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    #[allow(non_snake_case)]
    extern "system" {
        fn OpenProcess(dwDesiredAccess: u32, bInheritHandle: i32, dwProcessId: u32) -> *mut std::ffi::c_void;
        fn CloseHandle(hObject: *mut std::ffi::c_void) -> i32;
        fn QueryFullProcessImageNameW(
            hProcess: *mut std::ffi::c_void,
            dwFlags: u32,
            lpExeName: *mut u16,
            lpdwSize: *mut u32,
        ) -> i32;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() { return (String::new(), String::new()); }
        let mut buf = vec![0u16; 260];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(handle);
        if ok == 0 { return (String::new(), String::new()); }
        let path = OsString::from_wide(&buf[..size as usize])
            .to_string_lossy()
            .to_string();
        let name = std::path::Path::new(&path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        (name, path)
    }
}

/// Check if a window is a real top-level app window (not a tool window, not owned)
fn is_app_window(hwnd: isize) -> bool {
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        if ex_style & WS_EX_TOOLWINDOW != 0 { return false; }
        let owner = GetWindow(hwnd, GW_OWNER);
        if owner != 0 { return false; }
        true
    }
}

struct EnumState {
    windows: Vec<(WindowInfo, String)>, // (info, exe_path) — path used for icon extraction
}

extern "system" fn enum_callback(hwnd: isize, lparam: isize) -> i32 {
    unsafe {
        if IsWindowVisible(hwnd) == 0 { return 1; }
        if !is_app_window(hwnd) { return 1; }

        let title = match get_window_title(hwnd) {
            Some(t) => t,
            None => return 1,
        };

        // Skip our own window
        if title.contains("Kage") { return 1; }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let (process_name, exe_path) = if pid > 0 { get_process_info(pid) } else { (String::new(), String::new()) };

        let state = &mut *(lparam as *mut EnumState);
        state.windows.push((WindowInfo {
            title,
            process_name,
            handle: hwnd as u64,
            icon_base64: None,
        }, exe_path));
    }
    1 // continue enumeration
}

use std::collections::HashMap;
use std::sync::Mutex;

// Cache icons by executable path — same exe always has the same icon
static ICON_CACHE: std::sync::LazyLock<Mutex<HashMap<String, Option<String>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

// Cache icons by process name (lowercase) — for quick lookup by name
static ICON_BY_NAME: std::sync::LazyLock<Mutex<HashMap<String, String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn list_windows_impl() -> Vec<WindowInfo> {
    let mut state = EnumState { windows: Vec::new() };
    unsafe {
        EnumWindows(enum_callback, &mut state as *mut EnumState as isize);
    }

    let mut cache = ICON_CACHE.lock().unwrap();
    let mut name_cache = ICON_BY_NAME.lock().unwrap();

    for (win, exe_path) in &mut state.windows {
        if exe_path.is_empty() { continue; }
        let icon = cache.entry(exe_path.clone()).or_insert_with(|| {
            crate::os::icon::extract_icon_base64(exe_path)
        });
        win.icon_base64 = icon.clone();
        // Also cache by process name for lookup
        if let Some(ref icon_str) = *icon {
            if !win.process_name.is_empty() {
                name_cache.entry(win.process_name.to_lowercase()).or_insert_with(|| icon_str.clone());
            }
        }
    }

    state.windows.into_iter().map(|(w, _)| w).collect()
}

/// Look up a cached icon by process name (e.g. "winword", "chrome").
/// Returns the base64 data URI if found, or None.
/// If the cache is empty, triggers a window enumeration to populate it.
pub fn get_icon_by_process_name(name: &str) -> Option<String> {
    let cache = ICON_BY_NAME.lock().unwrap();
    if let Some(icon) = cache.get(&name.to_lowercase()) {
        return Some(icon.clone());
    }
    // Cache miss — if the cache is empty, populate it by listing windows
    let is_empty = cache.is_empty();
    drop(cache);

    if is_empty {
        // Prime the cache by enumerating windows (populates both ICON_CACHE and ICON_BY_NAME)
        let _ = list_windows_impl();
        let cache = ICON_BY_NAME.lock().unwrap();
        return cache.get(&name.to_lowercase()).cloned();
    }

    None
}

pub fn focus_window_impl(handle: u64) -> Result<(), String> {
    let hwnd = handle as isize;
    unsafe {
        // Restore if minimized
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd) == 0 {
            return Err("Failed to set foreground window".to_string());
        }
    }
    Ok(())
}

/// Get the foreground window's title and process name.
/// Returns None if no foreground window or it's our own window.
pub fn get_foreground_window_info() -> Option<(String, String)> {
    #[allow(non_snake_case)]
    extern "system" {
        fn GetForegroundWindow() -> isize;
    }

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == 0 { return None; }

        let title = get_window_title(hwnd)?;
        if title.contains("Kage") { return None; }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let (process_name, _) = if pid > 0 { get_process_info(pid) } else { (String::new(), String::new()) };

        Some((title, process_name))
    }
}
