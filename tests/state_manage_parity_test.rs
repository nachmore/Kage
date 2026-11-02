//! Every Tauri state access — `State<'_, T>` in a command signature or
//! let-binding, `.state::<T>()`, `.try_state::<T>()` — must name a type
//! that main.rs actually `.manage()`s. The compiler can't check this
//! (state lookup is runtime-typed), and the failure modes are nasty:
//!
//!   - `.state::<T>()` on an unmanaged type PANICS. Shipped once:
//!     `spawn_changelog_cache_refresh` requested `Arc<Mutex<Config>>`
//!     (Config is only managed inside FeatureServices) and crashed
//!     every launch of that nightly during setup.
//!   - `State<'_, T>` in a `#[tauri::command]` fails the invoke with
//!     "state not managed" — the command is dead but nothing says so
//!     until a user hits it.
//!   - `.try_state::<T>()` returns `None` forever — silent feature
//!     rot.
//!
//! This test parses the source as text (same approach as
//! `command_error_type_parity_test.rs`). MANAGED_TYPES mirrors the
//! `.manage(...)` calls in main.rs; if you add a new managed type,
//! add it here too — the test failure message will tell you.

use std::fs;
use std::path::{Path, PathBuf};

/// The types registered via `.manage()` on the Builder in main.rs.
/// Keep in sync with the `.manage(...)` block there (the variables are
/// snake_case instances of exactly these types).
const MANAGED_TYPES: &[&str] = &["AcpHandles", "UiState", "ChildProcesses", "FeatureServices"];

/// State owned by a Tauri plugin (managed by the plugin's own init, not by
/// main.rs). Allowed ONLY through `try_state::<…>()`, which returns `None`
/// instead of panicking when the plugin isn't registered — used to detect
/// the headless mock harness, which skips global-shortcut.
const PLUGIN_STATE_TYPES_VIA_TRY_STATE: &[&str] = &["GlobalShortcut"];

fn is_plugin_state(type_param: &str) -> bool {
    let t: String = type_param.split_whitespace().collect();
    let no_generics = t.split('<').next().unwrap_or(&t);
    let base = no_generics.rsplit("::").next().unwrap_or(no_generics);
    PLUGIN_STATE_TYPES_VIA_TRY_STATE.contains(&base)
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_source_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(&repo_root().join("src"), &mut out);
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// Extract the text between `open_idx` (pointing at `<`) and its
/// matching `>`, handling nested generics.
fn balanced_angle(source: &str, open_idx: usize) -> Option<&str> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate().skip(open_idx) {
        match b {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&source[open_idx + 1..i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether a state type parameter names a managed type. Strips path
/// prefixes (`crate::state::UiState` → `UiState`). Generic types can't
/// match — none of the managed types are generic.
fn is_managed(type_param: &str) -> bool {
    let t: String = type_param.split_whitespace().collect();
    if t.contains('<') {
        return false;
    }
    let base = t.rsplit("::").next().unwrap_or(&t);
    MANAGED_TYPES.contains(&base)
}

fn line_of(source: &str, byte_idx: usize) -> usize {
    source[..byte_idx].matches('\n').count() + 1
}

/// Collect every state-access type parameter in a file:
/// `State<'_, T>`, `.state::<T>()`, `.try_state::<T>()`.
/// `(line, type, via_try_state)` for every state access in `source`.
fn scan_file(source: &str) -> Vec<(usize, String)> {
    scan_file_tagged(source)
        .into_iter()
        .filter(|(_, ty, via_try)| !(*via_try && is_plugin_state(ty)))
        .map(|(line, ty, _)| (line, ty))
        .collect()
}

fn scan_file_tagged(source: &str) -> Vec<(usize, String, bool)> {
    let mut found = Vec::new();

    // `State<'_, T>` — command args and let-binding annotations.
    let mut from = 0;
    while let Some(rel) = source[from..].find("State<") {
        let idx = from + rel;
        // Skip matches that are part of a longer identifier (e.g. AppState<).
        let prev = source[..idx].chars().next_back();
        let standalone = !prev.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        let open = idx + "State".len();
        if let Some(inner) = balanced_angle(source, open) {
            if standalone {
                // Drop the lifetime: `'_, T` / `'r, T` → `T`.
                if let Some((_, ty)) = inner.split_once(',') {
                    found.push((line_of(source, idx), ty.trim().to_string(), false));
                }
            }
            from = open + 1;
        } else {
            break;
        }
    }

    // `.state::<T>()` and `.try_state::<T>()` turbofish.
    for needle in ["state::<", "try_state::<"] {
        let mut from = 0;
        while let Some(rel) = source[from..].find(needle) {
            let idx = from + rel;
            // `state::<` also matches inside `try_state::<`; require a
            // non-identifier char before so each occurrence is counted
            // once per pattern (dedup below handles the overlap).
            let open = idx + needle.len() - 1;
            if let Some(inner) = balanced_angle(source, open) {
                // `state::<` is a suffix of `try_state::<`; tag by what
                // precedes it so the plugin allowance can't leak to state::<.
                let via_try = needle == "try_state::<" || source[..idx].ends_with("try_");
                found.push((line_of(source, idx), inner.trim().to_string(), via_try));
                from = open + 1;
            } else {
                break;
            }
        }
    }

    found.sort();
    found.dedup();
    found
}

#[test]
fn every_state_access_names_a_managed_type() {
    let mut offenses = Vec::new();
    for path in rust_source_files() {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if !content.contains("State<") && !content.contains("state::<") {
            continue;
        }
        for (line, ty) in scan_file(&content) {
            if !is_managed(&ty) {
                let rel = path
                    .strip_prefix(repo_root())
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|_| path.to_string_lossy().to_string());
                offenses.push(format!("  {rel}:{line} — State<{ty}>"));
            }
        }
    }

    if !offenses.is_empty() {
        panic!(
            "These Tauri state accesses name types that main.rs never .manage()s.\n\
             state::<T>() on an unmanaged type PANICS at runtime; a command arg\n\
             fails every invoke with 'state not managed'. Either access the value\n\
             through a managed container (Config lives in FeatureServices), or add\n\
             the type to .manage() in main.rs AND to MANAGED_TYPES in this test:\n\n{}",
            offenses.join("\n")
        );
    }
}

#[cfg(test)]
mod parser_self_test {
    use super::*;

    #[test]
    fn detects_unmanaged_command_arg() {
        let src = "pub fn cmd(config: State<'_, std::sync::Arc<Mutex<Config>>>) {}";
        let found = scan_file(src);
        assert_eq!(found.len(), 1);
        assert!(!is_managed(&found[0].1));
    }

    #[test]
    fn detects_unmanaged_turbofish() {
        let src = "let c = app.state::<Arc<Mutex<Config>>>();";
        let found = scan_file(src);
        assert!(found.iter().any(|(_, t)| !is_managed(t)));
    }

    #[test]
    fn allows_managed_types() {
        let src = r#"
            fn a(s: State<'_, FeatureServices>) {}
            fn b(s: tauri::State<'_, crate::state::UiState>) {}
            fn c(app: &AppHandle) { let _ = app.try_state::<AcpHandles>(); }
            fn d(app: &AppHandle) { let _ = app.state::<state::ChildProcesses>(); }
        "#;
        for (_, ty) in scan_file(src) {
            assert!(is_managed(&ty), "expected managed: {ty}");
        }
    }

    #[test]
    fn plugin_state_allowed_only_via_try_state() {
        let ok = "fn a(app: &AppHandle<R>) { app.try_state::<tauri_plugin_global_shortcut::GlobalShortcut<R>>(); }";
        assert!(
            scan_file(ok).is_empty(),
            "try_state on plugin state is safe"
        );
        // state::<…>() panics when the plugin is missing — still flagged.
        let bad = "fn a(app: &AppHandle<R>) { app.state::<GlobalShortcut<R>>(); }";
        assert_eq!(scan_file(bad).len(), 1);
        // As a command arg it fails every invoke — still flagged.
        let bad_arg = "fn a(s: State<'_, GlobalShortcut<R>>) {}";
        assert_eq!(scan_file(bad_arg).len(), 1);
        // The allowance is by name: other plugin-ish types stay flagged.
        let other = "fn a(app: &AppHandle) { app.try_state::<SomethingElse>(); }";
        assert_eq!(scan_file(other).len(), 1);
    }

    #[test]
    fn ignores_longer_identifiers_ending_in_state() {
        // `AppState<T>` or `MyState<..>` must not be parsed as tauri State.
        let src = "fn x(s: AppState<Config>) {}";
        assert!(scan_file(src).is_empty());
    }
}
