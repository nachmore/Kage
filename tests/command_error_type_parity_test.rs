//! Every `#[tauri::command]` must return `Result<_, AppError>` (or no
//! Result at all). The frontend depends on the structured `{ kind,
//! message }` payload to render error UIs and (in the future) branch on
//! error categories. A command that returns `Result<_, String>` instead
//! breaks `errMessage(e)` consumers — they'd receive a string rather
//! than an object — and silently degrades the error surface.
//!
//! This test parses the source files as text and asserts the rule. Catches:
//!   1. New `#[tauri::command]` added with `Result<_, String>` (the easy
//!      mistake — clippy doesn't flag it).
//!   2. An old command being modified back to `Result<_, String>` from
//!      `Result<_, AppError>` (e.g. by a hand-rolled error type).
//!
//! Allowed return shapes:
//!   - `Result<T, AppError>` — the canonical case
//!   - `Result<T, crate::error::AppError>` — fully qualified
//!   - any non-Result type — commands that can't fail (e.g. fire-and-forget
//!     state mutators) don't need a Result at all
//!
//! Anything else fails the test.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Walk `src/` recursively and collect `.rs` files.
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

/// One offending command, with the file path and the return-type slice
/// for a useful failure message.
#[derive(Debug)]
struct Offense {
    file: PathBuf,
    line: usize,
    function: String,
    return_type: String,
}

/// Returns the function name in `pub (async )?fn <name>(`, or None if
/// the line doesn't declare a function.
fn parse_fn_name(signature_line: &str) -> Option<String> {
    let trimmed = signature_line.trim_start();
    let after_pub = trimmed
        .strip_prefix("pub ")
        .or(Some(trimmed))
        .unwrap_or(trimmed);
    let after_async = after_pub.strip_prefix("async ").unwrap_or(after_pub);
    let after_fn = after_async.strip_prefix("fn ")?;
    let name: String = after_fn
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Whether the return-type slice (between `->` and the first `{` of the
/// body) is allowed. Allowed: any non-Result, or `Result<_, AppError>` /
/// `Result<_, crate::error::AppError>`. Rejected: anything else,
/// notably `Result<_, String>`.
fn is_allowed_return_type(rt: &str) -> bool {
    let rt = rt.trim();
    // Whitespace-collapse to make the regex-free match below readable.
    let collapsed: String = rt.split_whitespace().collect::<Vec<_>>().join(" ");
    if !collapsed.starts_with("Result<") {
        // Returns `()` implicitly or some other type — not our concern.
        return true;
    }
    // Require AppError as the error type. The Result<…> in the signature
    // can have arbitrarily-nested generics in the Ok-arm, so we look for
    // an explicit AppError suffix.
    collapsed.ends_with(", AppError>")
        || collapsed.ends_with(",AppError>")
        || collapsed.ends_with(", crate::error::AppError>")
        || collapsed.ends_with(",crate::error::AppError>")
}

/// Scan a single file for `#[tauri::command]`-annotated functions
/// returning a non-conforming Result.
fn scan_file(path: &Path, content: &str) -> Vec<Offense> {
    let mut offenses = Vec::new();
    let lines: Vec<&str> = content.lines().collect();

    for (idx, line) in lines.iter().enumerate() {
        if !line.trim_start().starts_with("#[tauri::command]") {
            continue;
        }
        // Find the function signature on the next non-attribute line.
        // Allow extra `#[…]` attributes between `#[tauri::command]` and the
        // function (e.g. `#[allow(clippy::too_many_arguments)]`).
        let mut sig_start = idx + 1;
        while sig_start < lines.len() {
            let trimmed = lines[sig_start].trim_start();
            if trimmed.starts_with("#[") {
                sig_start += 1;
                continue;
            }
            break;
        }
        if sig_start >= lines.len() {
            continue;
        }
        let fn_line = lines[sig_start];
        let function_name = parse_fn_name(fn_line).unwrap_or_else(|| "<unknown>".to_string());

        // Find the `->` that begins the return type. The signature can
        // span multiple lines; concatenate from sig_start until we find
        // the opening brace of the body.
        let mut sig_chars = String::new();
        for line in &lines[sig_start..] {
            sig_chars.push_str(line);
            sig_chars.push('\n');
            if line.contains('{') {
                break;
            }
        }
        let Some(arrow_idx) = sig_chars.find("->") else {
            // Function returns ().
            continue;
        };
        // Slice from after `->` to the `{` of the body.
        let after_arrow = &sig_chars[arrow_idx + 2..];
        let Some(brace_idx) = after_arrow.find('{') else {
            continue;
        };
        let return_type = after_arrow[..brace_idx].trim();

        if !is_allowed_return_type(return_type) {
            offenses.push(Offense {
                file: path.to_path_buf(),
                line: sig_start + 1,
                function: function_name,
                return_type: return_type.to_string(),
            });
        }
    }

    offenses
}

#[test]
fn every_tauri_command_returns_app_error() {
    // Files we don't scan:
    //   - the test files themselves (false positives via included literal).
    //   - the standalone MCP binary (`computer_control_mcp/src/main.rs`)
    //     doesn't talk to the frontend; it speaks JSON-RPC over stdio
    //     and has no Tauri commands.
    let skip: HashSet<&str> = ["computer_control_mcp/src/main.rs"].into_iter().collect();

    let mut all_offenses = Vec::new();
    for path in rust_source_files() {
        let rel = path
            .strip_prefix(repo_root())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if skip.contains(rel.as_str()) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        // Cheap pre-filter — avoid parsing files that have no commands.
        if !content.contains("#[tauri::command]") {
            continue;
        }
        all_offenses.extend(scan_file(&path, &content));
    }

    if !all_offenses.is_empty() {
        let mut msg = String::from(
            "These #[tauri::command] functions don't return Result<_, AppError>.\n\
             They MUST be migrated — frontend errMessage(e) and errKind(e)\n\
             helpers depend on the AppError shape ({ kind, message }):\n\n",
        );
        for o in &all_offenses {
            let rel = o
                .file
                .strip_prefix(repo_root())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| o.file.to_string_lossy().to_string());
            msg.push_str(&format!(
                "  {}:{} — fn {}: returns `{}`\n",
                rel, o.line, o.function, o.return_type
            ));
        }
        panic!("{}", msg);
    }
}

#[cfg(test)]
mod parser_self_test {
    //! Sanity checks for the regex-free parser. Without these, a parser
    //! bug would let real offenders slip through silently.

    use super::*;

    #[test]
    fn detects_string_error_command() {
        let src = r#"
#[tauri::command]
pub async fn bad_one() -> Result<(), String> {
    Ok(())
}
"#;
        let offenses = scan_file(Path::new("test.rs"), src);
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].function, "bad_one");
    }

    #[test]
    fn allows_app_error_command() {
        let src = r#"
#[tauri::command]
pub async fn good_one() -> Result<String, AppError> {
    Ok("hello".to_string())
}
"#;
        assert!(scan_file(Path::new("test.rs"), src).is_empty());
    }

    #[test]
    fn allows_fully_qualified_app_error() {
        let src = r#"
#[tauri::command]
pub fn good_two() -> Result<(), crate::error::AppError> {
    Ok(())
}
"#;
        assert!(scan_file(Path::new("test.rs"), src).is_empty());
    }

    #[test]
    fn allows_non_result_returns() {
        // Some commands return `()` (e.g. fire-and-forget signals) or a
        // plain value. They don't need an error type at all.
        let src = r#"
#[tauri::command]
pub fn good_three(state: tauri::State<'_, S>) {
    state.do_thing();
}

#[tauri::command]
pub async fn good_four() -> bool {
    true
}
"#;
        assert!(scan_file(Path::new("test.rs"), src).is_empty());
    }

    #[test]
    fn handles_multi_line_signature() {
        // Commands with state often have their signature broken across
        // many lines for readability. The parser must concatenate them
        // before finding `->`.
        let src = r#"
#[tauri::command]
pub async fn good_five(
    a: String,
    b: u32,
) -> Result<(), AppError> {
    Ok(())
}
"#;
        assert!(scan_file(Path::new("test.rs"), src).is_empty());
    }

    #[test]
    fn handles_extra_attributes_between_command_and_fn() {
        // `#[allow(clippy::too_many_arguments)]` between
        // `#[tauri::command]` and the function signature is common —
        // it must not throw the parser off.
        let src = r#"
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn good_six(
    a: String,
) -> Result<(), AppError> {
    Ok(())
}
"#;
        assert!(scan_file(Path::new("test.rs"), src).is_empty());
    }

    #[test]
    fn rejects_other_error_types() {
        // anyhow::Error, io::Error, custom errors — all rejected. The
        // contract is specifically AppError because that's what the
        // frontend deserialises.
        let src = r#"
#[tauri::command]
pub fn bad_two() -> Result<(), anyhow::Error> {
    Ok(())
}
"#;
        let offenses = scan_file(Path::new("test.rs"), src);
        assert_eq!(offenses.len(), 1);
        assert!(offenses[0].return_type.contains("anyhow"));
    }

    #[test]
    fn nested_generics_in_ok_arm_are_fine() {
        // Result<HashMap<String, Vec<u32>>, AppError> — the parser must
        // not be confused by nested generics in the Ok arm.
        let src = r#"
#[tauri::command]
pub async fn good_seven() -> Result<std::collections::HashMap<String, Vec<u32>>, AppError> {
    Ok(Default::default())
}
"#;
        assert!(scan_file(Path::new("test.rs"), src).is_empty());
    }
}
