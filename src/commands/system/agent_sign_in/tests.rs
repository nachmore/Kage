//! What the sign-in would run, worked out from Config. Pure: nothing here
//! spawns `kiro-cli` (or anything else).

use super::*;
use crate::config::AgentConnection;

fn cfg(preset: Option<&str>, mode: AcpMode) -> Config {
    let mut cfg = Config::default();
    cfg.acp.connections = vec![AgentConnection {
        id: "c1".into(),
        name: "Agent".into(),
        preset_id: preset.map(str::to_string),
        mode,
        sessions_directory: None,
        ollama_settings: None,
    }];
    cfg.acp.active_connection_id = "c1".into();
    cfg
}

fn local(cmd: &str) -> AcpMode {
    AcpMode::Local {
        spawn_command: cmd.into(),
    }
}

fn remote() -> AcpMode {
    AcpMode::Remote {
        host: "10.0.0.2".into(),
        port: 8765,
        timeout_ms: 30000,
    }
}

#[test]
fn sign_in_target_kiro_preset_local_bare() {
    assert_eq!(
        sign_in_target(&cfg(Some("kiro"), local("kiro-cli acp"))),
        Ok("kiro-cli".to_string())
    );
}

#[test]
fn sign_in_target_quoted_path_with_spaces() {
    assert_eq!(
        sign_in_target(&cfg(
            Some("kiro"),
            local("\"/Users/me/My Tools/kiro-cli\" acp")
        )),
        Ok("/Users/me/My Tools/kiro-cli".to_string())
    );
}

#[test]
fn sign_in_target_detects_kiro_without_preset() {
    assert_eq!(
        sign_in_target(&cfg(None, local("/opt/homebrew/bin/kiro-cli acp"))),
        Ok("/opt/homebrew/bin/kiro-cli".to_string())
    );
}

#[test]
fn sign_in_target_refuses_remote() {
    assert_eq!(
        sign_in_target(&cfg(Some("kiro"), remote())),
        Err(Unavailable::Remote)
    );
}

#[test]
fn sign_in_target_refuses_other_agents() {
    for preset in ["claude-code", "codex", "ollama"] {
        assert_eq!(
            sign_in_target(&cfg(Some(preset), local("kiro-cli acp"))),
            Err(Unavailable::NotKiro),
            "{preset}"
        );
    }
    for command in [
        "my-kiro-wrapper acp",
        "kiro-cli-chat acp",
        "wsl kiro-cli acp",
        "sh -c \"kiro-cli acp\"",
        "",
        "   ",
    ] {
        for preset in [Some("kiro"), None] {
            assert_eq!(
                sign_in_target(&cfg(preset, local(command))),
                Err(Unavailable::NotKiro),
                "{command:?} with preset {preset:?}"
            );
        }
    }
    // A remote connection that isn't explicitly Kiro isn't Kiro.
    assert_eq!(
        sign_in_target(&cfg(None, remote())),
        Err(Unavailable::NotKiro)
    );
}

#[test]
fn sign_in_target_no_connection() {
    let mut c = Config::default();
    c.acp.connections.clear();
    assert_eq!(sign_in_target(&c), Err(Unavailable::NoConnection));
}

#[test]
fn is_kiro_program_cases() {
    for yes in ["kiro-cli", "/a/b/kiro-cli", "KIRO-CLI", "/opt/Kiro-CLI"] {
        assert!(is_kiro_program(yes), "{yes}");
    }
    for no in [
        "kiro",
        "kiro-cli2",
        "kiro-cli.sh",
        "kiro-cli-chat",
        "my-kiro-cli",
        "",
        "/a/kiro-cli/",
    ] {
        assert!(!is_kiro_program(no), "{no}");
    }
    #[cfg(windows)]
    {
        assert!(is_kiro_program("C:\\x\\kiro-cli.exe"));
        assert!(is_kiro_program("KIRO-CLI.EXE"));
    }
    #[cfg(not(windows))]
    assert!(!is_kiro_program("kiro-cli.exe"));
}

fn tempdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kage-sign-in-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(unix)]
#[test]
fn resolve_program_absolute_must_be_executable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir();
    let file = dir.join("kiro-cli");
    std::fs::write(&file, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let token = file.to_string_lossy().into_owned();
    assert_eq!(resolve_program(&token, None, is_executable), None);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(resolve_program(&token, None, is_executable), Some(file));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resolve_program_searches_path_in_order() {
    let a = tempdir();
    let b = tempdir();
    let path = std::env::join_paths([a.clone(), b.clone()]).unwrap();
    let in_b = b.join("kiro-cli");
    let only_b = |p: &Path| p == in_b.as_path();
    assert_eq!(
        resolve_program("kiro-cli", Some(&path), only_b),
        Some(in_b.clone())
    );
    let both = |p: &Path| p == a.join("kiro-cli") || p == in_b.as_path();
    assert_eq!(
        resolve_program("kiro-cli", Some(&path), both),
        Some(a.join("kiro-cli"))
    );
    assert_eq!(resolve_program("kiro-cli", Some(&path), |_| false), None);
    assert_eq!(resolve_program("kiro-cli", None, |_| true), None);
    // Relative PATH entries (".", "bin") are never searched.
    let rel = std::env::join_paths([PathBuf::from("."), PathBuf::from("bin")]).unwrap();
    assert_eq!(resolve_program("kiro-cli", Some(&rel), |_| true), None);
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

#[test]
fn resolve_program_refuses_relative_with_separator() {
    let path = std::env::join_paths([tempdir()]).unwrap();
    for token in ["./kiro-cli", "bin/kiro-cli", "../kiro-cli", ""] {
        assert_eq!(
            resolve_program(token, Some(&path), |_| true),
            None,
            "{token}"
        );
    }
}

#[test]
fn sign_in_argv_is_fixed() {
    let home = std::env::temp_dir();
    for command in [
        "kiro-cli acp",
        "kiro-cli acp --trust-all-tools; rm -rf ~",
        "\"/opt/x/kiro-cli\" chat --evil",
    ] {
        let c = cfg(Some("kiro"), local(command));
        let token = sign_in_target(&c).unwrap();
        let spec = sign_in_spec(
            PathBuf::from("/abs").join(token.rsplit('/').next().unwrap()),
            home.clone(),
            Vec::new(),
            TerminalSize {
                cols: 0,
                rows: 9999,
            },
        );
        assert_eq!(spec.args, vec!["login".to_string()], "{command}");
        assert_eq!(spec.cwd, home);
        assert!(spec.program.ends_with("kiro-cli"));
        assert_eq!(
            spec.size,
            TerminalSize {
                cols: 20,
                rows: 200
            }
        );
    }
}

#[test]
fn context_never_offers_for_remote_or_non_kiro() {
    let cases = [
        (cfg(Some("kiro"), remote()), "remote"),
        (cfg(Some("codex"), local("codex-acp")), "not_kiro"),
        (cfg(None, local("wsl kiro-cli acp")), "not_kiro"),
        (
            {
                let mut c = Config::default();
                c.acp.connections.clear();
                c
            },
            "no_connection",
        ),
    ];
    for (c, reason) in cases {
        let ctx = sign_in_context(&c);
        assert!(!ctx.available, "{reason}");
        assert_eq!(ctx.reason, Some(reason));
        assert_eq!(ctx.command, None);
        assert_eq!(ctx.agent, None);
    }
    let ctx = sign_in_context(&cfg(Some("kiro"), local("kiro-cli acp")));
    assert!(ctx.available);
    assert_eq!(ctx.reason, None);
    assert_eq!(ctx.agent.as_deref(), Some("Kiro CLI"));
    assert_eq!(ctx.command.as_deref(), Some("kiro-cli login"));
    assert_eq!(ctx.timeout_minutes, 15);
    assert_eq!(ctx.terminal, cfg!(target_os = "macos"));
    let json = serde_json::to_value(&ctx).unwrap();
    assert!(json.get("timeoutMinutes").is_some());
}

#[test]
fn posix_quote_round_trips() {
    assert_eq!(
        posix_quote("/opt/homebrew/bin/kiro-cli"),
        "/opt/homebrew/bin/kiro-cli"
    );
    assert_eq!(
        posix_quote("/Users/me/My Tools/kiro-cli"),
        "'/Users/me/My Tools/kiro-cli'"
    );
    assert_eq!(posix_quote("/a/it's/kiro-cli"), r"'/a/it'\''s/kiro-cli'");
    assert_eq!(posix_quote(""), "''");
    assert_eq!(posix_quote("/a/$(x)/kiro-cli"), "'/a/$(x)/kiro-cli'");
    for path in [
        "/Users/me/My Tools/kiro-cli",
        "/a/it's/kiro-cli",
        "/a/\"q\"/kiro-cli",
    ] {
        let typed = format!("{} login", posix_quote(path));
        assert!(runner::validate_terminal_command(&typed).is_ok(), "{typed}");
        // Read back the way a POSIX shell would: one word, then `login`.
        let words = sh_words(&typed);
        assert_eq!(words, vec![path.to_string(), "login".to_string()]);
    }
}

/// A POSIX-shell word splitter for single quotes and plain words (all
/// `posix_quote` produces).
fn sh_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '\\' => {
                in_word = true;
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            ' ' => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            _ => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}
