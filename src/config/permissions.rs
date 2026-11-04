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
    /// ACP tool *kinds* the user trusts as a whole category (today only
    /// `"execute"` — "trust all CLI commands"). Per-tool policies are keyed
    /// by title, and an agent's shell tool titles every call with the
    /// command itself ("Running: git status"), so a per-tool "Always allow"
    /// only ever covered one exact command string.
    #[serde(default)]
    pub trusted_kinds: Vec<String>,
    /// MCP servers the user trusts as a whole ("Trust all tools from
    /// <server>"): every tool the server has now or adds later. Per-tool
    /// policies are keyed by title, which for an MCP call names one tool
    /// ("Running: @builder-mcp/InternalSearch"), so "Always allow" only
    /// ever covered that one tool. Exact, case-sensitive names as parsed by
    /// [`mcp_server_from_title`].
    #[serde(default)]
    pub trusted_mcp_servers: Vec<String>,
}

/// ACP `ToolKind` for running shell / CLI commands.
pub const TOOL_KIND_EXECUTE: &str = "execute";

/// Kinds a user may trust as a category (see `trusted_kinds`). Anything
/// else is rejected so a hand-edited or hostile value can't widen trust.
pub const TRUSTABLE_TOOL_KINDS: &[&str] = &[TOOL_KIND_EXECUTE];

/// Longest MCP server name Kage will trust. Real names are short
/// (`builder-mcp`, `kage-computer-control`); the bound keeps a hostile
/// title from bloating config, the prompt and the audit log.
pub const MAX_MCP_SERVER_NAME_LEN: usize = 64;

/// Longest MCP tool name accepted when parsing a title (the MCP spec
/// recommends 1–128 characters).
const MAX_MCP_TOOL_NAME_LEN: usize = 128;

/// kiro-cli titles MCP tool calls `Running: @<server>/<tool>`.
const KIRO_MCP_TITLE_PREFIX: &str = "Running: @";

/// Claude Code names MCP tools `mcp__<server>__<tool>`, and its ACP adapter
/// uses that name as the tool call's title.
const CLAUDE_MCP_NAME_PREFIX: &str = "mcp__";

/// Claude Code's ACP adapter runs its own built-in file and terminal tools
/// through an in-process MCP server of this name (`mcp__acp__Edit`,
/// `mcp__acp__Bash`, …). Those are the agent's core tools, not a
/// third-party server, so they never count as one.
const CLAUDE_ADAPTER_BUILTIN_SERVER: &str = "acp";

/// Whether `name` may be trusted as an MCP server: 1–64 ASCII letters,
/// digits, `-`, `_` or `.`. No whitespace, slashes, `@` or non-ASCII (no
/// look-alike characters), so a parsed name is a plain identifier the
/// prompt can show unambiguously.
pub fn is_valid_mcp_server_name(name: &str) -> bool {
    is_mcp_identifier(name, MAX_MCP_SERVER_NAME_LEN)
}

fn is_mcp_identifier(s: &str, max_len: usize) -> bool {
    !s.is_empty()
        && s.len() <= max_len
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// The MCP server named by a tool call title, when the title is exactly an
/// agent's MCP tool form and the server is unambiguous:
///
/// - kiro-cli: `Running: @<server>/<tool>`
/// - Claude Code: `mcp__<server>__<tool>` (one `__` separator; a run of
///   three or more underscores, or a second separator, could split more
///   than one way and is refused)
///
/// Server and tool must both be plain identifiers, so anything with
/// spaces, extra slashes or trailing text is not MCP. Callers must also
/// check the tool kind: kiro-cli titles shell runs `Running: <command>`,
/// so a command spelled `@foo/bar` produces the same title — see
/// `classify_mcp_server` in the permission handler.
pub fn mcp_server_from_title(title: &str) -> Option<&str> {
    if let Some(rest) = title.strip_prefix(KIRO_MCP_TITLE_PREFIX) {
        let (server, tool) = rest.split_once('/')?;
        return (is_valid_mcp_server_name(server)
            && is_mcp_identifier(tool, MAX_MCP_TOOL_NAME_LEN))
        .then_some(server);
    }
    if let Some(rest) = title.strip_prefix(CLAUDE_MCP_NAME_PREFIX) {
        if rest.matches("__").count() != 1 || rest.contains("___") {
            return None;
        }
        let (server, tool) = rest.split_once("__")?;
        if server == CLAUDE_ADAPTER_BUILTIN_SERVER {
            return None;
        }
        return (is_valid_mcp_server_name(server)
            && is_mcp_identifier(tool, MAX_MCP_TOOL_NAME_LEN))
        .then_some(server);
    }
    None
}

impl ToolPermissionsConfig {
    /// Whether every tool from this MCP server is trusted. A name that
    /// isn't a valid server name (hand-edited config) never matches.
    pub fn is_mcp_server_trusted(&self, server: Option<&str>) -> bool {
        match server {
            Some(s) => {
                is_valid_mcp_server_name(s) && self.trusted_mcp_servers.iter().any(|t| t == s)
            }
            None => false,
        }
    }

    /// Trust or untrust an MCP server. `Some(changed)`, or `None` when
    /// asked to trust a name that isn't a valid server name (refused).
    /// Untrusting removes any entry with that exact name — including junk
    /// from a hand-edited config — so Settings can always clear it.
    pub fn set_mcp_server_trusted(&mut self, server: &str, trusted: bool) -> Option<bool> {
        let has = self.trusted_mcp_servers.iter().any(|t| t == server);
        if trusted {
            if !is_valid_mcp_server_name(server) {
                return None;
            }
            if has {
                return Some(false);
            }
            self.trusted_mcp_servers.push(server.to_string());
            return Some(true);
        }
        if has {
            self.trusted_mcp_servers.retain(|t| t != server);
        }
        Some(has)
    }

    /// Whether every request of this ACP tool kind is trusted.
    pub fn is_kind_trusted(&self, kind: Option<&str>) -> bool {
        match kind {
            Some(k) => {
                TRUSTABLE_TOOL_KINDS.contains(&k) && self.trusted_kinds.iter().any(|t| t == k)
            }
            None => false,
        }
    }

    /// Trust or untrust a kind. `Some(changed)`, or `None` for a kind that
    /// can't be trusted (refused).
    pub fn set_kind_trusted(&mut self, kind: &str, trusted: bool) -> Option<bool> {
        if !TRUSTABLE_TOOL_KINDS.contains(&kind) {
            return None;
        }
        let has = self.trusted_kinds.iter().any(|t| t == kind);
        match (trusted, has) {
            (true, false) => {
                self.trusted_kinds.push(kind.to_string());
                Some(true)
            }
            (false, true) => {
                self.trusted_kinds.retain(|t| t != kind);
                Some(true)
            }
            _ => Some(false),
        }
    }

    /// Resolve the effective policy for a tool by title.
    ///
    /// An explicit per-tool policy is consulted FIRST and always wins — in
    /// particular an explicit `Deny` is honoured even under `trust_all` /
    /// `terminator_mode`. The blanket-allow modes only upgrade a tool that
    /// would otherwise be `Ask` (or has no recorded policy). This matches the
    /// contract in docs/TOOL_PERMISSIONS.md: "allow everything except explicit
    /// deny" — not "allow everything, period".
    ///
    /// `kind` is the request's ACP tool kind, if known: a trusted kind (see
    /// `trusted_kinds`) acts like a blanket allow for that category, and an
    /// explicit per-tool `Deny` still wins over it. `mcp_server` is the MCP
    /// server the tool belongs to, if identified: a trusted server (see
    /// `trusted_mcp_servers`) is a blanket allow for its tools, again below
    /// an explicit per-tool `Deny`.
    pub fn resolve_policy_for(
        &self,
        tool_title: &str,
        kind: Option<&str>,
        mcp_server: Option<&str>,
    ) -> PolicyKind {
        let explicit = self
            .tools
            .iter()
            .find(|t| t.title == tool_title)
            .map(|t| t.effective_policy());
        let blanket_allow = self.terminator_mode
            || self.trust_all
            || self.is_kind_trusted(kind)
            || self.is_mcp_server_trusted(mcp_server);
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
