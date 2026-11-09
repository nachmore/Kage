//! Backups end to end: two "machines" are two configs, two extension-data
//! roots and two steering paths, all under the isolated test config dir
//! (`crate::paths` isolates unit tests), so nothing here touches a real
//! profile.

use super::*;
use crate::config::{AcpMode, AgentConnection, PolicyKind, ToolPolicy};
use serde_json::{json, Value};

/// Tests here share the isolated config.json, extension-data root (with
/// `commands::extensions::files`' tests) and auto steering doc (with
/// `steering_io`'s). Always taken in that order.
struct Disk {
    _extension_data: std::sync::MutexGuard<'static, ()>,
    _auto_doc: std::sync::MutexGuard<'static, ()>,
}

fn disk() -> Disk {
    let extension_data = EXTENSION_DATA_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let auto_doc = crate::steering_io::AUTO_DOC_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    Disk {
        _extension_data: extension_data,
        _auto_doc: auto_doc,
    }
}

fn assert_isolated() {
    let dir = crate::paths::config_dir().expect("config dir");
    assert!(crate::paths::is_isolated(), "unit tests must run isolated");
    assert!(crate::paths::is_under_isolation_root(&dir));
}

fn no_auto_steering() {
    if let Ok(p) = Config::get_auto_steering_path() {
        let _ = std::fs::remove_file(p);
    }
}

fn item(manifest: Value) -> InstalledItem {
    InstalledItem {
        manifest: serde_json::from_value(manifest).expect("manifest"),
        path: String::new(),
        enabled: true,
    }
}

fn installed() -> Vec<InstalledItem> {
    vec![
        item(json!({
            "id": "github", "name": "GitHub", "version": "1.0.0", "type": "extension",
            "sensitiveData": ["token", "state"],
            "config": {
                "personal_token": { "type": "string", "default": "", "sensitive": true },
                "trigger": { "type": "string", "default": "gh", "backup": "plain" },
                "show_badge": { "type": "boolean", "default": true },
                "limit": { "type": "number", "default": 10 },
                "sort": { "type": "string", "default": "new", "enum": ["new", "top"] },
                "note": { "type": "string", "default": "" },
                "server_url": { "type": "string", "default": "", "backup": "plain" },
                "keep_session": { "type": "boolean", "default": false },
                "auth_method": { "type": "string", "default": "pat", "enum": ["oauth", "pat"] }
            },
        })),
        item(json!({
            "id": "spotify", "name": "Spotify", "version": "1.0.0", "type": "extension",
            "sensitiveData": ["creds"],
        })),
        item(json!({ "id": "todos", "name": "Todos", "version": "1.0.0", "type": "extension" })),
        // Installed, declares nothing (the review's "Notes").
        item(json!({ "id": "notes", "name": "Notes", "version": "1.0.0", "type": "extension" })),
    ]
}

/// One machine: its config, extension data and steering doc.
struct Machine {
    config: Config,
    root: tempfile::TempDir,
    steering: PathBuf,
    _dir: tempfile::TempDir,
}

impl Machine {
    fn new() -> Self {
        assert_isolated();
        let dir = tempfile::tempdir().unwrap();
        let steering = dir.path().join("user-steering.md");
        let mut config = Config::default();
        config.acp.agent.user_steering_path = Some(steering.to_string_lossy().into_owned());
        Self {
            config,
            root: tempfile::tempdir().unwrap(),
            steering,
            _dir: dir,
        }
    }

    fn file(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.root.path().join(rel)).ok()
    }

    fn write(&self, rel: &str, body: &str) {
        let p = self.root.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn export(&self, mode: ExportMode, passphrase: Option<&str>) -> Result<Vec<u8>> {
        export_from(
            &self.config,
            mode,
            passphrase,
            &installed(),
            Some(self.root.path()),
        )
    }

    fn try_import(
        &self,
        bytes: &[u8],
        passphrase: Option<&str>,
    ) -> Result<(Config, ImportSummary)> {
        let prepared = prepare_import(bytes, passphrase)?;
        let (config, summary, files) = apply_import_at(
            prepared,
            &self.config,
            &installed(),
            Some(self.root.path().to_path_buf()),
        )?
        .into_parts();
        files.finish()?;
        Ok((config, summary))
    }

    /// Import `bytes` here and keep the result as this machine's config.
    fn import(&mut self, bytes: &[u8], passphrase: Option<&str>) -> ImportSummary {
        let (merged, summary) = self.try_import(bytes, passphrase).expect("import");
        let steering = self.config.acp.agent.user_steering_path.clone();
        self.config = merged;
        assert_eq!(self.config.acp.agent.user_steering_path, steering);
        summary
    }
}

fn connection(id: &str, command: &str) -> AgentConnection {
    AgentConnection {
        id: id.into(),
        name: format!("{} agent", id),
        preset_id: None,
        mode: AcpMode::Local {
            spawn_command: command.into(),
        },
        sessions_directory: None,
        ollama_settings: None,
    }
}

fn spawn(config: &Config, i: usize) -> String {
    match &config.acp.connections[i].mode {
        AcpMode::Local { spawn_command } => spawn_command.clone(),
        other => panic!("{:?}", other),
    }
}

fn rule(title: &str, policy: PolicyKind) -> ToolPolicy {
    ToolPolicy {
        title: title.into(),
        policy,
        last_seen: "2026-01-01".into(),
        granted_at: String::new(),
        grant_type: Default::default(),
    }
}

fn entries(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut body = Vec::new();
        e.read_to_end(&mut body).unwrap();
        out.insert(e.name().to_string(), body);
    }
    out
}

fn manifest_of(bytes: &[u8]) -> BundleManifest {
    serde_json::from_slice(&entries(bytes)["manifest.json"]).unwrap()
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
    }
    buf
}

fn v1() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "format": "kage-backup", "version": 1,
        "exported_at": "2026-01-01T00:00:00Z", "kage_version": "0.9.0"
    }))
    .unwrap()
}

/// A hand-made v1 bundle: manifest + `config` + `entries`.
fn handmade(config: &Config, extra: &[(&str, &[u8])]) -> Vec<u8> {
    let manifest = v1();
    let config = serde_json::to_vec(config).unwrap();
    let mut all: Vec<(&str, &[u8])> = vec![("manifest.json", &manifest), ("config.json", &config)];
    all.extend_from_slice(extra);
    zip_of(&all)
}

// --- Sanitisation ---------------------------------------------------------------

fn fake_config() -> Config {
    let mut c = Config::default();
    c.telemetry.install_id = Some("device-A".to_string());
    c.ui.chat_window_width = 1234;
    c.ui.last_window_x = Some(99);
    c.updates.last_check_time = Some("2026-01-01T00:00:00Z".to_string());
    c.last_extension_update_check = Some("2026-01-01T00:00:00Z".to_string());
    c.system.auto_start = false;
    c
}

#[test]
fn sanitise_keeps_device_local_fields() {
    let mut imported = fake_config();
    imported.telemetry.install_id = Some("device-source".into());
    imported.ui.chat_window_width = 9999;
    imported.ui.last_window_x = Some(-100);
    imported.system.auto_start = true;
    imported.acp.agent.import_login_shell_env = true;
    imported.acp.agent.user_steering_path = Some("/home/me/.zshrc".into());
    let mut local = fake_config();
    local.telemetry.install_id = Some("device-local".into());
    local.ui.chat_window_width = 1024;
    local.ui.last_window_x = Some(50);
    local.acp.agent.import_login_shell_env = false;
    local.acp.agent.user_steering_path = Some("/home/me/steering.md".into());
    let merged = sanitise_imported_config(imported, &local);
    assert_eq!(merged.telemetry.install_id.as_deref(), Some("device-local"));
    assert_eq!(merged.ui.chat_window_width, 1024);
    assert_eq!(merged.ui.last_window_x, Some(50));
    assert!(!merged.system.auto_start);
    assert!(!merged.acp.agent.import_login_shell_env);
    assert_eq!(
        merged.acp.agent.user_steering_path.as_deref(),
        Some("/home/me/steering.md")
    );
    assert!(merged.updates.last_check_time.is_none());
    assert!(merged.updates.last_updated_version.is_none());
    assert!(merged.last_extension_update_check.is_none());
}

#[test]
fn sanitise_folds_a_legacy_extension_enabled_false() {
    let mut imported = fake_config();
    imported.extensions.insert(
        "link-preview".to_string(),
        json!({ "enabled": false, "max_previews": 3 }),
    );
    imported
        .extension_states
        .insert("link-preview".to_string(), true);
    let merged = sanitise_imported_config(imported, &fake_config());
    assert_eq!(merged.extension_states.get("link-preview"), Some(&false));
    assert_eq!(
        merged.extensions.get("link-preview"),
        Some(&json!({ "max_previews": 3 }))
    );
}

#[test]
fn default_filenames_say_which_kind_of_backup() {
    let enc = default_filename(ExportMode::Encrypted);
    assert!(enc.starts_with("kage-backup-") && enc.ends_with(".kage.enc"));
    let plain = default_filename(ExportMode::SettingsOnly);
    assert!(plain.starts_with("kage-settings-") && plain.ends_with(".kage"));
}

// --- Export modes -------------------------------------------------------------------

#[test]
fn an_encrypted_backup_needs_a_real_passphrase() {
    let m = Machine::new();
    for (pw, want) in [
        (None, "needed"),
        (Some(""), "blank"),
        (Some("   "), "blank"),
    ] {
        let err = m.export(ExportMode::Encrypted, pw).unwrap_err();
        match want {
            "needed" => assert!(
                err.downcast_ref::<PassphraseNeeded>().is_some(),
                "{:#}",
                err
            ),
            _ => assert!(err.downcast_ref::<BlankPassphrase>().is_some(), "{:#}", err),
        }
    }
    assert!(
        m.export(ExportMode::SettingsOnly, Some("pw")).is_err(),
        "a settings-only backup is never encrypted"
    );
}

/// A machine with a credential (or something only its user should read)
/// in every kind of place the reviews found one, plus values hand-edited
/// into allowlisted fields.
fn loaded_machine() -> Machine {
    let mut m = Machine::new();
    m.config.acp.connections = vec![
        connection("quoted", r#"cmd /c "my-agent Qz8vRt2LmN""#),
        connection("envs", r#"env -S "my-agent Qz8vRt2LmN""#),
        connection(
            "claude",
            "env ANTHROPIC_API_KEY=sk-ant-api03-LEAKleakLEAKleak claude-code-acp",
        ),
    ];
    m.config.store_url = Some("https://me:$PASSWORD123@store.example.com/catalog.json".into());
    m.config.store_sources = vec![serde_json::from_value(json!({
        "name": "Mine", "url": "https://store.example.com/catalog.json?auth[token]=Qz8vRt2LmN"
    }))
    .unwrap()];
    m.config.shortcuts = vec![serde_json::from_value(json!({
        "name": "deploy", "action_type": "run_program", "path": "deploy",
        "arguments": "--token Qz8vRt2LmN", "url": "https://x/?token=Qz8vRt2LmN"
    }))
    .unwrap()];
    m.config.macros = vec![serde_json::from_value(json!({
        "name": "m", "steps": [{ "step_type": "script", "script": "fetch('x', {headers:{Authorization:'Bearer Qz8vRt2LmN'}})" }]
    }))
    .unwrap()];
    m.config.quick_actions.custom_actions = [
        "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQ",
        "PuTTY-User-Key-File-3: ssh-ed25519\nPrivate-Lines: 1\nAAAALEAKLEAK",
        "use xai-LEAKleakLEAKleak123 and gsk_LEAKleakLEAKleak123",
        "whsec_LEAKleakLEAKleak123 dckr_pat_LEAKleakLEAK hvs.LEAKleakLEAKleak xoxe-LEAK-leak",
    ]
    .iter()
    .map(|p| serde_json::from_value(json!({ "label": "x", "prompt": p })).unwrap())
    .collect();
    m.config.internal_sites = vec![serde_json::from_value(json!({
        "id": "wiki", "name": "Wiki",
        "url_rules": [{ "kind": "domain_suffix", "pattern": "wiki.corp.example" }],
        "proxy": { "lang": "bash", "script": "curl -H 'Authorization: Bearer Qz8vRt2LmN' \"$KAGE_URL\"" }
    }))
    .unwrap()];
    m.config.context_rules = vec![serde_json::from_value(json!({
        "friendly_name": "Terminal", "executable": "iTerm2", "steering": "password: sunshinebeachwaves"
    }))
    .unwrap()];
    m.config.tool_permissions.tools = vec![
        rule("mysql -h db -u admin -pQz8vRt2LmN mydb", PolicyKind::Allow),
        rule("cat notes.txt", PolicyKind::Ask),
    ];
    m.config.tool_permissions.terminator_mode = true;
    m.config.mcp_config_path = Some("/Users/me/Qz8vRt2LmN/mcp.json".into());
    m.config.acp.agent.default_model = Some("model-Qz8vRt2LmN".into());
    // Hand-edited into allowlisted fields: never copied.
    m.config.ui.theme = "Qz8vRt2LmN-secret-token-xyz123".into();
    m.config.quick_actions.translate_language = Some("password: hunter2".into());
    m.config.system.capture_selection_blocklist = vec!["iTerm2".into(), "token=Qz8vRt2LmN".into()];
    m.config.ui.date_format = "Qz8vRt2LmN".into();
    m.config.hotkey.key = "Qz8vRt2LmN".into();
    m.config.telemetry.install_id = Some("install-id-Qz8vRt2LmN".into());
    m.config
        .extension_states
        .insert("ghp_0123456789abcdefABCDEF0123".into(), true);
    // Extension settings: declared, undeclared, structured.
    m.config.extensions.insert(
        "github".into(),
        json!({
            "personal_token": "ghp_LEAKleakLEAKleakLEAKleak",
            "trigger": "gh", "show_badge": false, "limit": 25, "sort": "top",
            "note": "password: sunshinebeachwaves",
            "__scratch": "ghp_LEAKleakLEAKleakLEAKleak"
        }),
    );
    m.config.extensions.insert(
        "notes".into(),
        json!({
            "headers": [{ "name": "Authorization", "value": "Bearer Qz8vRt2LmN" }],
            "cookies": [{ "domain": "service.example", "name": "session", "value": "Qz8vRt2LmN" }],
            "options": { "authHeader": "Bearer Qz8vRt2LmN" },
            "flag": true
        }),
    );
    m.config.extensions.insert(
        "uninstalled".into(),
        json!({ "flag": true, "key": "Qz8vRt2LmN" }),
    );
    m.write(
        "notes/state.json",
        r#"{"headers":[{"name":"Authorization","value":"Bearer Qz8vRt2LmN"}]}"#,
    );
    m.write(
        "notes/cookies.json",
        r#"[{"domain":"service.example","name":"session","value":"Qz8vRt2LmN"}]"#,
    );
    m.write(
        "github/token.json",
        r#"{"token":"ghp_LEAKleakLEAKleakLEAKleak"}"#,
    );
    let utf16: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain(
            "password: Qz8vRt2LmN\n"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        )
        .collect();
    std::fs::write(&m.steering, &utf16).unwrap();
    m
}

const LEAK_MARKERS: &[&str] = &[
    "Qz8vRt2LmN",
    "LEAK",
    "sunshinebeachwaves",
    "PASSWORD123",
    "hunter2",
    "AGE-SECRET-KEY",
    "PuTTY",
    "ghp_",
    "deploy",
    "my-agent",
    "claude-code-acp",
    "wiki",
    "mysql",
    "install-id",
];

#[test]
fn a_settings_only_backup_holds_only_allowlisted_settings() {
    let _disk = disk();
    no_auto_steering();
    let m = loaded_machine();
    let bytes = m.export(ExportMode::SettingsOnly, None).unwrap();
    assert!(!bytes.starts_with(ENCRYPTED_MAGIC));
    let files = entries(&bytes);
    assert_eq!(
        files.keys().collect::<Vec<_>>(),
        vec!["config.json", "manifest.json"],
        "no steering docs, no extension data"
    );
    // Not one of the secrets, in any encoding the inputs used.
    for (name, body) in &files {
        let text = String::from_utf8_lossy(body);
        for marker in LEAK_MARKERS {
            assert!(
                !text.contains(marker),
                "{} carries {:?}:\n{}",
                name,
                marker,
                text
            );
            let utf16: Vec<u8> = marker.encode_utf16().flat_map(u16::to_le_bytes).collect();
            assert!(
                !body.windows(utf16.len()).any(|w| w == utf16.as_slice()),
                "{} carries UTF-16 {:?}",
                name,
                marker
            );
        }
    }
    let manifest = manifest_of(&bytes);
    assert_eq!(manifest.version, 3);
    assert_eq!(manifest.contents, Some(BundleContents::SettingsOnly));

    // Structurally: every value is at an allowlisted path, or is an
    // extension setting its installed manifest allows.
    let config: Value = serde_json::from_slice(&files["config.json"]).unwrap();
    let rules = sensitive::Rules::new(&installed());
    fn leaves(v: &Value, path: &mut Vec<String>, out: &mut Vec<(Vec<String>, Value)>) {
        match v {
            Value::Object(o) if !o.is_empty() => {
                for (k, c) in o {
                    path.push(k.clone());
                    leaves(c, path, out);
                    path.pop();
                }
            }
            _ => out.push((path.clone(), v.clone())),
        }
    }
    let mut out = Vec::new();
    leaves(&config, &mut Vec::new(), &mut out);
    for (path, value) in &out {
        if path[0] == "extensions" {
            assert_eq!(path.len(), 3, "{:?}", path);
            assert_eq!(
                rules.plain_setting(&path[1], &path[2], value),
                sensitive::Plain::Include,
                "{:?}",
                path
            );
            continue;
        }
        // A rule covers everything under its path (a hotkey is one object).
        let allowed = allowlist::FIELDS.iter().any(|(pattern, rule)| {
            matches!(rule, allowlist::Rule::Include(_))
                && pattern.split('.').count() <= path.len()
                && pattern
                    .split('.')
                    .zip(path)
                    .all(|(p, s)| p == "*" || p == s)
        });
        assert!(allowed, "{:?} = {} isn't allowlisted", path, value);
    }
    // The declared-plain extension settings are there.
    assert_eq!(
        config["extensions"]["github"],
        json!({ "trigger": "gh", "show_badge": false, "limit": 25, "sort": "top" })
    );
    assert!(config["extensions"].get("notes").is_none());
    assert!(config["extensions"].get("uninstalled").is_none());
}

#[test]
fn the_preview_names_what_a_settings_only_backup_leaves_out() {
    let _disk = disk();
    no_auto_steering();
    let m = loaded_machine();
    let preview = preview_from(&m.config, &installed(), Some(m.root.path())).unwrap();
    let kinds: Vec<&str> = preview.left_out.iter().map(|i| i.kind).collect();
    for kind in [
        "steering",
        "shortcuts",
        "automations",
        "quick_actions",
        "app_modes",
        "agents",
        "internal_sites",
        "tool_permissions",
        "stores",
        "extension_data",
        "extension_settings",
    ] {
        assert!(kinds.contains(&kind), "{} missing from {:?}", kind, kinds);
    }
    assert_eq!(
        preview
            .left_out
            .iter()
            .find(|i| i.kind == "extension_data")
            .unwrap()
            .count,
        3
    );
    // The hand-edited allowlisted values are flagged, by name only.
    assert_eq!(preview.flagged.len(), 3, "{:?}", preview.flagged);
    let text = serde_json::to_string(&preview).unwrap();
    for marker in LEAK_MARKERS
        .iter()
        .filter(|m| !["deploy", "wiki"].contains(m))
    {
        assert!(
            !text.contains(marker),
            "preview carries {:?}: {}",
            marker,
            text
        );
    }
    // A fresh machine leaves out only what it has.
    let fresh = Machine::new();
    let preview = preview_from(&fresh.config, &installed(), Some(fresh.root.path())).unwrap();
    assert!(preview.flagged.is_empty(), "{:?}", preview.flagged);
    assert!(preview
        .left_out
        .iter()
        .all(|i| ["agents", "app_modes"].contains(&i.kind)));
}

#[test]
fn an_encrypted_backup_carries_everything_and_round_trips() {
    let _disk = disk();
    no_auto_steering();
    let a = loaded_machine();
    let bytes = a.export(ExportMode::Encrypted, Some("pw")).unwrap();
    assert!(bytes.starts_with(ENCRYPTED_MAGIC));
    let err = prepare_import(bytes.clone(), None).err().unwrap();
    assert!(err.downcast_ref::<PassphraseRequired>().is_some());
    let err = prepare_import(bytes.clone(), Some("nope")).err().unwrap();
    assert!(err.downcast_ref::<WrongPassphrase>().is_some());

    let mut b = Machine::new();
    let summary = b.import(&bytes, Some("pw"));
    assert_eq!(summary.kind, BundleKind::Full);
    assert_eq!(spawn(&b.config, 2), spawn(&a.config, 2));
    assert_eq!(b.config.store_url, a.config.store_url);
    assert_eq!(b.config.shortcuts.len(), 1);
    assert_eq!(
        b.config.extensions["github"]["personal_token"],
        "ghp_LEAKleakLEAKleakLEAKleak"
    );
    assert!(
        b.config
            .tool_permissions
            .tools
            .iter()
            .all(|t| t.policy != PolicyKind::Ask),
        "history never travels"
    );
    assert!(b.config.extensions["github"].get("__scratch").is_none());
    assert_eq!(
        b.file("github/token.json").unwrap(),
        a.file("github/token.json").unwrap()
    );
    assert_eq!(
        std::fs::read(&b.steering).unwrap(),
        std::fs::read(&a.steering).unwrap(),
        "steering travels byte for byte (UTF-16 too)"
    );
    assert_eq!(summary.extensions, 3);
    // Device-local values are this machine's.
    assert_eq!(b.config.telemetry.install_id, None);
}

// --- Settings-only import -----------------------------------------------------------

#[test]
fn a_settings_only_import_changes_only_allowlisted_settings() {
    let _disk = disk();
    no_auto_steering();
    let mut a = Machine::new();
    a.config.ui.theme = "dark".into();
    a.config.ui.language = Some("fr".into());
    a.config.hotkey.key = "K".into();
    a.config.updates.channel = crate::config::Channel::Beta;
    a.config.extension_states.insert("todos".into(), false);
    a.config.extensions.insert(
        "github".into(),
        json!({ "trigger": "g", "limit": 3, "note": "free" }),
    );
    let bytes = a.export(ExportMode::SettingsOnly, None).unwrap();

    let mut b = loaded_machine();
    b.config.ui.theme = "light".into();
    let before = b.config.clone();
    let token_before = b.file("github/token.json");
    let steering_before = std::fs::read(&b.steering).unwrap();
    let summary = b.import(&bytes, None);
    assert_eq!(summary.kind, BundleKind::SettingsOnly);
    assert!(summary.settings > 10, "{}", summary.settings);
    assert_eq!(summary.kept_local, vec!["GitHub".to_string()]);
    assert_eq!(b.config.ui.theme, "dark");
    assert_eq!(b.config.ui.language.as_deref(), Some("fr"));
    assert_eq!(b.config.hotkey.key, "K");
    assert_eq!(b.config.updates.channel, crate::config::Channel::Beta);
    assert_eq!(b.config.extension_states.get("todos"), Some(&false));
    // Everything else is untouched.
    assert_eq!(spawn(&b.config, 0), spawn(&before, 0));
    assert_eq!(b.config.store_url, before.store_url);
    assert_eq!(b.config.shortcuts.len(), 1);
    assert_eq!(b.config.macros.len(), 1);
    assert_eq!(b.config.internal_sites.len(), 1);
    assert_eq!(b.config.tool_permissions.tools.len(), 2);
    assert!(b.config.tool_permissions.terminator_mode);
    assert_eq!(b.config.telemetry.install_id, before.telemetry.install_id);
    // Signed in here: GitHub's settings here stay whole.
    assert_eq!(b.config.extensions["github"], before.extensions["github"]);
    assert_eq!(b.config.extensions["notes"], before.extensions["notes"]);
    assert_eq!(b.file("github/token.json"), token_before);
    assert_eq!(std::fs::read(&b.steering).unwrap(), steering_before);
}

#[test]
fn a_hand_made_settings_only_backup_cant_reach_anything_else() {
    let _disk = disk();
    no_auto_steering();
    let mut b = Machine::new();
    b.config.acp.connections = vec![connection("c", "my-agent --key local")];
    b.config.ui.theme = "dark".into();
    b.config.extensions.insert(
        "github".into(),
        json!({ "personal_token": "ghp_local", "trigger": "x" }),
    );
    let manifest = serde_json::to_vec(&json!({
        "format": "kage-backup", "version": 3, "contents": "settings_only",
        "exported_at": "2026-01-01T00:00:00Z", "kage_version": "9.9.9"
    }))
    .unwrap();
    let config = serde_json::to_vec(&json!({
        "version": 1,
        "ui": { "theme": "", "language": "de", "chat_window_x": 5000 },
        "acp": { "connections": [{ "id": "c", "mode": { "type": "local", "spawn_command": "evil" } }] },
        "tool_permissions": { "terminator_mode": true, "trust_all": true },
        "store_url": "https://evil.example/store.json",
        "telemetry": { "enabled": false, "install_id": "evil" },
        "extension_states": { "todos": false, "BAD ID": true },
        "extensions": { "github": { "personal_token": "", "trigger": "", "show_badge": false, "note": "evil" },
                        "notes": { "flag": true, "endpoint": "https://evil.example", "pin": 1234 } },
        "updates": { "channel": "nightly-evil" },
        "hotkey": { "key": "hunter2!" }
    }))
    .unwrap();
    let bundle = zip_of(&[("manifest.json", &manifest), ("config.json", &config)]);
    let before = b.config.clone();
    b.import(&bundle, None);
    assert_eq!(spawn(&b.config, 0), "my-agent --key local");
    assert_eq!(
        b.config.ui.theme, "dark",
        "an empty value keeps this machine's"
    );
    assert_eq!(b.config.ui.language.as_deref(), Some("de"));
    assert_eq!(b.config.ui.chat_window_x, before.ui.chat_window_x);
    assert!(!b.config.tool_permissions.terminator_mode);
    assert!(!b.config.tool_permissions.trust_all);
    assert_eq!(b.config.store_url, None);
    assert!(!b.config.telemetry.enabled, "the opt-out travels");
    assert_eq!(b.config.telemetry.install_id, before.telemetry.install_id);
    assert_eq!(b.config.extension_states.get("todos"), Some(&false));
    assert!(!b.config.extension_states.contains_key("BAD ID"));
    assert_eq!(b.config.updates.channel, before.updates.channel);
    assert_eq!(b.config.hotkey.key, before.hotkey.key);
    // Signed in here: none of GitHub's settings change.
    assert_eq!(b.config.extensions["github"], before.extensions["github"]);
    assert!(
        !b.config.extensions.contains_key("notes"),
        "an installed extension that declares nothing gets nothing"
    );

    // A settings-only backup carrying files was made by hand: refused.
    let bundle = zip_of(&[
        ("manifest.json", &manifest),
        ("config.json", &config),
        ("extension-data/github/token.json", br#"{"token":"evil"}"#),
    ]);
    assert!(prepare_import(bundle, None).is_err());
    let bundle = zip_of(&[
        ("manifest.json", &manifest),
        ("config.json", &config),
        ("steering/user.md", b"evil"),
    ]);
    assert!(prepare_import(bundle, None).is_err());
}

// --- Keeping this machine's values ----------------------------------------------------

#[test]
fn a_to_c_to_a_keeps_every_credential_on_a() {
    let _disk = disk();
    no_auto_steering();
    let mut a = Machine::new();
    a.config.extensions.insert(
        "github".into(),
        json!({ "personal_token": "ghp_AAAA", "trigger": "gh" }),
    );
    a.config.acp.connections = vec![connection("claude", "claude-code-acp --api-key AsecretA")];
    a.write("github/token.json", r#"{"token":"ghp_ADATA"}"#);
    a.write("spotify/creds.json", r#"{"refresh":"AspotifyR"}"#);
    std::fs::write(&a.steering, "Deploy notes\npassword: AsteerSecret9x\n").unwrap();

    // A → C: settings only.
    let mut c = Machine::new();
    c.import(&a.export(ExportMode::SettingsOnly, None).unwrap(), None);
    assert!(!serde_json::to_string(&c.config)
        .unwrap()
        .contains("ghp_AAAA"));
    // C → A: everything C has, encrypted.
    let back = c.export(ExportMode::Encrypted, Some("pw")).unwrap();
    let summary = a.import(&back, Some("pw"));
    assert_eq!(a.config.extensions["github"]["personal_token"], "ghp_AAAA");
    assert!(
        summary.sign_in_again.is_empty(),
        "{:?}",
        summary.sign_in_again
    );
    assert_eq!(
        a.file("github/token.json").unwrap(),
        r#"{"token":"ghp_ADATA"}"#
    );
    assert_eq!(
        a.file("spotify/creds.json").unwrap(),
        r#"{"refresh":"AspotifyR"}"#
    );
    assert!(std::fs::read_to_string(&a.steering)
        .unwrap()
        .contains("AsteerSecret9x"));
    // C's own agent setup is a real value from C's backup.
    assert_eq!(a.config.acp.connections.len(), 1);
}

#[test]
fn an_empty_or_missing_sensitive_value_never_replaces_a_local_one() {
    let _disk = disk();
    no_auto_steering();
    let mut b = Machine::new();
    b.config.extensions.insert(
        "github".into(),
        json!({ "personal_token": "ghp_local", "trigger": "x" }),
    );
    b.write("github/token.json", r#"{"token":"ghp_localdata"}"#);
    b.write("spotify/creds.json", r#"{"refresh":"localR","client":"c"}"#);
    for (settings, token_file, creds) in [
        (
            json!({ "personal_token": "", "trigger": "y" }),
            &b"{}"[..],
            &br#"{"client":"c2"}"#[..],
        ),
        (
            json!({ "trigger": "y" }),
            br#"{"token":""}"#,
            br#"{"client":"c2","refresh":""}"#,
        ),
        (json!(null), b"   ", b"null"),
    ] {
        let mut incoming = Config::default();
        incoming
            .extensions
            .insert("github".into(), settings.clone());
        let bundle = handmade(
            &incoming,
            &[
                ("extension-data/github/token.json", token_file),
                ("extension-data/spotify/creds.json", creds),
            ],
        );
        let (merged, summary) = b.try_import(&bundle, None).unwrap();
        assert_eq!(
            merged.extensions["github"]["personal_token"], "ghp_local",
            "{}",
            settings
        );
        assert!(summary.sign_in_again.is_empty());
        assert_eq!(
            b.file("github/token.json").unwrap(),
            r#"{"token":"ghp_localdata"}"#
        );
        let creds: Value = serde_json::from_str(&b.file("spotify/creds.json").unwrap()).unwrap();
        assert_eq!(creds["refresh"], "localR", "{}", settings);
    }
}

#[test]
fn a_backup_missing_newer_fields_keeps_this_machines() {
    let _disk = disk();
    no_auto_steering();
    let mut b = Machine::new();
    b.config.internal_sites =
        vec![serde_json::from_value(json!({ "id": "s", "name": "S" })).unwrap()];
    b.config.ui.language = Some("ja".into());
    let config = serde_json::to_vec(&json!({ "version": 1, "ui": { "theme": "dark" } })).unwrap();
    let bundle = zip_of(&[("manifest.json", &v1()), ("config.json", &config)]);
    b.import(&bundle, None);
    assert_eq!(b.config.ui.theme, "dark");
    assert_eq!(b.config.ui.language.as_deref(), Some("ja"));
    assert_eq!(b.config.internal_sites.len(), 1);
}

// --- Older bundles ---------------------------------------------------------------------

/// The envelope origin/main writes (same layout and KDF as today).
fn legacy_encrypt(plain: &[u8], pw: &str) -> Vec<u8> {
    encrypt(plain, pw).unwrap()
}

#[test]
fn v1_bundles_still_import_plain_and_encrypted() {
    let _disk = disk();
    no_auto_steering();
    let mut source = Config::default();
    source.ui.language = Some("fr".into());
    source.extensions.insert(
        "github".into(),
        json!({ "personal_token": "legacySecret123" }),
    );
    let plain = handmade(
        &source,
        &[
            ("steering/user.md", b"legacy steering"),
            (
                "extension-data/github/token.json",
                br#"{"token":"legacySecret123"}"#,
            ),
        ],
    );
    for (bytes, pw) in [
        (plain.clone(), None),
        (
            legacy_encrypt(&plain, "old-passphrase"),
            Some("old-passphrase"),
        ),
        (legacy_encrypt(&plain, " "), Some(" ")),
    ] {
        let mut b = Machine::new();
        let summary = b.import(&bytes, pw);
        assert_eq!(b.config.ui.language.as_deref(), Some("fr"));
        assert_eq!(
            b.config.extensions["github"]["personal_token"],
            "legacySecret123"
        );
        assert_eq!(
            std::fs::read_to_string(&b.steering).unwrap(),
            "legacy steering"
        );
        assert!(b
            .file("github/token.json")
            .unwrap()
            .contains("legacySecret123"));
        assert_eq!(summary.extensions, 1);
    }
}

#[test]
fn an_earlier_leave_out_bundle_keeps_this_machines_values() {
    let _disk = disk();
    no_auto_steering();
    let p = sensitive::PLACEHOLDER;
    let mut b = Machine::new();
    b.config.acp.connections = vec![connection("claude", "claude-code-acp --api-key LOCALkey")];
    b.config.store_url = Some("https://me:localpw@store.example.com/x.json".into());
    b.config.tool_permissions.tools = vec![rule("mysql -plocal", PolicyKind::Allow)];
    b.config.extensions.insert(
        "github".into(),
        json!({ "personal_token": "ghp_local", "trigger": "x" }),
    );
    b.write("github/token.json", r#"{"token":"ghp_localdata"}"#);
    std::fs::write(&b.steering, "local steering").unwrap();

    let mut incoming = Config::default();
    incoming.acp.connections = vec![connection("claude", &format!("claude-code-acp {p}"))];
    incoming.store_url = Some(format!("https://me:{p}@store.example.com/x.json"));
    incoming.shortcuts =
        vec![serde_json::from_value(json!({ "name": "s", "arguments": p })).unwrap()];
    incoming
        .extensions
        .insert("github".into(), json!({ "trigger": "y" }));
    let manifest = serde_json::to_vec(&json!({
        "format": "kage-backup", "version": 2, "contents": "leave_out",
        "exported_at": "2026-01-01T00:00:00Z", "kage_version": "0.9.0",
        "excluded": [
            { "kind": "extension_data", "extension_id": "github", "key": "token", "file": "token.json" },
            { "kind": "extension_config", "extension_id": "github", "field": "personal_token" },
            { "kind": "host_setting", "path": "/acp/connections/0/mode/spawn_command" },
            { "kind": "tool_permission_rules" },
            { "kind": "steering_user" },
            { "kind": "extension_config", "extension_id": "spotify", "field": "" },
            { "kind": "a_kind_from_the_future" }
        ],
        "extension_names": { "spotify": "Spotify" }
    }))
    .unwrap();
    let config = serde_json::to_vec(&incoming).unwrap();
    let redacted = format!("notes\napi_key={p}\n");
    let bundle = zip_of(&[
        ("manifest.json", &manifest),
        ("config.json", &config),
        ("steering/user.md", redacted.as_bytes()),
        // A hand-placed sensitive file in a "leave out" bundle never wins.
        (
            "extension-data/spotify/creds.json",
            br#"{"refresh":"attacker"}"#,
        ),
    ]);
    b.write("spotify/creds.json", r#"{"refresh":"localR"}"#);
    let summary = b.import(&bundle, None);
    assert_eq!(spawn(&b.config, 0), "claude-code-acp --api-key LOCALkey");
    assert_eq!(
        b.config.store_url.as_deref(),
        Some("https://me:localpw@store.example.com/x.json")
    );
    assert_eq!(b.config.tool_permissions.tools.len(), 1);
    assert_eq!(b.config.extensions["github"]["personal_token"], "ghp_local");
    // Its credential stays here (the bundle carries none), so GitHub keeps
    // this machine's settings whole, and is named.
    assert_eq!(b.config.extensions["github"]["trigger"], "x");
    assert_eq!(summary.kept_local, vec!["GitHub".to_string()]);
    assert_eq!(
        b.file("github/token.json").unwrap(),
        r#"{"token":"ghp_localdata"}"#
    );
    assert_eq!(
        b.file("spotify/creds.json").unwrap(),
        r#"{"refresh":"localR"}"#
    );
    assert_eq!(
        std::fs::read_to_string(&b.steering).unwrap(),
        "local steering"
    );
    assert_eq!(summary.left_out, 7);
    // Spotify had no settings here; the shortcut's arguments have no twin;
    // the unknown kind is always reported.
    assert_eq!(summary.sign_in_again, vec!["Spotify".to_string()]);
    assert_eq!(summary.reenter.len(), 2, "{:?}", summary.reenter);

    // Listed as left out, carried anyway (under another spelling): refused.
    let manifest = serde_json::to_vec(&json!({
        "format": "kage-backup", "version": 2, "exported_at": "2026-01-01T00:00:00Z",
        "kage_version": "0.9.0",
        "excluded": [{ "kind": "extension_data", "extension_id": "github", "key": "token", "file": "token.json" }],
    }))
    .unwrap();
    let bundle = zip_of(&[
        ("manifest.json", &manifest),
        ("config.json", &config),
        (
            "extension-data/GitHub/TOKEN.json",
            br#"{"token":"attacker"}"#,
        ),
    ]);
    assert!(b.try_import(&bundle, None).is_err());
    assert_eq!(
        b.file("github/token.json").unwrap(),
        r#"{"token":"ghp_localdata"}"#
    );
}

#[test]
fn newer_bundles_and_config_schemas_are_refused_before_anything_changes() {
    let _disk = disk();
    no_auto_steering();
    let b = Machine::new();
    b.write("todos/a.json", r#"["mine"]"#);
    // Config schema v2 under a v1 bundle (the review's probe).
    let config = serde_json::to_vec(&json!({
        "version": 2,
        "shortcuts": [{ "name": "saved", "action_type": "open_url", "url": "https://example.com" }],
        "future_preferences": { "notes": "preserve me" }
    }))
    .unwrap();
    let bundle = zip_of(&[
        ("manifest.json", &v1()),
        ("config.json", &config),
        ("extension-data/todos/a.json", br#"["bundle"]"#),
    ]);
    let err = prepare_import(bundle, None).err().unwrap();
    let schema = err
        .downcast_ref::<UnsupportedConfigSchema>()
        .expect("schema error");
    assert!(schema.newer && schema.stored == 2);
    assert_eq!(b.file("todos/a.json").unwrap(), r#"["mine"]"#);
    // Format v4.
    let manifest = serde_json::to_vec(&json!({
        "format": "kage-backup", "version": 4, "exported_at": "x", "kage_version": "9"
    }))
    .unwrap();
    let cfg = serde_json::to_vec(&Config::default()).unwrap();
    assert!(prepare_import(
        zip_of(&[("manifest.json", &manifest), ("config.json", &cfg)]),
        None
    )
    .is_err());
    // v3 with an unknown kind.
    let manifest = serde_json::to_vec(&json!({
        "format": "kage-backup", "version": 3, "contents": "something_else",
        "exported_at": "x", "kage_version": "9"
    }))
    .unwrap();
    assert!(prepare_import(
        zip_of(&[("manifest.json", &manifest), ("config.json", &cfg)]),
        None
    )
    .is_err());
    // An imported config is saved at this build's schema.
    let mut b = Machine::new();
    let config = serde_json::to_vec(&json!({ "ui": { "theme": "dark" } })).unwrap();
    b.import(
        &zip_of(&[("manifest.json", &v1()), ("config.json", &config)]),
        None,
    );
    assert_eq!(b.config.version, crate::config_migrations::CURRENT_VERSION);
}

#[test]
fn a_full_backup_with_earlier_placeholders_is_v2_and_imports() {
    let _disk = disk();
    no_auto_steering();
    let p = sensitive::PLACEHOLDER;
    let mut c = Machine::new();
    c.config.acp.connections = vec![connection("claude", &format!("claude-code-acp {p}"))];
    let bytes = c.export(ExportMode::Encrypted, Some("pw")).unwrap();
    let archive = decrypt(bytes.clone(), "pw").unwrap();
    assert_eq!(
        manifest_of(&archive).version,
        2,
        "older builds must refuse it"
    );
    let mut a = Machine::new();
    a.config.acp.connections = vec![connection("claude", "claude-code-acp --key A1")];
    a.import(&bytes, Some("pw"));
    assert_eq!(spawn(&a.config, 0), "claude-code-acp --key A1");
}

// --- Writing files: where, how, and all or nothing ------------------------------------

fn local_with_steering(path: &Path) -> Config {
    let mut local = Config::default();
    local.acp.agent.user_steering_path = Some(path.to_string_lossy().into_owned());
    local
}

fn no_temp_files_in(dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(TEMP_PREFIX)
                && !crate::atomic_write::is_temp_file_name(&name)
                && !name.ends_with(".tmp"),
            "left behind {:?}",
            name
        );
    }
}

fn import_at(bytes: &[u8], local: &Config, root: &Path) -> Result<(Config, ImportSummary)> {
    let prepared = prepare_import(bytes, None)?;
    let (config, summary, files) =
        apply_import_at(prepared, local, &installed(), Some(root.to_path_buf()))?.into_parts();
    files.finish()?;
    Ok((config, summary))
}

fn expect_refused(bytes: &[u8], local: &Config, root: &Path) -> UnsafeImportPath {
    let err = import_at(bytes, local, root).expect_err("import must be refused");
    let refused = err
        .downcast_ref::<UnsafeImportPath>()
        .unwrap_or_else(|| panic!("not an UnsafeImportPath: {:#}", err));
    UnsafeImportPath {
        path: refused.path.clone(),
        reason: refused.reason.clone(),
    }
}

#[test]
fn import_writes_steering_only_where_this_machine_keeps_it() {
    let _disk = disk();
    no_auto_steering();
    let scratch = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let chosen_by_archive = scratch.path().join("outside").join("chosen-by-archive.txt");
    let local_path = scratch.path().join("mine").join("user-steering.md");
    let local = local_with_steering(&local_path);
    let mut imported = fake_config();
    imported.acp.agent.user_steering_path = Some(chosen_by_archive.to_string_lossy().into_owned());
    let bundle = handmade(
        &imported,
        &[("steering/user.md", b"archive-controlled content")],
    );

    let (merged, summary) = import_at(&bundle, &local, root.path()).unwrap();
    assert!(!chosen_by_archive.exists());
    assert!(!chosen_by_archive.parent().unwrap().exists());
    assert_eq!(
        std::fs::read_to_string(&local_path).unwrap(),
        "archive-controlled content"
    );
    assert_eq!(
        merged.acp.agent.user_steering_path,
        local.acp.agent.user_steering_path
    );
    assert_eq!(summary.steering_bytes, 26);
    no_temp_files_in(local_path.parent().unwrap());

    // No local choice: the default path under Kage's config dir.
    let default_path = crate::steering_io::default_user_steering_path().unwrap();
    assert!(crate::paths::is_under_isolation_root(&default_path));
    let _ = std::fs::remove_file(&default_path);
    let (merged, _) = import_at(&bundle, &Config::default(), root.path()).unwrap();
    assert!(!chosen_by_archive.exists());
    assert_eq!(merged.acp.agent.user_steering_path, None);
    assert_eq!(
        std::fs::read_to_string(&default_path).unwrap(),
        "archive-controlled content"
    );
    std::fs::remove_file(&default_path).unwrap();
}

#[cfg(unix)]
#[test]
fn import_refuses_symlinks_under_extension_data_and_writes_nothing() {
    let _disk = disk();
    no_auto_steering();
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let scratch = tempfile::tempdir().unwrap();
    let outside = scratch.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let steering = scratch.path().join("user-steering.md");
    let local = local_with_steering(&steering);
    std::os::unix::fs::symlink(&outside, root.join("symlinked-ext")).unwrap();
    let ext = root.join("nested-link-ext");
    std::fs::create_dir_all(ext.join("real")).unwrap();
    std::os::unix::fs::symlink(&outside, ext.join("real").join("sub")).unwrap();
    std::os::unix::fs::symlink(outside.join("target.json"), ext.join("link.json")).unwrap();
    std::os::unix::fs::symlink(outside.join("dangling.json"), ext.join("dangling.json")).unwrap();
    for entry in [
        "extension-data/symlinked-ext/escaped.json",
        "extension-data/nested-link-ext/real/sub/deep/x.json",
        "extension-data/nested-link-ext/real/sub/x.json",
        "extension-data/nested-link-ext/link.json",
        "extension-data/nested-link-ext/dangling.json",
    ] {
        let bundle = handmade(
            &fake_config(),
            &[
                ("steering/user.md", b"steering"),
                ("extension-data/link-ok/a.json", b"[1]"),
                (entry, b"overwritten"),
            ],
        );
        let refused = expect_refused(&bundle, &local, root);
        assert!(refused.reason.contains("symlink"), "{}: {}", entry, refused);
        assert!(!root.join("link-ok").exists(), "{}", entry);
        assert!(!steering.exists(), "{}", entry);
    }
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);

    // The root itself a link.
    let other = scratch.path().join("extension-data");
    std::os::unix::fs::symlink(&outside, &other).unwrap();
    let rel = Path::new("ext").join("a.json");
    assert!(check_destination_under(&other, &rel)
        .unwrap_err()
        .downcast_ref::<UnsafeImportPath>()
        .is_some());
    assert!(write_under(&other, &rel, b"x")
        .unwrap_err()
        .downcast_ref::<UnsafeImportPath>()
        .is_some());
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[test]
fn write_under_creates_real_dirs_and_replaces_files() {
    let _disk = disk();
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().join("extension-data");
    let rel = Path::new("ext").join("deep").join("a.json");
    check_destination_under(&root, &rel).unwrap();
    write_under(&root, &rel, b"one").unwrap();
    write_under(&root, &rel, b"two").unwrap();
    assert_eq!(std::fs::read(root.join(&rel)).unwrap(), b"two");
    no_temp_files_in(&root.join("ext").join("deep"));
    let dir_rel = Path::new("ext").join("deep");
    assert!(check_destination_under(&root, &dir_rel).is_err());
    assert!(write_under(&root, &dir_rel, b"x").is_err());
    assert!(root.join(&rel).is_file());
}

#[test]
fn import_refuses_bad_names_and_file_folder_clashes_and_writes_nothing() {
    let _disk = disk();
    no_auto_steering();
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let scratch = tempfile::tempdir().unwrap();
    let steering = scratch.path().join("user-steering.md");
    let local = local_with_steering(&steering);
    for bad in [
        "extension-data/bad-names/../../escape.json",
        "extension-data/bad-names/CON",
        "extension-data/bad-names/nul.json",
        "extension-data/C:/escape.json",
        "extension-data/bad-names\\escape.json",
        "extension-data/bad-names/a.json:stream",
        "extension-data//abs/escape.json",
        "extension-data/bad-names/GITHUB~1/x.json",
        "extension-data/bad-names/a<b.json",
    ] {
        let bundle = handmade(
            &fake_config(),
            &[
                ("steering/user.md", b"steering"),
                ("extension-data/bad-names-ok/a.json", b"[1]"),
                (bad, b"x"),
            ],
        );
        let refused = expect_refused(&bundle, &local, root);
        assert!(refused.path.starts_with("extension-data/"), "{}", refused);
        assert!(!root.join("bad-names-ok").exists(), "{}", bad);
        assert!(!steering.exists(), "{}", bad);
    }
    for (file, nested) in [
        ("extension-data/clash/a", "extension-data/clash/a/b.json"),
        ("extension-data/clash/A", "extension-data/clash/a/b.json"),
    ] {
        for order in [[file, nested], [nested, file]] {
            let bundle = handmade(
                &fake_config(),
                &[(order[0], b"1".as_slice()), (order[1], b"2".as_slice())],
            );
            let err = import_at(&bundle, &Config::default(), root).unwrap_err();
            assert!(format!("{:#}", err).contains("both a file and a folder"));
            assert!(!root.join("clash").exists(), "{:?}", order);
        }
    }
    let fine = handmade(
        &fake_config(),
        &[
            ("extension-data/clash/a.json", b"1"),
            ("extension-data/clash/a/b.json", b"2"),
        ],
    );
    assert_eq!(
        import_at(&fine, &Config::default(), root)
            .unwrap()
            .1
            .extensions,
        2
    );
}

#[test]
fn archive_paths_are_validated_the_same_on_every_platform() {
    for bad in [
        "../escape",
        "/tmp/escape",
        "C:/escape",
        "c:escape",
        "C:\\escape",
        "..\\escape",
        "ext\\file.json",
        "\\\\server\\share\\x",
        "a\0b",
        "ext/a\nb.json",
        ".",
        "./",
        "//",
        "x/./../y",
        "x/..",
        "ext/file.json:stream",
        "ext/trailing.",
        "ext/trailing ",
        "ext/CON",
        "CON/a.json",
        "ext/con.json",
        "ext/nul.json",
        "ext/NUL",
        "ext/Prn.txt",
        "ext/aux",
        "ext/com1.txt",
        "ext/COM9",
        "ext/lpt1",
        "ext/LPT0.json",
        "ext/COM\u{b9}",
        "ext/CON .txt",
        "ext/CONIN$",
        "ext/conout$.json",
        "ext/GITHUB~1",
        "ext/a?.json",
        "",
    ] {
        assert!(sanitise_archive_path(bad).is_err(), "{:?}", bad);
    }
    assert_eq!(
        sanitise_archive_path("./ext//a.json").unwrap(),
        PathBuf::from("ext").join("a.json")
    );
    for good in [
        "ext/console.json",
        "ext/com10.json",
        "ext/lpt.json",
        "ext/nul-state.json",
        "my-ext_2/nested/key.v2.json",
    ] {
        assert!(sanitise_archive_path(good).is_ok(), "{:?}", good);
    }
}

#[test]
fn an_aliased_name_writes_to_this_machines_file() {
    let _disk = disk();
    no_auto_steering();
    let mut b = Machine::new();
    b.write("todos/state.json", r#"{"items":["local"]}"#);
    let bundle = handmade(
        &Config::default(),
        &[(
            "extension-data/todos/\u{17f}tate.json",
            br#"{"items":["bundle"]}"#,
        )],
    );
    b.import(&bundle, None);
    let same_name_here = std::fs::read_dir(b.root.path().join("todos"))
        .unwrap()
        .count()
        == 1;
    if same_name_here {
        // APFS / NTFS: one file, written under this machine's name.
        assert_eq!(
            b.file("todos/state.json").unwrap(),
            r#"{"items":["bundle"]}"#
        );
    }
}

#[cfg(unix)]
#[test]
fn import_writes_extension_data_and_auto_steering_owner_only() {
    // Extension data can hold an extension's API keys, and the auto doc
    // what Kage learned about the user: owner-only, whatever they had
    // before (an import once wrote them with the umask's default, 0644
    // under umask 022). The user's own doc keeps the mode they gave it.
    use std::os::unix::fs::PermissionsExt;
    let _disk = disk();
    no_auto_steering();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let set_mode =
        |p: &Path, m: u32| std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
    let mut b = Machine::new();
    std::fs::write(&b.steering, "mine").unwrap();
    set_mode(&b.steering, 0o640);
    b.write("todos/notes.json", r#"["mine"]"#);
    let notes = b.root.path().join("todos/notes.json");
    set_mode(&notes, 0o644);
    let auto_doc = Config::get_auto_steering_path().unwrap();
    assert!(crate::paths::is_under_isolation_root(&auto_doc));
    std::fs::create_dir_all(auto_doc.parent().unwrap()).unwrap();
    std::fs::write(&auto_doc, "- likes coffee").unwrap();
    set_mode(&auto_doc, 0o644);

    let source = Machine::new();
    std::fs::write(&source.steering, "BUNDLE USER").unwrap();
    source.write("todos/notes.json", r#"["BUNDLE"]"#);
    source.write("todos/fresh.json", r#"["BUNDLE"]"#);
    // Carries the auto doc too: there's one per process.
    let bytes = source.export(ExportMode::Encrypted, Some("pw")).unwrap();
    let bundle = handmade(
        &Config::default(),
        &[
            ("extension-data/todos/notes.json", br#"["BUNDLE"]"#),
            ("extension-data/todos/fresh.json", br#"["BUNDLE"]"#),
            ("steering/auto.md", b"- likes tea"),
            ("steering/user.md", b"BUNDLE USER"),
        ],
    );
    for (bytes, pw) in [(&bytes, Some("pw")), (&bundle, None)] {
        set_mode(&notes, 0o644);
        set_mode(&auto_doc, 0o644);
        b.import(bytes, pw);
        assert_eq!(b.file("todos/notes.json").unwrap(), r#"["BUNDLE"]"#);
        assert_eq!(mode(&notes), 0o600);
        assert_eq!(mode(&b.root.path().join("todos/fresh.json")), 0o600);
        assert_eq!(std::fs::read_to_string(&b.steering).unwrap(), "BUNDLE USER");
        assert_eq!(
            mode(&b.steering),
            0o640,
            "the user's own doc keeps its mode"
        );
        assert_eq!(mode(&auto_doc), 0o600);
        no_temp_files_in(&b.root.path().join("todos"));
        no_temp_files_in(b.steering.parent().unwrap());
    }
    assert_eq!(std::fs::read_to_string(&auto_doc).unwrap(), "- likes tea");
    no_auto_steering();
}

#[test]
fn export_skips_leftover_temps() {
    let _disk = disk();
    no_auto_steering();
    let m = Machine::new();
    m.write("ext/data.json", "{}");
    for leftover in [
        // An interrupted atomic write.
        ".data.json.kage-tmp-0123456789abcdef".to_string(),
        // An import by an older build.
        ".data.json.4242-7.kage-import.tmp".to_string(),
        // An import by this build: a staged file, a rollback copy.
        format!("{}4242-7.tmp", TEMP_PREFIX),
        format!("{}4242-8~data.json.bak", TEMP_PREFIX),
    ] {
        m.write(&format!("ext/{}", leftover), "{");
    }
    // Look-alikes a user (or an extension) could have named.
    for kept in [
        "notes.kage-import.tmp",
        ".data.json.x-7.kage-import.tmp",
        ".data.json.kage-import.tmp",
    ] {
        m.write(&format!("ext/{}", kept), "mine");
    }
    let bytes = m.export(ExportMode::Encrypted, Some("pw")).unwrap();
    let names: Vec<String> = entries(&decrypt(bytes, "pw").unwrap())
        .into_keys()
        .filter_map(|n| n.strip_prefix("extension-data/").map(str::to_string))
        .collect();
    assert_eq!(
        names,
        vec![
            "ext/.data.json.kage-import.tmp",
            "ext/.data.json.x-7.kage-import.tmp",
            "ext/data.json",
            "ext/notes.kage-import.tmp",
        ]
    );
}

#[test]
fn committed_files_roll_back_when_the_config_cant_be_saved() {
    let _disk = disk();
    no_auto_steering();
    let b = Machine::new();
    std::fs::write(&b.steering, "mine").unwrap();
    b.write("todos/keep.json", r#"["mine"]"#);
    let source = Machine::new();
    std::fs::write(&source.steering, "BUNDLE USER").unwrap();
    source.write("todos/keep.json", r#"["BUNDLE"]"#);
    source.write("todos/new/added.json", r#"["BUNDLE"]"#);
    let bytes = source.export(ExportMode::Encrypted, Some("pw")).unwrap();
    let apply = || {
        apply_import_at(
            prepare_import(bytes.clone(), Some("pw")).unwrap(),
            &b.config,
            &installed(),
            Some(b.root.path().to_path_buf()),
        )
        .unwrap()
    };
    let (_, _, files) = apply().into_parts();
    assert_eq!(std::fs::read_to_string(&b.steering).unwrap(), "BUNDLE USER");
    files.rollback();
    assert_eq!(std::fs::read_to_string(&b.steering).unwrap(), "mine");
    assert_eq!(b.file("todos/keep.json").unwrap(), r#"["mine"]"#);
    assert!(
        !b.root.path().join("todos/new").exists(),
        "created folders go too"
    );
    assert!(!journal::journal_path().exists());
    // Dropped without finishing: the same.
    drop(apply());
    assert_eq!(b.file("todos/keep.json").unwrap(), r#"["mine"]"#);
    no_temp_files_in(&b.root.path().join("todos"));
}

#[test]
fn committed_files_name_the_extensions_whose_data_they_write() {
    // So the import can tell those extensions' open instances to re-read
    // (EXTENSION_VIEWS §20).
    let _disk = disk();
    no_auto_steering();
    let b = Machine::new();
    b.write("todos/list.json", r#"["mine"]"#);
    let source = Machine::new();
    std::fs::write(&source.steering, "BUNDLE USER").unwrap();
    source.write("todos/list.json", r#"["BUNDLE"]"#);
    source.write("todos/more/deep.json", "{}");
    source.write("notes/a.json", "{}");
    let apply = |bytes: Vec<u8>, pw: Option<&str>| {
        apply_import_at(
            prepare_import(bytes, pw).unwrap(),
            &b.config,
            &installed(),
            Some(b.root.path().to_path_buf()),
        )
        .unwrap()
        .into_parts()
        .2
    };
    let files = apply(
        source.export(ExportMode::Encrypted, Some("pw")).unwrap(),
        Some("pw"),
    );
    assert_eq!(files.data_extensions(), ["notes", "todos"]);
    files.finish().unwrap();

    // A settings-only backup writes no data.
    let files = apply(source.export(ExportMode::SettingsOnly, None).unwrap(), None);
    assert!(files.data_extensions().is_empty());
    files.finish().unwrap();
    no_auto_steering();
}

#[test]
fn an_interrupted_import_is_undone_and_its_rollback_copies_kept_until_then() {
    let _disk = disk();
    no_auto_steering();
    let config_path = Config::get_config_path().unwrap();
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, br#"{"ui":{"language":"en"}}"#).unwrap();
    let b = Machine::new();
    b.write("notes/state.json", r#"{"notes":"LOCAL user data"}"#);
    let bundle = handmade(
        &Config::default(),
        &[(
            "extension-data/notes/state.json",
            br#"{"notes":"BUNDLE user data"}"#,
        )],
    );
    // The review's crash: files committed, then the process dies before
    // saving config.json or finishing.
    let applied = apply_import_at(
        prepare_import(bundle.clone(), None).unwrap(),
        &b.config,
        &installed(),
        Some(b.root.path().to_path_buf()),
    )
    .unwrap();
    let (_, _, mut files) = applied.into_parts();
    let mut saved = Config::default();
    saved.ui.language = Some("fr".into());
    files.save_config(&saved).unwrap();
    files.abandon();
    assert!(b.file("notes/state.json").unwrap().contains("BUNDLE"));
    assert!(journal::journal_path().exists());
    let backups = |dir: &Path| {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".bak"))
            .count()
    };
    assert_eq!(backups(&b.root.path().join("notes")), 1);

    // The next import (with no data files) first undoes it, keeping the
    // rollback copy until it's back in place.
    let other = handmade(&Config::default(), &[]);
    import_at(&other, &b.config, b.root.path()).unwrap();
    assert_eq!(
        b.file("notes/state.json").unwrap(),
        r#"{"notes":"LOCAL user data"}"#
    );
    assert_eq!(
        std::fs::read_to_string(&config_path).unwrap(),
        r#"{"ui":{"language":"en"}}"#,
        "config.json is put back with the files"
    );
    assert_eq!(backups(&b.root.path().join("notes")), 0);
    assert!(!journal::journal_path().exists());

    // A committed one is completed at startup instead.
    let applied = apply_import_at(
        prepare_import(bundle, None).unwrap(),
        &b.config,
        &installed(),
        Some(b.root.path().to_path_buf()),
    )
    .unwrap();
    let (_, _, files) = applied.into_parts();
    files.finish().unwrap();
    assert!(b.file("notes/state.json").unwrap().contains("BUNDLE"));
    assert_eq!(journal::recover().unwrap(), journal::Recovery::Nothing);
    assert_eq!(backups(&b.root.path().join("notes")), 0);

    // Without a journal, a rollback copy is never deleted.
    let orphan = b
        .root
        .path()
        .join("notes")
        .join(".kage-import.1-1~state.json.bak");
    std::fs::write(&orphan, "only copy").unwrap();
    import_at(&handmade(&Config::default(), &[]), &b.config, b.root.path()).unwrap();
    assert!(orphan.exists());
    std::fs::remove_file(&config_path).unwrap();
}

// --- Size limits ------------------------------------------------------------------------

#[test]
fn oversized_files_and_envelopes_are_refused_before_they_are_read() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("huge.kage.enc");
    let f = std::fs::File::create(&path).unwrap();
    f.set_len(MAX_FILE_BYTES + 1).unwrap(); // sparse: no disk used
    let started = std::time::Instant::now();
    let err = read_backup_file(&path).unwrap_err();
    assert!(err.downcast_ref::<BackupTooLarge>().is_some(), "{:#}", err);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    // An envelope over the limit is refused before any key derivation or
    // decryption.
    let mut envelope = ENCRYPTED_MAGIC.to_vec();
    envelope.resize((MAX_FILE_BYTES + 1) as usize, 0);
    let started = std::time::Instant::now();
    let err = prepare_import(envelope, Some("pw")).err().unwrap();
    assert!(err.downcast_ref::<BackupTooLarge>().is_some(), "{:#}", err);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    // A small file reads.
    std::fs::write(&path, b"x").unwrap();
    assert_eq!(read_backup_file(&path).unwrap(), b"x");
}

#[test]
fn entries_are_capped_before_they_fill_memory_or_disk() {
    let cfg = serde_json::to_vec(&Config::default()).unwrap();
    let big = vec![0u8; (MAX_ENTRY_BYTES + 1) as usize];
    let bundle = zip_of(&[
        ("manifest.json", &v1()),
        ("config.json", &cfg),
        ("extension-data/todos/big.json", &big),
    ]);
    assert!(
        bundle.len() < 1024 * 1024,
        "the bomb is small: {}",
        bundle.len()
    );
    let err = prepare_import(bundle, None).err().unwrap();
    assert!(err.downcast_ref::<BackupTooLarge>().is_some(), "{:#}", err);
    // An unknown entry isn't even inflated.
    let bundle = zip_of(&[
        ("manifest.json", &v1()),
        ("config.json", &cfg),
        ("themes/huge.bin", &big),
    ]);
    assert!(prepare_import(bundle, None).is_ok());
    // Too many entries.
    let names: Vec<String> = (0..=MAX_ENTRIES).map(|i| format!("x/{}", i)).collect();
    let mut many: Vec<(&str, &[u8])> = vec![("manifest.json", b"{}")];
    many.extend(names.iter().map(|n| (n.as_str(), &b""[..])));
    let err = prepare_import(zip_of(&many), None).err().unwrap();
    assert!(err.downcast_ref::<BackupTooLarge>().is_some(), "{:#}", err);
}

// --- Encryption ---------------------------------------------------------------------------

#[test]
fn encryption_round_trips_and_fails_closed() {
    let envelope = encrypt(b"hello backup", "correct horse battery staple").unwrap();
    assert!(envelope.starts_with(ENCRYPTED_MAGIC));
    assert_eq!(
        decrypt(envelope.clone(), "correct horse battery staple").unwrap(),
        b"hello backup"
    );
    assert!(decrypt(envelope.clone(), "wrong")
        .unwrap_err()
        .downcast_ref::<WrongPassphrase>()
        .is_some());
    assert_ne!(encrypt(b"x", "pw").unwrap(), encrypt(b"x", "pw").unwrap());
    assert!(decrypt(b"too short".to_vec(), "pw").is_err());
    let mut no_magic = envelope;
    no_magic[0] = b'X';
    assert!(decrypt(no_magic, "pw").is_err());
}

// --- Review round: settings-only import edge cases ------------------------------------

fn settings_only_bundle(config: Value) -> Vec<u8> {
    let manifest = serde_json::to_vec(&json!({
        "format": "kage-backup", "version": 3, "contents": "settings_only",
        "exported_at": "2026-01-01T00:00:00Z", "kage_version": "9.9.9"
    }))
    .unwrap();
    let config = serde_json::to_vec(&config).unwrap();
    zip_of(&[("manifest.json", &manifest), ("config.json", &config)])
}

#[test]
fn a_settings_only_import_never_moves_where_a_local_credential_goes() {
    let _disk = disk();
    no_auto_steering();
    let mut b = Machine::new();
    b.config.extensions.insert(
        "github".into(),
        json!({ "server_url": "https://b.example", "personal_token": "B-key", "limit": 1 }),
    );
    b.config.extensions.insert(
        "vault".into(),
        json!({ "endpoint": "https://vault.corp", "api_key": "K" }),
    );
    let before = b.config.extensions.clone();
    let summary = b.import(
        &settings_only_bundle(json!({ "version": 1, "extensions": {
            "github": { "server_url": "https://a.example", "limit": 2 },
            "vault": { "endpoint": "https://evil.example", "flag": true, "count": 3 }
        }})),
        None,
    );
    // Signed in here: neither takes any of the backup's settings, and both
    // are named.
    assert_eq!(b.config.extensions["github"], before["github"]);
    assert_eq!(b.config.extensions["vault"], before["vault"]);
    let mut kept = summary.kept_local.clone();
    kept.sort();
    assert_eq!(kept, vec!["GitHub".to_string(), "vault".to_string()]);
    // With no credential here, a plain endpoint does travel.
    let mut c = Machine::new();
    c.import(
        &settings_only_bundle(json!({ "version": 1, "extensions": {
            "github": { "server_url": "https://a.example" } } })),
        None,
    );
    assert_eq!(
        c.config.extensions["github"]["server_url"],
        "https://a.example"
    );
}

#[test]
fn a_credential_named_setting_never_travels_whatever_its_declared_shape() {
    let _disk = disk();
    no_auto_steering();
    // The review's probes: `api_key` declared as an enum holding a token,
    // and a numeric `pin` enum. GitHub's `keep_session` (boolean) and
    // `auth_method` (enum) are named like credentials too.
    let token = "ghp_0123456789abcdefABCDEF0123";
    let mut items = installed();
    items.push(item(json!({
        "id": "vault", "name": "Vault", "version": "1.0.0", "type": "extension",
        "config": {
            "api_key": { "type": "string", "enum": [token, "none"] },
            "pin": { "type": "number", "enum": [1234, 0] },
            "mode": { "type": "string", "enum": [token, "fast"] },
            "units": { "type": "string", "enum": ["metric", "imperial"] }
        }
    })));
    let mut a = Machine::new();
    a.config.extensions.insert(
        "vault".into(),
        json!({ "api_key": token, "pin": 1234, "mode": token, "units": "metric" }),
    );
    a.config.extensions.insert(
        "github".into(),
        json!({ "keep_session": true, "auth_method": "oauth", "sort": "top" }),
    );
    let bytes = export_from(
        &a.config,
        ExportMode::SettingsOnly,
        None,
        &items,
        Some(a.root.path()),
    )
    .unwrap();
    let text = String::from_utf8_lossy(&entries(&bytes)["config.json"]).into_owned();
    assert!(!text.contains(token), "{}", text);
    assert!(!text.contains("1234"), "{}", text);
    let config: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(config["extensions"]["vault"], json!({ "units": "metric" }));
    assert_eq!(config["extensions"]["github"], json!({ "sort": "top" }));
    // A declared choice that looks like a secret is left out and named.
    let preview = preview_from(&a.config, &items, Some(a.root.path())).unwrap();
    assert_eq!(preview.flagged.len(), 1, "{:?}", preview.flagged);

    // A crafted file carrying them is refused field by field on import.
    let b = Machine::new();
    let crafted = settings_only_bundle(json!({ "version": 1, "extensions": {
        "vault": { "api_key": token, "pin": 1234, "mode": token, "units": "imperial" },
        "github": { "keep_session": true, "auth_method": "oauth" }
    }}));
    let (merged, _) = import_into(&b, &b.config, &items, &crafted, None);
    assert_eq!(merged.extensions["vault"], json!({ "units": "imperial" }));
    assert!(!merged.extensions.contains_key("github"));
}

#[test]
fn a_hotkey_travels_with_its_own_modifiers() {
    let _disk = disk();
    no_auto_steering();
    let mut a = Machine::new();
    a.config.hotkey = crate::config::HotkeyConfig {
        modifiers: vec![],
        key: "F13".into(),
    };
    let bytes = a.export(ExportMode::SettingsOnly, None).unwrap();
    for (bytes, pw) in [
        (bytes, None),
        (
            a.export(ExportMode::Encrypted, Some("pw")).unwrap(),
            Some("pw"),
        ),
    ] {
        let mut b = Machine::new();
        assert_eq!(b.config.hotkey.modifiers, vec!["Alt".to_string()]);
        b.import(&bytes, pw);
        assert!(
            b.config.hotkey.modifiers.is_empty(),
            "{:?}",
            b.config.hotkey
        );
        assert_eq!(b.config.hotkey.key, "F13");
    }
    // A key with unusable modifiers keeps this machine's hotkey whole.
    let mut b = Machine::new();
    b.import(
        &settings_only_bundle(json!({ "version": 1,
            "hotkey": { "key": "K", "modifiers": ["Hyper9"] } })),
        None,
    );
    assert_eq!(b.config.hotkey.key, "Space");
    assert_eq!(b.config.hotkey.modifiers, vec!["Alt".to_string()]);
}

#[test]
fn imports_that_would_outgrow_config_json_are_refused() {
    let _disk = disk();
    no_auto_steering();
    let b = Machine::new();
    // Settings-only: ids past the cap are ignored, so it fits.
    let states: serde_json::Map<String, Value> = (0..40_000)
        .map(|i| (format!("e-{:06}", i), json!(true)))
        .collect();
    let (merged, _) = b
        .try_import(
            &settings_only_bundle(json!({ "version": 1, "extension_states": states })),
            None,
        )
        .unwrap();
    assert!(merged.extension_states.len() <= 600);
    // A full bundle carrying megabytes of settings: refused, nothing written.
    b.write("todos/a.json", r#"["mine"]"#);
    let mut big = Config::default();
    big.extensions.insert(
        "todos".into(),
        json!({ "blob": "x".repeat(2 * 1024 * 1024) }),
    );
    let bundle = handmade(&big, &[("extension-data/todos/a.json", br#"["bundle"]"#)]);
    let err = b.try_import(&bundle, None).unwrap_err();
    assert!(err.downcast_ref::<ConfigTooLarge>().is_some(), "{:#}", err);
    assert_eq!(b.file("todos/a.json").unwrap(), r#"["mine"]"#);
}

#[test]
fn a_rollback_that_cant_reach_a_file_keeps_the_journal() {
    let _disk = disk();
    no_auto_steering();
    let b = Machine::new();
    b.write("notes/state.json", r#"{"notes":"LOCAL"}"#);
    let bundle = handmade(
        &Config::default(),
        &[("extension-data/notes/state.json", br#"{"notes":"BUNDLE"}"#)],
    );
    let (_, _, files) = apply_import_at(
        prepare_import(bundle, None).unwrap(),
        &b.config,
        &installed(),
        Some(b.root.path().to_path_buf()),
    )
    .unwrap()
    .into_parts();
    files.abandon();
    // The folder goes away (an unmounted volume) before the next start.
    let parked = b.root.path().with_extension("parked");
    std::fs::rename(b.root.path(), &parked).unwrap();
    assert!(journal::recover().is_err());
    assert!(journal::journal_path().exists(), "kept for the next try");
    // Back again: the next try puts the file back.
    std::fs::rename(&parked, b.root.path()).unwrap();
    assert_eq!(journal::recover().unwrap(), journal::Recovery::RolledBack);
    assert_eq!(b.file("notes/state.json").unwrap(), r#"{"notes":"LOCAL"}"#);
}

// --- Fix-verification round ----------------------------------------------------------

#[test]
fn a_settings_only_import_keeps_settings_whole_for_a_token_kept_in_extension_data() {
    let _disk = disk();
    no_auto_steering();
    let mut b = Machine::new();
    b.config.extensions.insert(
        "github".into(),
        json!({ "server_url": "https://ghe.b.corp" }),
    );
    b.write("github/token.json", r#"{"token":"ghp_B"}"#);
    let summary = b.import(
        &settings_only_bundle(json!({ "version": 1, "extensions": {
            "github": { "server_url": "https://evil.example", "limit": 4 } } })),
        None,
    );
    assert_eq!(
        b.config.extensions["github"],
        json!({ "server_url": "https://ghe.b.corp" })
    );
    assert_eq!(summary.kept_local, vec!["GitHub".to_string()]);
    // Nothing the backup would change: nothing to say.
    let summary = b.import(
        &settings_only_bundle(json!({ "version": 1, "extensions": {
            "github": { "server_url": "https://ghe.b.corp" } } })),
        None,
    );
    assert!(summary.kept_local.is_empty(), "{:?}", summary.kept_local);
}

#[test]
fn a_full_import_never_points_a_credential_kept_here_somewhere_else() {
    let _disk = disk();
    no_auto_steering();
    let local_settings = json!({ "base_url": "https://a.example", "units": "m" });
    let local_token = r#"{"access_token":"T"}"#;
    let setup = || {
        let mut b = Machine::new();
        b.config
            .extensions
            .insert("todos".into(), local_settings.clone());
        b.write("todos/state.json", local_token);
        b
    };
    let mut incoming = Config::default();
    incoming.extensions.insert(
        "todos".into(),
        json!({ "base_url": "https://evil.example", "units": "km" }),
    );
    // The review's probe: a v1 plain bundle moves `base_url` to another
    // origin, keeps this machine's token file, and carries an unrelated
    // cache.json. Plain or encrypted, the token's file isn't replaced, so
    // Todos keeps this machine's settings and files whole, and is named.
    let plain = handmade(
        &incoming,
        &[("extension-data/todos/cache.json", br#"{"items":[1]}"#)],
    );
    for (bytes, pw) in [
        (plain.clone(), None),
        (encrypt(&plain, "pw").unwrap(), Some("pw")),
    ] {
        let mut b = setup();
        let summary = b.import(&bytes, pw);
        assert_eq!(b.config.extensions["todos"], local_settings);
        assert_eq!(b.file("todos/state.json").unwrap(), local_token);
        assert!(b.file("todos/cache.json").is_none());
        assert_eq!(summary.kept_local, vec!["Todos".to_string()]);
        assert!(summary.sign_in_again.is_empty());
    }
    // Same origin, other path: still this machine's settings, whole.
    let mut same = Config::default();
    same.extensions.insert(
        "todos".into(),
        json!({ "base_url": "https://a.example/v2" }),
    );
    let mut b = setup();
    b.import(&handmade(&same, &[]), None);
    assert_eq!(b.config.extensions["todos"], local_settings);
    // A credential in its settings the bundle doesn't carry: the same.
    let mut b = Machine::new();
    b.config.extensions.insert(
        "todos".into(),
        json!({ "base_url": "https://a.example", "api_key": "K" }),
    );
    let summary = b.import(&plain, None);
    assert_eq!(
        b.config.extensions["todos"]["base_url"],
        "https://a.example"
    );
    assert_eq!(b.config.extensions["todos"]["api_key"], "K");
    assert!(b.file("todos/cache.json").is_none());
    assert_eq!(summary.kept_local, vec!["Todos".to_string()]);
    // A bundle that replaces the token's file with its own brings its
    // settings with it: its token goes where it says.
    let mut b = setup();
    let own = handmade(
        &incoming,
        &[(
            "extension-data/todos/state.json",
            br#"{"access_token":"BUNDLE"}"#,
        )],
    );
    let summary = b.import(&own, None);
    assert_eq!(
        b.config.extensions["todos"]["base_url"],
        "https://evil.example"
    );
    assert!(b.file("todos/state.json").unwrap().contains("BUNDLE"));
    assert!(summary.kept_local.is_empty());
    // Judged per file: a local file holding no credential keeps nothing.
    let mut c = Machine::new();
    c.config
        .extensions
        .insert("todos".into(), local_settings.clone());
    c.write("todos/cache.json", r#"{"items":[0]}"#);
    let summary = c.import(&plain, None);
    assert_eq!(
        c.config.extensions["todos"]["base_url"],
        "https://evil.example"
    );
    assert_eq!(c.file("todos/cache.json").unwrap(), r#"{"items":[1]}"#);
    assert!(summary.kept_local.is_empty());
}

#[test]
fn a_rollback_blocked_by_a_missing_folder_retries_then_steps_aside() {
    let _disk = disk();
    no_auto_steering();
    let b = Machine::new();
    std::fs::write(&b.steering, "LOCAL steering").unwrap();
    let bundle = handmade(
        &Config::default(),
        &[
            ("steering/user.md", b"BUNDLE steering"),
            ("extension-data/brandnew/data.json", br#"{"x":1}"#),
        ],
    );
    let apply = || {
        let (_, _, files) = apply_import_at(
            prepare_import(bundle.clone(), None).unwrap(),
            &b.config,
            &installed(),
            Some(b.root.path().to_path_buf()),
        )
        .unwrap()
        .into_parts();
        files.abandon();
    };
    let steering_dir = b.steering.parent().unwrap().to_path_buf();
    let parked = steering_dir.with_extension("parked");

    // Retried until the folder is back; the folder the import created for
    // a new file doesn't block it.
    apply();
    std::fs::rename(&steering_dir, &parked).unwrap();
    assert!(journal::recover().is_err());
    assert!(!b.root.path().join("brandnew").join("data.json").exists());
    assert!(journal::recover().is_err());
    std::fs::rename(&parked, &steering_dir).unwrap();
    assert_eq!(journal::recover().unwrap(), journal::Recovery::RolledBack);
    assert_eq!(
        std::fs::read_to_string(&b.steering).unwrap(),
        "LOCAL steering"
    );
    assert!(!b.root.path().join("brandnew").exists());

    // A folder that never comes back: set aside after a few tries, its
    // rollback copy kept, and imports work again.
    apply();
    std::fs::rename(&steering_dir, &parked).unwrap();
    let mut outcome = journal::recover();
    for _ in 0..10 {
        if matches!(outcome, Ok(journal::Recovery::SetAside)) {
            break;
        }
        outcome = journal::recover();
    }
    assert_eq!(outcome.unwrap(), journal::Recovery::SetAside);
    assert!(!journal::journal_path().exists());
    let kage = journal::journal_path().parent().unwrap().to_path_buf();
    let aside: Vec<PathBuf> = std::fs::read_dir(&kage)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().contains("import-journal.unresolved-"))
        .collect();
    assert_eq!(aside.len(), 1);
    std::fs::rename(&parked, &steering_dir).unwrap();
    let kept = std::fs::read_dir(&steering_dir)
        .unwrap()
        .flatten()
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .ends_with("~user-steering.md.bak")
        });
    assert!(kept, "the rollback copy is never deleted");
    import_at(&handmade(&Config::default(), &[]), &b.config, b.root.path()).unwrap();
    for p in aside {
        std::fs::remove_file(p).unwrap();
    }
}

#[test]
fn an_in_session_rollback_never_leaves_a_stale_config_for_the_next_start() {
    let _disk = disk();
    no_auto_steering();
    let config_path = Config::get_config_path().unwrap();
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, br#"{"ui":{"language":"en"}}"#).unwrap();
    let b = Machine::new();
    b.write("notes/state.json", r#"{"notes":"LOCAL"}"#);
    let bundle = handmade(
        &Config::default(),
        &[("extension-data/notes/state.json", br#"{"notes":"BUNDLE"}"#)],
    );
    let (_, _, files) = apply_import_at(
        prepare_import(bundle, None).unwrap(),
        &b.config,
        &installed(),
        Some(b.root.path().to_path_buf()),
    )
    .unwrap()
    .into_parts();
    files.rollback();
    assert_eq!(b.file("notes/state.json").unwrap(), r#"{"notes":"LOCAL"}"#);
    // Saved after the failed import: a later start keeps it.
    std::fs::write(&config_path, br#"{"ui":{"language":"ja"}}"#).unwrap();
    assert_eq!(journal::recover().unwrap(), journal::Recovery::Nothing);
    assert_eq!(
        std::fs::read_to_string(&config_path).unwrap(),
        r#"{"ui":{"language":"ja"}}"#
    );
    std::fs::remove_file(&config_path).unwrap();
}

#[test]
fn a_settings_only_import_leaves_room_in_config_json() {
    let _disk = disk();
    no_auto_steering();
    let b = Machine::new();
    // Many settings for extensions not installed here: capped per id.
    let exts: serde_json::Map<String, Value> = (0..512)
        .map(|i| {
            let fields: serde_json::Map<String, Value> = (0..1000)
                .map(|j| (format!("f{:06}", j), json!(j)))
                .collect();
            (format!("x-{:04}", i), Value::Object(fields))
        })
        .collect();
    let result = b.try_import(
        &settings_only_bundle(json!({ "version": 1, "extensions": exts })),
        None,
    );
    match result {
        Ok((merged, _)) => {
            assert!(merged
                .extensions
                .values()
                .all(|v| v.as_object().unwrap().len() <= 128));
            assert!(
                (serde_json::to_string_pretty(&merged).unwrap().len() as u64)
                    <= Config::MAX_CONFIG_SIZE * 3 / 4
            );
        }
        Err(e) => assert!(e.downcast_ref::<ConfigTooLarge>().is_some(), "{:#}", e),
    }
}

// --- Review round: rollback retries ------------------------------------------------

/// A bundle writing a steering doc (in a folder that can go away) and two
/// data files, one new and one replacing this machine's, applied and then
/// left as a crash would leave it. Returns the steering folder and where
/// it's parked.
fn crash_mid_import(b: &Machine) -> (PathBuf, PathBuf) {
    std::fs::write(&b.steering, "LOCAL steering").unwrap();
    b.write("notes/state.json", r#"{"notes":"LOCAL"}"#);
    let bundle = handmade(
        &Config::default(),
        &[
            ("steering/user.md", b"BUNDLE steering"),
            ("extension-data/notes/fresh.json", br#"{"from":"bundle"}"#),
            ("extension-data/notes/state.json", br#"{"notes":"BUNDLE"}"#),
        ],
    );
    let (_, _, files) = apply_import_at(
        prepare_import(bundle, None).unwrap(),
        &b.config,
        &installed(),
        Some(b.root.path().to_path_buf()),
    )
    .unwrap()
    .into_parts();
    files.abandon();
    assert_eq!(b.file("notes/fresh.json").unwrap(), r#"{"from":"bundle"}"#);
    let dir = b.steering.parent().unwrap().to_path_buf();
    let parked = dir.with_extension("parked");
    (dir, parked)
}

#[test]
fn a_rollback_retry_never_deletes_data_created_since() {
    let _disk = disk();
    no_auto_steering();
    let b = Machine::new();
    let (dir, parked) = crash_mid_import(&b);
    // The review's sequence: a first undo can't reach the steering folder,
    // so it stops partway, having removed the new file and put the old one
    // back.
    std::fs::rename(&dir, &parked).unwrap();
    assert!(journal::recover().is_err());
    assert!(b.file("notes/fresh.json").is_none());
    assert_eq!(b.file("notes/state.json").unwrap(), r#"{"notes":"LOCAL"}"#);
    // Kage runs on; the extension writes both files again, one with the
    // very bytes the import had written.
    b.write("notes/fresh.json", r#"{"from":"bundle"}"#);
    b.write("notes/state.json", r#"{"notes":"NEWER"}"#);
    // The retry finishes the undo without touching either.
    std::fs::rename(&parked, &dir).unwrap();
    assert_eq!(journal::recover().unwrap(), journal::Recovery::RolledBack);
    assert_eq!(b.file("notes/fresh.json").unwrap(), r#"{"from":"bundle"}"#);
    assert_eq!(b.file("notes/state.json").unwrap(), r#"{"notes":"NEWER"}"#);
    assert_eq!(
        std::fs::read_to_string(&b.steering).unwrap(),
        "LOCAL steering"
    );
    assert!(!journal::journal_path().exists());
}

#[test]
fn undoing_an_import_keeps_files_written_after_it() {
    let _disk = disk();
    no_auto_steering();
    let b = Machine::new();
    crash_mid_import(&b);
    // Before the next start, the extension rewrites what the import wrote.
    b.write("notes/fresh.json", r#"{"from":"extension"}"#);
    b.write("notes/state.json", r#"{"notes":"NEWER"}"#);
    assert_eq!(journal::recover().unwrap(), journal::Recovery::RolledBack);
    assert_eq!(
        b.file("notes/fresh.json").unwrap(),
        r#"{"from":"extension"}"#
    );
    assert_eq!(b.file("notes/state.json").unwrap(), r#"{"notes":"NEWER"}"#);
    // This machine's earlier copy isn't deleted either: it stays beside it.
    let kept: Vec<String> = std::fs::read_dir(b.root.path().join("notes"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with("~state.json.bak"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    assert_eq!(kept, vec![r#"{"notes":"LOCAL"}"#.to_string()]);
    // What the import alone wrote is still undone.
    assert_eq!(
        std::fs::read_to_string(&b.steering).unwrap(),
        "LOCAL steering"
    );
}

#[test]
fn a_rollback_retry_never_removes_a_config_saved_since() {
    let _disk = disk();
    no_auto_steering();
    let config_path = Config::get_config_path().unwrap();
    let _ = std::fs::remove_file(&config_path);
    let b = Machine::new();
    std::fs::write(&b.steering, "LOCAL steering").unwrap();
    let bundle = handmade(
        &Config::default(),
        &[("steering/user.md", b"BUNDLE steering")],
    );
    let (_, _, files) = apply_import_at(
        prepare_import(bundle, None).unwrap(),
        &b.config,
        &installed(),
        Some(b.root.path().to_path_buf()),
    )
    .unwrap()
    .into_parts();
    // There was no config.json; the import saved one, then Kage died.
    let mut saved = Config::default();
    saved.ui.language = Some("fr".into());
    let mut files = files;
    files.save_config(&saved).unwrap();
    files.abandon();
    let dir = b.steering.parent().unwrap().to_path_buf();
    let parked = dir.with_extension("parked");
    std::fs::rename(&dir, &parked).unwrap();
    assert!(journal::recover().is_err());
    assert!(
        !config_path.exists(),
        "the import's own config.json is undone"
    );
    // Kage starts with defaults and the user saves a setting.
    std::fs::write(&config_path, br#"{"ui":{"language":"ja"}}"#).unwrap();
    std::fs::rename(&parked, &dir).unwrap();
    assert_eq!(journal::recover().unwrap(), journal::Recovery::RolledBack);
    assert_eq!(
        std::fs::read_to_string(&config_path).unwrap(),
        r#"{"ui":{"language":"ja"}}"#
    );
    assert_eq!(
        std::fs::read_to_string(&b.steering).unwrap(),
        "LOCAL steering"
    );
    std::fs::remove_file(&config_path).unwrap();
}

// --- Review round: consent -----------------------------------------------------------

/// The test extensions plus one asking for the network and storage.
fn with_fetcher() -> Vec<InstalledItem> {
    let mut items = installed();
    items.push(item(json!({
        "id": "fetcher", "name": "Fetcher", "version": "1.0.0", "type": "extension",
        "permissions": ["network", "storage"]
    })));
    items
}

fn import_into(
    b: &Machine,
    local: &Config,
    items: &[InstalledItem],
    bytes: &[u8],
    pw: Option<&str>,
) -> (Config, ImportSummary) {
    let (config, summary, files) = apply_import_at(
        prepare_import(bytes, pw).unwrap(),
        local,
        items,
        Some(b.root.path().to_path_buf()),
    )
    .unwrap()
    .into_parts();
    files.finish().unwrap();
    (config, summary)
}

fn grant(caps: &[&str]) -> crate::config::ExtensionGrant {
    crate::config::ExtensionGrant {
        granted: caps.iter().map(|c| c.to_string()).collect(),
        approved_version: "1.0.0".into(),
        approved_at: "2026-01-01T00:00:00Z".into(),
        origin: None,
    }
}

/// The same grants and switches, as every kind of backup carries them:
/// settings-only, a v1 plain full backup, and an encrypted full one.
fn every_kind_of_bundle(grants: Value, states: Value) -> Vec<(Vec<u8>, Option<&'static str>)> {
    let settings = settings_only_bundle(json!({
        "version": 1, "extension_grants": grants.clone(), "extension_states": states.clone()
    }));
    let full = Config {
        extension_grants: serde_json::from_value(grants).unwrap(),
        extension_states: serde_json::from_value(states).unwrap(),
        ..Default::default()
    };
    let plain = handmade(&full, &[]);
    let encrypted = encrypt(&plain, "pw").unwrap();
    vec![(settings, None), (plain, None), (encrypted, Some("pw"))]
}

#[test]
fn a_backup_never_grants_an_extension_what_this_machine_didnt() {
    let _disk = disk();
    no_auto_steering();
    let items = with_fetcher();
    let b = Machine::new();
    let claimed = json!({ "fetcher": {
        "granted": ["network", "storage"], "approved_version": "1.0.0",
        "approved_at": "2026-01-01T00:00:00Z" } });
    // The review's probe: no grant here, a crafted bundle granting
    // ["network", "storage"]. Then: a narrower grant here the bundle widens.
    for local_grant in [None, Some(grant(&["storage"]))] {
        let mut local = b.config.clone();
        if let Some(g) = &local_grant {
            local.extension_grants.insert("fetcher".into(), g.clone());
        }
        for (bytes, pw) in every_kind_of_bundle(claimed.clone(), json!({ "fetcher": true })) {
            let (merged, summary) = import_into(&b, &local, &items, &bytes, pw);
            assert_eq!(
                merged.extension_grants, local.extension_grants,
                "{:?}",
                local_grant
            );
            assert_eq!(
                merged.extension_states.get("fetcher"),
                Some(&false),
                "off until approved here"
            );
            assert_eq!(summary.needs_approval, vec!["Fetcher".to_string()]);
        }
    }
}

#[test]
fn a_backup_switches_an_extension_on_only_within_this_machines_grant() {
    let _disk = disk();
    no_auto_steering();
    let items = with_fetcher();
    let b = Machine::new();
    let mut local = b.config.clone();
    local.extension_states.insert("fetcher".into(), false);
    // No grant here: the backup's "on" (with or without a grant of its
    // own) doesn't switch it on.
    for (bytes, pw) in every_kind_of_bundle(json!({}), json!({ "fetcher": true })) {
        let (merged, summary) = import_into(&b, &local, &items, &bytes, pw);
        assert_eq!(merged.extension_states.get("fetcher"), Some(&false));
        assert_eq!(summary.needs_approval, vec!["Fetcher".to_string()]);
    }
    // This machine's grant covers what it asks for: switched on, nothing
    // to approve, and the grant is this machine's.
    local
        .extension_grants
        .insert("fetcher".into(), grant(&["network", "storage"]));
    for (bytes, pw) in every_kind_of_bundle(json!({}), json!({ "fetcher": true })) {
        let (merged, summary) = import_into(&b, &local, &items, &bytes, pw);
        assert_eq!(merged.extension_states.get("fetcher"), Some(&true));
        assert!(summary.needs_approval.is_empty());
        assert_eq!(merged.extension_grants, local.extension_grants);
    }
    // An extension this machine has, granted, and on, stays so when the
    // backup claims less.
    let mut local = b.config.clone();
    local
        .extension_grants
        .insert("fetcher".into(), grant(&["network", "storage"]));
    let narrower = json!({ "fetcher": { "granted": ["storage"], "approved_version": "1.0.0" } });
    for (bytes, pw) in every_kind_of_bundle(narrower, json!({})) {
        let (merged, summary) = import_into(&b, &local, &items, &bytes, pw);
        assert_eq!(merged.extension_grants, local.extension_grants);
        assert_ne!(merged.extension_states.get("fetcher"), Some(&false));
        assert!(summary.needs_approval.is_empty());
    }
}

// --- Background AI access (EXTENSION_AGENT_CALLS §9) ---------------------------------

fn agent_access(
    enabled: bool,
    agent: Option<&str>,
    allowance: crate::config::AgentAllowance,
) -> crate::config::ExtensionAgentAccess {
    crate::config::ExtensionAgentAccess {
        enabled,
        consent: agent.map(|a| crate::config::AgentConsent {
            agent_fingerprint: format!("fingerprint-of-{a}"),
            agent_label: a.into(),
            granted_at: "2026-11-09T10:00:00+00:00".into(),
        }),
        allowance,
    }
}

/// What this machine may have: nothing, an entry the user switched off,
/// or one switched on for its own agent next to one switched off.
fn local_agent_access() -> Vec<HashMap<String, crate::config::ExtensionAgentAccess>> {
    use crate::config::AgentAllowance::{Low, Normal};
    vec![
        HashMap::new(),
        [("mindmap".to_string(), agent_access(false, None, Normal))].into(),
        [
            (
                "mindmap".to_string(),
                agent_access(true, Some("Local model"), Low),
            ),
            ("notes".to_string(), agent_access(false, None, Normal)),
        ]
        .into(),
    ]
}

/// `config.json` bodies a backup could carry for `extension_agent_access`:
/// another machine's consent for another agent, a bare "on" (which a merge
/// filling gaps from this machine would complete with this machine's
/// consent), an empty map, an unknown allowance, and no field at all.
fn backup_agent_access() -> Vec<Value> {
    let consent = json!({
        "agent_fingerprint": "fingerprint-of-Cloud agent",
        "agent_label": "Cloud agent", "granted_at": "2026-01-01T00:00:00Z"
    });
    vec![
        json!({ "version": 1, "extension_agent_access": {
            "mindmap": { "enabled": true, "consent": consent.clone(), "allowance": "high" },
            "todos": { "enabled": true, "consent": consent.clone() }
        } }),
        json!({ "version": 1, "extension_agent_access": { "mindmap": { "enabled": true } } }),
        json!({ "version": 1, "extension_agent_access": {} }),
        json!({ "version": 1, "extension_agent_access": {
            "mindmap": { "enabled": true, "consent": consent, "allowance": "unlimited" }
        } }),
        json!({ "version": 1 }),
    ]
}

/// `config` in every kind and version of bundle an import reads:
/// settings-only (v3), v1 and v2 (full, and an earlier build's "leave
/// out") plain, and, with `encrypted`, v1, v2 and v3 full encrypted too.
/// (Argon2 is slow in a debug build, so callers encrypt only the cases
/// that matter.)
fn every_kind_and_version(
    config: &Value,
    encrypted: bool,
) -> Vec<(&'static str, Vec<u8>, Option<&'static str>)> {
    let manifest = |version: u32, contents: Option<&str>| {
        let mut m = json!({
            "format": "kage-backup", "version": version,
            "exported_at": "2026-01-01T00:00:00Z", "kage_version": "0.9.0"
        });
        if let Some(c) = contents {
            m["contents"] = json!(c);
        }
        serde_json::to_vec(&m).unwrap()
    };
    let body = serde_json::to_vec(config).unwrap();
    let bundle = |m: Vec<u8>| {
        zip_of(&[
            ("manifest.json", m.as_slice()),
            ("config.json", body.as_slice()),
        ])
    };
    let v1 = bundle(manifest(1, None));
    let v2_full = bundle(manifest(2, Some("full")));
    let mut all = vec![
        (
            "settings-only v3",
            settings_only_bundle(config.clone()),
            None,
        ),
        ("v1 plain", v1.clone(), None),
        ("v2 full plain", v2_full.clone(), None),
        ("v2 leave-out plain", bundle(manifest(2, None)), None),
    ];
    if encrypted {
        let v3_full = bundle(manifest(3, Some("full")));
        all.extend([
            ("v1 encrypted", encrypt(&v1, "pw").unwrap(), Some("pw")),
            (
                "v2 full encrypted",
                encrypt(&v2_full, "pw").unwrap(),
                Some("pw"),
            ),
            (
                "v3 full encrypted",
                encrypt(&v3_full, "pw").unwrap(),
                Some("pw"),
            ),
        ]);
    }
    all
}

#[test]
fn a_settings_only_export_never_carries_background_ai_access() {
    let _disk = disk();
    no_auto_steering();
    let mut a = Machine::new();
    a.config.extension_agent_access = local_agent_access().pop().unwrap();
    let bytes = a.export(ExportMode::SettingsOnly, None).unwrap();
    let files = entries(&bytes);
    let config: Value = serde_json::from_slice(&files["config.json"]).unwrap();
    assert!(config.get("extension_agent_access").is_none(), "{}", config);
    let text = String::from_utf8_lossy(&files["config.json"]);
    for marker in ["fingerprint-of-", "Local model", "agent_fingerprint"] {
        assert!(!text.contains(marker), "carries {:?}: {}", marker, text);
    }

    // An encrypted backup carries config.json whole, this field too: only
    // the import keeps this machine's (below).
    let bytes = a.export(ExportMode::Encrypted, Some("pw")).unwrap();
    let plain = decrypt(bytes, "pw").unwrap();
    let config: Value = serde_json::from_slice(&entries(&plain)["config.json"]).unwrap();
    assert_eq!(
        config["extension_agent_access"],
        serde_json::to_value(&a.config.extension_agent_access).unwrap()
    );
}

#[test]
fn every_import_keeps_this_machines_background_ai_access_whole() {
    let _disk = disk();
    no_auto_steering();
    let b = Machine::new();
    let items = installed();
    // The first two bodies are the attacks (another machine's consent; a
    // bare "on" to be completed with this machine's): every encrypted
    // kind too.
    for (i, config) in backup_agent_access().iter().enumerate() {
        let bundles = every_kind_and_version(config, i < 2);
        for here in local_agent_access() {
            let mut local = b.config.clone();
            local.extension_agent_access = here;
            for (kind, bytes, pw) in &bundles {
                let (merged, summary) = import_into(&b, &local, &items, bytes, *pw);
                assert_eq!(
                    merged.extension_agent_access, local.extension_agent_access,
                    "{kind} with {config}"
                );
                // Nothing the user chose changed, so nothing is listed.
                assert!(summary.needs_approval.is_empty(), "{kind}");
                assert!(summary.kept_local.is_empty(), "{kind}");
            }
        }
    }
}

#[test]
fn a_real_encrypted_backup_never_moves_background_ai_access() {
    let _disk = disk();
    no_auto_steering();
    // Machine A allowed Mind Map to use its cloud agent.
    let mut a = Machine::new();
    a.config.extension_agent_access = [(
        "mindmap".to_string(),
        agent_access(
            true,
            Some("Cloud agent"),
            crate::config::AgentAllowance::High,
        ),
    )]
    .into();
    a.config.ui.theme = "dark".into();
    let bytes = a.export(ExportMode::Encrypted, Some("pw")).unwrap();
    // Onto a new machine, one where the user switched it off (A itself,
    // after making the backup), and one on for its own agent.
    for here in local_agent_access() {
        let mut b = Machine::new();
        b.config.extension_agent_access = here.clone();
        b.import(&bytes, Some("pw"));
        assert_eq!(b.config.extension_agent_access, here);
        assert_eq!(b.config.ui.theme, "dark", "the rest of the backup applies");
    }
}

// --- Review round: numbers and hotkeys ------------------------------------------

/// A v1 plain bundle whose `config.json` is `config`, as written.
fn raw_v1(config: &Value) -> Vec<u8> {
    let manifest = v1();
    let body = serde_json::to_vec(config).unwrap();
    zip_of(&[("manifest.json", &manifest), ("config.json", &body)])
}

#[test]
fn an_out_of_range_number_never_reaches_the_saved_config() {
    let _disk = disk();
    no_auto_steering();
    // The review's probe: opacity 1e100 became INFINITY as an f32, was
    // saved as null, and the next start reset the whole UI section.
    let ui = json!({ "floating_window_opacity": 1e100, "font_size": 200,
                     "speech_silence_timeout": -3.0, "language": "fr", "show_date": true });
    let settings = settings_only_bundle(json!({ "version": 1, "ui": ui.clone(),
        "pocket_tts": { "temp": 7.0, "eos_threshold": -2.0 },
        "automation_power": { "battery_multiplier": 0.0 } }));
    let mut full = serde_json::to_value(Config::default()).unwrap();
    for (k, v) in ui.as_object().unwrap() {
        full["ui"][k] = v.clone();
    }
    full["pocket_tts"]["temp"] = json!(7.0);
    full["pocket_tts"]["eos_threshold"] = json!(-2.0);
    full["automation_power"]["battery_multiplier"] = json!(0.0);
    for bytes in [settings, raw_v1(&full)] {
        let mut b = Machine::new();
        let local = b.config.clone();
        b.import(&bytes, None);
        // What's saved reads back whole: in-range values arrived, every
        // out-of-range one is this machine's.
        let saved = serde_json::to_string_pretty(&b.config).unwrap();
        let reread: Config = serde_json::from_str(&saved).unwrap();
        assert!(b.config.ui.floating_window_opacity.is_finite());
        assert_eq!(reread.ui.language.as_deref(), Some("fr"));
        assert!(reread.ui.show_date);
        assert_eq!(
            reread.ui.floating_window_opacity,
            local.ui.floating_window_opacity
        );
        assert_eq!(reread.ui.font_size, local.ui.font_size);
        assert_eq!(
            reread.ui.speech_silence_timeout,
            local.ui.speech_silence_timeout
        );
        assert_eq!(reread.pocket_tts.temp, local.pocket_tts.temp);
        assert_eq!(reread.pocket_tts.eos_threshold, -2.0);
        assert_eq!(
            reread.automation_power.battery_multiplier,
            local.automation_power.battery_multiplier
        );
    }
}

#[test]
fn a_hotkey_the_runtime_cant_register_never_replaces_this_machines() {
    let _disk = disk();
    no_auto_steering();
    // The review's probe: `Win+K` and `Meta` passed the name list, but the
    // runtime's parser refuses them.
    for modifiers in [json!(["Win"]), json!(["Meta", "Shift"])] {
        let hotkey = json!({ "modifiers": modifiers, "key": "K" });
        let settings = settings_only_bundle(json!({ "version": 1, "hotkey": hotkey.clone(),
            "voice_hotkey": hotkey.clone() }));
        let mut full = serde_json::to_value(Config::default()).unwrap();
        full["hotkey"] = hotkey.clone();
        full["voice_hotkey"] = hotkey;
        for bytes in [settings, raw_v1(&full)] {
            let mut b = Machine::new();
            let local = b.config.clone();
            b.import(&bytes, None);
            assert_eq!(b.config.hotkey.modifiers, local.hotkey.modifiers);
            assert_eq!(b.config.hotkey.key, local.hotkey.key);
            assert!(b.config.voice_hotkey.is_none());
            assert!(allowlist::hotkey_registers(
                &b.config.hotkey.modifiers,
                &b.config.hotkey.key
            ));
        }
    }
    // An alias the runtime reads the same way arrives under the name the
    // Settings picker writes, and registers.
    let mut b = Machine::new();
    b.import(
        &settings_only_bundle(json!({ "version": 1,
            "hotkey": { "modifiers": ["Cmd", "Option"], "key": "K" } })),
        None,
    );
    assert_eq!(b.config.hotkey.modifiers, vec!["Super", "Alt"]);
    assert!(allowlist::hotkey_registers(
        &b.config.hotkey.modifiers,
        &b.config.hotkey.key
    ));
}

#[test]
fn a_file_created_while_importing_is_never_replaced() {
    let _disk = disk();
    no_auto_steering();
    let b = Machine::new();
    let staged = stage(vec![PlannedWrite::Data {
        root: b.root.path().to_path_buf(),
        rel: PathBuf::from("notes/new.json"),
        body: b"BUNDLE".to_vec(),
    }])
    .unwrap();
    let lock = journal::ImportLock::acquire().unwrap().unwrap();
    let mut txn =
        journal::Transaction::begin(lock, staged.files.clone(), staged.created_dirs).unwrap();
    // An extension saves the file after the journal planned a new one: it
    // has no rollback copy, so the import is undone rather than replace it.
    b.write("notes/new.json", "EXTENSION");
    assert!(txn.commit().is_err());
    assert_eq!(b.file("notes/new.json").unwrap(), "EXTENSION");
    assert!(!journal::journal_path().exists());
    no_temp_files_in(&b.root.path().join("notes"));
}

// --- Review round: one rule for a credential kept here ------------------------------

/// The test extensions plus one with a declared key, a numeric account and
/// a webhook address it may copy.
fn with_hooks() -> Vec<InstalledItem> {
    let mut items = installed();
    items.push(item(json!({
        "id": "hooks", "name": "Hooks", "version": "1.0.0", "type": "extension",
        "config": {
            "api_key": { "type": "string", "default": "", "sensitive": true },
            "account_id": { "type": "number", "default": 0 },
            "webhook_url": { "type": "string", "default": "", "backup": "plain" }
        }
    })));
    items
}

/// `extensions` as every kind of backup carries them: settings-only, a v1
/// plain full backup, and an encrypted full one.
fn every_kind_with_extensions(exts: Value) -> Vec<(Vec<u8>, Option<&'static str>)> {
    let settings = settings_only_bundle(json!({ "version": 1, "extensions": exts.clone() }));
    let full = Config {
        extensions: serde_json::from_value(exts).unwrap(),
        ..Default::default()
    };
    let plain = handmade(&full, &[]);
    let encrypted = encrypt(&plain, "pw").unwrap();
    vec![(settings, None), (plain, None), (encrypted, Some("pw"))]
}

#[test]
fn an_extension_signed_in_here_takes_none_of_a_backups_settings() {
    let _disk = disk();
    no_auto_steering();
    let items = with_hooks();
    let b = Machine::new();
    // The review's probe: the key stays, and the backup moves the account
    // (a number) and the webhook (same origin, another path).
    let signed_in = json!({ "api_key": "K", "account_id": 1,
                            "webhook_url": "https://hooks.example/alice" });
    let mut local = b.config.clone();
    local.extensions.insert("hooks".into(), signed_in.clone());
    // Not installed here: only its numbers would travel.
    local
        .extensions
        .insert("vault".into(), json!({ "api_key": "V", "account_id": 1 }));
    let incoming = json!({
        "hooks": { "account_id": 2, "webhook_url": "https://hooks.example/bob" },
        "vault": { "account_id": 2 }
    });
    for (bytes, pw) in every_kind_with_extensions(incoming.clone()) {
        let (merged, summary) = import_into(&b, &local, &items, &bytes, pw);
        assert_eq!(merged.extensions["hooks"], signed_in, "{:?}", summary.kind);
        assert_eq!(merged.extensions["vault"], local.extensions["vault"]);
        let mut kept = summary.kept_local.clone();
        kept.sort();
        assert_eq!(kept, vec!["Hooks".to_string(), "vault".to_string()]);
    }
    // Not signed in here: the same backups' settings arrive.
    let mut plain = b.config.clone();
    plain.extensions.insert(
        "hooks".into(),
        json!({ "account_id": 1, "webhook_url": "https://hooks.example/alice" }),
    );
    plain
        .extensions
        .insert("vault".into(), json!({ "account_id": 1 }));
    for (bytes, pw) in every_kind_with_extensions(incoming) {
        let (merged, summary) = import_into(&b, &plain, &items, &bytes, pw);
        assert_eq!(
            merged.extensions["hooks"],
            json!({ "account_id": 2, "webhook_url": "https://hooks.example/bob" }),
            "{:?}",
            summary.kind
        );
        assert_eq!(merged.extensions["vault"], json!({ "account_id": 2 }));
        assert!(summary.kept_local.is_empty(), "{:?}", summary.kept_local);
    }
    // A full backup carrying its own key brings its settings with it.
    let own = json!({ "hooks": { "api_key": "BACKUP", "account_id": 2,
                                 "webhook_url": "https://hooks.example/bob" } });
    for (bytes, pw) in every_kind_with_extensions(own).into_iter().skip(1) {
        let (merged, summary) = import_into(&b, &local, &items, &bytes, pw);
        assert_eq!(merged.extensions["hooks"]["api_key"], "BACKUP");
        assert_eq!(merged.extensions["hooks"]["account_id"], 2);
        assert!(!summary.kept_local.contains(&"Hooks".to_string()));
    }
}

#[cfg(unix)]
#[test]
fn a_data_file_kage_cant_read_keeps_its_extensions_settings_whole() {
    use std::os::unix::fs::PermissionsExt;
    let _disk = disk();
    no_auto_steering();
    let mode = |p: &Path, m: u32| std::fs::set_permissions(p, std::fs::Permissions::from_mode(m));
    let local_settings = json!({ "server_url": "https://ghe.b.corp", "limit": 1 });
    let token = r#"{"token":"ghp_B"}"#;
    let setup = || {
        let mut b = Machine::new();
        b.config
            .extensions
            .insert("github".into(), local_settings.clone());
        b.write("github/token.json", token);
        mode(&b.root.path().join("github/token.json"), 0o000).unwrap();
        b
    };
    if std::fs::read(setup().root.path().join("github/token.json")).is_ok() {
        // Running as root, which reads anything: nothing to test.
        return;
    }
    let mut a = Machine::new();
    a.config.extensions.insert(
        "github".into(),
        json!({ "server_url": "https://evil.example", "limit": 2 }),
    );
    let full = a.export(ExportMode::Encrypted, Some("pw")).unwrap();
    let settings_only = a.export(ExportMode::SettingsOnly, None).unwrap();
    // A backup carrying its own copy of the file doesn't help: Kage can't
    // tell what this machine's holds.
    a.write("github/token.json", r#"{"token":"ghp_A"}"#);
    let full_with_file = a.export(ExportMode::Encrypted, Some("pw")).unwrap();
    for (bytes, pw) in [
        (&full, Some("pw")),
        (&full_with_file, Some("pw")),
        (&settings_only, None),
    ] {
        // The review's probe: a declared token file at mode 000.
        let mut b = setup();
        let summary = b.import(bytes, pw);
        assert_eq!(
            b.config.extensions["github"], local_settings,
            "{:?}",
            summary.kind
        );
        assert_eq!(summary.kept_local, vec!["GitHub".to_string()]);
        let path = b.root.path().join("github/token.json");
        mode(&path, 0o600).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), token);
    }
    // A link in its folder, which Kage never follows: the same.
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("t.json"), token).unwrap();
    for (bytes, pw) in [(&full, Some("pw")), (&settings_only, None)] {
        let mut b = Machine::new();
        b.config
            .extensions
            .insert("github".into(), local_settings.clone());
        std::fs::create_dir_all(b.root.path().join("github")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("t.json"),
            b.root.path().join("github/linked.json"),
        )
        .unwrap();
        let summary = b.import(bytes, pw);
        assert_eq!(b.config.extensions["github"], local_settings);
        assert_eq!(summary.kept_local, vec!["GitHub".to_string()]);
    }
}

/// The rollback copies of config.json in Kage's folder, with what they hold.
fn config_rollback_copies() -> Vec<(PathBuf, String)> {
    let kage = journal::journal_path().parent().unwrap().to_path_buf();
    std::fs::read_dir(&kage)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with("~config.json.bak"))
        .map(|p| {
            let body = std::fs::read_to_string(&p).unwrap();
            (p, body)
        })
        .collect()
}

#[test]
fn a_recovery_retry_never_puts_back_or_removes_a_config_saved_since() {
    let _disk = disk();
    no_auto_steering();
    let config_path = Config::get_config_path().unwrap();
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let original = r#"{"ui":{"language":"en"}}"#;
    let language = || {
        let body = std::fs::read(&config_path).ok()?;
        let v: Value = serde_json::from_slice(&body).ok()?;
        v["ui"]["language"].as_str().map(String::from)
    };
    let with_language = |l: &str| {
        let mut c = Config::default();
        c.ui.language = Some(l.into());
        c
    };
    // Whether config.json existed before the import, and which recorded
    // step of the first recovery fails: either file's, or config.json's
    // (after it was put back or removed).
    for had_one in [true, false] {
        for fail_after in 0..3 {
            let case = format!("had_one={} fail_after={}", had_one, fail_after);
            match had_one {
                true => std::fs::write(&config_path, original).unwrap(),
                false => {
                    let _ = std::fs::remove_file(&config_path);
                }
            }
            let b = Machine::new();
            b.write("notes/state.json", r#"{"notes":"LOCAL"}"#);
            let bundle = handmade(
                &Config::default(),
                &[
                    ("extension-data/notes/fresh.json", br#"{"from":"bundle"}"#),
                    ("extension-data/notes/state.json", br#"{"notes":"BUNDLE"}"#),
                ],
            );
            let (_, _, mut files) = apply_import_at(
                prepare_import(bundle, None).unwrap(),
                &b.config,
                &installed(),
                Some(b.root.path().to_path_buf()),
            )
            .unwrap()
            .into_parts();
            files.save_config(&with_language("fr")).unwrap();
            files.abandon();
            // The review's sequence: the first recovery can't record a
            // step, Kage runs on and saves newer settings, and recovery
            // runs again.
            journal::fail_record_after(fail_after);
            assert!(journal::recover().is_err(), "{}", case);
            with_language("ja").save().unwrap();
            assert_eq!(
                journal::recover().unwrap(),
                journal::Recovery::RolledBack,
                "{}",
                case
            );
            assert_eq!(language(), Some("ja".into()), "{}", case);
            assert_eq!(
                b.file("notes/state.json").unwrap(),
                r#"{"notes":"LOCAL"}"#,
                "{}",
                case
            );
            assert!(b.file("notes/fresh.json").is_none(), "{}", case);
            assert!(!journal::journal_path().exists(), "{}", case);
            // This machine's earlier config, if it wasn't back yet, stays
            // beside the newer one.
            let copies = config_rollback_copies();
            let expected: Vec<String> = if had_one && fail_after < 2 {
                vec![original.to_string()]
            } else {
                Vec::new()
            };
            assert_eq!(
                copies.iter().map(|(_, b)| b.clone()).collect::<Vec<_>>(),
                expected,
                "{}",
                case
            );
            for (p, _) in copies {
                std::fs::remove_file(p).unwrap();
            }
        }
    }

    // Untouched since, the import's own config.json is still undone, and
    // on Unix even a later save of the very same bytes is a new file.
    std::fs::write(&config_path, original).unwrap();
    let b = Machine::new();
    b.write("notes/state.json", r#"{"notes":"LOCAL"}"#);
    let bundle = handmade(
        &Config::default(),
        &[("extension-data/notes/state.json", br#"{"notes":"BUNDLE"}"#)],
    );
    let apply = || {
        let (_, _, mut files) = apply_import_at(
            prepare_import(bundle.clone(), None).unwrap(),
            &b.config,
            &installed(),
            Some(b.root.path().to_path_buf()),
        )
        .unwrap()
        .into_parts();
        files.save_config(&with_language("fr")).unwrap();
        files.abandon();
    };
    apply();
    assert_eq!(journal::recover().unwrap(), journal::Recovery::RolledBack);
    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
    apply();
    with_language("fr").save().unwrap();
    assert_eq!(journal::recover().unwrap(), journal::Recovery::RolledBack);
    let expected = if cfg!(unix) { Some("fr") } else { Some("en") };
    assert_eq!(language(), expected.map(String::from));
    for (p, _) in config_rollback_copies() {
        std::fs::remove_file(p).unwrap();
    }
    std::fs::remove_file(&config_path).unwrap();
}
