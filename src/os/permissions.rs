//! OS privacy permissions: live status, plus "repair" for a stale grant.
//!
//! Only macOS gates the APIs Kage uses behind per-app privacy grants (TCC,
//! System Settings → Privacy & Security). Windows and Linux report every
//! permission as `NotApplicable`, and repair is unsupported there.
//!
//! What each permission backs on macOS (keep in sync with the copy in
//! `ui/js/shared/mac-permissions.js`):
//!
//!   - **Accessibility** — synthetic Cmd+C / Cmd+V (`os::clipboard`'s
//!     selection capture and `simulate_paste`), raising one specific
//!     window in the window switcher and listing/restoring its minimized
//!     windows (`os::window_list`), and the Computer Control sidecar's
//!     AX tree + mouse/keyboard synthesis (kage-core).
//!   - **Screen Recording** — `kCGWindowName` from CGWindowList, i.e.
//!     window titles for the active-window context, activity tracking and
//!     the open-windows list (`os::window_list`).
//!   - **Input Monitoring** — listen-only CGEventTaps. Global hotkeys go
//!     through tauri-plugin-global-shortcut (Carbon `RegisterEventHotKey`),
//!     which needs no permission, so no shipping feature depends on this
//!     one and the UI doesn't show it. It stays here so status/repair
//!     covers every TCC service Kage has asked for in the past.
//!
//! # Why "repair" exists
//!
//! TCC stores a grant against the code signature of the binary that was
//! approved. Kage's macOS builds are currently ad-hoc signed, so that
//! signature is the exact cdhash of one build: every update or local
//! rebuild yields a binary TCC doesn't recognise, while System Settings
//! keeps showing "Kage" toggled on (for the old cdhash). Toggling the row
//! doesn't help — it still belongs to the old binary. [`repair`] deletes
//! Kage's rows for one service (`tccutil reset <service> <bundle id>`) and
//! then asks macOS for access again, so the *running* binary is the one
//! listed; the user then switches it on.

use serde::Serialize;
use std::fmt;

/// A privacy permission Kage can query and repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    Accessibility,
    InputMonitoring,
    ScreenRecording,
}

impl Permission {
    /// Every permission, in the order `all_statuses` reports them.
    pub const ALL: [Permission; 3] = [
        Permission::Accessibility,
        Permission::InputMonitoring,
        Permission::ScreenRecording,
    ];

    /// Wire id shared with the UI (`MAC_PERMISSIONS[].id` in
    /// `ui/js/shared/mac-permissions.js`). Matches the serde form.
    pub fn id(self) -> &'static str {
        match self {
            Permission::Accessibility => "accessibility",
            Permission::InputMonitoring => "input-monitoring",
            Permission::ScreenRecording => "screen-recording",
        }
    }

    /// Parse a wire id. `None` for anything that isn't exactly one of
    /// [`Permission::id`]'s values.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }

    /// TCC service name as `tccutil` spells it (the `kTCCService…` suffix).
    pub fn tcc_service(self) -> &'static str {
        match self {
            Permission::Accessibility => "Accessibility",
            Permission::InputMonitoring => "ListenEvent",
            Permission::ScreenRecording => "ScreenCapture",
        }
    }
}

/// Whether the OS currently grants a permission to *this* binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
// Variants are platform-specific (each OS backend constructs only its own
// subset), hence the per-variant dead_code allowances.
pub enum PermissionStatus {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Granted,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    NotGranted,
    /// The platform has no such permission (Windows, Linux).
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    NotApplicable,
}

/// One permission's status, as sent to the frontend:
/// `{ "id": "accessibility", "status": "granted" }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PermissionState {
    pub id: Permission,
    pub status: PermissionStatus,
}

/// Why [`repair`] refused or failed. `Display` is English, for logs; the
/// command layer maps each variant to a localised `AppError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairError {
    /// This platform has no privacy grants to reset.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Unsupported,
    /// The bundle identifier isn't a plain reverse-DNS id, so it's never
    /// handed to `tccutil`.
    InvalidBundleId,
    /// Not running from inside a `.app` bundle (a bare dev binary). TCC
    /// attributes such a process to whatever launched it (the terminal),
    /// so resetting by bundle id would only wipe the *installed* app's
    /// grants without helping this process.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    NotBundled,
    /// `tccutil` couldn't be run or exited non-zero; carries its stderr.
    ResetFailed(String),
}

impl fmt::Display for RepairError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepairError::Unsupported => {
                write!(f, "permission repair is not supported on this platform")
            }
            RepairError::InvalidBundleId => write!(f, "bundle identifier failed validation"),
            RepairError::NotBundled => write!(f, "not running from an app bundle"),
            RepairError::ResetFailed(detail) => write!(f, "tccutil reset failed: {detail}"),
        }
    }
}

/// Current status of one permission. Never prompts.
pub fn status(permission: Permission) -> PermissionStatus {
    crate::os::platform::permissions::status_impl(permission)
}

/// Current status of every permission, in [`Permission::ALL`] order.
/// Never prompts.
pub fn all_statuses() -> Vec<PermissionState> {
    Permission::ALL
        .into_iter()
        .map(|id| PermissionState {
            id,
            status: status(id),
        })
        .collect()
}

/// Clear the OS's record of `permission` for `bundle_id`, then ask the OS
/// for it again so the running binary is listed (macOS may show its own
/// prompt). Returns the status afterwards — normally `NotGranted` until
/// the user switches Kage on in System Settings. Blocking (spawns
/// `tccutil`): call from a blocking-capable thread.
///
/// `bundle_id` must be the running app's own identifier; it is validated
/// here before any platform code sees it.
pub fn repair(permission: Permission, bundle_id: &str) -> Result<PermissionStatus, RepairError> {
    if !is_valid_bundle_id(bundle_id) {
        return Err(RepairError::InvalidBundleId);
    }
    crate::os::platform::permissions::repair_impl(permission, bundle_id)
}

/// Plain reverse-DNS bundle identifier: two or more dot-separated,
/// non-empty segments of ASCII letters, digits and hyphens (Apple's
/// CFBundleIdentifier charset), each starting with a letter or digit, at
/// most 255 bytes. Starting with an alphanumeric also means the id can
/// never be parsed as a command-line flag.
pub fn is_valid_bundle_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 255 {
        return false;
    }
    let mut segments = 0;
    for segment in id.split('.') {
        segments += 1;
        let mut chars = segment.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphanumeric() => {}
            _ => return false,
        }
        if !chars.all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return false;
        }
    }
    segments >= 2
}

/// `true` when `exe` is a bundle's main-executable location,
/// `<Name>.app/Contents/MacOS/<binary>`. Pure path check, so it's
/// testable on every platform.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn is_app_bundle_executable(exe: &std::path::Path) -> bool {
    let Some(macos_dir) = exe.parent() else {
        return false;
    };
    let Some(contents_dir) = macos_dir.parent() else {
        return false;
    };
    let Some(bundle_dir) = contents_dir.parent() else {
        return false;
    };
    let name_is = |path: &std::path::Path, expected: &str| {
        path.file_name().and_then(|n| n.to_str()) == Some(expected)
    };
    let is_app = bundle_dir
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("app"));
    exe.file_name().is_some()
        && name_is(macos_dir, "MacOS")
        && name_is(contents_dir, "Contents")
        && is_app
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn ids_round_trip_and_match_serde() {
        for p in Permission::ALL {
            assert_eq!(Permission::from_id(p.id()), Some(p));
            // The wire id the UI matches on is the serde form.
            assert_eq!(serde_json::to_value(p).unwrap(), serde_json::json!(p.id()));
        }
    }

    #[test]
    fn from_id_rejects_anything_else() {
        for bad in [
            "",
            "Accessibility",
            "input_monitoring",
            "screen-recording ",
            "camera",
        ] {
            assert_eq!(Permission::from_id(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn tcc_service_names_match_tccutil() {
        assert_eq!(Permission::Accessibility.tcc_service(), "Accessibility");
        assert_eq!(Permission::InputMonitoring.tcc_service(), "ListenEvent");
        assert_eq!(Permission::ScreenRecording.tcc_service(), "ScreenCapture");
    }

    #[test]
    fn status_serializes_snake_case() {
        let v = |s: PermissionStatus| serde_json::to_value(s).unwrap();
        assert_eq!(v(PermissionStatus::Granted), "granted");
        assert_eq!(v(PermissionStatus::NotGranted), "not_granted");
        assert_eq!(v(PermissionStatus::NotApplicable), "not_applicable");
    }

    #[test]
    fn permission_state_serializes_for_the_ui() {
        let state = PermissionState {
            id: Permission::ScreenRecording,
            status: PermissionStatus::NotGranted,
        };
        assert_eq!(
            serde_json::to_value(state).unwrap(),
            serde_json::json!({ "id": "screen-recording", "status": "not_granted" })
        );
    }

    #[test]
    fn all_statuses_reports_every_permission_in_order() {
        // Non-prompting on every platform, so safe to call from a test.
        let ids: Vec<Permission> = all_statuses().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, Permission::ALL.to_vec());
    }

    #[test]
    fn accepts_reverse_dns_bundle_ids() {
        for ok in [
            "com.kage.launcher",
            "com.apple.Chess",
            "io.github.some-user.kage-fork",
            "com.1password.app",
            "a.b",
        ] {
            assert!(is_valid_bundle_id(ok), "{ok:?} should be valid");
        }
    }

    #[test]
    fn rejects_unsafe_or_malformed_bundle_ids() {
        let too_long = format!("com.{}", "a".repeat(252));
        for bad in [
            "",
            "kage",               // single segment
            "-reset.kage",        // would read as a flag
            "com.kage.",          // empty segment
            ".com.kage",          // empty segment
            "com..kage",          // empty segment
            "com.-kage",          // segment starts with a hyphen
            "com.kage launcher",  // whitespace
            "com.kage;rm -rf ~",  // shell metacharacters
            "com.kage/../../etc", // path separators
            "com.kage_launcher",  // underscore isn't in Apple's charset
            "com.käge.launcher",  // non-ASCII
            "com.kage\0launcher", // NUL
            "com.kage\nlauncher", // newline
            too_long.as_str(),    // > 255 bytes
        ] {
            assert!(!is_valid_bundle_id(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn repair_validates_bundle_id_before_platform_code() {
        // Must fail on every platform without touching tccutil.
        assert_eq!(
            repair(Permission::Accessibility, "-h"),
            Err(RepairError::InvalidBundleId)
        );
        assert_eq!(
            repair(Permission::ScreenRecording, "not a bundle id"),
            Err(RepairError::InvalidBundleId)
        );
    }

    #[test]
    fn detects_app_bundle_executables() {
        assert!(is_app_bundle_executable(Path::new(
            "/Applications/Kage.app/Contents/MacOS/kage"
        )));
        assert!(is_app_bundle_executable(Path::new(
            "/Users/me/Applications/Kage Dev.app/Contents/MacOS/kage"
        )));
        for not_bundled in [
            "/Users/me/src/Kage/target/debug/kage",
            "/Applications/Kage.app/Contents/Resources/kage",
            "/Applications/Kage/Contents/MacOS/kage",
            "/Contents/MacOS/kage",
            "kage",
            "",
        ] {
            assert!(
                !is_app_bundle_executable(Path::new(not_bundled)),
                "{not_bundled:?} is not a bundle executable"
            );
        }
    }

    #[test]
    fn repair_error_display_is_english_for_logs() {
        assert_eq!(
            RepairError::ResetFailed("boom".into()).to_string(),
            "tccutil reset failed: boom"
        );
        assert_eq!(
            RepairError::NotBundled.to_string(),
            "not running from an app bundle"
        );
    }
}
