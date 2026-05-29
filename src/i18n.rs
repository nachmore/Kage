//! Localization (i18n) for Rust-side strings.
//!
//! # Architecture
//!
//! Strings shown to users are stored in `locales/<lang>/messages.json` (one file per
//! language). The English catalog is canonical and authored by hand; every other
//! language is a one-for-one mirror, with most entries machine-translated by
//! `scripts/translate.py` and flagged for review.
//!
//! Catalog entries look like:
//!
//! ```json
//! "errors.connection.lost": {
//!   "message": "Connection lost: {reason}",
//!   "description": "ACP transport dropped"
//! }
//! ```
//!
//! `{name}` placeholders are interpolated at lookup time. ICU plural forms are not
//! supported on the Rust side — Rust callers stick to simple substitution because
//! every plural-bearing string surfaces in the frontend, where the JS implementation
//! handles the full ICU MessageFormat subset we care about.
//!
//! # Where translation happens
//!
//! Logs are NEVER translated. They go through `log::*` in English, every time, so
//! that a developer reading `app.jsonl` from a non-English user sees the same
//! string they would see locally. The only translation point is at the boundary
//! where text is about to be displayed:
//!
//!   * `AppError` carries `(kind, key, params)` and gets its `message` field
//!     materialized in the active locale right before serialisation crosses the
//!     Tauri command boundary.
//!   * Tray menu construction calls `t!("tray.show", &[])` once per item.
//!   * Window titles, native dialog text, etc. translate at the call site.
//!
//! # Adding a new string
//!
//! 1. Add the key to `locales/en/messages.json` (canonical).
//! 2. Use `t!("the.key", "param", value)` from Rust or `t("the.key", { param: value })`
//!    from JS.
//! 3. Run `python scripts/translate.py` to fill in the other 30 languages. The
//!    drift-check CI gate will fail any PR that adds a key without translations.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

/// One entry in a `messages.json` catalog. We deserialise into this struct rather
/// than `serde_json::Value` so that catalogs with malformed entries fail fast at
/// startup instead of at lookup time.
#[derive(Debug, Clone, Deserialize)]
struct Entry {
    message: String,
    /// Translator-facing context. Only consumed by `scripts/translate.py`; the
    /// runtime ignores it. We still deserialise it to avoid a "panic on unknown
    /// fields" surprise if we ever switch to `deny_unknown_fields`.
    #[serde(default)]
    #[allow(dead_code)]
    description: String,
}

/// Top-level catalog metadata. Lives under the reserved `_meta` key.
#[derive(Debug, Clone, Deserialize, Default)]
struct Meta {
    #[serde(default)]
    language: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    rtl: bool,
    /// Whether this catalog is mostly machine-translated. Surfaced in the
    /// settings UI as a "please report errors" banner; the runtime doesn't
    /// otherwise act on it.
    #[serde(default)]
    machine_translated: bool,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    pub language: String,
    pub display_name: String,
    pub rtl: bool,
    pub machine_translated: bool,
    entries: HashMap<String, Entry>,
}

impl Catalog {
    fn parse(raw: &str) -> Result<Self, String> {
        // Two-pass parse: first as a Map<String, Value> so we can pull `_meta`
        // out before the rest. Avoids defining a dual-shape struct that would
        // accept both Meta and Entry under the same field.
        let mut value: HashMap<String, serde_json::Value> =
            serde_json::from_str(raw).map_err(|e| format!("catalog json parse failed: {}", e))?;

        let meta: Meta = match value.remove("_meta") {
            Some(v) => {
                serde_json::from_value(v).map_err(|e| format!("_meta block invalid: {}", e))?
            }
            None => Meta::default(),
        };

        let mut entries = HashMap::with_capacity(value.len());
        for (k, v) in value {
            let entry: Entry = serde_json::from_value(v)
                .map_err(|e| format!("catalog entry {:?} invalid: {}", k, e))?;
            entries.insert(k, entry);
        }

        Ok(Catalog {
            language: meta.language,
            display_name: meta.name,
            rtl: meta.rtl,
            machine_translated: meta.machine_translated,
            entries,
        })
    }

    /// Look up a key. Returns `None` for missing keys; the caller picks the
    /// fallback strategy.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(|e| e.message.as_str())
    }

    /// Number of message keys, excluding `_meta`. Used by drift-check tests.
    #[allow(dead_code)]
    pub fn key_count(&self) -> usize {
        self.entries.len()
    }

    /// Iterate over message keys. Used by drift-check tests.
    #[allow(dead_code)]
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(|s| s.as_str())
    }
}

/// Embedded catalogs. The `include_str!` calls are resolved at compile time so
/// the binary ships with every locale baked in — no disk I/O at runtime, no
/// installer changes, no resource path drama.
///
/// To add a language: drop a new file under `locales/<code>/messages.json` and
/// add an `embed!` line below. The drift-check CI gate verifies that every
/// non-English catalog has the same key set as `en`, so missing translations
/// fail the build rather than silently fall back to English at runtime.
macro_rules! embed_locales {
    ($($code:literal),* $(,)?) => {
        const EMBEDDED: &[(&str, &str)] = &[
            $(
                ($code, include_str!(concat!("../locales/", $code, "/messages.json"))),
            )*
        ];
    };
}

embed_locales!(
    // Canonical English. MUST come first — fallback chain depends on it.
    "en",
);

/// All catalogs successfully loaded at startup, keyed by language code.
/// `OnceLock` so we pay the parse cost exactly once per process. A failed
/// parse is treated as a startup error in `init()`.
static CATALOGS: OnceLock<HashMap<String, Catalog>> = OnceLock::new();

/// The user's currently active language code (e.g. "en", "ja", "ar").
/// Set by `set_language()`; defaults to "en" until `init()` runs.
static ACTIVE: OnceLock<std::sync::RwLock<String>> = OnceLock::new();

fn active_lock() -> &'static std::sync::RwLock<String> {
    ACTIVE.get_or_init(|| std::sync::RwLock::new("en".to_string()))
}

/// Load every embedded catalog and pick the active language. Should be called
/// exactly once during `main()` startup, before any code touches `t!`.
///
/// Returns the resolved active language code so callers can log it.
pub fn init(preferred: Option<&str>) -> String {
    let mut map: HashMap<String, Catalog> = HashMap::new();
    for (code, raw) in EMBEDDED {
        match Catalog::parse(raw) {
            Ok(cat) => {
                // Use the embed key as the canonical code rather than trusting
                // the catalog's own _meta.language — that way a copy/paste
                // mistake in the json doesn't silently route lookups for
                // "ja" to the "ko" file.
                let mut cat = cat;
                if cat.language.is_empty() {
                    cat.language = (*code).to_string();
                }
                map.insert((*code).to_string(), cat);
            }
            Err(e) => {
                // Catalog parse failures are programmer errors, not user
                // errors — they mean a hand-edited or build-corrupted JSON
                // file shipped. Crash loudly during dev, but degrade
                // gracefully in release: fall through to English.
                debug_assert!(false, "i18n catalog {} failed to parse: {}", code, e);
                log::error!("i18n: catalog {} failed to parse: {}", code, e);
            }
        }
    }
    let _ = CATALOGS.set(map);

    let resolved = resolve_language(preferred);
    *active_lock().write().unwrap() = resolved.clone();
    resolved
}

/// Pick the best available language given a user preference. Falls back through
/// region-stripped variants ("zh-CN" → "zh") and finally to "en".
fn resolve_language(preferred: Option<&str>) -> String {
    let catalogs = match CATALOGS.get() {
        Some(c) => c,
        None => return "en".to_string(),
    };
    if let Some(p) = preferred {
        if catalogs.contains_key(p) {
            return p.to_string();
        }
        if let Some((stem, _)) = p.split_once('-') {
            if catalogs.contains_key(stem) {
                return stem.to_string();
            }
        }
    }
    "en".to_string()
}

/// Replace the active language. Called when the user changes the setting or
/// when config reload picks up an externally-edited config.json.
pub fn set_language(lang: &str) {
    let resolved = resolve_language(Some(lang));
    *active_lock().write().unwrap() = resolved;
}

/// The currently active language code.
pub fn active_language() -> String {
    active_lock().read().unwrap().clone()
}

/// `true` if the active language is right-to-left.
pub fn active_is_rtl() -> bool {
    let lang = active_language();
    CATALOGS
        .get()
        .and_then(|m| m.get(&lang))
        .map(|c| c.rtl)
        .unwrap_or(false)
}

/// `true` if the active catalog is mostly machine-translated. Surfaced in the
/// settings UI as a banner.
pub fn active_is_machine_translated() -> bool {
    let lang = active_language();
    CATALOGS
        .get()
        .and_then(|m| m.get(&lang))
        .map(|c| c.machine_translated)
        .unwrap_or(false)
}

/// Snapshot of a single catalog entry suitable for shipping to the frontend.
/// Layered on top of the internal `Entry` so consumers don't get to peek at
/// the storage representation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EntrySnapshot {
    pub message: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// Serialise a catalog as a `{ key: { message, description } }` map for shipping
/// to the frontend. Keeps the on-the-wire shape identical to what's stored in
/// `messages.json` so the JS side has a single mental model.
pub fn serialise_catalog(code: &str) -> Option<HashMap<String, EntrySnapshot>> {
    let cat = CATALOGS.get()?.get(code)?;
    let mut out = HashMap::with_capacity(cat.entries.len());
    for (k, v) in &cat.entries {
        out.insert(
            k.clone(),
            EntrySnapshot {
                message: v.message.clone(),
                description: v.description.clone(),
            },
        );
    }
    Some(out)
}

/// Every loaded language as `(code, display_name, rtl, machine_translated)`. Used
/// by the settings UI to populate the language dropdown.
pub fn available_languages() -> Vec<(String, String, bool, bool)> {
    let mut out: Vec<(String, String, bool, bool)> = CATALOGS
        .get()
        .map(|m| {
            m.iter()
                .map(|(code, cat)| {
                    (
                        code.clone(),
                        if cat.display_name.is_empty() {
                            code.clone()
                        } else {
                            cat.display_name.clone()
                        },
                        cat.rtl,
                        cat.machine_translated,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Translate a key in the active language with the given `{name}` substitutions.
///
/// Falls back to English if the key is missing in the active catalog. Falls back
/// to the key itself if it's missing in English (a programmer error — drift-check
/// will fail the build, but at runtime we surface the key rather than a panic).
pub fn translate(key: &str, params: &[(&str, &str)]) -> String {
    translate_in(&active_language(), key, params)
}

/// Translate a key in a specific language. Used by `Display for AppError` to
/// keep log output stable in English regardless of the user's UI locale.
pub fn translate_in(lang: &str, key: &str, params: &[(&str, &str)]) -> String {
    let raw = CATALOGS
        .get()
        .and_then(|m| {
            m.get(lang)
                .and_then(|c| c.get(key))
                .or_else(|| m.get("en").and_then(|c| c.get(key)))
        })
        .unwrap_or(key);
    interpolate(raw, params)
}

/// Substitute `{name}` placeholders. Unknown placeholders are left literal so a
/// missing param shows as `{name}` rather than truncating the message — easier
/// to spot during dev.
fn interpolate(template: &str, params: &[(&str, &str)]) -> String {
    if params.is_empty() || !template.contains('{') {
        return template.to_string();
    }
    let mut out = String::with_capacity(template.len() + 16);
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '{' {
            out.push(ch);
            continue;
        }
        // Found a `{`. Consume until the matching `}` and look up the name.
        let mut name = String::new();
        let mut closed = false;
        for nc in chars.by_ref() {
            if nc == '}' {
                closed = true;
                break;
            }
            name.push(nc);
        }
        if !closed {
            // Malformed template — preserve as-is. Drift-check would catch
            // a mismatched brace in EN, so this only fires for hand-edited
            // catalogs in production.
            out.push('{');
            out.push_str(&name);
            continue;
        }
        match params.iter().find(|(k, _)| *k == name.as_str()) {
            Some((_, v)) => out.push_str(v),
            None => {
                out.push('{');
                out.push_str(&name);
                out.push('}');
            }
        }
    }
    out
}

/// Convenience macro: `t!("key.path", "name", value, "other", value)`.
///
/// Expands to `crate::i18n::translate("key.path", &[("name", value), ("other", value)])`.
/// Param values must be `&str` — call `.to_string()` or `&format!(...)` at the
/// call site if you need to format a number first. We deliberately don't
/// stringify Display-impls inside the macro because doing so silently allocates
/// for every call, including the no-substitution case.
#[macro_export]
macro_rules! t {
    ($key:expr) => {{
        $crate::i18n::translate($key, &[])
    }};
    ($key:expr, $($name:expr => $val:expr),+ $(,)?) => {{
        $crate::i18n::translate($key, &[$(($name, $val)),+])
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_catalog_loads() {
        // Drives init() on a fresh process so we exercise the embedded path.
        // Other tests in this module can rely on `init()` having run.
        init(Some("en"));
        let lang = active_language();
        assert_eq!(lang, "en");
    }

    #[test]
    fn unknown_language_falls_back_to_english() {
        init(Some("xx-YY"));
        assert_eq!(active_language(), "en");
    }

    #[test]
    fn region_stripped_fallback() {
        // `en-GB` should resolve to `en` because we ship `en` but not `en-GB`.
        // This test currently asserts trivially because `en` is always present;
        // it becomes load-bearing once we ship a region-tagged catalog.
        init(Some("en-GB"));
        assert_eq!(active_language(), "en");
    }

    #[test]
    fn translate_known_key() {
        init(Some("en"));
        let s = translate("errors.unknown", &[]);
        assert_eq!(s, "Unknown error");
    }

    #[test]
    fn translate_with_params() {
        init(Some("en"));
        let s = translate("errors.connection.lost", &[("reason", "socket closed")]);
        assert_eq!(s, "Connection lost: socket closed");
    }

    #[test]
    fn missing_param_left_literal() {
        init(Some("en"));
        // Drop the `reason` param on purpose. Output keeps the placeholder so
        // the bug is visible during dev rather than swallowed.
        let s = translate("errors.connection.lost", &[]);
        assert_eq!(s, "Connection lost: {reason}");
    }

    #[test]
    fn missing_key_surfaces_the_key() {
        init(Some("en"));
        let s = translate("does.not.exist", &[]);
        assert_eq!(s, "does.not.exist");
    }

    #[test]
    fn macro_expands_correctly() {
        init(Some("en"));
        let s = crate::t!("errors.connection.lost", "reason" => "boom");
        assert_eq!(s, "Connection lost: boom");
        let s2 = crate::t!("errors.unknown");
        assert_eq!(s2, "Unknown error");
    }

    #[test]
    fn catalog_parse_rejects_invalid_entry() {
        let bad = r#"{ "_meta": { "language": "xx" }, "k": 42 }"#;
        let r = Catalog::parse(bad);
        assert!(r.is_err());
    }

    #[test]
    fn interpolate_escapes_unmatched_braces() {
        // A literal `{not_a_param}` with no matching key should pass through.
        // The drift-check only checks well-formed `{name}` segments inside
        // catalog values, so a malformed runtime template still degrades gracefully.
        let s = interpolate("hello {a} world", &[("b", "BOOM")]);
        assert_eq!(s, "hello {a} world");
    }
}
