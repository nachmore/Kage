use super::default_true;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreSource {
    /// Empty name/url are inert entries — the store UI renders them
    /// as-is and fetching an empty URL fails per-source without
    /// affecting the others.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Update channel. Resolved to a concrete endpoint URL by
/// `updater::endpoint_for_channel`. The `#[serde(other)]` fallback on
/// `Stable` means a stale / corrupted config or future-version variant
/// can't silently trap the user on a dead channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Beta,
    Dev,
    /// Default. Listed last so `#[serde(other)]` lands here — unknown
    /// wire values fall back to Stable rather than failing config load.
    #[serde(other)]
    Stable,
}

impl Default for Channel {
    fn default() -> Self {
        default_update_channel()
    }
}

impl Channel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Dev => "dev",
        }
    }

    /// Every defined channel, in display order. Surfaced to the
    /// settings UI so the dropdown is built from a single source.
    pub fn all() -> &'static [Channel] {
        &[Channel::Stable, Channel::Beta, Channel::Dev]
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateConfig {
    /// Automatically check for updates once per day
    #[serde(default)]
    pub auto_check: bool,
    /// Silently download and install updates when idle
    #[serde(default)]
    pub silent_update: bool,
    /// ISO 8601 timestamp of the last update check
    #[serde(default)]
    pub last_check_time: Option<String>,
    /// Version that was last installed via auto-update (to detect fresh updates)
    #[serde(default)]
    pub last_updated_version: Option<String>,
    /// Which release channel this install tracks.
    #[serde(default)]
    pub channel: Channel,
}

fn default_update_channel() -> Channel {
    // Dev builds embed "+dev." in the version (e.g. 0.9.202511171430+dev.abc1234),
    // beta builds embed "+beta.". Default new installs to the channel that
    // matches their build so the updater hits an endpoint that actually exists.
    let version = env!("CARGO_PKG_VERSION");
    if version.contains("+dev.") {
        Channel::Dev
    } else if version.contains("+beta.") {
        Channel::Beta
    } else {
        Channel::Stable
    }
}
