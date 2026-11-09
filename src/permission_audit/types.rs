use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Kinds of events we track. Each variant carries the data needed to
/// reconstruct what happened without cross-referencing the config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AuditEvent {
    /// User approved a tool request. `grant_type` is the scope they
    /// picked at the prompt.
    Granted {
        tool: String,
        grant_type: crate::config::GrantType,
        /// Optional: the session id the request belonged to, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        /// Optional: arguments the agent was asking to use, if the
        /// caller surfaced them. We store up to 2 KB per entry to
        /// keep the log navigable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args_preview: Option<String>,
    },
    /// User denied a single request. Not the same as revoke — the
    /// existing policy stays in place, this was a one-time "no".
    Denied {
        tool: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    /// User revoked a standing grant (removed it from settings, or
    /// changed its policy to "ask" / "deny").
    Revoked {
        tool: String,
        /// What the policy was before the revoke happened.
        prior_policy: crate::config::PolicyKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prior_grant_type: Option<crate::config::GrantType>,
    },
    /// A grant expired on its own (24h or 30-day staleness).
    Expired {
        tool: String,
        prior_grant_type: crate::config::GrantType,
    },
    /// User turned terminator mode on or off. We track this because
    /// during terminator mode every request is auto-approved and
    /// doesn't get its own `Granted` entry.
    TerminatorModeChanged { enabled: bool },
    /// The user ran a command themselves, outside the agent's sandbox,
    /// from a shell block's sandbox notice ("Run it here" / "Run in
    /// Terminal"): their own click in a confirm, not an agent grant.
    UserRanCommand {
        /// The command, clipped to [`USER_RAN_COMMAND_PREVIEW`] bytes.
        command: String,
        /// The working directory it ran in.
        cwd: String,
        /// `kage` (run by Kage, output captured) or `terminal` (handed
        /// to the terminal app).
        via: String,
        /// `exited`, `cancelled`, `timed_out`, `failed_to_start`, or
        /// `opened` (terminal: handed over, outcome unknown to Kage).
        outcome: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    /// An extension asked for a file the user picked, and Kage handled it,
    /// whatever the outcome (docs/design/EXTENSION_FILE_TEXT.md §5.13).
    /// Not written when the user closed the dialog without a file.
    ExtensionFileRead {
        extension_id: String,
        /// The cleaned file name; `None` if it looks like a secret.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_name: Option<String>,
        /// The detected format (`pdf`, …), when known.
        #[serde(default)]
        format: Option<String>,
        #[serde(default)]
        bytes: u64,
        /// 0 on failure.
        #[serde(default)]
        chars: u32,
        #[serde(default)]
        truncated: bool,
        /// `ok`, or the error key's last segment (`too_large`, …).
        outcome: String,
        #[serde(default)]
        duration_ms: u64,
    },
}

/// Longest command an audit entry keeps, in bytes — the same 2 KB the
/// `Granted` entries' `args_preview` keeps.
pub const USER_RAN_COMMAND_PREVIEW: usize = 2048;

/// `command` clipped to [`USER_RAN_COMMAND_PREVIEW`] bytes on a character
/// boundary, with `…` when cut.
pub fn command_preview(command: &str) -> String {
    if command.len() <= USER_RAN_COMMAND_PREVIEW {
        return command.to_string();
    }
    let mut cut = USER_RAN_COMMAND_PREVIEW;
    while !command.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &command[..cut])
}

impl AuditEvent {
    /// Human-readable summary used by UI and logs.
    #[allow(dead_code)] // Kept for log formatting and future Rust-side UIs.
    pub fn summary(&self) -> String {
        match self {
            AuditEvent::Granted {
                tool, grant_type, ..
            } => {
                format!("Granted '{}' ({})", tool, grant_type.as_str())
            }
            AuditEvent::Denied { tool, .. } => format!("Denied '{}'", tool),
            AuditEvent::Revoked {
                tool, prior_policy, ..
            } => {
                format!("Revoked '{}' (was {})", tool, prior_policy.as_str())
            }
            AuditEvent::Expired {
                tool,
                prior_grant_type,
            } => {
                format!("Expired '{}' ({})", tool, prior_grant_type.as_str())
            }
            AuditEvent::TerminatorModeChanged { enabled } => {
                if *enabled {
                    "Terminator mode enabled".to_string()
                } else {
                    "Terminator mode disabled".to_string()
                }
            }
            AuditEvent::UserRanCommand { via, outcome, .. } => {
                format!("User ran a command ({}, {})", via, outcome)
            }
            // Never the file name: summaries end up in logs.
            AuditEvent::ExtensionFileRead {
                extension_id,
                format,
                chars,
                outcome,
                ..
            } => {
                if outcome == "ok" {
                    format!(
                        "Extension '{}' read a file ({}, {} characters)",
                        extension_id,
                        format.as_deref().unwrap_or("unknown"),
                        chars
                    )
                } else {
                    format!(
                        "Extension '{}' couldn't read a file ({})",
                        extension_id, outcome
                    )
                }
            }
        }
    }
}

/// One row in the audit log. Flat so serde_json produces a single
/// ordered-key JSON object that's easy to eyeball in the file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEntry {
    /// ISO 8601 UTC timestamp, e.g. "2026-04-28T14:23:00.123Z".
    pub at: String,
    #[serde(flatten)]
    pub event: AuditEvent,
}

impl AuditEntry {
    /// Construct an entry timestamped now (UTC). Tests use
    /// `AuditEntry::at_time` to control the timestamp.
    pub fn now(event: AuditEvent) -> Self {
        Self {
            at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            event,
        }
    }

    #[allow(dead_code)] // used by tests
    pub fn at_time(at: impl Into<String>, event: AuditEvent) -> Self {
        Self {
            at: at.into(),
            event,
        }
    }
}

/// The on-disk path for the audit log. Returns `None` if the config
/// directory itself is unavailable (very rare).
pub fn default_log_path() -> Option<PathBuf> {
    Some(
        crate::paths::config_dir()?
            .join("kage")
            .join("permission-audit.jsonl"),
    )
}
