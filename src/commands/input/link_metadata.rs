use crate::error::AppError;

/// Fetch metadata (title, description, image, favicon) from a URL for
/// link previews. Consults the on-disk cache first; on a fresh hit
/// returns the cached value (including the negative-cache `null`
/// shape) without going to the network. Misses do the live fetch and
/// persist before returning. See `src/link_metadata_cache.rs` for the
/// TTL + eviction policy.
#[tauri::command]
pub async fn fetch_link_metadata(url: String) -> Result<serde_json::Value, AppError> {
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
            None => Ok(serde_json::json!({
                "url": url,
                "title": null,
                "description": null,
                "image": null,
                "favicon": null,
            })),
        };
    }

    // Cache miss — go to the network. Capture the outcome so we
    // can persist it (negative or positive) before returning. The
    // helper is split out so both arms write through the same cache
    // path without having to thread an Option around.
    let fetched = fetch_link_metadata_uncached(&url).await;
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

/// Live-network half of `fetch_link_metadata`. Pulled out so the
/// command wrapper can sit a cache check + cache write around it
/// without nesting too deeply. Returns the same shape the command
/// returns.
async fn fetch_link_metadata_uncached(url: &str) -> Result<serde_json::Value, AppError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("Mozilla/5.0 (compatible; Kage/1.0)")
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Fetch error: {}", e))?;

    let final_url = resp.url().to_string();
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status).into());
    }

    // Only process HTML responses
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    if !content_type.contains("text/html") {
        return Ok(serde_json::json!({
            "url": final_url,
            "title": null,
            "description": null,
            "image": null,
            "favicon": null,
        }));
    }

    // Read only the first 32KB to extract meta tags (don't download entire pages)
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Read error: {}", e))?;
    let html = String::from_utf8_lossy(&bytes[..bytes.len().min(32768)]);

    let title = extract_meta(&html, "og:title")
        .or_else(|| extract_meta(&html, "twitter:title"))
        .or_else(|| extract_tag_content(&html, "title"));

    let description = extract_meta(&html, "og:description")
        .or_else(|| extract_meta(&html, "description"))
        .or_else(|| extract_meta(&html, "twitter:description"));

    // Hero image (Open Graph / Twitter Card) — separate from the
    // favicon since the formatter renders them in different places.
    // Folding both into one field (which previous versions did) meant
    // the card always showed a 28-px favicon even when the page had a
    // proper og:image; the result looked half-built next to peer link
    // previews in Slack / Discord / Notion.
    let image = extract_meta(&html, "og:image")
        .or_else(|| extract_meta(&html, "twitter:image"))
        .or_else(|| extract_meta(&html, "twitter:image:src"))
        .map(|raw| resolve_url(&raw, &final_url));

    // Favicon is a small site-identity glyph. We try the explicit
    // <link rel="icon"> first, then a /favicon.ico fallback. Never
    // reuse the hero image — they have different aspect ratios and
    // CSS treatment.
    let favicon = extract_link_icon(&html, &final_url);

    Ok(serde_json::json!({
        "url": final_url,
        "title": title,
        "description": description,
        "image": image,
        "favicon": favicon,
    }))
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
fn extract_meta(html: &str, name: &str) -> Option<String> {
    let lower = html.to_lowercase();
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

/// Extract text content from <tag>...</tag>
fn extract_tag_content(html: &str, tag: &str) -> Option<String> {
    let lower = html.to_lowercase();
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

/// Extract <link rel="icon" href="..."> or <link rel="shortcut icon" href="...">
fn extract_link_icon(html: &str, base_url: &str) -> Option<String> {
    let lower = html.to_lowercase();
    for pattern in &["rel=\"icon\"", "rel=\"shortcut icon\""] {
        if let Some(pos) = lower.find(pattern) {
            let tag_start = lower[..pos].rfind('<').unwrap_or(0);
            let tag_end = lower[pos..]
                .find('>')
                .map(|i| pos + i)
                .unwrap_or(lower.len());
            let tag = &html[tag_start..tag_end];
            if let Some(href) = extract_attr(tag, "href") {
                return Some(resolve_url(href.trim(), base_url));
            }
        }
    }
    // Fallback: /favicon.ico
    if let Ok(parsed) = url::Url::parse(base_url) {
        return Some(format!(
            "{}://{}/favicon.ico",
            parsed.scheme(),
            parsed.host_str().unwrap_or("")
        ));
    }
    None
}

/// Extract an attribute value from an HTML tag string
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let needle = format!("{}=", attr);
    if let Some(pos) = lower.find(&needle) {
        let after = &tag[pos + needle.len()..];
        let after = after.trim_start();
        if let Some(content) = after.strip_prefix('"') {
            if let Some(end) = content.find('"') {
                return Some(content[..end].to_string());
            }
        } else if let Some(content) = after.strip_prefix('\'') {
            if let Some(end) = content.find('\'') {
                return Some(content[..end].to_string());
            }
        }
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
