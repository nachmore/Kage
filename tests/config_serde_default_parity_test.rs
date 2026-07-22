//! Every field of every `Deserialize` struct under `src/config/` (and
//! `src/config.rs`) must carry `#[serde(default)]` (bare or with a
//! function). CLAUDE.md rule: old configs must keep loading after a
//! schema change. One field without a default makes the ENTIRE
//! `Config::load` deserialization fail for any config.json written
//! before that field existed — silently resetting the user's hotkeys,
//! permissions, and telemetry consent via the backup-and-reset path.
//!
//! This test parses the source files as text and asserts the rule, in
//! the same spirit as `command_error_type_parity_test.rs`.
//!
//! What counts as covered:
//!   - `#[serde(default)]` / `#[serde(default = "...")]` on the field
//!   - a struct-level `#[serde(default)]` (covers every field)
//!   - `#[serde(skip)]` / `#[serde(skip_deserializing)]` (never read
//!     from the wire)
//!   - `#[serde(flatten)]` — delegates to the inner type, whose own
//!     fields this test checks separately
//!
//! Escape hatch: a field that legitimately MUST be required (where a
//! silent default would be wrong) can carry a marker comment on the
//! line directly above its attributes/doc:
//!
//!     // serde-default-exempt: <reason>
//!     pub some_field: String,
//!
//! Use it sparingly — a required field means old configs fail to load.
//! Enums are out of scope (they deserialize whole; `#[serde(other)]`
//! fallbacks are a separate, enum-specific concern).

use std::fs;
use std::path::{Path, PathBuf};

const EXEMPT_MARKER: &str = "serde-default-exempt:";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The config source files under enforcement.
fn config_source_files() -> Vec<PathBuf> {
    let root = repo_root();
    let mut out = vec![root.join("src/config.rs")];
    if let Ok(entries) = fs::read_dir(root.join("src/config")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    out
}

#[derive(Debug)]
struct Offense {
    file: PathBuf,
    line: usize,
    struct_name: String,
    field: String,
}

/// True if the attribute line marks the field (or struct) as covered.
fn attr_covers(attr: &str) -> bool {
    let a: String = attr.split_whitespace().collect();
    a.contains("serde(default")
        || a.contains("serde(skip)")
        || a.contains("serde(skip_deserializing")
        || a.contains("serde(flatten")
}

/// Parse `pub name: Type,` → `name`. Returns None for non-field lines.
fn parse_field_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    // Only public named fields exist in the config structs; a private
    // field would still be a wire field, so match both.
    let rest = trimmed.strip_prefix("pub ").unwrap_or(trimmed);
    let colon = rest.find(':')?;
    let name = rest[..colon].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(name.to_string())
}

/// Scan one file for Deserialize structs whose fields lack defaults.
fn scan_file(path: &Path, content: &str) -> Vec<Offense> {
    let mut offenses = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim_start();

        // Find a derive containing Deserialize; remember any struct-level
        // serde attributes between the derive and the `struct` keyword.
        if !(trimmed.starts_with("#[derive(") && trimmed.contains("Deserialize")) {
            i += 1;
            continue;
        }

        let mut struct_level_default = false;
        let mut j = i + 1;
        while j < lines.len() {
            let t = lines[j].trim_start();
            if t.starts_with("#[") {
                if attr_covers(t) {
                    struct_level_default = true;
                }
                j += 1;
                continue;
            }
            break;
        }
        // Only structs with named fields are in scope. Enums deserialize
        // whole; tuple/unit structs have no per-field attrs.
        let Some(decl) = lines.get(j) else { break };
        let decl_trim = decl.trim_start();
        let is_struct = decl_trim.starts_with("pub struct ") || decl_trim.starts_with("struct ");
        let has_body = decl_trim.contains('{') || lines.get(j + 1).is_some_and(|l| l.trim() == "{");
        if !is_struct || !has_body {
            i = j + 1;
            continue;
        }
        let struct_name = decl_trim
            .trim_start_matches("pub ")
            .trim_start_matches("struct ")
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<String>();

        if struct_level_default {
            // Every field covered — skip to the closing brace.
            i = j + 1;
            continue;
        }

        // Walk the struct body: track per-field attribute state.
        let mut field_covered = false;
        let mut field_exempt = false;
        let mut depth = 0usize;
        let mut k = j;
        while k < lines.len() {
            let line = lines[k];
            depth += line.matches('{').count();
            let closing = line.matches('}').count();
            if closing >= depth && k > j {
                break; // end of struct body
            }
            depth -= closing;

            let t = line.trim_start();
            if t.starts_with("//") {
                if t.contains(EXEMPT_MARKER) {
                    field_exempt = true;
                }
                k += 1;
                continue;
            }
            if t.starts_with("#[") {
                if attr_covers(t) {
                    field_covered = true;
                }
                k += 1;
                continue;
            }
            if depth == 1 {
                if let Some(field) = parse_field_name(line) {
                    if !field_covered && !field_exempt {
                        offenses.push(Offense {
                            file: path.to_path_buf(),
                            line: k + 1,
                            struct_name: struct_name.clone(),
                            field,
                        });
                    }
                    field_covered = false;
                    field_exempt = false;
                }
            }
            k += 1;
        }
        i = k + 1;
    }

    offenses
}

#[test]
fn every_config_field_has_serde_default() {
    let mut all_offenses = Vec::new();
    for path in config_source_files() {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        all_offenses.extend(scan_file(&path, &content));
    }

    if !all_offenses.is_empty() {
        let mut msg = String::from(
            "These config struct fields lack #[serde(default)]. An old\n\
             config.json missing any of them fails the WHOLE Config::load\n\
             and silently resets the user to defaults (CLAUDE.md rule).\n\
             Add #[serde(default)] (+ a Default impl / default fn), or —\n\
             only if a silent default would be genuinely wrong — put\n\
             `// serde-default-exempt: <reason>` above the field:\n\n",
        );
        for o in &all_offenses {
            let rel = o
                .file
                .strip_prefix(repo_root())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| o.file.to_string_lossy().to_string());
            msg.push_str(&format!(
                "  {}:{} — {}.{}\n",
                rel, o.line, o.struct_name, o.field
            ));
        }
        panic!("{}", msg);
    }
}

#[cfg(test)]
mod parser_self_test {
    //! Sanity checks — a parser bug must not let real offenders slip
    //! through silently.

    use super::*;

    #[test]
    fn detects_missing_default() {
        let src = r#"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bad {
    pub name: String,
}
"#;
        let offenses = scan_file(Path::new("test.rs"), src);
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].struct_name, "Bad");
        assert_eq!(offenses[0].field, "name");
    }

    #[test]
    fn allows_field_default() {
        let src = r#"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Good {
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}
"#;
        assert!(scan_file(Path::new("test.rs"), src).is_empty());
    }

    #[test]
    fn allows_struct_level_default() {
        let src = r#"
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Good {
    pub name: String,
    pub count: u32,
}
"#;
        assert!(scan_file(Path::new("test.rs"), src).is_empty());
    }

    #[test]
    fn allows_exempt_marker() {
        let src = r#"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mixed {
    // serde-default-exempt: this id must come from the wire
    pub id: String,
    #[serde(default)]
    pub name: String,
}
"#;
        assert!(scan_file(Path::new("test.rs"), src).is_empty());
    }

    #[test]
    fn skips_enums() {
        let src = r#"
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Beta,
    Dev,
    #[serde(other)]
    Stable,
}
"#;
        assert!(scan_file(Path::new("test.rs"), src).is_empty());
    }

    #[test]
    fn skips_serialize_only_structs() {
        let src = r#"
#[derive(Debug, Clone, Serialize)]
pub struct WireOut {
    pub name: String,
}
"#;
        assert!(scan_file(Path::new("test.rs"), src).is_empty());
    }

    #[test]
    fn doc_comments_do_not_reset_attr_state() {
        let src = r#"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Good {
    /// Documented field.
    #[serde(default)]
    pub name: String,
}
"#;
        assert!(scan_file(Path::new("test.rs"), src).is_empty());
    }

    #[test]
    fn detects_second_field_missing_after_first_covered() {
        // Per-field attribute state must reset between fields.
        let src = r#"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bad {
    #[serde(default)]
    pub covered: String,
    pub uncovered: String,
}
"#;
        let offenses = scan_file(Path::new("test.rs"), src);
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].field, "uncovered");
    }

    #[test]
    fn nested_enum_variant_fields_are_not_struct_fields() {
        // AcpMode-style tagged enums have named fields inside variant
        // blocks — those are depth 2, not struct fields, and enums are
        // skipped anyway.
        let src = r#"
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AcpMode {
    Local {
        spawn_command: String,
    },
}
"#;
        assert!(scan_file(Path::new("test.rs"), src).is_empty());
    }
}
