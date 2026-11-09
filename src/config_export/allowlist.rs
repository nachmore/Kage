//! What an unencrypted, settings-only backup may hold: an explicit
//! allowlist.
//!
//! Every field of [`Config`] has a decision in [`FIELDS`]: copied (with the
//! shape its value must have) or left out (and why). A settings-only backup
//! is built by walking that table, never the config, so a field nobody
//! decided on is left out; `every_config_field_has_a_decision` fails until
//! one is made. Extension settings are copied only where their manifest
//! says so ([`Rules::plain_setting`]); extension data never is.
//!
//! Import reads the same table: only the fields it copies are merged, each
//! re-checked against its shape, and an empty incoming value keeps this
//! machine's. Everything else on the machine is left as it is.

use super::sensitive::{self, is_host_managed, looks_like_secret, Plain, Rules};
use crate::config::Config;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::sync::LazyLock;

/// What a settings-only backup does with one field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rule {
    /// Copied when the value has this shape; anything else is left out.
    Include(Shape),
    /// Never in a settings-only backup.
    Leave(Why),
}

/// The values a number setting may hold: finite, of its type (an integer
/// where the config field is one), and within what the setting really
/// takes (its Settings control, or for one without a control what the
/// code that reads it can use). A value outside is **rejected**, never
/// clamped: left out of a backup (and named to the user), and ignored on
/// import so this machine's value stays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub min: f64,
    pub max: f64,
    /// The config field is an integer type (`u8`, `u32`).
    pub integer: bool,
}

impl Range {
    const fn ints(min: f64, max: f64) -> Self {
        Self {
            min,
            max,
            integer: true,
        }
    }

    const fn floats(min: f64, max: f64) -> Self {
        Self {
            min,
            max,
            integer: false,
        }
    }

    /// Whether `n` is a value of this setting. An `f32` field's value went
    /// through `f32` on its way to JSON (0.3 is 0.30000001…), so the bounds
    /// allow for that.
    fn holds(&self, n: &serde_json::Number) -> bool {
        if self.integer && n.as_u64().is_none() && n.as_i64().is_none() {
            return false;
        }
        let Some(v) = n.as_f64().filter(|v| v.is_finite()) else {
            return false;
        };
        const SLACK: f64 = 1e-6;
        v >= self.min - SLACK && v <= self.max + SLACK
    }
}

/// `acp.agent.auto_compact_threshold` (`u32`): a percentage, the Agent
/// settings slider's 0–100.
const COMPACT_THRESHOLD: Range = Range::ints(0.0, 100.0);
/// `ui.floating_window_opacity` (`f32`): the Appearance slider's 0.3–1.
const OPACITY: Range = Range::floats(0.3, 1.0);
/// `ui.font_size` (`u8`): the Appearance slider's 11–20 px.
const FONT_SIZE: Range = Range::ints(11.0, 20.0);
/// `ui.speech_silence_timeout` (`f32`): the Speech slider's 0–5 s.
const SILENCE_TIMEOUT: Range = Range::floats(0.0, 5.0);
/// `pocket_tts.temp` (`f32`): the Local voice slider's 0.3–1.
const TTS_TEMP: Range = Range::floats(0.3, 1.0);
/// `pocket_tts.eos_threshold` (`f32`): the Local voice slider's −8 to −1.
const TTS_EOS: Range = Range::floats(-8.0, -1.0);
/// `automation_power.*_multiplier` (`f32`): no control; each multiplies
/// scheduled automations' intervals on battery. Below 1 would run them
/// more often on battery than on power (0 or less: constantly); the
/// defaults are 2 and 4, and past 10 automations effectively stop.
const POWER_MULTIPLIER: Range = Range::floats(1.0, 10.0);

/// The values a copied field may hold. Free-form text has no shape here:
/// every string a settings-only backup copies comes from a closed set (a
/// list in this file, the languages Kage ships, the hotkey key names), or
/// from this machine's installed manifests (theme and extension ids,
/// versions), or is a timestamp — and is checked by [`looks_like_secret`]
/// as well.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Bool,
    /// A number in this range.
    Number(Range),
    /// One of these strings.
    OneOf(&'static [&'static str]),
    /// A built-in theme (`system`, `dark`, `light`) or an installed theme's
    /// id.
    Theme,
    /// A language Kage ships a catalog for (`en`, `pt`, `zh-CN`).
    Language,
    /// A whole hotkey (`{ "modifiers": ["Ctrl"], "key": "K" }`): its key
    /// and modifiers from closed lists, and together a shortcut the
    /// runtime's parser registers ([`hotkey_registers`]). Modifier aliases
    /// are written under the name the runtime reads the same way (`Cmd` →
    /// `Super`, `Option` → `Alt`).
    Hotkey,
    /// Capability names an extension was granted.
    Capabilities,
    /// The version of the installed item a grant was approved for.
    GrantVersion,
    /// An RFC 3339 timestamp.
    Timestamp,
    /// `config.extensions`: per field, as the extension's manifest allows.
    ExtensionSettings,
    /// `version`: the config schema, always written.
    SchemaVersion,
}

/// Why a field is left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// It can hold a password, key, script, prompt, address or path, or is
    /// one of the user's own documents (shortcuts, automations, agents…).
    Private,
    /// It belongs to this machine (window positions, paths, install id).
    DeviceLocal,
    /// A cache or timestamp that only triggers fresh checks.
    Cache,
    /// Troubleshooting switches.
    Diagnostics,
    /// What the agent and extensions may do without asking: a plain file
    /// mustn't grant it.
    Permissions,
}

const TIME_FORMATS: &[&str] = &["HH:mm", "HH:mm:ss", "h:mm A", "h:mm:ss A"];
const DATE_FORMATS: &[&str] = &[
    "ddd, MMM D",
    "dddd, MMMM D",
    "MMM D, YYYY",
    "D MMM YYYY",
    "YYYY-MM-DD",
    "MM/DD/YYYY",
    "DD/MM/YYYY",
];
const START_POSITIONS: &[&str] = &["center", "mouse", "remember"];
const CHANNELS: &[&str] = &["stable", "beta", "dev"];
const POWER_MODES: &[&str] = &["auto", "full", "saving"];
/// `ui/js/shared/theme.js` (`BUILTIN_THEMES`).
const BUILTIN_THEMES: &[&str] = &["system", "dark", "light"];
/// Local TTS's built-in voices (`pocket_tts_voices`); others are files on
/// one machine.
const POCKET_VOICES: &[&str] = &[
    "alba", "marius", "javert", "jean", "fantine", "cosette", "eponine", "azelma",
];

use Rule::{Include, Leave};
use Shape::*;
use Why::*;

/// Every field of [`Config`], by dotted path (`*` is any map key, which
/// must be a valid extension id), and what a settings-only backup does with
/// it. A rule covers everything under its path. Keep in the order of the
/// structs in `src/config.rs` and `src/config/*.rs`.
pub const FIELDS: &[(&str, Rule)] = &[
    ("version", Include(SchemaVersion)),
    // Launcher hotkey.
    ("hotkey", Include(Hotkey)),
    // Agent connections: commands, hosts, models, folders.
    ("acp.connections", Leave(Private)),
    ("acp.active_connection_id", Leave(Private)),
    ("acp.agent.start_session_on_launch", Include(Bool)),
    ("acp.agent.auto_steering_enabled", Include(Bool)),
    ("acp.agent.user_steering_path", Leave(DeviceLocal)),
    ("acp.agent.default_model", Leave(Private)),
    ("acp.agent.working_directory", Leave(DeviceLocal)),
    (
        "acp.agent.auto_compact_threshold",
        Include(Number(COMPACT_THRESHOLD)),
    ),
    ("acp.agent.import_login_shell_env", Leave(DeviceLocal)),
    // Appearance and windows.
    ("ui.theme", Include(Theme)),
    ("ui.floating_window_opacity", Include(Number(OPACITY))),
    ("ui.chat_window_width", Leave(DeviceLocal)),
    ("ui.chat_window_height", Leave(DeviceLocal)),
    ("ui.chat_window_x", Leave(DeviceLocal)),
    ("ui.chat_window_y", Leave(DeviceLocal)),
    ("ui.preserve_last_response", Include(Bool)),
    ("ui.window_start_position", Include(OneOf(START_POSITIONS))),
    ("ui.last_window_x", Leave(DeviceLocal)),
    ("ui.last_window_y", Leave(DeviceLocal)),
    ("ui.font_size", Include(Number(FONT_SIZE))),
    ("ui.show_time", Include(Bool)),
    ("ui.show_date", Include(Bool)),
    ("ui.show_speech_button", Include(Bool)),
    ("ui.speech_read_back", Include(Bool)),
    ("ui.show_response_actions", Include(Bool)),
    ("ui.show_floating_toolbar", Include(Bool)),
    ("ui.remember_launcher_size", Include(Bool)),
    ("ui.launcher_width", Leave(DeviceLocal)),
    ("ui.launcher_height", Leave(DeviceLocal)),
    (
        "ui.speech_silence_timeout",
        Include(Number(SILENCE_TIMEOUT)),
    ),
    // A system voice's name: each OS has its own voices.
    ("ui.speech_voice", Leave(DeviceLocal)),
    ("ui.time_format", Include(OneOf(TIME_FORMATS))),
    ("ui.date_format", Include(OneOf(DATE_FORMATS))),
    ("ui.language", Include(Language)),
    ("ui.mascot_animations", Include(Bool)),
    ("ui.mascot_extension_hints", Include(Bool)),
    // A per-Mac escape hatch for a misbehaving macOS release.
    ("ui.macos_overlay_panels", Leave(DeviceLocal)),
    // System integration and notifications.
    ("system.auto_start", Leave(DeviceLocal)),
    ("system.capture_selection", Include(Bool)),
    // Process names, typed by the user; they differ by OS anyway.
    ("system.capture_selection_blocklist", Leave(DeviceLocal)),
    ("system.show_notifications", Include(Bool)),
    ("system.screen_context", Include(Bool)),
    ("system.log_buffer_size", Leave(Diagnostics)),
    ("system.verbose_frontend_logging", Leave(Diagnostics)),
    ("system.log_message_content", Leave(Diagnostics)),
    ("system.last_seen_crash_timestamp", Leave(Cache)),
    // Program paths, arguments, scripts, URLs, prompts.
    ("shortcuts", Leave(Private)),
    ("debug_mode", Leave(Diagnostics)),
    ("tool_permissions", Leave(Permissions)),
    ("first_run_completed", Leave(DeviceLocal)),
    // Updates.
    ("updates.auto_check", Include(Bool)),
    ("updates.silent_update", Include(Bool)),
    ("updates.last_check_time", Leave(Cache)),
    ("updates.last_updated_version", Leave(Cache)),
    ("updates.channel", Include(OneOf(CHANNELS))),
    // Quick actions: custom ones are prompts.
    ("quick_actions.enabled", Include(Bool)),
    // Free text that goes into the translate prompt.
    ("quick_actions.translate_language", Leave(Private)),
    ("quick_actions.custom_actions", Leave(Private)),
    // Extensions: settings as their manifests allow, which are on, and
    // what they were granted.
    ("extensions", Include(ExtensionSettings)),
    ("extension_states.*", Include(Bool)),
    ("extension_grants.*.granted", Include(Capabilities)),
    ("extension_grants.*.approved_version", Include(GrantVersion)),
    ("extension_grants.*.approved_at", Include(Timestamp)),
    // Where the installed copy came from (folder sync, SYNC.md §4.5): this
    // machine's install.
    ("extension_grants.*.origin", Leave(DeviceLocal)),
    // Background AI switch, consent and allowance: consent named an agent
    // on this machine.
    ("extension_agent_access", Leave(Permissions)),
    // Local text-to-speech: the server is installed per machine; the voice
    // and how it speaks are preferences.
    ("pocket_tts.enabled", Leave(DeviceLocal)),
    ("pocket_tts.voice", Include(OneOf(POCKET_VOICES))),
    ("pocket_tts.port", Leave(DeviceLocal)),
    ("pocket_tts.python_path", Leave(DeviceLocal)),
    ("pocket_tts.installed", Leave(DeviceLocal)),
    ("pocket_tts.auto_start", Leave(DeviceLocal)),
    ("pocket_tts.temp", Include(Number(TTS_TEMP))),
    ("pocket_tts.eos_threshold", Include(Number(TTS_EOS))),
    // Other hotkeys.
    ("clipboard_hotkey", Include(Hotkey)),
    ("inline_assist_hotkey", Include(Hotkey)),
    ("voice_hotkey", Include(Hotkey)),
    // Store addresses can carry tokens.
    ("store_url", Leave(Private)),
    ("store_sources", Leave(Private)),
    ("mcp_config_path", Leave(DeviceLocal)),
    ("auto_update_extensions", Include(Bool)),
    ("last_extension_update_check", Leave(Cache)),
    // Automations: prompts and scripts.
    ("macros", Leave(Private)),
    ("automation_power.mode", Include(OneOf(POWER_MODES))),
    (
        "automation_power.battery_multiplier",
        Include(Number(POWER_MULTIPLIER)),
    ),
    (
        "automation_power.low_battery_multiplier",
        Include(Number(POWER_MULTIPLIER)),
    ),
    (
        "automation_power.disable_signals_on_low_battery",
        Include(Bool),
    ),
    // Telemetry: the opt-in only.
    ("telemetry.enabled", Include(Bool)),
    ("telemetry.install_id", Leave(DeviceLocal)),
    ("telemetry.consent_version", Leave(DeviceLocal)),
    ("telemetry.last_daily_ping", Leave(Cache)),
    ("telemetry.last_seen_version", Leave(Cache)),
    // App modes carry steering text; internal sites carry scripts.
    ("context_rules", Leave(Private)),
    ("internal_sites", Leave(Private)),
];

/// Key names a hotkey can have (`codeToKeyName` in
/// ui/js/shared/hotkey-capture.js, and the native capture paths): a letter,
/// a digit, `Num0`–`Num9`, `F1`–`F24`, or one of these.
const NAMED_KEYS: &[&str] = &[
    "Space",
    "Enter",
    "Tab",
    "Backspace",
    "Delete",
    "Insert",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Left",
    "Right",
    "Up",
    "Down",
    "Escape",
    "-",
    "=",
    "[",
    "]",
    "\\",
    ";",
    "'",
    ",",
    ".",
    "/",
    "`",
    "NumDecimal",
    "NumMultiply",
    "NumAdd",
    "NumSubtract",
    "NumDivide",
    "NumEnter",
    "NumEqual",
];
static KEY_NAME: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(?i:[A-Z0-9]|Num[0-9]|F(?:[1-9]|1[0-9]|2[0-4]))$").unwrap()
});
/// Modifier names the runtime's shortcut parser (`global-hotkey`, through
/// `tauri-plugin-global-shortcut`) reads, each with the name a backup
/// writes for it: one the parser reads the same way. Anything else
/// (`Meta`, `Win`, …) the parser refuses, so a hotkey using it never
/// registers and isn't copied.
const MODIFIERS: &[(&str, &str)] = &[
    ("ctrl", "Ctrl"),
    ("control", "Ctrl"),
    ("alt", "Alt"),
    ("option", "Alt"),
    ("shift", "Shift"),
    ("super", "Super"),
    ("cmd", "Super"),
    ("command", "Super"),
    // Super on macOS, Ctrl elsewhere: kept as it is.
    ("cmdorctrl", "CmdOrCtrl"),
    ("cmdorcontrol", "CmdOrCtrl"),
    ("commandorctrl", "CmdOrCtrl"),
    ("commandorcontrol", "CmdOrCtrl"),
];

/// Most modifiers a hotkey may list.
const MAX_MODIFIERS: usize = 6;

/// `modifier`'s name as a backup writes it, if the runtime reads it.
fn canonical_modifier(modifier: &str) -> Option<&'static str> {
    let lower = modifier.trim().to_ascii_lowercase();
    MODIFIERS
        .iter()
        .find(|(alias, _)| *alias == lower)
        .map(|(_, name)| *name)
}

/// Whether `key` is a hotkey key name from the closed list.
fn is_key_name(key: &str) -> bool {
    KEY_NAME.is_match(key) || NAMED_KEYS.iter().any(|k| k.eq_ignore_ascii_case(key))
}

/// Whether `modifiers` + `key` is a shortcut the runtime registers: the
/// same string `Config::get_hotkey_string` builds, read by the parser
/// `register_all_hotkeys` hands it to.
pub fn hotkey_registers(modifiers: &[String], key: &str) -> bool {
    use std::str::FromStr;
    let mut parts: Vec<&str> = modifiers.iter().map(String::as_str).collect();
    parts.push(key);
    tauri_plugin_global_shortcut::Shortcut::from_str(&parts.join("+")).is_ok()
}

/// A hotkey object (see [`Shape::Hotkey`]): its modifiers under the names
/// a backup writes, its key, and nothing else — or why not.
fn hotkey_check(v: &Value) -> Checked {
    let obj = match v {
        Value::Null => return Checked::Nothing,
        Value::Object(o) => o,
        _ => return Checked::Flagged,
    };
    let key = match obj.get("key") {
        None | Some(Value::Null) => return Checked::Nothing,
        Some(Value::String(s)) if s.trim().is_empty() => return Checked::Nothing,
        Some(Value::String(s)) if is_key_name(s) && !looks_like_secret(s) => s.clone(),
        Some(_) => return Checked::Flagged,
    };
    // A key comes with its own modifiers (none is a list too): without
    // them there's no telling which hotkey it is.
    let mods = match obj.get("modifiers") {
        None | Some(Value::Null) => return Checked::Nothing,
        Some(Value::Array(a)) if a.len() <= MAX_MODIFIERS => a,
        Some(_) => return Checked::Flagged,
    };
    let mut names: Vec<String> = Vec::new();
    for m in mods {
        let Some(name) = m
            .as_str()
            .filter(|s| !looks_like_secret(s))
            .and_then(canonical_modifier)
        else {
            return Checked::Flagged;
        };
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    if !hotkey_registers(&names, &key) {
        return Checked::Flagged;
    }
    Checked::Ok(serde_json::json!({ "modifiers": names, "key": key }))
}
static VERSION: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[0-9][0-9A-Za-z.+-]{0,63}$").unwrap());

/// Most items a copied list may hold.
const MAX_LIST: usize = 256;

/// Which way a value is being checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// Writing a backup: ids must be installed here.
    Export,
    /// Reading one: ids need only be well-formed (the extension may be
    /// installed later).
    Import,
}

/// What a shape check knows about this machine.
struct Ctx<'r> {
    rules: &'r Rules,
    side: Side,
}

impl Ctx<'_> {
    /// A map key (an extension id) a settings-only backup copies: a valid
    /// id, and on export one an installed item has.
    fn key_ok(&self, key: &str) -> bool {
        crate::extensions::validate_extension_id(key).is_ok()
            && !looks_like_secret(key)
            && (self.side == Side::Import || self.rules.is_installed_item(key))
    }
}

/// Whether `code` is a language Kage ships a catalog for.
fn is_shipped_language(code: &str) -> bool {
    crate::i18n::available_languages()
        .iter()
        .any(|(c, ..)| c.eq_ignore_ascii_case(code))
}

/// `true` for values that hold nothing: `null`, a blank string, `[]`, `{}`.
pub fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

/// What a shape check makes of a value.
#[derive(Debug, PartialEq)]
enum Checked {
    /// Copy this (possibly with unusable list items dropped).
    Ok(Value),
    /// Nothing there.
    Nothing,
    /// A value that isn't the shape: left out, and the user is told.
    Flagged,
}

fn string_check(v: &Value, ok: impl Fn(&str) -> bool) -> Checked {
    match v {
        Value::String(s) if s.trim().is_empty() => Checked::Nothing,
        Value::String(s) if ok(s) && !looks_like_secret(s) => Checked::Ok(v.clone()),
        Value::Null => Checked::Nothing,
        _ => Checked::Flagged,
    }
}

fn list_check(v: &Value, ok: impl Fn(&str) -> bool) -> Checked {
    let Value::Array(items) = v else {
        return if v.is_null() {
            Checked::Nothing
        } else {
            Checked::Flagged
        };
    };
    if items.is_empty() {
        return Checked::Ok(Value::Array(Vec::new()));
    }
    let kept: Vec<Value> = items
        .iter()
        .filter(|i| i.as_str().is_some_and(|s| ok(s) && !looks_like_secret(s)))
        .take(MAX_LIST)
        .cloned()
        .collect();
    if kept.is_empty() {
        Checked::Nothing
    } else {
        Checked::Ok(Value::Array(kept))
    }
}

fn check(shape: Shape, v: &Value, path: &[String], ctx: &Ctx) -> Checked {
    match shape {
        Bool => match v {
            Value::Bool(_) => Checked::Ok(v.clone()),
            _ => Checked::Nothing,
        },
        Number(range) => match v {
            Value::Number(n) if range.holds(n) => Checked::Ok(v.clone()),
            Value::Number(_) => Checked::Flagged,
            _ => Checked::Nothing,
        },
        OneOf(choices) => string_check(v, |s| choices.contains(&s)),
        Theme => string_check(v, |s| {
            BUILTIN_THEMES.contains(&s)
                || (crate::extensions::validate_extension_id(s).is_ok()
                    && (ctx.side == Side::Import || ctx.rules.is_installed_theme(s)))
        }),
        Language => string_check(v, is_shipped_language),
        Hotkey => hotkey_check(v),
        Capabilities => list_check(v, |s| crate::extensions::VALID_CAPABILITIES.contains(&s)),
        // A grant for another version is refused on import anyway.
        GrantVersion => match string_check(v, |s| match ctx.side {
            Side::Export => path.get(1).is_some_and(|id| ctx.rules.has_version(id, s)),
            Side::Import => VERSION.is_match(s),
        }) {
            Checked::Flagged => Checked::Nothing,
            other => other,
        },
        Timestamp => match string_check(v, |s| {
            s.len() <= 40 && chrono::DateTime::parse_from_rfc3339(s).is_ok()
        }) {
            Checked::Flagged => Checked::Nothing,
            other => other,
        },
        ExtensionSettings | SchemaVersion => Checked::Nothing,
    }
}

/// The shape a settings-only backup checks the value at `path` against,
/// when it copies that field. Folder sync validates a synced value with it
/// before applying it (docs/design/SYNC.md §8.11).
pub(crate) fn shape_of(path: &str) -> Option<Shape> {
    FIELDS.iter().find_map(|(p, rule)| match rule {
        Include(shape) if *p == path => Some(*shape),
        _ => None,
    })
}

/// Whether `v` is a value of `shape` that another computer may hand this
/// one: [`check`] as on import, so ids need only be well-formed. A value
/// that holds nothing is accepted (the setting's own default applies).
pub(crate) fn accepts(shape: Shape, v: &Value, rules: &Rules) -> bool {
    let ctx = Ctx {
        rules,
        side: Side::Import,
    };
    match check(shape, v, &[], &ctx) {
        Checked::Ok(_) => true,
        Checked::Nothing => is_empty(v),
        Checked::Flagged => false,
    }
}

/// Every value at `pattern` in `tree`, with its concrete path. A `*`
/// segment matches any key that [`Ctx::key_ok`] accepts.
fn matching<'v>(tree: &'v Value, pattern: &str, ctx: &Ctx) -> Vec<(Vec<String>, &'v Value)> {
    let mut out = vec![(Vec::new(), tree)];
    for seg in pattern.split('.') {
        let mut next = Vec::new();
        for (path, v) in out {
            let Some(obj) = v.as_object() else {
                continue;
            };
            if seg == "*" {
                for (k, child) in obj {
                    if ctx.key_ok(k) {
                        let mut p = path.clone();
                        p.push(k.clone());
                        next.push((p, child));
                    }
                }
            } else if let Some(child) = obj.get(seg) {
                let mut p = path.clone();
                p.push(seg.to_string());
                next.push((p, child));
            }
        }
        out = next;
    }
    out
}

/// Set `value` at `path` in `tree`, creating objects (also over a `null`)
/// along the way. A non-object in the way is left alone.
fn set(tree: &mut Value, path: &[String], value: Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut cur = tree;
    for seg in parents {
        if cur.is_null() {
            *cur = Value::Object(Map::new());
        }
        let Some(obj) = cur.as_object_mut() else {
            return;
        };
        cur = obj
            .entry(seg.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if cur.is_null() {
        *cur = Value::Object(Map::new());
    }
    if let Some(obj) = cur.as_object_mut() {
        obj.insert(last.clone(), value);
    }
}

/// The settings-only part of a backup of `config`.
pub struct SettingsOnly {
    /// `config.json` as the backup carries it: only allowlisted fields.
    pub tree: Value,
    /// How many settings it holds.
    pub settings: usize,
    /// Labels of allowlisted values left out because they didn't have
    /// their field's shape or looked like a secret.
    pub flagged: Vec<String>,
    /// How many extension settings were left out (no manifest allows them).
    pub extension_settings_left_out: usize,
}

/// Build the settings-only `config.json` for `config`.
pub fn extract(config: &Config, rules: &Rules, names: &ExtensionNames) -> SettingsOnly {
    // Serialising a Config can't fail: every map key is a string.
    let src = serde_json::to_value(super::exportable(config)).unwrap_or_default();
    let mut tree = Value::Object(Map::new());
    set(
        &mut tree,
        &["version".to_string()],
        Value::from(crate::config_migrations::CURRENT_VERSION),
    );
    let ctx = Ctx {
        rules,
        side: Side::Export,
    };
    let mut settings = 0usize;
    let mut flagged = Vec::new();
    for (pattern, rule) in FIELDS {
        let Include(shape) = rule else {
            continue;
        };
        if matches!(shape, ExtensionSettings | SchemaVersion) {
            continue;
        }
        for (path, v) in matching(&src, pattern, &ctx) {
            match check(*shape, v, &path, &ctx) {
                Checked::Ok(value) => {
                    set(&mut tree, &path, value);
                    settings += 1;
                }
                Checked::Nothing => {}
                Checked::Flagged => flagged.push(sensitive::setting_path_label(&path)),
            }
        }
    }

    let mut extension_settings_left_out = 0usize;
    let mut ids: Vec<&String> = config.extensions.keys().collect();
    ids.sort();
    for id in ids {
        let saved = &config.extensions[id];
        let Some(obj) = saved.as_object() else {
            extension_settings_left_out += usize::from(!is_empty(saved));
            continue;
        };
        let mut fields: Vec<&String> = obj.keys().collect();
        fields.sort();
        for field in fields {
            let value = &obj[field];
            if is_host_managed(field) || value.is_null() {
                continue;
            }
            if !ctx.key_ok(id) {
                extension_settings_left_out += 1;
                continue;
            }
            match rules.plain_setting(id, field, value) {
                Plain::Include => {
                    set(
                        &mut tree,
                        &["extensions".to_string(), id.clone(), field.clone()],
                        value.clone(),
                    );
                    settings += 1;
                }
                Plain::Flagged => flagged.push(names.setting_label(id, field)),
                Plain::No => extension_settings_left_out += 1,
            }
        }
    }
    SettingsOnly {
        tree,
        settings,
        flagged,
        extension_settings_left_out,
    }
}

/// Whether an extension setting from a settings-only backup may be merged
/// for an extension that isn't installed here, so its manifest can't be
/// checked: a boolean or a number under a name that isn't a credential's.
/// No string: without the manifest there's no telling a free-form one from
/// a choice.
fn plain_without_manifest(field: &str, v: &Value) -> bool {
    !crate::extensions::is_sensitive_name(field)
        && !looks_like_secret(field)
        && matches!(v, Value::Bool(_) | Value::Number(_))
}

/// Most extension ids a settings-only import takes per map
/// (`extension_states`, `extension_grants`, `extensions`): far more than
/// anyone installs, few enough that the merged config stays well under
/// `Config::load`'s size limit.
const MAX_IMPORTED_IDS: usize = 512;
/// Most settings a settings-only import takes per extension, and the
/// longest setting name.
const MAX_IMPORTED_FIELDS: usize = 128;
const MAX_FIELD_NAME: usize = 128;

/// For a full import (which takes Kage's settings from the backup, not
/// through this table): put this machine's value back wherever the merged
/// `tree` holds an allowlisted number that isn't one of that setting's
/// values ([`Range`]). Done on the JSON, before `Config` reads it: as an
/// `f32`, 1e100 would become infinity, be saved as `null`, and reset the
/// whole section on the next start.
pub fn keep_local_numbers_in_range(tree: &mut Value, local: &Value) {
    let rules = Rules::default();
    let ctx = Ctx {
        rules: &rules,
        side: Side::Import,
    };
    for (pattern, rule) in FIELDS {
        let Include(Number(range)) = rule else {
            continue;
        };
        let refused: Vec<Vec<String>> = matching(tree, pattern, &ctx)
            .into_iter()
            .filter(|(_, v)| matches!(v, Value::Number(n) if !range.holds(n)))
            .map(|(path, _)| path)
            .collect();
        for path in refused {
            let mut here = Some(local);
            for seg in &path {
                here = here.and_then(|v| v.get(seg));
            }
            match here {
                Some(value) => set(tree, &path, value.clone()),
                None => remove(tree, &path),
            }
        }
    }
}

/// Remove the value at `path` in `tree`, if it's there.
fn remove(tree: &mut Value, path: &[String]) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut cur = tree;
    for seg in parents {
        let Some(next) = cur.get_mut(seg) else {
            return;
        };
        cur = next;
    }
    if let Some(obj) = cur.as_object_mut() {
        obj.remove(last);
    }
}

/// For a full import: put this machine's hotkey back wherever the merged
/// one isn't a shortcut the runtime registers ([`hotkey_registers`]).
pub fn keep_local_unregistrable_hotkeys(merged: &mut Config, local: &Config) {
    let registers = |h: &crate::config::HotkeyConfig| hotkey_registers(&h.modifiers, &h.key);
    if !registers(&merged.hotkey) {
        merged.hotkey = local.hotkey.clone();
    }
    for (m, l) in [
        (&mut merged.clipboard_hotkey, &local.clipboard_hotkey),
        (
            &mut merged.inline_assist_hotkey,
            &local.inline_assist_hotkey,
        ),
        (&mut merged.voice_hotkey, &local.voice_hotkey),
    ] {
        if m.as_ref().is_some_and(|h| !registers(h)) {
            *m = l.clone();
        }
    }
}

/// Merge a settings-only backup's `config.json` (`incoming`, already
/// migrated) into this machine's `local`: only allowlisted fields, each
/// re-checked; an empty value keeps this machine's; a hotkey is taken
/// whole, and only one the runtime registers; grants never. An extension
/// setting is taken only as this machine's manifests allow it (booleans
/// and numbers only for an extension that isn't installed here), and none
/// at all for an extension in `keep_whole` (folded ids: it keeps a
/// credential here, so its settings here stay whole). Returns the merged
/// config, how many settings it took from the backup, and the extensions
/// (the backup's ids) kept whole that it would otherwise have changed.
pub fn merge(
    local: &Config,
    incoming: &Value,
    rules: &Rules,
    keep_whole: &std::collections::BTreeSet<String>,
) -> anyhow::Result<(Config, usize, Vec<String>)> {
    let ctx = Ctx {
        rules,
        side: Side::Import,
    };
    let mut tree = serde_json::to_value(local)?;
    let mut settings = 0usize;
    let mut kept: Vec<String> = Vec::new();
    for (pattern, rule) in FIELDS {
        let Include(shape) = rule else {
            continue;
        };
        if matches!(shape, ExtensionSettings | SchemaVersion) {
            continue;
        }
        // Grants come only from consent on this machine, never from a
        // backup (see `super::consent`).
        if pattern.starts_with("extension_grants.") {
            continue;
        }
        let mut ids = 0usize;
        for (path, v) in matching(incoming, pattern, &ctx) {
            if pattern.contains('*') {
                ids += 1;
                if ids > MAX_IMPORTED_IDS {
                    break;
                }
            }
            let Checked::Ok(value) = check(*shape, v, &path, &ctx) else {
                continue;
            };
            if is_empty(&value) {
                continue;
            }
            set(&mut tree, &path, value);
            settings += 1;
        }
    }
    if let Some(exts) = incoming.get("extensions").and_then(Value::as_object) {
        for (id, saved) in exts.iter().take(MAX_IMPORTED_IDS) {
            let Some(obj) = saved.as_object() else {
                continue;
            };
            if !ctx.key_ok(id) {
                continue;
            }
            let installed = rules.is_installed(id);
            let whole = keep_whole.contains(&sensitive::fold(id));
            let local_obj = tree
                .get("extensions")
                .and_then(|e| e.get(id))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            for (field, v) in obj.iter().take(MAX_IMPORTED_FIELDS) {
                if is_host_managed(field) || is_empty(v) || field.len() > MAX_FIELD_NAME {
                    continue;
                }
                let ok = if installed {
                    rules.plain_setting(id, field, v) == Plain::Include
                } else {
                    plain_without_manifest(field, v)
                };
                if !ok {
                    continue;
                }
                if whole {
                    // Its credential stays here: none of its settings here
                    // change, whatever they're called or hold.
                    if local_obj.get(field) != Some(v) && !kept.contains(id) {
                        kept.push(id.clone());
                    }
                    continue;
                }
                set(
                    &mut tree,
                    &["extensions".to_string(), id.clone(), field.clone()],
                    v.clone(),
                );
                settings += 1;
            }
        }
    }
    let merged = Config::deserialize(&tree)
        .map_err(|e| anyhow::anyhow!("Backup settings don't fit this machine's: {}", e))?;
    Ok((merged, settings, kept))
}

/// Extension display names, for labels.
pub struct ExtensionNames(pub std::collections::HashMap<String, String>);

impl ExtensionNames {
    fn setting_label(&self, id: &str, field: &str) -> String {
        crate::t!(
            "settings.about.backup.settings_only.flagged_extension_setting",
            "extension" => &sensitive::isolate(&sensitive::extension_label(
                id,
                &self.0,
                &Default::default()
            )),
            "field" => &sensitive::isolate(field)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A config with every optional field set and every list and map
    /// holding an item, so its JSON names every field there is.
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
                origin: Some("sideload".into()),
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

    fn rule_for(path: &str) -> Option<Rule> {
        FIELDS.iter().find(|(p, _)| *p == path).map(|(_, r)| *r)
    }

    /// Every path the walk stops at: a table entry, or a leaf no entry
    /// covers (returned in `undecided`).
    fn walk(
        v: &Value,
        path: &mut Vec<String>,
        seen: &mut Vec<String>,
        undecided: &mut Vec<String>,
    ) {
        let here = path.join(".");
        if !path.is_empty() && rule_for(&here).is_some() {
            seen.push(here);
            return;
        }
        match v {
            Value::Object(o) if !o.is_empty() => {
                for (k, child) in o {
                    // Map keys (extension ids) are the `*` segment.
                    let parent = path.join(".");
                    let seg = if matches!(parent.as_str(), "extension_states" | "extension_grants")
                    {
                        "*".to_string()
                    } else {
                        k.clone()
                    };
                    path.push(seg);
                    walk(child, path, seen, undecided);
                    path.pop();
                }
            }
            _ => undecided.push(here),
        }
    }

    #[test]
    fn every_config_field_has_a_decision() {
        let tree = serde_json::to_value(complete_config()).unwrap();
        let (mut seen, mut undecided) = (Vec::new(), Vec::new());
        walk(&tree, &mut Vec::new(), &mut seen, &mut undecided);
        assert!(
            undecided.is_empty(),
            "Config fields with no settings-only backup decision: {:?}. Add each to \
             config_export::allowlist::FIELDS as Include(shape) or Leave(why).",
            undecided
        );
        let stale: Vec<&str> = FIELDS
            .iter()
            .map(|(p, _)| *p)
            .filter(|p| !seen.iter().any(|s| s == p))
            .collect();
        assert!(
            stale.is_empty(),
            "FIELDS names fields Config doesn't have: {:?}",
            stale
        );
        let mut sorted: Vec<&str> = FIELDS.iter().map(|(p, _)| *p).collect();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), FIELDS.len(), "a field is listed twice");
    }

    fn weather_rules() -> Rules {
        let item = |kind: &str, id: &str| crate::extensions::InstalledItem {
            manifest: serde_json::from_value(json!({
                "id": id, "name": id, "version": "1.0.0", "type": kind
            }))
            .unwrap(),
            path: String::new(),
            enabled: true,
        };
        Rules::new(&[item("extension", "weather"), item("theme", "catppuccin")])
    }

    #[test]
    fn the_defaults_of_every_copied_field_fit_its_shape() {
        let tree = serde_json::to_value(complete_config()).unwrap();
        let rules = weather_rules();
        let ctx = Ctx {
            rules: &rules,
            side: Side::Export,
        };
        for (pattern, rule) in FIELDS {
            let Include(shape) = rule else { continue };
            if matches!(shape, ExtensionSettings | SchemaVersion) {
                continue;
            }
            let found = matching(&tree, pattern, &ctx);
            assert!(!found.is_empty(), "{}", pattern);
            for (path, v) in found {
                assert!(
                    matches!(check(*shape, v, &path, &ctx), Checked::Ok(_)),
                    "{} = {} doesn't fit {:?}",
                    path.join("."),
                    v,
                    shape
                );
            }
        }
    }

    #[test]
    fn shapes_refuse_free_text() {
        let rules = weather_rules();
        let ctx = Ctx {
            rules: &rules,
            side: Side::Export,
        };
        let at = |p: &str| p.split('.').map(str::to_string).collect::<Vec<_>>();
        for (shape, bad, path) in [
            (Theme, json!("Qz8vRt2LmN"), "ui.theme"),
            (Theme, json!("not-installed-theme"), "ui.theme"),
            (Theme, json!("weather"), "ui.theme"),
            (Language, json!("password: x"), "ui.language"),
            (Language, json!("abc-Qz8vRt2L"), "ui.language"),
            (
                Hotkey,
                json!({ "modifiers": ["Ctrl"], "key": "hunter2!" }),
                "hotkey",
            ),
            (
                Hotkey,
                json!({ "modifiers": ["Ctrl"], "key": "Qz8vRt2LmN" }),
                "hotkey",
            ),
            (
                Hotkey,
                json!({ "modifiers": ["Ctrl"], "key": "F25" }),
                "hotkey",
            ),
            (
                Hotkey,
                json!({ "modifiers": ["Ctrl", "Hyper2"], "key": "K" }),
                "hotkey",
            ),
            (
                OneOf(TIME_FORMATS),
                json!("HH:mm Qz8vRt2LmN"),
                "ui.time_format",
            ),
            (
                Hotkey,
                json!({ "modifiers": ["Ctrl", "Qz8vRt2LmN!"], "key": "K" }),
                "hotkey",
            ),
        ] {
            assert_eq!(
                check(shape, &bad, &at(path), &ctx),
                Checked::Flagged,
                "{:?} {}",
                shape,
                bad
            );
        }
        for (shape, value, path) in [
            (Theme, json!("catppuccin"), "ui.theme"),
            (Theme, json!("dark"), "ui.theme"),
            (Language, json!("pt"), "ui.language"),
            (Language, json!("zh-CN"), "ui.language"),
            (Hotkey, json!({ "modifiers": [], "key": "F13" }), "hotkey"),
            (
                Hotkey,
                json!({ "modifiers": ["Ctrl", "Shift"], "key": "/" }),
                "hotkey",
            ),
        ] {
            assert_eq!(
                check(shape, &value, &at(path), &ctx),
                Checked::Ok(value.clone()),
                "{:?}",
                shape
            );
        }
        // A grant's version must be the installed one; timestamps must parse.
        let grant = at("extension_grants.weather.approved_version");
        assert_eq!(
            check(GrantVersion, &json!("1.0.0"), &grant, &ctx),
            Checked::Ok(json!("1.0.0"))
        );
        assert_eq!(
            check(GrantVersion, &json!("Qz8vRt2LmN-x"), &grant, &ctx),
            Checked::Nothing
        );
        assert_eq!(
            check(GrantVersion, &json!("2.0.0"), &grant, &ctx),
            Checked::Nothing
        );
        let ts = at("extension_grants.weather.approved_at");
        assert_eq!(
            check(Timestamp, &json!("Qz8vRt2LmN"), &ts, &ctx),
            Checked::Nothing
        );
        assert_eq!(
            check(Timestamp, &json!("2026-01-01T00:00:00Z"), &ts, &ctx),
            Checked::Ok(json!("2026-01-01T00:00:00Z"))
        );
        assert_eq!(
            check(
                Capabilities,
                &json!(["storage", "evil", "network"]),
                &at("x"),
                &ctx
            ),
            Checked::Ok(json!(["storage", "network"]))
        );
        assert_eq!(
            check(Bool, &json!("true"), &at("x"), &ctx),
            Checked::Nothing
        );
        assert_eq!(
            check(Language, &json!(null), &at("x"), &ctx),
            Checked::Nothing
        );
        // Importing, an id needn't be installed yet.
        let import = Ctx {
            rules: &rules,
            side: Side::Import,
        };
        assert_eq!(
            check(
                Theme,
                &json!("not-installed-theme"),
                &at("ui.theme"),
                &import
            ),
            Checked::Ok(json!("not-installed-theme"))
        );
    }

    #[test]
    fn wildcard_keys_must_be_installed_ids_on_export() {
        let tree = json!({ "extension_states": {
            "weather": true, "catppuccin": true, "qz8vrt2lmn": true, "Bad Id": true,
            "ghp_0123456789abcdefABCDEF0123": true, "../x": false
        }});
        let rules = weather_rules();
        let keys = |side| {
            let ctx = Ctx {
                rules: &rules,
                side,
            };
            let mut found: Vec<String> = matching(&tree, "extension_states.*", &ctx)
                .into_iter()
                .map(|(p, _)| p.join("."))
                .collect();
            found.sort();
            found
        };
        assert_eq!(
            keys(Side::Export),
            vec!["extension_states.catppuccin", "extension_states.weather"]
        );
        assert_eq!(
            keys(Side::Import),
            vec![
                "extension_states.catppuccin",
                "extension_states.qz8vrt2lmn",
                "extension_states.weather"
            ]
        );
    }

    #[test]
    fn every_number_must_be_one_of_its_settings_values() {
        // The review's probe: 1e100 opacity became INFINITY as an f32, was
        // saved as null, and the next start reset the whole UI section.
        let rules = weather_rules();
        let local = Config::default();
        let incoming = json!({
            "version": 1,
            "ui": { "floating_window_opacity": 1e100, "font_size": 300,
                    "speech_silence_timeout": -1.0 },
            "acp": { "agent": { "auto_compact_threshold": 12.5 } },
            "pocket_tts": { "temp": 0.5, "eos_threshold": 3.0 },
            "automation_power": { "battery_multiplier": 0.0, "low_battery_multiplier": 1e9 }
        });
        let (merged, settings, _) = merge(&local, &incoming, &rules, &Default::default()).unwrap();
        assert_eq!(settings, 1, "only the in-range temp");
        assert_eq!(merged.pocket_tts.temp, 0.5);
        assert_eq!(
            merged.ui.floating_window_opacity,
            local.ui.floating_window_opacity
        );
        assert_eq!(merged.ui.font_size, local.ui.font_size);
        assert_eq!(
            merged.ui.speech_silence_timeout,
            local.ui.speech_silence_timeout
        );
        assert_eq!(
            merged.acp.agent.auto_compact_threshold,
            local.acp.agent.auto_compact_threshold
        );
        assert_eq!(
            merged.pocket_tts.eos_threshold,
            local.pocket_tts.eos_threshold
        );
        assert_eq!(
            merged.automation_power.battery_multiplier,
            local.automation_power.battery_multiplier
        );
        assert_eq!(
            merged.automation_power.low_battery_multiplier,
            local.automation_power.low_battery_multiplier
        );
        // What it saves reads back whole.
        let saved = serde_json::to_string(&merged).unwrap();
        let reread: Config = serde_json::from_str(&saved).unwrap();
        assert_eq!(reread.ui.floating_window_opacity, 1.0);

        // On export, an out-of-range local value is left out and named.
        let mut odd = Config::default();
        odd.ui.floating_window_opacity = 7.0;
        odd.ui.font_size = 99;
        let only = extract(&odd, &rules, &ExtensionNames(Default::default()));
        assert!(only.tree["ui"].get("floating_window_opacity").is_none());
        assert!(only.tree["ui"].get("font_size").is_none());
        assert_eq!(only.flagged.len(), 2, "{:?}", only.flagged);
        // The bounds of every field hold its own defaults and slider ends.
        for (range, ok, bad) in [
            (
                OPACITY,
                vec![json!(0.3), json!(1)],
                vec![json!(0.29), json!(1.5)],
            ),
            (
                FONT_SIZE,
                vec![json!(11), json!(20)],
                vec![json!(14.5), json!(21)],
            ),
            (
                COMPACT_THRESHOLD,
                vec![json!(0), json!(100)],
                vec![json!(-5), json!(101)],
            ),
            (
                TTS_EOS,
                vec![json!(-8.0), json!(-1)],
                vec![json!(0), json!(-9)],
            ),
        ] {
            for v in ok {
                assert!(range.holds(v.as_number().unwrap()), "{} {:?}", v, range);
            }
            for v in bad {
                assert!(!range.holds(v.as_number().unwrap()), "{} {:?}", v, range);
            }
        }
        // A full import's tree gets the same rule.
        let mut tree = json!({ "ui": { "floating_window_opacity": 1e100, "font_size": 12 } });
        keep_local_numbers_in_range(&mut tree, &serde_json::to_value(&local).unwrap());
        assert_eq!(tree["ui"]["floating_window_opacity"], json!(1.0));
        assert_eq!(tree["ui"]["font_size"], json!(12));
    }

    #[test]
    fn a_hotkey_is_taken_only_whole_and_only_if_the_runtime_registers_it() {
        // The review's probe: `Win+K` and `Meta` passed the name list but
        // the runtime's parser refuses them, so the hotkey never registered.
        let rules = weather_rules();
        let local = Config::default();
        for (hotkey, why) in [
            (json!({ "modifiers": ["Win"], "key": "K" }), "Win"),
            (
                json!({ "modifiers": ["Meta", "Shift"], "key": "K" }),
                "Meta",
            ),
            (json!({ "modifiers": ["Ctrl"], "key": "" }), "no key"),
            (json!({ "key": "K" }), "no modifiers"),
            (
                json!({ "modifiers": ["Ctrl", "Ctrl", "Ctrl", "Ctrl", "Ctrl", "Ctrl", "Ctrl"], "key": "K" }),
                "too many",
            ),
        ] {
            let (merged, ..) = merge(
                &local,
                &json!({ "version": 1, "hotkey": hotkey, "voice_hotkey": hotkey }),
                &rules,
                &Default::default(),
            )
            .unwrap();
            assert_eq!(merged.hotkey.modifiers, local.hotkey.modifiers, "{}", why);
            assert_eq!(merged.hotkey.key, local.hotkey.key, "{}", why);
            assert!(merged.voice_hotkey.is_none(), "{}", why);
        }
        // Aliases the runtime reads the same way are written canonically,
        // and the result registers.
        let (merged, settings, _) = merge(
            &local,
            &json!({ "version": 1,
                     "hotkey": { "modifiers": ["command", "Option", "control"], "key": "k" },
                     "clipboard_hotkey": { "modifiers": ["CmdOrCtrl"], "key": "F13" } }),
            &rules,
            &Default::default(),
        )
        .unwrap();
        assert_eq!(settings, 2);
        assert_eq!(merged.hotkey.modifiers, vec!["Super", "Alt", "Ctrl"]);
        assert_eq!(merged.hotkey.key, "k");
        assert!(hotkey_registers(
            &merged.hotkey.modifiers,
            &merged.hotkey.key
        ));
        let clip = merged.clipboard_hotkey.unwrap();
        assert_eq!(clip.modifiers, vec!["CmdOrCtrl"]);
        // Every key name the closed list holds is one the runtime reads.
        for key in NAMED_KEYS
            .iter()
            .map(|k| k.to_string())
            .chain((1..=24).map(|n| format!("F{}", n)))
            .chain(('A'..='Z').map(String::from))
            .chain((0..=9).map(|n| n.to_string()))
            .chain((0..=9).map(|n| format!("Num{}", n)))
        {
            assert!(
                hotkey_registers(&["Ctrl".to_string()], &key),
                "Ctrl+{} doesn't register",
                key
            );
        }
        // On export a hotkey the runtime can't register is left out, named.
        let mut odd = Config::default();
        odd.hotkey.modifiers = vec!["Win".into()];
        let only = extract(&odd, &rules, &ExtensionNames(Default::default()));
        assert!(only.tree.get("hotkey").is_none());
        assert_eq!(only.flagged.len(), 1, "{:?}", only.flagged);
        // A full import keeps this machine's where the backup's can't register.
        let mut full = Config::default();
        full.hotkey.modifiers = vec!["Meta".into()];
        full.voice_hotkey = Some(crate::config::HotkeyConfig {
            modifiers: vec!["Win".into()],
            key: "V".into(),
        });
        keep_local_unregistrable_hotkeys(&mut full, &local);
        assert_eq!(full.hotkey.modifiers, local.hotkey.modifiers);
        assert!(full.voice_hotkey.is_none());
    }
}
