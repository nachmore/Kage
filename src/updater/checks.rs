use anyhow::{Context, Result};
use log::{info, warn};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Compile-time endpoint URLs per release channel.
pub const ENDPOINT_STABLE: &str = env!("UPDATE_ENDPOINT_STABLE");
pub const ENDPOINT_BETA: &str = env!("UPDATE_ENDPOINT_BETA");
pub const ENDPOINT_DEV: &str = env!("UPDATE_ENDPOINT_DEV");
/// Legacy diagnostic URL retained for the Settings command.
pub const CHANGELOG_URL: &str = env!("UPDATE_CHANGELOG_URL");
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PUBKEY: Option<&str> = option_env!("TAURI_UPDATER_PUBKEY");

/// Resolve a configured channel, falling back to stable for empty endpoints.
pub fn endpoint_for_channel(channel: crate::config::Channel) -> &'static str {
    let endpoint = match channel {
        crate::config::Channel::Stable => ENDPOINT_STABLE,
        crate::config::Channel::Beta => ENDPOINT_BETA,
        crate::config::Channel::Dev => ENDPOINT_DEV,
    };
    if endpoint.is_empty() {
        ENDPOINT_STABLE
    } else {
        endpoint
    }
}

/// Ask the signed updater plugin for an update on the requested channel.
pub async fn plugin_check<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    channel: crate::config::Channel,
) -> Result<Option<Update>> {
    let Some(pubkey) = PUBKEY else {
        warn!("Updater: no public key configured - skipping check");
        return Ok(None);
    };
    let endpoint = endpoint_for_channel(channel);
    if endpoint.is_empty() {
        warn!(
            "Updater: no endpoint configured for channel '{}'",
            channel.as_str()
        );
        return Ok(None);
    }

    info!(
        "Checking for updates (channel={}, endpoint={})",
        channel.as_str(),
        endpoint
    );
    let endpoint_url = reqwest::Url::parse(endpoint)
        .with_context(|| format!("Invalid endpoint URL: {endpoint}"))?;
    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint_url])
        .context("Failed to configure updater endpoints")?
        .pubkey(pubkey)
        .build()
        .context("Failed to build updater")?;

    match updater.check().await {
        Ok(update) => Ok(update),
        Err(error) if is_manifest_not_found(&error) => {
            info!(
                "No release available in the '{}' channel (nothing published at {}); reporting up-to-date",
                channel.as_str(),
                endpoint
            );
            Ok(None)
        }
        Err(error) => Err(anyhow::Error::new(error).context("Update check failed")),
    }
}

/// True when the check failed because the channel simply has no release.
///
/// The plugin signals this with `Error::ReleaseNotFound`: any non-2xx
/// endpoint response (404 on a channel with no published release, but
/// also 403/410 from GitHub's CDN) leaves it without a manifest and it
/// returns that variant. Matching on the variant — not on "404" in the
/// message text, which the plugin never includes — is what routes this
/// to the friendly "no release available in this channel" path instead
/// of a scary "Update check failed" error.
fn is_manifest_not_found(error: &tauri_plugin_updater::Error) -> bool {
    matches!(error, tauri_plugin_updater::Error::ReleaseNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_not_found_is_treated_as_up_to_date() {
        // The friendly "no release available in this channel" path keys
        // off the enum variant. If this match ever regresses to message
        // sniffing, a channel with no published release logs a scary
        // "Update check failed" instead of quietly reporting up-to-date.
        assert!(is_manifest_not_found(
            &tauri_plugin_updater::Error::ReleaseNotFound
        ));
    }

    #[test]
    fn other_updater_errors_still_fail_the_check() {
        // Real failures (bad signature, network) must NOT collapse into
        // the silent up-to-date path.
        assert!(!is_manifest_not_found(
            &tauri_plugin_updater::Error::EmptyEndpoints
        ));
        assert!(!is_manifest_not_found(
            &tauri_plugin_updater::Error::Network("connection refused".into())
        ));
    }
}
