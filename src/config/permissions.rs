use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolPermissionsConfig {
    #[serde(default)]
    pub trust_all: bool,
    #[serde(default)]
    pub tools: Vec<ToolPolicy>,
    /// Terminator mode: auto-approve all tool requests without any prompts
    #[serde(default)]
    pub terminator_mode: bool,
}

impl ToolPermissionsConfig {
    /// Resolve the effective policy for a tool by title.
    ///
    /// An explicit per-tool policy is consulted FIRST and always wins — in
    /// particular an explicit `Deny` is honoured even under `trust_all` /
    /// `terminator_mode`. The blanket-allow modes only upgrade a tool that
    /// would otherwise be `Ask` (or has no recorded policy). This matches the
    /// contract in docs/TOOL_PERMISSIONS.md: "allow everything except explicit
    /// deny" — not "allow everything, period".
    pub fn resolve_policy(&self, tool_title: &str) -> PolicyKind {
        let explicit = self
            .tools
            .iter()
            .find(|t| t.title == tool_title)
            .map(|t| t.effective_policy());
        let blanket_allow = self.terminator_mode || self.trust_all;
        match explicit {
            Some(PolicyKind::Deny) => PolicyKind::Deny,
            Some(PolicyKind::Allow) => PolicyKind::Allow,
            _ if blanket_allow => PolicyKind::Allow,
            Some(p) => p,
            None => PolicyKind::Ask,
        }
    }
}

/// Per-tool permission policy. The frontend's UI exposes three states —
/// Always Ask, Allow, Deny — combined with a separate `grant_type` for
/// the duration of an Allow grant.
///
/// `#[serde(other)]` on `Ask` means an unknown wire value (e.g. a future
/// variant or a hand-edited config) collapses to "Ask" rather than
/// failing config load. Forward-compat without back-compat shims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PolicyKind {
    Allow,
    Deny,
    /// Default. Listed last so `#[serde(other)]` can land on it — that
    /// makes any unknown wire value (future variant, hand-edited
    /// config) collapse to "ask" and re-prompt the user, which is the
    /// safe-by-default behaviour.
    #[default]
    #[serde(other)]
    Ask,
}

impl PolicyKind {
    /// Wire-format string. Stable across releases.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

/// Duration of an Allow grant. `Hours24` serialises as `"24h"` because
/// that's the wire format the JS UI emits — `#[serde(rename)]` handles
/// the digit prefix that snake_case can't reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GrantType {
    /// One-shot grant — consumed after the next tool call.
    #[default]
    Once,
    /// Sliding 24-hour grant from `granted_at`.
    #[serde(rename = "24h")]
    Hours24,
    /// Persistent grant; re-prompts after 30 days of inactivity (see
    /// `effective_policy`'s staleness check).
    Always,
}

impl GrantType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Hours24 => "24h",
            Self::Always => "always",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPolicy {
    /// Display title; the map key is the tool id, so an empty title
    /// only affects the settings list label.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub policy: PolicyKind,
    #[serde(default)]
    pub last_seen: String, // ISO 8601 — last time this tool was requested
    #[serde(default)]
    pub granted_at: String, // ISO 8601 — when the current grant was issued
    #[serde(default)]
    pub grant_type: GrantType,
}

/// A user-approved capability grant for an installed extension.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ExtensionGrant {
    /// Capabilities the user approved at install or upgrade time.
    /// See ui/js/shared/extension-permissions.js for the authoritative list.
    #[serde(default)]
    pub granted: Vec<String>,
    /// Version of the extension manifest at the time of approval. If the
    /// extension updates and requests a larger capability set, the runtime
    /// drops the new caps until the user re-approves.
    #[serde(default)]
    pub approved_version: String,
    /// ISO 8601 timestamp of the approval.
    #[serde(default)]
    pub approved_at: String,
}

impl ToolPolicy {
    /// Check if this tool's grant is still valid.
    /// Returns the effective policy considering expiry and staleness.
    pub fn effective_policy(&self) -> PolicyKind {
        match self.policy {
            PolicyKind::Deny => PolicyKind::Deny,
            PolicyKind::Ask => PolicyKind::Ask,
            PolicyKind::Allow => match self.grant_type {
                GrantType::Always => {
                    // Check 30-day staleness. If the stored timestamp is in the
                    // future (clock skew), treat the grant as suspicious and
                    // re-prompt rather than silently honouring it forever.
                    if let Ok(last) = chrono::DateTime::parse_from_rfc3339(&self.last_seen) {
                        let delta = chrono::Utc::now() - last.with_timezone(&chrono::Utc);
                        if delta < chrono::Duration::zero() || delta.num_days() > 30 {
                            return PolicyKind::Ask;
                        }
                    }
                    PolicyKind::Allow
                }
                GrantType::Hours24 => {
                    // Check if granted_at is within 24 hours AND not in the future.
                    // A negative delta would previously satisfy `hours < 24` and
                    // keep the permission indefinitely-granted whenever the clock
                    // was ever set forward and then corrected back.
                    if let Ok(granted) = chrono::DateTime::parse_from_rfc3339(&self.granted_at) {
                        let delta = chrono::Utc::now() - granted.with_timezone(&chrono::Utc);
                        if delta >= chrono::Duration::zero() && delta.num_hours() < 24 {
                            return PolicyKind::Allow;
                        }
                    }
                    PolicyKind::Ask // expired or future-dated
                }
                GrantType::Once => {
                    // "once" — already consumed, back to ask.
                    PolicyKind::Ask
                }
            },
        }
    }
}
