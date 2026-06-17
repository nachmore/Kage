//! Per-agent slash-command result formatters.
//!
//! Different ACP agents return slash-command results in different shapes,
//! and at different richness. Kiro CLI's `/context`, for example, returns a
//! one-line `message` ("Context breakdown - 3% used") plus a detailed
//! structured `data.breakdown` that the bare message throws away. This layer
//! turns that structured data into nicely-rendered markdown.
//!
//! The shape mirrors `agent_sessions`: a `SlashFormatter` trait, one impl per
//! agent, and a registry that dispatches by `AgentKind`. Adding an agent is a
//! single impl + a single registry arm. Adding a command to an existing agent
//! is one match arm in that agent's `format`.
//!
//! **Formatters emit markdown, never HTML.** The frontend renders the string
//! through the hardened `marked` pipeline (which escapes raw HTML), so a
//! formatter that emitted `<div>` would have it shown literally. GFM tables,
//! headings, and Unicode bars are the palette.
//!
//! **Formatters are pure and shape-defensive.** `format` takes the raw JSON
//! result and returns `Some(markdown)` only when it recognises the shape;
//! `None` means "fall back to the agent's own `message`". A formatter that
//! can't find the data it expects returns `None` rather than rendering
//! garbage — so a contract change on the agent side degrades to the plain
//! message instead of breaking.
//!
//! Current coverage (verified via `scripts/probe_slash.py`):
//!   - Kiro: `/context`. Other Kiro commands fall through to their message.
//!   - Claude/Codex: no-op. Claude Code ACP (v0.16.2) doesn't expose the
//!     `commands/*` vendor extension at all (no `commands/available`, and
//!     `commands/execute` returns -32601), so there's nothing to format yet.
//!     Codex isn't wired up locally. Both are TODO stubs that fall back.

mod kiro;

use crate::agent_presets::AgentKind;

/// One agent's slash-result formatter. Pure: no Tauri, no I/O, no state — just
/// `(command, raw result) -> optional markdown`. This keeps it unit-testable
/// against captured fixtures with zero harness.
pub trait SlashFormatter: Send + Sync {
    /// Format the result of `command` (bare name, no leading `/`). Return
    /// `Some(markdown)` to override the agent's plain message, or `None` to
    /// fall back to it.
    fn format(&self, command: &str, result: &serde_json::Value) -> Option<String>;
}

/// Resolve the formatter for an agent kind. Returns `None` for agents with no
/// formatter (Claude/Codex today) so the caller falls back to the raw message.
pub fn formatter_for(kind: AgentKind) -> Option<&'static dyn SlashFormatter> {
    match kind {
        AgentKind::Kiro => Some(&kiro::KiroFormatter),
        // Claude Code ACP doesn't surface the commands/* extension yet, and
        // Codex isn't wired up locally. Add arms here when they do.
        AgentKind::ClaudeCode | AgentKind::Codex | AgentKind::Ollama => None,
    }
}

/// Format a slash-command result for the given agent, falling back to the
/// raw `message` field (then to `None`) when no formatter applies. This is the
/// single entry point the command handler calls.
pub fn format_slash_result(
    kind: AgentKind,
    command: &str,
    result: &serde_json::Value,
) -> Option<String> {
    formatter_for(kind).and_then(|f| f.format(command, result))
}

// --- Shared rendering helpers, used by the per-agent impls ---

/// Render a horizontal usage bar out of filled/empty block glyphs.
/// `pct` is 0..=100. Width is the number of cells.
pub(crate) fn usage_bar(pct: f64, width: usize) -> String {
    let clamped = pct.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let mut s = String::with_capacity(width * 3);
    for _ in 0..filled {
        s.push('█');
    }
    for _ in 0..(width - filled) {
        s.push('░');
    }
    s
}

/// Format a token count compactly: 1234 -> "1.2k", 58 -> "58", 1_000_000 -> "1.0m".
pub(crate) fn fmt_tokens(n: f64) -> String {
    let n = n.max(0.0);
    if n >= 1_000_000.0 {
        format!("{:.1}m", n / 1_000_000.0)
    } else if n >= 1_000.0 {
        format!("{:.1}k", n / 1_000.0)
    } else {
        format!("{}", n.round() as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_bar_endpoints() {
        assert_eq!(usage_bar(0.0, 10), "░░░░░░░░░░");
        assert_eq!(usage_bar(100.0, 10), "██████████");
        assert_eq!(usage_bar(50.0, 10), "█████░░░░░");
        // Clamps out-of-range input.
        assert_eq!(usage_bar(250.0, 4), "████");
        assert_eq!(usage_bar(-5.0, 4), "░░░░");
    }

    #[test]
    fn fmt_tokens_scales() {
        assert_eq!(fmt_tokens(58.0), "58");
        assert_eq!(fmt_tokens(1234.0), "1.2k");
        assert_eq!(fmt_tokens(241_700.0), "241.7k");
        assert_eq!(fmt_tokens(1_000_000.0), "1.0m");
    }

    #[test]
    fn claude_and_codex_have_no_formatter() {
        assert!(formatter_for(AgentKind::ClaudeCode).is_none());
        assert!(formatter_for(AgentKind::Codex).is_none());
        assert!(formatter_for(AgentKind::Kiro).is_some());
    }

    #[test]
    fn format_falls_back_to_none_for_unformatted_agent() {
        let v = serde_json::json!({ "message": "hi", "data": {} });
        assert_eq!(
            format_slash_result(AgentKind::ClaudeCode, "context", &v),
            None
        );
    }
}
