//! Internal sites — link previews for pages that need the user's own auth.
//!
//! A direct HTTP fetch can't preview a page behind a corporate SSO, an
//! intranet proxy, or anything else that authenticates with credentials
//! only the user's machine holds. Instead of teaching Kage about any
//! particular auth scheme, the user defines *profiles*, each made of:
//!
//!   1. **Detection rules** — declarative and deterministic. URL rules
//!      (domain suffix / regex) run before anything touches the network;
//!      a match skips the direct fetch entirely. Content rules run on
//!      whatever the direct fetch got back (body text or final URL,
//!      regardless of status) — that's how a login wall on a host the
//!      URL rules didn't know about is recognised.
//!   2. **A proxy script** — PowerShell or Bash, written by the user (or
//!      drafted by the agent). Kage runs it with the target URL in the
//!      `KAGE_URL` environment variable — never spliced into the script
//!      text — and reads the response from stdout. `KAGE_RESPONSE` is
//!      `text` for pages and `base64` for favicons.
//!
//! The script's stdout is either the page HTML (Kage extracts metadata
//! with the same parser as the direct path) or a JSON object carrying
//! `title` / `description` / `favicon` directly, for scripts that talk
//! to an API rather than scraping a page.
//!
//! Profiles live only in the user's local config. Nothing here knows
//! about specific hosts, and logging must never record the URLs or page
//! content that pass through — internal URLs are themselves sensitive.

use serde::{Deserialize, Serialize};

use kage_core::computer_control::script_runner::{run_script_with_env, RunOutcome, ScriptLang};

/// Default per-run proxy timeout. Generous enough for an SSO round trip,
/// short enough that a wedged script doesn't hold a preview forever.
pub const DEFAULT_PROXY_TIMEOUT_MS: u64 = 15_000;
/// Upper bound accepted from config.
pub const MAX_PROXY_TIMEOUT_MS: u64 = 60_000;
/// Largest decoded favicon we'll inline as a data URI.
pub const MAX_ICON_BYTES: usize = 64 * 1024;

/// One user-defined internal-site profile. Persisted in
/// `Config::internal_sites`; fields mirror the settings UI 1:1.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct InternalSiteProfile {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "crate::config::default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub url_rules: Vec<UrlRule>,
    #[serde(default)]
    pub content_rules: Vec<ContentRule>,
    #[serde(default)]
    pub proxy: ProxyScript,
    /// URL the profile was built from; used by the settings Test button
    /// and as context when the agent drafts rules or the script.
    #[serde(default)]
    pub sample_url: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UrlRuleKind {
    /// Host equals the pattern or is a subdomain of it.
    #[default]
    DomainSuffix,
    /// Regex matched anywhere in the full URL.
    Regex,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UrlRule {
    #[serde(default)]
    pub kind: UrlRuleKind,
    #[serde(default)]
    pub pattern: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContentTarget {
    /// The fetched body (the head window the direct path reads).
    #[default]
    Body,
    /// The URL after redirects — catches a bounce to a login host.
    FinalUrl,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    /// Case-insensitive substring.
    #[default]
    Contains,
    Regex,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ContentRule {
    #[serde(default)]
    pub target: ContentTarget,
    #[serde(default)]
    pub kind: MatchKind,
    #[serde(default)]
    pub pattern: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProxyLang {
    #[default]
    Powershell,
    Bash,
}

impl ProxyLang {
    fn script_lang(self) -> ScriptLang {
        match self {
            Self::Powershell => ScriptLang::PowerShell,
            Self::Bash => ScriptLang::Bash,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProxyScript {
    #[serde(default)]
    pub lang: ProxyLang,
    #[serde(default)]
    pub script: String,
    /// Per-run timeout; `0` / missing means [`DEFAULT_PROXY_TIMEOUT_MS`].
    #[serde(default)]
    pub timeout_ms: u64,
}

/// What a direct fetch saw, for content-rule evaluation.
pub struct DirectObservation<'a> {
    pub final_url: &'a str,
    pub body: &'a str,
}

/// Regexes come from user config and are compiled per evaluation. The
/// `regex` crate is linear-time, so a hostile pattern can't hang us; the
/// size limit just bounds compile cost.
fn compile(pattern: &str) -> Option<regex::Regex> {
    regex::RegexBuilder::new(pattern)
        .case_insensitive(true)
        .size_limit(1 << 20)
        .build()
        .ok()
}

fn host_matches_suffix(host: &str, suffix: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let suffix = suffix
        .trim()
        .trim_start_matches("*.")
        .trim_start_matches('.')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if suffix.is_empty() {
        return false;
    }
    host == suffix || host.ends_with(&format!(".{suffix}"))
}

impl UrlRule {
    pub fn matches(&self, url: &url::Url) -> bool {
        if self.pattern.trim().is_empty() {
            return false;
        }
        match self.kind {
            UrlRuleKind::DomainSuffix => url
                .host_str()
                .is_some_and(|h| host_matches_suffix(h, &self.pattern)),
            UrlRuleKind::Regex => {
                compile(&self.pattern).is_some_and(|re| re.is_match(url.as_str()))
            }
        }
    }
}

impl ContentRule {
    pub fn matches(&self, obs: &DirectObservation<'_>) -> bool {
        if self.pattern.trim().is_empty() {
            return false;
        }
        let haystack = match self.target {
            ContentTarget::Body => obs.body,
            ContentTarget::FinalUrl => obs.final_url,
        };
        match self.kind {
            MatchKind::Contains => haystack
                .to_lowercase()
                .contains(&self.pattern.to_lowercase()),
            MatchKind::Regex => compile(&self.pattern).is_some_and(|re| re.is_match(haystack)),
        }
    }
}

impl InternalSiteProfile {
    /// Usable profiles: enabled and with a script to run.
    fn is_active(&self) -> bool {
        self.enabled && !self.proxy.script.trim().is_empty()
    }

    pub fn matches_url(&self, url: &url::Url) -> bool {
        self.url_rules.iter().any(|r| r.matches(url))
    }

    pub fn matches_content(&self, obs: &DirectObservation<'_>) -> bool {
        self.content_rules.iter().any(|r| r.matches(obs))
    }
}

/// First active profile whose URL rules match. Order = priority.
pub fn match_url<'a>(
    profiles: &'a [InternalSiteProfile],
    url: &url::Url,
) -> Option<&'a InternalSiteProfile> {
    profiles
        .iter()
        .find(|p| p.is_active() && p.matches_url(url))
}

/// First active profile whose content rules match a direct fetch.
pub fn match_content<'a>(
    profiles: &'a [InternalSiteProfile],
    obs: &DirectObservation<'_>,
) -> Option<&'a InternalSiteProfile> {
    profiles
        .iter()
        .find(|p| p.is_active() && p.matches_content(obs))
}

/// True when any active profile has content rules — lets the direct
/// path skip reading error bodies when nobody would inspect them.
pub fn any_content_rules(profiles: &[InternalSiteProfile]) -> bool {
    profiles
        .iter()
        .any(|p| p.is_active() && !p.content_rules.is_empty())
}

/// How the proxy should return the response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyResponse {
    Text,
    Base64,
}

impl ProxyResponse {
    fn env_value(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Base64 => "base64",
        }
    }
}

/// Result of one proxy run, kept verbose for the settings Test button.
/// Callers that log must use only `exit_code` / `timed_out` — stdout and
/// stderr can carry internal URLs and page content.
#[derive(Debug, Clone, Serialize)]
pub struct ProxyRun {
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Run a profile's proxy script for `url`. Blocking — call from a
/// blocking-pool thread.
pub fn run_proxy(profile: &InternalSiteProfile, url: &str, response: ProxyResponse) -> ProxyRun {
    let timeout = match profile.proxy.timeout_ms {
        0 => DEFAULT_PROXY_TIMEOUT_MS,
        t => t.min(MAX_PROXY_TIMEOUT_MS),
    };
    let env = [("KAGE_URL", url), ("KAGE_RESPONSE", response.env_value())];
    match run_script_with_env(
        profile.proxy.lang.script_lang(),
        &profile.proxy.script,
        Some(timeout),
        &env,
    ) {
        RunOutcome::Ran(r) => ProxyRun {
            ok: r.exit_code == Some(0) && !r.timed_out,
            exit_code: r.exit_code,
            timed_out: r.timed_out,
            stdout: r.stdout,
            stderr: r.stderr,
        },
        RunOutcome::Unsupported(msg) => ProxyRun {
            ok: false,
            exit_code: None,
            timed_out: false,
            stdout: String::new(),
            stderr: msg,
        },
    }
}

/// Metadata fields a proxy can hand back directly as JSON.
#[derive(Debug, Default, PartialEq)]
pub struct DirectMetadata {
    pub title: Option<String>,
    pub description: Option<String>,
    pub favicon: Option<String>,
}

/// Interpret proxy stdout: a JSON object yields metadata directly;
/// anything else is treated as HTML (`None`).
pub fn parse_json_output(stdout: &str) -> Option<DirectMetadata> {
    let trimmed = stdout.trim_start_matches('\u{feff}').trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let obj: serde_json::Map<String, serde_json::Value> = serde_json::from_str(trimmed).ok()?;
    let field = |k: &str| {
        obj.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(DirectMetadata {
        title: field("title"),
        description: field("description"),
        favicon: field("favicon"),
    })
}

/// Image types we'll inline. SVG is deliberately absent: it's markup,
/// and the extension HTML sanitizer only admits raster data URIs.
fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0, 0, 1, 0]) {
        Some("image/x-icon")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Turn base64 proxy output into a `data:` URI, or `None` if it isn't a
/// small raster image. The MIME type comes from the bytes, never from
/// the script, so a misbehaving script can't smuggle markup through.
pub fn icon_data_uri(base64_stdout: &str) -> Option<String> {
    use base64::Engine;
    let cleaned: String = base64_stdout
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{feff}')
        .collect();
    if cleaned.is_empty() || cleaned.len() > MAX_ICON_BYTES * 4 / 3 + 4 {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .ok()?;
    let mime = sniff_image_mime(&bytes)?;
    Some(format!(
        "data:{};base64,{}",
        mime,
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

/// Accept a `data:` favicon a JSON-mode proxy returned only if it
/// re-validates as a small raster image.
pub fn sanitize_data_icon(uri: &str) -> Option<String> {
    let (_, b64) = uri.strip_prefix("data:")?.split_once(";base64,")?;
    icon_data_uri(b64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    fn profile(url_rules: Vec<UrlRule>, content_rules: Vec<ContentRule>) -> InternalSiteProfile {
        InternalSiteProfile {
            id: "p".into(),
            name: "Test".into(),
            enabled: true,
            url_rules,
            content_rules,
            proxy: ProxyScript {
                script: "echo hi".into(),
                ..Default::default()
            },
            sample_url: String::new(),
        }
    }

    fn suffix(p: &str) -> UrlRule {
        UrlRule {
            kind: UrlRuleKind::DomainSuffix,
            pattern: p.into(),
        }
    }

    #[test]
    fn domain_suffix_matches_host_and_subdomains_only() {
        let r = suffix("corp.example.test");
        assert!(r.matches(&url("https://corp.example.test/x")));
        assert!(r.matches(&url("https://wiki.corp.example.test/x")));
        assert!(!r.matches(&url("https://notcorp.example.test/x")));
        assert!(!r.matches(&url("https://corp.example.test.evil.test/")));
        assert!(suffix("*.corp.example.test").matches(&url("https://a.corp.example.test")));
        assert!(suffix(".CORP.example.test").matches(&url("https://A.corp.example.test")));
        assert!(!suffix("  ").matches(&url("https://a.test")));
    }

    #[test]
    fn regex_rule_matches_full_url_and_ignores_bad_patterns() {
        let r = UrlRule {
            kind: UrlRuleKind::Regex,
            pattern: r"^https://[^/]+\.internal\.test/docs/".into(),
        };
        assert!(r.matches(&url("https://a.internal.test/docs/page")));
        assert!(!r.matches(&url("https://a.internal.test/other")));
        let bad = UrlRule {
            kind: UrlRuleKind::Regex,
            pattern: "(".into(),
        };
        assert!(!bad.matches(&url("https://a.test/(")));
    }

    #[test]
    fn content_rules_check_body_or_final_url() {
        let obs = DirectObservation {
            final_url: "https://sso.example.test/login?next=x",
            body: "<title>Sign In Required</title>",
        };
        let body = ContentRule {
            target: ContentTarget::Body,
            kind: MatchKind::Contains,
            pattern: "sign in required".into(),
        };
        let final_url = ContentRule {
            target: ContentTarget::FinalUrl,
            kind: MatchKind::Regex,
            pattern: r"//sso\.example\.test/".into(),
        };
        let miss = ContentRule {
            target: ContentTarget::FinalUrl,
            kind: MatchKind::Contains,
            pattern: "sign in".into(),
        };
        assert!(body.matches(&obs));
        assert!(final_url.matches(&obs));
        assert!(!miss.matches(&obs));
    }

    #[test]
    fn match_skips_disabled_and_scriptless_profiles() {
        let mut disabled = profile(vec![suffix("a.test")], vec![]);
        disabled.enabled = false;
        let mut empty = profile(vec![suffix("a.test")], vec![]);
        empty.proxy.script = "  ".into();
        let mut live = profile(vec![suffix("a.test")], vec![]);
        live.id = "live".into();
        let profiles = vec![disabled, empty, live];
        assert_eq!(
            match_url(&profiles, &url("https://x.a.test")).map(|p| p.id.as_str()),
            Some("live")
        );
        assert!(match_url(&profiles, &url("https://b.test")).is_none());
    }

    #[test]
    fn any_content_rules_ignores_inactive_profiles() {
        let rule = ContentRule {
            pattern: "x".into(),
            ..Default::default()
        };
        let mut p = profile(vec![], vec![rule]);
        assert!(any_content_rules(std::slice::from_ref(&p)));
        p.enabled = false;
        assert!(!any_content_rules(&[p]));
    }

    #[test]
    fn json_output_is_parsed_and_html_is_not() {
        let md =
            parse_json_output("\u{feff} {\"title\":\" Page \",\"description\":\"\",\"x\":1}\n")
                .unwrap();
        assert_eq!(md.title.as_deref(), Some("Page"));
        assert_eq!(md.description, None);
        assert!(parse_json_output("<html><title>x</title>").is_none());
        assert!(parse_json_output("{not json").is_none());
    }

    #[test]
    fn icon_data_uri_sniffs_raster_and_rejects_markup() {
        use base64::Engine;
        let png = b"\x89PNG\r\n\x1a\n rest";
        let b64 = base64::engine::general_purpose::STANDARD.encode(png);
        let uri = icon_data_uri(&format!("{}\r\n", b64)).unwrap();
        assert!(uri.starts_with("data:image/png;base64,"));
        let svg = base64::engine::general_purpose::STANDARD.encode(b"<svg onload=x>");
        assert!(icon_data_uri(&svg).is_none());
        assert!(icon_data_uri("not base64!!").is_none());
        let big = base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_ICON_BYTES + 10]);
        assert!(icon_data_uri(&big).is_none());
        assert!(sanitize_data_icon(&uri).is_some());
        assert!(sanitize_data_icon(&format!("data:image/svg+xml;base64,{svg}")).is_none());
    }

    #[test]
    fn profile_deserializes_from_minimal_json() {
        let p: InternalSiteProfile = serde_json::from_str(
            r#"{"name":"x","url_rules":[{"pattern":"a.test"}],"proxy":{"lang":"bash","script":"cat"}}"#,
        )
        .unwrap();
        assert!(p.enabled);
        assert_eq!(p.url_rules[0].kind, UrlRuleKind::DomainSuffix);
        assert_eq!(p.proxy.lang, ProxyLang::Bash);
        assert_eq!(p.proxy.timeout_ms, 0);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn proxy_receives_url_via_env_not_script_text() {
        let mut p = profile(vec![], vec![]);
        p.proxy.lang = ProxyLang::Bash;
        p.proxy.script = r#"printf '%s|%s' "$KAGE_URL" "$KAGE_RESPONSE""#.into();
        let run = run_proxy(
            &p,
            "https://a.test/?q=$(touch /tmp/x)",
            ProxyResponse::Base64,
        );
        assert!(run.ok, "{run:?}");
        assert_eq!(run.stdout, "https://a.test/?q=$(touch /tmp/x)|base64");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn proxy_receives_url_via_env_not_script_text() {
        let mut p = profile(vec![], vec![]);
        p.proxy.script = r#"[Console]::Out.Write("$env:KAGE_URL|$env:KAGE_RESPONSE")"#.into();
        let run = run_proxy(&p, "https://a.test/?q=$(Get-Date)", ProxyResponse::Text);
        assert!(run.ok, "{run:?}");
        assert_eq!(run.stdout, "https://a.test/?q=$(Get-Date)|text");
    }
}
