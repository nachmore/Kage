// Linux application launcher

use anyhow::{Context, Result};
use log::info;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::os::launcher::AppInfo;

/// `applications/` dirs to scan, highest priority first: XDG_DATA_HOME
/// (user overrides win), then XDG_DATA_DIRS, then the flatpak/snap export
/// dirs in case the session didn't add them to XDG_DATA_DIRS.
fn desktop_dirs() -> Vec<PathBuf> {
    let home = dirs::home_dir();
    let non_empty = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty());

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(data_home) = non_empty("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")))
    {
        roots.push(data_home);
    }
    if let Some(data_dirs) = non_empty("XDG_DATA_DIRS") {
        roots.extend(std::env::split_paths(&data_dirs));
    }
    // Spec defaults — always included (dedup'd below) so a session that
    // sets XDG_DATA_DIRS without them still finds the system apps.
    roots.push(PathBuf::from("/usr/local/share"));
    roots.push(PathBuf::from("/usr/share"));
    roots.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    if let Some(h) = &home {
        roots.push(h.join(".local/share/flatpak/exports/share"));
    }

    let mut out: Vec<PathBuf> = Vec::new();
    let candidates = roots
        .into_iter()
        .map(|r| r.join("applications"))
        .chain(std::iter::once(PathBuf::from(
            "/var/lib/snapd/desktop/applications",
        )));
    for dir in candidates {
        if !out.contains(&dir) {
            out.push(dir);
        }
    }
    out
}

pub fn scan_applications_impl() -> Result<Vec<AppInfo>> {
    let mut apps = Vec::new();
    // Desktop-file IDs already claimed by a higher-priority dir — per the
    // spec, the first one found shadows the rest.
    let mut seen_ids = HashSet::new();

    for dir in desktop_dirs() {
        // Missing dirs (no flatpak/snap, etc.) are normal.
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("desktop") {
                continue;
            }
            if !seen_ids.insert(entry.file_name()) {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&path) {
                if let Some(app_info) = parse_desktop_file(&content, &path) {
                    apps.push(app_info);
                }
            }
        }
    }

    Ok(apps)
}

/// First value of each key in the `[Desktop Entry]` section. Actions
/// (`[Desktop Action ...]`) have their own Name/Exec we must not pick up,
/// and localised keys (`Name[fr]`) are distinct keys so never shadow
/// `Name`.
fn desktop_entry_fields(content: &str) -> HashMap<&str, &str> {
    let mut fields = HashMap::new();
    let mut in_main_section = false;
    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_main_section = line == "[Desktop Entry]";
            continue;
        }
        if !in_main_section || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            fields.entry(key.trim()).or_insert(value.trim());
        }
    }
    fields
}

/// Build an AppInfo whose `path` is the .desktop file itself. The Exec
/// line is often a wrapper (`env FOO=1 /snap/bin/x`, `flatpak run …`,
/// `sh -c "…"`) whose first token alone launches the wrong program, so
/// launching goes through the desktop file (see `launch_desktop_file`).
fn parse_desktop_file(content: &str, path: &Path) -> Option<AppInfo> {
    let fields = desktop_entry_fields(content);
    // Hidden/NoDisplay entries are helpers, MIME handlers and deleted
    // entries — not things a user launches.
    if fields.get("Type").is_some_and(|t| *t != "Application")
        || fields.get("NoDisplay") == Some(&"true")
        || fields.get("Hidden") == Some(&"true")
    {
        return None;
    }
    let name = fields.get("Name")?.to_string();
    // Still require a parseable Exec — an entry we can't run isn't an app.
    let program = parse_exec_field(fields.get("Exec")?)?;
    // Icon= (a theme icon name or absolute path) is what an icon lookup
    // wants; fall back to the program like before.
    let icon_path = fields
        .get("Icon")
        .filter(|i| !i.is_empty())
        .map_or(program, |i| i.to_string());

    Some(AppInfo {
        name,
        path: path.to_path_buf(),
        icon_path: Some(icon_path),
        emoji_icon: None,
        icon_data: None,
    })
}

/// Launch a .desktop entry. `gio launch` and `gtk-launch` implement the
/// full spec (field codes, Terminal=, DBusActivatable); running the Exec
/// argv ourselves is the last resort for systems with neither. Note
/// `xdg-open file.desktop` is not an option — many desktops open the file
/// in a text editor.
fn launch_desktop_file(path: &Path) -> Result<()> {
    match Command::new("gio").arg("launch").arg(path).status() {
        Ok(status) if status.success() => return Ok(()),
        Ok(status) => info!("gio launch {:?} exited with {}", path, status),
        Err(e) => info!("gio unavailable ({e}); trying gtk-launch"),
    }
    // gtk-launch takes the desktop-file ID (basename without .desktop) and
    // only finds entries in the XDG dirs, which covers what we scan.
    if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
        if matches!(Command::new("gtk-launch").arg(id).status(), Ok(s) if s.success()) {
            return Ok(());
        }
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read desktop file {:?}", path))?;
    let fields = desktop_entry_fields(&content);
    let argv = fields
        .get("Exec")
        .copied()
        .and_then(parse_exec_argv)
        .with_context(|| format!("No usable Exec= in {:?}", path))?;
    let (program, args) = argv.split_first().context("Exec= has no program token")?;
    info!("Launching Exec= argv directly: {:?}", argv);
    Command::new(program)
        .args(args)
        .spawn()
        .context("Failed to launch application")?;
    Ok(())
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
/// We never have files/URLs to substitute, so `parse_exec_argv` strips
/// every `%X` token, honours `\\\\` and `\\"` escapes inside quoted strings,
/// and splits the rest into argv (the last-resort launch path in
/// `launch_desktop_file`). `parse_exec_field` keeps just the program.
///
/// Returns `None` if the Exec field has no usable program token (e.g.
/// only field codes, only whitespace, or unbalanced quoting).
fn parse_exec_field(exec: &str) -> Option<String> {
    parse_exec_argv(exec)?.into_iter().next()
}

fn parse_exec_argv(exec: &str) -> Option<Vec<String>> {
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

    (!tokens.is_empty()).then_some(tokens)
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
        // The program recorded from Exec= must not carry field codes like `%U`.
        let content = "[Desktop Entry]\nName=Firefox\nExec=/usr/bin/firefox %U\n";
        let info = parse_desktop_file(content, &fake_path()).expect("parses");
        assert_eq!(info.name, "Firefox");
        // Launching goes through the .desktop file (wrapper Exec lines break
        // first-token launches); the program is only the icon fallback.
        assert_eq!(info.path, fake_path());
        assert_eq!(info.icon_path.as_deref(), Some("/usr/bin/firefox"));
    }

    #[test]
    fn desktop_file_prefers_icon_key() {
        let content = "[Desktop Entry]\nName=Firefox\nExec=firefox %U\nIcon=firefox-esr\n";
        let info = parse_desktop_file(content, &fake_path()).expect("parses");
        assert_eq!(info.icon_path.as_deref(), Some("firefox-esr"));
    }

    #[test]
    fn desktop_file_skips_hidden_nodisplay_and_non_applications() {
        for extra in ["NoDisplay=true", "Hidden=true", "Type=Link"] {
            let content = format!("[Desktop Entry]\nName=X\nExec=/usr/bin/x\n{extra}\n");
            assert!(
                parse_desktop_file(&content, &fake_path()).is_none(),
                "{extra} should be skipped"
            );
        }
        let ok = "[Desktop Entry]\nType=Application\nName=X\nExec=/usr/bin/x\nNoDisplay=false\n";
        assert!(parse_desktop_file(ok, &fake_path()).is_some());
    }

    #[test]
    fn exec_argv_keeps_wrapper_arguments() {
        assert_eq!(
            parse_exec_argv("env BAMF_DESKTOP_FILE_HINT=/x.desktop /snap/bin/foo %U"),
            Some(vec![
                "env".to_string(),
                "BAMF_DESKTOP_FILE_HINT=/x.desktop".to_string(),
                "/snap/bin/foo".to_string(),
            ])
        );
        assert_eq!(
            parse_exec_argv(r#"sh -c "echo hi""#),
            Some(vec![
                "sh".to_string(),
                "-c".to_string(),
                "echo hi".to_string()
            ])
        );
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
        assert_eq!(info.icon_path.as_deref(), Some("/usr/bin/main"));
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
        launch_desktop_file(path)?;
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

    // list_installed_apps hands the agent .desktop paths; launch those as
    // desktop entries rather than trying to exec the file.
    let as_path = Path::new(name);
    if as_path.extension().and_then(|s| s.to_str()) == Some("desktop") && as_path.is_file() {
        info!("shell_launch_impl: desktop entry '{}'", name);
        return launch_desktop_file(as_path);
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
