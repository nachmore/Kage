// Windows file operations for launcher results: CF_HDROP clipboard (copy
// and cut), Recycle Bin via SHFileOperation, and the shell's "runas" /
// "properties" verbs.

use crate::os::file_ops::{build_hdrop_payload, preferred_drop_effect, TrashError};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::UI::Shell::{
    SHFileOperationW, ShellExecuteExW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_WANTNUKEWARNING,
    FO_DELETE, SEE_MASK_INVOKEIDLIST, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, SHFILEOPSTRUCTW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// SHFileOperation's "access denied" results: ERROR_ACCESS_DENIED and the
/// legacy DE_ACCESSDENIEDSRC.
const SHFILEOP_DENIED: [i32; 2] = [5, 0x78];

pub(crate) fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Run `f` on a fresh thread with a single-threaded COM apartment, as the
/// shell APIs require (verbs may be implemented by COM shell extensions).
/// A dedicated thread keeps the apartment off the async runtime's pooled
/// threads, which other code initialises differently.
pub(crate) fn with_com_sta<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> anyhow::Result<T> {
    std::thread::Builder::new()
        .name("shell-op".into())
        .spawn(move || {
            use windows::Win32::System::Com::{
                CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
            };
            let hr =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
            let out = f();
            if hr.is_ok() {
                unsafe { CoUninitialize() };
            }
            out
        })?
        .join()
        .map_err(|_| anyhow::anyhow!("shell operation thread panicked"))
}

/// `ShellExecuteExW(verb, file, params)`. `Ok(false)` means the user
/// cancelled (declined the UAC prompt).
pub(crate) fn shell_execute(
    verb: Option<&str>,
    file: &std::ffi::OsStr,
    params: Option<&std::ffi::OsStr>,
    mask: u32,
) -> anyhow::Result<bool> {
    let verb_w = verb.map(|v| wide(std::ffi::OsStr::new(v)));
    let file_w = wide(file);
    let params_w = params.map(wide);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: mask,
        lpVerb: verb_w
            .as_ref()
            .map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr())),
        lpFile: PCWSTR(file_w.as_ptr()),
        lpParameters: params_w
            .as_ref()
            .map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..unsafe { std::mem::zeroed() }
    };
    match unsafe { ShellExecuteExW(&mut info) } {
        Ok(()) => Ok(true),
        Err(e) if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) => Ok(false),
        Err(e) => Err(anyhow::anyhow!("ShellExecuteExW failed: {e}")),
    }
}

pub fn copy_files_to_clipboard_impl(paths: &[PathBuf], cut: bool) -> anyhow::Result<()> {
    let strings: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let refs: Vec<&str> = strings.iter().map(String::as_str).collect();
    let hdrop = build_hdrop_payload(&refs);
    let effect = preferred_drop_effect(cut).to_le_bytes();
    let effect_format = super::clipboard::register_clipboard_format("Preferred DropEffect");
    let mut formats: Vec<(u32, &[u8])> = vec![(super::clipboard::CF_HDROP, &hdrop)];
    if effect_format != 0 {
        formats.push((effect_format, &effect));
    } else if cut {
        // Without the DropEffect a paste copies — not what "Cut" promised.
        anyhow::bail!("couldn't register the Preferred DropEffect clipboard format");
    }
    if super::clipboard::set_clipboard_formats(&formats) {
        Ok(())
    } else {
        anyhow::bail!("couldn't write the file list to the clipboard")
    }
}

pub fn move_to_trash_impl(path: &Path) -> Result<(), TrashError> {
    let path = path.to_path_buf();
    with_com_sta(move || {
        // pFrom is a list: each path NUL-terminated, the list ends with
        // one more NUL.
        let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
        from.extend([0, 0]);
        let mut op = SHFILEOPSTRUCTW {
            hwnd: HWND::default(),
            wFunc: FO_DELETE,
            pFrom: PCWSTR(from.as_ptr()),
            pTo: PCWSTR::null(),
            // ALLOWUNDO sends it to the Recycle Bin; WANTNUKEWARNING makes
            // the shell ask before deleting something that can't be
            // recycled (network share, oversized) instead of silently
            // destroying it, which NOCONFIRMATION alone would do.
            fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING).0 as u16,
            ..Default::default()
        };
        let rc = unsafe { SHFileOperationW(&mut op) };
        if rc != 0 {
            log::warn!("[file_ops] SHFileOperationW failed: {rc:#x}");
            return Err(if SHFILEOP_DENIED.contains(&rc) {
                TrashError::PermissionDenied(format!("error {rc:#x}"))
            } else {
                TrashError::Other(format!("error {rc:#x}"))
            });
        }
        // Copy the field out first: the struct is packed on 32-bit x86, so
        // calling a method on the field in place would borrow it unaligned.
        let aborted: windows::core::BOOL = op.fAnyOperationsAborted;
        if aborted.as_bool() {
            return Err(TrashError::Other("cancelled".into()));
        }
        Ok(())
    })
    .map_err(|e| TrashError::Other(e.to_string()))?
}

pub fn trash_available_impl() -> bool {
    true
}

pub fn run_as_admin_impl(path: &Path) -> anyhow::Result<bool> {
    let file = path.as_os_str().to_os_string();
    with_com_sta(move || shell_execute(Some("runas"), &file, None, SEE_MASK_NOASYNC))?
}

pub fn show_properties_impl(path: &Path) -> anyhow::Result<()> {
    // INVOKEIDLIST routes the verb through the item's context menu, which
    // is where "properties" lives. No NOASYNC: the property sheet runs
    // modeless on the calling (main, message-pumping) thread.
    shell_execute(
        Some("properties"),
        path.as_os_str(),
        None,
        SEE_MASK_INVOKEIDLIST,
    )
    .map(|_| ())
}
