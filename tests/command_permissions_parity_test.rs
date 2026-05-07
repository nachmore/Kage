//! Parity check: every Tauri command registered in `generate_handler!` must
//! appear in `ui/js/shared/extension-permissions.js`'s `COMMAND_CAPABILITIES`
//! map — either with a capability name (callable from extensions if granted)
//! or with `null` (never callable from extensions).
//!
//! Today the sandbox host fails closed for unknown commands, so a missing
//! entry is *safe* — but it's not *intentional*. This test forces the
//! decision to be made (and reviewed) at the time the command is added.
//!
//! Catches:
//!   1. New `#[tauri::command]` added + wired into `generate_handler!` but
//!      forgotten in `extension-permissions.js`. Without this, a future
//!      refactor that loosens the default would silently expose the new
//!      command.
//!   2. A command removed from `generate_handler!` whose entry was left
//!      behind in the JS map (cosmetic but a parity violation).
//!
//! The test reads both files as text and parses with simple regex — no
//! macro expansion or JS evaluation needed.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Pull every `commands::name,` (and `crate::automation::name,`) out of the
/// `tauri::generate_handler![...]` call in `src/main.rs`.
fn registered_commands() -> HashSet<String> {
    let path = repo_root().join("src").join("main.rs");
    let src = fs::read_to_string(&path).expect("read src/main.rs");

    let start = src
        .find("tauri::generate_handler![")
        .expect("generate_handler! not found");
    let after_open = start + "tauri::generate_handler![".len();
    // Walk forward to the matching closing bracket. Cheap and good enough —
    // there are no nested `[` inside the macro arg list in this codebase.
    let close = src[after_open..]
        .find(']')
        .expect("closing ] for generate_handler! not found");
    let body = &src[after_open..after_open + close];

    let mut out = HashSet::new();
    for line in body.lines() {
        let line = line.trim();
        // Skip blanks and rust line comments.
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        // Strip trailing comma + optional comment.
        let token = line.trim_end_matches(',').trim();
        // Last `::` segment is the command name.
        let name = match token.rsplit_once("::") {
            Some((_, rest)) => rest.trim(),
            None => token,
        };
        // Defensively reject anything that isn't an identifier (catches
        // stray comments or future syntax).
        if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.is_empty() {
            out.insert(name.to_string());
        }
    }
    out
}

/// Pull every key out of the `COMMAND_CAPABILITIES = Object.freeze({...})`
/// block in `ui/js/shared/extension-permissions.js`. The file uses
/// unquoted JS identifier keys, so we match `\bname:` lines inside that
/// block.
fn permissioned_commands() -> HashSet<String> {
    let path = repo_root()
        .join("ui")
        .join("js")
        .join("shared")
        .join("extension-permissions.js");
    let src = fs::read_to_string(&path).expect("read extension-permissions.js");

    let marker = "COMMAND_CAPABILITIES = Object.freeze({";
    let start = src.find(marker).expect("COMMAND_CAPABILITIES marker not found");
    let after_open = start + marker.len();
    // Walk forward counting brace depth. The block contains nested literal
    // `{` only inside string descriptions — there are none in this map —
    // so a simple counter is fine.
    let mut depth: i32 = 1;
    let bytes = src[after_open..].as_bytes();
    let mut end = 0;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(end > 0, "could not find end of COMMAND_CAPABILITIES object");
    let body = &src[after_open..after_open + end];

    let mut out = HashSet::new();
    for raw_line in body.lines() {
        // Strip line comments that follow the entry.
        let line = match raw_line.find("//") {
            Some(i) => &raw_line[..i],
            None => raw_line,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Match `name: value,` — the name is everything before the first ':'.
        let Some(colon) = line.find(':') else {
            continue;
        };
        let name = line[..colon].trim();
        // Reject string-quoted keys (none in this file today, but be safe).
        let name = name.trim_matches(|c| c == '\'' || c == '"');
        if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.is_empty() {
            out.insert(name.to_string());
        }
    }
    out
}

#[test]
fn every_registered_command_has_a_permission_entry() {
    let registered = registered_commands();
    let permissioned = permissioned_commands();

    assert!(
        !registered.is_empty(),
        "regex/parser couldn't find any registered commands — parser is broken"
    );
    assert!(
        !permissioned.is_empty(),
        "regex/parser couldn't find any permission entries — parser is broken"
    );

    let mut missing: Vec<&String> = registered.difference(&permissioned).collect();
    missing.sort();
    assert!(
        missing.is_empty(),
        "\n\
         Tauri commands registered in generate_handler! but missing from \n\
         ui/js/shared/extension-permissions.js::COMMAND_CAPABILITIES:\n\n  - {}\n\n\
         Add each one with either a capability name (extension-callable) or \n\
         `null` (never callable from extensions). The sandbox host fails \n\
         closed today, but the explicit decision is required so a future \n\
         refactor doesn't silently expose new commands.\n",
        missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  - "),
    );
}

#[test]
fn no_stale_permission_entries_for_removed_commands() {
    let registered = registered_commands();
    let permissioned = permissioned_commands();

    let mut stale: Vec<&String> = permissioned.difference(&registered).collect();
    stale.sort();
    assert!(
        stale.is_empty(),
        "\n\
         Entries in ui/js/shared/extension-permissions.js::COMMAND_CAPABILITIES \n\
         that don't correspond to any registered Tauri command:\n\n  - {}\n\n\
         Either the command was removed (drop the JS entry) or the parser \n\
         is missing it (extend tests/command_permissions_parity_test.rs).\n",
        stale.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  - "),
    );
}

// ---- Parser sanity tests ---------------------------------------------------
//
// The parsers are simple but the cost of getting them wrong is silent
// false-passes, so pin a few invariants on real fixtures.

#[test]
fn registered_commands_includes_known_anchors() {
    // These three commands exist and are obviously registered. If the
    // parser stops finding them, it's broken and the parity assertion
    // above can't be trusted.
    let registered = registered_commands();
    for anchor in ["save_config", "send_message_streaming", "emit_automation_signal"] {
        assert!(
            registered.contains(anchor),
            "parser missed expected command: {anchor}"
        );
    }
}

#[test]
fn permissioned_commands_includes_known_anchors() {
    // Same idea for the JS map.
    let permissioned = permissioned_commands();
    for anchor in ["save_config", "save_extension_data", "execute_system_command"] {
        assert!(
            permissioned.contains(anchor),
            "parser missed expected permission entry: {anchor}"
        );
    }
}
