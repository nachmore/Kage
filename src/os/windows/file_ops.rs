// Windows file operations for launcher results: CF_HDROP clipboard (copy
// and cut), Recycle Bin via SHFileOperation, and the shell's "runas" /
// "properties" verbs.

use crate::os::file_ops::{
    build_hdrop_payload, exceeds_path_limit, preferred_drop_effect, trash_path_limit,
    ClipboardFilesError, Preflight, TrashError,
};
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
/// threads, which other code initialises differently. Scoped, so `f` may
/// borrow — a preflight check runs inside `f`, on the same thread as the
/// call it guards.
pub(crate) fn with_com_sta<T: Send>(f: impl FnOnce() -> T + Send) -> anyhow::Result<T> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("shell-op".into())
            .spawn_scoped(scope, move || {
                use windows::Win32::System::Com::{
                    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
                    COINIT_DISABLE_OLE1DDE,
                };
                let hr = unsafe {
                    CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)
                };
                let out = f();
                if hr.is_ok() {
                    unsafe { CoUninitialize() };
                }
                out
            })?
            .join()
            .map_err(|_| anyhow::anyhow!("shell operation thread panicked"))
    })
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

/// `preflight` runs with the clipboard open, right before it's replaced
/// (`set_clipboard_formats`). Pasting a Cut later moves whatever the paths
/// name then — `CF_HDROP` is path-based by design (see `os::file_ops`).
pub fn copy_files_to_clipboard_impl(
    paths: &[PathBuf],
    cut: bool,
    preflight: Option<Preflight>,
) -> Result<(), ClipboardFilesError> {
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
        return Err(
            anyhow::anyhow!("couldn't register the Preferred DropEffect clipboard format").into(),
        );
    }
    super::clipboard::set_clipboard_formats(&formats, preflight)
}

/// `preflight` runs on the shell thread, right before `SHFileOperationW`.
/// That API is limited to `MAX_PATH`; a longer path is refused up front
/// (`TrashError::PathTooLong`) rather than handed to it.
pub fn move_to_trash_impl(path: &Path, preflight: Preflight) -> Result<(), TrashError> {
    if exceeds_path_limit(path, trash_path_limit()) {
        return Err(TrashError::PathTooLong);
    }
    with_com_sta(move || {
        preflight().map_err(TrashError::Refused)?;
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

pub fn file_id_impl(path: &Path) -> Option<crate::os::file_ops::FileId> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    // No access rights (attributes only), every share mode, the reparse
    // point itself rather than its target, and BACKUP_SEMANTICS so folders
    // open too.
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(path)
        .ok()?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.ok()?;
    Some(crate::os::file_ops::FileId {
        volume: u64::from(info.dwVolumeSerialNumber),
        index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

pub fn is_link_impl(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    meta.file_type().is_symlink() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

pub fn on_disk_name_impl(path: &Path) -> Option<std::ffi::OsString> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW};
    // FindFirstFile treats `*` and `?` as wildcards; neither is legal in a
    // Windows file name, so such a path can't name one item.
    if path.to_string_lossy().contains(['*', '?']) {
        return None;
    }
    let w = wide(path.as_os_str());
    let mut data = WIN32_FIND_DATAW::default();
    // Lists the directory entry itself, so a link isn't followed.
    let handle = unsafe { FindFirstFileW(PCWSTR(w.as_ptr()), &mut data) }.ok()?;
    let _ = unsafe { FindClose(handle) };
    let len = data
        .cFileName
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(data.cFileName.len());
    (len > 0).then(|| std::ffi::OsString::from_wide(&data.cFileName[..len]))
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
