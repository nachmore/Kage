//! macOS TCC status + repair. See `os::permissions` for what each
//! permission backs and why repair exists.
//!
//! Status checks use the non-prompting preflight APIs, so they're safe to
//! call on every settings-window focus. Repair shells out to
//! `/usr/bin/tccutil` (no shell, fixed absolute path, validated args) and
//! then calls the matching *request* API, which re-adds the running binary
//! to the System Settings list and may show macOS's own prompt. All of
//! these are thread-safe XPC calls to tccd (prompting happens out of
//! process, asynchronously), so they run fine off the main thread — the
//! hotkey and sidecar code already call `AXIsProcessTrustedWithOptions`
//! from worker threads.

use crate::os::permissions::{is_app_bundle_executable, Permission, PermissionStatus, RepairError};
use accessibility_sys as ax;
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_graphics::access::ScreenCaptureAccess;
use std::process::Command;

const TCCUTIL: &str = "/usr/bin/tccutil";

// core-graphics 0.25 binds the screen-capture pair (`access.rs`) but not
// the listen-event pair. Both are macOS 10.15+ (we target 11.0).
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

pub fn status_impl(permission: Permission) -> PermissionStatus {
    let granted = match permission {
        // Plain AXIsProcessTrusted never prompts.
        Permission::Accessibility => unsafe { ax::AXIsProcessTrusted() },
        Permission::InputMonitoring => unsafe { CGPreflightListenEventAccess() },
        Permission::ScreenRecording => ScreenCaptureAccess.preflight(),
    };
    if granted {
        PermissionStatus::Granted
    } else {
        PermissionStatus::NotGranted
    }
}

pub fn repair_impl(
    permission: Permission,
    bundle_id: &str,
) -> Result<PermissionStatus, RepairError> {
    let in_bundle = std::env::current_exe()
        .map(|exe| is_app_bundle_executable(&exe))
        .unwrap_or(false);
    if !in_bundle {
        return Err(RepairError::NotBundled);
    }
    reset(permission, bundle_id)?;
    request(permission);
    Ok(status_impl(permission))
}

/// `tccutil reset <service> <bundle_id>`: drops every TCC row for that
/// service + bundle id, stale or not. Works unprivileged for the user's
/// own TCC database. `bundle_id` was validated by `os::permissions::repair`.
fn reset(permission: Permission, bundle_id: &str) -> Result<(), RepairError> {
    let output = Command::new(TCCUTIL)
        .args(["reset", permission.tcc_service(), bundle_id])
        .output()
        .map_err(|e| RepairError::ResetFailed(format!("could not run {TCCUTIL}: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    // tccutil reports failures on stderr, e.g. `No such bundle identifier`
    // (exit 64) or `Failed to reset …` (exit 70).
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(RepairError::ResetFailed(if stderr.is_empty() {
        format!("{TCCUTIL} exited with {}", output.status)
    } else {
        stderr
    }))
}

/// Ask macOS for the permission so the running binary appears in the
/// System Settings list (switched off). The return values are just the
/// pre-prompt status, so they're ignored; the caller re-reads status.
fn request(permission: Permission) {
    match permission {
        Permission::Accessibility => {
            let options: CFDictionary<CFString, CFBoolean> = CFDictionary::from_CFType_pairs(&[(
                unsafe { CFString::wrap_under_get_rule(ax::kAXTrustedCheckOptionPrompt) },
                CFBoolean::true_value(),
            )]);
            unsafe { ax::AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) };
        }
        Permission::InputMonitoring => {
            unsafe { CGRequestListenEventAccess() };
        }
        Permission::ScreenRecording => {
            ScreenCaptureAccess.request();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reads_without_prompting() {
        // Preflight-only; whatever the answer, it must be a real status.
        for p in Permission::ALL {
            assert_ne!(status_impl(p), PermissionStatus::NotApplicable);
        }
    }

    #[test]
    fn reset_reports_tccutil_failure_for_unknown_bundle() {
        // A bundle id LaunchServices can't resolve: tccutil exits non-zero
        // without touching any real app's grants.
        let err = reset(
            Permission::Accessibility,
            "com.example.kage-test-does-not-exist",
        )
        .expect_err("tccutil should reject an unknown bundle id");
        match err {
            RepairError::ResetFailed(detail) => {
                assert!(!detail.is_empty(), "failure should carry tccutil's message");
            }
            other => panic!("expected ResetFailed, got {other:?}"),
        }
    }

    #[test]
    fn repair_refuses_outside_an_app_bundle() {
        // The test binary lives in target/…/deps, not a .app bundle, so
        // repair must bail before tccutil runs or anything prompts.
        assert_eq!(
            repair_impl(
                Permission::Accessibility,
                "com.example.kage-test-does-not-exist"
            ),
            Err(RepairError::NotBundled)
        );
    }
}
