use super::{dedupe_shim_candidates, extract_version};
use crate::agent_presets::{detection_hints, AgentKind, ALLOWED_WRAPPER_NPM_PACKAGES};
use std::path::PathBuf;

#[test]
fn claude_code_acp_hint_falls_back_to_claude_for_version() {
    // claude-code-acp prints nothing for `--version` today; without
    // the fallback the wrapper card would never show a version
    // even though the bare claude CLI right next to it prints a
    // perfectly good one.
    let hints = detection_hints();
    let wrapper = hints
        .iter()
        .find(|h| h.kind == AgentKind::ClaudeCode && h.binary_names.contains(&"claude-code-acp"))
        .expect("claude-code-acp hint missing");
    assert_eq!(wrapper.fallback_version_probe, Some("claude"));
}

#[test]
fn bare_claude_hint_runs_version_probe() {
    // Wrapper-needed cards are still informative when they show
    // which Claude install Kage will end up wrapping. Skipping the
    // probe here was the bug.
    let hints = detection_hints();
    let bare = hints
        .iter()
        .find(|h| h.kind == AgentKind::ClaudeCode && h.binary_names == ["claude"])
        .expect("bare-claude hint missing");
    assert!(
        !bare.version_args.is_empty(),
        "bare-claude hint should run a version probe so the card shows the underlying version"
    );
}

#[test]
fn detection_hints_include_bare_claude_with_wrapper() {
    let hints = detection_hints();
    let bare = hints
        .iter()
        .find(|h| h.kind == AgentKind::ClaudeCode && h.binary_names.contains(&"claude"))
        .expect("bare-claude detection hint missing");
    assert_eq!(
        bare.wrapper_npm_package,
        Some("@zed-industries/claude-code-acp"),
        "bare-claude hint must point at the Zed wrapper package"
    );
}

#[test]
fn ready_to_use_hints_have_no_wrapper() {
    for hint in detection_hints() {
        // Only the bare-claude hint declares a wrapper requirement.
        // A ready-to-use binary advertising one would mean the UI
        // shows an "install wrapper" button for an already-working
        // agent.
        let is_bare_claude = hint.kind == AgentKind::ClaudeCode && hint.binary_names == ["claude"];
        if !is_bare_claude {
            assert!(
                hint.wrapper_npm_package.is_none(),
                "hint for {:?} ({:?}) should not require a wrapper",
                hint.kind,
                hint.binary_names
            );
        }
    }
}

#[test]
fn wrapper_install_rejects_unallowlisted_package() {
    // The `install_acp_wrapper` allowlist is the security boundary
    // for the IPC surface — drift here and we'd be exposing an
    // arbitrary `npm install -g` runner to the frontend.
    assert!(
        ALLOWED_WRAPPER_NPM_PACKAGES.contains(&"@zed-industries/claude-code-acp"),
        "the Claude wrapper must be in the install allowlist"
    );
    assert!(
        !ALLOWED_WRAPPER_NPM_PACKAGES.contains(&"left-pad"),
        "allowlist must reject arbitrary packages"
    );
    assert!(
        !ALLOWED_WRAPPER_NPM_PACKAGES.contains(&""),
        "allowlist must reject empty package names"
    );
}

#[test]
fn extract_version_picks_first_dotted_digit_token() {
    // The bug we're fixing: whitespace-splitting the first line of
    // `kiro-cli --version` returned "kiro-cli-chat" as the version.
    assert_eq!(
        extract_version("kiro-cli-chat 0.0.0-dev"),
        Some("0.0.0-dev".to_string()),
    );
    // Claude prints diagnostics to a leading line and the version
    // is on its own line, with the actual number followed by a
    // human label in parens. We want only the version.
    assert_eq!(
        extract_version("claude: info: builder-mcp setup: stamp_exists\n2.1.128 (Claude Code)"),
        Some("2.1.128".to_string()),
    );
    // `v`-prefixed versions are common (semver tooling, Go, …) —
    // strip the prefix so the badge shows the version proper.
    assert_eq!(extract_version("foo v1.2.3"), Some("1.2.3".to_string()));
    // Bare digit-only strings without a dot aren't semver-shaped
    // and are usually exit codes or year-stamps — skip.
    assert_eq!(extract_version("build 12345"), None);
    assert_eq!(extract_version(""), None);
}

#[test]
fn dedupe_shim_candidates_collapses_npm_pair_keeping_cmd() {
    // npm `-g` on Windows installs a Unix script (no extension)
    // alongside a `.cmd` shim in the same directory. `where`
    // returns both, so without dedup the user sees two cards.
    // Both files exist, but `.cmd` is the one Windows knows how
    // to run via `Command::new`.
    let inputs = vec![
        PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\claude-code-acp"),
        PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\claude-code-acp.cmd"),
    ];
    let out = dedupe_shim_candidates(inputs);
    assert_eq!(out.len(), 1);
    assert_eq!(
        out[0],
        PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\claude-code-acp.cmd"),
        ".cmd should win over the no-extension shim"
    );
}

#[test]
fn dedupe_shim_candidates_keeps_distinct_installs() {
    // Two genuinely different installs must NOT collapse — same
    // stem in different directories is a real "user has two
    // copies" case (Toolbox vs. local install). Preserve both.
    let inputs = vec![
        PathBuf::from(r"C:\Users\me\AppData\Local\Toolbox\bin\kiro-cli.exe"),
        PathBuf::from(r"C:\Users\me\AppData\Local\kiro-cli\kiro-cli.exe"),
    ];
    let out = dedupe_shim_candidates(inputs.clone());
    assert_eq!(out, inputs);
}

#[test]
fn dedupe_shim_candidates_prefers_exe_over_cmd() {
    let inputs = vec![
        PathBuf::from(r"C:\foo\agent.cmd"),
        PathBuf::from(r"C:\foo\agent.exe"),
    ];
    let out = dedupe_shim_candidates(inputs);
    assert_eq!(out, vec![PathBuf::from(r"C:\foo\agent.exe")]);
}
