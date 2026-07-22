use super::default_true;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PocketTtsConfig {
    /// Enable pocket-tts as the TTS engine (instead of browser speechSynthesis)
    #[serde(default)]
    pub enabled: bool,
    /// Voice to use (built-in: alba, marius, javert, jean, fantine, cosette, eponine, azelma)
    #[serde(default = "default_pocket_tts_voice")]
    pub voice: String,
    /// Port for the pocket-tts HTTP server
    #[serde(default = "default_pocket_tts_port")]
    pub port: u16,
    /// Path to Python executable (auto-detected if empty)
    #[serde(default)]
    pub python_path: Option<String>,
    /// Whether pocket-tts pip package is installed
    #[serde(default)]
    pub installed: bool,
    /// Auto-start the TTS server when the app launches
    #[serde(default)]
    pub auto_start: bool,
    /// Sampling temperature (0.3=consistent, 0.7=default, 1.0=expressive)
    #[serde(default = "default_pocket_tts_temp")]
    pub temp: f32,
    /// End-of-sequence threshold (default: -4.0, lower = less likely to stop early)
    #[serde(default = "default_pocket_tts_eos_threshold")]
    pub eos_threshold: f32,
}

fn default_pocket_tts_voice() -> String {
    "alba".to_string()
}

fn default_pocket_tts_port() -> u16 {
    9877
}

fn default_pocket_tts_temp() -> f32 {
    0.7
}

fn default_pocket_tts_eos_threshold() -> f32 {
    -4.0
}

impl Default for PocketTtsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            voice: "alba".to_string(),
            port: 9877,
            python_path: None,
            installed: false,
            auto_start: false,
            temp: 0.7,
            eos_threshold: -4.0,
        }
    }
}

/// Anonymous product analytics configuration.
///
/// We collect minimum viable telemetry through Aptabase: a randomly-generated
/// install ID, app version, OS/locale, and feature-usage event names. No
/// prompts, file paths, clipboard contents, or PII. See docs/PRIVACY.md for
/// the full disclosure.
///
/// Defaults:
///  - `enabled`: `true`. Opt-out with clear disclosure on the welcome screen
///    and a toggle in Settings → Privacy. Kept simple for now — if the build
///    was produced without an APTABASE_KEY the plugin is a no-op anyway, so
///    this flag only matters for distribution builds.
///  - `install_id`: generated lazily on first use (not here) so resetting it
///    via Settings actually changes the ID sent to Aptabase.
///  - `consent_version`: bumped whenever the privacy policy materially
///    changes. The UI compares this to the current policy version and
///    re-prompts if it lags behind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    /// Whether to send anonymous usage events. Respected by every call site
    /// through `telemetry::track()`, which short-circuits when false.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Anonymous UUID generated on first consent. Not linked to any account
    /// or device fingerprint — the user can reset it from Settings at any
    /// time, which orphans all prior events for that install.
    #[serde(default)]
    pub install_id: Option<String>,
    /// Version of the privacy policy the user last consented to. If the
    /// current `PRIVACY_POLICY_VERSION` exceeds this, we re-prompt.
    #[serde(default)]
    pub consent_version: u32,
    /// ISO 8601 date (YYYY-MM-DD) of the last `app_daily_active` event. Used
    /// to throttle that event to once per UTC day per install so DAU counts
    /// aren't skewed by users who open/close the app many times.
    #[serde(default)]
    pub last_daily_ping: Option<String>,
    /// The app version that last fired `app_started`. Used to detect upgrades
    /// (fire `app_upgraded` when this differs from the current version) and
    /// first installs (fire `app_installed` when this is `None`).
    #[serde(default)]
    pub last_seen_version: Option<String>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            install_id: None,
            consent_version: 0,
            last_daily_ping: None,
            last_seen_version: None,
        }
    }
}
