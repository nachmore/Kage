//! Tauri commands for extension, theme, and store management, split by theme:
//!   - `discovery` — list extensions/themes/command-packs, per-extension
//!     config, enable/disable, theme-colour loading.
//!   - `files` — read extension locale catalogs and provider/settings files,
//!     plus generic extension-data persistence.
//!   - `install` — local install/uninstall, install commit, grant removal.
//!   - `store` — store window plus the catalog/detail/install HTTP surface.
//!   - `welcome` — first-run batch provisioning from the welcome screen.
//!
//! Submodules pull this module's shared imports via `use super::*`, and the
//! flat re-exports below preserve the original `commands::extensions::*`
//! surface so callers (and `tauri::generate_handler!`) are unaffected. The
//! store base-URL/HTTP-client helpers live here so both `store` and `welcome`
//! can share them.

use crate::error::{AppError, ErrorKind};
use crate::events;
use crate::extensions;
use crate::lock_ext::LockExt;
use crate::state::{FeatureServices, UiState};
use crate::window_labels;
use log::{error, info, warn};
use tauri::{Emitter, Manager, State};

mod discovery;
mod files;
mod install;
mod store;
mod welcome;

// Flat re-export preserves the previous `commands::extensions::*` surface.
pub use discovery::*;
pub use files::*;
pub use install::*;
pub use store::*;
pub use welcome::*;

/// Dev server URL used as default store in dev mode.
const DEV_STORE_URL: &str = "http://localhost:1420";

/// Default production store URL — the public Kage-Extensions catalog.
const DEFAULT_STORE_URL: &str = "https://nachmore.github.io/Kage-Extensions";

/// Request timeout for store API calls.
const STORE_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Connect timeout for package downloads.
const DOWNLOAD_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Idle timeout between body reads. Package downloads get no total deadline:
/// a large package on a slow link can legitimately outlast the 15 s JSON
/// budget, so we only give up once the server stops sending.
const DOWNLOAD_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Cap on a downloaded package, checked against Content-Length up front and
/// enforced while streaming so a bad source can't make us buffer an
/// unbounded body before the zip checks run.
const MAX_PACKAGE_BYTES: u64 = 50 * 1024 * 1024;

/// `_source` name the catalog gives items from the primary store.
const PRIMARY_SOURCE_NAME: &str = "Default";

/// Resolve the store base URL: user-configured > production default > dev default.
fn resolve_store_url(config: &crate::config::Config, dev_mode: bool) -> String {
    if let Some(ref url) = config.store_url {
        if !url.is_empty() {
            return url.trim_end_matches('/').to_string();
        }
    }
    if dev_mode {
        return DEV_STORE_URL.to_string();
    }
    DEFAULT_STORE_URL.to_string()
}

/// Resolve the base URL for a catalog `_source` name. `None` or the primary
/// name is the primary store; anything else must be an enabled user source.
/// Callers still run `validate_store_url` on the result.
fn resolve_source_url(
    config: &crate::config::Config,
    dev_mode: bool,
    source: Option<&str>,
) -> Option<String> {
    match source {
        None | Some(PRIMARY_SOURCE_NAME) => Some(resolve_store_url(config, dev_mode)),
        Some(name) => config
            .store_sources
            .iter()
            .find(|s| s.enabled && s.name == name)
            .map(|s| s.url.trim_end_matches('/').to_string()),
    }
    .filter(|url| !url.is_empty())
}

/// Build a reqwest client with timeout.
fn store_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(STORE_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))
}

fn check_package_size(len: u64) -> Result<(), String> {
    if len > MAX_PACKAGE_BYTES {
        return Err(format!(
            "Package is too large ({} bytes, max {} MB)",
            len,
            MAX_PACKAGE_BYTES / (1024 * 1024)
        ));
    }
    Ok(())
}

/// Download a store package with idle (not total) timeouts and a size cap.
async fn download_package(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .read_timeout(DOWNLOAD_READ_TIMEOUT)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?;

    let declared = resp.content_length();
    if let Some(len) = declared {
        check_package_size(len)?;
    }
    let mut bytes = Vec::with_capacity(declared.unwrap_or(0) as usize);
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("Failed to read download: {}", e))?
    {
        check_package_size(bytes.len() as u64 + chunk.len() as u64)?;
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Check the zip magic and, when the catalog published one, the sha256. A
/// mismatch means the catalog and the zip are out of sync — better to refuse
/// the install than silently load tampered code.
fn verify_package(bytes: &[u8], expected_sha: Option<&str>, id: &str) -> Result<(), String> {
    if !bytes.starts_with(b"PK\x03\x04") {
        return Err("Invalid zip archive".into());
    }
    if let Some(expected) = expected_sha {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(bytes);
        let actual = hex::encode(hasher.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(format!(
                "Checksum mismatch for '{}' (expected {}, got {})",
                id, expected, actual
            ));
        }
    }
    Ok(())
}

/// Removes a temp file on drop, so every exit path cleans up.
struct TempFileGuard(std::path::PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Write a verified package to a uniquely named temp file and install it.
/// The name is random rather than id-derived so nothing from the catalog
/// ends up in the path.
fn install_package_bytes(
    bytes: &[u8],
    mode: extensions::InstallMode,
) -> Result<extensions::Installed, String> {
    let zip = TempFileGuard(
        std::env::temp_dir().join(format!("kage-download-{}.zip", uuid::Uuid::new_v4())),
    );
    std::fs::write(&zip.0, bytes).map_err(|e| format!("Failed to save download: {}", e))?;
    extensions::install_from_zip(&zip.0, mode).map_err(|e| format!("Installation failed: {}", e))
}

/// Record the enable flag for an install that landed live (not parked). A
/// user-initiated install enables the item — they just asked for it — while
/// an auto-update keeps whatever the user chose, so a disabled item stays
/// disabled.
fn record_install_state(
    states: &mut std::collections::HashMap<String, bool>,
    id: &str,
    mode: extensions::InstallMode,
) {
    match mode {
        extensions::InstallMode::DeferUpgrade => {
            states.insert(id.to_string(), true);
        }
        extensions::InstallMode::Replace => {
            states.entry(id.to_string()).or_insert(true);
        }
    }
}

/// Resolve a relative path inside the catalog (`packages/foo.zip`) to an
/// absolute URL using the store base. Strips a leading slash so the
/// result is always `<base>/<rel>` regardless of how the catalog quotes
/// it.
fn resolve_relative(base: &str, rel: &str) -> String {
    let r = rel.trim_start_matches('/');
    format!("{}/{}", base.trim_end_matches('/'), r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::InstallMode;

    #[test]
    fn auto_update_keeps_a_disabled_item_disabled() {
        let mut states = std::collections::HashMap::from([("cal".to_string(), false)]);
        record_install_state(&mut states, "cal", InstallMode::Replace);
        assert_eq!(states.get("cal"), Some(&false));

        record_install_state(&mut states, "new", InstallMode::Replace);
        assert_eq!(states.get("new"), Some(&true));
    }

    #[test]
    fn user_install_enables_the_item() {
        let mut states = std::collections::HashMap::from([("cal".to_string(), false)]);
        record_install_state(&mut states, "cal", InstallMode::DeferUpgrade);
        assert_eq!(states.get("cal"), Some(&true));
    }

    #[test]
    fn source_url_resolves_primary_and_enabled_user_sources() {
        let config = crate::config::Config {
            store_url: Some("https://primary.example/".to_string()),
            store_sources: vec![
                crate::config::StoreSource {
                    name: "Team".to_string(),
                    url: "https://team.example/store/".to_string(),
                    enabled: true,
                },
                crate::config::StoreSource {
                    name: "Off".to_string(),
                    url: "https://off.example".to_string(),
                    enabled: false,
                },
            ],
            ..Default::default()
        };

        let primary = Some("https://primary.example".to_string());
        assert_eq!(resolve_source_url(&config, false, None), primary);
        assert_eq!(
            resolve_source_url(&config, false, Some(PRIMARY_SOURCE_NAME)),
            primary
        );
        assert_eq!(
            resolve_source_url(&config, false, Some("Team")).as_deref(),
            Some("https://team.example/store")
        );
        assert_eq!(resolve_source_url(&config, false, Some("Off")), None);
        assert_eq!(resolve_source_url(&config, false, Some("Nope")), None);
    }

    #[test]
    fn package_size_cap() {
        assert!(check_package_size(MAX_PACKAGE_BYTES).is_ok());
        assert!(check_package_size(MAX_PACKAGE_BYTES + 1).is_err());
    }

    #[test]
    fn verify_package_checks_magic_and_checksum() {
        use sha2::Digest;
        let bytes = b"PK\x03\x04rest";
        let sha = hex::encode(sha2::Sha256::digest(bytes));

        assert!(verify_package(bytes, None, "x").is_ok());
        assert!(verify_package(bytes, Some(sha.to_uppercase().as_str()), "x").is_ok());
        assert!(verify_package(bytes, Some("00"), "x").is_err());
        assert!(verify_package(b"<html>", None, "x").is_err());
        assert!(verify_package(b"PK", None, "x").is_err());
    }
}
