//! What sync does with every field of `config.json` (docs/design/SYNC.md
//! §4.3) and every list in it (§4.4), as code.
//!
//! Every field of [`Config`](crate::config::Config) has a row in
//! [`SYNC_FIELDS`], in the style of `config_export::allowlist::FIELDS`; a
//! field nobody decided on fails `every_config_field_has_a_sync_decision`.
//! A new `Config` field needs a row here as well as in the allowlist, in
//! the same commit (§4.7, §23).
//!
//! A row covers everything under its path. `*` is any map key (an
//! extension id). For a `Shared` or `PerOs` row, the path is the record id
//! in the `cfg` namespace, so a hotkey is one record: its modifiers and
//! key change together.

use super::types::Category;

/// What sync does with one field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// One value for every computer. It can be overridden for one OS or
    /// for this computer.
    Shared,
    /// Synced, but a new value applies to this OS by default. It can be
    /// made shared.
    PerOs,
    /// Never leaves this computer and can't be changed to sync. Settings
    /// shows the reason.
    ThisComputer(Why),
    /// Not synced and not shown: the schema, caches, markers and window
    /// geometry.
    Never,
    /// A list merged item by item and field by field: the namespace of its
    /// [`ListSpec`] in [`SYNC_LISTS`].
    List(&'static str),
    /// `extensions.<id>.<field>`: per field, by the §12.1 rule.
    ExtensionSettings,
}

/// Why a field stays on this computer. Each has a reason string, shown as
/// "Not synced. {reason}".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Why {
    /// It names a file Kage writes. A synced value never decides where
    /// Kage writes (§11.7).
    WritesFile,
    /// It decides whether this computer's shell start-up files run.
    ShellFiles,
    /// It mirrors an OS login item. A new computer must not gain one
    /// silently.
    LoginItem,
    /// A workaround for one Mac's macOS release.
    MacWorkaround,
    /// Troubleshooting switches.
    Diagnostics,
    /// What the agent and extensions may do without asking. A grant on one
    /// computer must not grant power on another.
    Permissions,
    /// Consent, given on each computer.
    Consent,
    /// Each computer runs its own welcome.
    FirstRun,
    /// The local voice server installed on this computer, and the program
    /// it starts.
    LocalVoice,
}

impl Why {
    /// The `settings.sync_scope.reason.*` key for this reason.
    pub const fn reason_key(self) -> &'static str {
        match self {
            Why::WritesFile => "settings.sync_scope.reason.writes_file",
            Why::ShellFiles => "settings.sync_scope.reason.shell_files",
            Why::LoginItem => "settings.sync_scope.reason.login_item",
            Why::MacWorkaround => "settings.sync_scope.reason.mac_workaround",
            Why::Diagnostics => "settings.sync_scope.reason.diagnostics",
            Why::Permissions => "settings.sync_scope.reason.permissions",
            Why::Consent => "settings.sync_scope.reason.consent",
            Why::FirstRun => "settings.sync_scope.reason.first_run",
            Why::LocalVoice => "settings.sync_scope.reason.local_voice",
        }
    }
}

/// One field of `config.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// Dotted path; `*` is any map key.
    pub path: &'static str,
    pub rule: Rule,
    /// The Settings → Sync category it syncs with. `None` when it doesn't
    /// sync.
    pub category: Option<Category>,
    /// The value can start a program on its own or turn an extension on,
    /// so it waits for approval on each computer (§8.10).
    pub guard: bool,
    /// Not published when the value looks like it holds a credential; it
    /// stays here as a `held_back` pin (§4.3).
    pub held_back: bool,
}

impl Row {
    const fn new(path: &'static str, rule: Rule, category: Option<Category>) -> Self {
        Self {
            path,
            rule,
            category,
            guard: false,
            held_back: false,
        }
    }

    const fn guarded(mut self) -> Self {
        self.guard = true;
        self
    }

    const fn held_back(mut self) -> Self {
        self.held_back = true;
        self
    }
}

const fn shared(path: &'static str, category: Category) -> Row {
    Row::new(path, Rule::Shared, Some(category))
}

const fn per_os(path: &'static str, category: Category) -> Row {
    Row::new(path, Rule::PerOs, Some(category))
}

const fn here(path: &'static str, why: Why) -> Row {
    Row::new(path, Rule::ThisComputer(why), None)
}

const fn never(path: &'static str) -> Row {
    Row::new(path, Rule::Never, None)
}

const fn list(path: &'static str, ns: &'static str, category: Category) -> Row {
    Row::new(path, Rule::List(ns), Some(category))
}

use Category::{Automations, Extensions, Settings};
use Why::*;

/// Every field of `Config`, in the order of the structs in `src/config.rs`
/// and `src/config/*.rs` (the order of `config_export::allowlist::FIELDS`).
pub const SYNC_FIELDS: &[Row] = &[
    // The file's schema: config_migrations owns it.
    never("version"),
    // Hotkeys differ by OS (⌘ against Ctrl, OS-reserved combos).
    per_os("hotkey", Settings),
    // Agents: the spawn command is reviewed on each computer.
    list("acp.connections", "list.connections", Settings)
        .guarded()
        .held_back(),
    // A connection whose spawn command isn't approved here isn't made
    // active.
    per_os("acp.active_connection_id", Settings).guarded(),
    shared("acp.agent.start_session_on_launch", Settings),
    shared("acp.agent.auto_steering_enabled", Settings),
    here("acp.agent.user_steering_path", WritesFile),
    // A model id is not a credential.
    shared("acp.agent.default_model", Settings),
    per_os("acp.agent.working_directory", Settings),
    shared("acp.agent.auto_compact_threshold", Settings),
    here("acp.agent.import_login_shell_env", ShellFiles),
    // Appearance. A theme that isn't installed here falls back as today.
    shared("ui.theme", Settings),
    shared("ui.floating_window_opacity", Settings),
    never("ui.chat_window_width"),
    never("ui.chat_window_height"),
    never("ui.chat_window_x"),
    never("ui.chat_window_y"),
    shared("ui.preserve_last_response", Settings),
    shared("ui.window_start_position", Settings),
    never("ui.last_window_x"),
    never("ui.last_window_y"),
    shared("ui.font_size", Settings),
    shared("ui.show_time", Settings),
    shared("ui.show_date", Settings),
    shared("ui.show_speech_button", Settings),
    shared("ui.speech_read_back", Settings),
    shared("ui.show_response_actions", Settings),
    shared("ui.show_floating_toolbar", Settings),
    shared("ui.remember_launcher_size", Settings),
    never("ui.launcher_width"),
    never("ui.launcher_height"),
    shared("ui.speech_silence_timeout", Settings),
    // Each OS has its own voices.
    per_os("ui.speech_voice", Settings),
    shared("ui.time_format", Settings),
    shared("ui.date_format", Settings),
    shared("ui.language", Settings),
    shared("ui.mascot_animations", Settings),
    shared("ui.mascot_extension_hints", Settings),
    here("ui.macos_overlay_panels", MacWorkaround),
    // System.
    here("system.auto_start", LoginItem),
    shared("system.capture_selection", Settings),
    // Process names differ by OS.
    per_os("system.capture_selection_blocklist", Settings),
    shared("system.show_notifications", Settings),
    shared("system.screen_context", Settings),
    here("system.log_buffer_size", Diagnostics),
    here("system.verbose_frontend_logging", Diagnostics),
    here("system.log_message_content", Diagnostics),
    never("system.last_seen_crash_timestamp"),
    list("shortcuts", "list.shortcuts", Automations),
    here("debug_mode", Diagnostics),
    here("tool_permissions", Permissions),
    here("first_run_completed", FirstRun),
    // Updates. A dev channel on one computer only is "Only on this
    // computer".
    shared("updates.auto_check", Settings),
    shared("updates.silent_update", Settings),
    never("updates.last_check_time"),
    never("updates.last_updated_version"),
    shared("updates.channel", Settings),
    shared("quick_actions.enabled", Automations),
    shared("quick_actions.translate_language", Automations),
    list(
        "quick_actions.custom_actions",
        "list.quick_actions",
        Automations,
    ),
    // Extensions. Credentials never sync.
    Row::new("extensions", Rule::ExtensionSettings, Some(Extensions)),
    // Switched on here only if this computer's grant covers the manifest.
    shared("extension_states.*", Extensions).guarded(),
    // Including `origin` (§4.5): consent is given per computer.
    here("extension_grants", Permissions),
    // EXTENSION_AGENT_CALLS §10: consent and allowance, per computer.
    here("extension_agent_access", Consent),
    // Local voice: the server is installed per computer; how it speaks is
    // a preference.
    here("pocket_tts.enabled", LocalVoice),
    shared("pocket_tts.voice", Settings),
    here("pocket_tts.port", LocalVoice),
    here("pocket_tts.python_path", LocalVoice),
    here("pocket_tts.installed", LocalVoice),
    here("pocket_tts.auto_start", LocalVoice),
    shared("pocket_tts.temp", Settings),
    shared("pocket_tts.eos_threshold", Settings),
    per_os("clipboard_hotkey", Settings),
    per_os("inline_assist_hotkey", Settings),
    per_os("voice_hotkey", Settings),
    // `config_export::sensitive::looks_like_secret` holds it back.
    shared("store_url", Extensions).held_back(),
    list("store_sources", "list.store_sources", Extensions).held_back(),
    here("mcp_config_path", WritesFile),
    shared("auto_update_extensions", Extensions),
    never("last_extension_update_check"),
    // Non-manual triggers wait for approval on each computer.
    list("macros", "list.macros", Automations).guarded(),
    shared("automation_power.mode", Automations),
    shared("automation_power.battery_multiplier", Automations),
    shared("automation_power.low_battery_multiplier", Automations),
    shared(
        "automation_power.disable_signals_on_low_battery",
        Automations,
    ),
    // Consent and the install id are per install.
    here("telemetry", Consent),
    // Matching is by tokens of the process name, which tolerates OS naming.
    list("context_rules", "list.app_modes", Settings),
    // Proxy scripts wait for approval on each computer.
    list("internal_sites", "list.internal_sites", Settings)
        .guarded()
        .held_back(),
];

/// Where a new list item exists by default (its `avail` cell).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Avail {
    /// On every computer.
    All,
    /// Only on the OS it was made on.
    MadeOn,
    /// On every computer, except items that run a program
    /// (`action_type: run_program`), which stay on the OS they were made on.
    ProgramsMadeOn,
}

/// One list of `config.json` (§4.4). Fields not named here are shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListSpec {
    pub ns: &'static str,
    /// The list's path in `config.json`.
    pub path: &'static str,
    pub category: Category,
    /// The item field holding its stable id.
    pub id_field: &'static str,
    pub avail: Avail,
    pub per_os: &'static [&'static str],
    /// Item fields that never leave this computer.
    pub this_computer: &'static [&'static str],
    /// Item fields that wait for approval on each computer.
    pub guarded: &'static [&'static str],
    /// Item fields not published when they look like they hold a key.
    pub held_back: &'static [&'static str],
}

const NONE: &[&str] = &[];

/// Every list of `config.json`, in the order of §4.4.
pub const SYNC_LISTS: &[ListSpec] = &[
    ListSpec {
        ns: "list.shortcuts",
        path: "shortcuts",
        category: Automations,
        id_field: "sync_id",
        avail: Avail::ProgramsMadeOn,
        per_os: &["shortcut", "path", "working_directory", "arguments"],
        this_computer: NONE,
        guarded: NONE,
        held_back: NONE,
    },
    ListSpec {
        ns: "list.macros",
        path: "macros",
        category: Automations,
        id_field: "sync_id",
        avail: Avail::All,
        per_os: NONE,
        this_computer: &["trigger.last_run"],
        // A non-manual trigger together with its steps.
        guarded: &["trigger", "steps"],
        held_back: NONE,
    },
    ListSpec {
        ns: "list.quick_actions",
        path: "quick_actions.custom_actions",
        category: Automations,
        id_field: "sync_id",
        avail: Avail::All,
        per_os: NONE,
        this_computer: NONE,
        guarded: NONE,
        held_back: NONE,
    },
    ListSpec {
        ns: "list.app_modes",
        path: "context_rules",
        category: Settings,
        id_field: "sync_id",
        avail: Avail::All,
        per_os: NONE,
        this_computer: NONE,
        guarded: NONE,
        held_back: NONE,
    },
    ListSpec {
        ns: "list.internal_sites",
        path: "internal_sites",
        category: Settings,
        id_field: "id",
        avail: Avail::All,
        per_os: &["proxy"],
        this_computer: NONE,
        guarded: &["proxy"],
        // When the script looks like it holds a key.
        held_back: &["proxy"],
    },
    ListSpec {
        ns: "list.connections",
        path: "acp.connections",
        category: Settings,
        id_field: "id",
        avail: Avail::MadeOn,
        per_os: &["mode", "sessions_directory"],
        this_computer: NONE,
        // The local spawn command.
        guarded: &["mode"],
        // When the spawn command looks like it holds a key.
        held_back: &["mode"],
    },
    ListSpec {
        ns: "list.store_sources",
        path: "store_sources",
        category: Extensions,
        id_field: "sync_id",
        avail: Avail::All,
        per_os: NONE,
        this_computer: NONE,
        guarded: NONE,
        held_back: &["url"],
    },
];

/// The row for `path`, matched exactly (`extension_states.*` for any
/// extension's on/off).
pub fn row(path: &str) -> Option<&'static Row> {
    SYNC_FIELDS.iter().find(|r| r.path == path)
}

/// The list with namespace `ns`.
pub fn list_spec(ns: &str) -> Option<&'static ListSpec> {
    SYNC_LISTS.iter().find(|l| l.ns == ns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use serde_json::{json, Value};

    /// A config with every optional field set and every map holding an
    /// entry, so its JSON names every field there is (as the allowlist's
    /// test builds it).
    fn complete_config() -> Config {
        let mut c = Config::default();
        let hk = || crate::config::HotkeyConfig {
            modifiers: vec!["Ctrl".into()],
            key: "K".into(),
        };
        c.clipboard_hotkey = Some(hk());
        c.inline_assist_hotkey = Some(hk());
        c.voice_hotkey = Some(hk());
        c.ui.chat_window_x = Some(1);
        c.ui.chat_window_y = Some(1);
        c.ui.last_window_x = Some(1);
        c.ui.last_window_y = Some(1);
        c.ui.launcher_width = Some(1);
        c.ui.launcher_height = Some(1);
        c.ui.speech_voice = Some("v".into());
        c.ui.language = Some("en".into());
        c.system.last_seen_crash_timestamp = Some("t".into());
        c.updates.last_check_time = Some("t".into());
        c.updates.last_updated_version = Some("1".into());
        c.quick_actions.translate_language = Some("English".into());
        c.quick_actions.custom_actions = vec![serde_json::from_value(json!({})).unwrap()];
        c.shortcuts = vec![serde_json::from_value(json!({})).unwrap()];
        c.macros = vec![serde_json::from_value(json!({})).unwrap()];
        c.tool_permissions.tools = vec![serde_json::from_value(json!({})).unwrap()];
        c.extensions
            .insert("weather".into(), json!({ "units": "metric" }));
        c.extension_states.insert("weather".into(), true);
        c.extension_grants.insert(
            "weather".into(),
            crate::config::ExtensionGrant {
                granted: vec!["storage".into()],
                approved_version: "1.0.0".into(),
                approved_at: "2026-01-01T00:00:00Z".into(),
            },
        );
        c.extension_agent_access.insert(
            "weather".into(),
            crate::config::ExtensionAgentAccess {
                enabled: true,
                consent: Some(crate::config::AgentConsent {
                    agent_fingerprint: "f".into(),
                    agent_label: "a".into(),
                    granted_at: "2026-01-01T00:00:00Z".into(),
                }),
                allowance: crate::config::AgentAllowance::High,
            },
        );
        c.pocket_tts.python_path = Some("p".into());
        c.store_url = Some("s".into());
        c.store_sources = vec![serde_json::from_value(json!({})).unwrap()];
        c.mcp_config_path = Some("m".into());
        c.last_extension_update_check = Some("t".into());
        c.telemetry.install_id = Some("i".into());
        c.telemetry.last_daily_ping = Some("t".into());
        c.telemetry.last_seen_version = Some("1".into());
        c.acp.agent.user_steering_path = Some("p".into());
        c.acp.agent.default_model = Some("m".into());
        c.acp.agent.working_directory = Some("w".into());
        c.internal_sites = vec![serde_json::from_value(json!({})).unwrap()];
        c
    }

    /// Every path the walk stops at: a row, or a leaf no row covers
    /// (returned in `undecided`).
    fn walk(
        v: &Value,
        path: &mut Vec<String>,
        seen: &mut Vec<String>,
        undecided: &mut Vec<String>,
    ) {
        let at = path.join(".");
        if !path.is_empty() && row(&at).is_some() {
            seen.push(at);
            return;
        }
        match v {
            Value::Object(o) if !o.is_empty() => {
                for (k, child) in o {
                    // Extension ids are the `*` segment.
                    let seg = if at == "extension_states" {
                        "*".to_string()
                    } else {
                        k.clone()
                    };
                    path.push(seg);
                    walk(child, path, seen, undecided);
                    path.pop();
                }
            }
            _ => undecided.push(at),
        }
    }

    #[test]
    fn every_config_field_has_a_sync_decision() {
        let tree = serde_json::to_value(complete_config()).unwrap();
        let (mut seen, mut undecided) = (Vec::new(), Vec::new());
        walk(&tree, &mut Vec::new(), &mut seen, &mut undecided);
        assert!(
            undecided.is_empty(),
            "Config fields with no sync decision: {:?}. Add each to \
             sync::catalog::SYNC_FIELDS (docs/design/SYNC.md §4.3).",
            undecided
        );
        let stale: Vec<&str> = SYNC_FIELDS
            .iter()
            .map(|r| r.path)
            .filter(|p| !seen.iter().any(|s| s == p))
            .collect();
        assert!(
            stale.is_empty(),
            "SYNC_FIELDS names fields Config doesn't have: {:?}",
            stale
        );
        let mut paths: Vec<&str> = SYNC_FIELDS.iter().map(|r| r.path).collect();
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(paths.len(), SYNC_FIELDS.len(), "a field is listed twice");
    }

    #[test]
    fn every_list_row_has_one_spec_and_back() {
        for r in SYNC_FIELDS {
            if let Rule::List(ns) = r.rule {
                let spec = list_spec(ns)
                    .unwrap_or_else(|| panic!("{}: no SYNC_LISTS entry for {}", r.path, ns));
                assert_eq!(spec.path, r.path, "{} names the list at another path", ns);
                assert_eq!(Some(spec.category), r.category, "{}: categories differ", ns);
            }
        }
        for spec in SYNC_LISTS {
            assert_eq!(
                row(spec.path).map(|r| r.rule),
                Some(Rule::List(spec.ns)),
                "{} has no List row",
                spec.ns
            );
        }
        let mut names: Vec<&str> = SYNC_LISTS.iter().map(|l| l.ns).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SYNC_LISTS.len(), "a list is listed twice");
    }

    #[test]
    fn rows_that_sync_have_a_category_and_others_none() {
        for r in SYNC_FIELDS {
            let syncs = !matches!(r.rule, Rule::ThisComputer(_) | Rule::Never);
            assert_eq!(r.category.is_some(), syncs, "{}: category", r.path);
            if !syncs {
                assert!(
                    !r.guard && !r.held_back,
                    "{}: guard or held back on a row that doesn't sync",
                    r.path
                );
            }
        }
    }

    /// A synced value never names a file Kage writes or a program it starts
    /// (§11.7), and consent never leaves the computer (§11.5).
    #[test]
    fn files_programs_and_consent_stay_on_this_computer() {
        for path in [
            "acp.agent.user_steering_path",
            "acp.agent.import_login_shell_env",
            "mcp_config_path",
            "pocket_tts.python_path",
            "system.auto_start",
            "tool_permissions",
            "extension_grants",
            "extension_agent_access",
            "telemetry",
            "first_run_completed",
        ] {
            assert!(
                matches!(row(path).map(|r| r.rule), Some(Rule::ThisComputer(_))),
                "{} must be This computer",
                path
            );
        }
    }

    #[test]
    fn every_reason_has_an_english_string() {
        crate::i18n::init(Some("en"));
        for r in SYNC_FIELDS {
            if let Rule::ThisComputer(why) = r.rule {
                let key = why.reason_key();
                assert_ne!(
                    crate::i18n::translate_in("en", key, &[]),
                    key,
                    "{} has no EN string",
                    key
                );
            }
        }
    }
}
