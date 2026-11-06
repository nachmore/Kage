// macOS app uninstall: move the .app bundle to the Trash (see
// `os::app_uninstall` for the contract).

use crate::os::app_uninstall::{mac_bundle_is_protected, UninstallError, UninstallOutcome};
use crate::os::file_ops::TrashError;
use objc2::msg_send;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::NSString;
use std::os::macos::fs::MetadataExt;
use std::path::Path;

/// `SF_RESTRICTED` from <sys/stat.h>: the file is protected by SIP.
const SF_RESTRICTED: u32 = 0x0008_0000;

fn sip_restricted(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.st_flags() & SF_RESTRICTED != 0)
}

pub fn is_protected_app_impl(bundle: &Path) -> bool {
    let canonical = std::fs::canonicalize(bundle).unwrap_or_else(|_| bundle.to_path_buf());
    mac_bundle_is_protected(
        &canonical,
        sip_restricted(bundle) || sip_restricted(&canonical),
    )
}

pub fn bundle_identifier_impl(bundle: &Path) -> Option<String> {
    let path = bundle.to_str()?;
    autoreleasepool(|_| {
        let cls = AnyClass::get(c"NSBundle")?;
        let s = NSString::from_str(path);
        // SAFETY: +[NSBundle bundleWithPath:] takes an NSString and returns
        // a nullable NSBundle; -bundleIdentifier returns a nullable NSString.
        let b: Option<Retained<AnyObject>> = unsafe { msg_send![cls, bundleWithPath: &*s] };
        let b = b?;
        let id: Option<Retained<NSString>> = unsafe { msg_send![&*b, bundleIdentifier] };
        id.map(|s| s.to_string())
    })
}

/// Whether a running app was launched from `bundle` (same canonical path).
fn is_running(bundle: &Path) -> bool {
    use objc2_app_kit::NSRunningApplication;
    let Some(id) = bundle_identifier_impl(bundle) else {
        return false;
    };
    let target = std::fs::canonicalize(bundle).unwrap_or_else(|_| bundle.to_path_buf());
    autoreleasepool(|_| {
        let apps =
            NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(&id));
        (0..apps.len()).any(|i| {
            let app = apps.objectAtIndex(i);
            app.bundleURL()
                .and_then(|u| u.path())
                .map(|p| {
                    std::fs::canonicalize(p.to_string()).unwrap_or_else(|_| p.to_string().into())
                })
                .is_some_and(|p| p == target)
        })
    })
}

pub fn uninstall_impl(path: &Path, _name: &str) -> Result<UninstallOutcome, UninstallError> {
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("app"))
    {
        return Err(UninstallError::Unsupported);
    }
    if is_protected_app_impl(path) {
        return Err(UninstallError::Protected);
    }
    if is_running(path) {
        return Err(UninstallError::AppRunning);
    }
    match crate::os::file_ops::move_to_trash(path) {
        Ok(()) => {
            log::info!("[uninstall] moved {} to the Trash", path.display());
            Ok(UninstallOutcome::MovedToTrash)
        }
        Err(TrashError::PermissionDenied(reason)) => {
            // Root-owned (pkg/MDM installs) and App Store bundles: Finder can
            // still remove them with an admin password, Kage can't.
            log::info!(
                "[uninstall] macOS denied trashing {} ({reason}); revealing in Finder",
                path.display()
            );
            crate::os::reveal_in_file_manager(&path.to_string_lossy())
                .map_err(|e| UninstallError::Failed(e.to_string()))?;
            Ok(UninstallOutcome::RevealedForManualRemoval)
        }
        Err(e) => Err(UninstallError::Failed(e.to_string())),
    }
}
