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
        dirs::config_dir()?
            .join("kage")
            .join("permission-audit.jsonl"),
    )
}
