//! Line-oriented IO for the steering documents.
//!
//! The Personalization settings page surfaces both the auto-generated
//! steering doc and the user steering doc as a list of editable
//! lines. This module is the shim between that UI and the on-disk
//! markdown files:
//!
//!   - `read_lines(kind)` — open the file, strip framing the editor
//!     shouldn't see (the auto-doc's HTML header), and return lines.
//!   - `write_lines(kind, lines)` — re-frame and write back. Auto kind
//!     re-prepends the canonical header; user kind writes verbatim.
//!   - `import_lines_from_path(path)` — read an arbitrary file the
//!     user picked via the dialog, return its lines without touching
//!     the configured doc. The settings UI then merges + saves.
//!
//! The Tauri command wrappers live in `commands::system`. This module
//! is pure (config in, lines out) so the splitting / framing logic
//! is unit-testable without spinning up the full app.
//!
//! User-steering default path: when `config.acp.agent.user_steering_path`
//! is `None`, we default to `<config_dir>/kage/user-steering.md`. The
//! command wrapper persists that default into config the first time
//! the user saves a non-empty document — see
//! `commands::system::write_steering_lines`.
//!
//! Why split-and-rejoin rather than ship raw text: the editor reorders
//! lines via UI buttons. Sending the whole document as a string and
//! diffing is wasteful when the canonical representation IS a list.
//! Splitting on `\n` (no `\r\n` quirks — we normalise) round-trips
//! losslessly for the cases the user can produce in the editor.

use crate::auto_steering::auto_steering_header;
use crate::config::Config;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Which steering document we're operating on. The two have different
/// framing rules — auto carries an HTML header that the editor
/// shouldn't see / mutate, user is plain markdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteeringKind {
    /// `<config_dir>/kage/auto-steering.md`. Carries the header
    /// comment that warns "this file may be regenerated."
    Auto,
    /// `<config_dir>/kage/user-steering.md` by default, or whatever
    /// path the user pointed at via `user_steering_path` config.
    User,
}

impl SteeringKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

/// Default location for the user-written steering doc when the user
/// hasn't explicitly chosen a path. Living next to the auto doc keeps
/// both files together and survives portable installs that share a
/// config dir.
pub fn default_user_steering_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("Failed to get config directory")?;
    Ok(config_dir.join("kage").join("user-steering.md"))
}

/// Resolve the on-disk path for a steering kind, given current config.
/// Returns the chosen path so callers can show it in the UI / persist
/// it after a save.
pub fn resolve_path(kind: SteeringKind, config: &Config) -> Result<PathBuf> {
    match kind {
        SteeringKind::Auto => Config::get_auto_steering_path(),
        SteeringKind::User => match config.acp.agent.user_steering_path.as_deref() {
            Some(p) if !p.trim().is_empty() => Ok(PathBuf::from(p)),
            _ => default_user_steering_path(),
        },
    }
}

/// Strip the auto-steering HTML header (`<!-- … -->`) from the front
/// of a body if present. Returns the remainder. Used when reading
/// the auto doc into the editor — the user shouldn't see / be able
/// to edit the framing comment.
pub fn strip_auto_header(text: &str) -> &str {
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<!--") {
        if let Some(end) = rest.find("-->") {
            // +3 to consume the closing `-->` itself.
            return rest[end + 3..].trim_start_matches(['\n', '\r']);
        }
    }
    text
}

/// Split a body into editor-friendly lines. We normalise CRLF first
/// so a doc edited on Windows + saved from the editor on macOS
/// doesn't grow `\r` remnants on every save. Trailing empty lines
/// are dropped — the editor adds them back implicitly via the join.
pub fn split_lines(body: &str) -> Vec<String> {
    let normalised = body.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = normalised.split('\n').map(str::to_string).collect();
    while lines.last().map(|s| s.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines
}

/// Join lines back into a body the editor produced. Always trailing
/// newline so external tools (git, less) treat the file as
/// well-formed. Empty input → empty body (no spurious newline).
pub fn join_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Read a steering doc into `(path, lines)`. Missing files are not an
/// error — they round-trip as an empty line list, which the editor
/// shows as an empty document the user can populate. The caller
/// gets the resolved path so it can show it in the UI.
pub fn read_lines(kind: SteeringKind, config: &Config) -> Result<(PathBuf, Vec<String>)> {
    let path = resolve_path(kind, config)?;
    if !path.exists() {
        return Ok((path, Vec::new()));
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read steering doc at {:?}", path))?;
    let body = match kind {
        SteeringKind::Auto => strip_auto_header(&raw).to_string(),
        SteeringKind::User => raw,
    };
    Ok((path, split_lines(&body)))
}

/// Write lines back to the canonical path for `kind`. Auto re-prepends
/// the header comment so the file remains self-describing for users
/// who open it in an editor. Creates the parent directory if needed.
/// Returns the path actually written so the caller can persist it
/// (e.g. into `user_steering_path` for first-time user saves).
pub fn write_lines(kind: SteeringKind, config: &Config, lines: &[String]) -> Result<PathBuf> {
    let path = resolve_path(kind, config)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory for {:?}", parent))?;
    }

    let body = join_lines(lines);
    let to_write = match kind {
        SteeringKind::Auto => {
            let body = body.trim_start_matches('\n');
            format!("{}{}", auto_steering_header(), body)
        }
        SteeringKind::User => body,
    };
    std::fs::write(&path, to_write)
        .with_context(|| format!("Failed to write steering doc at {:?}", path))?;
    Ok(path)
}

/// Read an arbitrary file the user picked via the file dialog, strip
/// any auto-doc header that might be present (so importing your
/// previous auto doc doesn't double up the comment), and return
/// lines. Caller writes via `write_lines` after merging.
pub fn import_lines_from_path(path: &str) -> Result<Vec<String>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read import file at {}", path))?;
    let body = strip_auto_header(&raw);
    Ok(split_lines(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_auto_header_removes_html_comment() {
        let raw = "<!-- AUTO-GENERATED -->\n## Heading\n- Bullet";
        assert_eq!(strip_auto_header(raw), "## Heading\n- Bullet");
    }

    #[test]
    fn strip_auto_header_handles_leading_whitespace() {
        let raw = "\n\n  <!-- AUTO -->\n## Heading";
        assert_eq!(strip_auto_header(raw), "## Heading");
    }

    #[test]
    fn strip_auto_header_passes_through_when_absent() {
        let raw = "## Heading\nbody";
        assert_eq!(strip_auto_header(raw), "## Heading\nbody");
    }

    #[test]
    fn strip_auto_header_with_unterminated_comment_is_left_alone() {
        // Defensive: we don't want to silently eat half a document if
        // the file is corrupted somehow.
        let raw = "<!-- starts but never closes\n## Heading";
        assert_eq!(strip_auto_header(raw), raw);
    }

    #[test]
    fn split_lines_normalises_crlf() {
        let body = "one\r\ntwo\rthree\nfour";
        assert_eq!(split_lines(body), vec!["one", "two", "three", "four"]);
    }

    #[test]
    fn split_lines_drops_trailing_blanks() {
        let body = "alpha\nbeta\n\n\n";
        assert_eq!(split_lines(body), vec!["alpha", "beta"]);
    }

    #[test]
    fn split_lines_keeps_interior_blanks_for_paragraph_breaks() {
        let body = "alpha\n\nbeta";
        assert_eq!(split_lines(body), vec!["alpha", "", "beta"]);
    }

    #[test]
    fn split_lines_empty_input_yields_empty_vec() {
        assert!(split_lines("").is_empty());
        assert!(split_lines("\n\n").is_empty());
    }

    #[test]
    fn join_lines_terminates_with_newline() {
        let lines = vec!["a".to_string(), "b".to_string()];
        assert_eq!(join_lines(&lines), "a\nb\n");
    }

    #[test]
    fn join_lines_empty_yields_empty_string() {
        assert_eq!(join_lines(&[]), "");
    }

    #[test]
    fn round_trip_preserves_interior_blanks() {
        let body = "## Section\n\n- item one\n- item two\n";
        let lines = split_lines(strip_auto_header(body));
        let rejoined = join_lines(&lines);
        // join always terminates with a single newline; original had
        // one too — round-trip equality holds.
        assert_eq!(rejoined, body);
    }

    #[test]
    fn round_trip_strips_auto_header_then_writes_with_one_back() {
        // What happens when the editor reads a fresh auto doc, the
        // user hits Save with no changes: the on-disk content
        // shouldn't drift.
        let raw = format!("{}## A\n- one\n- two\n", auto_steering_header());
        let body_no_header = strip_auto_header(&raw);
        let lines = split_lines(body_no_header);

        let body = join_lines(&lines);
        let trimmed = body.trim_start_matches('\n');
        let rewritten = format!("{}{}", auto_steering_header(), trimmed);
        assert_eq!(rewritten, raw);
    }

    #[test]
    fn parse_kind_round_trip() {
        assert_eq!(SteeringKind::parse("auto"), Some(SteeringKind::Auto));
        assert_eq!(SteeringKind::parse("user"), Some(SteeringKind::User));
        assert_eq!(SteeringKind::parse(""), None);
        assert_eq!(SteeringKind::parse("garbage"), None);
    }
}
