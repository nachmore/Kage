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

/// Build a reqwest client with timeout.
fn store_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(STORE_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))
}

/// Resolve a relative path inside the catalog (`packages/foo.zip`) to an
/// absolute URL using the store base. Strips a leading slash so the
/// result is always `<base>/<rel>` regardless of how the catalog quotes
/// it.
fn resolve_relative(base: &str, rel: &str) -> String {
    let r = rel.trim_start_matches('/');
    format!("{}/{}", base.trim_end_matches('/'), r)
}
