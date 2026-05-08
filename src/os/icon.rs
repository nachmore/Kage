// Cross-platform application icon extraction with bounded LRU caching.
//
// Icon extraction (Win32 SHGetFileInfo, NSWorkspace, GdkPixbuf, etc.) is
// expensive: tens of milliseconds per call on a cold path, plus AV
// scanning on Windows. The same exe path always yields the same icon, so
// we cache the result keyed by exe path. A second cache keys by process
// name (e.g. "winword", "chrome") for the activity-tracker's quick
// lookup path which doesn't have the full path on hand.
//
// The pre-2026-05 implementation lived inside the Windows window-enum
// impl as a HashMap that "evicted" by clearing the entire 512-entry
// table when full — pessimal: crossing the cap dumped 512 cached
// extractions on the floor and re-paid for them all on the next
// list_windows call. LRU evicts one entry to add one, keeping the
// working set hot.
//
// The cache lives at the cross-platform layer so future macOS/Linux
// icon support gets the same caching behaviour for free; today the
// non-Windows extract impls return None and the cache simply stays
// empty there.

use crate::lock_ext::LockExt;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::{LazyLock, Mutex};

/// Per-cache capacity. Each entry is a string key plus a base64 PNG
/// (~few KB on average); 512 entries caps each cache at a few MB.
const ICON_CACHE_MAX: usize = 512;

fn new_cache<K: std::hash::Hash + Eq, V>() -> LruCache<K, V> {
    LruCache::new(NonZeroUsize::new(ICON_CACHE_MAX).expect("ICON_CACHE_MAX must be > 0"))
}

/// exe path → extracted icon (Some(base64) on success, None when the
/// platform impl returned no icon — also cached, so we don't re-attempt
/// the expensive extraction every time).
#[allow(dead_code)] // consumed only by src/os/windows/window_list.rs today; macOS/Linux wiring pending
static ICON_BY_PATH: LazyLock<Mutex<LruCache<String, Option<String>>>> =
    LazyLock::new(|| Mutex::new(new_cache()));

/// process name (lowercased) → icon base64. Populated during window
/// enumeration via `register_process_name_icon`; consumed by
/// `get_icon_by_process_name`.
static ICON_BY_NAME: LazyLock<Mutex<LruCache<String, String>>> =
    LazyLock::new(|| Mutex::new(new_cache()));

/// Extract an icon for the given exe path, caching the result.
/// Returns the base64 data URI or None if extraction is unsupported or
/// failed. A None result is also cached — callers shouldn't re-attempt
/// extraction on every call for an exe that legitimately has no icon.
#[allow(dead_code)] // consumed only by src/os/windows/window_list.rs today
pub fn extract_icon_base64_cached(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let mut cache = ICON_BY_PATH.lock_or_recover();
    cache
        .get_or_insert(path.to_string(), || {
            crate::os::platform::icon::extract_icon_base64_impl(path)
        })
        .clone()
}

/// Extract an icon without consulting the cache. Callers that maintain
/// their own per-feature storage (e.g. `app_launcher`'s registry, which
/// scans once per startup and stores the icon directly on the
/// `Application` struct) should use this so we don't double-store.
pub fn extract_icon_base64(path: &str) -> Option<String> {
    crate::os::platform::icon::extract_icon_base64_impl(path)
}

/// Record a (process_name, icon) pair for later quick lookup. Called
/// during window enumeration once we've identified both the process
/// name and (via the path cache) its icon.
#[allow(dead_code)] // consumed only by src/os/windows/window_list.rs today
pub fn register_process_name_icon(process_name: &str, icon: &str) {
    if process_name.is_empty() || icon.is_empty() {
        return;
    }
    let key = process_name.to_lowercase();
    let mut cache = ICON_BY_NAME.lock_or_recover();
    if cache.get(&key).is_none() {
        cache.put(key, icon.to_string());
    }
}

/// Look up an icon by lower-cased process name (e.g. "winword").
/// Returns None if not registered yet — the caller is responsible for
/// triggering a `list_windows()` if that's the right priming step.
/// Bumps the entry to MRU on hit.
pub fn get_icon_by_process_name(process_name: &str) -> Option<String> {
    let key = process_name.to_lowercase();
    let mut cache = ICON_BY_NAME.lock_or_recover();
    cache.get(&key).cloned()
}

/// True iff the by-name cache has no entries. Used by callers that may
/// want to prime via window enumeration on the very first lookup.
pub fn process_name_cache_is_empty() -> bool {
    ICON_BY_NAME.lock_or_recover().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fresh cache with a small capacity so eviction is easy to
    /// trigger. Mirrors the production `new_cache` call shape.
    fn small_cache(cap: usize) -> LruCache<String, String> {
        LruCache::new(NonZeroUsize::new(cap).unwrap())
    }

    #[test]
    fn lru_evicts_least_recently_used_not_the_whole_table() {
        // The pre-2026-05 implementation `.clear()`-ed the entire HashMap
        // when length crossed ICON_CACHE_MAX. With LRU, exceeding capacity
        // by one entry evicts exactly one entry — the least recently used.
        let mut cache = small_cache(3);
        cache.put("a".into(), "icon-a".into());
        cache.put("b".into(), "icon-b".into());
        cache.put("c".into(), "icon-c".into());
        cache.put("d".into(), "icon-d".into());

        assert_eq!(cache.len(), 3);
        assert!(
            cache.get(&"a".to_string()).is_none(),
            "a should have been evicted"
        );
        assert_eq!(cache.get(&"b".to_string()), Some(&"icon-b".to_string()));
        assert_eq!(cache.get(&"c".to_string()), Some(&"icon-c".to_string()));
        assert_eq!(cache.get(&"d".to_string()), Some(&"icon-d".to_string()));
    }

    #[test]
    fn lru_get_bumps_recency() {
        let mut cache = small_cache(3);
        cache.put("a".into(), "icon-a".into());
        cache.put("b".into(), "icon-b".into());
        cache.put("c".into(), "icon-c".into());
        // Touch "a" — now most recently used; "b" is the LRU.
        let _ = cache.get(&"a".to_string());
        cache.put("d".into(), "icon-d".into());

        assert!(
            cache.get(&"a".to_string()).is_some(),
            "a should survive — was just touched"
        );
        assert!(
            cache.get(&"b".to_string()).is_none(),
            "b should be evicted as the LRU"
        );
    }

    #[test]
    fn get_or_insert_extracts_at_most_once_per_key() {
        // Mirrors the list_windows_impl pattern: same exe path appearing
        // in N windows should run the expensive extraction closure once.
        let mut cache = small_cache(8);
        let mut extractions = 0;
        for _ in 0..5 {
            let _ = cache.get_or_insert("notepad.exe".to_string(), || {
                extractions += 1;
                "icon-notepad".to_string()
            });
        }
        assert_eq!(
            extractions, 1,
            "expected a single extraction across 5 lookups"
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn register_process_name_icon_ignores_empty_inputs() {
        // Defensive: don't poison the cache with empty keys/values.
        register_process_name_icon("", "icon-x");
        register_process_name_icon("notepad", "");
        // Cache is process-global so this assertion is conservative —
        // we only assert these specific entries weren't recorded.
        assert!(get_icon_by_process_name("").is_none());
    }
}
