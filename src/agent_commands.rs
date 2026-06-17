//! Per-agent built-in slash-command catalogs.
//!
//! Some agents advertise far fewer slash commands over ACP than they actually
//! support. Claude Code's ACP adapter, for example, advertises ~8
//! (`available_commands_update`) but the CLI supports ~90
//! (https://code.claude.com/docs/en/commands). This module supplies a curated
//! hardcoded catalog per agent that we MERGE with whatever the agent
//! advertised at runtime — advertised always wins, so live data can't be
//! shadowed by a stale constant, and the hardcoded list only fills gaps.
//!
//! Mirrors the `slash_format` per-agent shape: dispatch by `AgentKind`, one
//! catalog function per agent, pure and unit-testable. Adding an agent is one
//! match arm + one list.
//!
//! Everything here is `dispatch = "prompt"`: these are standard-ACP agents
//! whose commands run by sending the slash text as a normal prompt (see
//! `commands::messaging`). Kiro is intentionally absent — it advertises its
//! full set over the vendor extension, so there's nothing to augment.

use crate::agent_presets::AgentKind;
use crate::state::{SlashCommand, SlashCommandMeta};

/// Built-in command catalog for an agent, or empty when we have nothing to add
/// beyond what the agent advertises.
pub fn builtin_commands(kind: AgentKind) -> Vec<SlashCommand> {
    match kind {
        AgentKind::ClaudeCode => claude_commands(),
        // Kiro advertises everything via the vendor extension; Codex/Ollama
        // have no curated catalog yet.
        AgentKind::Kiro | AgentKind::Codex | AgentKind::Ollama => Vec::new(),
    }
}

/// Merge advertised commands with a built-in catalog. Advertised entries win
/// on name collision (live data is authoritative); built-ins only fill gaps.
/// Order: advertised first (preserving the agent's order), then the built-ins
/// that weren't already present.
pub fn merge_commands(
    advertised: Vec<SlashCommand>,
    builtin: Vec<SlashCommand>,
) -> Vec<SlashCommand> {
    let mut seen: std::collections::HashSet<String> =
        advertised.iter().map(|c| c.name.clone()).collect();
    let mut out = advertised;
    for cmd in builtin {
        if seen.insert(cmd.name.clone()) {
            out.push(cmd);
        }
    }
    out
}

/// Build a prompt-dispatch command with an optional input hint.
fn cmd(name: &str, description: &str, hint: Option<&str>) -> SlashCommand {
    let meta = hint.map(|h| SlashCommandMeta {
        options_method: None,
        input_type: Some("text".to_string()),
        hint: Some(h.to_string()),
        local: None,
    });
    SlashCommand {
        name: format!("/{name}"),
        description: description.to_string(),
        meta,
        dispatch: "prompt".to_string(),
    }
}

/// Curated subset of Claude Code's built-in commands that make sense in Kage's
/// chat/floating UX. Deliberately excludes terminal-only / environment-specific
/// commands (`/vim`, `/terminal-setup`, `/tui`, `/scroll-speed`, `/focus`,
/// `/keybindings`, `/statusline`, …) and things Kage owns itself (theme,
/// config). Descriptions condensed from https://code.claude.com/docs/en/commands.
fn claude_commands() -> Vec<SlashCommand> {
    vec![
        cmd("context", "Visualize current context usage", Some("[all]")),
        cmd(
            "compact",
            "Summarize the conversation to free up context",
            Some("[instructions]"),
        ),
        cmd("clear", "Start a new conversation with empty context", None),
        cmd("model", "Switch the AI model", Some("[model]")),
        cmd("effort", "Set the model effort level", Some("[level|auto]")),
        cmd("review", "Review a pull request locally", Some("[PR]")),
        cmd(
            "security-review",
            "Analyze pending changes for security vulnerabilities",
            None,
        ),
        cmd(
            "code-review",
            "Review the current diff for bugs and cleanups",
            Some("[--fix] [target]"),
        ),
        cmd(
            "simplify",
            "Review changed code for cleanup opportunities and apply fixes",
            Some("[target]"),
        ),
        cmd("init", "Initialize a CLAUDE.md guide for the project", None),
        cmd("memory", "Edit CLAUDE.md memory files", None),
        cmd("agents", "Manage subagent configurations", None),
        cmd("mcp", "Manage MCP server connections", None),
        cmd("usage", "Show session cost, usage limits, and stats", None),
        cmd("plan", "Enter plan mode", Some("[description]")),
        cmd(
            "rewind",
            "Rewind the conversation and/or code to an earlier point",
            None,
        ),
        cmd(
            "resume",
            "Resume a conversation by id or name",
            Some("[session]"),
        ),
        cmd("rename", "Rename the current session", Some("[name]")),
        cmd(
            "export",
            "Export the current conversation",
            Some("[filename]"),
        ),
        cmd(
            "copy",
            "Copy the last assistant response to clipboard",
            Some("[N]"),
        ),
        cmd("diff", "Open an interactive diff viewer", None),
        cmd("hooks", "View hook configurations for tool events", None),
        cmd("permissions", "Manage tool permission rules", None),
        cmd("skills", "List available skills", None),
        cmd("doctor", "Diagnose and verify the installation", None),
        cmd(
            "insights",
            "Generate a report analyzing your sessions",
            None,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(name: &str) -> SlashCommand {
        SlashCommand {
            name: name.to_string(),
            description: "advertised".to_string(),
            meta: None,
            dispatch: "prompt".to_string(),
        }
    }

    #[test]
    fn claude_has_a_catalog_others_dont() {
        assert!(!builtin_commands(AgentKind::ClaudeCode).is_empty());
        assert!(builtin_commands(AgentKind::Kiro).is_empty());
        assert!(builtin_commands(AgentKind::Codex).is_empty());
    }

    #[test]
    fn catalog_names_are_slash_prefixed_and_prompt_dispatch() {
        for c in claude_commands() {
            assert!(c.name.starts_with('/'), "{} missing slash", c.name);
            assert_eq!(c.dispatch, "prompt");
        }
    }

    #[test]
    fn merge_keeps_advertised_on_collision() {
        let advertised = vec![sc("/context"), sc("/compact")];
        let merged = merge_commands(advertised, claude_commands());
        // The advertised /context wins — its description is "advertised".
        let ctx = merged.iter().find(|c| c.name == "/context").unwrap();
        assert_eq!(ctx.description, "advertised");
        // Exactly one /context (no dup).
        assert_eq!(merged.iter().filter(|c| c.name == "/context").count(), 1);
    }

    #[test]
    fn merge_appends_builtins_not_advertised() {
        let advertised = vec![sc("/context")];
        let merged = merge_commands(advertised, claude_commands());
        // A built-in the agent didn't advertise is present.
        assert!(merged.iter().any(|c| c.name == "/security-review"));
        // Advertised stays first.
        assert_eq!(merged[0].name, "/context");
    }

    #[test]
    fn merge_with_empty_builtin_is_identity() {
        let advertised = vec![sc("/foo"), sc("/bar")];
        let merged = merge_commands(advertised.clone(), Vec::new());
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "/foo");
    }
}
