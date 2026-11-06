// macOS file operations for launcher results: file URLs on NSPasteboard,
// NSFileManager trash. Raw objc runtime calls, like `clipboard.rs` — the
// objc2-foundation NSFileManager/NSURL features aren't enabled.

use crate::os::file_ops::{Preflight, TrashError};
use objc2::msg_send;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2_foundation::NSString;
use std::path::{Path, PathBuf};
use std::ptr;

/// `NSCocoaErrorDomain` codes that mean "not allowed" rather than "broken".
const NS_FILE_READ_NO_PERMISSION: isize = 257;
const NS_FILE_WRITE_NO_PERMISSION: isize = 513;
const NS_FILE_WRITE_VOLUME_READ_ONLY: isize = 642;
/// POSIX EPERM / EACCES / EROFS.
const POSIX_DENIED: [isize; 3] = [1, 13, 30];

fn file_url(path: &Path) -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"NSURL")?;
    let s = NSString::from_str(path.to_str()?);
    // SAFETY: +[NSURL fileURLWithPath:] takes an NSString and returns a
    // non-null NSURL.
    unsafe { msg_send![cls, fileURLWithPath: &*s] }
}

/// Copy only (macOS has no file cut), so nothing for `_preflight` to guard.
pub fn copy_files_to_clipboard_impl(
    paths: &[PathBuf],
    cut: bool,
    _preflight: Option<Preflight>,
) -> Result<(), crate::os::file_ops::ClipboardFilesError> {
    if cut {
        return Err(anyhow::anyhow!("macOS has no cut for files").into());
    }
    autoreleasepool(|_| {
        let pb_cls = AnyClass::get(c"NSPasteboard")
            .ok_or_else(|| anyhow::anyhow!("NSPasteboard unavailable"))?;
        let array_cls = AnyClass::get(c"NSMutableArray")
            .ok_or_else(|| anyhow::anyhow!("NSMutableArray unavailable"))?;
        // SAFETY: +[NSMutableArray array] returns an empty mutable array.
        let urls: Retained<AnyObject> = unsafe { msg_send![array_cls, array] };
        for path in paths {
            let url = file_url(path)
                .ok_or_else(|| anyhow::anyhow!("not a valid file path: {}", path.display()))?;
            // SAFETY: -addObject: takes a non-nil object.
            let _: () = unsafe { msg_send![&*urls, addObject: &*url] };
        }
        // SAFETY: +generalPasteboard returns the shared NSPasteboard.
        let pb: Option<Retained<AnyObject>> = unsafe { msg_send![pb_cls, generalPasteboard] };
        let pb = pb.ok_or_else(|| anyhow::anyhow!("no general pasteboard"))?;
        // SAFETY: -clearContents returns the new change count (NSInteger);
        // -writeObjects: takes an NSArray of NSPasteboardWriting objects
        // (NSURL is one) and returns BOOL.
        let _: isize = unsafe { msg_send![&*pb, clearContents] };
        let ok: bool = unsafe { msg_send![&*pb, writeObjects: &*urls] };
        if ok {
            Ok(())
        } else {
            anyhow::bail!("NSPasteboard refused the file URLs")
        }
    })
    .map_err(Into::into)
}

/// `preflight` runs on this thread right before `trashItemAtURL:`.
pub fn move_to_trash_impl(path: &Path, preflight: Preflight) -> Result<(), TrashError> {
    autoreleasepool(|_| {
        let fm_cls = AnyClass::get(c"NSFileManager")
            .ok_or_else(|| TrashError::Other("NSFileManager unavailable".into()))?;
        let url = file_url(path)
            .ok_or_else(|| TrashError::Other(format!("not a valid path: {}", path.display())))?;
        // SAFETY: +defaultManager returns the shared NSFileManager.
        let fm: Retained<AnyObject> = unsafe { msg_send![fm_cls, defaultManager] };
        preflight().map_err(TrashError::Refused)?;
        // SAFETY: -trashItemAtURL:resultingItemURL:error: takes an NSURL, a
        // nullable `NSURL **` out-parameter (NULL: we don't need it) and
        // the error out-parameter `_` turns into the Result.
        let result: Result<(), Retained<NSObject>> = unsafe {
            msg_send![
                &*fm,
                trashItemAtURL: &*url,
                resultingItemURL: ptr::null_mut::<*mut AnyObject>(),
                error: _
            ]
        };
        result.map_err(|err| classify_error(&err))
    })
}

fn classify_error(err: &NSObject) -> TrashError {
    // SAFETY: the error object is an NSError: -code returns NSInteger,
    // -domain and -localizedDescription return non-null NSStrings.
    let (code, domain, text) = unsafe {
        let code: isize = msg_send![err, code];
        let domain: Retained<NSString> = msg_send![err, domain];
        let text: Retained<NSString> = msg_send![err, localizedDescription];
        (code, domain.to_string(), text.to_string())
    };
    let denied = match domain.as_str() {
        "NSCocoaErrorDomain" => matches!(
            code,
            NS_FILE_READ_NO_PERMISSION
                | NS_FILE_WRITE_NO_PERMISSION
                | NS_FILE_WRITE_VOLUME_READ_ONLY
        ),
        "NSPOSIXErrorDomain" => POSIX_DENIED.contains(&code),
        _ => false,
    };
    log::warn!("[file_ops] trash failed: {domain} {code}: {text}");
    if denied {
        TrashError::PermissionDenied(text)
    } else {
        TrashError::Other(text)
    }
}

pub fn trash_available_impl() -> bool {
    true
}

pub fn run_as_admin_impl(_path: &Path) -> anyhow::Result<bool> {
    anyhow::bail!("run as administrator is Windows-only")
}

pub fn show_properties_impl(_path: &Path) -> anyhow::Result<()> {
    anyhow::bail!("the Properties dialog is Windows-only")
}

pub fn file_id_impl(path: &Path) -> Option<crate::os::file_ops::FileId> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(crate::os::file_ops::FileId {
        volume: meta.dev(),
        index: meta.ino(),
    })
}

pub fn is_link_impl(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

pub fn on_disk_name_impl(_path: &Path) -> Option<std::ffi::OsString> {
    None
}
