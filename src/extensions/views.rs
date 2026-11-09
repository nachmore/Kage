//! `contributes.views`: windows an extension draws itself
//! (docs/design/EXTENSION_VIEWS.md §4).
//!
//! The manifest keeps `views` as raw JSON, so a bad value can't make the
//! manifest unreadable. [`normalize_views`] reads it. A bad entry is
//! dropped with a warning, like an unknown capability. The JS mirror is
//! `ui/js/shared/extension-view-policy.js`; both sides apply these rules:
//!
//! - Only the first [`MAX_VIEWS`] entries are read; the rest are ignored.
//! - An entry is dropped when it isn't an object, its `id` is invalid or
//!   already used, or its `module` isn't a bundle-relative `.js` path.
//! - Other bad fields fall back instead: `title` to the extension's name,
//!   `icon` to the extension's icon, `css` entries one by one, sizes to
//!   their defaults.
//! - Lengths count Unicode code points (JS: `Array.from(s).length`).
//!
//! Params validation for `open_view` (§6.2) lives here too, because the
//! JS host and `ext_view_open` apply the same rule.

use crate::error::{AppError, ErrorKind};
use serde::Serialize;

/// Most views a manifest may declare.
pub const MAX_VIEWS: usize = 4;
/// Longest view id, in characters.
pub const MAX_VIEW_ID_LEN: usize = 32;
/// Longest title after `__MSG_*__` resolution, in code points.
pub const MAX_TITLE_CHARS: usize = 60;
/// Longest icon, in code points.
pub const MAX_ICON_CHARS: usize = 8;
/// Most CSS files a view may list.
pub const MAX_CSS_FILES: usize = 8;
/// Largest single CSS file (256 KiB). The view page checks it when it
/// reads the file.
pub const MAX_CSS_FILE_BYTES: u64 = 256 * 1024;
/// Largest total of a view's CSS files (1 MiB).
pub const MAX_CSS_TOTAL_BYTES: u64 = 1024 * 1024;

/// Size used when `defaultSize` is missing.
pub const DEFAULT_SIZE: ViewSize = ViewSize {
    width: 1100,
    height: 760,
};
/// Smallest `defaultSize`.
pub const DEFAULT_SIZE_MIN: ViewSize = ViewSize {
    width: 480,
    height: 360,
};
/// Largest `defaultSize`.
pub const DEFAULT_SIZE_MAX: ViewSize = ViewSize {
    width: 3840,
    height: 2160,
};
/// `minSize` used when it's missing.
pub const DEFAULT_MIN_SIZE: ViewSize = ViewSize {
    width: 480,
    height: 360,
};
/// Smallest `minSize`. The largest is the view's `defaultSize`.
pub const MIN_SIZE_FLOOR: ViewSize = ViewSize {
    width: 320,
    height: 240,
};

/// Largest `open_view` params, as compact JSON, in bytes.
pub const MAX_PARAMS_BYTES: usize = 4096;
/// Deepest `open_view` params. The top-level object is depth 1; each
/// nested object or array adds one.
pub const MAX_PARAMS_DEPTH: usize = 4;

/// A width and height in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ViewSize {
    pub width: u32,
    pub height: u32,
}

/// One valid `contributes.views` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewDecl {
    /// Matches `^[a-z][a-z0-9-]{0,31}$`; unique within the extension.
    pub id: String,
    /// Control and bidi-override characters removed. A `__MSG_key__`
    /// token is kept whole: the view page resolves it and applies
    /// [`MAX_TITLE_CHARS`] to the result.
    pub title: String,
    /// One emoji, or the extension's icon (which may be empty).
    pub icon: String,
    /// Bundle-relative `.js` path, as written in the manifest.
    pub module: String,
    /// Bundle-relative `.css` paths, at most [`MAX_CSS_FILES`].
    pub css: Vec<String>,
    pub default_size: ViewSize,
    /// At least [`MIN_SIZE_FLOOR`], at most `default_size`.
    pub min_size: ViewSize,
}

/// Whether `id` is a valid view id: `^[a-z][a-z0-9-]{0,31}$`. A valid id
/// can't contain the `:` that separates the parts of a view window label.
pub fn is_valid_view_id(id: &str) -> bool {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    id.len() <= MAX_VIEW_ID_LEN
        && first.is_ascii_lowercase()
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Bidi embedding, override and isolate characters (U+202A–U+202E,
/// U+2066–U+2069). The same set `copyTextError` refuses in
/// `ui/js/shared/settings-schema.js`.
pub fn is_bidi_override(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Whether `s` is a whole `__MSG_key__` manifest token.
fn is_msg_token(s: &str) -> bool {
    s.len() > "__MSG___".len() && s.starts_with("__MSG_") && s.ends_with("__")
}

/// Whether `path` is a bundle-relative path to a file ending in `ext`:
/// an optional leading `./`, then `/`-separated segments that are not
/// empty, `.` or `..`, with no `\`, `:`, `?`, `#`, `%` or control
/// characters. The same rules `resolveExtensionModule` applies to imports;
/// `read_extension_file` still checks containment on disk.
pub fn is_bundle_path(path: &str, ext: &str) -> bool {
    let rest = path.strip_prefix("./").unwrap_or(path);
    if rest.len() <= ext.len() || !rest.ends_with(ext) {
        return false;
    }
    if rest
        .chars()
        .any(|c| c.is_control() || matches!(c, '\\' | ':' | '?' | '#' | '%'))
    {
        return false;
    }
    rest.split('/')
        .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// Read a manifest's raw `contributes.views` value. `name` and `icon` are
/// the extension's, used when an entry's title or icon is unusable.
/// `context` names the extension in log messages.
pub fn normalize_views(
    raw: Option<&serde_json::Value>,
    name: &str,
    icon: &str,
    context: &str,
) -> Vec<ViewDecl> {
    let list = match raw {
        None | Some(serde_json::Value::Null) => return Vec::new(),
        Some(serde_json::Value::Array(list)) => list,
        Some(_) => {
            log::warn!(
                "Extension '{}': contributes.views must be an array — ignored",
                context
            );
            return Vec::new();
        }
    };
    if list.len() > MAX_VIEWS {
        log::warn!(
            "Extension '{}': contributes.views has more than {} entries; the rest are ignored",
            context,
            MAX_VIEWS
        );
    }
    let mut out: Vec<ViewDecl> = Vec::new();
    for (i, entry) in list.iter().take(MAX_VIEWS).enumerate() {
        match normalize_view(entry, name, icon, context) {
            Ok(decl) if out.iter().any(|v| v.id == decl.id) => log::warn!(
                "Extension '{}': view {} repeats the id {:?} — ignored",
                context,
                i,
                decl.id
            ),
            Ok(decl) => out.push(decl),
            Err(reason) => log::warn!(
                "Extension '{}': view {} is invalid ({}) — ignored",
                context,
                i,
                reason
            ),
        }
    }
    out
}

fn normalize_view(
    entry: &serde_json::Value,
    name: &str,
    icon: &str,
    context: &str,
) -> Result<ViewDecl, &'static str> {
    let obj = entry.as_object().ok_or("not an object")?;
    let id = obj
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|id| is_valid_view_id(id))
        .ok_or("id must match ^[a-z][a-z0-9-]{0,31}$")?;
    let module = obj
        .get("module")
        .and_then(|v| v.as_str())
        .filter(|m| is_bundle_path(m, ".js"))
        .ok_or("module must be a bundle-relative .js path")?;
    let default_size = read_size(
        obj.get("defaultSize"),
        DEFAULT_SIZE,
        DEFAULT_SIZE_MIN,
        DEFAULT_SIZE_MAX,
    );
    let min_size = read_size(
        obj.get("minSize"),
        DEFAULT_MIN_SIZE,
        MIN_SIZE_FLOOR,
        default_size,
    );
    Ok(ViewDecl {
        id: id.to_string(),
        title: view_title(obj.get("title"), name),
        icon: view_icon(obj.get("icon"), icon),
        module: module.to_string(),
        css: view_css(obj.get("css"), id, context),
        default_size,
        min_size,
    })
}

/// Remove control and bidi-override characters, trim, and cap a literal
/// title at [`MAX_TITLE_CHARS`]. Empty or missing: the extension's name.
fn view_title(raw: Option<&serde_json::Value>, name: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_control() && !is_bidi_override(*c))
            .collect::<String>()
            .trim()
            .to_string()
    };
    let title = clean(raw.and_then(|v| v.as_str()).unwrap_or(""));
    let title = if title.is_empty() { clean(name) } else { title };
    if is_msg_token(&title) {
        title
    } else {
        title.chars().take(MAX_TITLE_CHARS).collect()
    }
}

/// One emoji: 1–8 code points after trimming, none of them ASCII,
/// whitespace, control or bidi-override characters. Anything else: the
/// extension's icon.
fn view_icon(raw: Option<&serde_json::Value>, fallback: &str) -> String {
    let icon = raw.and_then(|v| v.as_str()).unwrap_or("").trim();
    let count = icon.chars().count();
    let ok = (1..=MAX_ICON_CHARS).contains(&count)
        && icon.chars().all(|c| {
            !c.is_ascii() && !c.is_whitespace() && !c.is_control() && !is_bidi_override(c)
        });
    if ok {
        icon.to_string()
    } else {
        fallback.to_string()
    }
}

/// Valid `.css` paths, deduplicated, at most [`MAX_CSS_FILES`]. Bad
/// entries are dropped one by one.
fn view_css(raw: Option<&serde_json::Value>, view: &str, context: &str) -> Vec<String> {
    let list = match raw {
        None | Some(serde_json::Value::Null) => return Vec::new(),
        Some(serde_json::Value::Array(list)) => list,
        Some(_) => {
            log::warn!(
                "Extension '{}': view '{}' css must be an array — ignored",
                context,
                view
            );
            return Vec::new();
        }
    };
    let mut out: Vec<String> = Vec::new();
    for entry in list {
        if out.len() == MAX_CSS_FILES {
            log::warn!(
                "Extension '{}': view '{}' lists more than {} css files; the rest are ignored",
                context,
                view,
                MAX_CSS_FILES
            );
            break;
        }
        match entry.as_str() {
            Some(path) if is_bundle_path(path, ".css") => {
                if !out.iter().any(|p| p == path) {
                    out.push(path.to_string());
                }
            }
            _ => log::warn!(
                "Extension '{}': view '{}' css entry {} isn't a bundle-relative .css path — ignored",
                context,
                view,
                entry
            ),
        }
    }
    out
}

/// Read `{ width, height }`. A dimension that's missing or not a finite
/// number uses `default`; every dimension is rounded and clamped to
/// `min..=max`.
fn read_size(
    raw: Option<&serde_json::Value>,
    default: ViewSize,
    min: ViewSize,
    max: ViewSize,
) -> ViewSize {
    let dim = |key: &str, default: u32, min: u32, max: u32| -> u32 {
        let value = raw
            .and_then(|v| v.get(key))
            .and_then(|v| v.as_f64())
            .filter(|n| n.is_finite())
            .map(|n| n.round().clamp(0.0, u32::MAX as f64) as u32)
            .unwrap_or(default);
        value.clamp(min, max.max(min))
    };
    ViewSize {
        width: dim("width", default.width, min.width, max.width),
        height: dim("height", default.height, min.height, max.height),
    }
}

/// Check `open_view` params (§6.2): absent or null, or a JSON object at
/// most [`MAX_PARAMS_DEPTH`] deep and [`MAX_PARAMS_BYTES`] long as compact
/// JSON. `serde_json` can't hold a non-finite number, so every number is
/// finite already.
pub fn validate_params(params: Option<&serde_json::Value>) -> Result<(), AppError> {
    let value = match params {
        None | Some(serde_json::Value::Null) => return Ok(()),
        Some(value) => value,
    };
    if !value.is_object() {
        return Err(params_invalid("must be a JSON object"));
    }
    if depth(value) > MAX_PARAMS_DEPTH {
        return Err(params_invalid(&format!(
            "nested more than {} levels deep",
            MAX_PARAMS_DEPTH
        )));
    }
    let size = serde_json::to_string(value)
        .map_err(|e| params_invalid(&e.to_string()))?
        .len();
    if size > MAX_PARAMS_BYTES {
        return Err(AppError::keyed(
            ErrorKind::InvalidInput,
            "errors.ext_view.params_too_large",
            &[
                ("size", &size.to_string()),
                ("limit", &MAX_PARAMS_BYTES.to_string()),
            ],
        ));
    }
    Ok(())
}

fn params_invalid(reason: &str) -> AppError {
    AppError::keyed(
        ErrorKind::InvalidInput,
        "errors.ext_view.params_invalid",
        &[("reason", reason)],
    )
}

/// How deeply `value` nests objects and arrays: a scalar is 0, `{}` is 1.
fn depth(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
        serde_json::Value::Array(list) => 1 + list.iter().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn views(raw: serde_json::Value) -> Vec<ViewDecl> {
        normalize_views(Some(&raw), "Mind Map", "🧠", "test")
    }

    fn one(entry: serde_json::Value) -> Option<ViewDecl> {
        views(json!([entry])).into_iter().next()
    }

    #[test]
    fn valid_entry_keeps_every_field() {
        let decl = one(json!({
            "id": "explorer",
            "title": "__MSG_view.explorer.title__",
            "icon": "🗺️",
            "module": "./explorer.js",
            "css": ["./explorer.css"],
            "defaultSize": { "width": 1200, "height": 800 },
            "minSize": { "width": 640, "height": 420 },
            "alwaysOnTop": true
        }))
        .expect("valid");
        assert_eq!(
            decl,
            ViewDecl {
                id: "explorer".into(),
                title: "__MSG_view.explorer.title__".into(),
                icon: "🗺️".into(),
                module: "./explorer.js".into(),
                css: vec!["./explorer.css".into()],
                default_size: ViewSize {
                    width: 1200,
                    height: 800
                },
                min_size: ViewSize {
                    width: 640,
                    height: 420
                },
            }
        );
        let wire = serde_json::to_value(&decl).unwrap();
        assert_eq!(wire["defaultSize"], json!({ "width": 1200, "height": 800 }));
        assert!(
            wire.get("alwaysOnTop").is_none(),
            "unknown fields are dropped"
        );
    }

    #[test]
    fn minimal_entry_gets_defaults() {
        let decl = one(json!({ "id": "board", "module": "board.js" })).expect("valid");
        assert_eq!(
            decl.title, "Mind Map",
            "missing title: the extension's name"
        );
        assert_eq!(decl.icon, "🧠", "missing icon: the extension's icon");
        assert!(decl.css.is_empty());
        assert_eq!(decl.default_size, DEFAULT_SIZE);
        assert_eq!(decl.min_size, DEFAULT_MIN_SIZE);
    }

    #[test]
    fn view_id_rule() {
        for ok in ["a", "explorer", "map-2", "a".repeat(32).as_str()] {
            assert!(is_valid_view_id(ok), "{ok:?} should be valid");
        }
        for bad in [
            "",
            "Explorer",
            "2map",
            "-map",
            "map_view",
            "map:view",
            "map view",
            "é",
            "a".repeat(33).as_str(),
        ] {
            assert!(!is_valid_view_id(bad), "{bad:?} should be invalid");
            assert!(one(json!({ "id": bad, "module": "./v.js" })).is_none());
        }
    }

    #[test]
    fn bad_entries_are_dropped() {
        let got = views(json!([
            "explorer",
            { "module": "./v.js" },
            { "id": "a", "module": 42 },
            { "id": "c", "module": "./c.js" },
        ]));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "c");
        assert!(one(json!({ "id": "b" })).is_none(), "module is required");
    }

    #[test]
    fn duplicate_id_keeps_the_first() {
        let got = views(json!([
            { "id": "map", "module": "./one.js" },
            { "id": "map", "module": "./two.js" },
        ]));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].module, "./one.js");
    }

    #[test]
    fn only_the_first_four_entries_are_read() {
        let entries: Vec<_> = (0..6)
            .map(|i| json!({ "id": format!("v{i}"), "module": "./v.js" }))
            .collect();
        let ids: Vec<String> = views(json!(entries)).into_iter().map(|v| v.id).collect();
        assert_eq!(ids, vec!["v0", "v1", "v2", "v3"]);

        // A bad entry still uses up one of the four places.
        let mut entries = vec![json!({ "id": "BAD", "module": "./v.js" })];
        entries.extend((0..4).map(|i| json!({ "id": format!("v{i}"), "module": "./v.js" })));
        assert_eq!(views(json!(entries)).len(), 3);
    }

    #[test]
    fn module_path_rules() {
        for ok in ["./view.js", "view.js", "./lib/view.js", "a/b/c.js"] {
            assert!(is_bundle_path(ok, ".js"), "{ok:?} should be valid");
        }
        for bad in [
            "",
            "./",
            ".js",
            "./.js",
            "./view.mjs",
            "./view.css",
            "../view.js",
            "./lib/../view.js",
            "./lib/./view.js",
            "./lib//view.js",
            "/abs/view.js",
            "C:/view.js",
            "lib\\view.js",
            "view.js?x=1",
            "view.js#top",
            "view%2e.js",
            "vi\new.js",
            "https://example.com/view.js",
        ] {
            assert!(!is_bundle_path(bad, ".js"), "{bad:?} should be invalid");
        }
        assert!(one(json!({ "id": "v", "module": "../escape.js" })).is_none());
    }

    #[test]
    fn title_is_cleaned_capped_and_falls_back() {
        let title = |t: serde_json::Value| {
            one(json!({ "id": "v", "module": "./v.js", "title": t }))
                .unwrap()
                .title
        };
        assert_eq!(title(json!("  Explorer  ")), "Explorer");
        assert_eq!(
            title(json!("Ex\u{202E}plo\u{0007}rer\u{2066}")),
            "Explorer",
            "control and bidi-override characters are removed"
        );
        assert_eq!(title(json!("")), "Mind Map");
        assert_eq!(title(json!("\u{202E}\n ")), "Mind Map");
        assert_eq!(title(json!(42)), "Mind Map");
        let long = "é".repeat(70);
        assert_eq!(title(json!(long)).chars().count(), MAX_TITLE_CHARS);
        let token = format!("__MSG_{}__", "k".repeat(80));
        assert_eq!(
            title(json!(token)),
            token,
            "a token is resolved, then capped, by the view page"
        );
    }

    #[test]
    fn icon_is_one_emoji_or_the_extension_icon() {
        let icon = |i: serde_json::Value| {
            one(json!({ "id": "v", "module": "./v.js", "icon": i }))
                .unwrap()
                .icon
        };
        assert_eq!(icon(json!("🗺️")), "🗺️");
        assert_eq!(icon(json!(" 👨‍👩‍👧 ")), "👨‍👩‍👧");
        for bad in [
            json!(""),
            json!("map"),
            json!("<b>"),
            json!("🗺️ 🗺️"),
            json!("🗺️\u{202E}"),
            json!(7),
            json!("🗺️".repeat(5)),
        ] {
            assert_eq!(icon(bad.clone()), "🧠", "{bad} should fall back");
        }
    }

    #[test]
    fn sizes_are_clamped() {
        let sizes = |d: serde_json::Value, m: serde_json::Value| {
            let v = one(json!({ "id": "v", "module": "./v.js", "defaultSize": d, "minSize": m }))
                .unwrap();
            (v.default_size, v.min_size)
        };
        let s = |width, height| ViewSize { width, height };
        assert_eq!(
            sizes(json!({ "width": 100, "height": 99999 }), json!(null)),
            (s(480, 2160), s(480, 360))
        );
        assert_eq!(
            sizes(
                json!({ "width": 1000.6, "height": -5 }),
                json!({ "width": 10, "height": 5000 })
            ),
            (s(1001, 360), s(320, 360)),
            "minSize is at least the floor and at most defaultSize"
        );
        assert_eq!(
            sizes(json!("big"), json!({ "width": "wide" })),
            (DEFAULT_SIZE, DEFAULT_MIN_SIZE),
            "a non-number dimension uses its default"
        );
        assert_eq!(
            sizes(
                json!({ "width": 3840, "height": 2160 }),
                json!({ "width": 4000, "height": 3000 })
            ),
            (s(3840, 2160), s(3840, 2160))
        );
    }

    #[test]
    fn css_limits() {
        let css = |c: serde_json::Value| {
            one(json!({ "id": "v", "module": "./v.js", "css": c }))
                .unwrap()
                .css
        };
        assert_eq!(
            css(json!([
                "./a.css", "./a.css", "../b.css", 3, "c.css", "./d.js"
            ])),
            vec!["./a.css", "c.css"],
            "bad entries drop one by one; duplicates collapse"
        );
        let many: Vec<String> = (0..12).map(|i| format!("./s{i}.css")).collect();
        assert_eq!(css(json!(many)).len(), MAX_CSS_FILES);
        assert!(
            css(json!("./a.css")).is_empty(),
            "not an array: ignored, view kept"
        );
    }

    #[test]
    fn non_array_views_is_ignored() {
        for raw in [
            json!({ "id": "v", "module": "./v.js" }),
            json!("explorer"),
            json!(true),
        ] {
            assert!(views(raw).is_empty());
        }
        assert!(normalize_views(None, "n", "i", "test").is_empty());
        assert!(normalize_views(Some(&json!(null)), "n", "i", "test").is_empty());
    }

    fn err_key(r: Result<(), AppError>) -> String {
        let e = r.expect_err("should be refused");
        assert!(matches!(e.kind, ErrorKind::InvalidInput), "{:?}", e.kind);
        e.key
    }

    #[test]
    fn params_rules() {
        assert!(validate_params(None).is_ok());
        assert!(validate_params(Some(&json!(null))).is_ok());
        assert!(validate_params(Some(&json!({}))).is_ok());
        assert!(validate_params(Some(
            &json!({ "focus": "e1", "n": 1.5, "ok": true, "list": [1, null] })
        ))
        .is_ok());
        for bad in [json!([]), json!("x"), json!(3), json!(false)] {
            assert_eq!(
                err_key(validate_params(Some(&bad))),
                "errors.ext_view.params_invalid"
            );
        }
    }

    #[test]
    fn params_depth() {
        assert_eq!(depth(&json!(1)), 0);
        assert_eq!(depth(&json!({})), 1);
        assert_eq!(depth(&json!({ "a": [1] })), 2);
        let four = json!({ "a": { "b": { "c": [1] } } });
        assert_eq!(depth(&four), 4);
        assert!(validate_params(Some(&four)).is_ok());
        let five = json!({ "a": { "b": { "c": [[]] } } });
        assert_eq!(
            err_key(validate_params(Some(&five))),
            "errors.ext_view.params_invalid"
        );
    }

    #[test]
    fn params_size() {
        // {"s":"…"} is 8 bytes plus the string.
        let fits = json!({ "s": "x".repeat(MAX_PARAMS_BYTES - 8) });
        assert!(validate_params(Some(&fits)).is_ok());
        let over = json!({ "s": "x".repeat(MAX_PARAMS_BYTES - 7) });
        let e = validate_params(Some(&over)).expect_err("too large");
        assert_eq!(e.key, "errors.ext_view.params_too_large");
        assert_eq!(e.params["size"], (MAX_PARAMS_BYTES + 1).to_string());
        assert_eq!(e.params["limit"], MAX_PARAMS_BYTES.to_string());
    }
}
