// Shared contract for the on-disk changelog cache.
//
// The app (`kage`) fetches release notes from GitHub (see
// src/updater/changelog.rs) and persists them here; the MCP sidecar's
// `get_kage_changelog` tool reads the cache so the agent can answer
// "what changed in the last update?" without the sidecar needing an
// HTTP client (and it works offline — post-update is exactly when the
// cache is fresh, because the app refreshes it on upgrade).
//
// Lives in kage-core because BOTH binaries must agree on the path and
// shape; a field added here reaches writer and reader in one edit.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangelogCache {
    /// App version that wrote this cache (CARGO_PKG_VERSION at fetch time).
    #[serde(default)]
    pub version: String,
    /// Release channel the notes were fetched for ("stable"/"beta"/"dev").
    #[serde(default)]
    pub channel: String,
    /// RFC 3339 timestamp of the fetch.
    #[serde(default)]
    pub fetched_at: String,
    /// Rendered release-notes markdown (most recent releases first).
    #[serde(default)]
    pub markdown: String,
}

/// `<config_dir>/kage/changelog-cache.json` — next to config.json and
/// the updater marker files.
pub fn cache_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("kage").join("changelog-cache.json"))
}

pub fn read() -> Option<ChangelogCache> {
    let path = cache_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write(cache: &ChangelogCache) -> std::io::Result<()> {
    let Some(path) = cache_path() else {
        return Err(std::io::Error::other("no config directory"));
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string(cache).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_round_trips_through_json() {
        let cache = ChangelogCache {
            version: "0.9.1".into(),
            channel: "dev".into(),
            fetched_at: "2026-07-22T00:00:00Z".into(),
            markdown: "## Kage Nightly\n- stuff".into(),
        };
        let json = serde_json::to_string(&cache).unwrap();
        let back: ChangelogCache = serde_json::from_str(&json).unwrap();
        assert_eq!(back.version, cache.version);
        assert_eq!(back.channel, cache.channel);
        assert_eq!(back.markdown, cache.markdown);
    }

    #[test]
    fn cache_tolerates_missing_fields() {
        // Old/foreign cache files must not fail the read — every field
        // is #[serde(default)].
        let back: ChangelogCache = serde_json::from_str("{}").unwrap();
        assert!(back.version.is_empty());
        assert!(back.markdown.is_empty());
    }
}
