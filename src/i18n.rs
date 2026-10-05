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
//! "errors.passthrough": {
//!   "message": "{message}",
//!   "description": "Wrapper for free-form error text from upstream sources"
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

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use std::collections::HashMap;
use std::sync::OnceLock;

/// One raw `messages.json` value. Regular entries carry `message`; the
/// reserved `_meta` block carries the metadata fields instead. A single
/// shape lets the catalog parse in one pass straight from the text, with
/// no intermediate `serde_json::Value` DOM. Translator-only fields
/// (`description`, `_source_hash`, `_machine_translated`) are skipped by
/// serde without allocating.
#[derive(Deserialize)]
struct RawEntry {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    language: String,
    #[serde(default)]
    rtl: bool,
    #[serde(default)]
    machine_translated: bool,
}

/// Top-level catalog metadata. Lives under the reserved `_meta` key.
#[derive(Debug, Clone, Deserialize, Default)]
struct Meta {
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

/// Just the `_meta` block of a catalog; every other key is skipped. Used to
/// list languages without parsing all of their messages.
#[derive(Deserialize)]
struct MetaOnly {
    #[serde(default, rename = "_meta")]
    meta: Meta,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    pub language: String,
    pub rtl: bool,
    pub machine_translated: bool,
    /// key → message.
    entries: HashMap<String, String>,
}

impl Catalog {
    fn parse(raw: &str) -> Result<Self, String> {
        let value: HashMap<String, RawEntry> =
            serde_json::from_str(raw).map_err(|e| format!("catalog json parse failed: {}", e))?;

        let mut cat = Catalog {
            language: String::new(),
            rtl: false,
            machine_translated: false,
            entries: HashMap::with_capacity(value.len()),
        };
        for (k, v) in value {
            if k == "_meta" {
                cat.language = v.language;
                cat.rtl = v.rtl;
                cat.machine_translated = v.machine_translated;
                continue;
            }
            // A malformed entry fails the whole catalog so a broken file is
            // caught by the parse-every-catalog test, not at lookup time.
            let message = v
                .message
                .ok_or_else(|| format!("catalog entry {:?} has no message", k))?;
            cat.entries.insert(k, message);
        }
        Ok(cat)
    }

    /// Look up a key. Returns `None` for missing keys; the caller picks the
    /// fallback strategy.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(|s| s.as_str())
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
    // CLDR top-30 languages plus the four RTL ones (ar, he, fa, ur).
    // Catalogs under `locales/<code>/messages.json` are seeded by
    // `scripts/translate.py` against the canonical English catalog and
    // re-validated by `scripts/check_i18n.py` on every CI run, so adding
    // a language here is paired one-for-one with the file under `locales/`.
    "ar", "bn", "cs", "da", "de", "el", "es", "fa", "fi", "fr", "he", "hi", "hu", "id", "it", "ja",
    "ko", "nl", "no", "pl", "pt", "ro", "ru", "sv", "th", "tr", "uk", "ur", "vi", "zh-CN", "zh-TW",
);

/// One lazily-parsed slot per `EMBEDDED` entry (same index). Parsing all 32
/// catalogs (~11 MB) up front cost startup time before any window could
/// paint, for languages nobody uses; now only EN and the active language
/// are parsed, each at most once per process. `None` in a slot means that
/// catalog failed to parse.
static PARSED: OnceLock<Vec<OnceLock<Option<Catalog>>>> = OnceLock::new();

/// The parsed catalog for `code`, parsing it on first use. `None` when the
/// code isn't embedded or its catalog failed to parse.
fn catalog(code: &str) -> Option<&'static Catalog> {
    let idx = EMBEDDED.iter().position(|(c, _)| *c == code)?;
    let slots = PARSED.get_or_init(|| EMBEDDED.iter().map(|_| OnceLock::new()).collect());
    slots[idx]
        .get_or_init(|| {
            let (code, raw) = EMBEDDED[idx];
            match Catalog::parse(raw) {
                Ok(mut cat) => {
                    // Use the embed key as the canonical code rather than
                    // trusting the catalog's own _meta.language — that way a
                    // copy/paste mistake in the json doesn't silently route
                    // lookups for "ja" to the "ko" file.
                    if cat.language.is_empty() {
                        cat.language = code.to_string();
                    }
                    Some(cat)
                }
                Err(e) => {
                    // Catalog parse failures are programmer errors, not user
                    // errors — they mean a hand-edited or build-corrupted JSON
                    // file shipped. Crash loudly during dev, but degrade
                    // gracefully in release: fall through to English.
                    debug_assert!(false, "i18n catalog {} failed to parse: {}", code, e);
                    log::error!("i18n: catalog {} failed to parse: {}", code, e);
                    None
                }
            }
        })
        .as_ref()
}

/// The user's currently active language code (e.g. "en", "ja", "ar").
/// Set by `set_language()`; defaults to "en" until `init()` runs.
static ACTIVE: OnceLock<std::sync::RwLock<String>> = OnceLock::new();

fn active_lock() -> &'static std::sync::RwLock<String> {
    ACTIVE.get_or_init(|| std::sync::RwLock::new("en".to_string()))
}

/// Pick the active language and parse its catalog plus the English fallback.
/// Should be called once during `main()` startup, before any code touches
/// `t!`. Other catalogs are parsed on demand by `set_language`.
///
/// Returns the resolved active language code so callers can log it.
pub fn init(preferred: Option<&str>) -> String {
    let _ = catalog("en");
    let resolved = resolve_language(preferred);
    *active_lock().write().unwrap() = resolved.clone();
    resolved
}

/// Pick the best available language for a user preference or OS locale tag
/// (see `language_candidates`), falling back to "en".
fn resolve_language(preferred: Option<&str>) -> String {
    if let Some(p) = preferred {
        for candidate in language_candidates(p) {
            if catalog(&candidate).is_some() {
                return candidate;
            }
        }
    }
    "en".to_string()
}

/// Catalog codes to try for a locale tag, best match first. OS tags don't
/// line up with our codes one-to-one: case varies, POSIX uses `_` and a
/// `.UTF-8` / `@euro` suffix, macOS adds a script subtag (`zh-Hans-CN`),
/// Windows reports Norwegian as `nb-NO` / `nn-NO` (we ship `no`), and
/// Chinese has to map onto `zh-CN` (Simplified) or `zh-TW` (Traditional).
fn language_candidates(tag: &str) -> Vec<String> {
    let tag = tag.split(['.', '@']).next().unwrap_or("").trim();
    let parts: Vec<&str> = tag.split(['-', '_']).filter(|s| !s.is_empty()).collect();
    let Some(first) = parts.first() else {
        return Vec::new();
    };
    let lang = first.to_ascii_lowercase();
    let mut script: Option<String> = None;
    let mut region: Option<String> = None;
    for p in &parts[1..] {
        if p.len() == 4 {
            if script.is_none() {
                script = Some(p.to_ascii_lowercase());
            }
        } else if region.is_none() {
            region = Some(p.to_ascii_uppercase());
        }
    }

    if lang == "zh" {
        let traditional = match script.as_deref() {
            Some("hant") => true,
            Some("hans") => false,
            _ => matches!(region.as_deref(), Some("TW" | "HK" | "MO")),
        };
        let code = if traditional { "zh-TW" } else { "zh-CN" };
        return vec![code.to_string()];
    }

    let lang = match lang.as_str() {
        "nb" | "nn" => "no".to_string(),
        _ => lang,
    };
    let mut out = Vec::with_capacity(2);
    if let Some(r) = region {
        out.push(format!("{}-{}", lang, r));
    }
    out.push(lang);
    out
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
    catalog(&active_language()).is_some_and(|c| c.rtl)
}

/// `true` if the active catalog is mostly machine-translated. Surfaced in the
/// settings UI as a banner.
pub fn active_is_machine_translated() -> bool {
    catalog(&active_language()).is_some_and(|c| c.machine_translated)
}

/// A catalog serialised for the frontend as `{ key: { message } }` — the
/// same shape as `messages.json` minus translator-only fields. Serialises
/// straight from the parsed catalog, so shipping it clones nothing. An
/// unknown or unparseable code serialises as `{}`.
#[derive(Debug, Clone, Copy)]
pub struct CatalogView(Option<&'static Catalog>);

#[derive(Serialize)]
struct EntrySnapshot<'a> {
    message: &'a str,
}

impl Serialize for CatalogView {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let entries = self.0.map(|c| &c.entries);
        let mut map = serializer.serialize_map(Some(entries.map_or(0, |e| e.len())))?;
        for (k, v) in entries.into_iter().flatten() {
            map.serialize_entry(k, &EntrySnapshot { message: v })?;
        }
        map.end()
    }
}

/// The catalog for `code`, ready to ship to the frontend.
pub fn catalog_view(code: &str) -> CatalogView {
    CatalogView(catalog(code))
}

/// Every embedded language as `(code, display_name, rtl, machine_translated)`.
/// Used by the settings UI to populate the language dropdown. Reads only each
/// catalog's `_meta` block (once per process) instead of parsing them all.
pub fn available_languages() -> Vec<(String, String, bool, bool)> {
    static LIST: OnceLock<Vec<(String, String, bool, bool)>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut out: Vec<(String, String, bool, bool)> = EMBEDDED
            .iter()
            .filter_map(|(code, raw)| {
                let meta = match serde_json::from_str::<MetaOnly>(raw) {
                    Ok(m) => m.meta,
                    Err(e) => {
                        log::error!("i18n: catalog {} metadata failed to parse: {}", code, e);
                        return None;
                    }
                };
                let name = if meta.name.is_empty() {
                    (*code).to_string()
                } else {
                    meta.name
                };
                Some(((*code).to_string(), name, meta.rtl, meta.machine_translated))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    })
    .clone()
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
    let raw = catalog(lang)
        .and_then(|c| c.get(key))
        .or_else(|| catalog("en").and_then(|c| c.get(key)))
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

    // Tests use stable keys that are guaranteed to exist in the EN catalog
    // for the lifetime of the project: `errors.passthrough` (the wrapper
    // every free-form AppError routes through) and `settings.about.title`
    // (a static label). Avoids brittle coupling between the i18n test suite
    // and the catalog's evolving content.

    #[test]
    fn translate_known_key() {
        init(Some("en"));
        let s = translate("settings.about.title", &[]);
        assert_eq!(s, "About Kage");
    }

    #[test]
    fn translate_with_params() {
        init(Some("en"));
        let s = translate("errors.passthrough", &[("message", "socket closed")]);
        assert_eq!(s, "socket closed");
    }

    #[test]
    fn missing_param_left_literal() {
        init(Some("en"));
        // Drop the `message` param on purpose. Output keeps the placeholder so
        // the bug is visible during dev rather than swallowed.
        let s = translate("errors.passthrough", &[]);
        assert_eq!(s, "{message}");
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
        let s = crate::t!("errors.passthrough", "message" => "boom");
        assert_eq!(s, "boom");
        let s2 = crate::t!("settings.about.title");
        assert_eq!(s2, "About Kage");
    }

    #[test]
    fn catalog_parse_rejects_invalid_entry() {
        let bad = r#"{ "_meta": { "language": "xx" }, "k": 42 }"#;
        let r = Catalog::parse(bad);
        assert!(r.is_err());
    }

    #[test]
    fn catalog_parse_rejects_entry_without_message() {
        let bad = r#"{ "_meta": { "language": "xx" }, "k": { "description": "d" } }"#;
        assert!(Catalog::parse(bad).is_err());
    }

    #[test]
    fn every_embedded_catalog_parses() {
        // Catalogs now parse lazily, so a corrupt one would otherwise only
        // surface when a user picks that language. Catch it in CI instead.
        for (code, raw) in EMBEDDED {
            let cat = Catalog::parse(raw)
                .unwrap_or_else(|e| panic!("catalog {} failed to parse: {}", code, e));
            assert!(cat.key_count() > 0, "catalog {} is empty", code);
        }
    }

    #[test]
    fn available_languages_lists_every_embedded_catalog() {
        let langs = available_languages();
        assert_eq!(langs.len(), EMBEDDED.len());
        let ja = langs.iter().find(|l| l.0 == "ja").expect("ja listed");
        assert_ne!(ja.1, "ja", "display name comes from _meta");
        assert!(langs.iter().any(|l| l.0 == "ar" && l.2), "ar is rtl");
    }

    #[test]
    fn os_locale_tags_resolve_to_shipped_catalogs() {
        // Windows Norwegian, macOS script-tagged Chinese, POSIX-style tags,
        // and odd casing must all land on the catalog we actually ship.
        assert_eq!(resolve_language(Some("nb-NO")), "no");
        assert_eq!(resolve_language(Some("nn-NO")), "no");
        assert_eq!(resolve_language(Some("zh-Hans-CN")), "zh-CN");
        assert_eq!(resolve_language(Some("zh-Hant-TW")), "zh-TW");
        assert_eq!(resolve_language(Some("zh-Hant")), "zh-TW");
        assert_eq!(resolve_language(Some("zh-HK")), "zh-TW");
        assert_eq!(resolve_language(Some("zh-SG")), "zh-CN");
        assert_eq!(resolve_language(Some("zh")), "zh-CN");
        assert_eq!(resolve_language(Some("zh_CN.UTF-8")), "zh-CN");
        assert_eq!(resolve_language(Some("ZH-cn")), "zh-CN");
        assert_eq!(resolve_language(Some("de_DE@euro")), "de");
        assert_eq!(resolve_language(Some("pt-BR")), "pt");
        assert_eq!(resolve_language(Some("JA")), "ja");
        assert_eq!(resolve_language(Some("")), "en");
        assert_eq!(resolve_language(Some("xx-YY")), "en");
    }

    #[test]
    fn catalog_view_ships_messages_only() {
        let json = serde_json::to_value(catalog_view("en")).unwrap();
        let entry = json.get("settings.about.title").expect("key present");
        assert_eq!(entry, &serde_json::json!({ "message": "About Kage" }));
        assert_eq!(
            serde_json::to_value(catalog_view("xx")).unwrap(),
            serde_json::json!({})
        );
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
