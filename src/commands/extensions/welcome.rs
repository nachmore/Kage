//! First-run welcome batch provisioning.
//!
//! Extensions selected on the welcome screen are pulled from the
//! configured store URL (production catalog by default). If the user is
//! offline at first launch, individual installs will fail and be reported
//! in the [`WelcomeProvisionReport`] — the user can pick up the rest later
//! from the store. The store-URL/client helpers are shared with
//! [`super::store`] via the parent module.

use super::*;

#[derive(Debug, serde::Deserialize)]
pub struct WelcomeExtensionDecision {
    pub id: String,
    /// Whether the user ticked the box on the welcome screen.
    pub checked: bool,
}

#[derive(Debug, serde::Serialize, Default)]
pub struct WelcomeProvisionReport {
    pub installed: u32,
    pub enabled: u32,
    pub disabled: u32,
    pub skipped: u32,
    pub failed: u32,
}

/// Apply the extension decisions the user made on the welcome screen.
///
/// Fire-and-forget from the caller's perspective: the command returns
/// immediately after spawning a blocking task, so the welcome window can
/// transition to a "Launching Kage…" state and close without waiting on
/// local disk I/O.
///
/// The work runs on `spawn_blocking` because it's all synchronous disk
/// I/O (zip extraction, config writes). Putting it on the main Tokio
/// runtime would tie up a thread that other Tauri commands need.
///
/// Individual failures are logged but do not short-circuit the batch —
/// we'd rather enable 4 of 5 extensions than fail atomically on the 3rd.
/// Aggregate counts land in the `first_run_extensions_provisioned`
/// telemetry event; there's no cross-window signal because no caller
/// needs one today.
#[tauri::command]
pub async fn welcome_provision_extensions<R: tauri::Runtime>(
    decisions: Vec<WelcomeExtensionDecision>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    info!(
        "welcome_provision_extensions: dispatching {} decisions to background task",
        decisions.len()
    );

    // Clone the state we need to move into the blocking task. The Tauri
    // State can't cross the spawn boundary directly — it's tied to the
    // command invocation — so we pull the Arc<Mutex<Config>> out and
    // move that instead.
    let config = features.config.clone();
    let app_handle = app.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let report = provision_decisions(&decisions, &config, &app_handle);

        info!(
            "welcome_provision: done — installed={} enabled={} disabled={} skipped={} failed={}",
            report.installed, report.enabled, report.disabled, report.skipped, report.failed
        );

        // Aggregate telemetry. Opt-out is already respected inside track().
        crate::telemetry::track(
            &app_handle,
            "first_run_extensions_provisioned",
            Some(serde_json::json!({
                "installed": report.installed,
                "enabled": report.enabled,
                "disabled": report.disabled,
                "skipped": report.skipped,
                "failed": report.failed,
            })),
        );
    });

    Ok(())
}

/// Core provisioning loop — pulled into its own function so the
/// spawn_blocking task in [`welcome_provision_extensions`] can call it
/// with a plain Arc<Mutex<Config>> rather than needing a Tauri State.
fn provision_decisions<R: tauri::Runtime>(
    decisions: &[WelcomeExtensionDecision],
    config: &std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    app: &tauri::AppHandle<R>,
) -> WelcomeProvisionReport {
    // Snapshot what's already installed so we don't re-stage duplicates.
    let already_installed: std::collections::HashSet<String> = {
        let states = {
            let cfg = config.lock_or_recover();
            cfg.extension_states.clone()
        };
        let mut set = std::collections::HashSet::new();
        for kind in &["extension", "theme"] {
            for item in extensions::discover_items(kind, &states) {
                set.insert(item.manifest.id);
            }
        }
        set
    };

    let mut report = WelcomeProvisionReport::default();

    for decision in decisions {
        if !decision.checked {
            report.skipped += 1;
            continue;
        }
        // One path for every checked extension: ensure the files are on
        // disk (network install only when missing), then ensure the
        // enable flag + capability grant are committed. "Already on
        // disk" must NOT short-circuit the grant step — a config reset
        // leaves the install dir intact while wiping extension_states /
        // extension_grants, and skipping here loaded those extensions
        // with zero capabilities ("no user grant recorded" on every
        // invoke).
        match provision_one(config, app, &decision.id, &already_installed) {
            Ok(ProvisionOutcome::Installed) => {
                report.installed += 1;
                info!("welcome_provision: installed '{}'", decision.id);
            }
            Ok(ProvisionOutcome::Granted) => {
                report.enabled += 1;
                info!(
                    "welcome_provision: committed grant for already-installed '{}'",
                    decision.id
                );
            }
            Ok(ProvisionOutcome::AlreadyProvisioned) => report.skipped += 1,
            Err(e) => {
                report.failed += 1;
                warn!("welcome_provision: '{}' failed: {}", decision.id, e);
            }
        }
    }

    report
}

enum ProvisionOutcome {
    /// Files were fetched from the store and the grant committed.
    Installed,
    /// Files were already on disk; the missing state/grant was committed.
    Granted,
    /// Files, enable flag, and grant were all already in place.
    AlreadyProvisioned,
}

/// Ensure one welcome-screen extension is installed, enabled, and granted.
fn provision_one<R: tauri::Runtime>(
    config: &std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    app: &tauri::AppHandle<R>,
    id: &str,
    already_installed: &std::collections::HashSet<String>,
) -> Result<ProvisionOutcome, String> {
    if !already_installed.contains(id) {
        install_and_commit_direct(config, app, id)?;
        return Ok(ProvisionOutcome::Installed);
    }

    extensions::validate_extension_id(id).map_err(|e| format!("Invalid extension id: {}", e))?;

    // Find the installed manifest to read its requested permissions.
    let states = {
        let cfg = config.lock_or_recover();
        cfg.extension_states.clone()
    };
    let item = ["extension", "theme"]
        .iter()
        .flat_map(|kind| extensions::discover_items(kind, &states))
        .find(|item| item.manifest.id == id)
        .ok_or_else(|| format!("'{}' not found on disk", id))?;

    {
        let mut cfg = config.lock_or_recover();
        let had_state = cfg.extension_states.get(id).copied() == Some(true);
        let had_grant = cfg.extension_grants.contains_key(id);
        if had_state && had_grant {
            return Ok(ProvisionOutcome::AlreadyProvisioned);
        }
        commit_grant_locked(&mut cfg, &item.manifest)?;
    }

    if let Err(e) = app.emit(events::EXTENSIONS_CHANGED, ()) {
        error!("Failed to emit extensions_changed: {}", e);
    }
    Ok(ProvisionOutcome::Granted)
}

/// Enable + record the capability grant for `manifest`, then persist.
/// The single grant-writing step both welcome paths (fresh install,
/// already-on-disk) share. Caller holds the config lock.
fn commit_grant_locked(
    cfg: &mut crate::config::Config,
    manifest: &extensions::ExtensionManifest,
) -> Result<(), String> {
    seed_grant(cfg, manifest);
    cfg.save()
        .map_err(|e| format!("Failed to save config: {}", e))
}

/// Pure mutation half of [`commit_grant_locked`]: flip the enable flag
/// and record the grant from the manifest's requested permissions.
fn seed_grant(cfg: &mut crate::config::Config, manifest: &extensions::ExtensionManifest) {
    let raw_perms: Vec<String> = manifest.permissions.clone().unwrap_or_default();
    let granted = extensions::normalize_permissions(&raw_perms, &manifest.id);
    cfg.extension_states.insert(manifest.id.clone(), true);
    cfg.extension_grants.insert(
        manifest.id.clone(),
        crate::config::ExtensionGrant {
            granted,
            approved_version: manifest.version.clone(),
            approved_at: chrono::Utc::now().to_rfc3339(),
        },
    );
}

/// Install + commit in one go for the welcome flow.
///
/// Fetches the extension package from the configured store URL
/// (`https://nachmore.github.io/Kage-Extensions/` by default), verifies
/// the SHA-256 against the catalog, extracts to the user's install dir,
/// and commits the capability grant. If the user is offline at first
/// launch, individual installs fail and surface in the
/// `WelcomeProvisionReport`; the user can install the rest later from
/// the store window.
fn install_and_commit_direct<R: tauri::Runtime>(
    config: &std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    app: &tauri::AppHandle<R>,
    id: &str,
) -> Result<String, String> {
    extensions::validate_extension_id(id).map_err(|e| format!("Invalid extension id: {}", e))?;

    // Resolve the store URL the same way the live install path does.
    let (base_url, dev_mode) = {
        let cfg = config.lock_or_recover();
        let dev = std::env::var("KAGE_DEV").is_ok() || cfg!(debug_assertions);
        (resolve_store_url(&cfg, dev), dev)
    };
    let _ = dev_mode; // currently unused beyond the resolve call above
    if base_url.is_empty() {
        return Err("No store URL configured".to_string());
    }
    extensions::validate_store_url(&base_url).map_err(|e| format!("Bad store URL: {}", e))?;

    // Run the network install + extract on the runtime via block_on.
    let runtime = tokio::runtime::Handle::try_current().map_err(|e| {
        format!(
            "No tokio runtime available for welcome install of '{}': {}",
            id, e
        )
    })?;

    // Fetch detail + zip + verify hash + extract.
    let item = runtime.block_on(welcome_store_install(&base_url, id))?;
    let installed_id = item.manifest.id.clone();

    {
        let mut cfg = config.lock_or_recover();
        commit_grant_locked(&mut cfg, &item.manifest)?;
    }

    crate::telemetry::track(
        app,
        "extension_installed",
        Some(serde_json::json!({
            "extension_id": installed_id,
            "source": "welcome_network",
        })),
    );

    if let Err(e) = app.emit(events::EXTENSIONS_CHANGED, ()) {
        error!("Failed to emit extensions_changed: {}", e);
    }

    Ok(installed_id)
}

/// Welcome-flow variant of the store install: fetches detail, downloads
/// the zip, verifies the published sha256, and extracts. Doesn't touch
/// the config — that's the caller's job (so we can fold the grant write
/// into the same critical section as the enable flag).
async fn welcome_store_install(
    base_url: &str,
    id: &str,
) -> Result<extensions::InstalledItem, String> {
    let client = store_client()?;
    let detail_url = format!("{}/detail/{}.json", base_url, id);
    let detail: serde_json::Value = client
        .get(&detail_url)
        .send()
        .await
        .map_err(|e| format!("Detail fetch failed: {}", e))?
        .json()
        .await
        .map_err(|e| format!("Invalid detail JSON: {}", e))?;

    let download_rel = detail
        .get("downloadUrl")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Detail response missing downloadUrl".to_string())?;
    let download_url = if download_rel.starts_with("http") {
        download_rel.to_string()
    } else {
        resolve_relative(base_url, download_rel)
    };
    let expected_sha = detail
        .get("sha256")
        .and_then(|v| v.as_str())
        .map(String::from);

    let bytes = client
        .get(&download_url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?
        .bytes()
        .await
        .map_err(|e| format!("Failed to read download: {}", e))?;
    if bytes.len() < 4 || &bytes[0..4] != b"PK\x03\x04" {
        return Err("Invalid zip archive".into());
    }
    if let Some(expected) = expected_sha {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        let actual = hex::encode(hasher.finalize());
        if !actual.eq_ignore_ascii_case(&expected) {
            return Err(format!(
                "Checksum mismatch for '{}' (expected {}, got {})",
                id, expected, actual
            ));
        }
    }

    let zip_path = std::env::temp_dir().join(format!("kage-welcome-{}.zip", id));
    std::fs::write(&zip_path, &bytes).map_err(|e| format!("Failed to save download: {}", e))?;
    let item = extensions::install_from_zip(&zip_path)
        .map_err(|e| format!("Installation failed: {}", e))?;
    let _ = std::fs::remove_file(&zip_path);
    Ok(item)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, perms: &[&str]) -> extensions::ExtensionManifest {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": "__MSG_manifest.name__",
            "version": "1.2.3",
            "type": "extension",
            "permissions": perms,
        }))
        .expect("test manifest must deserialize")
    }

    #[test]
    fn seed_grant_enables_and_records_normalized_permissions() {
        let mut cfg = crate::config::Config::default();
        seed_grant(
            &mut cfg,
            &manifest("spotify", &["storage", "urls", "oauth"]),
        );

        assert_eq!(cfg.extension_states.get("spotify"), Some(&true));
        let grant = cfg
            .extension_grants
            .get("spotify")
            .expect("grant must be recorded");
        assert_eq!(grant.granted, vec!["storage", "urls", "oauth"]);
        assert_eq!(grant.approved_version, "1.2.3");
        assert!(!grant.approved_at.is_empty());
    }

    #[test]
    fn seed_grant_drops_unknown_capabilities() {
        // normalize_permissions is authoritative: a typo'd or hostile
        // capability in a manifest must not be stored as granted.
        let mut cfg = crate::config::Config::default();
        seed_grant(&mut cfg, &manifest("x", &["storage", "root_of_all_evil"]));
        assert_eq!(
            cfg.extension_grants.get("x").unwrap().granted,
            vec!["storage"]
        );
    }

    #[test]
    fn seed_grant_records_empty_grant_for_permissionless_extensions() {
        // A no-permissions extension still needs a grant RECORD — a
        // missing record means "no user grant recorded" warnings and
        // zero capabilities forever (the wiped-config bug).
        let mut cfg = crate::config::Config::default();
        seed_grant(&mut cfg, &manifest("math", &[]));
        assert!(cfg.extension_grants.contains_key("math"));
        assert!(cfg.extension_grants.get("math").unwrap().granted.is_empty());
        assert_eq!(cfg.extension_states.get("math"), Some(&true));
    }
}
