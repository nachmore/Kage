//! Consent binding: the active agent connection's fingerprint, its label,
//! where it sends text, and the access state
//! (docs/design/EXTENSION_AGENT_CALLS.md §2, §4.3, §5.8).

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::agent_presets::AgentKind;
use crate::config::{AcpMode, AgentConnection, Config, ExtensionAgentAccess};

/// Gates 3 and 4 for one extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// The host switch is off.
    Off,
    /// Switched on, but no consent is recorded (a hand-edited config).
    NeedsConsent,
    /// The consent names another agent connection.
    ConsentStale,
    On,
}

/// Hex SHA-256 of what decides where text goes: the connection's `id`,
/// `preset_id`, mode type, `spawn_command` (local) or `host:port`
/// (remote), and the Ollama `base_url`, joined with `\0`. The model is
/// left out: a model change on the same agent doesn't move the text.
pub fn fingerprint(conn: &AgentConnection) -> String {
    let (mode, target) = match &conn.mode {
        AcpMode::Local { spawn_command } => ("local", spawn_command.clone()),
        AcpMode::Remote { host, port, .. } => ("remote", format!("{host}:{port}")),
    };
    let fields = [
        conn.id.as_str(),
        conn.preset_id.as_deref().unwrap_or(""),
        mode,
        target.as_str(),
        conn.ollama_settings
            .as_ref()
            .map_or("", |o| o.base_url.as_str()),
    ];
    hex::encode(Sha256::digest(fields.join("\0").as_bytes()))
}

/// The fingerprint of the active connection, if there is one.
pub fn active_fingerprint(config: &Config) -> Option<String> {
    config.acp.active_connection().map(fingerprint)
}

/// The connection's display name, with the preset's in brackets when they
/// differ: "Work agent (Claude Code)".
pub fn agent_label(conn: &AgentConnection) -> String {
    let preset = conn
        .preset_id
        .as_deref()
        .and_then(AgentKind::from_id)
        .map(|k| k.preset().display_name);
    let name = conn.name.trim();
    match (name.is_empty(), preset) {
        (false, Some(p)) if p != name => format!("{name} ({p})"),
        (false, _) => name.to_string(),
        (true, Some(p)) => p.to_string(),
        (true, None) => conn.id.clone(),
    }
}

/// Where the active agent sends what it receives, for the consent dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Location {
    pub kind: LocationKind,
    /// The other computer, for [`LocationKind::Remote`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationKind {
    /// A Kiro, Claude Code or Codex preset run locally: it sends what it
    /// receives to its online service.
    Cloud,
    /// An Ollama preset with a loopback base URL.
    Local,
    /// A remote TCP agent, or Ollama on another computer.
    Remote,
    /// Kage can't tell.
    Unknown,
}

fn is_loopback_host(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost")
        || h.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The host part of a base URL such as `http://127.0.0.1:11434`.
fn url_host(base_url: &str) -> Option<String> {
    url::Url::parse(base_url.trim())
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

pub fn location(conn: &AgentConnection) -> Location {
    let unknown = Location {
        kind: LocationKind::Unknown,
        host: None,
    };
    let kind = conn.preset_id.as_deref().and_then(AgentKind::from_id);
    if kind == Some(AgentKind::Ollama) {
        let Some(host) = conn
            .ollama_settings
            .as_ref()
            .and_then(|o| url_host(&o.base_url))
        else {
            return unknown;
        };
        return if is_loopback_host(&host) {
            Location {
                kind: LocationKind::Local,
                host: None,
            }
        } else {
            Location {
                kind: LocationKind::Remote,
                host: Some(host),
            }
        };
    }
    match &conn.mode {
        // A TCP agent on this computer could send its text anywhere.
        AcpMode::Remote { host, .. } if is_loopback_host(host) => unknown,
        AcpMode::Remote { host, .. } => Location {
            kind: LocationKind::Remote,
            host: Some(host.clone()),
        },
        AcpMode::Local { .. } => match kind {
            Some(AgentKind::Kiro | AgentKind::ClaudeCode | AgentKind::Codex) => Location {
                kind: LocationKind::Cloud,
                host: None,
            },
            _ => unknown,
        },
    }
}

/// Gates 3 and 4 from the stored entry and the current fingerprint.
pub fn access_state(access: Option<&ExtensionAgentAccess>, current: Option<&str>) -> Access {
    let Some(a) = access.filter(|a| a.enabled) else {
        return Access::Off;
    };
    match &a.consent {
        None => Access::NeedsConsent,
        Some(c) if c.agent_fingerprint.is_empty() => Access::NeedsConsent,
        Some(c) if Some(c.agent_fingerprint.as_str()) == current => Access::On,
        Some(_) => Access::ConsentStale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentConsent, OllamaConnectionSettings};

    fn local(preset: Option<&str>, cmd: &str) -> AgentConnection {
        AgentConnection {
            id: "c1".into(),
            name: "Claude Code".into(),
            preset_id: preset.map(str::to_string),
            mode: AcpMode::Local {
                spawn_command: cmd.into(),
            },
            sessions_directory: None,
            ollama_settings: None,
        }
    }

    fn ollama(url: &str) -> AgentConnection {
        let mut c = local(Some("ollama"), "codex-acp");
        c.ollama_settings = Some(OllamaConnectionSettings {
            base_url: url.into(),
            model: "llama3".into(),
            show_status_widget: false,
        });
        c
    }

    #[test]
    fn fingerprint_changes_with_each_field_but_not_the_model() {
        let base = ollama("http://127.0.0.1:11434");
        let fp = fingerprint(&base);
        assert_eq!(fp.len(), 64);

        let mut c = base.clone();
        c.id = "c2".into();
        assert_ne!(fingerprint(&c), fp);
        let mut c = base.clone();
        c.preset_id = Some("codex".into());
        assert_ne!(fingerprint(&c), fp);
        let mut c = base.clone();
        c.mode = AcpMode::Local {
            spawn_command: "codex-acp --other".into(),
        };
        assert_ne!(fingerprint(&c), fp);
        let mut c = base.clone();
        c.mode = AcpMode::Remote {
            host: "127.0.0.1".into(),
            port: 8765,
            timeout_ms: 1,
        };
        let remote = fingerprint(&c);
        assert_ne!(remote, fp);
        if let AcpMode::Remote { port, .. } = &mut c.mode {
            *port = 8766;
        }
        assert_ne!(fingerprint(&c), remote);
        let mut c = base.clone();
        c.ollama_settings.as_mut().unwrap().base_url = "http://10.0.0.2:11434".into();
        assert_ne!(fingerprint(&c), fp);

        // Not the model, the name, the sessions folder or the timeout.
        let mut c = base.clone();
        c.ollama_settings.as_mut().unwrap().model = "mistral".into();
        c.name = "Renamed".into();
        c.sessions_directory = Some("/tmp/x".into());
        assert_eq!(fingerprint(&c), fp);
    }

    #[test]
    fn labels() {
        assert_eq!(agent_label(&local(Some("claude-code"), "x")), "Claude Code");
        let mut c = local(Some("claude-code"), "x");
        c.name = "Work agent".into();
        assert_eq!(agent_label(&c), "Work agent (Claude Code)");
        c.name = "  ".into();
        assert_eq!(agent_label(&c), "Claude Code");
        c.preset_id = None;
        assert_eq!(agent_label(&c), "c1");
        c.name = "Mine".into();
        assert_eq!(agent_label(&c), "Mine");
    }

    #[test]
    fn locations() {
        for preset in ["kiro", "claude-code", "codex"] {
            assert_eq!(
                location(&local(Some(preset), "x")).kind,
                LocationKind::Cloud
            );
        }
        assert_eq!(
            location(&local(None, "my-agent")).kind,
            LocationKind::Unknown
        );
        assert_eq!(
            location(&ollama("http://localhost:11434")).kind,
            LocationKind::Local
        );
        assert_eq!(
            location(&ollama("http://[::1]:11434")).kind,
            LocationKind::Local
        );
        assert_eq!(
            location(&ollama("http://gpu-box.lan:11434")),
            Location {
                kind: LocationKind::Remote,
                host: Some("gpu-box.lan".into())
            }
        );
        assert_eq!(location(&ollama("not a url")).kind, LocationKind::Unknown);
        let mut r = local(Some("kiro"), "x");
        r.mode = AcpMode::Remote {
            host: "10.1.2.3".into(),
            port: 9000,
            timeout_ms: 1,
        };
        assert_eq!(
            location(&r),
            Location {
                kind: LocationKind::Remote,
                host: Some("10.1.2.3".into())
            }
        );
        r.mode = AcpMode::Remote {
            host: "127.0.0.1".into(),
            port: 9000,
            timeout_ms: 1,
        };
        assert_eq!(location(&r).kind, LocationKind::Unknown);
    }

    #[test]
    fn access_states() {
        let fp = "abc";
        assert_eq!(access_state(None, Some(fp)), Access::Off);
        let mut a = ExtensionAgentAccess::default();
        assert_eq!(access_state(Some(&a), Some(fp)), Access::Off);
        a.enabled = true;
        assert_eq!(access_state(Some(&a), Some(fp)), Access::NeedsConsent);
        a.consent = Some(AgentConsent {
            agent_fingerprint: "abc".into(),
            agent_label: "Agent".into(),
            granted_at: "2026-11-09T10:00:00Z".into(),
        });
        assert_eq!(access_state(Some(&a), Some(fp)), Access::On);
        assert_eq!(access_state(Some(&a), Some("other")), Access::ConsentStale);
        assert_eq!(access_state(Some(&a), None), Access::ConsentStale);
        a.consent.as_mut().unwrap().agent_fingerprint.clear();
        assert_eq!(access_state(Some(&a), Some(fp)), Access::NeedsConsent);
    }
}
