/// Split a spawn command into program + args, honouring single and double
/// quotes so a path containing spaces survives intact.
///
/// `split_whitespace` broke any command whose program path had a space
/// (`C:\Program Files\agent\kiro-cli.exe` → spawn `C:\Program`). This is a
/// minimal shell-word splitter: whitespace separates tokens except inside a
/// quoted span, and a matching quote pair is stripped. For unquoted input it
/// is identical to `split_whitespace`, so existing commands (including the
/// `cmd /c set …&& …` / `env … codex-acp` Ollama forms) tokenize unchanged.
/// We intentionally do NOT interpret backslash escapes — on Windows the
/// backslash is a path separator, and escaping isn't needed once quotes group
/// the spaces.
pub(super) fn tokenize_spawn_command(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;

    for c in command.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None; // closing quote — drop it, keep the token open
                } else {
                    current.push(c);
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                    in_token = true; // `""` alone is a valid empty argument
                } else if c.is_whitespace() {
                    if in_token {
                        tokens.push(std::mem::take(&mut current));
                        in_token = false;
                    }
                } else {
                    current.push(c);
                    in_token = true;
                }
            }
        }
    }
    if in_token {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod tokenize_tests {
    use super::tokenize_spawn_command;

    #[test]
    fn unquoted_matches_split_whitespace() {
        // The common case must behave exactly like the old split_whitespace,
        // including the Ollama env-prefixed forms.
        assert_eq!(
            tokenize_spawn_command("kiro-cli acp"),
            vec!["kiro-cli", "acp"]
        );
        assert_eq!(
            tokenize_spawn_command("env OPENAI_MODEL=llama3:8b codex-acp"),
            vec!["env", "OPENAI_MODEL=llama3:8b", "codex-acp"]
        );
        // Collapses runs of whitespace, like split_whitespace.
        assert_eq!(tokenize_spawn_command("  a   b\tc  "), vec!["a", "b", "c"]);
    }

    #[test]
    fn quoted_program_path_with_spaces_stays_one_token() {
        assert_eq!(
            tokenize_spawn_command(r#""C:\Program Files\agent\kiro-cli.exe" acp"#),
            vec![r"C:\Program Files\agent\kiro-cli.exe", "acp"]
        );
        assert_eq!(
            tokenize_spawn_command("'/Applications/Some App/bin/agent' --acp"),
            vec!["/Applications/Some App/bin/agent", "--acp"]
        );
    }

    #[test]
    fn empty_input_yields_no_tokens() {
        assert!(tokenize_spawn_command("").is_empty());
        assert!(tokenize_spawn_command("   ").is_empty());
    }

    #[test]
    fn empty_quoted_string_is_a_token() {
        assert_eq!(
            tokenize_spawn_command(r#"prog "" x"#),
            vec!["prog", "", "x"]
        );
    }
}
