// Linux application launcher

use anyhow::{Context, Result};
use log::info;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::os::launcher::AppInfo;

pub fn scan_applications_impl() -> Result<Vec<AppInfo>> {
    let mut apps = Vec::new();

    // Scan .desktop files in standard locations
    let mut desktop_dirs = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];

    if let Some(home) = dirs::home_dir() {
        desktop_dirs.push(home.join(".local/share/applications"));
    }

    for dir in desktop_dirs {
        if dir.exists() {
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("desktop") {
                        if let Ok(content) = fs::read_to_string(&path) {
                            if let Some(app_info) = parse_desktop_file(&content, &path) {
                                apps.push(app_info);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(apps)
}

fn parse_desktop_file(content: &str, _path: &PathBuf) -> Option<AppInfo> {
    let mut name = None;
    let mut exec = None;
    // Only parse the [Desktop Entry] section — actions ([Desktop Action ...])
    // have their own Name/Exec we don't want to pick up by accident.
    let mut in_main_section = false;

    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_main_section = line == "[Desktop Entry]";
            continue;
        }
        if !in_main_section {
            continue;
        }
        // The first matching key in the section wins; skip later overrides
        // (locale-specific Name[xx]=... lines never start with plain "Name=").
        if name.is_none() {
            if let Some(rest) = line.strip_prefix("Name=") {
                name = Some(rest.to_string());
                continue;
            }
        }
        if exec.is_none() {
            if let Some(rest) = line.strip_prefix("Exec=") {
                exec = Some(rest.to_string());
            }
        }
    }

    let (name, exec) = (name?, exec?);
    let program = parse_exec_field(&exec)?;

    Some(AppInfo {
        name,
        path: PathBuf::from(&program),
        icon_path: Some(program),
        emoji_icon: None,
        icon_data: None,
    })
}

/// Extract the program path from a freedesktop `Exec=` field.
///
/// The Exec field contains a command line plus optional field codes that
/// the launcher is supposed to substitute at run time:
///   %f / %F   single file / list of files
///   %u / %U   single URL / list of URLs
///   %i / %c / %k   icon flag / translated name / desktop-file path
///   %d / %D / %n / %N / %v / %m   deprecated, ignored
///   %%        literal %
/// We don't run the app directly, we just record the program for AppInfo,
/// so we strip every `%X` token, honour `\\\\` and `\\"` escapes inside
/// quoted strings, and return the first whitespace-separated token.
///
/// Returns `None` if the Exec field has no usable program token (e.g.
/// only field codes, only whitespace, or unbalanced quoting).
fn parse_exec_field(exec: &str) -> Option<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = exec.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' if in_quotes => {
                // Inside double quotes, `\\` and `\"` are the only valid
                // escapes per the spec. Pass the literal next char through
                // so paths with embedded quotes survive.
                if let Some(&next) = chars.peek() {
                    chars.next();
                    current.push(next);
                }
            }
            '"' => in_quotes = !in_quotes,
            '%' if !in_quotes => {
                // Field code — consume the next char and skip both. `%%`
                // becomes a literal `%`.
                match chars.next() {
                    Some('%') => current.push('%'),
                    Some(_) => {} // f / F / u / U / i / c / k / etc — drop
                    None => {}    // trailing %, malformed — drop
                }
            }
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if in_quotes {
        return None;
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    tokens.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_exec_field --------------------------------------------------

    #[test]
    fn exec_strips_field_codes() {
        // %U, %F, %u, %f, %i, %c, %k all need to be dropped — the resulting
        // path must not include them.
        assert_eq!(
            parse_exec_field("/usr/bin/firefox %U").as_deref(),
            Some("/usr/bin/firefox")
        );
        assert_eq!(
            parse_exec_field("/usr/bin/code %F").as_deref(),
            Some("/usr/bin/code")
        );
        assert_eq!(
            parse_exec_field("/usr/bin/foo %u %f").as_deref(),
            Some("/usr/bin/foo")
        );
        assert_eq!(
            parse_exec_field("/usr/bin/bar %i").as_deref(),
            Some("/usr/bin/bar")
        );
        assert_eq!(
            parse_exec_field("/usr/bin/baz %c").as_deref(),
            Some("/usr/bin/baz")
        );
        assert_eq!(
            parse_exec_field("/usr/bin/qux %k").as_deref(),
            Some("/usr/bin/qux")
        );
    }

    #[test]
    fn exec_preserves_literal_percent() {
        // The spec maps `%%` to a literal `%`. A path with one in it must
        // survive — even though it's pathological, the parser shouldn't
        // mangle valid input.
        assert_eq!(
            parse_exec_field("/usr/bin/weird%%name %U").as_deref(),
            Some("/usr/bin/weird%name"),
        );
    }

    #[test]
    fn exec_handles_quoted_path_with_spaces() {
        // Common when an app lives somewhere like "/opt/My App/bin".
        assert_eq!(
            parse_exec_field(r#""/opt/My App/bin/foo" %U"#).as_deref(),
            Some("/opt/My App/bin/foo"),
        );
    }

    #[test]
    fn exec_handles_escaped_quotes_inside_quotes() {
        // Per the spec, inside a double-quoted argument the only escapes
        // are `\\` and `\"`. The result must contain a literal quote.
        assert_eq!(
            parse_exec_field(r#""/opt/has\"quote/bin" %U"#).as_deref(),
            Some(r#"/opt/has"quote/bin"#),
        );
    }

    #[test]
    fn exec_returns_none_when_only_field_codes() {
        // No actual program token left — caller should treat this as
        // unparseable rather than emitting an empty AppInfo.path.
        assert!(parse_exec_field("%U %F").is_none());
        assert!(parse_exec_field("").is_none());
        assert!(parse_exec_field("   ").is_none());
    }

    #[test]
    fn exec_returns_none_for_unbalanced_quotes() {
        // A trailing-unclosed-quote line means the .desktop file is
        // malformed — refuse rather than silently truncating.
        assert!(parse_exec_field(r#""/opt/missing-end %U"#).is_none());
    }

    #[test]
    fn exec_first_token_wins_for_multi_arg_commands() {
        // Many .desktop files have lines like `sh -c "..."` — we want the
        // launcher path, which is the first token.
        assert_eq!(
            parse_exec_field("/bin/sh -c \"my command %U\"").as_deref(),
            Some("/bin/sh"),
        );
    }

    // ---- parse_desktop_file ------------------------------------------------

    fn fake_path() -> PathBuf {
        PathBuf::from("/usr/share/applications/test.desktop")
    }

    #[test]
    fn desktop_file_extracts_name_and_exec_path_without_field_codes() {
        // Headline regression: the original parser stored the entire Exec=
        // line — including `%U` — as AppInfo.path. The fix routes through
        // parse_exec_field so the path is launchable.
        let content = "[Desktop Entry]\nName=Firefox\nExec=/usr/bin/firefox %U\n";
        let info = parse_desktop_file(content, &fake_path()).expect("parses");
        assert_eq!(info.name, "Firefox");
        assert_eq!(info.path, PathBuf::from("/usr/bin/firefox"));
    }

    #[test]
    fn desktop_file_ignores_action_subsections() {
        // Some .desktop files have `[Desktop Action ...]` subsections
        // with their own Name/Exec. The main section's values must win.
        let content = "\
[Desktop Entry]\n\
Name=Main App\n\
Exec=/usr/bin/main %U\n\
\n\
[Desktop Action newwin]\n\
Name=New Window\n\
Exec=/usr/bin/main --new-window %U\n";
        let info = parse_desktop_file(content, &fake_path()).expect("parses");
        assert_eq!(info.name, "Main App");
        assert_eq!(info.path, PathBuf::from("/usr/bin/main"));
    }

    #[test]
    fn desktop_file_skips_localized_name_overrides() {
        // Locale-specific keys like Name[de_DE]= must not override Name=.
        let content = "\
[Desktop Entry]\n\
Name=Original\n\
Name[fr]=Originale\n\
Exec=/usr/bin/x\n";
        let info = parse_desktop_file(content, &fake_path()).expect("parses");
        assert_eq!(info.name, "Original");
    }

    #[test]
    fn desktop_file_returns_none_when_required_fields_missing() {
        // No Exec → no AppInfo (we can't launch anything).
        assert!(parse_desktop_file("[Desktop Entry]\nName=NoExec\n", &fake_path()).is_none());
        // No Name → no AppInfo (we can't display anything sensibly).
        assert!(parse_desktop_file("[Desktop Entry]\nExec=/usr/bin/x\n", &fake_path()).is_none());
    }

    #[test]
    fn desktop_file_returns_none_when_exec_is_only_field_codes() {
        // Pathological but possible: malformed file with only `%U` after
        // `Exec=`. parse_exec_field returns None, so we should too.
        let content = "[Desktop Entry]\nName=Bad\nExec=%U\n";
        assert!(parse_desktop_file(content, &fake_path()).is_none());
    }
}

pub fn launch_application_impl(path: &PathBuf) -> Result<()> {
    info!("Launching Linux application at {:?}", path);

    if path.extension().and_then(|s| s.to_str()) == Some("desktop") {
        Command::new("xdg-open")
            .arg(path)
            .spawn()
            .context("Failed to launch application")?;
    } else {
        Command::new(path)
            .spawn()
            .context("Failed to launch application")?;
    }

    Ok(())
}

/// Launch by name. Linux has no built-in app-name → binary resolution the
/// way Windows (ShellExecuteW) or macOS (`open -a`) do, so this is a best
/// effort: URIs go through `xdg-open`, everything else is attempted as a
/// direct command. Proper name resolution would require walking the
/// freedesktop `.desktop` index; tracked as future work.
pub fn shell_launch_impl(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("shell_launch called with empty name");
    }

    // Leading RFC 3986 URI scheme (same shape as macOS) goes through xdg-open.
    if looks_like_uri(name) {
        info!("shell_launch_impl: xdg-open '{}'", name);
        let status = Command::new("xdg-open")
            .arg(name)
            .status()
            .context("xdg-open failed to start")?;
        if !status.success() {
            anyhow::bail!("xdg-open '{}' exited with {}", name, status);
        }
        return Ok(());
    }

    // Split on first space so `"firefox --private-window"` lands as
    // `firefox` + `["--private-window"]` (parity with the Windows impl).
    let (program, args) = match name.split_once(' ') {
        Some((p, rest)) => (p, rest.split_whitespace().collect::<Vec<_>>()),
        None => (name, Vec::new()),
    };

    info!("shell_launch_impl: exec '{}' args={:?}", program, args);
    Command::new(program)
        .args(&args)
        .spawn()
        .with_context(|| format!("Failed to launch '{}'", name))?;
    Ok(())
}

/// True if `s` starts with an RFC 3986 URI scheme. Mirrors the macOS helper —
/// kept inline rather than shared because the rest of the Linux launcher is
/// already platform-specific and duplicating a 10-line helper is cheaper
/// than threading another cross-platform module.
fn looks_like_uri(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    for &b in bytes.iter().skip(1) {
        if b == b':' {
            return true;
        }
        let ok = b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.';
        if !ok {
            return false;
        }
    }
    false
}
