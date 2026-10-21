use crate::error::AppError;
use crate::internal_sites::{self, InternalSiteProfile, ProxyResponse};
use crate::state::FeatureServices;
use kage_core::lock_ext::LockExt;
use std::sync::LazyLock;
use tauri::State;

/// Extraction window: every tag we read lives in the document head.
const MAX_HTML_BYTES: usize = 32 * 1024;

const HEAD_CLOSE: &[u8] = b"</head>";

/// One client for the process so a chat full of links reuses connections
/// and TLS setup instead of rebuilding them per preview.
static HTTP_CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("Mozilla/5.0 (compatible; Kage/1.0)")
        .build()
        .map_err(|e| e.to_string())
});

/// Case-insensitive search for `</head>`; past it there's nothing the
/// extractors look at.
fn contains_head_close(bytes: &[u8]) -> bool {
    bytes
        .windows(HEAD_CLOSE.len())
        .any(|w| w.eq_ignore_ascii_case(HEAD_CLOSE))
}

/// Fetch metadata (title, description, image, favicon) from a URL for
/// link previews. Consults the on-disk cache first; on a fresh hit
/// returns the cached value (including the negative-cache `null`
/// shape) without going to the network. Misses do the live fetch and
/// persist before returning. See `src/link_metadata_cache.rs` for the
/// TTL + eviction policy.
///
/// URLs matching a user-defined internal-site profile are fetched by
/// that profile's proxy script instead of directly — see
/// `src/internal_sites.rs`.
#[tauri::command]
pub async fn fetch_link_metadata(
    url: String,
    features: State<'_, FeatureServices>,
) -> Result<serde_json::Value, AppError> {
    // Validate URL
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("Invalid URL".into());
    }

    // Cache lookup first. Disk reads are blocking but tiny — a
    // single JSON file deserialised on each call. Acceptable here
    // because a hit avoids a full HTTP round trip; we'd lose more
    // than we save by trying to be clever.
    let cache_url = url.clone();
    let cached = tauri::async_runtime::spawn_blocking(move || {
        crate::link_metadata_cache::lookup(&cache_url)
    })
    .await
    .ok()
    .flatten();
    if let Some(maybe_meta) = cached {
        return match maybe_meta {
            Some(value) => Ok(value),
            // Negative cache hit — surface the same empty-shape
            // response the live path returns, so callers don't need
            // to special-case "we tried and got nothing." The
            // background TTL guarantees we'll re-fetch in an hour
            // even if this stays in the cache.
            None => Ok(empty_metadata(&url)),
        };
    }

    let profiles = features.config.lock_or_recover().internal_sites.clone();

    // Cache miss — go to the network. Capture the outcome so we
    // can persist it (negative or positive) before returning. The
    // helper is split out so both arms write through the same cache
    // path without having to thread an Option around.
    let fetched = fetch_link_metadata_uncached(&url, &profiles).await;
    let to_persist: Option<serde_json::Value> = match &fetched {
        Ok(value) => Some(value.clone()),
        Err(_) => None, // negative cache: short TTL re-tries soon
    };
    let url_for_store = url.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = crate::link_metadata_cache::store(&url_for_store, to_persist);
    });

    fetched
}

fn empty_metadata(url: &str) -> serde_json::Value {
    serde_json::json!({
        "url": url,
        "title": null,
        "description": null,
        "image": null,
        "favicon": null,
    })
}

/// Fields extracted from a page head.
struct PageMeta {
    title: Option<String>,
    description: Option<String>,
    image: Option<String>,
    favicon: Option<String>,
}

fn extract_page_meta(html: &str, base_url: &str) -> PageMeta {
    let title = extract_meta(html, "og:title")
        .or_else(|| extract_meta(html, "twitter:title"))
        .or_else(|| extract_tag_content(html, "title"));

    let description = extract_meta(html, "og:description")
        .or_else(|| extract_meta(html, "description"))
        .or_else(|| extract_meta(html, "twitter:description"));

    // Hero image (Open Graph / Twitter Card) — separate from the
    // favicon since the formatter renders them in different places.
    // Folding both into one field (which previous versions did) meant
    // the card always showed a 28-px favicon even when the page had a
    // proper og:image; the result looked half-built next to peer link
    // previews in Slack / Discord / Notion.
    let image = extract_meta(html, "og:image")
        .or_else(|| extract_meta(html, "twitter:image"))
        .or_else(|| extract_meta(html, "twitter:image:src"))
        .map(|raw| resolve_url(&raw, base_url));

    // Favicon is a small site-identity glyph. We try the explicit
    // <link rel="icon"> first, then a /favicon.ico fallback. Never
    // reuse the hero image — they have different aspect ratios and
    // CSS treatment.
    let favicon = extract_link_icon(html, base_url);

    PageMeta {
        title,
        description,
        image,
        favicon,
    }
}

/// Live half of `fetch_link_metadata`. Pulled out so the command
/// wrapper can sit a cache check + cache write around it without
/// nesting too deeply. Returns the same shape the command returns.
///
/// Routing: a URL-rule match goes straight to the profile's proxy,
/// before anything leaves the machine. Otherwise the direct fetch runs,
/// and what it saw (final URL and body — error responses included) is
/// checked against content rules, which is how login walls are caught.
async fn fetch_link_metadata_uncached(
    url: &str,
    profiles: &[InternalSiteProfile],
) -> Result<serde_json::Value, AppError> {
    let parsed = url::Url::parse(url).map_err(|_| AppError::from("Invalid URL"))?;
    if let Some(profile) = internal_sites::match_url(profiles, &parsed) {
        return proxy_link_metadata(profile, url).await;
    }

    let inspect = internal_sites::any_content_rules(profiles);
    let direct = direct_fetch(url, inspect).await?;
    if inspect {
        let obs = internal_sites::DirectObservation {
            final_url: &direct.final_url,
            body: &direct.body,
        };
        if let Some(profile) = internal_sites::match_content(profiles, &obs) {
            return proxy_link_metadata(profile, url).await;
        }
    }

    if !direct.status.is_success() {
        return Err(format!("HTTP {}", direct.status).into());
    }
    // Only process HTML responses
    if !direct.is_html {
        return Ok(empty_metadata(&direct.final_url));
    }

    let meta = extract_page_meta(&direct.body, &direct.final_url);
    Ok(serde_json::json!({
        "url": direct.final_url,
        "title": meta.title,
        "description": meta.description,
        "image": meta.image,
        "favicon": meta.favicon,
    }))
}

/// What the direct fetch saw.
pub(crate) struct DirectResponse {
    pub(crate) status: reqwest::StatusCode,
    pub(crate) final_url: String,
    is_html: bool,
    /// Head window of the body. Read for HTML successes, and for error
    /// responses when `read_error_body` is set (content rules need the
    /// login / 401 page); empty otherwise.
    pub(crate) body: String,
}

pub(crate) async fn direct_fetch(
    url: &str,
    read_error_body: bool,
) -> Result<DirectResponse, AppError> {
    let client = HTTP_CLIENT
        .as_ref()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Fetch error: {}", e))?;

    let final_url = resp.url().to_string();
    let status = resp.status();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let is_html = content_type.contains("text/html");

    let wants_body = if status.is_success() {
        is_html
    } else {
        read_error_body
    };
    if !wants_body {
        return Ok(DirectResponse {
            status,
            final_url,
            is_html,
            body: String::new(),
        });
    }

    // Read only the first 32KB to extract meta tags. Stream it: `bytes()`
    // buffers the whole body, so a huge page blew the 5s timeout and got
    // negative-cached even though its <head> arrived in the first chunk.
    let mut buf: Vec<u8> = Vec::with_capacity(MAX_HTML_BYTES);
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let scan_from = buf.len().saturating_sub(HEAD_CLOSE.len() - 1);
                buf.extend_from_slice(&chunk);
                if buf.len() >= MAX_HTML_BYTES || contains_head_close(&buf[scan_from..]) {
                    break;
                }
            }
            Ok(None) => break,
            // Timed out or dropped mid-body: a partial head still has the
            // tags we want, so only fail if nothing arrived.
            Err(e) if buf.is_empty() && status.is_success() => {
                return Err(format!("Read error: {}", e).into())
            }
            Err(_) => break,
        }
    }
    drop(resp);
    let body = String::from_utf8_lossy(&buf[..buf.len().min(MAX_HTML_BYTES)]).into_owned();
    Ok(DirectResponse {
        status,
        final_url,
        is_html,
        body,
    })
}

/// Cap on concurrent proxy runs — a chat full of internal links
/// shouldn't fork a dozen interpreters at once.
static PROXY_SLOTS: LazyLock<tokio::sync::Semaphore> =
    LazyLock::new(|| tokio::sync::Semaphore::new(2));

/// Cap on how much proxy stdout is handed to the HTML extractors.
const MAX_PROXY_HTML_BYTES: usize = 256 * 1024;

/// Run one proxy request on the blocking pool while holding a slot.
pub(crate) async fn run_proxy_limited(
    profile: &InternalSiteProfile,
    url: &str,
    response: ProxyResponse,
) -> internal_sites::ProxyRun {
    let _permit = PROXY_SLOTS.acquire().await;
    let profile = profile.clone();
    let url = url.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        internal_sites::run_proxy(&profile, &url, response)
    })
    .await
    .unwrap_or_else(|e| internal_sites::ProxyRun {
        ok: false,
        exit_code: None,
        timed_out: false,
        stdout: String::new(),
        stderr: format!("proxy task failed: {}", e),
    })
}

/// Head window of proxy HTML output: through `</head>`, capped, cut on
/// a char boundary.
fn proxy_head_window(stdout: &str) -> &str {
    let mut end = stdout
        .as_bytes()
        .windows(HEAD_CLOSE.len())
        .position(|w| w.eq_ignore_ascii_case(HEAD_CLOSE))
        .map(|i| i + HEAD_CLOSE.len())
        .unwrap_or(stdout.len())
        .min(MAX_PROXY_HTML_BYTES);
    while !stdout.is_char_boundary(end) {
        end -= 1;
    }
    &stdout[..end]
}

/// Link metadata via an internal-site profile's proxy script. The
/// favicon is fetched through the proxy too and inlined as a data URI
/// (the webview can't authenticate to the host itself); the hero image
/// is dropped for the same reason.
///
/// Logging here records the profile name and exit status only — never
/// the URL, stdout or stderr, which can all carry internal details.
async fn proxy_link_metadata(
    profile: &InternalSiteProfile,
    url: &str,
) -> Result<serde_json::Value, AppError> {
    let run = run_proxy_limited(profile, url, ProxyResponse::Text).await;
    metadata_from_proxy_run(profile, url, &run).await
}

/// Second half of [`proxy_link_metadata`], split out so the settings Test
/// button can show the raw run and the resulting card from one execution.
pub(crate) async fn metadata_from_proxy_run(
    profile: &InternalSiteProfile,
    url: &str,
    run: &internal_sites::ProxyRun,
) -> Result<serde_json::Value, AppError> {
    metadata_from_proxy_run_traced(profile, url, run)
        .await
        .map(|(meta, _)| meta)
}

/// Most favicon candidates tried through the proxy per page.
const MAX_ICON_CANDIDATES: usize = 4;

/// [`metadata_from_proxy_run`], also returning each favicon candidate it
/// tried and why it failed (for the settings Test).
pub(crate) async fn metadata_from_proxy_run_traced(
    profile: &InternalSiteProfile,
    url: &str,
    run: &internal_sites::ProxyRun,
) -> Result<(serde_json::Value, Vec<internal_sites::IconAttempt>), AppError> {
    if !run.ok {
        log::warn!(
            "Internal-site proxy '{}' failed (exit {:?}, timed out: {})",
            profile.name,
            run.exit_code,
            run.timed_out
        );
        return Err(AppError::from("Internal-site proxy failed"));
    }

    let (title, description, mut candidates) = match internal_sites::parse_json_output(&run.stdout)
    {
        Some(md) => {
            let mut c: Vec<String> = md.favicon.into_iter().collect();
            c.extend(icon_candidates("", url));
            (md.title, md.description, c)
        }
        None => {
            let head = proxy_head_window(&run.stdout);
            let meta = extract_page_meta(head, url);
            (meta.title, meta.description, icon_candidates(head, url))
        }
    };
    dedup_in_order(&mut candidates);

    let mut attempts = Vec::new();
    let mut favicon = None;
    for candidate in candidates.into_iter().take(MAX_ICON_CANDIDATES) {
        if candidate.starts_with("data:") {
            favicon = internal_sites::sanitize_data_icon(&candidate);
            if favicon.is_some() {
                break;
            }
            continue;
        }
        if !candidate.starts_with("http://") && !candidate.starts_with("https://") {
            continue;
        }
        let icon = run_proxy_limited(profile, &candidate, ProxyResponse::Base64).await;
        let decoded = icon.ok.then(|| internal_sites::decode_icon(&icon.stdout));
        attempts.push(internal_sites::IconAttempt {
            url: candidate,
            ok: matches!(decoded, Some(Ok(_))),
            exit_code: icon.exit_code,
            timed_out: icon.timed_out,
            rejection: decoded.as_ref().and_then(|d| d.as_ref().err().copied()),
        });
        if let Some(Ok(uri)) = decoded {
            favicon = Some(uri);
            break;
        }
    }

    Ok((
        serde_json::json!({
            "url": url,
            "title": title,
            "description": description,
            "image": null,
            "favicon": favicon,
        }),
        attempts,
    ))
}

fn dedup_in_order(items: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    items.retain(|i| seen.insert(i.clone()));
}

/// Wipe the on-disk link-metadata cache. Surfaced from the Link
/// Preview settings page so a user can force a refresh after a
/// publisher changes their OG tags or after a transient outage.
#[tauri::command]
pub async fn link_metadata_clear_cache() -> Result<(), AppError> {
    tauri::async_runtime::spawn_blocking(crate::link_metadata_cache::clear)
        .await
        .map_err(|e| format!("Cache clear task failed: {}", e))?
        .map_err(|e| format!("Failed to clear link metadata cache: {}", e))?;
    Ok(())
}

/// Counts + on-disk size of the cache, for the Settings UI.
#[tauri::command]
pub async fn link_metadata_cache_stats() -> Result<crate::link_metadata_cache::CacheStats, AppError>
{
    tauri::async_runtime::spawn_blocking(crate::link_metadata_cache::stats)
        .await
        .map_err(|e| AppError::from(format!("Stats task failed: {}", e)))
}

/// Extract content from <meta property="X" content="..."> or <meta name="X" content="...">
///
/// Every helper here searches a lowercased copy and slices the original
/// with the same offsets, so it must be `to_ascii_lowercase`: Unicode
/// lowercasing changes byte lengths ('İ' grows, Kelvin 'K' shrinks) and
/// would shift offsets onto the wrong text or mid-codepoint.
fn extract_meta(html: &str, name: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    // Try property= first (Open Graph), then name= (standard meta)
    for attr in &["property", "name"] {
        let needle = format!("{}=\"{}\"", attr, name);
        if let Some(pos) = lower.find(&needle) {
            // Find content= in the same <meta> tag
            let tag_start = lower[..pos].rfind('<').unwrap_or(0);
            let tag_end = lower[pos..]
                .find('>')
                .map(|i| pos + i)
                .unwrap_or(lower.len());
            let tag = &html[tag_start..tag_end];
            if let Some(content) = extract_attr(tag, "content") {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

/// The page `<title>`, for the settings probe.
pub(crate) fn page_title(html: &str) -> Option<String> {
    extract_tag_content(html, "title")
}

/// Extract text content from <tag>...</tag>
fn extract_tag_content(html: &str, tag: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);
    if let Some(start) = lower.find(&open) {
        if let Some(gt) = lower[start..].find('>') {
            let content_start = start + gt + 1;
            if let Some(end) = lower[content_start..].find(&close) {
                let text = html[content_start..content_start + end].trim();
                if !text.is_empty() {
                    return Some(html_decode(text));
                }
            }
        }
    }
    None
}

/// The page's favicon: its first declared icon, else `/favicon.ico`.
fn extract_link_icon(html: &str, base_url: &str) -> Option<String> {
    icon_candidates(html, base_url).into_iter().next()
}

/// Favicon URLs to try, best first: `<link>` tags whose `rel` has the
/// `icon` token (`icon`, `shortcut icon`, …) in document order, then
/// `apple-touch-icon` ones, then `/favicon.ico`. `mask-icon` (a
/// monochrome Safari pin glyph) is skipped.
fn icon_candidates(html: &str, base_url: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut icons = Vec::new();
    let mut touch = Vec::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<link") {
        let start = from + i;
        let end = lower[start..]
            .find('>')
            .map(|j| start + j)
            .unwrap_or(lower.len());
        let tag = &html[start..end];
        from = end;
        let (Some(rel), Some(href)) = (extract_attr(tag, "rel"), extract_attr(tag, "href")) else {
            continue;
        };
        let rel = rel.to_ascii_lowercase();
        let href = href.trim();
        if href.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = rel.split_whitespace().collect();
        if tokens.contains(&"icon") {
            icons.push(resolve_url(href, base_url));
        } else if tokens
            .iter()
            .any(|t| *t == "apple-touch-icon" || *t == "apple-touch-icon-precomposed")
        {
            touch.push(resolve_url(href, base_url));
        }
    }
    icons.extend(touch);
    if let Ok(parsed) = url::Url::parse(base_url) {
        icons.push(format!(
            "{}://{}/favicon.ico",
            parsed.scheme(),
            parsed.host_str().unwrap_or("")
        ));
    }
    dedup_in_order(&mut icons);
    icons
}

/// Extract an attribute value from an HTML tag string
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{}=", attr);
    let mut from = 0;
    while let Some(i) = lower[from..].find(&needle) {
        let pos = from + i;
        from = pos + needle.len();
        // Whole attribute name only: `rel=` must not match inside `data-rel=`.
        let prev = lower[..pos].chars().next_back();
        if prev.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            continue;
        }
        let after = tag[pos + needle.len()..].trim_start();
        if let Some(content) = after.strip_prefix('"') {
            return content.find('"').map(|end| content[..end].to_string());
        } else if let Some(content) = after.strip_prefix('\'') {
            return content.find('\'').map(|end| content[..end].to_string());
        }
        // Unquoted: up to whitespace or the end of the tag.
        let end = after
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(after.len());
        let value = after[..end].trim_end_matches('/');
        return (!value.is_empty()).then(|| value.to_string());
    }
    None
}

/// Resolve a potentially relative URL against a base URL
fn resolve_url(href: &str, base: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("data:") {
        return href.to_string();
    }
    if href.starts_with("//") {
        return format!("https:{}", href);
    }
    if let Ok(base_url) = url::Url::parse(base) {
        if let Ok(resolved) = base_url.join(href) {
            return resolved.to_string();
        }
    }
    href.to_string()
}

/// Basic HTML entity decoding
fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_offsets_survive_length_changing_lowercase_chars() {
        // 'İ' lowercases to 3 bytes and Kelvin 'K' to 1, so Unicode
        // lowercasing would shift every later offset.
        let html = "<title>İstanbul \u{212A}elvin</title>\
                    <meta property=\"og:title\" content=\"İzmir news\">\
                    <meta name=\"description\" content=\"\u{212A}\u{212A} \u{2126} ok\">";
        assert_eq!(
            extract_meta(html, "og:title").as_deref(),
            Some("İzmir news")
        );
        assert_eq!(
            extract_meta(html, "description").as_deref(),
            Some("\u{212A}\u{212A} \u{2126} ok")
        );
        assert_eq!(
            extract_tag_content(html, "title").as_deref(),
            Some("İstanbul \u{212A}elvin")
        );
    }

    #[test]
    fn link_icon_offsets_survive_non_ascii_prefix() {
        let html = "<title>\u{212A}\u{212A}\u{212A}</title><link rel=\"icon\" href=\"/i.png\">";
        assert_eq!(
            extract_link_icon(html, "https://example.com/page").as_deref(),
            Some("https://example.com/i.png")
        );
    }

    #[test]
    fn proxy_head_window_stops_after_head_on_a_char_boundary() {
        assert_eq!(
            proxy_head_window("<head><title>x</title></HEAD><body>secret"),
            "<head><title>x</title></HEAD>"
        );
        let long = "é".repeat(MAX_PROXY_HTML_BYTES);
        let w = proxy_head_window(&long);
        assert!(w.len() <= MAX_PROXY_HTML_BYTES && !w.is_empty());
    }

    #[test]
    fn page_meta_falls_back_to_title_and_favicon_ico() {
        let meta = extract_page_meta("<title>Doc</title>", "https://wiki.example.test/a/b");
        assert_eq!(meta.title.as_deref(), Some("Doc"));
        assert_eq!(
            meta.favicon.as_deref(),
            Some("https://wiki.example.test/favicon.ico")
        );
        assert!(meta.image.is_none());
    }

    #[test]
    fn icon_candidates_cover_rel_variants_in_priority_order() {
        let html = r#"<head>
            <link rel="mask-icon" href="/mask.svg">
            <link href="/touch.png" rel="apple-touch-icon">
            <link rel='shortcut icon' href='/a.ico'>
            <link rel=icon href=/b.svg type=image/svg+xml>
            <link data-rel="icon" rel="stylesheet" href="/s.css">
            <link rel="icon" href="/a.ico">
        </head>"#;
        assert_eq!(
            icon_candidates(html, "https://wiki.example.test/x/y"),
            vec![
                "https://wiki.example.test/a.ico",
                "https://wiki.example.test/b.svg",
                "https://wiki.example.test/touch.png",
                "https://wiki.example.test/favicon.ico",
            ]
        );
        assert_eq!(
            icon_candidates("", "https://wiki.example.test/p"),
            vec!["https://wiki.example.test/favicon.ico"]
        );
    }

    #[test]
    fn head_close_is_found_case_insensitively() {
        assert!(contains_head_close(b"<html><HEAD></Head><body>"));
        assert!(!contains_head_close(b"<html><head><title>x</title>"));
    }
}
