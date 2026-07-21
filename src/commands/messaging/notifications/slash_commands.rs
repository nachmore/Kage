/// Normalize standard-ACP `availableCommands` entries into our `SlashCommand`
/// shape. Standard ACP (Claude Code, etc.) sends
pub(super) fn parse_standard_acp_commands(
    cmds: &[serde_json::Value],
) -> Vec<crate::state::SlashCommand> {
    cmds.iter()
        .filter_map(|c| {
            let raw_name = c.get("name").and_then(|v| v.as_str())?;
            // Normalize to a leading-slash name. Kiro's vendor commands arrive
            // as "/agent"; standard ACP sends bare "context". The frontend
            // assumes the "/" prefix (it does `name.substring(1)`), so prepend
            // it here to keep one shape across agents.
            let name = if raw_name.starts_with('/') {
                raw_name.to_string()
            } else {
                format!("/{raw_name}")
            };
            let description = c
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input = c.get("input");
            let hint = input
                .and_then(|i| i.get("hint"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            // `input: null` → plain command; `input: { ... }` → takes text.
            let input_type = match input {
                Some(v) if !v.is_null() => Some("text".to_string()),
                _ => None,
            };
            let meta = if hint.is_some() || input_type.is_some() {
                Some(crate::state::SlashCommandMeta {
                    options_method: None,
                    input_type,
                    hint,
                    local: None,
                })
            } else {
                None
            };
            Some(crate::state::SlashCommand {
                name,
                description,
                meta,
                dispatch: "prompt".to_string(),
            })
        })
        .collect()
}

/// Set up the notification handler on the ACP client.
/// This should be called once after the client is created.

#[cfg(test)]
mod slash_discovery_tests {
    use super::parse_standard_acp_commands;

    #[test]
    fn normalizes_claude_available_commands() {
        // Trimmed copy of a real Claude Code available_commands_update.
        let cmds = serde_json::json!([
            { "name": "context", "description": "Show current context usage", "input": null },
            { "name": "compact", "description": "Clear history but keep a summary",
              "input": { "hint": "<optional custom summarization instructions>" } },
            { "name": "review", "description": "Review a pull request", "input": null }
        ]);
        let parsed = parse_standard_acp_commands(cmds.as_array().unwrap());
        assert_eq!(parsed.len(), 3);

        // Names get a leading slash so the frontend's substring(1) holds.
        assert_eq!(parsed[0].name, "/context");
        assert_eq!(parsed[1].name, "/compact");
        // All standard-ACP commands are prompt-dispatched.
        assert!(parsed.iter().all(|c| c.dispatch == "prompt"));

        // input:null → no meta (plain fire-and-run command).
        assert!(parsed[0].meta.is_none());
        // input:{hint} → meta carries the hint and marks free-text input.
        let compact_meta = parsed[1].meta.as_ref().unwrap();
        assert_eq!(compact_meta.input_type.as_deref(), Some("text"));
        assert_eq!(
            compact_meta.hint.as_deref(),
            Some("<optional custom summarization instructions>")
        );
    }

    #[test]
    fn already_slashed_names_are_left_alone() {
        let cmds = serde_json::json!([{ "name": "/foo", "description": "", "input": null }]);
        let parsed = parse_standard_acp_commands(cmds.as_array().unwrap());
        assert_eq!(parsed[0].name, "/foo");
    }

    #[test]
    fn entries_without_a_name_are_skipped() {
        let cmds = serde_json::json!([
            { "description": "no name here", "input": null },
            { "name": "ok", "description": "fine", "input": null }
        ]);
        let parsed = parse_standard_acp_commands(cmds.as_array().unwrap());
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "/ok");
    }

    #[test]
    fn vendor_commands_default_to_vendor_dispatch() {
        // The Kiro path deserializes via serde; dispatch must default to
        // "vendor" when the wire payload omits it.
        let cmd: crate::state::SlashCommand = serde_json::from_value(serde_json::json!({
            "name": "/agent", "description": "Select an agent"
        }))
        .unwrap();
        assert_eq!(cmd.dispatch, "vendor");
    }
}
