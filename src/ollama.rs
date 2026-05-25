//! First-class Ollama support.
//!
//! Ollama isn't an ACP agent — it's a local LLM runtime that exposes
//! an HTTP API on `http://localhost:11434` by default. Kage talks to
//! Ollama indirectly: the user picks an ACP agent (codex-acp is the
//! easiest, since it speaks OpenAI-compatible) and we point that
//! agent at Ollama's `/v1` endpoint.
//!
//! This module is the *discoverability* half — small HTTP probes the
//! settings page uses to:
//!
//!   - Tell the user "yes, Ollama is running" or "looks like it isn't —
//!     here's the install link."
//!   - Populate the model picker from `/api/tags` (the canonical
//!     Ollama "what's installed" endpoint).
//!   - Build a working `spawn_command` for codex-acp wired to Ollama
//!     via env vars, used by the "Use Ollama with Codex" one-click
//!     setup in Settings → Ollama.
//!
//! Why not a full ACP shim: an "Ollama-ACP" adapter is a separate
//! project that would have to translate Ollama's chat/generate API
//! into ACP message-pump semantics. The community-maintained
//! `codex-acp` already speaks OpenAI-compatible, and Ollama exposes
//! `/v1/chat/completions`. Letting the existing adapter point at
//! Ollama gets us a working setup today without us shipping a new
//! agent. If/when an ACP-native Ollama adapter shows up, swap the
//! preset over — the settings UI doesn't need to change.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Default base URL for a local Ollama install. The Settings page
/// pre-fills this; advanced users can point at a remote host (e.g.
/// a homelab GPU box) by overriding it.
pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";

/// Short timeout for the probe — Ollama responds in ms when running.
/// Anything longer than this is "not running" for UX purposes; the
/// user wants a fast answer in Settings, not a 30s wait.
const PROBE_TIMEOUT_SECS: u64 = 3;

/// Slightly longer for the model list — `/api/tags` walks disk and
/// can take a beat on machines with many large models.
const LIST_TIMEOUT_SECS: u64 = 8;

/// Single line of `/api/tags` output. Mirrors the Ollama wire format
/// shape — extra fields are ignored via serde defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// Tag-form name, e.g. `llama3:8b`. This is what the Codex
    /// adapter receives via `OPENAI_MODEL`.
    pub name: String,
    /// Optional size in bytes (Ollama returns this for installed
    /// models). Surfaced in the dropdown so the user can pick by
    /// "is this the 4 GB one or the 40 GB one." `None` if the
    /// payload didn't carry it (older Ollama versions) — UI just
    /// hides the size column in that case.
    #[serde(default)]
    pub size: Option<u64>,
    /// Optional last-modified timestamp string. Pass-through; the
    /// UI can format it or skip it.
    #[serde(default)]
    pub modified_at: Option<String>,
}

/// Outcome of a connection probe. The settings UI renders different
/// copy for each variant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status")]
pub enum ProbeResult {
    /// Ollama responded 200 with a recognisable payload. `version` is
    /// the daemon's reported version when available (`/api/version`),
    /// or `None` if we hit `/api/tags` to confirm reachability and
    /// the version probe failed.
    Reachable { version: Option<String> },
    /// Connection refused / DNS failure / TLS error. `reason` is a
    /// short human-readable string the UI surfaces below the URL.
    Unreachable { reason: String },
}

fn http_client(timeout_secs: u64) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent(format!("Kage/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("Failed to build HTTP client")
}

/// Normalise a base URL — strips trailing slashes, defaults to
/// http:// if no scheme present, fills in `localhost` if the user
/// just typed `:11434`. Pure so the settings UI's input handling and
/// the probe agree on the same canonical form.
pub fn normalize_base_url(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return DEFAULT_BASE_URL.to_string();
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else if let Some(rest) = trimmed.strip_prefix(':') {
        // ":11434" → "http://localhost:11434"
        format!("http://localhost:{}", rest)
    } else {
        format!("http://{}", trimmed)
    };
    with_scheme.trim_end_matches('/').to_string()
}

/// Hit `/api/version` (cheap, no model walk) — falls back to
/// `/api/tags` if `/api/version` returns 404 (older Ollama versions
/// didn't ship it). Either response counts as "reachable."
pub fn probe(base_url: &str) -> ProbeResult {
    let base = normalize_base_url(base_url);
    let client = match http_client(PROBE_TIMEOUT_SECS) {
        Ok(c) => c,
        Err(e) => {
            return ProbeResult::Unreachable {
                reason: e.to_string(),
            };
        }
    };

    // /api/version first — single tiny payload.
    match client.get(format!("{}/api/version", base)).send() {
        Ok(resp) if resp.status().is_success() => {
            let version = resp.json::<serde_json::Value>().ok().and_then(|v| {
                v.get("version")
                    .and_then(|s| s.as_str())
                    .map(str::to_string)
            });
            return ProbeResult::Reachable { version };
        }
        Ok(_) => {
            // Any other status (404 from older versions, 5xx) — fall
            // through to /api/tags to prove reachability.
        }
        Err(e) => {
            return ProbeResult::Unreachable {
                reason: classify_reqwest_error(&e),
            };
        }
    }

    match client.get(format!("{}/api/tags", base)).send() {
        Ok(resp) if resp.status().is_success() => ProbeResult::Reachable { version: None },
        Ok(resp) => ProbeResult::Unreachable {
            reason: format!("HTTP {}", resp.status().as_u16()),
        },
        Err(e) => ProbeResult::Unreachable {
            reason: classify_reqwest_error(&e),
        },
    }
}

/// List installed models via `/api/tags`. Returns an empty list when
/// Ollama is reachable but has no models installed (rather than
/// erroring) so the UI can show "no models — pull one with `ollama
/// pull llama3`" without a failure path.
pub fn list_models(base_url: &str) -> Result<Vec<ModelEntry>> {
    let base = normalize_base_url(base_url);
    let client = http_client(LIST_TIMEOUT_SECS)?;
    let resp = client
        .get(format!("{}/api/tags", base))
        .send()
        .context("Failed to reach Ollama /api/tags")?;
    if !resp.status().is_success() {
        anyhow::bail!("Ollama /api/tags returned {}", resp.status());
    }
    let body: serde_json::Value = resp.json().context("Failed to parse /api/tags response")?;
    let models = body
        .get("models")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| serde_json::from_value::<ModelEntry>(item.clone()).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(models)
}

/// Build a spawn command that launches `codex-acp` with the right env
/// vars to talk to Ollama. The exact incantation differs per platform
/// because we need a shell wrapper for `KEY=VALUE` syntax (the spawn
/// path splits on whitespace and doesn't support env injection
/// natively).
///
/// Returns a string suitable to drop into `AcpMode::Local
/// { spawn_command }`. Keep both shapes in mind for tests:
///   - Windows: `cmd /c set OPENAI_BASE_URL=… && set … && codex-acp`
///   - macOS / Linux: `env OPENAI_BASE_URL=… OPENAI_MODEL=… codex-acp`
pub fn build_codex_spawn_command(base_url: &str, model: &str) -> String {
    let base = normalize_base_url(base_url);
    // OPENAI_API_KEY just has to be non-empty for the OpenAI client
    // libraries to send anything; Ollama ignores it. We use a clearly-
    // labelled placeholder so logs / debug dumps don't mislead.
    let api_base = format!("{}/v1", base);
    let api_key = "ollama-no-key-required";

    if cfg!(target_os = "windows") {
        // `cmd /c "set X=Y && set Z=W && codex-acp"` — one /c for the
        // whole pipeline. Quoting around the chained command keeps
        // codex-acp's stdin/stdout streams unbuffered.
        format!(
            "cmd /c set OPENAI_BASE_URL={base}&& set OPENAI_API_BASE={base}&& set OPENAI_API_KEY={key}&& set OPENAI_MODEL={model}&& codex-acp",
            base = api_base,
            key = api_key,
            model = model,
        )
    } else {
        format!(
            "env OPENAI_BASE_URL={base} OPENAI_API_BASE={base} OPENAI_API_KEY={key} OPENAI_MODEL={model} codex-acp",
            base = api_base,
            key = api_key,
            model = model,
        )
    }
}

/// Classify a `reqwest::Error` into a short reason string for the UI.
/// Going through Display would surface `reqwest::Url` debug noise we
/// don't need; a coarse bucket is more readable.
fn classify_reqwest_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "Timed out — is Ollama running?".to_string()
    } else if e.is_connect() {
        "Connection refused — is Ollama running on this URL?".to_string()
    } else if e.is_decode() {
        "Reached the URL but couldn't parse the response — is this really Ollama?".to_string()
    } else {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_trailing_slash() {
        assert_eq!(normalize_base_url("http://x:1/"), "http://x:1");
        assert_eq!(normalize_base_url("http://x:1//"), "http://x:1");
    }

    #[test]
    fn normalize_adds_http_scheme() {
        assert_eq!(
            normalize_base_url("localhost:11434"),
            "http://localhost:11434"
        );
    }

    #[test]
    fn normalize_handles_port_only() {
        assert_eq!(normalize_base_url(":11434"), "http://localhost:11434");
    }

    #[test]
    fn normalize_keeps_https() {
        assert_eq!(normalize_base_url("https://my.host/"), "https://my.host");
    }

    #[test]
    fn normalize_falls_back_to_default_when_blank() {
        assert_eq!(normalize_base_url(""), DEFAULT_BASE_URL);
        assert_eq!(normalize_base_url("   "), DEFAULT_BASE_URL);
    }

    #[test]
    fn build_codex_spawn_includes_required_env_vars() {
        let cmd = build_codex_spawn_command("http://localhost:11434", "llama3:8b");
        // Both OpenAI-style env names must be present so the adapter
        // works regardless of which one it reads. OPENAI_BASE_URL is
        // the newer canonical; OPENAI_API_BASE is the legacy fallback.
        assert!(cmd.contains("OPENAI_BASE_URL="));
        assert!(cmd.contains("OPENAI_API_BASE="));
        assert!(cmd.contains("OPENAI_API_KEY="));
        assert!(cmd.contains("OPENAI_MODEL=llama3:8b"));
        assert!(cmd.contains("codex-acp"));
    }

    #[test]
    fn build_codex_spawn_normalises_url() {
        // Trailing slash + no scheme → /v1 still appended cleanly.
        let cmd = build_codex_spawn_command("localhost:11434/", "qwen2:7b");
        assert!(cmd.contains("OPENAI_BASE_URL=http://localhost:11434/v1"));
        assert!(cmd.contains("OPENAI_MODEL=qwen2:7b"));
    }

    #[test]
    fn model_entry_deserialises_from_tags_payload_shape() {
        // Real Ollama /api/tags response shape — the test pins the
        // fields we actually rely on so a wire format bump shows up
        // here rather than as a cryptic runtime decode failure.
        let raw = serde_json::json!({
            "name": "llama3:8b",
            "modified_at": "2026-01-15T12:34:56Z",
            "size": 4_700_000_000_u64,
            "digest": "ignored-by-us",
            "details": { "parent_model": "" }
        });
        let entry: ModelEntry = serde_json::from_value(raw).unwrap();
        assert_eq!(entry.name, "llama3:8b");
        assert_eq!(entry.size, Some(4_700_000_000));
        assert_eq!(entry.modified_at.as_deref(), Some("2026-01-15T12:34:56Z"));
    }

    #[test]
    fn model_entry_tolerates_missing_optional_fields() {
        // Older / minimal payloads (some third-party Ollama-compatible
        // servers don't return size or modified_at). Should still
        // decode the bare name.
        let raw = serde_json::json!({ "name": "tinyllama:latest" });
        let entry: ModelEntry = serde_json::from_value(raw).unwrap();
        assert_eq!(entry.name, "tinyllama:latest");
        assert_eq!(entry.size, None);
        assert_eq!(entry.modified_at, None);
    }

    #[test]
    fn probe_reports_unreachable_for_blocked_localhost_port() {
        // Hit a port nothing's listening on — should surface as a
        // connect-refused, not a timeout or decode error. Picks a
        // port well above the user-friendly range; if CI ever has
        // something on this port the test will helpfully fail loudly.
        let result = probe("http://127.0.0.1:1");
        match result {
            ProbeResult::Unreachable { reason } => {
                let lower = reason.to_lowercase();
                assert!(
                    lower.contains("refused")
                        || lower.contains("connection")
                        || lower.contains("connect"),
                    "expected connect-style failure, got: {}",
                    reason
                );
            }
            ProbeResult::Reachable { .. } => {
                panic!("expected unreachable for 127.0.0.1:1");
            }
        }
    }
}
