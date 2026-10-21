//! Settings-side commands for internal-site profiles (see
//! `src/internal_sites.rs`): probe what a direct fetch sees, test a
//! profile end to end, and have the agent draft detection rules or a
//! proxy script from a sample URL.
//!
//! All of these are settings-only (`null` in
//! `ui/js/shared/extension-permissions.js`): they run user scripts and
//! return internal page content, so no extension may reach them.

use crate::commands::input::link_metadata::{
    direct_fetch, metadata_from_proxy_run, page_title, run_proxy_limited,
};
use crate::error::AppError;
use crate::internal_sites::{self, InternalSiteProfile, ProxyLang, ProxyResponse};
use crate::state::{AcpHandles, FeatureServices};
use serde::Serialize;
use tauri::State;

/// How much of the probed body the settings UI (and the drafting
/// prompt) gets to see.
const EXCERPT_CHARS: usize = 2000;
/// Tail of proxy stderr shown in the Test result.
const STDERR_TAIL_CHARS: usize = 2000;

fn require_http_url(url: &str) -> Result<url::Url, AppError> {
    let parsed = url::Url::parse(url.trim()).map_err(|_| AppError::from("Invalid URL"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        _ => Err(AppError::from("Invalid URL")),
    }
}

fn head_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn tail_chars(s: &str, n: usize) -> String {
    let count = s.chars().count();
    s.chars().skip(count.saturating_sub(n)).collect()
}

/// What an unauthenticated direct fetch of a URL saw.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    pub ok: bool,
    pub status: Option<u16>,
    pub final_url: Option<String>,
    pub title: Option<String>,
    pub excerpt: String,
    pub error: Option<String>,
}

/// Probe plus the full body window, for content-rule evaluation.
async fn probe_with_body(url: &str) -> (ProbeResult, String) {
    match direct_fetch(url, true).await {
        Ok(d) => (
            ProbeResult {
                ok: true,
                status: Some(d.status.as_u16()),
                final_url: Some(d.final_url.clone()),
                title: page_title(&d.body),
                excerpt: head_chars(&d.body, EXCERPT_CHARS),
                error: None,
            },
            d.body,
        ),
        Err(e) => (
            ProbeResult {
                ok: false,
                status: None,
                final_url: None,
                title: None,
                excerpt: String::new(),
                error: Some(e.to_string()),
            },
            String::new(),
        ),
    }
}

#[tauri::command]
pub async fn internal_sites_probe(url: String) -> Result<ProbeResult, AppError> {
    let url = require_http_url(&url)?.to_string();
    Ok(probe_with_body(&url).await.0)
}

#[derive(Debug, Serialize)]
pub struct ProxyReport {
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout_bytes: usize,
    pub stderr_tail: String,
}

#[derive(Debug, Serialize)]
pub struct TestResult {
    /// The profile's URL rules match this URL.
    pub url_match: bool,
    /// The profile's content rules match the direct fetch; `None` when
    /// the direct fetch itself failed.
    pub content_match: Option<bool>,
    pub probe: ProbeResult,
    pub proxy: ProxyReport,
    /// The card the proxy path would produce.
    pub metadata: Option<serde_json::Value>,
}

/// Run a (possibly unsaved) profile against `url`: report which rules
/// match, run the proxy script regardless, and return the card it yields.
#[tauri::command]
pub async fn internal_sites_test(
    profile: InternalSiteProfile,
    url: String,
) -> Result<TestResult, AppError> {
    let parsed = require_http_url(&url)?;
    let url = parsed.to_string();
    if profile.proxy.script.trim().is_empty() {
        return Err(AppError::from("The proxy script is empty"));
    }
    let url_match = profile.matches_url(&parsed);
    let (probe, body) = probe_with_body(&url).await;
    let content_match = probe.ok.then(|| {
        profile.matches_content(&internal_sites::DirectObservation {
            final_url: probe.final_url.as_deref().unwrap_or(""),
            body: &body,
        })
    });

    let run = run_proxy_limited(&profile, &url, ProxyResponse::Text).await;
    let metadata = metadata_from_proxy_run(&profile, &url, &run).await.ok();
    Ok(TestResult {
        url_match,
        content_match,
        probe,
        proxy: ProxyReport {
            ok: run.ok,
            exit_code: run.exit_code,
            timed_out: run.timed_out,
            stdout_bytes: run.stdout.len(),
            stderr_tail: tail_chars(&run.stderr, STDERR_TAIL_CHARS),
        },
        metadata,
    })
}

/// Agent-drafted detection rules. Fields mirror `InternalSiteProfile`
/// so the settings UI can merge them straight in.
#[derive(Debug, Default, Serialize, serde::Deserialize, PartialEq)]
pub struct DraftRules {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url_rules: Vec<internal_sites::UrlRule>,
    #[serde(default)]
    pub content_rules: Vec<internal_sites::ContentRule>,
}

fn rules_prompt(url: &str, probe: &ProbeResult) -> String {
    let observed = if probe.ok {
        format!(
            "Status: {}\nFinal URL after redirects: {}\nPage title: {}\n<excerpt>\n{}\n</excerpt>",
            probe.status.map(|s| s.to_string()).unwrap_or_default(),
            probe.final_url.as_deref().unwrap_or(""),
            probe.title.as_deref().unwrap_or("(none)"),
            probe.excerpt
        )
    } else {
        format!(
            "The direct fetch failed: {}",
            probe.error.as_deref().unwrap_or("unknown error")
        )
    };
    format!(
        "You are configuring link previews for an internal website that needs the \
user's own authentication. Kage previews links by fetching them directly, without \
credentials; for sites like this one it must instead recognise them and hand them to \
a proxy script.\n\n\
Sample URL: {url}\n\n\
What Kage's unauthenticated direct fetch of that URL saw:\n{observed}\n\n\
Write detection rules for the family of internal sites this URL belongs to, as a \
single JSON object and nothing else:\n\
{{\"name\": \"short label for this group of sites\",\n \
\"url_rules\": [{{\"kind\": \"domain_suffix\" | \"regex\", \"pattern\": \"...\"}}],\n \
\"content_rules\": [{{\"target\": \"body\" | \"final_url\", \"kind\": \"contains\" | \"regex\", \"pattern\": \"...\"}}]}}\n\n\
Guidance:\n\
- url_rules: domain_suffix matches a host and all its subdomains; regex is matched \
against the full URL. Use the narrowest suffix that still covers sibling internal \
sites, and never one that would match public websites.\n\
- content_rules recognise what an unauthenticated fetch of such a site returns (for \
example the sign-in page or redirect above), so internal hosts the url_rules miss are \
still caught. Use stable markers such as the sign-in host or form, never text that is \
specific to this one page. Use an empty list if the response has nothing distinctive.\n\
- Do not use tools; answer from the information above."
    )
}

fn script_prompt(url: &str, lang: ProxyLang) -> String {
    let lang_name = match lang {
        ProxyLang::Powershell => "PowerShell (Windows PowerShell 5.1 compatible)",
        ProxyLang::Bash => "Bash",
    };
    format!(
        "Write a {lang_name} script that fetches internal web pages the way you would \
read them with your own tools in this environment, authenticating as the current user. \
Kage runs it to build link previews for pages it cannot fetch directly.\n\n\
Contract:\n\
- The URL is in the environment variable KAGE_URL. Never hardcode it and never \
evaluate it as code.\n\
- KAGE_RESPONSE is \"text\" or \"base64\". For \"text\", print the response body (the \
page HTML) to stdout. For \"base64\", print the raw response bytes base64-encoded \
(used for favicons).\n\
- For \"text\" you may instead print a JSON object {{\"title\": \"...\", \
\"description\": \"...\"}} if the site is better read through an API.\n\
- Exit 0 on success and non-zero on failure. Write diagnostics to stderr only.\n\
- Never prompt or wait for input, and finish within 15 seconds.\n\
- Never print cookies, tokens or other secrets.\n\n\
Sample URL: {url}\n\n\
First work out how this environment authenticates to that host: look at how the tools \
you use to read internal sites do it (cookie files, credential helpers, command-line \
tools, auth headers) and inspect their configuration if you need to. Prefer reusing \
that same mechanism over inventing a new one. If you can, run the script against the \
sample URL to check it works.\n\n\
Reply with the final script in a single fenced code block."
    )
}

/// The outermost `{...}` in an agent reply.
fn extract_json_object(reply: &str) -> Option<&str> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    (end > start).then(|| &reply[start..=end])
}

/// Body of the first fenced code block in an agent reply.
fn extract_code_block(reply: &str) -> Option<String> {
    let start = reply.find("```")?;
    let after = &reply[start + 3..];
    // Skip the info string (e.g. "powershell") on the opening fence line.
    let body_start = after.find('\n')? + 1;
    let body = &after[body_start..];
    let end = body.find("```").unwrap_or(body.len());
    let script = body[..end].trim_end();
    (!script.trim().is_empty()).then(|| script.to_string())
}

async fn prompt_agent(
    acp: &State<'_, AcpHandles>,
    features: &State<'_, FeatureServices>,
    prompt: String,
) -> Result<String, AppError> {
    let client = acp.client.clone();
    let config = features.config.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::ephemeral_session::prompt_once(&client, &config, &prompt)
            .map_err(|e| AppError::internal(format!("Agent request failed: {}", e)))
    })
    .await
    .map_err(|e| AppError::internal(format!("Draft task failed: {}", e)))?
}

/// Have the agent draft detection rules from a sample URL. The direct
/// fetch is probed first so the agent can see what an unauthenticated
/// request gets back (the basis for content rules).
#[tauri::command]
pub async fn internal_sites_draft_rules(
    url: String,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
) -> Result<DraftRules, AppError> {
    let url = require_http_url(&url)?.to_string();
    let (probe, _) = probe_with_body(&url).await;
    let reply = prompt_agent(&acp, &features, rules_prompt(&url, &probe)).await?;
    extract_json_object(&reply)
        .and_then(|json| serde_json::from_str::<DraftRules>(json).ok())
        .ok_or_else(|| AppError::internal("The agent's reply did not contain usable rules"))
}

/// Have the agent draft a proxy script for a sample URL.
#[tauri::command]
pub async fn internal_sites_draft_script(
    url: String,
    lang: ProxyLang,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
) -> Result<String, AppError> {
    let url = require_http_url(&url)?.to_string();
    let reply = prompt_agent(&acp, &features, script_prompt(&url, lang)).await?;
    extract_code_block(&reply)
        .ok_or_else(|| AppError::internal("The agent's reply did not contain a script"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_object_from_chatty_reply() {
        let reply = "Here you go:\n```json\n{\"name\":\"Wiki\",\"url_rules\":[{\"pattern\":\"wiki.example.test\"}]}\n```\nDone.";
        let rules: DraftRules = serde_json::from_str(extract_json_object(reply).unwrap()).unwrap();
        assert_eq!(rules.name, "Wiki");
        assert_eq!(rules.url_rules[0].pattern, "wiki.example.test");
        assert_eq!(
            rules.url_rules[0].kind,
            internal_sites::UrlRuleKind::DomainSuffix
        );
        assert!(rules.content_rules.is_empty());
        assert!(extract_json_object("no json here").is_none());
        assert!(extract_json_object("} backwards {").is_none());
    }

    #[test]
    fn extracts_first_code_block_without_info_string() {
        let reply =
            "Script:\n```powershell\n$u = $env:KAGE_URL\nWrite-Output $u\n```\nand ```other```";
        assert_eq!(
            extract_code_block(reply).as_deref(),
            Some("$u = $env:KAGE_URL\nWrite-Output $u")
        );
        assert!(extract_code_block("no fence").is_none());
        assert!(extract_code_block("```\n   \n```").is_none());
        // Unterminated fence: take the rest.
        assert_eq!(
            extract_code_block("```bash\necho hi").as_deref(),
            Some("echo hi")
        );
    }

    #[test]
    fn prompts_carry_the_url_and_observation() {
        let probe = ProbeResult {
            ok: true,
            status: Some(302),
            final_url: Some("https://login.example.test/".into()),
            title: Some("Sign in".into()),
            excerpt: "<form id=login>".into(),
            error: None,
        };
        let p = rules_prompt("https://wiki.example.test/page", &probe);
        assert!(p.contains("https://wiki.example.test/page"));
        assert!(p.contains("https://login.example.test/"));
        assert!(p.contains("<form id=login>"));
        let s = script_prompt("https://wiki.example.test/page", ProxyLang::Bash);
        assert!(s.contains("KAGE_URL") && s.contains("Bash"));
    }

    #[test]
    fn rejects_non_http_urls() {
        assert!(require_http_url("file:///etc/passwd").is_err());
        assert!(require_http_url("not a url").is_err());
        assert!(require_http_url(" https://a.example.test/ ").is_ok());
    }

    #[test]
    fn char_windows_respect_multibyte_text() {
        assert_eq!(head_chars("ééé", 2), "éé");
        assert_eq!(tail_chars("abcdé", 2), "dé");
        assert_eq!(tail_chars("ab", 5), "ab");
    }
}
