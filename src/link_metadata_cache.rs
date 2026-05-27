//! Persistent disk cache for link-preview metadata.
//!
//! `fetch_link_metadata` consults this cache before hitting the
//! network. A warm hit returns instantly; a miss does the live fetch
//! and persists the result. The cache survives app restarts so a user
//! reopening yesterday's chat doesn't re-fetch every URL.
//!
//! ## Why per-URL TTLs
//!
//! Live fetches return one of three things:
//!
//!   - Full metadata (title + description + image + favicon). Stable
//!     for the lifetime of the page; we keep these for 7 days.
//!   - Partial metadata (e.g. only a title, no OG image). Same TTL —
//!     publishers fix images later, so a long cache is fine; users
//!     can always force a refresh by clearing the cache.
//!   - `null` — fetch failed or the URL didn't return HTML. Could be
//!     transient (rate limit, brief downtime). Cached for 1 hour
//!     instead of 7 days so we don't carry a transient failure for a
//!     week.
//!
//! ## Capacity
//!
//! Plain LRU on a `BTreeMap` ordered by `fetched_at`. We don't try to
//! be clever — chats accumulate maybe dozens of unique URLs, and we
//! evict the oldest when we cross a hard cap. A capacity larger than
//! the typical user's URL count means the LRU is effectively a "trim
//! the unused" rather than "fight for slots."
//!
//! ## File layout (JSON)
//!
//! ```json
//! { "version": 1, "entries": { "https://…": { "meta": {...}, "fetched_at": "2026-…" } } }
//! ```
//!
//! Sentinel `meta = null` is preserved so the negative-cache TTL kicks
//! in correctly. Older entries that don't carry `fetched_at` are
//! treated as expired on read so we never serve stale-of-unknown-age.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const CACHE_FILE: &str = "link-metadata.json";
const FORMAT_VERSION: u32 = 1;

/// 7 days for successful fetches. Publishers update OG metadata
/// rarely; a week is the right balance between stale risk and avoided
/// network hits.
const FRESH_TTL_SECS: i64 = 7 * 24 * 60 * 60;
/// 1 hour for negative results (`null` meta). Short enough to recover
/// from transient outages, long enough to absorb burst re-formats
/// during a chat.
const NEGATIVE_TTL_SECS: i64 = 60 * 60;
/// Hard cap on entries. Past this, the oldest get evicted. 500 covers
/// a heavy power user; the file stays well under 1 MB at this size.
const MAX_ENTRIES: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    /// `None` means "we tried and got nothing" — the negative-cache
    /// signal. Distinguished from "missing entry" (which means
    /// "never tried").
    #[serde(default)]
    meta: Option<serde_json::Value>,
    /// RFC 3339 timestamp. Used for TTL checks + LRU eviction order.
    /// Missing on legacy entries — those are treated as expired.
    #[serde(default)]
    fetched_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CacheFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: BTreeMap<String, CacheEntry>,
}

fn cache_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("kage").join(CACHE_FILE))
}

fn load_file() -> CacheFile {
    let Some(path) = cache_path() else {
        return CacheFile::default();
    };
    let body = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return CacheFile::default(),
    };
    serde_json::from_str(&body).unwrap_or_default()
}

fn save_file(file: &CacheFile) -> Result<()> {
    let path = cache_path().context("config dir unavailable")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("failed to create kage config dir")?;
    }
    let body = serde_json::to_string(file).context("serialize link metadata cache")?;
    // Best-effort atomic-ish write: write to a temp sibling then rename.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).context("write temp link metadata cache")?;
    std::fs::rename(&tmp, &path).context("rename temp link metadata cache")?;
    Ok(())
}

/// In-memory cache. Lazy-loaded from disk on first access; mutations
/// route through `store_in_memory` and are flushed asynchronously by a
/// debounced background thread.
///
/// The previous design re-read and re-parsed the entire JSON file on
/// every lookup AND every store, which was meaningful overhead for
/// chats with dozens of fetched links — each `fetch_link_metadata` call
/// was paying the deserialise cost twice (once on the lookup probe,
/// once before the store). The in-memory copy collapses both paths to
/// a `BTreeMap::get` / `BTreeMap::insert` under a brief lock.
struct InMemoryCache {
    file: CacheFile,
    /// True after the first successful disk load. Initialized lazily
    /// rather than at module init so the file is only opened when a
    /// link-preview is actually requested.
    loaded: bool,
    /// Set when the in-memory state has changes the flush thread
    /// hasn't written yet. Drives the debounced-flush loop.
    dirty: bool,
    /// Wallclock of the last mutation. The flush thread waits 500ms
    /// past this before writing — coalesces the bursts that happen
    /// when a chat lands with many links at once.
    last_dirty_at: Option<Instant>,
}

impl InMemoryCache {
    fn lazy_load(&mut self) {
        if !self.loaded {
            self.file = load_file();
            self.loaded = true;
        }
    }
}

static CACHE: LazyLock<Mutex<InMemoryCache>> = LazyLock::new(|| {
    let cache = Mutex::new(InMemoryCache {
        file: CacheFile::default(),
        loaded: false,
        dirty: false,
        last_dirty_at: None,
    });
    spawn_flush_thread();
    cache
});

/// Debounce window. Bursts of `store` calls within this many ms after
/// the last mutation are coalesced into a single disk write.
const FLUSH_DEBOUNCE: Duration = Duration::from_millis(500);

fn spawn_flush_thread() {
    std::thread::Builder::new()
        .name("link-metadata-cache-flush".to_string())
        .spawn(|| loop {
            std::thread::sleep(Duration::from_millis(250));
            let snapshot: Option<CacheFile> = {
                let mut cache = match CACHE.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                let due = cache
                    .last_dirty_at
                    .is_some_and(|t| t.elapsed() >= FLUSH_DEBOUNCE);
                if cache.dirty && due {
                    cache.dirty = false;
                    cache.last_dirty_at = None;
                    Some(cache.file.clone())
                } else {
                    None
                }
            };
            if let Some(file) = snapshot {
                if let Err(e) = save_file(&file) {
                    log::warn!("link_metadata_cache: flush failed: {}", e);
                }
            }
        })
        .expect("spawn link-metadata-cache flush thread");
}

fn with_cache<R>(f: impl FnOnce(&mut InMemoryCache) -> R) -> R {
    let mut cache = match CACHE.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    cache.lazy_load();
    f(&mut cache)
}

/// True when an entry is still within its TTL relative to `now`. Pure
/// so tests can pin the clock.
pub fn is_fresh(
    entry_meta: &Option<serde_json::Value>,
    fetched_at: Option<&str>,
    now_secs: i64,
) -> bool {
    let Some(ts) = fetched_at else {
        return false; // missing timestamp: treat as expired
    };
    let parsed = match chrono::DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => dt.timestamp(),
        Err(_) => return false,
    };
    let age = now_secs.saturating_sub(parsed);
    let ttl = if entry_meta.is_some() {
        FRESH_TTL_SECS
    } else {
        NEGATIVE_TTL_SECS
    };
    age >= 0 && age < ttl
}

/// Try the cache. Returns `Some(meta)` on a fresh hit (which may be
/// `Some(Some(json))` for a real hit or `Some(None)` for a fresh
/// negative entry — the caller should treat both as "don't fetch").
/// Returns `None` if the cache had nothing for this URL or the entry
/// expired.
pub fn lookup(url: &str) -> Option<Option<serde_json::Value>> {
    let now = chrono::Utc::now().timestamp();
    with_cache(|c| {
        let entry = c.file.entries.get(url)?;
        if is_fresh(&entry.meta, entry.fetched_at.as_deref(), now) {
            Some(entry.meta.clone())
        } else {
            None
        }
    })
}

/// Persist the result of a live fetch. Caller passes through whatever
/// `fetch_link_metadata` produced — including `None` for failed
/// fetches; the negative-cache TTL handles transient errors.
///
/// The Result return is kept for API stability but the in-memory write
/// can't fail; the actual disk flush happens in the background. Errors
/// from the flush are logged, not surfaced to callers — by the time a
/// flush fails, the original `store` caller has long since moved on.
pub fn store(url: &str, meta: Option<serde_json::Value>) -> Result<()> {
    let now_str = chrono::Utc::now().to_rfc3339();
    with_cache(|c| {
        c.file.version = FORMAT_VERSION;
        c.file.entries.insert(
            url.to_string(),
            CacheEntry {
                meta,
                fetched_at: Some(now_str),
            },
        );
        evict_to_capacity(&mut c.file);
        c.dirty = true;
        c.last_dirty_at = Some(Instant::now());
    });
    Ok(())
}

/// Evict the oldest entries (by `fetched_at`) until we're under the
/// hard cap. Module-private; the live path calls it from `store` and
/// the unit tests reach in directly.
fn evict_to_capacity(file: &mut CacheFile) {
    if file.entries.len() <= MAX_ENTRIES {
        return;
    }
    // Collect (fetched_at_or_zero, url) and sort ascending — oldest
    // first. Missing fetched_at is treated as zero so legacy entries
    // get evicted before timestamped ones, which is the right
    // policy: those entries also fail `is_fresh`.
    let mut pairs: Vec<(i64, String)> = file
        .entries
        .iter()
        .map(|(k, v)| {
            let ts = v
                .fetched_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp())
                .unwrap_or(0);
            (ts, k.clone())
        })
        .collect();
    pairs.sort_by_key(|(ts, _)| *ts);
    let to_evict = file.entries.len() - MAX_ENTRIES;
    for (_ts, url) in pairs.into_iter().take(to_evict) {
        file.entries.remove(&url);
    }
}

/// Wipe every entry. Surfaced via the `link_metadata_clear_cache`
/// Tauri command for the Settings → Link Preview reset button. Flushes
/// synchronously so the user sees the cache size drop immediately when
/// the settings page re-fetches stats.
pub fn clear() -> Result<()> {
    let snapshot = with_cache(|c| {
        c.file = CacheFile {
            version: FORMAT_VERSION,
            entries: BTreeMap::new(),
        };
        c.dirty = false;
        c.last_dirty_at = None;
        c.file.clone()
    });
    save_file(&snapshot)
}

/// Total entries on disk + the file size in bytes. Returned by the
/// `link_metadata_cache_stats` command so the UI can show the user
/// "1.4 MB across 312 URLs" before they hit Clear.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: u64,
}

pub fn stats() -> CacheStats {
    let entries = with_cache(|c| c.file.entries.len());
    let bytes = cache_path()
        .and_then(|p| std::fs::metadata(&p).ok())
        .map(|m| m.len())
        .unwrap_or(0);
    CacheStats { entries, bytes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(secs_ago: i64) -> String {
        let now = chrono::Utc::now().timestamp();
        chrono::DateTime::from_timestamp(now - secs_ago, 0)
            .unwrap()
            .to_rfc3339()
    }

    #[test]
    fn fresh_entry_with_meta_inside_7_days() {
        let now = chrono::Utc::now().timestamp();
        let meta = Some(serde_json::json!({"title": "x"}));
        let stamp = ts(60); // 1 min old
        assert!(is_fresh(&meta, Some(&stamp), now));
    }

    #[test]
    fn fresh_entry_with_meta_just_inside_ttl() {
        let now = chrono::Utc::now().timestamp();
        let meta = Some(serde_json::json!({"title": "x"}));
        let stamp = ts(FRESH_TTL_SECS - 60); // a minute under the limit
        assert!(is_fresh(&meta, Some(&stamp), now));
    }

    #[test]
    fn stale_entry_with_meta_past_7_days() {
        let now = chrono::Utc::now().timestamp();
        let meta = Some(serde_json::json!({"title": "x"}));
        let stamp = ts(FRESH_TTL_SECS + 60);
        assert!(!is_fresh(&meta, Some(&stamp), now));
    }

    #[test]
    fn negative_entry_uses_short_ttl() {
        let now = chrono::Utc::now().timestamp();
        // 30 minutes ago — well under the 1-hour negative TTL.
        assert!(is_fresh(&None, Some(&ts(30 * 60)), now));
        // 90 minutes ago — over the 1-hour negative TTL. A negative
        // entry that would still be fresh under the 7-day rule for
        // success entries must be expired here.
        assert!(!is_fresh(&None, Some(&ts(90 * 60)), now));
    }

    #[test]
    fn missing_timestamp_is_always_stale() {
        let now = chrono::Utc::now().timestamp();
        assert!(!is_fresh(&Some(serde_json::json!({})), None, now));
        assert!(!is_fresh(&None, None, now));
    }

    #[test]
    fn unparseable_timestamp_is_always_stale() {
        let now = chrono::Utc::now().timestamp();
        assert!(!is_fresh(
            &Some(serde_json::json!({})),
            Some("not-a-date"),
            now
        ));
    }

    #[test]
    fn evict_drops_oldest_first() {
        let mut file = CacheFile {
            version: 1,
            entries: BTreeMap::new(),
        };
        // Build 3 over the cap with spread-out timestamps.
        let cap = MAX_ENTRIES;
        for i in 0..(cap + 3) {
            // Older URLs get older timestamps, so they should be the
            // first to go. ts(secs_ago) — bigger i = older.
            let stamp = ts((cap + 3 - i) as i64 * 10);
            file.entries.insert(
                format!("https://example.com/{}", i),
                CacheEntry {
                    meta: Some(serde_json::json!({"i": i})),
                    fetched_at: Some(stamp),
                },
            );
        }
        evict_to_capacity(&mut file);
        assert_eq!(file.entries.len(), cap);
        // The oldest 3 should be gone — those were i=0..3 (because
        // they got the largest secs_ago).
        for i in 0..3 {
            assert!(!file
                .entries
                .contains_key(&format!("https://example.com/{}", i)));
        }
    }

    #[test]
    fn evict_skips_when_under_capacity() {
        let mut file = CacheFile::default();
        for i in 0..10 {
            file.entries.insert(
                format!("u{}", i),
                CacheEntry {
                    meta: None,
                    fetched_at: Some(ts(0)),
                },
            );
        }
        evict_to_capacity(&mut file);
        assert_eq!(file.entries.len(), 10);
    }

    #[test]
    fn evict_treats_missing_timestamp_as_oldest() {
        let mut file = CacheFile::default();
        // Fill to cap with timestamped entries.
        for i in 0..MAX_ENTRIES {
            file.entries.insert(
                format!("u{}", i),
                CacheEntry {
                    meta: None,
                    fetched_at: Some(ts(i as i64 * 10)),
                },
            );
        }
        // Add one legacy entry (no timestamp) and one fresh one. The
        // legacy entry should get dropped first.
        file.entries.insert(
            "legacy".to_string(),
            CacheEntry {
                meta: None,
                fetched_at: None,
            },
        );
        file.entries.insert(
            "fresh".to_string(),
            CacheEntry {
                meta: None,
                fetched_at: Some(ts(0)),
            },
        );
        evict_to_capacity(&mut file);
        assert!(!file.entries.contains_key("legacy"));
        assert!(file.entries.contains_key("fresh"));
    }
}
