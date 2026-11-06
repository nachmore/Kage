//! Tauri commands behind the launcher's right-click menu for app and
//! file results. Logic lives in `crate::launcher_actions`; this layer
//! looks apps up in the registry, keeps blocking OS work off the async
//! runtime, and keeps the registry in step after an uninstall.

use crate::error::{AppError, ErrorKind};
use crate::launcher_actions::{
    self as actions, ActionOutcome, ActionResult, Resolved, ResultAction, ResultActionsInfo,
};
use crate::state::FeatureServices;
use tauri::State;

/// Kage's bundle id, resource folder and bundled resources — what "Kage
/// itself" means when deciding what may be moved or uninstalled.
fn own_locations<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> actions::OwnLocations {
    use tauri::utils::config::BundleResources;
    use tauri::Manager;
    // Tauri derives it from a canonicalised exe path: verbatim (`\\?\C:\…`)
    // on Windows. Compared with the plain forms everything else uses.
    let resource_dir = app.path().resource_dir().ok().map(actions::strip_verbatim);
    let entries: Vec<String> = match &app.config().bundle.resources {
        Some(BundleResources::List(list)) => list.clone(),
        // Source → target: where it lands is the target.
        Some(BundleResources::Map(map)) => map.values().cloned().collect(),
        None => Vec::new(),
    };
    actions::OwnLocations {
        bundle_id: app.config().identifier.clone(),
        resources: resource_dir
            .as_deref()
            .map(|dir| actions::bundled_resource_paths(dir, &entries))
            .unwrap_or_default(),
        resource_dir,
    }
}

/// Resolve the result the webview named. `kind` is `"app"` (target = the
/// app's display name, looked up in the registry — never a path) or
/// `"path"` (target = a filesystem path, validated). Anything else, or an
/// empty target, is refused.
async fn resolve<R: tauri::Runtime>(
    kind: &str,
    target: &str,
    features: &State<'_, FeatureServices>,
    app: &tauri::AppHandle<R>,
) -> Result<Resolved, AppError> {
    if target.trim().is_empty() {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.launcher.invalid_target",
            &[],
        ));
    }
    let target = target.to_string();
    let own = own_locations(app);
    let job: Box<dyn FnOnce() -> Result<Resolved, AppError> + Send> = match kind {
        "app" => {
            let registry = features.app_launcher.lock().await.get_by_name(&target);
            Box::new(move || actions::resolve_app(&target, registry, &own))
        }
        "path" => Box::new(move || actions::resolve_path(&target, &own)),
        _ => {
            return Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.launcher.invalid_target",
                &[],
            ))
        }
    };
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|e| AppError::internal(format!("launcher action task failed: {e}")))?
}

/// The menu for one launcher result: its resolved name/path, the OS, and
/// the actions that apply to it here.
#[tauri::command]
pub async fn launcher_result_actions<R: tauri::Runtime>(
    kind: String,
    target: String,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<ResultActionsInfo, AppError> {
    Ok(resolve(&kind, &target, &features, &app).await?.info())
}

/// Run one menu action on a launcher result. The target is re-resolved and
/// the action re-checked against what that target offers; `uninstall`
/// additionally requires `confirmed: true` — the frontend sets it only
/// after the user confirmed in Kage.
#[tauri::command]
pub async fn launcher_result_action<R: tauri::Runtime>(
    kind: String,
    target: String,
    action: ResultAction,
    confirmed: Option<bool>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<ActionResult, AppError> {
    let resolved = resolve(&kind, &target, &features, &app).await?;
    if !resolved.actions().contains(&action) {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.launcher.action_unavailable",
            &[],
        ));
    }
    if action == ResultAction::Uninstall && confirmed != Some(true) {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.launcher.confirmation_required",
            &[],
        ));
    }
    log::info!(
        "[launcher-action] {:?} on {:?} {}",
        action,
        resolved.kind,
        resolved.path.display()
    );

    let name = resolved.name.clone();
    let path = resolved.path.clone();
    let outcome = if action == ResultAction::Properties {
        // The property sheet lives on the calling thread's apartment and
        // needs its message loop: run it on the main thread.
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            let _ = tx.send(actions::perform(&resolved, action));
        })
        .map_err(|e| AppError::internal(format!("run_on_main_thread: {e}")))?;
        rx.await
            .map_err(|e| AppError::internal(format!("properties task dropped: {e}")))??
    } else {
        tauri::async_runtime::spawn_blocking(move || actions::perform(&resolved, action))
            .await
            .map_err(|e| AppError::internal(format!("launcher action task failed: {e}")))??
    };

    // A trashed app bundle leaves the list now; the install watcher's
    // rescan follows. (A launched uninstaller may still be cancelled, so
    // that case waits for the watcher.)
    if action == ResultAction::Uninstall && outcome == ActionOutcome::MovedToTrash {
        features.app_launcher.lock().await.remove_app_at(&path);
    }
    Ok(ActionResult { outcome, name })
}
