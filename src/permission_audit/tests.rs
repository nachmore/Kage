use super::reader::read_recent;
use super::writer::{append_to, clear};
use super::writer::{ensure_parent, serialize};
use super::*;
use std::path::PathBuf;

fn tempdir() -> PathBuf {
    let path = std::env::temp_dir().join(format!("kage-audit-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn logpath(base: &std::path::Path) -> PathBuf {
    base.join("permission-audit.jsonl")
}

fn denied_entry(at: impl Into<String>, tool: impl Into<String>) -> AuditEntry {
    AuditEntry::at_time(
        at,
        AuditEvent::Denied {
            tool: tool.into(),
            session_id: None,
        },
    )
}

#[test]
fn append_then_read_roundtrip() {
    let path = logpath(&tempdir());
    let entry = AuditEntry::at_time(
        "2026-04-28T12:00:00.000Z",
        AuditEvent::Granted {
            tool: "shell_exec".to_string(),
            grant_type: crate::config::GrantType::Hours24,
            session_id: Some("s-1".to_string()),
            args_preview: Some("git status".to_string()),
        },
    );
    append_to(&path, &entry);
    assert_eq!(read_recent(&path, 10), vec![entry]);
}

#[test]
fn read_returns_most_recent_first_and_respects_limit() {
    let path = logpath(&tempdir());
    for i in 0..10 {
        append_to(
            &path,
            &denied_entry(format!("2026-04-28T12:00:{i:02}.000Z"), format!("t{i}")),
        );
    }

    let got = read_recent(&path, 3);
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].event, denied_entry("", "t9").event);
    assert_eq!(got[2].event, denied_entry("", "t7").event);
}

#[test]
fn read_missing_file_or_zero_limit_returns_empty() {
    let path = logpath(&tempdir());
    assert!(read_recent(&path, 100).is_empty());

    append_to(&path, &denied_entry("2026-04-28T12:00:00.000Z", "t0"));
    assert!(read_recent(&path, 0).is_empty());
}

#[test]
fn read_tolerates_malformed_lines_and_crlf() {
    let path = logpath(&tempdir());
    let good = serde_json::to_string(&denied_entry("2026-04-28T12:00:01.000Z", "good")).unwrap();
    std::fs::write(
        &path,
        format!(
            "not json\n{{malformed\n{good}\r\n\n{{\"at\":\"2026\",\"event\":\"unknown_event\"}}\n"
        ),
    )
    .unwrap();

    let got = read_recent(&path, 10);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].event, denied_entry("", "good").event);
}

#[test]
fn read_handles_missing_trailing_newline() {
    let path = logpath(&tempdir());
    let first = serde_json::to_string(&denied_entry("2026-04-28T12:00:00.000Z", "first")).unwrap();
    let second =
        serde_json::to_string(&denied_entry("2026-04-28T12:00:01.000Z", "second")).unwrap();
    std::fs::write(&path, format!("{first}\n{second}")).unwrap();

    let got = read_recent(&path, 10);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].event, denied_entry("", "second").event);
}

#[test]
fn read_recent_handles_chunk_boundaries() {
    let path = logpath(&tempdir());
    const N: usize = 1500;
    for i in 0..N {
        append_to(
            &path,
            &AuditEntry::at_time(
                format!(
                    "2026-04-28T12:{:02}:{:02}.{:03}Z",
                    i / 3600,
                    (i / 60) % 60,
                    i % 1000
                ),
                AuditEvent::Granted {
                    tool: format!("tool_{i:04}_{}", "x".repeat(i % 50)),
                    grant_type: crate::config::GrantType::Once,
                    session_id: None,
                    args_preview: None,
                },
            ),
        );
    }

    let got = read_recent(&path, 10);
    assert_eq!(got.len(), 10);
    for (offset, entry) in got.iter().enumerate() {
        let expected = format!("tool_{:04}_", N - 1 - offset);
        let AuditEvent::Granted { tool, .. } = &entry.event else {
            panic!("unexpected event variant");
        };
        assert!(tool.starts_with(&expected));
    }
    assert_eq!(read_recent(&path, N + 100).len(), N);
}

#[test]
fn clear_empties_existing_log_and_ignores_missing_file() {
    let path = logpath(&tempdir());
    assert!(clear(&path).is_ok());
    append_to(&path, &denied_entry("2026-04-28T12:00:00.000Z", "x"));
    clear(&path).unwrap();
    assert!(read_recent(&path, 10).is_empty());
}

#[test]
fn all_event_kinds_roundtrip() {
    let path = logpath(&tempdir());
    let events = [
        AuditEvent::Granted {
            tool: "a".into(),
            grant_type: crate::config::GrantType::Once,
            session_id: None,
            args_preview: None,
        },
        AuditEvent::Denied {
            tool: "b".into(),
            session_id: Some("s".into()),
        },
        AuditEvent::Revoked {
            tool: "c".into(),
            prior_policy: crate::config::PolicyKind::Allow,
            prior_grant_type: Some(crate::config::GrantType::Always),
        },
        AuditEvent::Expired {
            tool: "d".into(),
            prior_grant_type: crate::config::GrantType::Hours24,
        },
        AuditEvent::TerminatorModeChanged { enabled: true },
        AuditEvent::UserRanCommand {
            command: "brew install awscli".into(),
            cwd: "/home/someone".into(),
            via: "kage".into(),
            outcome: "exited".into(),
            exit_code: Some(0),
            session_id: Some("s".into()),
        },
        AuditEvent::UserRanCommand {
            command: "sudo softwareupdate -l".into(),
            cwd: "/".into(),
            via: "terminal".into(),
            outcome: "opened".into(),
            exit_code: None,
            session_id: None,
        },
    ];
    for (i, event) in events.iter().cloned().enumerate() {
        append_to(
            &path,
            &AuditEntry::at_time(format!("2026-04-28T12:00:0{i}.000Z"), event),
        );
    }

    let got = read_recent(&path, 100);
    assert_eq!(got.len(), events.len());
    for (i, entry) in got.iter().enumerate() {
        assert_eq!(entry.event, events[events.len() - 1 - i]);
    }
}

#[test]
fn types_and_writer_helpers_keep_their_contracts() {
    assert_eq!(
        AuditEvent::Granted {
            tool: "shell_exec".into(),
            grant_type: crate::config::GrantType::Always,
            session_id: None,
            args_preview: None,
        }
        .summary(),
        "Granted 'shell_exec' (always)"
    );
    assert_eq!(
        AuditEvent::TerminatorModeChanged { enabled: true }.summary(),
        "Terminator mode enabled"
    );

    let dir = tempdir();
    let nested = dir.join("a").join("b").join("audit.jsonl");
    assert!(ensure_parent(&nested));
    assert!(nested.parent().unwrap().is_dir());
    assert!(ensure_parent(&PathBuf::from("audit.jsonl")));

    let entry = denied_entry("2026-04-28T12:00:00.000Z", "x");
    let serialized = serialize(&entry).unwrap();
    assert_eq!(
        serde_json::from_str::<AuditEntry>(&serialized).unwrap(),
        entry
    );
}

#[test]
fn absent_optional_fields_are_omitted_from_json() {
    let entry = AuditEntry::at_time(
        "2026-04-28T12:00:00.000Z",
        AuditEvent::Granted {
            tool: "x".into(),
            grant_type: crate::config::GrantType::Once,
            session_id: None,
            args_preview: None,
        },
    );
    let serialized = serde_json::to_string(&entry).unwrap();
    assert!(!serialized.contains("session_id"));
    assert!(!serialized.contains("args_preview"));
}

#[test]
fn user_ran_command_entries_are_tagged_and_clipped() {
    let entry = AuditEntry::at_time(
        "2026-04-28T12:00:00.000Z",
        AuditEvent::UserRanCommand {
            command: "brew install awscli".into(),
            cwd: "/tmp".into(),
            via: "kage".into(),
            outcome: "exited".into(),
            exit_code: Some(1),
            session_id: None,
        },
    );
    let json: serde_json::Value = serde_json::to_value(&entry).unwrap();
    assert_eq!(json["event"], "user_ran_command");
    assert_eq!(json["via"], "kage");
    assert_eq!(json["exit_code"], 1);
    assert!(json.get("session_id").is_none());
    assert_eq!(entry.event.summary(), "User ran a command (kage, exited)");

    assert_eq!(command_preview("ls"), "ls");
    // Clipped on a character boundary, never inside "é" (2 bytes).
    let long = "é".repeat(2000);
    let clipped = command_preview(&long);
    assert!(clipped.ends_with('…'));
    assert!(clipped.len() <= types::USER_RAN_COMMAND_PREVIEW + '…'.len_utf8());
    assert!(clipped.trim_end_matches('…').chars().all(|c| c == 'é'));
}
