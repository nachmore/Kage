//! OS privacy permissions (macOS TCC) for Settings → macOS Permissions and
//! the welcome wizard: live status, and repair for a grant macOS holds
//! for an older build. The OS work lives in `os::permissions`.

use crate::error::{AppError, ErrorKind};
use crate::os::permissions::{self, Permission, PermissionState, RepairError};
use log::{info, warn};

/// Status of every privacy permission, e.g.
/// `[{ "id": "accessibility", "status": "granted" }, …]`. Never prompts,
/// so the UI can call it on every window focus. Statuses are
/// `not_applicable` off macOS.
#[tauri::command]
pub async fn get_privacy_permissions() -> Result<Vec<PermissionState>, AppError> {
    Ok(permissions::all_statuses())
}

/// Reset Kage's grant for one permission (`tccutil reset`) and ask macOS
/// for it again so the running binary is the one listed in System
/// Settings. Returns the status afterwards (normally `not_granted` until
/// the user re-enables Kage). The bundle id is always the running app's
/// own identifier, never caller-supplied.
///
/// `permission` must stay a required argument: the mock-app command sweep
/// dispatches every command with empty args, and a required arg makes that
/// probe fail at deserialization instead of resetting real grants.
#[tauri::command]
pub async fn repair_privacy_permission<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    permission: String,
) -> Result<PermissionState, AppError> {
    let Some(perm) = Permission::from_id(&permission) else {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.privacy_permissions.unknown",
            &[("id", &permission)],
        ));
    };
    let bundle_id = app.config().identifier.clone();
    let task_bundle_id = bundle_id.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || permissions::repair(perm, &task_bundle_id))
            .await
            .map_err(|e| {
                repair_error(
                    &RepairError::ResetFailed(format!("task failed: {e}")),
                    &bundle_id,
                )
            })?;
    match result {
        Ok(status) => {
            info!(
                "[permissions] repaired {}: reset TCC {} for {}; status now {:?}",
                perm.id(),
                perm.tcc_service(),
                bundle_id,
                status
            );
            Ok(PermissionState { id: perm, status })
        }
        Err(err) => {
            warn!(
                "[permissions] repair of {} for {:?} failed: {}",
                perm.id(),
                bundle_id,
                err
            );
            Err(repair_error(&err, &bundle_id))
        }
    }
}

/// Map a repair failure to the localised error the UI shows.
fn repair_error(err: &RepairError, bundle_id: &str) -> AppError {
    match err {
        RepairError::Unsupported => AppError::keyed(
            ErrorKind::Internal,
            "errors.privacy_permissions.unsupported",
            &[],
        ),
        RepairError::InvalidBundleId => AppError::keyed(
            ErrorKind::Internal,
            "errors.privacy_permissions.invalid_bundle_id",
            &[("id", bundle_id)],
        ),
        RepairError::NotBundled => AppError::keyed(
            ErrorKind::Internal,
            "errors.privacy_permissions.not_bundled",
            &[],
        ),
        RepairError::ResetFailed(reason) => AppError::keyed(
            ErrorKind::Internal,
            "errors.privacy_permissions.reset_failed",
            &[("reason", reason)],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_errors_map_to_their_i18n_keys() {
        let cases = [
            (
                RepairError::Unsupported,
                "errors.privacy_permissions.unsupported",
            ),
            (
                RepairError::InvalidBundleId,
                "errors.privacy_permissions.invalid_bundle_id",
            ),
            (
                RepairError::NotBundled,
                "errors.privacy_permissions.not_bundled",
            ),
            (
                RepairError::ResetFailed("x".into()),
                "errors.privacy_permissions.reset_failed",
            ),
        ];
        for (err, key) in cases {
            assert_eq!(repair_error(&err, "com.kage.launcher").key, key, "{err:?}");
        }
    }

    #[test]
    fn repair_error_params_carry_context() {
        let e = repair_error(&RepairError::InvalidBundleId, "bad id");
        assert_eq!(e.params.get("id").map(String::as_str), Some("bad id"));
        let e = repair_error(&RepairError::ResetFailed("No such bundle".into()), "x.y");
        assert_eq!(
            e.params.get("reason").map(String::as_str),
            Some("No such bundle")
        );
    }
}
