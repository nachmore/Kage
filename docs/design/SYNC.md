# Kage folder sync

Status: being built. WP-S1, WP-S2, WP-S3 (the engine and host
sources) and WP-S5 (extension sync) are in; the app wiring (WP-S4) and
the rest follow §20.

Kage syncs through a folder the user already syncs: Dropbox, iCloud Drive,
OneDrive, Google Drive, Syncthing, a network share, or any folder. There is
no Kage server and no account. Settings, shortcuts and automations, and
extensions with their data sync by default. Chat history can be switched
on. Extensions plug in through a manifest declaration and a small runtime
API. Mind Map is the first extension that syncs records.

This design combines three earlier drafts: one built from the data model,
one from how cloud folders behave, and one from the product and UX.
Appendix A lists every point where they disagreed and what this design
chose.

Read first: [`docs/EXTENSIONS.md`](../EXTENSIONS.md),
[`docs/SECURITY_MODEL.md`](../SECURITY_MODEL.md),
[`docs/I18N.md`](../I18N.md), and the three extension designs sync touches:
[`EXTENSION_VIEWS.md`](EXTENSION_VIEWS.md) §21,
[`EXTENSION_AGENT_CALLS.md`](EXTENSION_AGENT_CALLS.md) §10 and
[`EXTENSION_FILE_TEXT.md`](EXTENSION_FILE_TEXT.md) §10. §21 below lists
what this design changes in them.

Words used here:
- **Computer**: one Kage install with its own user folders. Code calls it
  a *device*.
- **Sync folder**: the folder the user picked. Kage works inside
  `Kage Sync/` in it.
- `<config>` is `crate::paths::config_dir()`, `<data_local>` is
  `crate::paths::data_local_dir()`. On macOS both are the same folder.
- **Replica**: this computer's local database of everything it has merged.
- **Project**: write merged values into the local files Kage reads
  (`config.json`, extension data, steering docs).
- **Capture**: turn a local file change into new values for the replica.

---

## Contents

0. [Owner decisions this design keeps](#0-owner-decisions-this-design-keeps)
1. [Goals and non-goals](#1-goals-and-non-goals)
2. [Key decisions](#2-key-decisions)
3. [How cloud folders behave](#3-how-cloud-folders-behave)
4. [What is synced](#4-what-is-synced)
5. [Layout inside the sync folder](#5-layout-inside-the-sync-folder)
6. [Local state](#6-local-state)
7. [Data model](#7-data-model)
8. [Change tracking and merge](#8-change-tracking-and-merge)
9. [Per-OS and per-computer overrides](#9-per-os-and-per-computer-overrides)
10. [Conflicts list](#10-conflicts-list)
11. [Secrets and encryption](#11-secrets-and-encryption)
12. [Extension API](#12-extension-api)
13. [Settings → Sync](#13-settings--sync)
14. [When sync runs](#14-when-sync-runs)
15. [Failure modes and recovery](#15-failure-modes-and-recovery)
16. [Telemetry](#16-telemetry)
17. [i18n](#17-i18n)
18. [Test plan](#18-test-plan)
19. [Interfaces](#19-interfaces)
20. [Work packages](#20-work-packages)
21. [Changes to other designs](#21-changes-to-other-designs)
22. [Small points for the owner](#22-small-points-for-the-owner)
23. [Building the four designs together](#23-building-the-four-designs-together)
- [Appendix A: how the drafts were combined](#appendix-a-how-the-drafts-were-combined)

---

## 0. Owner decisions this design keeps

- Kage-wide sync through a folder the user chooses. Extensions and
  features plug into it.
- Synced by default: Settings (`config.json` and friends), Shortcuts &
  automations, Extensions and their data (each extension opts in). Chat
  history is off by default and can be switched on per category in
  Settings → Sync.
- Every setting has one shared value. It can be overridden for macOS,
  Windows or Linux, or for this computer only. Paths and hotkeys default
  to per-OS.
- Conflicts: the newest change wins per item. The losing version is kept
  in a conflicts list in Settings → Sync, where it can be restored.
- Mind Map: the map explorer ([`EXTENSION_VIEWS.md`](EXTENSION_VIEWS.md)),
  file text on capture ([`EXTENSION_FILE_TEXT.md`](EXTENSION_FILE_TEXT.md))
  and background organising
  ([`EXTENSION_AGENT_CALLS.md`](EXTENSION_AGENT_CALLS.md), off by default)
  are designed separately. Mind Map actions stay separate from Todos.
  Unassigned actions are owned by "me". Defaults stay: agent tools on,
  fetch link titles on, record source window on.

---

## 1. Goals and non-goals

Goals:
1. One person with 2 to 10 computers, on any mix of macOS, Windows and
   Linux.
2. Never lose data:
   - the sync folder is never the only copy of anything;
   - a value replaced by a concurrent edit is kept in the conflicts list
     for 90 days;
   - a delete can be undone for 30 days;
   - local data is never deleted because the folder is missing, partial,
     rolled back or empty.
3. Survive what cloud folders really do (§3): late, reordered, partial,
   evicted and duplicated files.
4. Converge. Two computers that have read the same files show the same
   values, whatever order the files arrived in.
5. Low surprise:
   - every setting row says where its value applies;
   - nothing that runs programs on its own starts on a computer without
     approval there;
   - bulk deletes ask first.
6. Mixed versions work. A newer Kage can write data that an older Kage
   carries without damage.
7. Nothing on the app startup path. Near zero cost when idle. Slower
   polling on battery.
8. One engine. Settings, lists, steering docs, extension data and chats
   all plug in as sources.

Non-goals (v1):
- A Kage service, accounts, or a network protocol.
- Sharing one sync folder between different people. Real-time
  collaboration.
- Syncing credentials, tool permissions, capability grants or consent
  (§11).
- Copying extension code, app binaries, or files the agent owns
  (`mcp.json`, the agent's session stores).
- Character-level text merge. A text field is one value. A concurrent
  edit keeps the other version in the conflicts list.
- Undo across computers. Each computer's undo stays its own.
- Replacing Backup & restore. The folder is not a backup.
- Mobile clients.

---

## 2. Key decisions

1. **Each computer writes only its own files**, under `devices/<id>/`.
   Every file is written once (temp file, then rename) and never changed.
   Cloud apps have nothing to make a conflicted copy of.
2. **Each computer publishes its full merged state** as content-named
   shards plus a small head file. Any one computer's directory is a
   complete copy, so a slow, missing or deleted directory only delays
   sync.
3. **A missing file never means "deleted".** Only tombstones delete.
   Merging is commutative, associative and idempotent, so arrival order
   doesn't matter.
4. **The unit of merge is a cell**: one field of one record, at one
   scope (all computers, or one OS). The newest cell wins, ordered by a
   hybrid logical clock (HLC). Each cell names up to three earlier values
   it replaced, so a replaced value counts as a conflict only when its
   editor hadn't seen it.
5. **A local edit is stamped against what this computer last showed the
   user**, not against what has arrived since. An edit made over a stale
   view becomes a conflict, never a silent overwrite.
6. **Everything in the folder is encrypted and authenticated**
   (AES-256-GCM) with a random data key. A sync passphrase wraps the key.
   Each computer caches the key in the OS keychain where the keychain is
   reliable, else in an owner-only file. Credentials, tool permissions,
   grants and consent never sync, not even encrypted.
7. **`config.json` stays the effective config of this computer.** Shared
   values and OS overrides live in the replica and the folder.
   "This computer" values never leave it. No code that reads `Config`
   changes.
8. **Synced items that start programs on their own wait for approval on
   each computer**: agent spawn commands, internal-site proxy scripts,
   scheduled and signal automations. Sync never decides where Kage writes
   a file.
9. **Extensions opt in** through a manifest `sync` block and a new `sync`
   capability. There are three modes: whole keys, lists inside a key, and
   record collections. The host does all merging. The merge never depends
   on which Kage or extension version a computer runs.
10. **No folder I/O on the startup path or on Tokio workers.** One engine
    thread. Folder reads run with timeouts. Placeholders are asked to
    download and never waited for.

---

## 3. How cloud folders behave

| Tool | How changes arrive | Conflicted copies | Placeholders | Other quirks |
|---|---|---|---|---|
| iCloud Drive | File by file, often minutes late, in any order | `name 2.ext` | "Optimize Mac Storage" evicts files. Older macOS leaves a `.name.ext.icloud` stub; newer macOS keeps the name and makes the file dataless. Windows uses Cloud Files placeholders. | Names ending `.nosync` are skipped. Leading-dot names are fragile. |
| Dropbox | Fast, but a file can arrive before files written earlier | `name (X's conflicted copy YYYY-MM-DD).ext` (localised), case conflicts | "Online-only": dataless on macOS (File Provider), Cloud Files on Windows | Skips `~$*`, `.~*` and `~*.tmp` names. Selective Sync can leave the folder out on one computer. |
| OneDrive | File by file. Files On-Demand is on by default on Windows | `name-COMPUTERNAME.ext` | Cloud Files placeholders on Windows (recall-on-access attributes); File Provider on macOS | Reserved names and characters. "Always keep on this device" is the pinned state. |
| Google Drive | Stream mode (virtual drive or File Provider) or Mirror mode | `name (1).ext`; two files may share a name | Stream mode fetches on read | Change notifications on the virtual drive are unreliable; reads can stall |
| Syncthing | Only while two peers are online together; temp file then rename | `name.sync-conflict-YYYYMMDD-HHMMSS-ID.ext` | None | `.syncthing.*.tmp` temps. "Receive Only" folders revert local changes. |
| Network share | Immediate | None | None | File watchers are often silent |

What all of them can do:
- deliver a file before the files it refers to;
- show a file that is still downloading (empty, truncated, or a
  placeholder that blocks when read);
- keep an old version of a file for hours while a newer one is in flight;
- bring back a deleted file (version history, a restore, a computer that
  was offline);
- copy a whole profile to a new computer (migration tools, disk images,
  VM clones), so two computers believe they are the same device;
- set file times from the server. File times are hints, never facts.

Rules that follow, used throughout:

| Rule | Statement |
|---|---|
| R1 | **Single writer.** A computer writes only under `devices/<its id>/` and its own key files. Exceptions: the root files, written once at setup or Start over; moving everything into `archive/` on Start over, and deleting archives older than 30 days (§13.6); deleting a retired or removed computer's directory (§8.9). |
| R2 | **Write once.** No file is changed after it is created. A new state is a new file. |
| R3 | **Self-verifying.** Every data file is AEAD-sealed. The authenticated data binds it to the folder, the device, its kind and its name. A content name is a keyed hash of the plaintext. |
| R4 | **Absence is not deletion.** Only tombstones delete. |
| R5 | **Order-free merge.** Merging any set of files in any order, with duplicates, gives the same result. |
| R6 | **Full state per device.** Each computer publishes everything it has merged. Any one directory is a complete copy. |
| R7 | **Strict names.** Lowercase ASCII, no leading dot, no spaces. Anything else is ignored as data and classified (§5.2). |
| R8 | **Never block.** Folder I/O runs on worker threads with timeouts. Placeholders are asked to download, never waited for. |
| R9 | **The folder is never the only copy.** Sync never deletes local data because the folder is missing, unreadable or empty. |
| R10 | **Don't trust clocks.** Order comes from the HLC. Wall time is used only for display and retention ages. |

Designs these rules rule out:
- **One shared state file per category.** Every computer writes it, so
  cloud apps make conflicted copies, and a stale whole-file write erases
  other computers' changes.
- **A SQLite database in the folder.** Cloud apps copy the database and
  its `-wal` file separately and corrupt it.
- **Operation logs.** They need snapshots, compaction and long replays
  for a computer that was offline for weeks. Publishing state gives the
  same convergence with less machinery.

---

## 4. What is synced

### 4.1 Categories

| Category (Settings → Sync) | Default | Contents (namespaces, §7.1) |
|---|---|---|
| Settings | On | `cfg` (single settings), `list.connections`, `list.app_modes`, `list.internal_sites`, `doc.steering` |
| Shortcuts & automations | On | `list.shortcuts`, `list.macros`, `list.quick_actions` (and the `automation_power.*` settings) |
| Extensions and their data | On | `installs`, extension on/off, extension settings, `list.store_sources`, and the data of extensions that opt in |
| Chat history | Off | Kage's own read-only copies of this computer's chats |
| Always | — | Sync's bookkeeping: `sync.devices`, `sync.conflicts`, `sync.ns_state` |

- Category switches are **per computer**. They live in local state
  (§6.1), not in `config.json`. An older Kage drops unknown
  `config.json` fields when it saves, which would silently turn sync off.
- Turning a category off stops sending and receiving it on this computer.
  What is already in the folder stays, and other computers keep using it.

### 4.2 Every file Kage keeps, and what sync does with it

Under `<config>/kage/`:

| File | Sync | Why |
|---|---|---|
| `config.json` | Field by field (§4.3) | Mostly shared; some fields per OS, some per computer, some never |
| `config.json.corrupt-*.bak` | Never | Local recovery copies |
| `.<name>.kage-tmp-*` | Never | Atomic-write temps |
| `user-steering.md` (default path) | Settings, shared (`doc.steering/user`) | The user's own instructions |
| User steering doc at a custom path | This computer | Sync never writes outside Kage's own folders. The path is the user's choice on this computer. Settings says "Not synced: kept at a path you chose". |
| `auto-steering.md` | Settings, shared (`doc.steering/auto`) | Generation already starts from the existing doc and folds in new learning, so starting from the latest synced copy works. Its conflicts are listed under "Generated by Kage" and don't raise the attention badge. |
| `extension-data/<id>/<key>.json` | Extensions, opt-in per key or collection (§12) | Sensitive keys never sync |
| `extensions/`, `themes/`, `command-packs/` | Never as files | Code never comes from the folder. *Which* items are installed syncs (`installs`, §4.5). |
| `search-frecency.json`, `search-frecency.key` | This computer | Ranking is hashed with a per-install key. It can't be compared across computers without sharing the key. |
| `shortcut-history.json` | This computer | Arguments typed into shortcuts: private, little value elsewhere |
| `permission-audit.jsonl` | Never | Audit of decisions made on this computer |
| `activity.db` | Never | App usage on this computer; large and sensitive |
| `system-shortcuts.json` | Never | macOS shortcuts Kage turned off this session. They must be handed back on this computer. |
| `link-metadata.json`, `changelog-cache.json` | Never | Caches |
| `last-session.txt`, `install-source.txt`, `webview-recovery.txt`, `updates/` | Never | Updater and recovery markers |
| `import-journal.json`, `.kage-import.lock` | Never | Backup-import transaction state |

Under `<data_local>/kage/`:

| File | Sync | Why |
|---|---|---|
| `logs/` (`app.jsonl`, `crash.log`, `relaunch.log`) | Never | Diagnostics. Logs stay English and local. |
| `spawned_cli.pid` | Never | Process tracking |
| `sync/` (new, §6) | Never | Sync's own state. It is in the local, non-roaming folder, so a Windows roaming profile or a backup never gives two computers the same identity. |
| `extension-views.json` (planned, EXTENSION_VIEWS) | Never | Window geometry |
| `agent-calls-usage.json`, `machine-id`, `ephemeral-sessions.json`, `agent-scratch/` (planned, EXTENSION_AGENT_CALLS) | Never | Per-computer usage, identity and scratch |
| `file-ref.key` (planned, EXTENSION_FILE_TEXT) | Never | Per-computer key |

Elsewhere:

| File | Sync | Why |
|---|---|---|
| WebView data (`com.kage.launcher/EBWebView` on Windows), the OS cache folder | Never | Browser engine state and caches |
| OS login items (LaunchAgent, Run key or Scheduled Task, autostart `.desktop`) | Never | OS registration per computer |
| The agent's `mcp.json` | Never | Owned by the agent. Often holds tokens in `env`. Kage registers its MCP server on each computer. |
| The agent's session stores | Chat history (off by default): Kage writes its own copy, never the agent's files (§4.6) | Owned by the agent; large and personal |
| `.title-cache.json` in the agent's sessions folder | Chat history: titles sync as part of Kage's copy | Kage-owned, but it sits in the agent's folder, so sync never writes it |

### 4.3 `config.json`, field by field

Rules:
- **Shared**: one value for all computers. Can be overridden per OS or
  for this computer.
- **Per OS**: synced, but a new value applies to this OS by default.
  Can be made shared.
- **This computer**: never leaves the computer. Can't be changed to
  sync. Settings shows the reason.
- **Never**: caches and markers.
- **Guard**: the value can start a program on its own, or turn an
  extension on. It waits for approval on each computer (§8.10).

| Path | Rule | Category | Why |
|---|---|---|---|
| `version` | Never | | The file's schema; `config_migrations` owns it |
| `hotkey`, `clipboard_hotkey`, `inline_assist_hotkey`, `voice_hotkey` | Per OS | Settings | Hotkeys differ by OS (⌘ against Ctrl, OS-reserved combos). One leaf each (modifiers and key together). |
| `acp.connections` | List `list.connections` (§4.4) | Settings | Guard on the spawn command |
| `acp.active_connection_id` | Per OS | Settings | Guard: a connection whose spawn command isn't approved here isn't made active |
| `acp.agent.start_session_on_launch`, `auto_steering_enabled`, `default_model`, `auto_compact_threshold` | Shared | Settings | Preferences. A model id is not a credential. |
| `acp.agent.working_directory` | Per OS | Settings | A path |
| `acp.agent.user_steering_path` | This computer | | Names a file Kage writes. Same rule as backup import. |
| `acp.agent.import_login_shell_env` | This computer | | Decides whether this computer's shell rc files run |
| `ui.theme`, `floating_window_opacity`, `preserve_last_response`, `window_start_position`, `font_size`, `show_time`, `show_date`, `show_speech_button`, `speech_read_back`, `show_response_actions`, `show_floating_toolbar`, `remember_launcher_size`, `speech_silence_timeout`, `time_format`, `date_format`, `language`, `mascot_animations`, `mascot_extension_hints` | Shared | Settings | Preferences. A theme that isn't installed yet falls back as it does today (and store themes install by themselves, §4.5). |
| `ui.chat_window_*`, `ui.last_window_*`, `ui.launcher_width`, `ui.launcher_height` | Never | | Pixel geometry of this screen |
| `ui.speech_voice` | Per OS | Settings | Each OS has its own voices |
| `ui.macos_overlay_panels` | This computer | | An escape hatch for one Mac's OS release |
| `system.auto_start` | This computer | | Mirrors an OS login item. A new computer must not gain one silently. |
| `system.capture_selection`, `show_notifications`, `screen_context` | Shared | Settings | Preferences |
| `system.capture_selection_blocklist` | Per OS | Settings | Process names differ by OS |
| `system.log_buffer_size`, `verbose_frontend_logging`, `log_message_content`, `debug_mode` (top level) | This computer | | Diagnostics |
| `system.last_seen_crash_timestamp` | Never | | Marker |
| `shortcuts` | List `list.shortcuts` | Shortcuts & automations | |
| `macros` | List `list.macros` | Shortcuts & automations | Guard on non-manual triggers |
| `automation_power.*` | Shared | Shortcuts & automations | Preferences |
| `quick_actions.enabled`, `quick_actions.translate_language` | Shared | Shortcuts & automations | Preferences |
| `quick_actions.custom_actions` | List `list.quick_actions` | Shortcuts & automations | |
| `context_rules` | List `list.app_modes` | Settings | Matching is by tokens of the process name, which tolerates OS naming |
| `internal_sites` | List `list.internal_sites` | Settings | Guard on proxy scripts |
| `tool_permissions` (all of it) | This computer | | What the agent may do without asking. A grant on one computer must not grant power on another. |
| `first_run_completed` | This computer | | Each computer runs its own welcome and consent |
| `updates.auto_check`, `silent_update`, `channel` | Shared | Settings | Preferences (a dev channel on one computer only: "Only on this computer") |
| `updates.last_check_time`, `updates.last_updated_version`, `last_extension_update_check` (top level) | Never | | Caches |
| `extensions.<id>.<field>` | Per field (§12.1) | Extensions | Credentials never sync |
| `extension_states.<id>` | Shared | Extensions | Guard: switched on here only if this computer's grant covers the manifest |
| `extension_grants` (including the new `origin`, §4.5) | This computer | | Consent is given per computer |
| `extension_agent_access` (EXTENSION_AGENT_CALLS) | This computer | | Consent and allowance, per computer (EXTENSION_AGENT_CALLS §10). It lands before this design (§23), so C0 adds the row. |
| `pocket_tts.voice`, `temp`, `eos_threshold` | Shared | Settings | Preferences |
| `pocket_tts.enabled`, `port`, `python_path`, `installed`, `auto_start` | This computer | | A local server install; `python_path` is a program Kage starts |
| `store_url` | Shared; held back when it looks like it holds a token | Extensions | `config_export::sensitive::looks_like_secret` |
| `store_sources` | List `list.store_sources` | Extensions | Same hold-back on `url` |
| `mcp_config_path` | This computer | | Names a file Kage writes |
| `auto_update_extensions` | Shared | Extensions | Preference |
| `telemetry.*` | This computer | | Consent and `install_id` are per install |

**Held back.** A value in a field marked "held back" is not published
when it looks like it holds a credential. It stays on this computer as a
pin (§6.2), and Settings → Sync lists it by label (never by value) under
"Not synced: looks like it contains a key". Two tests, both from
`config_export::sensitive`, so sync and backups agree:
- URLs (`store_url`, `store_sources[].url`): `looks_like_secret`.
- Text that starts programs (an agent connection's `mode` when its
  `spawn_command` matches, an internal site's `proxy` when its `script`
  matches): `secret_spans(value)` is non-empty.
  That is the `SECRET_SHAPE` part only (EXTENSION_AGENT_CALLS adds the
  function); `TOKEN_RUN` would hold back too many ordinary commands and
  scripts. This catches the common case of a key pasted into a command
  line, such as `OPENAI_API_KEY=sk-… codex-acp`. The held item still
  syncs; only that one field stays here, and the editor on other
  computers shows it as "set on another computer".

User-authored text elsewhere (prompts, shortcut URLs and arguments,
automation steps, notes) is the user's content, not a managed
credential. It syncs encrypted. Setup says so once (§13.2).

### 4.4 Lists

Lists merge item by item and field by field, never as a whole. Every item
also has two shared cells: `order` (a fractional index string, so two
reorders merge) and `avail` (where the item exists: `all`, or one OS).

| Namespace | Item id | `avail` default | Per-OS fields | This-computer fields | Guarded part |
|---|---|---|---|---|---|
| `list.shortcuts` | `sync_id` (new) | `all`; `run_program` shortcuts: the OS they were made on | `shortcut` (the hotkey), `path`, `working_directory`, `arguments` | | none (a shortcut runs only when the user triggers it) |
| `list.macros` | `sync_id` (new) | `all` | | `trigger.last_run` | non-manual `trigger` together with `steps` |
| `list.quick_actions` | `sync_id` (new) | `all` | | | none |
| `list.app_modes` (`context_rules`) | `sync_id` (new) | `all` | | | none |
| `list.internal_sites` | `id` (existing) | `all` | `proxy` | | `proxy` (script and language); held back when the script looks like it holds a key |
| `list.connections` (`acp.connections`) | `id` (existing) | the OS it was made on | `mode`, `sessions_directory` | | `mode` (local spawn command); held back when the command looks like it holds a key |
| `list.store_sources` | `sync_id` (new) | `all` | | | none; `url` held back when it looks like a token |

Other fields of each item are shared. A per-OS field of an item that is
available everywhere has no value on another OS until someone sets one
there. Until then it is empty there, which the existing editors already
show as "not set" (an empty hotkey registers nothing). The editor shows
the other OS's value as a hint, with ⌘ mapped to Ctrl and back: "On Mac:
⌘⇧N [Use Ctrl+Shift+N here]".

**Stable ids.** Add
`#[serde(default, skip_serializing_if = "String::is_empty")] pub sync_id: String`
to `ShortcutConfig`, `MacroConfig`, `QuickAction`, `ContextRule` and
`StoreSource`. `Config::ensure_item_ids()` runs in `Config::load` and in
`save_config`. It gives every empty `sync_id` (and every empty `id` on
connections and sites) `s` plus 16 random base32 characters. There is no
config version bump: the field is additive and old builds ignore it.

**Lost ids.** An older Kage drops unknown fields when it saves, and an
editor might rebuild an item without its id. Before minting a new id for
an item without one, the engine matches it against the replica's items
for that list, in this order:
1. all known fields equal;
2. the same `name` (or `label`, `friendly_name`), unique on both sides;
3. the same position with at most one field changed.

Anything unmatched becomes a new record, and an unmatched old record a
tombstone. That is noisier but lossless: a concurrent edit to the
"deleted" item on another computer becomes a conflict that can be
restored. The same match runs at join, so the starter app modes every
install seeds don't appear twice.

### 4.5 Extensions

- **Origin.** Kage doesn't record today where an installed item came
  from. From this design on, `commit_extension_install` records it in the
  grant: `extension_grants.<id>.origin` is `"store:<16 hex of SHA-256 of
  the store URL>"`, `"sideload"` or `"bundled"`. It is part of the grant,
  so it is "This computer" and backend-owned like the rest of the grant
  (`preserve_backend_owned_fields`). An older grant without one gets it
  the first time sync runs: `store:<hash>` when the id is in the current
  store's catalog (the same match `check_extension_updates` already uses
  to update it), else `sideload`. §12.3 uses it to keep one extension's
  data away from a different extension that reuses its id.
- **Installed list** (`installs`, id `<kind>:<id>`, value
  `{ version, source: "store" | "sideload" | "bundled", store }`, taken
  from the origin):
  - themes from the store install by themselves on other computers. They
    run no code, and the store's checksum is checked as today;
  - extensions and command packs are **suggestions**: "Mind Map and Timer
    are on your other computers [Review]". Review opens the normal install
    prompt for each;
  - sideloaded items say "Install it on this computer from where you got
    it";
  - an item removed on another computer shows "Timer was removed on
    Work PC [Remove here] [Keep]". Sync never uninstalls by itself,
    because uninstalling deletes the extension's local data;
  - "Not on this computer" and "Keep" add the id to `install_exclusions`
    (§6.1), so the suggestion doesn't come back.
- **On/off** (`extension_states.<id>`): shared, with the consent guard
  (§8.10).
- **Settings** (`extensions.<id>.<field>`): the §12.1 rule.
- **Data**: only for extensions granted `sync`, only what the manifest's
  `sync` block declares, never sensitive keys (§12).

### 4.6 Chat history (off by default)

- Read through `AgentSessionRegistry` (`list_sessions`, `load_session`),
  so it works for every agent format.
- Record `chat.sessions/<origin device>:<provider>:<session id>`:
  `{ title, created, updated, messages, bytes, blob }`. The transcript
  (normalised `AgentMessage[]`, compressed) is a blob.
- Only the origin computer publishes a chat, once it has been idle for
  10 minutes, and at shutdown. Other computers never echo chats.
- Never published: ephemeral sessions (live ones through
  `ephemeral_session::is_active`, and every id in the throwaway-session
  list that EXTENSION_AGENT_CALLS §5.5 adds, which the session providers
  already skip),
  hidden sessions, transcripts over 8 MiB. Budget: 200 MB per computer,
  newest first. Older chats show "Not synced: over the limit".
- Other computers list them under "On your other computers" in the chat
  history, with the computer's name. They open read-only. **Continue
  here** starts a new local chat with the transcript attached, because
  this computer's agent can't resume a session it never had.
- A rename anywhere syncs (`title` cell). Deleting a synced chat on a
  computer that isn't its origin hides it on every computer. Sync never
  deletes an agent's files on another computer. A delete on the origin
  works as today and tombstones the record.
- Turning Chat history on shows the estimate first ("About 120 chats,
  35 MB") and copies chats from the last 90 days, then every new chat.
  The backfill runs only on mains power.

### 4.7 Keeping the tables true

- `src/sync/catalog.rs` holds §4.3 and §4.4 as code (`SYNC_FIELDS`, in
  the style of `config_export::allowlist::FIELDS`). A row is
  `Row { path, rule, category, guard, held_back }`. `Rule` is `Shared`,
  `PerOs`, `ThisComputer(Why)`, `Never`, `List(ns)` or
  `ExtensionSettings`; `Why::reason_key()` names the
  `settings.sync_scope.reason.*` string. A `Shared` or `PerOs` path is
  the `cfg` record id. `SYNC_LISTS` holds §4.4 as `ListSpec`s. C0 lands
  both tables; WP-S3 adds the label keys and `RENAMED`.
- `every_config_field_has_a_sync_decision` walks a maximal `Config` the
  way `allowlist::every_config_field_has_a_decision` does. It fails when
  a field has no row.
- `no_machine_or_never_path_produces_cells` and
  `credential_rules_match_backup` (iterates `sensitive::Rules`) guard the
  other direction.
- `CLAUDE.md` gains one line: "A new `Config` field needs a row in
  `sync::catalog` as well as `config_export::allowlist`."
- A `Config` field that lands before C0 (EXTENSION_AGENT_CALLS'
  `extension_agent_access`) gets its row in C0. One that lands after C0
  adds its row in the same commit (§23). This design's own new field,
  `extension_grants.<id>.origin` (§4.5), is covered by the
  `extension_grants` row; WP-S3 adds it to `ExtensionGrant` with its
  `config_export::allowlist` row (`Leave(DeviceLocal)`), and WP-S5 writes
  it at install.

---

## 5. Layout inside the sync folder

### 5.1 Tree

The user picks a folder. If it isn't already a Kage sync folder, Kage
creates `Kage Sync/` inside it.

```text
Kage Sync/
  kage-sync.json                 plain JSON, written once at setup or reset
  README.txt                     plain text, written once
  keys/
    e0001-<dev>-0001.json        wrapped data key, written once (§11.3)
  devices/
    <dev>/                       one directory per computer; only that computer writes here
      head-000000000042.kage     written once; the owner keeps the newest 3
      shards/<32 hex>.kage       written once; named by content
      blobs/<32 hex>.kage        written once; values over 64 KiB
  archive/                       old generations after "Start over" (deleted after 30 days)
```

- `<dev>` is 16 lowercase Crockford base32 characters (80 random bits).
- The deepest path below the chosen folder is about 75 characters, well
  under Windows' 260-character limit inside a typical OneDrive path.
- No file name or plaintext field holds a user name, host name, path,
  setting name or value.

### 5.2 Names and the file classifier

`src/sync/folder.rs::classify(dir_kind, name) -> FileClass`:

| Class | Pattern | Action |
|---|---|---|
| Head | `^head-(\d{12})\.kage$` | Data |
| Shard, blob | `^[0-9a-f]{32}\.kage$` | Data |
| Key file | `^e(\d{4})-([0-9a-z]{16})-(\d{4})\.json$` in `keys/` | Data |
| Our temp | `^~kage-[0-9a-f]{16}\.tmp$` | Ignore. The owner deletes its own temps older than 10 minutes. Dropbox doesn't upload `~*.tmp` names at all. |
| iCloud stub | `^\.(.+)\.icloud$` | Placeholder for the inner name: ask for a download |
| Provider duplicate | A data name's stem plus extra text: `^(head-\d{12}\|[0-9a-f]{32}\|kage-sync\|e\d{4}-[0-9a-z]{16}-\d{4})(.+)\.(kage\|json)$`. Matches ` (… conflicted copy …)`, ` 2`, ` (1)`, `-COMPUTER`, `.sync-conflict-…`, ` (Case Conflict)`. | In our own directory: run the clone check (§8.9), then delete. Elsewhere: ignore, count in status. A duplicate head elsewhere is opened under its original name; if it opens and differs from the original, that device has a clone. |
| Sync-tool temp | `^\.syncthing\..+\.tmp$`, `^~syncthing~.+\.tmp$`, `^\.~.*`, `^~\$.*` | Ignore |
| OS junk | `config_export::sensitive::is_os_junk` | Ignore |
| Unknown | anything else | Ignore. Never read, never deleted. |

### 5.3 Root files

`kage-sync.json` (plain; it holds nothing secret):

```json
{ "kage_sync": 1, "folder_id": "5qg2vbn8h1xk0d3m", "created_at": "2026-11-09T10:00:00Z", "min_reader": 1 }
```

- Created with exclusive create. If it exists, the user is joining.
- Two computers creating a folder at the same moment produce a provider
  duplicate. The descriptor with the earlier `created_at` wins (ties: the
  smaller `folder_id`). The loser keeps its local data, asks for the
  winner's passphrase, and joins (§8.8). Kage re-reads the descriptor
  10 seconds after creating it, and on every ingest.
- `min_reader` above this build's `READER_VERSION`: sync pauses with
  "Update Kage on this computer to keep syncing". Nothing is written.

`README.txt` is written once, in the creating user's language with
English below. It says what the folder is and asks the user not to edit,
move or rename files in it.

Key files are in §11.3.

### 5.4 Envelope of `.kage` files

```text
magic   "KSYNC"     5 bytes
ver     0x01        1 byte
kind    'H' | 'S' | 'B'   1 byte
epoch   u32 LE      4 bytes
nonce   12 bytes, random
ciphertext = AES-256-GCM(enc key of the epoch, nonce, AAD, deflate(canonical JSON))
tag     16 bytes
```

- AAD: `"kage-sync|1|" + folder_id + "|" + device_id + "|" + kind + "|" + file_name`.
  A file copied into another device's directory, renamed, or moved to
  another folder fails to open. `kind` is the header's letter (`H`, `S`,
  `B`); `file_name` is the whole name, with `.kage`.
- `deflate` is raw DEFLATE (RFC 1951). Golden files of version 1 are in
  `tests/fixtures/sync/envelope/`.
- Limits, checked before decrypting (sealed size) and while inflating
  (plaintext): head 64 KiB, shard 4 MiB, blob 16 MiB of plaintext (a chat
  of 8 MiB escaped as JSON). A file over them opens as `TooLarge`.
- A file shorter than the header and tag opens as `Truncated`; one cut
  later fails its tag (`BadTag`), as the format has no length field. A
  file of zeros (a download not filled in yet) is `Truncated` too.
- Shard and blob names are `hex(HMAC-SHA256(name key, canonical plaintext))[..32]`.
  After decrypting, the reader recomputes the HMAC and requires it to
  equal the name. Two computers with the same state produce the same
  shard names, so a reader skips a shard it already applied, whichever
  directory it is in.
- Canonical JSON: sorted object keys, no whitespace, records sorted by
  id (RFC 8785 style). Numbers print as ECMAScript prints them, so `1.0`
  and `1` are the same value (`canonical::same`).
- Compress, then encrypt. No credential is ever in the stream, so
  compression can't leak one through file sizes.
- A file that fails to open is **not ready**, never "corrupt" at first
  (§8.4).

### 5.5 Head

Plaintext inside the envelope:

```json
{
  "format": 1, "min_reader": 1,
  "device": "k7m2p9xqa3rt5vwe",
  "instance": "c1d93f0a7b2e4d61",
  "seq": 42,
  "written_at": 1762686000000,
  "max_stamp": "0001930b6f1c0004k7m2p9xqa3rt5vwe",
  "kage_version": "0.12.202611091000+stable.ab12cd3",
  "catalog": 1,
  "os": "macos",
  "name": "Work laptop",
  "retired": false,
  "gc_floor": "000192f0aa120000a9x3h6t1qq8m0ve2",
  "seen": { "a9x3h6t1qq8m0ve2": 118 },
  "namespaces": {
    "cfg":                     { "schema": 1, "buckets": 1,  "shards": ["3f1c…"] },
    "list.shortcuts":          { "schema": 1, "buckets": 1,  "shards": ["91aa…"] },
    "ext.mindmap.c.items":     { "schema": 1, "buckets": 16, "shards": ["…"] }
  }
}
```

- `instance` is random per local sync state and per process start. It
  detects clones (§8.9).
- `name` is the user's name for the computer. It is encrypted, so the
  cloud app never sees it.
- `seen[D]` is the highest `seq` of device D whose head this device has
  fully ingested. It drives tombstone cleanup (§8.7).
- `namespaces` lists only the namespaces this computer has enabled. An
  absent namespace means "not synced here", never "empty".
- At most 64 KiB.

### 5.6 Shard

```json
{
  "ns": "list.shortcuts", "schema": 1, "bucket": 0, "of": 1,
  "records": [
    {
      "id": "s7hq2m0c5d1z9w3pa",
      "c": {
        "name@all":         ["Open notes", "0001930b6f1c0004k7m2p9xqa3rt5vwe", 1762686000000],
        "prompt@all":       ["Summarise {text}", "0001930b6f1c0004k7m2p9xqa3rt5vwe", 1762686000000, ["0001930a11220000a9x3h6t1qq8m0ve2"]],
        "shortcut@macos":   ["Cmd+Shift+N", "0001930b6f1c0004k7m2p9xqa3rt5vwe", 1762686000000],
        "shortcut@windows": ["Ctrl+Alt+N", "000193080b0e0001a9x3h6t1qq8m0ve2", 1762640000000],
        "order@all":        ["a0V", "0001930b6f1c0004k7m2p9xqa3rt5vwe", 1762686000000],
        "avail@all":        ["all", "0001930b6f1c0004k7m2p9xqa3rt5vwe", 1762686000000]
      }
    },
    { "id": "s0m4d…", "c": { }, "x": ["0001930c00000000k7m2p9xqa3rt5vwe", 1762690000000, []] }
  ]
}
```

- A cell is `[value, stamp, wall_ms]`, with an optional fourth item:
  up to 3 ancestor stamps (§7.4).
- `x` is the record tombstone: `[stamp, wall_ms, ancestors]`.
- A cell value of `null` means "no value at this scope".
- Values over 64 KiB are `{"$blob": "<32 hex>", "len": n}`.
- Bucket of a record: the first two bytes of `HMAC(name key, ns|id)`
  modulo the bucket count. Counts are powers of two, chosen so the
  average shard is at most 128 KiB of plaintext (up to 256 buckets). A
  change of count rewrites that namespace's shards once.

### 5.7 Blob

`{ "len": n, "data": <json> }` for values over 64 KiB: steering docs,
large extension records, Mind Map document bodies, chat transcripts,
large conflict losers. A reader looks for a blob in every device's
`blobs/`. A computer that holds a blob in its state also publishes its own
copy (R6), except chat transcripts, which only the origin publishes.

### 5.8 Retention and budgets

| Item | Rule |
|---|---|
| Heads | The owner keeps its newest 3. It deletes older ones once a newer head is at least 10 minutes old. |
| Shards, blobs | The owner deletes files no retained head references, after 24 hours unreferenced |
| Temps | The owner deletes its own `~kage-*.tmp` older than 10 minutes |
| Files per device directory | Target ≤ 400 |
| Shard | Average ≤ 128 KiB plaintext, max 4 MiB |
| Head | ≤ 64 KiB |
| Whole folder | Soft warning at 1 GiB: "Kage Sync uses 1.2 GB. Mind Map uses most of it." |

Every computer publishes its full state (R6), blobs included, so the
folder holds one copy per computer: Mind Map with 64 MB of document text
on 4 computers is about 256 MB in the folder, and each new computer
uploads its copy once. That is the price of R6 (any one directory is a
complete copy, and a missing directory only delays sync). The size line
in Settings → Sync shows the whole folder, so the user sees it.

---

## 6. Local state

### 6.1 `<data_local>/kage/sync/`

| File | Content |
|---|---|
| `device.json` | `DevicePrefs` (below), written with `atomic_write::write(…, Perms::Private)` |
| `replica.db` | SQLite (WAL). The source of truth for sync. `config.json` and extension data are projections of it. |
| `engine.lock` | Exclusive OS lock: one engine per profile. A dev build sharing the profile doesn't sync, and its status says so. |
| `key.bin` | The data key, owner-only, only when the key store is the file backend (§11.4) |
| `trash/` | Join backups and bulk-delete copies, 30 days |
| `chats/<dev>/<id>.jsonl` | Read-only mirrors of other computers' chats (Chat history on) |

None of this is synced or included in backups. A backup holds only
`config.json`, the steering docs and `extension-data/`.

```rust
// src/sync/prefs.rs
pub struct DevicePrefs {
    #[serde(default)] pub v: u32,
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub folder: Option<String>,          // the parent the user picked
    #[serde(default)] pub folder_id: Option<String>,
    #[serde(default)] pub device_id: Option<String>,
    #[serde(default)] pub device_name: Option<String>,
    #[serde(default)] pub fingerprint: Option<String>,     // §8.9
    #[serde(default)] pub previous_device_ids: Vec<String>,
    #[serde(default)] pub categories: SyncCategories,
    #[serde(default)] pub ext_data_off: Vec<String>,       // extensions whose data sync is off here
    #[serde(default)] pub paused_until: Option<String>,    // RFC 3339, or "forever"
    #[serde(default)] pub key_store: Option<String>,       // "keychain" | "credential_manager" | "secret_service" | "file"
    #[serde(default)] pub key_epoch: u32,
    #[serde(default)] pub joined_at: Option<String>,
    #[serde(default)] pub approved_runnables: BTreeMap<String, String>,  // guard key → SHA-256 of the runnable part
    #[serde(default)] pub install_exclusions: Vec<String>,
}
pub struct SyncCategories {
    #[serde(default = "default_true")] pub settings: bool,
    #[serde(default = "default_true")] pub automations: bool,
    #[serde(default = "default_true")] pub extensions: bool,
    #[serde(default)]                  pub chats: bool,
}
```

### 6.2 `replica.db`

```sql
CREATE TABLE meta      (k TEXT PRIMARY KEY, v TEXT NOT NULL);   -- hlc_last, my_seq, gc_floor, last_ingest, catalog, …
CREATE TABLE cells     (ns TEXT, id TEXT, field TEXT, scope TEXT, value BLOB, stamp TEXT, wall INTEGER, anc TEXT,
                        PRIMARY KEY (ns, id, field, scope));
CREATE TABLE records   (ns TEXT, id TEXT, bucket INTEGER, del_stamp TEXT, del_wall INTEGER, del_anc TEXT,
                        published INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (ns, id));
CREATE TABLE projected (ns TEXT, id TEXT, field TEXT, hash BLOB, stamp TEXT, scope TEXT,
                        PRIMARY KEY (ns, id, field));          -- what the local file shows, and which cell it came from
CREATE TABLE pins      (ns TEXT, id TEXT, field TEXT, value BLOB, reason TEXT,
                        PRIMARY KEY (ns, id, field));          -- never published; field '*' pins a whole item
CREATE TABLE devices   (dev TEXT PRIMARY KEY, name TEXT, os TEXT, kage_version TEXT, head_seq INTEGER,
                        ingested_seq INTEGER, last_seen INTEGER, instance TEXT, min_reader INTEGER,
                        gc_floor TEXT, state TEXT);
CREATE TABLE applied   (name TEXT PRIMARY KEY);              -- shard names already merged (any device)
CREATE TABLE acks      (ns TEXT, id TEXT, dev TEXT, PRIMARY KEY (ns, id, dev));
CREATE TABLE dirty     (ns TEXT, bucket INTEGER, own INTEGER, PRIMARY KEY (ns, bucket));
CREATE TABLE feed      (ext TEXT, seq INTEGER, coll TEXT, id TEXT, PRIMARY KEY (ext, seq));
CREATE TABLE ext_base  (ext TEXT, coll TEXT, id TEXT, field TEXT, stamp TEXT, hash BLOB,
                        PRIMARY KEY (ext, coll, id, field));   -- what the extension last saw (§12.6)
CREATE TABLE reads     (win TEXT, ext TEXT, key TEXT, stamp TEXT, PRIMARY KEY (win, ext, key));  -- keys mode base
CREATE TABLE ext_view  (win TEXT, ext TEXT, key TEXT, ids BLOB, PRIMARY KEY (win, ext, key));    -- lists mode base (§12.5)
```

- Every multi-row change is one transaction. Each shard is merged in one
  transaction together with its `applied` row, so a crash never leaves a
  half-merged shard.
- `pins.reason`: `this_computer`, `held_back`, `lossy`, `consent`,
  `needs_approval`, `invalid_here`, `not_installed`, `newer_schema`,
  `origin_mismatch`.
- A corrupt `replica.db` is moved aside. The engine rebuilds it from the
  folder and then joins with "use the other computers' values"; local
  differences go to the conflicts list (§15). A replica whose
  `user_version` is newer than this build's (the user went back to an
  older Kage) is moved aside the same way.
- `records.bucket` holds the record's 16-bit bucket hash,
  `Keys::bucket(ns, id, 1 << 16)`. The bucket for any power-of-two count
  up to 65,536 is that hash modulo the count, so a change of count needs
  no rewrite of the table.

---

## 7. Data model

### 7.1 Namespaces

| Namespace | Record id | Source | Echoed by other computers |
|---|---|---|---|
| `cfg` | setting path, e.g. `ui.theme` | single `config.json` settings; one field `v` | yes |
| `cfg.ext` | `<ext id>/<field>` | `config.extensions` | yes |
| `list.<name>` | item id (§4.4) | `config.json` lists | yes |
| `doc.steering` | `user`, `auto` | steering docs; field `text` | yes |
| `installs` | `<kind>:<id>` | installed items | yes |
| `ext.<id>.k` | storage key | keys mode (§12.4) | yes |
| `ext.<id>.l.<key>` | element id | lists mode (§12.5) | yes |
| `ext.<id>.c.<collection>` | record id | collections mode (§12.6) | yes |
| `chat.sessions` | `<origin>:<provider>:<session>` | §4.6 | no |
| `sync.devices` | device id | `revoked`, display name | yes |
| `sync.conflicts` | conflict id | §10 | yes |
| `sync.ns_state` | namespace, or `ext.<id>` for an extension | `purged` stamp (§8.7); `origin` of an extension (§12.3) | yes |

Unknown namespaces from a newer Kage are kept opaque: merged and
republished, never projected (§8.11).

### 7.2 Records and cells

```rust
// src/sync/types.rs
pub struct DeviceId(pub [u8; 10]);                        // 16 base32 chars on disk
pub struct Stamp { pub hlc: u64, pub dev: DeviceId }      // Ord = (hlc, dev); text "<16 hex><16 base32>"
pub enum OsKind { Macos, Windows, Linux }
pub enum Scope { All, Os(OsKind) }
pub struct FieldKey { pub name: String, pub scope: Scope }   // text "name@all", "shortcut@macos"
pub struct Cell { pub value: serde_json::Value, pub stamp: Stamp, pub wall_ms: i64, pub anc: Vec<Stamp> } // anc ≤ 3
pub struct Tomb { pub stamp: Stamp, pub wall_ms: i64, pub anc: Vec<Stamp> } // anc: newest cell stamps the deleter saw, ≤ 3
pub struct Record { pub ns: String, pub id: String, pub cells: BTreeMap<FieldKey, Cell>, pub del: Option<Tomb> }
```

Field names inside a record:

| Field key | Meaning |
|---|---|
| `f@s` | Whole field `f` at scope `s` |
| `f[]<element>@all` | One element of a set field. `<element>` is the element's canonical JSON (≤ 128 characters). Value `true` = present, `false` = removed. A longer element is named `#<32 hex of its SHA-256>`, and its value is the element itself while present. |
| `f{}<key>@all` | One entry of a map field. Value `null` = removed. A key over 128 characters, or one starting with `#`, is named `#<32 hex>` the same way, with the value `[key, value]`. |
| `+<first>@all` | A group of fields that must change together (`+status` holds `{status, doneAt, suggested, confidence}`) |
| `v@all` | The whole record, for a `whole` structure |
| `avail@all`, `order@all` | List items: where the item exists; its position |

Field names hold no `@`, `[]` or `{}` and don't start with `+`
(`split::valid_field_name`); the scope is after the last `@`. A set
comes back from `join` sorted by canonical JSON, and `null` reads as
"absent" (`src/sync/split.rs`).

Because sets, maps and groups are encoded as separate cells, **every cell
merges the same way: the higher stamp wins.** No merge rule is evaluated
from local code. Two computers running different Kage or extension
versions pick the same value for every cell.

A record is **visible** when it has no tombstone, or some cell is newer
than the tombstone. An edit made after a delete brings back the whole
record, with all its cells.

### 7.3 Hybrid logical clock

```rust
// src/sync/hlc.rs — physical part: wall ms << 16; logical part: low 16 bits
pub struct Hlc { last: u64 }
impl Hlc {
    /// A local event. `floor` is the highest stamp of the cell being replaced
    /// (both the one the user saw and the one the replica holds).
    pub fn tick(&mut self, wall_ms: i64, floor: u64) -> u64 {
        let pt = (wall_ms.max(0) as u64) << 16;
        self.last = (self.last + 1).max(pt).max(floor + 1);
        self.last
    }
    /// A remote stamp was merged. Advance, but never past wall + 1 hour, so a
    /// computer with a clock years ahead can't drag every computer with it.
    pub fn observe(&mut self, remote: u64, wall_ms: i64) {
        let cap = ((wall_ms.max(0) as u64) + 3_600_000) << 16;
        self.last = self.last.max(remote.min(cap));
    }
}
```

- `hlc_last` is saved in `meta` in the same transaction as the cells it
  stamps. A restart never issues an older stamp, even after the wall
  clock steps back.
- The `floor` makes a new edit newer than every value of that cell this
  computer has seen, even one stamped by a computer whose clock is far
  ahead. The user's edit wins over what they were looking at.
- Time only decides between edits that didn't see each other. There,
  wall time is the best guess at "newest", and the loser is kept.
- Display times come from `wall_ms`, never from the HLC.

### 7.4 Ancestors

Each cell keeps up to three ancestor stamps: the values it knowingly
replaced.

```text
anc(new cell) =
  if base is None:            []
  elif base.dev == me:        base.anc              // a run of my own edits counts as one edit
  else:                       ([base.stamp] ++ base.anc)[..3]
```

`base` is the cell the user actually saw: the one recorded in `projected`
when the local value was written (§8.2). It is **not** whatever the replica
holds now. A value that arrived after the last projection isn't in `anc`,
so an edit made over it is concurrent with it (§8.5).

### 7.5 Effective value on this computer

For field `f` of a record, on a computer running OS `o`:

```text
effective(f) =
  1. pin(f)          if a pin exists (this computer, held back, lossy, consent, …)
  2. cell(f@o)       if present and not null
  3. cell(f@all)     if present and not null
  4. the local value (absence never overrides: R4)
```

A list item exists on this computer when the record is visible, `avail`
is `all` or `o`, and no `*` pin says "this computer only".

---

## 8. Change tracking and merge

### 8.1 One tick

All of this runs on the `kage-sync` thread (§14).

```text
fn tick(wake):
  if paused or locked or folder unavailable: capture only (cells stay in the replica); return
  check_root()                       // descriptor present, folder_id matches, key epoch known (§8.9, §11.3)
  check_identity()                   // fingerprint, own directory, clones and rewinds (§8.9)
  for source in enabled sources:     // capture (§8.2); only sources marked dirty, all of them on the first tick
      capture(source)
  ingest()                           // §8.4: read heads and shards of other devices, merge (§8.5)
  project(changed)                   // §8.6: write effective values into local files, with guards (§8.10)
  if own edits pending and debounce elapsed: publish()        // §8.3
  elif echo due: publish()
  gc()                               // own old files, tombstones (§8.7)
  status.emit()
```

### 8.2 Capture: local change → cells

Sources say what is in the local files now (§19.3). The engine compares
each field with `projected`:

```text
fn capture(source):
  local = source.read_local()                       // ns → id → fields (only syncable fields)
  for each (ns, id, field) in local ∪ projected:
    p = projected[ns, id, field]
    if hash(local value) == p.hash: continue       // unchanged since we last wrote or read it
    if pinned(ns, id, field): update the pin; continue
    if held_back(field, value): pin(held_back); continue
    scope = edit_target(record, field)              // §9.2
    base  = p.stamp's cell, if any                  // what the user saw
    cur   = replica cell at (field, scope)
    if cur.value == value: projected = (hash, cur); continue   // same value: no new stamp, no echo loop
    stamp = hlc.tick(now, floor = max(base.stamp, cur.stamp))
    write cell (value, stamp, now, anc(base)); projected = (hash, new cell); mark bucket dirty (own)
  for each item in projected but missing locally, that exists on this computer:
    if it belongs to an extension that is no longer installed here: pin(not_installed); drop projected; continue
    write tombstone (stamp, now, anc = newest projected cell stamps ≤ 3)
```

- **Uninstalling is not deleting.** `uninstall_extension` removes the
  extension's `extensions.<id>`, `extension_states.<id>` and its
  `extension-data/<id>/` files on this computer. Capture must not turn
  that into tombstones, or one uninstall would wipe the extension's
  settings and data on every computer. So, for an extension that isn't
  installed here (`ConfigPort::installed`), capture never writes cells or
  tombstones for `cfg.ext/<id>/*`, `extension_states.<id>` or
  `ext.<id>.*`: their projected rows are dropped and the synced values
  get `not_installed` pins. The `installs` record is the one thing that
  says it was removed (§4.5). Reinstalling projects the synced values
  back (§12.8).
- **Initial pushes take no floor.** A collections-mode push with
  `initial: true` (§12.6) stamps each cell from the record's own
  `updatedAt`, capped at now, with no `floor` and without advancing the
  HLC. A local copy that is older than the synced one then loses, as it
  should, and is kept in the conflicts list when the two were concurrent.

- Hooks only wake the engine: `Config::save_recorded` (after a
  successful write), `save_extension_data` and `delete_extension_data`,
  steering writes, `ext_sync_push`. The diff is what counts, so a code
  path nobody remembered is still covered.
- Edits made while Kage wasn't running are caught on the first tick.
  They are stamped at capture time, not file time. That's conservative:
  an old offline edit can win against a newer remote one, but the remote
  one is then a conflict, not lost.
- Hooks carry an origin: `User`, `Import` (a backup restore, §8.10),
  `Reset` (`Config::load` reset `config.json`, §8.10).

### 8.3 Publish

```text
fn publish():                                       // engine thread only
  for ns in enabled namespaces that echo, or that have own edits:
    b = bucket_count(ns)                            // power of two; grows above 1.5× target, shrinks below 0.25×
    for bucket in dirty(ns) (all buckets if b changed):
      plain = canonical({ ns, schema, bucket, of: b, records: replica.records(ns, bucket, b) })
                                                    // pins and held-back values are never in records
      name = hmac32(name_key, plain)
      if not exists(own/shards/name.kage): write_once(own/shards/name.kage, seal('S', name, plain))
      head.namespaces[ns].shards[bucket] = name
  write blobs first, shards next, the head last:
  write_once(own/head-{seq+1:012}.kage, seal('H', name, head))
  meta.my_seq += 1; records.published = 1; clear dirty
  gc_own_files()                                    // §5.8

fn write_once(dir, name, bytes):
  tmp = dir/~kage-<16 hex>.tmp
  create_new(tmp); write; fsync; rename(tmp, dir/name); fsync(dir) where supported
  Windows sharing violation (OneDrive, antivirus): retry 50 ms … 5 s, then give up this tick
```

- **Own edits** publish 5 seconds after the last change, and at most
  30 seconds after the first.
- **Echo publishes** (only merged changes from others) wait for the next
  own publish, or 30 minutes, or shutdown. They add redundancy (R6), not
  freshness. Never on low battery.

### 8.4 Ingest

```text
fn ingest():
  for dev in list(devices/) except me:
    heads = strict head names, newest first; note provider duplicates
    for h in heads:
      if h.seq <= devices[dev].ingested_seq: break
      if residency(h) == Placeholder: request_download(h); waiting += 1; continue
      head = open(h) else { if age(h) < 10 min { waiting += 1 } else { problem(unreadable, dev) }; continue }
      if head.min_reader > READER_VERSION: devices[dev].state = needs_newer_kage; break
      complete = true
      for (ns, spec) in head.namespaces where ns is enabled here (unknown ns count as enabled):
        for name in spec.shards:
          if applied.has(name): continue
          f = dev/shards/name.kage
          if missing or placeholder: request_download(f); complete = false; continue
          plain = open_and_verify(f, expect_name = name) else { complete = false; continue }
          txn { for rec in plain.records: events += merge_record(rec); applied.add(name) }
      hlc.observe(head.max_stamp, now); warn if head.max_stamp is > 5 min ahead of now
      record acks (§8.7); devices[dev] = { name, os, version, head_seq, last_seen, gc_floor }
      if complete: devices[dev].ingested_seq = head.seq
      break                                          // only the newest readable head
  conflicts.record(events.conflicts)                 // §10, in the same transactions
```

- A head whose shards haven't arrived is merged as far as it goes (R5)
  and finished later.
- A shard that fails to open is retried while it is younger than 10
  minutes ("waiting"). After that it is skipped and reported as
  unreadable for that computer. Nothing is lost: the owner publishes a
  new head, and other computers echo the same data (R6).
- Reads run on the I/O pool with a 20-second timeout (§14.2). A
  dataless file is never read on the engine thread.

### 8.5 Merge and conflict detection

```text
fn merge_cell(local: Option<Cell>, inc: Cell) -> (Cell, Option<Conflict>):
  let Some(l) = local else return (inc, None)
  if l.stamp == inc.stamp:
    if l.value == inc.value: return (l, None)
    // Only possible after a clone (§8.9): break the tie by the value's canonical hash
    w, lo = by hash(value) descending; return (w, Some(concurrent(lo, w)))
  w, lo = higher stamp first
  if w.value == lo.value                   : return (w, None)
  if lo.stamp in w.anc                      : return (w, None)   // the winner saw it
  if lo.stamp.dev == w.stamp.dev            : return (w, None)   // one computer's own edits are ordered
  if field is a set element or map entry    : return (w, None)   // union semantics are the point
  return (w, Some(concurrent_edit(loser = lo, winner = w)))
```

Tombstones:
- `del` merges by the higher stamp.
- A tombstone newer than every cell hides the record. A cell newer than
  the tombstone shows it again.
- **Delete beats an edit it hadn't seen.** While the record is hidden,
  for each cell `c` older than `del` but newer than every stamp in
  `del.anc`, and from another computer than the deleter (so the deleter
  hadn't seen it): conflict `edit_vs_delete` ("Edited on Home Mac,
  deleted on Work PC"), one per field. Set elements and map entries don't
  count. Restore brings the record back. A concurrent edit this
  approximation misses is still in "Recently deleted" with the whole
  record for 30 days.
- **An edit beats a delete it hadn't seen.** When cells newer than `del`
  show the record again and none of them names `del.stamp` in its `anc`:
  one info conflict `delete_vs_edit` per record (no field; the winner is
  the newest such cell): "Deleted on Work PC, but changed later on Home
  Mac. [Delete anyway]". Restoring a deleted item puts `del.stamp` into
  the new cells' `anc`, so a deliberate restore isn't reported, even
  together with another computer's edit.
- A merge reports a cell-and-tombstone pair only when one of the two
  arrived in that merge; a pair it held already was reported when it
  first met. `delete_vs_edit` is reported only when the tombstone is new
  here or the record was hidden before the merge; a further edit of a
  record that is already back isn't reported again.

Properties (tested, §18.1):
- **No silent loss.** A value stops being current only because a cell
  with a higher stamp replaced it. If that cell's writer hadn't seen it,
  a conflict records it. A missed conflict would need the winner's `anc`
  to name the loser, which only happens when its writer had the loser as
  base.
- **For cells, false positives are possible**, never false negatives. They need
  four or more edits of one field, alternating between other computers,
  while one computer is offline, so the loser drops out of the 3-stamp
  ancestry. They add a dismissable entry; the winner is the same.
- **Every computer that merges the same cells picks the same winner.**
- **The losing computer always detects the conflict**, because it holds
  the loser until the winner arrives. Other computers may detect it too.
  They write identical conflict records (§10.1).

### 8.6 Project: cells → local files

```text
fn project(changed records):
  for each source with changed records:
    base = source.read_local()
    writes = []
    for record, field in changed:
      value = effective(field)                     // §7.5
      if hash(value) == projected.hash: continue
      if hash(base value) != projected.hash:       // the user changed it since we last looked
        capture(source) first; recompute; continue
      value = validate(value)                      // §8.11: shape, range, hotkey, struct; failure → pin invalid_here, keep local
      value = guards(value)                        // §8.10: consent, runnable review, mass change
      writes += (record, field, value)
    match source.project(writes):                  // one atomic local write per source
      Applied  => update projected (hash of the value read back after the write, and the cell it came from)
      Stale    => capture and retry, at most 3 times this tick
```

- **Config** goes through `ConfigPort` (§19.3), implemented by the app
  side:
  1. take a snapshot of `Config` and its revision;
  2. build the next `Config` (lists rebuilt from visible records in
     `(order, id)` order; per-computer fields taken from the local item
     with the same id);
  3. `consent::keep_local` so grants are never widened;
  4. round-trip through `Config` deserialisation. A synced path whose
     round trip differs (an enum variant this build doesn't know, a field
     it drops) gets a `lossy` pin. The replica keeps the newer value and
     it is never exported back;
  5. `commit(base_revision, next)`: one `Config::save`, the post-save
     work `save_config` does today (factored out as
     `commit_config_change`), then `sync_applied` and `config_updated`.
     A revision that moved since the snapshot returns `Stale`.
- **No folder I/O while the config lock is held.**
- `projected.hash` is the hash of the value read back *after* the
  write. A lossy round trip therefore never looks like a local edit, and
  an older build can't spread its lossy copy.
- **Steering docs**: atomic write to the default path only.
- **Extension keys and lists modes**: `atomic_write::write(…, Perms::Private)`
  under the per-key lock that WP-S5 adds to `save_extension_data` (it
  takes none today), then
  `extension_data_changed` with `source: "sync"` (the event from
  EXTENSION_VIEWS §16).
- **Extension collections mode**: append to `feed`, then
  `extension_sync_changes`. The extension pulls (§12.6). Sync never
  touches the extension's own files.
- **Installs**: never installed or removed (§4.5), except store themes.

### 8.7 Deletes, tombstones and cleanup

- **A delete** writes `del = (stamp, wall, anc)`. Cells are kept for 30
  days, so the record can be restored from "Recently deleted". After
  30 days the cells are dropped and only `del` stays.
- **Acks.** While ingesting, a tombstone gets an ack from device D when
  D's latest complete head holds `del` (or a newer cell) for that record,
  or D's `gc_floor` is at or above the tombstone's stamp.
- **Cleanup.** A tombstone is dropped when it is at least 90 days old
  and every *active* device that has the namespace enabled has acked it.
  Active means a head written in the last 60 days and not retired or
  removed. The device then raises its `gc_floor` to that stamp.
- **Coming back after more than 60 days.** A device knows when it last
  completed an ingest. On return:

  ```text
  floor = max(gc_floor of active heads)
  for rec in replica where visible and rec.published and max cell stamp <= floor:
    if no active head's state holds rec:
      // deleted while this computer was away; the tombstone is already gone
      move rec to conflicts as 'deleted_while_away' (restorable); drop it locally (through the mass-change guard)
  everything else merges normally; edits made here while away are new and get published
  ```

  A delete made elsewhere isn't undone by an old computer coming back.
  The item stays restorable from the conflicts list.
- **Removing an extension's data from the folder** (Settings → Sync →
  Extensions): write `sync.ns_state/<ns> = { purged: stamp }` for each of
  its namespaces. Every computer drops cells at or below that stamp from
  its replica and turns that extension's data sync off on itself
  (`ext_data_off`). Local data files are never touched. Turning it back
  on later publishes that computer's local data again.

### 8.8 Create and join

**Create** (no `Kage Sync` yet):
1. Write `kage-sync.json`, `README.txt` and the key file, each with
   exclusive create.
2. Every local syncable value becomes a cell stamped now.
3. Publish. Re-read the descriptor 10 seconds later (§5.3).

**Join** (the folder exists, and this computer has its own data):
1. Unlock the key: passphrase → key-encryption key → unwrap the data key
   (§11.3).
2. Take an encrypted backup of this computer with the existing export
   code (`config_export`, full mode, the sync passphrase) into
   `sync/trash/join-<time>.kagebackup`. **Undo joining** restores it for
   30 days (§13.4).
3. Ingest everything into an empty replica. Wait until every head's
   shards are readable, or 2 minutes; the status says what is still
   downloading.
4. Match list items by content and name (§4.4), so items set up by hand
   on both computers don't appear twice.
5. Show a preview, then apply the user's choice:
   - "Same on both: 41 settings, 6 shortcuts."
   - "Different: 9 settings."
   - "Only here: 3 shortcuts, Mind Map with 212 items. They will be
     added."

   | Choice | Differing values | Only here | Only synced |
   |---|---|---|---|
   | **Use the settings from my other computers** (default) | Synced values win. Each local value goes to the conflicts list as `replaced_on_join`. | Published | Applied |
   | **Use this computer's settings on all computers** | Local values become new cells; they win everywhere. The replaced synced values go to the conflicts list. | Published | Applied |

6. Per-OS values with no synced value for this OS keep the local value
   and publish it for this OS. A Mac joining a Windows-only set keeps its
   hotkeys.
7. Collections-mode extensions push their records with `initial: true`
   (§12.6). Those stamps come from each record's own `updatedAt`, so the
   newer copy wins, not the computer that joined later.

The conflicts list groups join entries: "Joined Work laptop: 9 settings
differed. [Review] [Keep all]".

### 8.9 Identity: clones, restores, rewinds

```text
fn check_identity_at_start():
  if device.json.fingerprint != fingerprint():     // SHA-256(OS machine id + config dir path)[..16 hex]
    // copied profile, migration tool, disk image, VM clone
    previous_device_ids.push(device_id); device_id = new_id(); republish everything under the new directory

fn check_own_dir():
  top = highest head seq in devices/me
  if top > meta.my_seq:
    if head(top).instance not in my recent instances: rotate the device id as above; problem(device_collision)
    else: // restored from a backup of this computer: fold my newer published state back into the replica, then continue
  if provider duplicates of my heads exist: same clone check
  if top < meta.my_seq or a shard my newest head names is missing:
    // the sync app rewound or deleted my directory (a restore, Syncthing "Receive Only", a user)
    mark everything dirty; republish; rewinds += 1
    if rewinds >= 3 within 1 hour: problem(folder_reverting); pause publishing for 1 hour
  delete my own provider duplicates and stale temps
```

- Equal stamps with different values can only come from a clone. The
  merge breaks the tie by value hash and records a conflict (§8.5). No
  loss, possibly a spurious conflict.
- **Leave** (Turn off sync on this computer): publish a final head with
  `retired: true`, delete the key from the key store, delete
  `replica.db`, keep all local data. Other computers delete a retired
  directory once they have fully ingested its last head.
- **Remove another computer**: write `sync.devices/<dev>.revoked = true`.
  When that computer next syncs, it stops and says "This computer was
  removed from sync on Home Mac". Its directory is deleted by the
  computer that removed it, once its last head is fully ingested and the
  head is 14 days old or the user confirms the computer is gone. "It's
  lost or stolen" also rotates the key (§11.3).
- **Start over**: §13.6.

### 8.10 Guards

**Review before it runs here.** Synced items that can start a program
without the user triggering them arrive held when they are new, or when
their runnable part changed:

| Item | Runnable part | Held means |
|---|---|---|
| Agent connection, when it is or becomes the active one | `mode` (local `spawn_command`) | The previous active connection stays. A new connection isn't made active. |
| `acp.active_connection_id` naming a connection whose command isn't approved here | the target's `mode` | The previous active connection stays |
| Internal site | `proxy` | The profile is off on this computer |
| Automation with a schedule or signal trigger | `trigger` (without `last_run`, which changes on every run and is "This computer") and `steps` | The automation is off on this computer |

- Held items appear under "Needs your attention → 1 synced item can run
  programs on this computer". Review shows the full command or script as
  escaped `dir="ltr"` text, with [Allow on this computer] and [Keep off].
- Approval stores `SHA-256(runnable part)` in
  `device.json.approved_runnables`. Turning the item on by hand in its own
  Settings page counts as approval.
- Held state is a `needs_approval` pin, so the hold never travels back to
  the folder as a change.
- [Keep off] leaves the item held and stops asking until its runnable
  part changes again. A held active pointer is listed under the
  connection it points to. It follows the synced value again once that
  value names an approved connection, one that runs nothing, or the one
  already active here.
- Shortcuts are not held. They run only when the user triggers them.

**Consent.** Synced `extension_states.<id> = true` switches an extension
on here only if this computer's grant covers everything its installed
manifest requests (`config_export::consent::keep_local`). Otherwise it
stays off with a `consent` pin, and Review offers the install-time prompt
(the Store's Approve path). A theme runs no code, so its switch is the
synced one.

**Mass change.** One tick would delete more than 50 items, or more than
20% of a namespace (at least 10):
- incoming: Kage holds that namespace and asks: "Work PC deleted 1,204
  Mind Map items. [Delete here too] [Keep them here]". **Keep**
  publishes them again as new edits. **Delete** first copies the data to
  `trash/`;
- outgoing: "This will delete 1,204 Mind Map items on all your
  computers. [Delete everywhere] [Only here] [Cancel]". **Only here**
  turns that extension's data sync off on this computer.
- outgoing in collections mode (§12.6): the extension has already
  deleted its own copy when it pushes the deletes (Mind Map's "Delete
  everything"). The push succeeds, but the host holds the tombstones: it
  doesn't publish that namespace and shows the question under "Needs your
  attention", with the Sync sidebar badge. **Delete everywhere**
  publishes them. **Only here** turns the extension's data sync off on
  this computer. **Cancel** is labelled "Bring them back here": the host
  drops the held tombstones and re-delivers the records through `pull`
  (`reason: 'reset'`), so the extension restores them.
- In every case, "copies the data to `trash/`" means the replica's last
  values for those records, as JSON, kept 30 days.

**Config reset.** When `Config::load` took its backup-and-reset path
(corrupt file, invalid JSON, failed migration, schema mismatch), the engine
does not capture `config.json`: the reset defaults would otherwise spread
to every computer. It projects the synced values instead and tells the
user: "Your settings on this computer couldn't be read, so Kage restored
them from sync." Two more cases count as a reset:
- the schema-mismatch salvage path, which resets only the top-level
  sections that failed to parse: `load_was_reset` reports those sections,
  and only they are projected instead of captured;
- a `config.json` that is missing while `device.json` says sync is on
  (`Config::load` then writes defaults, which must not spread either).

A `config.json` from a newer build (`Config::saves_suspended()`) pauses
the config namespaces entirely.

**Backup import.** A restore is a deliberate local change, so it reaches
the other computers. The import confirm says so when sync is on: "Sync is
on. The restored settings will also change on your other computers."
Every synced value the restore replaces goes to the conflicts list as
`restore_backup`, so the other computers' values can be brought back.

**Churn.** A cell that changes more than 20 times in an hour from two or
more computers is frozen for this session. It is logged by key, never by
value, and the Sync page shows "Kage stopped syncing 1 item that kept
changing". This protects against two builds fighting over a value.

### 8.11 Newer and older Kage in one folder

- **Layout version.** Heads and `kage-sync.json` carry `min_reader`. A
  reader below it skips those heads and shows "Update Kage to read
  changes from Work PC". It still publishes its own state. Writers raise
  `min_reader` only for a truly incompatible layout change.
- **Additive changes never raise anything.** New namespaces, new fields,
  new list-item fields: older readers merge and republish them (opaque),
  but don't project them.
- **Unknown fields survive older editors.** Capture only diffs fields
  this build knows. Cells for unknown fields stay in the replica.
- **Unknown enum values** become `lossy` pins (§8.6) and are never
  exported back over the newer value.
- **Validation before projecting.** Values `config_export::allowlist`
  shapes (bools, numbers, hotkeys, themes, languages, enums) go through
  its `check(shape, value)` and `Range::holds`. Hotkeys must pass
  `allowlist::hotkey_registers` for this OS. On macOS, a synced hotkey
  that `system_shortcut_takeover::conflicts_for` says an enabled system
  shortcut owns (⌘Space and Spotlight) is applied but would never fire.
  Sync never takes the system shortcut over: that needs the user's
  approval in the hotkey picker on each Mac, and the takeover state
  (`system-shortcuts.json`) never syncs. "Needs your attention" says
  "⌘Space is used by Spotlight on this Mac. [Choose in Settings →
  Hotkey]". List items must deserialise
  into their struct. A failing value is not applied: the local value
  stays, with an `invalid_here` pin. The Sync page says "1 setting from
  your other computers doesn't work on this version".
- **Namespace schema.** Each shard carries `schema`. A reader that sees a
  higher schema for a namespace it knows keeps it read-only: it merges and
  republishes, but projects nothing and captures no edits for it. A
  newer reader migrates lower-schema values while ingesting, with a pure
  `migrate_cell(ns, from, value)`.
- **Renamed config paths.** A config migration that renames a path adds
  `(old, new)` to `catalog::RENAMED`. While any active head reports an
  older `catalog`, the newer Kage writes both paths.
- **Extension schemas.** The manifest's `sync.schema` is stored on each
  record. An extension version that declares a lower schema doesn't get
  higher-schema records through `pull`, can't overwrite them, and its
  settings header says "Update Mind Map on this computer to see 12 newer
  items" (§12.6).
- **`config_migrations::CURRENT_VERSION`** never appears in sync. Records
  are keyed by path in the current schema plus `RENAMED`.

### 8.12 The hard cases, and what handles them

| Case | What happens |
|---|---|
| **Clock skew between computers** | Order comes from the HLC (§7.3). An edit made after seeing a change is always newer than it, whatever the clocks say (`floor`). Remote clocks can pull this computer's HLC at most 1 hour past its wall clock. When a remote stamp is more than 5 minutes ahead, the status shows "Home Mac's clock is about 40 minutes ahead" (at most once a day per computer). Concurrent edits favour the computer whose clock is ahead; the loser is in the conflicts list. |
| **Partial or in-progress cloud writes** | Files are written once and AEAD-sealed. A partial file fails to open and counts as "waiting" for 10 minutes, then "unreadable" for that computer. The head is written after its shards; a head whose shards haven't arrived is merged as far as it goes. |
| **iCloud evicted placeholders** | `os::cloud_files::residency()`: a `.name.icloud` stub, or `st_flags & SF_DATALESS`, means Placeholder. `request_download()` asks the system to download (`NSFileManager startDownloadingUbiquitousItemAtURL:error:`) and returns at once. For other File Provider folders (Dropbox, OneDrive, Google Drive on macOS), where that call may do nothing, the I/O pool's timed read (§14.2) downloads the file instead. The engine thread sets `IOPOL_MATERIALIZE_DATALESS_FILES_OFF` for itself (`setiopolicy_np`), so an accidental read of a dataless file there fails at once instead of blocking. The next tick picks the file up. Kage never needs its own files back (it has the replica), so evicting them costs nothing. After 3 ticks of placeholders the status says "Your sync folder hasn't downloaded some files yet" with the tip "In Finder, choose Keep Downloaded for Kage Sync". |
| **OneDrive Files On-Demand** (and Dropbox or iCloud on Windows: all Cloud Files) | `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`, `RECALL_ON_OPEN` or `OFFLINE` means Placeholder; only attributes are read. At setup and each start, Kage pins `Kage Sync` (`CfSetPinState(…, CF_PIN_STATE_PINNED, CF_SET_PIN_FLAG_RECURSE)`, what "Always keep on this device" does). If pinning fails, it hydrates in the background with a timed read and retries next tick. |
| **Dropbox conflicted copies** | Can't happen for Kage's files (R1, R2) unless two computers share a device id. The classifier finds them (§5.2). In our own directory they trigger the clone check and are deleted; elsewhere they are ignored. Content names make exact duplicates harmless. |
| **Two computers writing at once** | They never write the same file. Each publishes its own cells. Every computer picks the same winner per cell; the losing value goes to the conflicts list. Root files are written once with exclusive create (§5.3). |
| **A computer offline for weeks** | Its edits keep their stamps and merge normally. Concurrent losers go to the conflicts list. Tombstones wait for it for 60 days; after that the coming-back rule (§8.7) stops it from bringing back items deleted elsewhere. |
| **Deleted items** | Explicit tombstones, 30 days of "Recently deleted", cleanup after 90 days and acks from every active computer (§8.7). |
| **Schema versions; a newer Kage writes, an older one reads** | §8.11. |
| **Folder moved, renamed or unmounted** | Sync pauses and never re-creates the folder (that would split the computers). "Find folder" looks in the detected cloud roots, 3 levels deep, for a `kage-sync.json` with our `folder_id`. |
| **macOS privacy prompts** | Folders under Desktop, Documents, Downloads, iCloud Drive and `~/Library/CloudStorage` (Dropbox, OneDrive and Google Drive on File Provider) are protected. The first background read shows "Kage would like to access files in …". The answer is tied to the app's code signature, so on ad-hoc-signed builds macOS may ask again, or deny silently, after an update (the same cause as §11.4 and the hotkey notes in CLAUDE.md). Kage touches the folder only on the engine thread after startup, so a prompt never blocks launch. `EPERM` means `folder_denied` (§13.4): **Allow access** opens the folder picker on that folder (a pick grants access for this run), and the status links to System Settings → Privacy & Security → Files and Folders. Setup suggests a folder outside Desktop and Documents when there is a choice. |
| **Folder restored to an old version** | Older heads of other devices are ignored (`seq` must grow). Our own directory is republished (§8.9). |
| **Cloned or restored computer** | Fingerprint, `instance` and own-directory checks (§8.9). |

---

## 9. Per-OS and per-computer overrides

### 9.1 Model

- **Shared**: cell `f@all`.
- **Per OS**: cell `f@macos`, `f@windows` or `f@linux`. `null` means "no
  override for this OS".
- **This computer**: a `this_computer` pin in `replica.db`. It is never
  published.
- Each catalog row has a default scope: Shared or Per OS (§4.3, §4.4).
- There is no separate "mode" record. Where the value lives *is* the
  mode, so two computers can't disagree about it.
- `config.json` holds only the effective value (§7.5). Nothing that
  reads `Config` changes.

### 9.2 Where an edit goes

"Edit where it lives": a change from any Settings page, window or
command writes to the scope the effective value comes from.

```text
edit_target(f) on OS o =
  if pinned this_computer:        the pin (local only)
  elif cell f@o is non-null:      f@o              // editing the Mac override
  elif cell f@all is non-null:    f@all            // changes all computers
  elif rule(f) == PerOs:          f@o
  else:                           f@all
```

Users who never touch overrides just see "change it here, it changes
everywhere". A hotkey changed on a Mac changes the Macs.

### 9.3 Scope actions

One command, `sync_set_scope(path, action)`. With `x` the current
effective value on this computer:

| Action | Menu text (on a Mac) | Effect |
|---|---|---|
| `all` | Same on all computers | `f@all = x`; `f@<os> = null` for every OS that has a value; remove the pin |
| `os` | Only on Macs | `f@macos = x`; remove the pin |
| `use_shared` | Use the value for all computers (Dark) | `f@macos = null`; remove the pin |
| `this_computer` | Only on this computer | Pin `x`. Changes from other computers stop affecting this field here. |
| `stop_this_computer` | Stop keeping a value only here | Remove the pin; project the effective value |

For list items the path is the item (`shortcuts/<sync_id>`):

| Action | Menu text | Effect |
|---|---|---|
| `avail_all` | Available on all computers | `avail = "all"` |
| `avail_os` | Only on Macs | `avail = "macos"` |
| `avail_this_computer` | Only on this computer | `*` pin. If the item was published, confirm first: "Remove “gh” from your other computers and keep it only here?", then tombstone it. |

Per-OS fields inside an item use the field actions with the path
`shortcuts/<sync_id>/<field>`.

### 9.4 How Settings shows overrides

When sync is on, every Settings row that maps to a catalog path gets a
**where-it-applies control** at the end of its label line. When sync is
off, nothing changes on any page.

- `SettingsModule.createCheckboxRow` and `createControlRow` (`base.js`)
  take an optional `{ syncPath }`. Custom rows set `data-sync-path`.
- `ui/js/shared/sync-scope-chip.js` exports `attachScopeChips(root)`.
  It runs after each module renders, and again on `config_updated` and
  `sync_status_changed`. It makes one `sync_get_scopes` call per render.
- What the control shows:

| State | Control |
|---|---|
| Shared, no overrides | A quiet icon. "All computers" shows on hover and keyboard focus. |
| This OS has its own value | "Macs only" (or "Windows PCs only", "Linux PCs only"), always visible. Under the control: "All computers: 14 px". |
| Pinned | "This computer" |
| Another OS has its own value | A faint "Differs on Windows"; the value shows on hover |
| Per-OS by default (hotkeys, paths) | "Each kind of computer" |
| Never synced | A static grey "This computer" with the reason in a tooltip: "Not synced. Start at login is set for each computer." |
| Held | "Not synced here", with the reason: looks like it contains a key; doesn't work on this version; needs approval |

- Clicking the control (or the row's ⋯ menu, `shared/popup-menu.js`)
  opens the menu. On a Mac:

```text
 ✓ Macs only                              16 px
   All computers                          14 px    (remove the Mac override)
   Use 16 px on all computers
   Only this computer
 ─────────────
   Windows PCs use 14 px (All computers)
```

  For a hotkey:

```text
 ✓ Each kind of computer has its own      Mac: ⌥Space · Windows: Alt+Space
   Same on all computers                  (hotkeys often differ between Mac and Windows)
   Only this computer
```

- List editors (Shortcuts, Automations, App modes, Agents, Internal
  sites, Custom actions):
  - a row shows a small chip only when the item isn't shared ("Macs
    only", "This computer");
  - the item editor has one line:
    `Available on: (•) All computers  ( ) Macs only  ( ) Only this computer`;
  - per-OS fields carry an OS badge and the other OS's value as a hint
    (§4.4).
- Extension settings pages are drawn by the host from the manifest
  (`settings-renderer.js`), so the host adds the same control per
  declared field (`extensions.<id>.<field>`). Sensitive fields show
  "This computer · sign-ins don't sync".
- The first three times the user changes a shared setting after setting
  up sync, a toast says "Changed on all your computers." with [Undo] and
  [Only on this computer].
- When a remote change arrives for a section with unsaved edits
  (`sync_applied`), the section shows "Some settings here changed on
  Work PC. [Show the new values]". The user's edits stay until they
  choose.
- Controls have `aria-label` text ("Font size: same on all computers.
  Change") and use logical CSS properties for RTL.

### 9.5 Safe saves

Today a dirty Settings section writes every field it owns from its DOM,
and other windows send a whole `Config` to `save_config`. A field changed
remotely while a window was open would be silently reverted, and sync
would spread that revert as the newest change. Two fixes, both needed:

1. **Field-level save in the Settings manager** (frontend). When a
   module runs `load(config)`, the manager keeps a deep copy. On Save:

   ```js
   const before  = structuredClone(loadSnapshot[module.id]);
   const edited  = structuredClone(before);
   module.save(edited);                       // what the user's DOM says
   const changed = diffPaths(before, edited); // only what the user changed
   const { revision, config: fresh } = await getConfigSnapshot();
   applyPaths(fresh, edited, changed);
   await saveConfig(fresh, { baseRevision: revision });
   ```

   List editors diff by `sync_id`, so editing one shortcut doesn't
   rewrite the others. This also fixes the existing cross-window
   lost-update case, sync or not. After every save the manager takes a
   new snapshot (`save_config` returns the new `revision`; the module
   reloads from `get_config_snapshot`), so an item the user just created
   carries the `sync_id` the backend gave it, and a second save doesn't
   add it again.
2. **Three-way merge in `save_config`** (backend). `get_config_snapshot`
   returns `{ revision, config }`. `FeatureServices` keeps the last 64
   configs by revision. `save_config(config, base_revision)`: for each
   catalog path (and each list item field, by id) where the incoming
   value equals the value at `base_revision`, the caller didn't change
   it, so the current value stays. A missing `base_revision` keeps
   today's behaviour and logs a warning. All in-tree callers move to
   `saveConfig()` in `ui/js/shared/config-cache.js`.
3. **The same rule for `save_extension_config`.** It writes one
   extension's whole settings object, and an extension's own sandbox can
   call it (`storage`). The host records, per window, the copy that
   window last got from `get_extension_config` (or the last
   `config_updated`). On save, a field equal to that copy wasn't changed
   by the caller, so the current value stays. Without this, a stale
   sandbox would silently revert a setting changed on another computer,
   and sync would spread the revert.

---

## 10. Conflicts list

### 10.1 Storage

Conflicts are synced records in `sync.conflicts`, so every computer shows
the same list, and a restore or dismiss on one applies everywhere.

- Id: `c` + the first 16 hex of `HMAC(name key, ns | id | field | loser stamp)`.
- Two cells:
  - `body@all` (never changes):

    ```json
    { "ns": "list.shortcuts", "rid": "s7hq2m0c5d1z9w3pa", "field": "prompt@all",
      "kind": "concurrent_edit",
      "loser":  { "stamp": "000193080b0e0001a9x3h6t1qq8m0ve2", "wall": 1762640000000,
                  "deleted": false, "value": "Summarize: {text}" },
      "winner": { "stamp": "0001930b6f1c0004k7m2p9xqa3rt5vwe", "wall": 1762686000000,
                  "deleted": false } }
    ```

    `field` is `null` for a record-level entry (`delete_vs_edit`,
    `deleted_while_away`). A side's `value` is left out when it has none
    (a delete, or a winner, whose value the record holds); `winner` is
    `null` for entries made by an action.

  - `state@all`: `"open"`, `"dismissed"` or `"restored"`.
- **Deterministic stamps.** A conflict found by merging is written with
  the *loser's* stamp and wall time. The id names the loser, so that is
  the one thing every computer that finds the entry agrees on. Computers
  that find the same pair write identical cells, so there are no
  duplicates and no conflict about a conflict. A computer that was
  offline may find the same loser against a newer winner (someone edited
  the field again): it writes another body under the same id and stamp,
  and the merge's tie-break keeps one body everywhere. A later dismiss or
  restore gets a fresh stamp, which is always newer than the loser's, so
  a slow computer that finds the conflict later, even against a winner
  written after the dismiss, can't reopen it. (Stamping with the winner's
  stamp would let that newer winner reopen it.)
- Conflicts made by one computer's action (join, restore, backup import,
  deleted while away) are stamped normally.
- **No label and no detection time are stored.** They would differ
  between computers and versions. Labels are built at display time:
  - settings: the catalog's label key, reusing the existing Settings
    keys;
  - list items: the item's name from its cells (kept 30 days after a
    delete), else from the loser value;
  - extension records: the manifest's `label` field; Mind Map's private
    items show "Private item".
- Loser values are inline up to 64 KiB, and a blob above that, up to the
  record limit (4 MiB for large collections). Nothing is "too large to
  keep".
- `sync.*` namespaces never produce conflicts.

### 10.2 Kinds

| Kind | Shown as |
|---|---|
| `concurrent_edit` | "Theme · All computers. Now: Dark (Work PC, 10:42). Replaced: Light (Home Mac, 10:40)." |
| `edit_vs_delete` | "Shortcut “jira” was deleted on Work PC. It had been changed on Home Mac." |
| `delete_vs_edit` | "You deleted “jira” on Work PC. It was changed later on Home Mac, so it's back. [Delete anyway]" |
| `replaced_on_join` | Grouped: "Joined Work laptop: 9 settings differed. [Review] [Keep all]" |
| `restore_backup` | Grouped: "Restored a backup on Home Mac: 14 settings replaced." |
| `replaced_by_restore` | "Restored an older value of Theme. Replaced: Dark." |
| `deleted_while_away` | Grouped: "Old laptop hadn't synced since May. 12 items deleted elsewhere were removed from it." |

### 10.3 Retention

- Open entries: 90 days, counted from the winner's wall time. Then the
  cleanup pass dismisses them. Several computers doing so write equal
  values.
- Dismissed and restored entries stay under "Resolved" for 30 days, then
  are tombstoned (§8.7).
- At most 500 entries or 64 MiB. Past that, the oldest resolved entries
  go first, then the oldest open ones. Each drop is logged in English, by
  id only.
- **Recently deleted** (a second tab) lists tombstoned records whose
  cells are still kept (30 days). These are not conflict records.
- `sync/trash/` keeps join backups and bulk-delete copies for 30 days.

### 10.4 Actions

| Action | Effect |
|---|---|
| **Restore this version** | Write the loser's value as a new cell: fresh stamp, `anc = [current winner]` (and the tombstone, if the record was deleted; a deleted record comes back with all its cells). It becomes the newest everywhere and is not a conflict. A `replaced_by_restore` entry keeps the value it replaced, so a restore can be undone. That entry starts out `dismissed`: the user just chose, so it isn't an open conflict, and it waits under Resolved for 30 days. `state = restored`. A loser that was a delete writes a tombstone. A loser without a field (`deleted_while_away`) holds the whole record in its shard form (§5.6), and every cell of it is restored. |
| **Keep current** (Dismiss) | `state = dismissed` |
| **Keep all** / **Dismiss all** | Dismiss each entry in the group |
| **Delete anyway** (`delete_vs_edit`) | A tombstone whose `anc` covers the current cells; `state = dismissed` |
| **Compare** | Side by side, field by field, for list items and extension records. Values are escaped text. |
| **Restore** (Recently deleted) | Re-stamp every kept cell, with the tombstone's stamp in `anc` |

Restoring an extension record reaches the extension through `pull` on
every computer, this one included.

### 10.5 The screen

```text
Conflicts                                                [Dismiss all]
When two computers changed the same thing, Kage kept the newest change.
The other one is saved here for 90 days.

Today
  Theme · All computers
    Now: Dark (Work PC, 10:42)
    Replaced: Light (Home Mac, 10:40)                [Restore] [Dismiss]

  Shortcut “gh”
    Now: changed on Home Mac, 09:15
    Replaced: changed on Work PC, 09:14     [Compare] [Restore] [Dismiss]

  Mind Map · “Call Dana about the launch”
    Now: changed on Work PC
    Replaced: changed on Home Mac, 11:20             [Restore] [Dismiss]

Earlier
  Joined Work laptop · Nov 2
    9 settings differed.                       [Review] [Undo joining]

Generated by Kage                                  (collapsed by default)
  What Kage learned about you · 2 versions                       [Show]

[ Replaced ]  [ Recently deleted ]  [ Resolved ]
```

---

## 11. Secrets and encryption

### 11.1 Decision

1. **Everything Kage writes to the sync folder is encrypted and
   authenticated.** There is no unencrypted sync.
2. **Credentials never sync, not even encrypted.** Neither do tool
   permissions, capability grants or consent.
3. A **sync passphrase** wraps a random data key. Each computer caches
   the data key (never the passphrase) in the OS keychain where the
   keychain is reliable, and in an owner-only file otherwise (§11.4).

### 11.2 Why

Why encrypt everything, not just secrets:
- **Integrity matters as much as secrecy.** Synced shortcuts,
  automations, agent commands and internal-site scripts are code in
  effect. Without authentication, anyone who can write the folder (a
  shared link, a leaked cloud account) could plant one that runs on every
  computer. AES-GCM with the folder, device, kind and name in the
  authenticated data makes planted, edited or moved files fail.
- **What syncs is private**: notes, prompts, steering docs (what Kage
  learned about you), scripts, document text, chats. The cloud provider
  shouldn't read them.
- **It matches the backup rule**: "There is no unencrypted full backup."
  The sync folder is a continuously updated full backup in someone else's
  cloud.
- **It adds robustness for free**: the tag catches truncated and
  in-progress files.

Why credentials never sync, even encrypted:
- **Rotating refresh tokens break.** Two computers sharing one refresh
  token race to refresh it, and providers with reuse detection revoke the
  whole grant. `docs/EXTENSIONS.md` already describes this between
  windows of one computer; across computers there is no shared lock.
- **Smaller blast radius.** A leaked folder plus a guessed passphrase
  exposes notes and settings, not every account.
- **One rule, already tested.** Sync calls the same
  `config_export::sensitive` functions as backups, so a value is a
  credential for sync exactly when it is one for backups.
- **The cost is small and recoverable.** Signing in once per computer is
  easy to explain. A leaked token is not recoverable.

Why grants and consent never sync: a grant made on one computer must not
widen what an extension or the agent may do on another. The agent, its
MCP servers and the risk differ per computer (a work laptop and a
personal one). This is the backup rule (`consent::keep_local`).

### 11.3 Keys

```text
passphrase ──Argon2id(m = 64 MiB, t = 3, p = 1, 16-byte salt)──▶ KEK (32 bytes)
DEK (32 random bytes, one per key epoch) ──AES-256-GCM(KEK)──▶ keys/eNNNN-<dev>-NNNN.json
DEK ──HKDF-SHA256──▶ enc     file encryption
                 ──▶ name    content names, buckets, conflict ids
                 ──▶ extdev  per-extension device tags (§12.6)
```

Key file (plain JSON, written once by one computer):

```json
{
  "epoch": 1,
  "kdf": { "alg": "argon2id", "m_kib": 65536, "t": 3, "p": 1, "salt": "<b64, 16 bytes>" },
  "wrapped": "<b64: nonce(12) | AES-256-GCM(KEK, DEK) | tag(16)>",
  "aad": "kage-sync-dek|<folder_id>|1",
  "written_at": "2026-11-09T10:00:00Z"
}
```

- Argon2 is stronger than the backup's interactive parameters because
  it runs once per computer (setup, join, unlock, passphrase change),
  never per sync. The parameters are stored, so they can change later.
- **Passphrase**: at least 12 characters, or "Make one for me": 25
  Crockford base32 characters (125 bits) in five groups, shown
  `dir="ltr"` in a monospace font, with [Copy] and [Save as file…].
  The passphrase is never stored or logged. A wrong one is "That
  passphrase doesn't unlock this folder." The Argon2 cost (about half a
  second) is the rate limit.
- **Change passphrase**: re-wrap the same DEK in a new key file from this
  computer, then delete this computer's old key files. Other computers
  notice nothing; they keep the DEK. This works from any computer that is
  still syncing, which is also how a forgotten passphrase is recovered.
- Two computers changing the passphrase at the same time leave two key
  files in one epoch. Either passphrase works, and Settings says so.
- **Lost or stolen computer**: "Remove… → It's lost or stolen" makes
  epoch + 1 with a new DEK and a **new passphrase**, and publishes
  everything again under it. Every other computer shows "The encryption
  key changed on Home Mac. Enter the new passphrase." once, then
  republishes. Readers accept the old epoch until every active computer
  has moved. The lost computer can still read what was written before.
- **Every computer loses its key**: the data on each computer is
  untouched. "Start over" makes a new folder from one computer.
- The DEK lives in memory in `Zeroizing` buffers.
- Fixed labels (v1, golden vectors in `sync::crypto`'s tests): HKDF-SHA256
  of the DEK with no salt and the infos `kage-sync/v1/enc`,
  `kage-sync/v1/name` and `kage-sync/v1/extdev`. A bucket is the first two
  bytes of `HMAC-SHA256(name, ns "|" id)`, big-endian, modulo the count. A
  device tag is `HKDF-Expand(extdev, "kage-sync/v1/ext-device|" ext "|"
  device)`, 12 bytes as 24 lowercase hex characters.
- Argon2 gets the passphrase in NFC, so a passphrase typed on macOS and
  on Windows gives the same key. A passphrase that reads as a generated
  one (25 Crockford characters, any case, with or without the dashes and
  spaces, `I`/`L` for `1`, `O` for `0`) is used in its canonical
  `XXXXX-XXXXX-XXXXX-XXXXX-XXXXX` form.
- Unwrapping refuses a key file whose AAD doesn't name this folder and
  its epoch (`Malformed`), and one asking for more than 256 MiB, t = 8 or
  p = 4 (`UnsupportedKdf`): a key file is plain JSON anyone with write
  access to the folder could plant.

### 11.4 Key store

Service `Kage Sync`, account `<folder_id>`. The value is the DEK and its
epoch. Only the engine thread reads it, after startup, never on the
launch path.

| Backend | Used when |
|---|---|
| macOS Keychain | The build has a stable Developer ID signature (a team identifier in its signing information) |
| Windows Credential Manager | Always |
| Linux Secret Service | It answers within 2 seconds |
| Owner-only file `<data_local>/kage/sync/key.bin` | Otherwise: ad-hoc-signed macOS builds, Linux without a Secret Service, or a keychain the user denied |
| Memory | Whenever `kage_core::paths::is_isolated()` (tests and the mock-app sweep never touch a real keychain) |

- Why not always the macOS keychain: a keychain item's access list is
  tied to the app's code signature. Kage's ad-hoc-signed builds change
  signature on every update (the same reason TCC grants don't survive an
  update), so macOS would ask "Kage wants to use your confidential
  information" after each update.
- The file backend is no weaker than `config.json`, which already holds
  extension API keys owner-only. The encryption protects against the
  cloud and the folder, not against malware running as the user.
- The key must never roam, for the same reason `sync/` sits in the
  non-roaming folder (§6.1):
  - macOS: a generic password in the login keychain with
    `kSecAttrSynchronizable` false, so iCloud Keychain never copies it;
  - Windows: a generic credential persisted with
    `CRED_PERSIST_LOCAL_MACHINE`, never `CRED_PERSIST_ENTERPRISE` (which
    roams with a roaming profile, and is the default of some key store
    crates).
- The stored value is 37 bytes: version `1`, the epoch as u32 LE, the
  DEK. `key.bin` is `{ "v": 1, "keys": { "<folder_id>": "<base64>" } }`.
- Linux never shows a Secret Service prompt: a locked collection fails
  as unavailable, which is the "Keychain entry gone or denied" row of §15.
- `default_key_store` is the selection above, for a new setup.
  `key_store_for(kind)` reopens the store `DevicePrefs::key_store`
  recorded, so a key kept in the file is still found after an update makes
  the keychain reliable. `file_key_store` is the store §15 offers.
- Settings → Sync → Advanced shows where the key is kept.

### 11.5 What counts as a credential

Never published, in any form:
- extension settings where `Rules::is_sensitive_setting(id, field)` or
  `Rules::is_sensitive_data(id, field)`, or whose name `looks_like_secret`,
  and host-managed scratch (`__`) fields;
- extension data keys where `Rules::is_sensitive_data(id, key)`, whatever
  the manifest's `sync` block lists. Extensions follow the same rule for
  their own content: Mind Map never pushes a document (item or text)
  that EXTENSION_FILE_TEXT flagged `possible_secret` (§12.9);
- collection record fields whose name matches the credential name
  pattern (`extensions::is_sensitive_name`): the push is refused with
  `sensitive_field`. Content isn't scanned: notes may contain anything,
  and they are encrypted;
- the host fields marked "held back" in §4.3 and §4.4 that look like
  they hold a credential (the two tests of §4.3);
- `tool_permissions`, `extension_grants`, `extension_agent_access`,
  OAuth state, the agent's `mcp.json` and sign-in, telemetry consent.

### 11.6 Threat model

| Who | Can | Can't |
|---|---|---|
| The cloud provider, or anyone who can read the folder | See file sizes, counts, times, the folder id and device ids | Read any setting, item, note, computer name or OS |
| Anyone who can write the folder, without the passphrase | Delete files, or roll them back to older versions: Kage reports missing or unreadable files and never deletes local data because of it (R9) | Plant or change data (AEAD fails), move one computer's files into another's directory (AAD), replay an older head (`seq` must grow) |
| Someone with the passphrase and the folder | Read everything that syncs, and write as any computer | Get credentials (never synced), widen grants or tool trust (never synced), or start programs by themselves (§8.10 review) |
| One of the user's computers is compromised | Everything that computer can do | Widen another computer's permissions |
| Malware running as the user | Read the key store and local files | (out of scope, as for `config.json` today) |

### 11.7 Path safety

- The engine writes only under `devices/<me>/`, its own key files and
  the root files (§5.1, R1).
- Names come from strict patterns, never from file contents.
- Sync never follows a symlink or reparse point inside the folder,
  as backup import doesn't.
- Projection writes only `config.json`, `extension-data/<id>/` (validated
  with `resolve_extension_data_path`), the default steering path, and
  `<data_local>/kage/sync/`. **A synced value never names a file Kage
  writes**: `user_steering_path`, `mcp_config_path` and the Pocket TTS
  Python path are "This computer".

---

## 12. Extension API

### 12.1 What syncs for every extension without opting in

These ride on the Extensions category:
- **Installed list** and **on/off**: §4.5 and §8.10.
- **Settings.** A field `extensions.<id>.<field>` syncs when all of these
  hold:
  1. it is not a credential (§11.5) and not a scratch `__` field;
  2. either `Rules::plain_setting(id, field, value) == Plain::Include`
     (a boolean, number, declared `enum` choice, or a `backup: "plain"`
     string: the same fields a settings-only backup may copy), or the
     manifest marks it `"sync": "shared"` or `"sync": "os"`;
  3. the manifest doesn't mark it `"sync": "machine"` or `"sync": "never"`.

  A string that `looks_like_secret` is held back. Why not every
  non-credential field: a free-form string might be an endpoint the
  extension sends its API key to. Syncing it would let one computer
  redirect another computer's credential. The author opts such strings
  in explicitly.

  `config.<field>.sync` takes `"shared"`, `"os"`, `"machine"` or
  `"never"`. It sets the default scope (§9). Credentials never sync,
  whatever it says. A receiving computer applies a setting only when its
  own installed manifest still allows it and its copy has the same origin
  (§12.3, condition 5). An extension that isn't installed there gets a
  `not_installed` pin until it is.

### 12.2 Opting data in: the manifest `sync` block

```json
{
  "permissions": ["storage", "sync"],
  "config": {
    "my_names":            { "type": "string",  "default": "", "sync": "shared" },
    "background_organize": { "type": "boolean", "default": false, "sync": "machine" }
  },
  "sync": {
    "description": "__MSG_sync.description__",
    "schema": 1,
    "keys": ["prefs", "view-*"],
    "lists": {
      "todos": { "array": "items", "id": "id", "label": "title" }
    },
    "collections": {
      "items": {
        "label": "title",
        "private": "private",
        "maxBytes": 65536,
        "together": [["status", "doneAt", "suggested", "confidence"], ["title", "titleSource"],
                     ["summary", "summarySource"], ["url", "canonicalUrl"]],
        "sets": ["entityIds", "terms"],
        "maps": ["mentionOrigin"],
        "local": ["rev", "enrich", "v", "fileRefs"]
      },
      "bodies": { "whole": true, "large": true, "maxBytes": 4194304 }
    }
  }
}
```

Validation (`src/extensions.rs::normalize_sync`, following the
`normalize_sensitive_data` contract: a malformed part is ignored with a
warning in the log and console, and the rest still loads):

| Field | Rule |
|---|---|
| `sync.description` | Optional. At most 140 characters after `__MSG_…__` resolution. Shown in the install prompt and in Settings → Sync. |
| `sync.schema` | Integer ≥ 1. Default 1. |
| `sync.keys` | At most 32 entries. Storage-key rules, with an optional trailing `*`. An entry that `sensitiveData` or the safety net matches is dropped with a warning: it never syncs. |
| `sync.lists` | At most 16 keys (same key rules). `array`: the top-level field holding the array, or `""` when the file is itself an array. `id`: the element field holding a unique id. `label`: optional element field for the conflicts list. |
| `sync.collections` | At most 16. Names match `^[a-z][a-z0-9_]{0,31}$`. All field names are top-level. `whole` (default `false`): the record is one cell. `together`, `sets`, `maps` and `local` must not overlap. `maxBytes` defaults to 65,536; maximum 1,048,576, or 4,194,304 with `large: true`. `label` and `private` name fields. |
| A `sync` block without `"sync"` in `permissions` | Only settings sync. The block is ignored with a warning. |

Changing a collection's structure (`whole`, `together`, `sets`, `maps`)
needs `schema` + 1, because older copies would split records
differently (§8.11).

### 12.3 The `sync` capability and disclosure

- New capability in `extension-permissions.js` → `CAPABILITIES.sync`:
  icon 🔄, label "Sync between your computers", description "Copies this
  extension's data, encrypted, to your sync folder so your other
  computers get it. Only when Kage sync is on." The catalog keys follow
  the existing pattern: `settings.manager.cap.sync.label` and
  `settings.manager.cap.sync.desc`.
- The commands `ext_sync_status`, `ext_sync_push`, `ext_sync_pull` and
  `ext_sync_ack` map to `sync` and are in `IDENTITY_SCOPED_COMMANDS`, so
  the host force-injects `extensionId`. Every other `sync_*` command maps
  to `null` (never callable from an extension).
- The install prompt lists the capability with `sync.description` and,
  in plain words, what syncs ("Syncs: items, entities, links").
- An update that adds `sync` needs approval again, through the existing
  "requests capabilities that weren't previously approved" path. A silent
  upgrade can't start sending data.
- An extension's data syncs only when all of these hold:
  1. Kage sync is on and unlocked on this computer;
  2. the Extensions category is on here;
  3. this computer granted `sync` (and `storage`);
  4. this extension's data switch is on here (on by default once
     granted, except for sideloaded extensions, see 5);
  5. this computer's copy has the same **origin** as the copy that
     published the data. Extension ids are only unique within a store, so
     without this a different extension that reuses the id (a sideload, a
     second store, or a reinstall from somewhere else after an uninstall)
     would receive the first one's notes. The origin is the `installs`
     record's `{ source, store }` (§4.5). The first computer that
     publishes the extension's settings or data writes it as an `origin`
     cell of the record `sync.ns_state/ext.<id>`. A computer whose installed origin differs
     gets `origin_mismatch` pins for that extension's settings and data,
     pushes return `{ off: true }`, and Settings → Sync says "A different
     extension with this id is installed here, so its data doesn't sync."
     Sideloaded on both computers counts as the same origin only after
     the user turns the data switch on here, which is off by default for
     sideloaded extensions and says "Only turn this on if it's the same
     extension on all your computers".
- Settings → Sync → Extensions lists each extension with what it syncs,
  its size, and the data switch. The extension can't change the switch.
- The extension's own Settings header gets a small chip: "Syncs settings
  and data", "Syncs settings" or "Sign-in stays on this computer". It
  sits right after the capability badges, in the header order shared by
  the four designs (§23), in its own module
  `ui/js/settings/sync-ext-chip.js`. Keys: `settings.manager.ext.sync.*`.
- Host features (`HOST_FEATURE_CAPABILITY` in
  `extension-sandbox-host.js`, each requiring `sync`): `sync.keys`,
  `sync.lists`, `sync.records`.

### 12.4 Keys mode

For small data that one place writes: preferences, a layout.

- Each listed key is one record in `ext.<id>.k`. The whole value is one
  cell. Newest wins; conflicts are listed.
- No code needed. Capture runs after a successful `save_extension_data`
  or `delete_extension_data` of a matching key.
- **Base**: the version the calling window last loaded or saved for that
  key (`reads` table: window label, extension, key). A save from a
  window that read the key before a remote change arrived is concurrent
  with it, so the remote value goes to the conflicts list instead of
  being erased.
- Project: atomic write under the same per-key lock `save_extension_data`
  takes (added by WP-S5), then `extension_data_changed` with
  `source: "sync"` through EXTENSION_VIEWS' coalescer
  (`src/extension_data_events.rs`), which
  reaches `context.storage.onChanged` (EXTENSION_VIEWS §10).
- At most 1 MiB per key.
- `docs/EXTENSIONS.md` § "Your extension runs as MULTIPLE instances"
  gains: "A synced key can change at any time, as if another window wrote
  it. Re-read before acting."

### 12.5 Lists mode

For a key that holds a list of items with ids: Todos, bookmarks,
snippets.

- The host splits the file: each element is a record in
  `ext.<id>.l.<key>` (whole element, newest wins), and the rest of the
  object is one record `~rest`.
- The host keeps `ext_view[window, key]`: the element ids that window's
  instance last saw, from its last load or save of the key. It is per
  window, like the keys-mode `reads` base, because each window runs its
  own instance with its own copy. The host's own writes don't change any
  window's view: a window has seen an element only once it loaded it.
- On each save from a window:
  - added or changed elements become cells;
  - ids in that window's `ext_view` but missing from the save are deletes;
  - ids not in that window's `ext_view` and missing from the save (added
    elsewhere, never seen by this window) are kept and written back;
  - an element equal to the current merged value is not a change.
- Rows for a window are dropped when the window is destroyed.
- After a save or a remote change, the host writes the file: the
  extension's elements in its order, then unseen synced elements at the
  end in stamp order, minus deleted ones. Then it emits
  `extension_data_changed`. The extension must reload on that event.
- So a save over a stale copy never deletes an element the extension
  hadn't seen, and an element it saw and dropped is a real delete.
- Limits: 64 KiB per element, 20,000 elements per key, 8 MiB per file.

### 12.6 Collections mode and the runtime API

For larger stores with their own format, locks and journal: Mind Map.
The extension keeps its own storage as the primary copy. The host keeps
a replica, merges, and hands back a change feed. Sync never touches the
extension's files in this mode.

```js
// Present when context.hostFeatures includes 'sync.records' and the grant includes 'sync'.
const s = context.sync;

s.available;   // boolean snapshot: Kage sync is on and this extension's data sync is on here

await s.status();
// → { available, paused, needsInitialPush, deviceTag,
//     devices: [{ tag, os, lastSeenAt, thisDevice }],
//     usedBytes, maxBytes, lastSyncAt }
// No computer names: they often contain the user's name, which
// extensions never get (`get_user_info` is blocked for the same reason).

await s.push(collection, records, { deleted = [], initial = false } = {});
// records: [{ id, ...fields }]; the host drops `local` fields. deleted: [id].
// → { accepted, unchanged, rejected: [{ id, code }], off }
//   code: 'too_large' | 'bad_id' | 'bad_collection' | 'sensitive_field'
//       | 'newer_schema' | 'quota' | 'rate_limited'

await s.pull({ max = 500 } = {});
// → { cursor, more, changes: [{ collection, id, record /* null = deleted */, from: { device, at } }] }

await s.ack(cursor);

const off = s.onChanges(({ collections, reason }) => { /* pull */ });
// reason: 'remote' | 'enabled' | 'reset' | 'purged'; debounced 500 ms
```

Host semantics:
- **The host tracks the base.** `ext_base` records, per record and field,
  the cell the extension last saw: the one it pushed, or the one a pull
  delivered and it acked.
  - A pushed field equal to its base wasn't changed by the extension, so
    a newer remote value is kept.
  - A pushed field that differs is a new cell, with `anc` from the base.
  - So an extension never stores version tokens, and a push made over a
    stale copy becomes a conflict, never a silent overwrite.
- **Push after your own write succeeded**, and send it while you still
  hold your own cross-window lock, so pushes leave in the order the writes
  happened. You don't have to hold the lock until the reply arrives: the
  engine applies every push, pull and ack of an extension in the order it
  receives them, from whichever instance. (Mind Map sends the push, then
  releases its 4-second lease, and clears the pushed ids from
  `syncPending` when the reply says they were accepted.) A pushed record
  equal to the current merged value is dropped, so normalising data on
  apply can't start an echo loop.
- **Pull** delivers the newest merged version per id (coalesced), oldest
  first, for every record whose merged value differs from the
  extension's base: remote changes, and records where a push merged with
  a remote field. Delivery is at least once (several windows; a crash
  before `ack`): apply idempotently, as upserts by id.
- **Ack** advances the shared cursor and updates `ext_base`. Any
  instance may ack. It never moves a field's base back: a field that a
  later push already based on a newer cell keeps that base.
- **Initial push.** When `needsInitialPush` is true (first time on this
  computer, after `reason: 'enabled'` or `'reset'`, or a reinstall), push
  every record once with `initial: true`, in batches. Stamps come from
  each record's `updatedAt` (capped at now, with no floor, §8.2), so the
  newer copy wins, not the computer that joined later. Then pull what is
  missing.
- **Off.** While data sync is off for the extension, `push` returns
  `{ off: true }` and stores nothing.
- **Not installed here.** Records stay in the replica and keep syncing.
  After install, `needsInitialPush` is true and the first pull returns
  everything. A reinstall after an uninstall gets its data back.
- **Purged** (`reason: 'purged'`, §8.7): data sync is now off here; local
  data is untouched.
- **Schema.** A collection's shards carry the manifest's `sync.schema`
  (§8.11). An older extension version doesn't receive a newer-schema
  collection, and a push to it returns `newer_schema` (WP-S5 gates per
  collection, not per record).
- **Device tag.** `deviceTag` is `HKDF(extdev, extension id ‖ device id)[..12]`,
  as 24 lowercase hex characters (§11.3):
  stable per extension, not linkable across extensions, meaningless
  outside your computers.
- **Ids** match `^[A-Za-z0-9_.:-]{1,128}$`.
- **Crash safety on the extension side.** Mark records "not pushed yet"
  in the same write as the change, and push marked records on load.
  Re-pushing an unchanged record costs nothing.

Host side:
- `src/commands/extensions/sync.rs`: the four `ext_sync_*` commands.
  They validate sizes, ids and field names on the calling thread, then
  forward to the engine thread with a 10-second timeout. The engine
  answers extension requests between the steps of a tick (between shard
  merges, never in the middle of one), so a push waits for at most one
  merge, not for a whole ingest or a join.
- The sandbox host forwards `extension_sync_changes` for this extension
  over its port as `{ type: 'sync-changes', collections, reason }`. The
  runtime dispatches it to `onChanges` handlers.

### 12.7 Limits

| Limit | Value |
|---|---|
| Record (inline) | `maxBytes`, default 64 KiB |
| Record (`large` collection) | up to 4 MiB, stored as a blob |
| Keys-mode value | 1 MiB |
| Lists-mode element / elements / file | 64 KiB / 20,000 / 8 MiB |
| Push call | 500 records and 4 MiB |
| Push rate | 5,000 records per minute per extension (token bucket), then `rate_limited` |
| Pull page | 500 changes |
| Per extension, total | 128 MiB. Over it, pushes return `quota`, data sync pauses for that extension, and Settings → Sync says so. |
| Collections / key patterns / lists | 16 / 32 / 16 |
| Invokes | the existing 100 per second per sandbox |

### 12.8 Lifecycle

| Event | What happens |
|---|---|
| Install on computer A | `installs` record published. Other computers get a suggestion (§4.5). |
| Grant `sync` on A | `needsInitialPush`, then sync starts |
| Disable on A | `extension_states.<id> = false` syncs (shared). Data sync pauses on A while it is off. |
| Uninstall on A | A deletes its local data and settings (today's behaviour) and stops syncing them. Capture turns none of that into deletes (§8.2), so the folder copy and the other computers keep everything. Other computers get "Removed on A [Remove here] [Keep]". When the extension's data syncs, the uninstall confirm adds: "Its data stays in your sync folder and on your other computers. To delete it everywhere, use Settings → Sync → Extensions → Remove its data from the sync folder." |
| Reinstall on A | From the same origin (§12.3): settings come back, the initial push sends nothing, then everything arrives through `pull`. From a different origin: nothing comes back (`origin_mismatch`). |
| Remove its data from the folder (Settings → Sync) | Purge (§8.7), after a confirm |
| Different extension versions on two computers | `sync.schema` gating (§8.11) |

### 12.9 Mind Map on this API

Mind Map's records already have stable random ids, `rev`, `updatedAt`
and `mergedInto` (SPEC §6). Mapping:

| Mind Map storage | Sync |
|---|---|
| `mm-items-YYYY-MM` items | Collection `items`. Groups: `status`+`doneAt`+`suggested`+`confidence`, `title`+`titleSource`, `summary`+`summarySource`, `url`+`canonicalUrl`. Sets: `entityIds`, `terms`. Map: `mentionOrigin`. Local: `rev`, `enrich`, `v`, `fileRefs` (EXTENSION_FILE_TEXT §11.4: a `file_ref` means nothing on another computer). Label `title`, private `private`. Everything else (`text`, `owner`, `dueAt`, `org`, `source`, `file`, `body`, …) is a whole field. |
| `mm-entities` entities | Collection `entities`. Set: `aliases`. Local: `rev`. Label `name`, private `private`. `mergedInto` is its own field, so a merge on one computer survives a concurrent rename on another. |
| `mm-entities` candidates | Not synced: derived counters, rebuilt on each computer |
| `mm-links` links, rejections | Collections `links` and `rejections`, `whole`. Id `${s}:${t}:${type}`. |
| `mm-body-<id>-<rev>` (file text from EXTENSION_FILE_TEXT) | Collection `bodies`, `whole`, `large`, 4 MiB. Id = item id. Record `{ key, format, chars, text }`, where `key` is the item's `body.key`. On apply Mind Map writes the text under `key`, after checking it starts with `mm-body-<this id>-`. Never pushed for a private item while "Sync private items" is off. A document whose `file.warnings` include `possible_secret` is never pushed at all, item or body: the item's `text` holds the first 8,000 characters and may hold the secret too (EXTENSION_FILE_TEXT §10). |
| `mm-layout` pins (EXTENSION_VIEWS) | Collection `pins`, `whole`. Id = node ref. Record `{ x, y, at }`. |
| `mm-view`, `mm-meta`, `mm-journal`, `mm-lock`, `nx-*` | Not synced: per-computer view state, bookkeeping, locking, crash twins and undo |
| Settings | Through the Extensions category: `week_start`, the toggles, `agent_snippet_chars`, `trigger`, `doc_max_chars`; `my_names` with `"sync": "shared"`; `background_organize` with `"sync": "machine"` |

What Mind Map does (WP-S7):
1. **Push** after each committed op: the records the op touched and the
   ids it deleted. Their ids go into `meta.syncPending` in the same meta
   write. The push is sent inside `withLock`, after the journal and meta
   writes, and its reply is awaited after the lease is released (§12.6),
   so a slow reply never outlasts the 4-second lease. The next meta write
   clears ids whose push was accepted. On load, pending ids are pushed
   again.
2. **Pull** on `onChanges` and in `syncIfStale`: inside `withLock`,
   `pull`, then `applyRemote(state, changes, env)`, then persist the
   touched keys and meta, then `ack`, then `requestReformat()`.
   `applyRemote` is a new **silent** op kind `sync`, which `mm undo`
   skips. It sets `rev = local.rev + 1` and keeps local fields, so undo
   of an older local op that a remote change touched is refused as
   `superseded`, which is right. A record that is new on this computer
   gets `enrich = { state: 'done' }`, so a link's title is fetched once,
   on the computer that captured it, and two computers don't race to
   rewrite the same title. It never pushes what it applied.
3. **Deterministic entity dedupe.** Two computers can promote the same
   candidate before syncing. On apply, entities with the same `(type,
   key)` and no `mergedInto` merge: the larger id gets `mergedInto` the
   smaller one. Every computer reaches the same result and pushes equal
   values. Items pointing at the larger id resolve through the redirect.
4. **`me`** has the literal id `me` everywhere, so "my actions" merge
   naturally. Unassigned actions stay owned by `me`. "Names that mean
   you" is a shared setting.
5. **Private items sync**, encrypted. "Private" means "never sent to the
   agent", on every computer. A setting **Sync private items** (default
   on) lets the user keep them on the computer where they are: when off,
   Mind Map doesn't push private records, and making a synced item
   private asks "Remove it from your other computers and keep it only
   here?".
6. **Background organising** (off by default) is `"sync": "machine"`:
   each computer sends notes to its own agent, which may be a cloud
   service on one and a local model on another, and consent is per
   computer. Work is split across computers by the `machine_tag` rule of
   EXTENSION_AGENT_CALLS §11.3. The `org` marks on items sync like other
   fields. There is no lease.
7. **Document text.** Because sync encrypts extension data, bodies
   sync, and EXTENSION_FILE_TEXT has no document sync switch. Documents
   that look like they hold a credential stay local, item and body (the
   `bodies` row above).
   A body that isn't here yet, or stays on another computer, shows "The
   full text is on another of your computers" (extensions get no computer
   names, §12.6), and search uses title, summary and keyphrases.
8. **Feature detection.** Mind Map works unchanged on hosts without
   `sync.records`.
9. **Disclosure** (SPEC §15 and the settings page): "With Kage sync on,
   your mind map is also kept, encrypted, in your sync folder, so it's on
   all your computers." SPEC §15's "What goes where" changes too: window
   titles and app names (`source.app`, `source.window`, recorded by
   default) and document text are on disk and, with sync on, in the
   encrypted sync folder; they are still never sent to the agent. The
   OPEN_QUESTIONS Q13 wording "It stays on this computer" becomes "It's
   never sent to your agent".
10. Actions stay separate from Todos. Nothing here links the two.
    Defaults stay: agent tools on, fetch link titles on, record source
    window on.

---

## 13. Settings → Sync

A new sidebar item, `data-section="sync"`, the first item of the last
sidebar group (`settings.sidebar.about_group`), before Privacy. (The
Extensions group above it ends with the extensions' own items, which
`manager.js` adds at run time.) Module: `SyncSettingsModule` in `ui/js/settings/sync.js`, with
`persistsImmediately = true`. About → Backup & restore gets a one-line
pointer to it. The UI says "computer", never "device", "scope", "layer",
"tombstone" or "merge".

### 13.1 Sync is off

```text
Sync
Keep your settings, shortcuts and extensions the same on all your computers.
Kage uses a folder you already sync, like Dropbox, iCloud Drive or OneDrive.
Nothing goes to a Kage server, and everything Kage puts there is encrypted.

[ Set up sync… ]

Syncs: Settings · Shortcuts & automations · Extensions and their data
Stays on each computer: sign-ins and API keys, permissions, window positions,
logs. Chat history only if you turn it on.
```

### 13.2 Setup (steps in the page, not a dialog)

1. **Choose a folder.**
   - Quick picks from `sync_suggest_folders` (detected iCloud Drive,
     Dropbox, OneDrive and Google Drive roots; detection checks known
     locations and the clients' own info files, never scans the disk),
     plus "Another folder…" (the existing folder picker).
   - `sync_inspect_folder` result:
     - **new**: "Kage will create a folder named “Kage Sync” here."
     - **existing**: "This folder already has Kage sync from 2
       computers. This computer will join them." (Their names are
       encrypted; step 2 shows them once the passphrase unlocks the
       folder.)
     - **unknown sync**: a note, not a block: "Kage can't tell if this
       folder syncs. That's fine if another tool syncs it."
     - **can't use**: inside Kage's own folders, a drive root, a temp
       folder, read-only: explain and stay on this step;
     - **newer format**: "This sync folder needs a newer Kage. Update
       Kage on this computer first." [Check for updates].
   - Provider tip: OneDrive or Dropbox on Windows: "Kage will keep this
     folder downloaded." iCloud: "Keep Downloaded is recommended."
     Google Drive: "Use Mirror mode, or make the folder available
     offline." Syncthing: "Works best when two of your computers are
     often online at the same time."
2. **Passphrase.**
   - Create: two fields, the 12-character rule, [Make one for me], and
     "Kage can't recover this passphrase. You'll type it once on each of
     your computers. If you lose it, you can change it from any computer
     that still syncs."
   - Join: one field. Once it unlocks: "This folder has Kage sync from
     MacBook Air (macOS) and Desktop PC (Windows)."
3. **This computer's name.** Default: the OS computer name. Stored only
   inside the encrypted files.
4. **What to sync on this computer.** The four categories with their
   defaults. Chat history shows its size ("About 120 chats, 35 MB").
   "These choices are for this computer. Each computer chooses for
   itself." One line: "Shortcuts and automations sync as you typed them.
   If you pasted a key into one, it syncs too, encrypted."
5. **Join only: "This computer already has settings. Which should win
   where they differ?"** The two choices of §8.8, with the preview counts
   and "Nothing is lost. Replaced values go to Conflicts, where you can
   restore them. You can undo joining for 30 days."
6. **First sync.** Progress ("Waiting for Dropbox to download 4
   files…"), then:

```text
✓ Sync is on
Synced 42 settings, 12 shortcuts, 3 automations and 2 extensions' data.

From your other computers:
  Mind Map, Timer — not installed here yet           [Review]
  1 synced item can run programs on this computer    [Review]
9 settings on this computer were replaced by synced ones   [Review] [Undo]
                                                            [Done]
```

### 13.3 Sync is on

```text
Sync                                                   [Sync now] [Pause ▾]
● Up to date · Last synced 2 minutes ago
Folder: Dropbox › Kage Sync                  [Show folder] [Change…]
This computer: MacBook Air                   [Rename]

What syncs on this computer
 [✓] Settings                   Preferences, agents, app modes, personalization
 [✓] Shortcuts & automations    12 shortcuts · 3 automations · 2 custom actions
 [✓] Extensions                 6 extensions · data for Mind Map and Todos  [Choose…]
 [ ] Chat history               Off

Needs your attention                         (hidden when empty)
 ⚠ 2 conflicts                                                [Review]
 ⬇ Mind Map and Timer are on your other computers             [Review]
 ▶ 1 synced item can run programs on this computer            [Review]
 🗑 Work PC deleted 1,204 Mind Map items                       [Review]
 🔒 Stocks needs approval on this computer                     [Review]

Your computers
 MacBook Air   macOS     this computer
 Desktop PC    Windows   synced 3 hours ago                    [⋯]
 Old laptop    Windows   last synced 7 months ago · inactive   [⋯]

Not synced here
 2 values stay on this computer because they look like keys    [Details]
 1 setting from your other computers doesn't work on this version

Stays on each computer
 Sign-ins and API keys · tool permissions · extension permissions ·
 window positions · start at login · privacy choices · logs   [Learn more]

Advanced ▾
 Change passphrase… · Where the key is kept: Keychain ·
 Undo joining (until Dec 2) · Turn off sync on this computer… · Start over…
```

- **Pause**: 1 hour, until tomorrow, until I resume. Local changes are
  still captured, with their times, and sent on resume. Pause survives
  restarts.
- **Sync now**: an immediate tick.
- **Extensions → Choose…**:

```text
Extensions
Which extensions you use and their settings sync with "Extensions".
Some extensions can also sync their data.

 🧠 Mind Map   settings and notes · 14 MB       [✓] sync data   [Remove its data from the sync folder…]
               Your notes, actions, links and map layout. Encrypted.
 ✅ Todos      settings and todos · 40 KB       [✓] sync data
 🎵 Spotify    settings only                    sign-in stays on each computer
 📈 Stocks     settings only                    this extension doesn't sync data
```

- **Details** for held-back values lists labels only, never values.
- **Your computers → ⋯** on another computer: **Remove from sync…**
  (with "It's lost or stolen. Change the encryption key and the
  passphrase."). On this computer: **Rename**.

### 13.4 Status line

One line, one state, plain words. The state comes from
`sync_get_status` and updates through `sync_status_changed`.

| State | Text | Action |
|---|---|---|
| `up_to_date` | Up to date · Last synced {time} | |
| `syncing` | Syncing… | |
| `waiting` | Waiting for {count} files from {computer} to download | [Learn more] |
| `placeholders` | Your sync folder hasn't downloaded some files yet | [How to keep it downloaded] |
| `paused` | Paused until {time} | [Resume] |
| `locked` | Enter your sync passphrase to keep syncing | [Enter passphrase] |
| `folder_missing` | Can't find your sync folder | [Find folder] [Choose folder…] |
| `folder_denied` | Kage isn't allowed to open your sync folder | [Allow access] |
| `folder_empty` | Your sync folder no longer has Kage data | [Start over from this computer] [Turn off sync] |
| `reset` | Sync was started over on {computer} | [Join again] |
| `removed` | This computer was removed from sync on {computer} | [Join again] [OK] |
| `update_required` | Update Kage on this computer to keep syncing | [Check for updates] |
| `disk_full` | Your disk is full. Sync continues when there is space. | |
| `write_failed` | Kage can't write to your sync folder | [Learn more] |
| `folder_reverting` | Your sync app keeps undoing Kage's files | [Learn more] |
| `clock_warning` | {computer}'s clock is about {minutes} minutes off. Changes from it may win over newer ones. | [Learn more] |
| `unreadable` | {count} files from {computer} can't be read. Kage uses its other data. | [Learn more] |
| `other_engine` | Another copy of Kage is syncing this profile | |

"Up to date" means: this computer wrote all its changes and read
everything the other computers' newest heads list. The cloud app moves
files between computers, so "Your computers" shows when each one last
wrote; that is the real picture.

### 13.5 Turn off sync on this computer

```text
Turn off sync on this computer?
This computer keeps its settings as they are now.
Your other computers keep syncing.
                                         [Cancel] [Turn off]
```

Leave (§8.9). The local data stays. `sync/trash/` is kept until its
copies are 30 days old.

### 13.6 Start over

```text
Start sync over?
This moves everything in “Kage Sync” aside and starts again from this
computer's settings, with a new passphrase. Your other computers stop
syncing until you join them again. Their settings stay as they are.
                       [ ] I understand every computer must join again
                                         [Cancel] [Start over]
```

- Moves `devices/`, `keys/` and `kage-sync.json` into
  `archive/<YYYYMMDD-HHMMSS>/`, creates a new descriptor and key, and
  publishes this computer as the first one. Other computers see a new
  `folder_id` and show `reset`.
- Any computer deletes archives older than 30 days.

### 13.7 Undo joining, change passphrase, change folder

- **Undo joining** (30 days): turns sync off here, restores the join
  backup through the existing all-or-nothing import, and removes the
  join entries. Data already published stays in the folder: "Your other
  computers keep what this computer sent."
- **Change passphrase**: new passphrase twice (§11.3).
- **Change folder**: turns sync off here and runs setup with the new
  folder (create or join). The page says other computers need the same
  change. **Find folder** (when missing) looks for our `folder_id` in the
  detected cloud roots, 3 levels deep.

### 13.8 Tray and notifications

- The tray menu gets a disabled status line ("Sync: up to date") and
  **Sync now**.
- OS notifications only for states the user must act on, and only after
  they last more than an hour: `locked`, `folder_missing`,
  `update_required`, `removed`, `reset`. At most once a day per state.
- Conflicts and suggestions never notify. They show in "Needs your
  attention" and as a badge on the Sync sidebar item.

---

## 14. When sync runs

### 14.1 Triggers

| Trigger | What runs |
|---|---|
| App start | Nothing on the startup path. `FeatureServices` gains `sync: SyncHandle`, built with `SyncHandle::pending()` on the Builder (cheap, no thread, no I/O). It isn't a new managed type, so `tests/state_manage_parity_test.rs` doesn't change. The `kage-sync` thread starts 10 seconds after `setup()` finishes (or after the first window paints, whichever is later). It reads `device.json`, opens the replica, finishes unfinished projections, and runs one tick. `config.json` is already the effective config, so the app never waits for the folder. |
| Local change | A hook sends a wake (a channel send, no I/O). Capture 2 s after the last change (10 s on battery). Publish 5 s after the last own change, at most 30 s after the first. |
| Folder change | `notify` watcher on `Kage Sync/devices/`, recursive. Ingest 3 s after the last event. Events under our own directory and for temp names are ignored. |
| Poll (the watcher's backstop) | Every 5 minutes on mains power, 15 on battery, 30 on low battery or in power-saving mode (`os::power`, `automation_power.mode`). A poll lists directories; heads are read only when a listing changed. |
| A Kage window is shown or focused | Ingest, if the last one was more than 60 s ago |
| Wake from sleep, network back | Ingest after 10 s |
| Echo publish | With the next own publish, else after 30 minutes; never on low battery |
| Settings → Sync visible | Status refresh every 5 s |
| Shutdown (`graceful_shutdown`, `RunEvent::Exit`) | `sync::flush_for_exit(1.5 s)`: capture and publish own edits only, no ingest. 500 ms on the updater path, before `release_kill_on_exit_job`. Whatever doesn't finish is published at the next start; it is already in the replica. Order in `graceful_shutdown`: background agent calls stop first (EXTENSION_AGENT_CALLS §5.7, up to 2 s), then this flush, then the agent disconnect and `release_kill_on_exit_job`: at most 3.5 s, 2.5 s on the updater path. |
| Paused, locked, folder missing | No folder I/O at all. Capture continues into the replica. |

### 14.2 Threads

- One `kage-sync` engine thread owns the replica and does all the work.
- A `kage-sync-io` pool of 2 threads does all folder I/O: reads with a
  20-second timeout each, listings with 20 seconds, writes, renames and
  removes with 60. A call stuck in a cloud driver is abandoned and its
  thread replaced. At most 4 abandoned threads; then every call fails at
  once with `fsio::Unresponsive`, the status says the folder isn't
  responding, and sync pauses for 10 minutes. The threads start on the
  first call, not at app start.
- Argon2 runs on the engine thread only at setup, join, unlock and
  passphrase change.
- Tauri commands send a request over a channel and wait with a timeout.
  They never touch the folder on a Tokio worker.
- `engine.lock`: one engine per profile.

### 14.3 Cost

| Situation | Budget |
|---|---|
| Startup | Under 1 ms before the first window: one struct on the Builder, one thread spawn. No folder or database I/O. |
| Idle | No timers except the poll. A poll is a few directory listings. No decryption. |
| One setting changed | One shard of a few KiB, one head, one config save. Under 10 ms of CPU. |
| Mind Map at 10,000 items (about 6 MB) | First push about 0.3 s of CPU on the engine thread; after that one 128 KiB shard per edit burst |
| Tick with nothing changed | Under 10 ms |
| Tick with one changed bucket of 10,000 records | Under 200 ms |
| Memory | Replica in SQLite with an 8 MiB cache. Under 20 MB for 3 computers × 10,000 records. |
| Battery | Watcher events plus a slow poll; echo publishes and chat backfill wait for mains power |

---

## 15. Failure modes and recovery

Rules that hold in every row:
- local data is never deleted because of a folder problem (R9);
- another computer's files are never deleted, except a retired or
  removed computer's directory after its last head is fully ingested
  (R1);
- every pause shows its reason in Settings → Sync and in the tray line.

| Failure | Detection | Behaviour | Recovery |
|---|---|---|---|
| Partial or in-progress download | AEAD fails or length short | `waiting`, retried with backoff (5 s, 30 s, 2 min, 10 min, then the poll) | Automatic. After 10 minutes: skipped and reported as `unreadable` for that computer; nothing lost (R6). |
| Placeholder never downloads (offline, eviction) | `residency` | `waiting`, then `placeholders` after 3 ticks | Download requested; Windows pinning; macOS tip |
| A read hangs in a cloud driver | I/O pool timeout | That file counts as waiting | Thread abandoned; at most 4, then a 10-minute pause |
| Folder unmounted, moved or signed out | No `kage-sync.json` with our `folder_id` | `folder_missing`; no folder I/O; never re-created | [Find folder] or [Choose folder…]. Edits wait in the replica. |
| macOS denies folder access | `EPERM` | `folder_denied` | [Allow access] opens the picker on that folder |
| `Kage Sync` emptied by the user | Descriptor gone, `devices/` empty | `folder_empty` | [Start over from this computer] or [Turn off sync] |
| Started over on another computer | `folder_id` changed | `reset`; no writes | [Join again] |
| Key epoch changed | A newer key file | `locked`; reading old-epoch heads continues | [Enter passphrase] |
| Keychain entry gone or denied | Key store error | `locked` | Enter the passphrase. The file key store is offered. |
| Wrong passphrase | Unwrap fails for every key file of the epoch | Ask again; nothing written | |
| Forgotten passphrase | | | Change it from any computer still syncing (§11.3) |
| Head or layout from a newer Kage | `min_reader` | `update_required` for that computer's data; this computer keeps publishing | Update Kage |
| `config.json` from a newer build | `Config::saves_suspended()` | Config namespaces paused | Update Kage |
| `config.json` reset at load | `Config::load` reset flag | No capture; synced values projected; notice | Automatic (§8.10) |
| A value this version can't use | Validation fails, or a lossy round trip | `invalid_here` or `lossy` pin; local value stays | A newer Kage applies it |
| Disk full (local) | `ENOSPC` / `ERROR_DISK_FULL` | `disk_full`; capture and publish stop, the user's edits stay in the local files | Retried each tick; capture catches up from the files |
| Write to the folder fails (quota, sharing violation) | I/O error | Retry after 5 s, 30 s, 5 min, then hourly; `write_failed` | Edits stay in the replica |
| Cloud quota full or the sync app stopped | Other computers' heads stop moving | "Desktop PC · last synced 3 days ago" | Outside Kage; "Learn more" explains |
| Provider duplicates | Classifier | Deleted in our own directory; ignored elsewhere; counted | Automatic |
| Two computers with one device id | Fingerprint, `instance`, duplicate heads, equal stamps with different values | Rotate the id; break ties by value hash; conflicts recorded | Automatic |
| Our directory rewound or deleted | Lower `seq`, or missing shards | Republish everything; after 3 times in an hour `folder_reverting` and a 1-hour pause | Fix the sync app (for example Syncthing "Receive Only") |
| `replica.db` corrupt or deleted | SQLite error, or missing with `enabled` | Move it aside; rebuild from the folder; join with "use the other computers' values"; local differences become conflicts | Automatic, one notice |
| `device.json` corrupt | Parse failure | Treated as off; the folder is untouched | Set up again; joins the existing folder |
| Crash mid-ingest | | Per-shard transactions | Resumes next start |
| Crash mid-publish | Shards without a head | Orphans removed after 24 h | Automatic |
| Crash mid-projection | `projected` not yet updated | The next tick reads the local file back, sees it already holds the value, records it, publishes nothing | Automatic |
| Clock far off | Remote stamp ahead of local time | `clock_warning` | Fix the clock; correctness holds |
| Extension over its limits | Push size or rate | `quota` / `rate_limited`; data sync paused for it; a line in Settings → Sync | Remove data, or turn its data sync off |
| Extension not installed here | | Data kept in the replica | Applied on install |
| Bulk delete | Thresholds (§8.10) | Held; question with counts; trash copy | User answers |
| Two builds fighting over a value | Churn detector | Frozen for the session; logged by key | Automatic |
| A second Kage on one profile | `engine.lock` busy | That process doesn't sync | `other_engine` status |
| Removed elsewhere | `sync.devices/<me>.revoked` | Stop; `removed` | [Join again] |
| Syncthing peers rarely online together | Old `last_seen` | Each computer's last-seen time shown | User |

Logging: `log::*`, English. Item keys, namespaces, file names, counts and
device ids may be logged. Values, labels, computer names, passphrases and
folder paths never are; the folder is "the sync folder".

---

## 16. Telemetry

Through `telemetry::track`, so nothing is sent without consent. Props are
bucketed counts and fixed Kage-state enums only. Never sent: paths,
folder, provider or computer names, setting names, values, item counts
except in buckets, extension ids.

| Event | When | Props |
|---|---|---|
| `sync_enabled` | Setup finished | `role`: `create` / `join`; `categories`: 0–4 |
| `sync_disabled` | Turned off, removed, start over | `reason`: `leave` / `removed` / `reset` |
| `sync_daily` | With `app_daily_active`, while on | `computers`: `1`, `2`, `3-5`, `6+`; `categories`: 0–4; `ext_data`: `0`, `1-2`, `3-5`, `6+`; `conflicts_open`: `0`, `1-5`, `6+`; `held`: `0`, `1+`; `problems`: `0`, `1`, `2+` |
| `sync_conflict_action` | Restore, dismiss | `action`: `restore` / `dismiss` / `dismiss_all`; `category`: `settings` / `automations` / `extensions` / `chats` |
| `sync_problem` | First time per UTC day per kind | `kind`: one of the §13.4 problem states |
| `sync_scope_changed` | A scope action | `action`: one of the §9.3 actions |

Which sync app the folder belongs to is deliberately not sent (§22).
Add the names to `KNOWN_EVENTS` in `ui/js/shared/telemetry.js` and a
"Sync" entry to `docs/PRIVACY.md`: "Sync data never touches Kage servers.
Kage only counts that sync is used, how many computers, and which kinds
of problems happen."

---

## 17. i18n

- Every visible string goes through `t()` or `t!()`. Key groups:
  `settings.sync.*` (page and setup), `settings.sync_scope.*` (the
  per-row control), `sync.status.*`, `sync.problem.*`,
  `sync.conflicts.*` (including `sync.conflicts.kind.*`),
  `sync.review.*` (held items, suggestions, bulk changes),
  `settings.manager.cap.sync.*` (the capability, the pattern every
  capability uses), `settings.manager.ext.sync.*` (the chip on an
  extension's settings page; the four designs share the
  `settings.manager.ext.<block>.*` prefix, §23), `errors.sync.*`.
- Rust errors use `AppError::keyed`. New `ErrorKind`s: `sync_unavailable`
  (off, locked, paused or not built yet), `quota_exceeded`,
  `needs_confirmation` (a destructive command without a valid token).
  `rate_limited` is reused.
- Status problems and conflict labels are `{ key, params }`. The window
  renders them in the active language. Rust never renders them.
- User text in params (computer names, item names, extension names) is
  never translated and is bidi-isolated (`config_export::sensitive::isolate`,
  FSI…PDI in the frontend).
- `dir="ltr"`: folder names, the generated passphrase, hotkeys, commands
  and scripts in the review dialog, device ids in detail views, values
  that look like code.
- OS names are catalog strings. "This Mac", "This PC" and "This computer"
  are separate keys, chosen by OS, and "Macs only" uses ICU `select`:
  `{os, select, macos {Macs only} windows {Windows PCs only} other {Linux PCs only}}`.
- Plurals use ICU: `{count, plural, one {# conflict} other {# conflicts}}`
  for conflicts, files, computers, settings and items.
- Times use `Intl.DateTimeFormat` and `Intl.RelativeTimeFormat`, and the
  user's `ui.date_format` / `ui.time_format`.
- The manifest's `sync.description` supports `__MSG_key__`. Extension
  conflict labels use the extension's own catalog for its name.
- `README.txt` in the folder is written once in the creator's language,
  with English below.
- Logs stay English.
- A path from a newer Kage with no label here shows the raw path, LTR.
- `scripts/check_i18n.py` covers the new keys; `scripts/translate.py`
  fills the other 30 languages.

C0 adds the keys below, plus one key per value of the contract enums:
`sync.status.<state>` for every `SyncState` except `unavailable` (and
`sync.status.paused_forever`), `sync.problem.clock_warning` and
`.unreadable`, `sync.conflicts.kind.<kind>` with the list's lines, tabs
and buttons, `settings.sync_scope.action.<action>`,
`settings.sync_scope.reason.<why>` (the catalog's reasons and the held
reasons), `settings.sync.category.<category>`, `sync.os.<os>`,
`sync.this_computer.<os>`, `tray.sync_status`, `tray.sync_now` and the
three `errors.sync.*`. Later strings go through fragments (§19.1).

Main EN strings:

| Key | EN |
|---|---|
| `settings.sync.title` | Sync |
| `settings.sync.intro` | Keep your settings, shortcuts and extensions the same on all your computers. |
| `settings.sync.no_server` | Kage uses a folder you already sync, like Dropbox, iCloud Drive or OneDrive. Nothing goes to a Kage server, and everything Kage puts there is encrypted. |
| `settings.sync.setup` | Set up sync… |
| `settings.sync.passphrase.lost` | Kage can't recover this passphrase. If you lose it, you can change it from any computer that still syncs. |
| `settings.sync.passphrase.wrong` | That passphrase doesn't unlock this folder. |
| `settings.sync.categories.per_computer` | These choices are for this computer. Each computer chooses for itself. |
| `settings.sync.join.theirs` | Use the settings from my other computers |
| `settings.sync.join.mine` | Use this computer's settings on all computers |
| `settings.sync.join.safe` | Nothing is lost. Replaced values go to Conflicts, where you can restore them. You can undo joining for 30 days. |
| `settings.sync.stays_local` | Stays on each computer: sign-ins and API keys, permissions, window positions, logs. |
| `settings.sync_scope.all` | All computers |
| `settings.sync_scope.os_only` | {os, select, macos {Macs only} windows {Windows PCs only} other {Linux PCs only}} |
| `settings.sync_scope.this_computer` | This computer |
| `settings.sync_scope.per_os` | Each kind of computer has its own |
| `settings.sync_scope.not_synced` | Not synced. {reason} |
| `settings.sync_scope.changed_everywhere` | Changed on all your computers. |
| `sync.status.up_to_date` | Up to date · Last synced {time} |
| `sync.status.locked` | Enter your sync passphrase to keep syncing |
| `sync.conflicts.intro` | When two computers changed the same thing, Kage kept the newest change. The other one is saved here for 90 days. |
| `sync.review.runnable_title` | {count, plural, one {1 synced item can run programs on this computer} other {# synced items can run programs on this computer}} |
| `sync.review.mass_delete` | {computer} deleted {count, plural, one {1 item} other {# items}} from {what}. |
| `settings.manager.cap.sync.label` | Sync between your computers |
| `settings.manager.cap.sync.desc` | Copies this extension's data, encrypted, to your sync folder so your other computers get it. Only when Kage sync is on. |
| `settings.manager.ext.sync.settings_and_data` | Syncs settings and data |
| `settings.manager.ext.sync.settings_only` | Syncs settings |
| `settings.manager.ext.sync.sign_in_local` | Sign-in stays on this computer |
| `settings.sync.ext.origin_mismatch` | A different extension with this id is installed here, so its data doesn't sync. |
| `settings.sync.ext.sideload_hint` | Only turn this on if it's the same extension on all your computers. |
| `settings.sync.uninstall_note` | Its data stays in your sync folder and on your other computers. To delete it everywhere, use Settings → Sync → Extensions → Remove its data from the sync folder. |

---

## 18. Test plan

Rules for every test:
- No test touches real user folders or the real keychain. The engine
  takes explicit directories (`EngineEnv`), never `crate::paths`
  globals, so one test process can run several computers
  (`paths::isolate` is process-wide). The key store is the memory
  backend whenever `paths::is_isolated()`.
- Integration tests call `kage::paths::isolate(<tempdir>)` first.
  `tests/user_dirs_isolation_test.rs` stays green: no `dirs::` calls in
  `src/sync`.
- Never run the full `cargo test` locally. Run targeted suites:
  `cargo test --lib sync::`, `cargo test --test sync_two_computers_test`.
  CI runs everything.

### 18.1 Unit tests (pure, inline)

| Module | Tests |
|---|---|
| `hlc` | Monotonic when the wall clock steps back; `floor` respected; `observe` capped at +1 hour; logical counter carry; persistence round trip; the text form sorts like the number |
| `merge` | Higher stamp wins; same value means no conflict; `anc` and own-run rules; equal stamps with different values (clone) break by hash and record a conflict; sets and maps never conflict; tombstone visibility; edit after delete; `edit_vs_delete` and `delete_vs_edit` in both orders |
| `merge` (property, seeded `rand`, 10,000 cases in CI) | Random cell sets from 2–5 simulated computers following the base rule: every delivery order, with duplicates, gives the same records; merge is commutative, associative and idempotent; every value that stops being current is either named in its winner's `anc` or recorded as a conflict |
| `scope` | `effective`, `edit_target` and every row of §9.3 produce exactly the listed writes; `exists_here` for `avail` |
| `split` | Split then join round-trips; groups, sets, maps and local fields; element key length limit |
| `catalog` | `every_config_field_has_a_sync_decision`; `no_machine_or_never_path_produces_cells`; every "names a file Kage writes or a program it starts" path is "This computer"; label keys exist in the EN catalog; `RENAMED` resolves |
| `crypto`, `envelope` | Seal/open round trip; any changed AAD component fails; truncation at every byte fails as `Truncated`, not `BadTag` when short; a bit flip fails; wrong epoch; content-name mismatch detected; wrap, unwrap, rewrap; HKDF labels fixed (golden vectors) |
| `folder::classify` | Every provider duplicate pattern of §3, iCloud stubs, Syncthing, Dropbox and Office temps, OS junk, strict names |
| `sources::config` | Project then capture gives no diff; unknown enum → `lossy` pin, never exported; consent pin when the grant doesn't cover the manifest; held-back strings; config-reset guard; `ensure_item_ids` stable; re-identification after `sync_id` was stripped; fractional ordering merges two reorders |
| `sources::extensions` | `normalize_sync` drops malformed parts and keeps the rest; sensitive keys and fields never become cells; the §12.1 settings rule against `Rules`; keys-mode window base; lists-mode stale save keeps unseen elements and deletes seen-then-dropped ones, with the view tracked per window (a window that loaded before a remote add doesn't delete it); uninstall produces `not_installed` pins, never tombstones; collections base tracking, coalesced feed, at-least-once redelivery, `initial` stamps, quotas and rate limits |
| `guards` | Mass-change thresholds (50 / 20% / minimum 10); runnable hold and approval hash; churn freeze |
| `conflicts` | Deterministic ids and stamps; two detectors write identical cells; a late detector can't reopen a dismissed conflict; retention by age, count and size; restore writes a non-conflicting cell plus `replaced_by_restore` |
| `gc` | Acks, the 90-day and active-device rules, the coming-back rule, purge |

### 18.2 Integration: computers on a simulated cloud

Harness `tests/sync_support/`:
- `SimCloud`: each computer sees its own copy of the folder. `step()`
  copies files between copies under a policy: delay, drop, reorder,
  deliver truncated first, write in place, show an iCloud stub or a
  dataless marker the fake `CloudFiles` reports, deliver provider-named
  duplicates, deliver an old version after a new one, lose a file,
  rewind a whole directory.
- `SimComputer`: a `SyncEngine` with its own `EngineEnv` (temp dirs),
  in-memory sources (a `Config` behind a fake `ConfigPort`, extension
  stores), `FakeClock`, the memory key store and an OS kind. Driven by
  `tick()`; no threads, no timers.

| # | Scenario | Expected |
|---|---|---|
| I1 | A creates. B joins with each join choice. | Same effective config except "This computer", "Never" and per-OS fields. Join conflicts listed and restorable from A. |
| I2 | Edit on A; step; tick B. | B has it. No conflict. |
| I3 | A and B edit the same setting while apart. | The newer wins on both. One conflict, visible on both. Restore on B wins everywhere with no new conflict; `replaced_by_restore` recorded. |
| I4 | B edits after seeing A's edit. | No conflict. |
| I5 | A changes a setting; B's Settings window holds an old copy and saves another field. | A's change survives (field-level save and `base_revision`). |
| I6 | A (macOS) and B (Windows) set different hotkeys; C (macOS) joins. | C gets A's hotkey; B unchanged. "Same on all computers" on B nulls both OS cells. |
| I7 | Mac override for font size. | Windows unaffected. "Use the value for all computers" returns Macs to the shared value. |
| I8 | "Only this computer" on A. | A's later edits never leave A; remote edits never apply on A; removing the pin restores the effective value. |
| I9 | Delete on A against edit on B, both stamp orders. | Newest wins; the right kind of conflict. |
| I10 | Reorder shortcuts on A while editing one on B. | Both apply. |
| I11 | A `run_program` shortcut made on a Mac. | Not on Windows. "Available on all computers" shows it on Windows with an empty path and the Mac's value as a hint. |
| I12 | Partial file from A. | Nothing applied from it; applied once complete; `waiting` then `up_to_date`. |
| I13 | Placeholder on B. | The tick doesn't block (under 50 ms per placeholder); applied after "download"; `placeholders` after 3 ticks. |
| I14 | Provider duplicate in A's directory; a duplicate head with different content. | Exact duplicate ignored; different content → clone handling on A. |
| I15 | B's clock is +1 day. | Same winners everywhere. `clock_warning` on A. Reading B's files never moves an HLC more than 1 hour past wall (`observe`); only an edit of a value B stamped follows it (§7.3 `floor`). |
| I16 | B offline 40 simulated days with edits; A keeps editing. | Newer wins; losers listed. Tombstones aren't cleaned before B returns. |
| I17 | B away 120 days; A deleted items meanwhile and cleaned the tombstones. | B's copies go to `deleted_while_away` (with the mass-change question when over the threshold); B's own new items are published. |
| I18 | An "older build" (catalog without path X, struct without field Y, `READER_VERSION` 1 against 2). | X and Y survive the older computer's edits; `min_reader` gating; `RENAMED` dual write. |
| I19 | A deletes 300 shortcuts. | B holds and asks. |
| I20 | B's `config.json` is corrupt at load. | B projects the synced values, publishes no defaults, shows the notice. |
| I21 | B is a clone of A (copied profile); both edit before seeing each other. | B rotates its id; spurious conflicts at most; no loss. |
| I22 | Backup import on A. | Reaches B; B's replaced values listed as `restore_backup`. |
| I23 | A synced automation with a schedule and a script arrives on B; later its script changes on A. | Off on B until approved; held again after the change; approval publishes nothing. |
| I24 | `extension_states.x = true` arrives where the grant doesn't cover the manifest. | Stays off; a review item. |
| I25 | Fake collections extension, two sandboxes per computer. | Push, pull and ack idempotent across instances; base tracking turns a stale push into a conflict; `initial` uses `updatedAt`; reinstall restores data; purge turns data sync off without touching local data. |
| I26 | Fake lists-mode extension saves a stale copy, from a window that loaded before a remote add while another window has already seen the add. | Unseen elements come back; seen-then-dropped are deleted; no echo publish. |
| I31 | Uninstall an extension on A (keys, lists and collections data, settings, on/off). | Nothing is deleted on B; B gets the "Removed on A" suggestion; reinstalling on A brings settings and data back. |
| I32 | A stale sandbox on A calls `save_extension_config` with an old copy after a setting changed on B. | B's value survives on both computers. |
| I27 | Passphrase changed on A; later "lost or stolen" rotation on A. | B unaffected by the change. After rotation B is `locked` until the new passphrase, then republishes under the new epoch. |
| I28 | One byte of A's shard changed; an older head of A replayed; a file planted without the key. | Rejected; replay ignored; planted file ignored. |
| I29 | Chats on: A publishes; B lists read-only; hide on B; A's agent files untouched. | As stated. |
| I30 | Leave on B; remove C from A; start over on A. | Directories cleaned as §8.9; others see `removed` or `reset`; no local data lost anywhere. |

### 18.3 Fault injection

- `folder.rs` writes through `SyncFs`. `FaultyFs` injects `ENOSPC`,
  `EACCES`, Windows sharing violations that clear after N retries, slow
  reads and hanging reads (the I/O pool timeout fires).
- Named kill points panic in test builds, then the engine restarts from
  the same directories: `after_blob_write`, `after_shard_write`,
  `before_head_write`, `mid_merge_txn`, `after_config_commit_before_projected`,
  `mid_gc`. After restart: no lost edit, no duplicate publish with a new
  stamp for an unchanged value, `config.json` valid.
- Key store errors: not found, denied, unavailable.
- A watcher that drops every event: the poll still converges.

### 18.4 Chaos test (`sync_chaos_test.rs`, seeded; CI-sized and long modes)

- 3–5 computers on mixed OSes, 2,000 random operations: edits, deletes,
  scope changes, reorders, extension pushes, pause and resume, offline
  periods of up to 120 simulated days, delivery that drops, delays,
  duplicates and partially delivers files.
- After quiescence:
  - every replica is equal;
  - every projection is equal per (OS, pins);
  - every value that lost is in the conflicts list or Recently deleted;
  - no visible record exists that some computer deleted after its last
    edit, unless an edit after the delete brought it back;
  - file counts per directory stay within §5.8.
- 100 seeds in CI mode.

### 18.5 Frontend (vitest, `ui-tests/`)

- `settings/sync.test.js`: every state of §13.4 renders from a mocked
  status; setup steps (12-character rule, confirm must match, generated
  passphrase copy, join choices, error states); conflicts grouping,
  Compare escapes HTML, Restore and Dismiss call the right commands;
  reviews; computers.
- `shared/sync-scope-chip.test.js`: the right label for each `ScopeView`;
  each menu item calls `sync_set_scope` with the right action; hidden
  when sync is off; RTL.
- `settings/manager-field-save.test.js`: `diffPaths` and `applyPaths`
  keep a remote change to a field the user didn't touch; list diff by
  `sync_id`; `saveConfig` passes `baseRevision`.
- `settings/sync-paths.test.js`: every catalog path shown in Settings
  has a `data-sync-path` (fixture from `sync_get_scopes(None)`).
- `extension-sandbox/sync-api.test.js`: `context.sync` exists only with
  `sync.records` and the grant; argument validation; `onChanges`
  delivery; `IDENTITY_SCOPED_COMMANDS` covers the `ext_sync_*` commands;
  the permission table maps `ext_sync_*` to `sync` and `sync_*` to
  `null`.

### 18.6 Mind Map (Kage-Extensions, vitest)

- `lib/syncmap.test.js`: state ↔ collection records, link ids, local
  fields stripped, groups.
- `sync.test.js`: two simulated computers on `test-helpers/mindmap/sync-host.mjs`
  (a fake `context.sync` with the same cell rules) converge on
  captures, status changes, entity merges, deletes, undo after a remote
  change (`superseded`), private items with the setting on and off,
  crash between write and push (`syncPending`).
- Works unchanged on a host without `sync.records`.
- `npm run check:all` passes.

### 18.7 Manual matrix (before release)

| Pair | Checks |
|---|---|
| iCloud Drive, Mac ↔ Mac | Optimize Storage eviction, an offline Mac, Keep Downloaded off |
| Dropbox, Mac ↔ Windows | Online-only files, Selective Sync leaving the folder out, a conflicted copy forced by cloning a VM |
| OneDrive, Windows ↔ Windows | Files On-Demand, pinning applied, "Free up space" |
| Google Drive, Mac ↔ Windows | Stream mode, Mirror mode |
| Syncthing, Linux ↔ Mac | Peers never online together for a day; "Receive Only" |
| Network share, Linux ↔ Windows | Watcher silent, poll only |

Each pair also: Kage killed during publish, a laptop asleep for a week,
the clock set an hour off, one computer on an older Kage, remove a
computer, start over, change folder. Startup time to first window is
unchanged within noise (existing startup log timestamps).

---

## 19. Interfaces

These are the stable contracts between work packages. Exact secondary
helpers may be added by a package inside its own files; nothing listed
here changes without updating this section.

### 19.1 Contracts commit (C0)

WP-S1 lands **C0** before anyone else branches. It:
- creates every Rust file named in §19.2–§19.5 with these signatures and
  bodies that return `sync_unavailable` errors or empty values (never
  `todo!()`), declares `sync` in `src/lib.rs` and `src/main.rs`, and the
  new `ErrorKind`s. For §19.3's OS files that is the cross-platform
  `src/os/cloud_files.rs` and `src/os/secret_store.rs`; WP-S2 adds the
  per-OS files behind them. `crypto::new_dek` already draws from the OS
  random generator, so no stub can hand out a zero key. `Cargo.toml`
  gains `zeroize` for the key types;
- registers every command of §19.5 in `src/commands/registry.rs`, with
  `null` entries in `ui/js/shared/extension-permissions.js`, so
  `command_error_type_parity_test` and `command_permissions_parity_test`
  pass from day one. The four `ext_sync_*` commands are `null` too:
  WP-S5 adds the `sync` capability (`VALID_CAPABILITIES`, `CAPABILITIES`,
  the icon), maps them to it and adds them to `IDENTITY_SCOPED_COMMANDS`,
  as agent calls and file text do for theirs. `get_config_snapshot` is a
  stub in `src/commands/sync.rs` that returns the config with revision 0;
  the `baseRevision` change to `save_config` stays with WP-S4;
- adds the §19.6 event names to `src/events.rs` and
  `ui/js/shared/events.js`;
- creates the new JS files of §19.7 with their exports (inert), and the
  three `HOST_FEATURE_SYNC_*` names, which aren't in
  `HOST_FEATURE_CAPABILITY`, so no window offers them. The `{ syncPath }`
  option of `base.js` and the two `config-cache.js` exports come with
  WP-S6;
- adds the EN keys of §17 to `locales/en/messages.json`. The other
  catalogs need a `scripts/translate.py` run, as after the three
  contracts commits before it.

After C0, each file belongs to the package that owns it in §20. Later
EN strings go through fragments `docs/design/sync-i18n/<wp>.en.json`,
which WP-S6 merges into the catalog and deletes. Exception: WP-S2's Rust
code returns `settings.sync.folder.reason.*` keys, which its tests check
against the EN catalog, so WP-S2 added those 12 keys to
`locales/en/messages.json` directly; the other catalogs need a
`scripts/translate.py` run.

### 19.2 Core (WP-S1): `src/sync/types.rs`, `hlc.rs`, `merge.rs`, `scope.rs`, `split.rs`, `canonical.rs`, `replica.rs`, `conflicts.rs`

```rust
// types.rs — the §7.2 types, plus:
pub const SYNC_FORMAT: u32 = 1;
pub const READER_VERSION: u32 = 1;
pub enum Category { Settings, Automations, Extensions, Chats, System }
pub enum Origin { User, Import, Reset, Join, Sync }
pub enum DefaultScope { Shared, PerOs }
pub enum ScopeAction { All, Os, UseShared, ThisComputer, StopThisComputer, AvailAll, AvailOs, AvailThisComputer }
pub enum ConflictKind { ConcurrentEdit, EditVsDelete, DeleteVsEdit, ReplacedOnJoin, RestoreBackup,
                        ReplacedByRestore, DeletedWhileAway }
pub struct Side { pub stamp: Stamp, pub wall_ms: i64, pub deleted: bool, pub value: Option<serde_json::Value> }
pub struct ConflictBody { pub ns: String, pub rid: String, pub field: Option<FieldKey>,
                          pub kind: ConflictKind, pub loser: Side, pub winner: Option<Side> }
pub enum PinReason { ThisComputer, HeldBack, Lossy, Consent, NeedsApproval, InvalidHere, NotInstalled, NewerSchema,
                     OriginMismatch }
pub struct LocalKey { pub ns: String, pub id: String, pub field: String }
pub struct Structure { pub whole: bool, pub together: Vec<Vec<String>>, pub sets: Vec<String>,
                       pub maps: Vec<String>, pub local: Vec<String>, pub per_os: Vec<String> }

// hlc.rs — §7.3
impl Hlc { pub fn new(last: u64) -> Self; pub fn tick(&mut self, wall_ms: i64, floor: u64) -> u64;
           pub fn observe(&mut self, remote: u64, wall_ms: i64); pub fn last(&self) -> u64; }

// merge.rs
pub struct MergeOutcome { pub changed: bool, pub conflicts: Vec<ConflictBody> }
pub fn merge_record(local: &mut Record, incoming: &Record) -> MergeOutcome;
pub fn is_visible(r: &Record) -> bool;
pub fn ancestors(base: Option<&Cell>, me: DeviceId) -> Vec<Stamp>;

// scope.rs
pub struct Effective<'a> { pub value: Option<&'a serde_json::Value>, pub from: Option<Scope>, pub stamp: Option<Stamp> }
pub fn effective<'a>(r: &'a Record, field: &str, os: OsKind, pin: Option<&'a serde_json::Value>) -> Effective<'a>;
pub fn edit_target(r: &Record, field: &str, os: OsKind, default: DefaultScope) -> Scope;
pub fn scope_writes(r: &Record, field: &str, os: OsKind, action: ScopeAction,
                    current: &serde_json::Value) -> Vec<(FieldKey, serde_json::Value)>; // Null = clear
pub fn exists_here(r: &Record, os: OsKind) -> bool;

// split.rs — record fields ↔ cell field names (§7.2)
pub fn split(fields: &serde_json::Map<String, serde_json::Value>, s: &Structure) -> BTreeMap<String, serde_json::Value>;
pub fn join<'a>(cells: impl Iterator<Item = (&'a str, &'a serde_json::Value)>, s: &Structure)
    -> serde_json::Map<String, serde_json::Value>;

// canonical.rs
pub fn canonical(v: &serde_json::Value) -> Vec<u8>;
pub fn hash(v: &serde_json::Value) -> [u8; 32];

// replica.rs
pub struct Replica { /* rusqlite::Connection */ }
impl Replica {
    pub fn open(path: &Path) -> anyhow::Result<(Self, OpenNote)>;   // OpenNote::Fresh | Existing | RebuiltFromCorrupt
    pub fn open_in_memory() -> anyhow::Result<Self>;
    pub fn tx<T>(&mut self, f: impl FnOnce(&mut Tx<'_>) -> anyhow::Result<T>) -> anyhow::Result<T>;
}
impl Tx<'_> {
    pub fn record(&self, ns: &str, id: &str) -> anyhow::Result<Option<Record>>;
    pub fn put_record(&mut self, r: &Record, bucket: u32) -> anyhow::Result<()>;
    pub fn records_in_bucket(&self, ns: &str, bucket: u32, of: u32) -> anyhow::Result<Vec<Record>>;
    pub fn mark_dirty(&mut self, ns: &str, bucket: u32, own: bool) -> anyhow::Result<()>;
    pub fn take_dirty(&mut self) -> anyhow::Result<Vec<(String, u32, bool)>>;
    pub fn projected(&self, k: &LocalKey) -> anyhow::Result<Option<(/*hash*/ [u8; 32], Option<Stamp>, Option<Scope>)>>;
    pub fn set_projected(&mut self, k: &LocalKey, v: Option<([u8; 32], Option<Stamp>, Option<Scope>)>) -> anyhow::Result<()>;
    pub fn pin(&self, k: &LocalKey) -> anyhow::Result<Option<(serde_json::Value, PinReason)>>;
    pub fn set_pin(&mut self, k: &LocalKey, v: Option<(serde_json::Value, PinReason)>) -> anyhow::Result<()>;
    pub fn applied(&self, shard: &str) -> anyhow::Result<bool>;
    pub fn add_applied(&mut self, shard: &str) -> anyhow::Result<()>;
    pub fn ack(&mut self, ns: &str, id: &str, dev: DeviceId) -> anyhow::Result<()>;
    pub fn meta(&self, k: &str) -> anyhow::Result<Option<String>>;
    pub fn set_meta(&mut self, k: &str, v: &str) -> anyhow::Result<()>;
    pub fn device(&self, dev: DeviceId) -> anyhow::Result<Option<DeviceRow>>;   // the §6.2 columns
    pub fn put_device(&mut self, row: &DeviceRow) -> anyhow::Result<()>;
    pub fn devices(&self) -> anyhow::Result<Vec<DeviceRow>>;
    // Tx also implements ExtStore (§19.4). `Projected` names the tuple of `projected`.
}

// conflicts.rs
pub fn conflict_id(name_key: &[u8; 32], body: &ConflictBody) -> String;
pub fn conflict_record(id: &str, body: &ConflictBody, state: &str, stamp: Stamp, wall_ms: i64) -> Record;
pub fn retention_pass(tx: &mut Tx<'_>, now_ms: i64, me: DeviceId, hlc: &mut Hlc) -> anyhow::Result<usize>;
```

Secondary helpers WP-S1 added in its own files, for the engine:
- `types.rs`: the text forms (`Display`/`FromStr` and serde) of
  `DeviceId`, `Stamp`, `Scope` and `FieldKey`; the shard forms of `Cell`,
  `Tomb` and `Record` (`Record::to_json`, `Record::from_json`); the JSON
  of `ConflictBody` and `Side` (§10.1); `DeviceId::random`,
  `OsKind::{ALL, as_str, from_name}`, `PinReason::{as_str, from_name}`,
  `Record::{new, cell, max_stamp, newest_cell_stamps}`.
- `hlc.rs`: `physical(wall_ms)`, `wall_ms_of(hlc)`, `MAX_AHEAD_MS`.
- `merge.rs`: `cmp_cells`, `cmp_tombs`, `MAX_ANCESTORS`.
- `split.rs`: `parse_name`, `is_union`, `absent_value` (the value capture
  writes for a cell name `split` no longer produces), `valid_field_name`,
  `element_cell`, `entry_cell`, and the names `WHOLE`, `AVAIL`, `ORDER`.
- `canonical.rs`: `same(a, b)`, equality of canonical forms.
- `replica.rs`, on `Tx`: `records(ns)`, `namespaces`, `count_records`,
  `bucket_of`, `delete_record`, `merge(incoming, bucket)` (merge, store,
  mark dirty as an echo), `write(record, bucket)` (store, mark dirty as
  an own edit), `mark_published`, `is_published`, `projected_in`,
  `pins(ns)`, `remove_applied`, `acks`, `clear_acks`, `delete_meta`,
  `remove_device`, `feed_clear`, `feed_seq`.
- `conflicts.rs`: `detected` (a merge's conflict as its record, at the
  loser's stamp, §10.1), `record_detected` (merge those into the replica in the
  caller's transaction), `record_action` (an entry from this computer's
  own action, freshly stamped), `resolve` (the §10.4 actions),
  `restore_record` (Recently deleted → Restore), `body_of`, `state_of`.
  A new record's bucket comes from a `BucketFn` the engine passes,
  `|ns, id| keys.bucket(ns, id, 1 << 16)`.

### 19.3 Folder, crypto and OS (WP-S2)

```rust
// src/sync/fsio.rs
pub struct Entry { pub name: String, pub len: u64, pub modified_ms: Option<i64>, pub is_dir: bool }
pub trait SyncFs: Send + Sync {
    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>>;
    fn read(&self, path: &Path, max_len: u64, timeout: Duration) -> io::Result<Vec<u8>>; // on the I/O pool
    fn write_once(&self, dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()>;      // ~kage-*.tmp + fsync + rename
    fn create_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;                // exclusive create (root files)
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn remove(&self, path: &Path) -> io::Result<()>;
    fn create_dir_all(&self, dir: &Path) -> io::Result<()>;
}
pub struct RealFs { /* I/O pool */ }
// src/sync/testing.rs (pub, #[doc(hidden)]): FaultyFs, FakeClock, MemoryKeyStore, FakeCloudFiles

// src/sync/envelope.rs
pub enum FileKind { Head, Shard, Blob }
pub struct Aad<'a> { pub folder_id: &'a str, pub device: DeviceId, pub kind: FileKind, pub name: &'a str }
pub enum OpenError { Truncated, BadTag, UnknownEpoch(u32), TooLarge, NameMismatch }
pub fn seal(keys: &Keys, epoch: u32, aad: &Aad<'_>, plain: &[u8]) -> Vec<u8>;
pub fn open(keys: &dyn Fn(u32) -> Option<Keys>, aad: &Aad<'_>, bytes: &[u8]) -> Result<Vec<u8>, OpenError>;

// src/sync/crypto.rs
pub struct Keys { /* enc, name, extdev */ }
impl Keys {
    pub fn derive(dek: &[u8; 32]) -> Self;
    pub fn content_name(&self, plain: &[u8]) -> String;          // 32 hex
    pub fn bucket(&self, ns: &str, id: &str, of: u32) -> u32;
    pub fn name_key(&self) -> &[u8; 32];
    pub fn ext_device_tag(&self, ext: &str, dev: DeviceId) -> String;
}
pub struct KeyFile { pub epoch: u32, pub kdf: Kdf, pub wrapped: String, pub aad: String, pub written_at: String }
pub struct Kdf { pub alg: String, pub m_kib: u32, pub t: u32, pub p: u32, pub salt: String }
pub enum CryptoError { WrongPassphrase, UnsupportedKdf, Malformed }
pub fn new_dek() -> Zeroizing<[u8; 32]>;
pub fn wrap_dek(passphrase: &str, dek: &[u8; 32], folder_id: &str, epoch: u32) -> anyhow::Result<KeyFile>;
pub fn unwrap_dek(passphrase: &str, kf: &KeyFile, folder_id: &str) -> Result<Zeroizing<[u8; 32]>, CryptoError>;
pub fn generate_passphrase() -> String;                          // 25 Crockford base32 chars in 5 groups

// src/sync/keystore.rs
pub enum KeyStoreKind { Keychain, CredentialManager, SecretService, File, Memory }
pub struct StoredKey { pub epoch: u32, pub dek: Zeroizing<[u8; 32]> }
pub trait KeyStore: Send + Sync {
    fn kind(&self) -> KeyStoreKind;
    fn get(&self, folder_id: &str) -> anyhow::Result<Option<StoredKey>>;
    fn set(&self, folder_id: &str, key: &StoredKey) -> anyhow::Result<()>;
    fn delete(&self, folder_id: &str) -> anyhow::Result<()>;
}
pub fn default_key_store(local_dir: &Path) -> Arc<dyn KeyStore>;  // §11.4 selection

// src/sync/folder.rs
pub struct Layout { pub root: PathBuf }   // <chosen>/Kage Sync
pub enum DirKind { Root, Keys, Device, Shards, Blobs }
pub enum FileClass { Head(u64), Data, KeyFile { epoch: u32, dev: String, n: u32 }, OwnTemp, ICloudStub(String),
                     Duplicate { of: String }, ToolTemp, OsJunk, Unknown }
pub fn classify(dir: DirKind, name: &str) -> FileClass;
pub struct Descriptor { pub kage_sync: u32, pub folder_id: String, pub created_at: String, pub min_reader: u32 }
pub fn read_descriptor(fs: &dyn SyncFs, l: &Layout) -> anyhow::Result<Option<Descriptor>>;
pub fn create_folder(fs: &dyn SyncFs, l: &Layout, d: &Descriptor, readme: &str, key: &KeyFile, me: DeviceId) -> anyhow::Result<()>;
pub fn list_key_files(fs: &dyn SyncFs, l: &Layout) -> anyhow::Result<Vec<(FileClass, KeyFile)>>;
pub fn inspect(fs: &dyn SyncFs, cloud: &dyn CloudFiles, chosen: &Path) -> FolderInfo;
pub enum FolderKind { New, Existing, UnknownSync, NotUsable, NewerFormat }
pub struct FolderInfo { pub kind: FolderKind, pub provider: Provider, pub computer_count: u32,
                        pub reason: Option<String> }   // `sync_inspect_folder`'s answer

// src/os/cloud_files.rs (+ macos/windows/linux)
pub enum Residency { Local, Placeholder, Unknown }
pub enum Provider { ICloud, Dropbox, OneDrive, GoogleDrive, Syncthing, FileProvider, Network, Other }
pub struct SuggestedFolder { pub provider: Provider, pub label: String, pub path: PathBuf }
pub trait CloudFiles: Send + Sync {
    fn residency(&self, path: &Path) -> Residency;            // never reads content
    fn request_download(&self, path: &Path);                  // returns at once
    fn pin_folder(&self, dir: &Path) -> anyhow::Result<()>;   // Windows Cloud Files; no-op elsewhere
    fn detect_provider(&self, path: &Path) -> Provider;
    fn suggest_folders(&self) -> Vec<SuggestedFolder>;
}
pub fn system() -> Arc<dyn CloudFiles>;

// src/os/secret_store.rs (+ macos/windows/linux)
pub fn get(service: &str, account: &str) -> anyhow::Result<Option<Zeroizing<Vec<u8>>>>;
pub fn set(service: &str, account: &str, secret: &[u8]) -> anyhow::Result<()>;
pub fn delete(service: &str, account: &str) -> anyhow::Result<()>;
pub fn keychain_is_reliable() -> bool;   // macOS: stable Developer ID signature
```

What WP-S2 built, beyond the signatures above:
- `SyncFs` semantics. `write_once`, `create_new` and `rename` fail with
  `AlreadyExists` rather than replace a file. `read` fails with
  `FileTooLarge` past `max_len` and with `TimedOut` past its timeout.
  `list` leaves out links and names that aren't UTF-8. `remove` takes a
  file or a whole directory. A link inside the folder fails the call
  (`fsio::is_link_refused`); an exhausted pool fails it at once
  (`fsio::is_unresponsive`); `fsio::is_sharing_violation` matches a
  Windows sharing violation and `FaultyFs`'s fake one. `RealFs::stats()`
  reports the pool's threads.
- `envelope`: `max_plain(kind)` and `max_sealed(kind)` (pass the latter
  to `read`), `peek_epoch(bytes)`, `seal_with_nonce` (golden files only).
- `crypto`: `random_id()` (a folder or device id), `device_text`,
  `parse_device_text`, `is_id_text` (WP-S1's text form of `DeviceId` may
  replace the first two), `normalize_passphrase`, and `wrap_dek_with` at
  a cheap `KdfCost` for tests.
- `keystore`: `key_store_for(kind, dir)`, `file_key_store(dir)`,
  `KeyStoreKind::wire()`/`from_wire()` for `DevicePrefs::key_store`,
  `StoredKey::new`, and `KeyStoreError` (`Denied`, `Unavailable`) inside
  the stores' errors.
- `folder`: `Layout::{in_chosen, resolve, descriptor, keys_dir,
  device_dir, shards_dir, blobs_dir, archive_dir, …}` (`resolve` takes the
  picked folder itself when it already holds `kage-sync.json`),
  `head_name`, `data_name`, `key_file_name`, `write_key_file` (the next
  key file of this computer, for Change passphrase), and `NotReady`
  (`is_not_ready`): `read_descriptor` returns it when a descriptor exists
  but isn't downloaded or readable yet, never `None`. With two
  descriptors it returns the §5.3 winner. `create_folder` writes the key
  file, the README and, last, the descriptor; if the descriptor exists,
  it removes its key file again and fails with `AlreadyExists`.
  `inspect_in` takes the forbidden `Places` so tests can use temp
  folders. `FolderInfo::reason` is `settings.sync.folder.reason.<why>`:
  `missing`, `not_a_folder`, `drive_root`, `kage_folder`, `temp`,
  `read_only`, `denied`, `unreadable`, `not_responding`, `inside_sync`,
  `not_downloaded`, `incomplete`. A new folder whose provider is `Other`
  is `unknown_sync`.
- `os::secret_store`: `SecretError` (`Denied`, `Unavailable`,
  `Isolated`), `backend()`, and `os_calls()` for the isolation test.
  `Provider::FileProvider` also covers other Windows Cloud Files apps.
- `testing`: `FaultyFs` is one computer's view of the folder; a simulated
  cloud copies between several with `put`, `get`, `delete`, `snapshot`
  and `restore`. Faults (`Fault::{NoSpace, Denied, ReadOnly, Sharing,
  Slow, Hang, Missing}`) aim at an `FsOp` under a path, always or for N
  calls. `Hang` fails with `TimedOut` at once, without waiting.
  `MemoryKeyStore::fail(KeyFault::{NotFound, Denied, Unavailable})`,
  `FakeCloudFiles::{set_residency, download_requests,
  complete_downloads, fail_pinning, …}`, `FakeClock::{advance,
  set_power}`.

### 19.4 Engine (WP-S3): `src/sync/mod.rs`, `engine.rs`, `ingest.rs`, `publish.rs`, `gc.rs`, `identity.rs`, `membership.rs`, `guards.rs`, `scheduler.rs`, `status.rs`, `prefs.rs`, `catalog.rs`, `sources.rs`, `sources/{config,lists,steering,chats}.rs`

```rust
// sources.rs — implemented by S3 (config, lists, steering, chats) and S5 (extensions)
pub struct NsSpec { pub ns: String, pub category: Category, pub schema: u32, pub echo: bool,
                    pub structure: Structure, pub default_scope: DefaultScope, pub large: bool,
                    pub label_field: Option<String> }
pub struct LocalRecord { pub fields: serde_json::Map<String, serde_json::Value>, pub avail: Option<String> }
pub struct LocalState { pub records: BTreeMap<(String, String), Option<LocalRecord>> } // None = absent
pub struct LocalWrite { pub ns: String, pub id: String, pub value: Option<LocalRecord> }
pub enum ProjectResult { Applied { read_back: LocalState }, Stale }
pub trait Source: Send {
    fn id(&self) -> &'static str;
    fn specs(&self) -> Vec<NsSpec>;
    fn read_local(&mut self) -> anyhow::Result<LocalState>;
    fn project(&mut self, writes: Vec<LocalWrite>, origin: Origin) -> anyhow::Result<ProjectResult>;
    /// Collections-mode calls from extensions. Default: not handled.
    fn request(&mut self, _ctx: &mut dyn CaptureCtx, _req: &ExtRequest) -> Option<Result<ExtReply, AppError>> { None }
}
pub enum CaptureResult { Unchanged, Written, Rejected(&'static str) }
pub trait CaptureCtx {
    fn capture(&mut self, ns: &str, id: &str, local: Option<&LocalRecord>, origin: Origin, initial_stamp_ms: Option<i64>)
        -> anyhow::Result<CaptureResult>;                                    // the §8.2 rules, base = ext_base for collections
    fn merged(&self, ns: &str, id: &str) -> anyhow::Result<Option<LocalRecord>>;   // effective, without local fields
    fn store(&mut self) -> &mut dyn ExtStore;   // feed, ext_base, ext_view, reads (implemented by S1's Tx)
    fn keys(&self) -> &Keys;
    fn device(&self) -> DeviceId;
}
// replica.rs (S1): the extension tables, as a trait so S5 needs no Tx lifetimes
pub trait ExtStore {
    fn feed_push(&mut self, ext: &str, coll: &str, id: &str) -> anyhow::Result<u64>;
    fn feed_page(&self, ext: &str, after: u64, max: usize) -> anyhow::Result<Vec<(u64, String, String)>>;
    fn feed_ack(&mut self, ext: &str, upto: u64) -> anyhow::Result<()>;
    fn ext_base(&self, ext: &str, coll: &str, id: &str) -> anyhow::Result<BTreeMap<String, (Stamp, [u8; 32])>>;
    fn set_ext_base(&mut self, ext: &str, coll: &str, id: &str, f: &BTreeMap<String, (Stamp, [u8; 32])>) -> anyhow::Result<()>;
    fn ext_view(&self, window: &str, ext: &str, key: &str) -> anyhow::Result<BTreeSet<String>>;
    fn set_ext_view(&mut self, window: &str, ext: &str, key: &str, ids: &BTreeSet<String>) -> anyhow::Result<()>;
    fn forget_window(&mut self, window: &str) -> anyhow::Result<()>;   // reads and ext_view rows
    fn read_stamp(&self, window: &str, ext: &str, key: &str) -> anyhow::Result<Option<Stamp>>;
    fn set_read_stamp(&mut self, window: &str, ext: &str, key: &str, s: Option<Stamp>) -> anyhow::Result<()>;
}

// Ports the app implements (WP-S4)
pub trait ConfigPort: Send + Sync {
    fn snapshot(&self) -> (u64, crate::config::Config);
    fn commit(&self, base_revision: u64, next: crate::config::Config, origin: Origin) -> Result<u64, CommitError>;
    fn load_was_reset(&self) -> LoadReset;   // None | All | Sections(Vec<String>) (§8.10)
    fn saves_suspended(&self) -> bool;
    fn installed(&self) -> Vec<crate::extensions::InstalledItem>;
}
pub enum CommitError { Stale, Suspended, Io(anyhow::Error) }
pub trait HostEvents: Send + Sync {
    fn status_changed(&self, s: &SyncStatus);
    fn conflicts_changed(&self, open: usize);
    fn applied(&self, keys: &[String]);
    fn extension_data_changed(&self, ext: &str, keys: &[String]);         // source: "sync"
    fn extension_sync_changes(&self, ext: &str, collections: &[String], reason: &str);
    fn notify_user(&self, state: &str);                                   // §13.8
    fn track(&self, event: &'static str, props: Vec<(&'static str, String)>);
}
pub trait Clock: Send + Sync { fn now_ms(&self) -> i64; fn power(&self) -> crate::os::power::PowerState; }

pub struct EngineEnv {
    pub local_dir: PathBuf, pub os: OsKind, pub kage_version: String, pub fingerprint: String,
    pub fs: Arc<dyn SyncFs>, pub cloud: Arc<dyn CloudFiles>, pub keys: Arc<dyn KeyStore>,
    pub clock: Arc<dyn Clock>, pub events: Arc<dyn HostEvents>, pub config: Arc<dyn ConfigPort>,
}
pub enum Wake { Startup, Local { source: &'static str, origin: Origin }, Folder, Poll, Focus, Resume, Manual, Exit }
pub struct TickReport { pub captured: usize, pub ingested_shards: usize, pub projected: usize,
                        pub published: bool, pub new_conflicts: usize }
pub struct SyncEngine { /* … */ }
impl SyncEngine {
    pub fn open(env: EngineEnv, sources: Vec<Box<dyn Source>>) -> anyhow::Result<Self>;
    pub fn tick(&mut self, wake: Wake) -> TickReport;
    pub fn flush_for_exit(&mut self, budget: Duration);
    pub fn handle(&mut self, req: EngineRequest) -> Result<EngineReply, AppError>;
    pub fn status(&self) -> SyncStatus;
}

// mod.rs / scheduler.rs
#[derive(Clone)] pub struct SyncHandle { /* Arc<OnceLock<channel to the kage-sync thread>> */ }
impl SyncHandle {
    pub fn pending() -> Self;                                // built on the Builder; every request → sync_unavailable until started
    pub fn start(&self, make: impl FnOnce() -> anyhow::Result<(EngineEnv, Vec<Box<dyn Source>>)> + Send + 'static,
                 delay: Duration);                           // called from setup(); a second call is a no-op
    pub fn request(&self, req: EngineRequest, timeout: Duration) -> Result<EngineReply, AppError>;
    pub fn wake(&self, w: Wake);                             // never blocks
    pub fn flush_for_exit(&self, budget: Duration);
}
pub fn install_global(h: SyncHandle);                        // OnceLock
pub fn notify(source: &'static str, origin: Origin);         // no-op until installed; used by hooks

pub enum EngineRequest {
    Status, SuggestFolders, InspectFolder(PathBuf), GeneratePassphrase,
    Create(CreateReq), Join(JoinReq), ConfirmJoin(JoinChoice), Unlock { passphrase: String },
    SetPaused { until: Option<String> }, SyncNow, SetCategory { category: Category, on: bool },
    SetExtensionData { ext: String, on: bool },
    GetScopes(Option<Vec<String>>), SetScope { path: String, action: ScopeAction },
    ListConflicts(ConflictTab), ConflictDetail(String), ResolveConflict { id: String, action: ConflictAction },
    DismissAll(Option<String>),
    ListReviews, ApproveRunnable(String), KeepOff(String), DismissSuggestion { kind: String, id: String },
    AnswerMassChange { ns: String, accept: bool },
    ListDevices, RenameDevice(String), RemoveDevice { id: String, lost: bool, new_passphrase: Option<String> },
    ChangePassphrase(String), Leave, StartOver { passphrase: String }, UndoJoin, PurgeExtension(String),
    FindFolder, ApplyFolder(PathBuf),
    Ext(ExtRequest),
}
pub enum ExtRequest {
    Status { ext: String },
    Push { ext: String, collection: String, records: Vec<serde_json::Value>, deleted: Vec<String>, initial: bool },
    Pull { ext: String, max: usize },
    Ack { ext: String, cursor: String },
}
pub enum EngineReply { Done, Status(SyncStatus), Folders(Vec<SuggestedFolder>), Folder(FolderInfo),
    Passphrase(String), JoinPreview(JoinPreview), Scopes(HashMap<String, ScopeView>), Scope(ScopeView),
    Conflicts(Vec<ConflictSummary>), ConflictDetail(ConflictDetail), Reviews(Reviews),
    Devices(Vec<DeviceView>), FoundFolder(Option<PathBuf>), Ext(ExtReply) }
pub enum ExtReply { Status(ExtSyncStatus), Push(PushResult), Pull(PullResult), Ack }
// Also in mod.rs: CreateReq / JoinReq { path, passphrase, device_name, categories }, JoinChoice,
// ConflictTab, ConflictAction, DestructiveAction (the §19.5 argument values), ExtSyncStatus,
// PushResult, RejectCode, PullResult (§12.6, camelCase where JavaScript reads them), and the
// error helpers unavailable(), needs_confirmation(), quota_exceeded().
```

The command answers live in `src/sync/status.rs`: `SyncStatus` (with
`SyncState`), `DeviceView`, `Problem`, `ScopeView`, `ConflictSummary`,
`ConflictDetail` (the summary's fields plus `fields: [{ field, now,
replaced }]` for Compare), `Reviews` (five lists of `ReviewItem { kind,
id, label_key, label_params, computer, detail, count }`, where `id` is
what the matching command takes) and `JoinPreview`.

`SyncStatus` (serialised for the frontend):

```json
{
  "state": "off|unavailable|setting_up|up_to_date|syncing|waiting|placeholders|paused|locked|folder_missing|folder_denied|folder_empty|reset|removed|update_required|disk_full|write_failed|folder_reverting|other_engine",
  "folder": { "display": "Dropbox › Kage Sync", "provider": "dropbox" },
  "last_sync_at": 1762686000000, "last_publish_at": 1762685990000, "paused_until": null,
  "pending_local": 0, "waiting_files": 3,
  "categories": { "settings": true, "automations": true, "extensions": true, "chats": false },
  "devices": [{ "id": "k7m2…", "name": "Work laptop", "os": "macos", "kage_version": "0.12.x",
                "last_seen_at": 1762686000000, "this": true, "state": "ok|stale|inactive|needs_update|retired" }],
  "conflicts_open": 2, "held": 2, "reviews": 1, "suggestions": 2,
  "problems": [{ "kind": "clock_warning", "key": "sync.problem.clock_warning", "params": { "computer": "Home Mac", "minutes": "42" } }],
  "key_store": "keychain", "undo_join_until": "2026-12-02T10:00:00Z", "used_bytes": 15728640
}
```

What WP-S3 built, beyond the signatures above:
- **Source conventions** (`sources.rs`), so the engine stays generic:
  - a list reports each item's position as a number in the field
    `order`; a write to a list namespace carries every record that exists
    here, `order` as the fractional key, and the source rebuilds the whole
    list in `(order, id)` order, this computer's own items where they
    were;
  - `LocalRecord::avail` is the default `avail` of a *new* item;
  - a field may hold `sources::unchanged()` ("the value you last saw"),
    which the engine skips (chat transcripts aren't loaded every pass);
  - `Structure::per_os` may name a record as `@<id>`: that record's
    fields default to per OS (the `cfg` namespace's `PerOs` rows);
  - every `read_local` result is captured before anything is written over
    it.
- `sources::host_sources(port, HostDirs { config_kage_dir, sync_dir }, os,
  chats)` builds WP-S3's four sources for the app. `sources::chats` has a
  `ChatHistory` trait and `RegistryHistory` over
  `AgentSessionRegistry`; mirrors are `chats/<device>/<key>.json` and
  `.jsonl` (`key` is a hash of the record id), read by
  `agent_sessions::synced::SyncedChatsProvider` (id `synced`).
- `SyncEngine::{note_wake, next_deadline, now_ms, watch_dir}` for the
  thread, `set_kdf_cost` for tests, and `engine::kill::{arm, disarm}`:
  the §18.3 kill points (inert unless a test arms one).
- `SyncHandle::started()`; `sync::sync_available()` and `sync::RELEASED`
  (the release gate: debug builds and `+dev.` versions, until the owner
  sets `RELEASED`). `SyncHandle::start` does nothing where sync isn't
  available.
- Values: an explicit `null` cell at this OS or at `all` means "no value"
  (the field is cleared here); only when no cell exists at either does the
  local value stay (R4). A field written as absent that reads back as its
  type's default isn't lossy.
- List order: capture compares the local order with the order the file
  last showed (meta `order.<ns>`). A local order that still matches it is
  stale, not a reorder, so newer keys from elsewhere aren't overwritten.
- "Recently deleted" rows (`sync_list_conflicts("deleted")`) have ids
  `d|<ns>|<id>` and kind `edit_vs_delete`; `sync_resolve_conflict` with
  `restore` brings the record back.
- Removing an extension's data writes one record per extension,
  `sync.ns_state/ext.<id>` with a `purged` cell, covering all of its
  `ext.<id>.*` namespaces.
- Status problems the engine adds: `sync.problem.device_collision`,
  `.held_back`, `.not_this_version`, `.churn`, `.duplicates` and
  `.config_reset`, besides `clock_warning` and `unreadable`.

Decisions WP-S3 made where the design was open or didn't fit the code:
- **Extension on/off is WP-S3's.** `extension_states.<id>` is a `cfg`
  record, so the config source owns it, with the consent guard and the
  `not_installed` pins of §8.2 (I24 is WP-S3's). WP-S5 keeps `installs`,
  extension settings (`cfg.ext`) and the data modes.
- **Join backup.** The engine never reads app folders, so instead of a
  `config_export` backup it keeps every syncable local value the sources
  report (their `LocalState`) in `sync/trash/join-<time>.json`
  (owner-only). **Undo joining** writes it back through the sources, then
  leaves.
- **A join previews what has arrived.** `sync_join` reads the folder
  once and answers; the status says what is still downloading. Only a
  rebuild of a lost replica (an automatic join) waits up to 2 minutes.
- **Lost or stolen.** The computer removed with a key rotation can't read
  the revocation, which is in the new epoch: it shows `locked`, not
  `removed`.
- **The losing creator** of two folders made at once sees `reset` and
  joins the winner, as after a Start over elsewhere.
- **A bulk delete answered "keep"** brings the items back as new edits
  (for config lists that is also "Cancel"); "Only here" for an
  extension's data is `sync_set_extension_data(off)`.
- **Validation** uses `config_export::allowlist::{shape_of, accepts}`
  (`check` stays private). A ⌘Space-style system shortcut clash isn't
  reported by the engine; the hotkey picker (WP-S6) already checks it.
- **Dataless reads on macOS.** The engine thread never reads folder files
  itself (every read goes through `RealFs`'s pool with a timeout), so it
  doesn't set `IOPOL_MATERIALIZE_DATALESS_FILES_OFF`; that call would
  belong in `os::cloud_files` (WP-S2) if wanted.
- **Chats** publish once idle for 10 minutes; a chat still active at
  exit publishes at the next start.
- **I5** (a stale Settings save) is the `save_config` three-way merge of
  WP-S4 and the field-level save of WP-S6; the engine only sees the saved
  file, so I5 is tested there.
- **Strings.** WP-S3's EN strings went straight into
  `locales/en/messages.json` (`errors.sync.*`, `sync.problem.*`,
  `sync.label.*`, `sync.conflicts.label.*`, `sync.review.*`,
  `sync.readme`); the other catalogs need a `scripts/translate.py` run.

What WP-S5 added to these interfaces. The ones above couldn't carry the
per-window bases of §12.4 and §12.5, the `ext_base` of §12.6, or the
"not here" rules of §8.2 for extensions, so the extension source gets a
capture context of its own before every capture:

```rust
// sources.rs
trait Source {
    // …
    /// Runs in one transaction right before every `read_local` the engine
    /// captures. Default: nothing.
    fn prepare(&mut self, ctx: &mut dyn CaptureCtx, origin: Origin) -> anyhow::Result<Prepared> { … }
}
pub struct Prepared { pub written: usize, pub project: BTreeSet<(String, String)>, pub specs_changed: bool }
// CaptureCtx gains: record, records, projected, projected_field, set_projected, pin, set_pin
// (field `*` pins a whole record), meta, set_meta, write_cell (a cell at `all`, stamped now),
// devices, prefs, os, now, paused, last_sync_at, and the notes data_changed(ext, key) and
// sync_changes(ext, collection, reason), which the engine emits once the transaction commits.
pub mod extensions;   // ExtensionsSource, ExtensionHost, Journal, journal(), store_origin
```

- **Engine.** `capture_dirty` and `project_source` call `prepare`
  first. `prepare` returning `specs_changed` makes the engine ask every
  source for its specs again (`refresh_specs`, also run before each
  projection and for a push to an unknown collection) and call
  `prepare` once more, so an extension installed or granted since the
  last tick gets its namespaces. `(ns, "")` in `Prepared::project` is
  the whole namespace.
- **Collections mode** (`ext.<id>.c.<coll>`, `engine::collection_of`)
  is never captured from `read_local` and never projected into files: a
  changed record goes to the extension's feed and
  `extension_sync_changes(ext, colls, "remote")` (§8.6). A collection
  whose `schema.<ns>` is newer than this build's manifest isn't fed.
  `ExtCtx::capture` looks up a record's `projected` rows field by field
  (no namespace scan), and its tombstones count for the outgoing
  mass-change guard. The extension source keeps the base in `ext_base`
  and moves it into `projected` for the capture and back.
- **A config save wakes the extension and lists sources too**: the
  `config` hook dirties `lists` and `extensions` as well, because both
  read `config.json`.
- **`sync_set_extension_data`** also records the user's own switch
  (meta `extsync.on.<id>`, which a sideloaded extension needs), clears
  `needsInitialPush`'s meta, and on `on` emits
  `extension_sync_changes(ext, [], "enabled")`.
- **Status.** `SyncStatus::suggestions` and `sync_list_reviews`'
  `suggestions` and `removals` come from the `installs` records
  (`sources::extensions::install_reviews`).
- **Wiring for WP-S4.** `sources::host_sources` stays WP-S3's four; the
  app appends
  `sources::extensions::ExtensionsSource::new(port, <config>/kage/extension-data, Some(host))`,
  where `host` is `commands::extensions::AppExtensionHost { app }` (store
  themes, origin backfill). `scheduler::global_handle()` is how the
  `ext_sync_*` commands reach the engine: WP-S4 must call
  `install_global`. The window-destroyed hook in `main.rs` calls
  `journal().forget_window(label)`.

### 19.5 Tauri commands

Every command returns `Result<T, AppError>`, is registered in
`commands/registry.rs`, and runs on the engine thread through
`SyncHandle::request` (timeout 10 s; 60 s for setup, join and start
over). With sync off or not built yet, every command except
`sync_get_status`, `sync_suggest_folders`, `sync_inspect_folder` and
`sync_generate_passphrase` returns `sync_unavailable` and touches
nothing. Destructive commands take a `confirm` token from
`sync_prepare_destructive(action)` (one use, 60 s), so the mock-app sweep
and stray calls can't do damage.

Owned by WP-S4 (`src/commands/sync.rs`):

| Command | Args | Returns |
|---|---|---|
| `sync_get_status` | | `SyncStatus` |
| `sync_suggest_folders` | | `[{ provider, label, path }]` |
| `sync_inspect_folder` | `path` | `{ kind: "new" \| "existing" \| "unknown_sync" \| "not_usable" \| "newer_format", provider, computer_count, reason? }` (names are encrypted; `sync_join` returns them) |
| `sync_generate_passphrase` | | `String` |
| `sync_create` | `path, passphrase, deviceName, categories` | `SyncStatus` |
| `sync_join` | `path, passphrase, deviceName, categories` | `JoinPreview { computers: [{ name, os, last_seen_at }], same, different, only_here, only_synced }` (counts per category) |
| `sync_confirm_join` | `choice: "theirs" \| "mine"` | `SyncStatus` |
| `sync_unlock` | `passphrase` | `SyncStatus` |
| `sync_set_paused` | `until: Option<String>` (`"forever"` allowed) | `()` |
| `sync_now` | | `()` |
| `sync_set_category` | `category, on` | `()` |
| `sync_set_extension_data` | `extensionId, on` | `()` |
| `sync_get_scopes` | `paths: Option<Vec<String>>` | `HashMap<String, ScopeView>` |
| `sync_set_scope` | `path, action` | `ScopeView` |
| `sync_list_conflicts` | `tab: "replaced" \| "deleted" \| "resolved"` | `[ConflictSummary]` |
| `sync_conflict_detail` | `id` | `ConflictDetail` |
| `sync_resolve_conflict` | `id, action: "restore" \| "dismiss" \| "delete_anyway"` | `()` |
| `sync_dismiss_all` | `group: Option<String>` | `()` |
| `sync_list_reviews` | | `{ runnables, suggestions, removals, mass_changes, approvals }` |
| `sync_approve_runnable`, `sync_keep_off` | `key` | `()` |
| `sync_dismiss_suggestion` | `kind, id` | `()` |
| `sync_answer_mass_change` | `ns, accept` | `()` |
| `sync_list_devices` | | `[DeviceView]` |
| `sync_rename_device` | `name` | `()` |
| `sync_prepare_destructive` | `action: "leave" \| "start_over" \| "remove_device" \| "purge_extension" \| "undo_join"` | `String` token |
| `sync_remove_device` | `deviceId, lost, newPassphrase?, confirm` | `()` |
| `sync_change_passphrase` | `newPassphrase` | `()` |
| `sync_leave` | `confirm` | `()` |
| `sync_start_over` | `passphrase, confirm` | `SyncStatus` |
| `sync_undo_join` | `confirm` | `()` |
| `sync_purge_extension` | `extensionId, confirm` | `()` |
| `sync_find_folder` | | `Option<String>` path |
| `get_config_snapshot` | | `{ revision: u64, config: Config }` |
| `save_config` (changed) | `config, baseRevision?: u64` | `{ revision: u64 }` (callers that ignore it keep working) |

`ScopeView`:

```json
{ "synced": true, "rule": "shared|per_os|this_computer|never", "applies": "all|os|this_computer|not_synced",
  "reason_key": null, "held": null, "values": { "all": 14, "macos": 16 }, "others": [{ "os": "windows", "preview": "14 px" }] }
```

`ConflictSummary`: `{ id, kind, category, label_key, label_params,
loser: { computer, at, deleted, preview }, winner: { computer, at },
state, group }`. Previews are text, at most 200 characters, built at
read time.

Owned by WP-S5 (`src/commands/extensions/sync.rs`), capability `sync`,
`extensionId` force-injected by the sandbox host:

| Command | Args | Returns |
|---|---|---|
| `ext_sync_status` | `extensionId` | §12.6 `status()` |
| `ext_sync_push` | `extensionId, collection, records, deleted, initial` | `{ accepted, unchanged, rejected, off }` |
| `ext_sync_pull` | `extensionId, max` | `{ cursor, more, changes }` |
| `ext_sync_ack` | `extensionId, cursor` | `()` |

### 19.6 Events (`src/events.rs`, `ui/js/shared/events.js`)

| Event | Payload | Listeners |
|---|---|---|
| `sync_status_changed` | `SyncStatus` | Settings → Sync, tray, the Settings sidebar badge |
| `sync_conflicts_changed` | `{ open }` | Sidebar badge |
| `sync_applied` | `{ keys: [catalog paths] }`, emitted before `config_updated` | Settings manager (§9.4) |
| `extension_sync_changes` | `{ extension_id, collections, reason }` | Sandbox hosts → `sync-changes` port message |
| `extension_data_changed` (from EXTENSION_VIEWS) | `{ extension_id, changes: [{ key, op, source: "sync" }] }` | Sandbox hosts → `context.storage.onChanged` |
| `config_updated` (existing) | unchanged | every window |

### 19.7 JavaScript exports

| Module | Owner | Exports |
|---|---|---|
| `ui/js/shared/config-cache.js` | S6 | `getConfigSnapshot(): Promise<{ revision, config }>`; `saveConfig(config, { baseRevision }): Promise<void>` |
| `ui/js/shared/sync-scope-chip.js` | S6 | `attachScopeChips(root, { invoke, listen, t, os })`; `chipState(scopeView, os)` and `menuItems(scopeView, os)` (pure) |
| `ui/js/settings/base.js` | S6 | `createCheckboxRow(label, description, checkboxId, checked, { syncPath } = {})`; `createControlRow(label, description, controlHtml, { syncPath } = {})` |
| `ui/js/settings/sync.js` | S6 | `class SyncSettingsModule extends SettingsModule` (`id = 'sync'`) |
| `ui/js/settings/sync-ext-chip.js` | S6 | `renderSyncChip(extensionId, { invoke, t })` → HTML string for the extension page header (§12.3, §23.3) |
| `ui/js/shared/extension-sandbox-host.js` | S5 | `HOST_FEATURE_SYNC_KEYS = 'sync.keys'`, `HOST_FEATURE_SYNC_LISTS = 'sync.lists'`, `HOST_FEATURE_SYNC_RECORDS = 'sync.records'` (each requiring `sync`) |
| `ui/js/extension-sandbox/runtime/sync-api.js` (new, self-contained) | S5 | `createSyncApi({ invoke, onPortMessage })` → the `context.sync` object of §12.6 |
| `ui/js/shared/extension-permissions.js` | S5 | `CAPABILITIES.sync`; `COMMAND_CAPABILITIES` entries |

---

## 20. Work packages

Seven packages: six in this repo, one in Kage-Extensions. Each owns its
files; no file is in two of them. Files that packages of the other three
designs also list follow §23: C0 is the last of the four contracts
commits, and for a shared file this design's package merges after the
others' when both are in flight. Each builds and tests in its own git
worktree against §19, with fakes for what it doesn't own
(`src/sync/testing.rs`, mocked commands in vitest, the fake
`context.sync` host). WP-S1 lands the contracts commit C0 first, and
everyone branches from it.

| WP | Title | Repo | Codes against |
|---|---|---|---|
| S1 | Contracts and merge core | Kage | nothing |
| S2 | Folder, crypto, key store and cloud files | Kage | C0 |
| S3 | Engine and host sources | Kage | C0; S1 and S2 signatures (§19.2, §19.3) |
| S4 | App wiring and commands | Kage | C0; S3 signatures (§19.4) |
| S5 | Extension sync | Kage | C0; S3 `Source`, `CaptureCtx`, `ExtRequest`; S1 `ExtStore` |
| S6 | Settings → Sync, scope controls, safe saves | Kage | C0; §19.5 commands (mocked) |
| S7 | Mind Map sync participation | Kage-Extensions | §12.2 manifest and §12.6 `context.sync` (fake host) |

### WP-S1 Contracts and merge core

- **Files (owned):**
  - C0 (§19.1), touching once: `src/lib.rs`, `src/main.rs`,
    `src/error.rs`, `src/events.rs`, `ui/js/shared/events.js`,
    `src/commands/registry.rs`, `src/commands/mod.rs`,
    `src/commands/extensions/mod.rs`, `src/os/mod.rs`, `Cargo.toml`,
    `ui/js/shared/extension-permissions.js`,
    `ui/js/shared/extension-sandbox-host.js` (the three host feature
    names), `locales/en/messages.json`,
    `ui-tests/shared/extension-permissions.test.js`,
    `ui-tests/shared/extension-rust-alignment.test.js`, and the skeleton
    of every file in §19.2–§19.7;
  - afterwards: `src/lib.rs` (the `sync` module line), `src/sync/types.rs`,
    `src/sync/hlc.rs`, `src/sync/merge.rs`, `src/sync/scope.rs`,
    `src/sync/split.rs`, `src/sync/canonical.rs`, `src/sync/replica.rs`,
    `src/sync/conflicts.rs`.
- **Depends on:** nothing.
- **Done when:**
  - C0: `cargo check` passes; `cargo test --test command_error_type_parity_test`
    and `--test command_permissions_parity_test` pass;
    `python scripts/check_i18n.py` passes.
  - `cargo test --lib sync::` passes: the §18.1 rows for `hlc`, `merge`
    (including the property tests at 10,000 cases), `scope`, `split` and
    `conflicts`.
  - `replica.rs`: schema of §6.2 with its own `user_version` migrations;
    every multi-row change is one transaction; a corrupt file is moved
    aside and reported as `RebuiltFromCorrupt`; 10,000 records merged in
    under 1 second (benchmark test, ignored in debug CI).
  - No Tauri, no `crate::paths`, no clock except passed-in values.
  - `cargo clippy` and `cargo fmt --check` are clean.

### WP-S2 Folder, crypto, key store and cloud files

- **Files (owned):** `src/sync/fsio.rs`, `src/sync/testing.rs`,
  `src/sync/envelope.rs`, `src/sync/crypto.rs`, `src/sync/keystore.rs`,
  `src/sync/folder.rs`, `src/os/secret_store.rs`, `src/os/cloud_files.rs`,
  `src/os/{macos,windows,linux}/secret_store.rs`,
  `src/os/{macos,windows,linux}/cloud_files.rs`, the declarations in
  `src/os/mod.rs` and `src/os/{macos,windows,linux}/mod.rs`, `Cargo.toml`
  (`hkdf`, `hmac`, `zeroize`, the key store crate or platform bindings, the
  `windows` features for Cloud Files), `tests/fixtures/sync/envelope/`,
  `docs/OS_ABSTRACTION_GUIDE.md` (new sections), `docs/SECURITY_MODEL.md`
  (new "Folder sync" section).
- **Depends on:** C0 (`DeviceId`, `Stamp`).
- **Done when:**
  - The envelope is exactly §5.4. Golden files for version 1 are checked
    in. Every §18.1 `crypto` and `envelope` test passes.
  - Key store selection is §11.4. A test asserts that no OS key store
    is called when `paths::is_isolated()`. Keys are zeroized.
  - `classify` passes every pattern of §3 and §5.2.
  - `residency` never reads content: macOS stubs and `SF_DATALESS`,
    Windows recall attributes (unit fixtures, plus the manual matrix).
    `pin_folder` sets the pinned state on Windows; a failure is tolerated.
  - `RealFs::read` on a hanging file returns after its timeout; the pool
    never grows past its cap.
  - `write_once` uses `~kage-*.tmp`, fsyncs, retries Windows sharing
    violations, never follows links.
  - No platform module is imported from app code.
  - `cargo clippy` and `cargo fmt --check` are clean.

### WP-S3 Engine and host sources

- **Files (owned):** `src/sync/mod.rs`, `src/sync/engine.rs`,
  `src/sync/ingest.rs`, `src/sync/publish.rs`, `src/sync/gc.rs`,
  `src/sync/identity.rs`, `src/sync/membership.rs`, `src/sync/guards.rs`,
  `src/sync/scheduler.rs`, `src/sync/status.rs`, `src/sync/prefs.rs`,
  `src/sync/catalog.rs`, `src/sync/sources.rs`,
  `src/sync/sources/{config,lists,steering,chats}.rs`;
  `src/config.rs` (`ensure_item_ids`, the `sync::notify` hook after a
  successful save, the `load_was_reset` flag), `src/config/automations.rs`,
  `src/config/updates.rs`, `src/context_rules.rs` (`sync_id` fields),
  `src/config/permissions.rs` (`ExtensionGrant.origin`,
  `#[serde(default)]`, §4.5),
  `src/config_export.rs` (only: `allowlist` and `consent` become
  `pub(crate)`), `src/config_export/allowlist.rs` (rows for the new
  fields; `pub(crate)` on `check`, `Shape`, `Range`, `hotkey_registers`),
  `src/steering_io.rs` and `src/auto_steering.rs` (notify after writes),
  `src/agent_sessions/synced.rs` and `src/agent_sessions/mod.rs` (the
  read-only provider for mirrored chats), `CLAUDE.md` (the catalog line);
  tests: `tests/sync_support/`, `tests/sync_two_computers_test.rs`,
  `tests/sync_versions_test.rs`, `tests/sync_lifecycle_test.rs`,
  `tests/sync_faults_test.rs`, `tests/sync_chaos_test.rs`,
  `tests/fixtures/sync/` (except `envelope/`).
- **Depends on:** S1 and S2 by signature. Its unit tests use
  `Replica::open_in_memory` and `testing.rs`; its integration tests need
  S1 and S2 merged. `ConfigPort` and `HostEvents` are faked.
- **Done when:**
  - §8 is implemented; §4.3 and §4.4 are implemented exactly, and the
    §4.7 tests pass.
  - Integration scenarios I1–I24 (except I5, which is WP-S4's and WP-S6's,
    §19.4) and I27–I30 pass; §18.3 fault injection passes; the chaos test
    passes 100 seeds in CI mode. The harness runs in under 60 seconds with
    `jobs=2`.
  - The engine never does folder I/O on the caller's thread;
    `SyncHandle::wake` never blocks.
  - Existing config, allowlist, backup and `config_serde_default_parity_test`
    tests pass unchanged. A backup never contains `sync/` (test).
  - `tests/user_dirs_isolation_test.rs` passes (no `dirs::` in `src/sync`).

### WP-S4 App wiring and commands

- **Files (owned):** `src/commands/sync.rs`, `src/commands/mod.rs`,
  `src/commands/registry.rs`, `src/state.rs` (the `sync: SyncHandle` field and
  the config revision ring in `FeatureServices`), `src/main.rs`,
  `src/main/app_setup.rs`, `src/main/run_events.rs` (exit flush),
  `src/setup.rs` (start the engine 10 s after setup; focus wakes),
  `src/commands/system/lifecycle.rs` (flush in `graceful_shutdown`),
  `src/commands/system/config_io.rs` (`get_config_snapshot`, the
  `base_revision` three-way merge, `Config::ensure_item_ids` on save,
  `commit_config_change`, the import hook with `Origin::Import`),
  `src/sync_app.rs` (new: `TauriConfigPort`,
  `TauriHostEvents`, the real `Clock`, the fingerprint), `src/events.rs`,
  `ui/js/shared/events.js`, `src/error.rs`, `src/tray.rs`,
  `ui/js/shared/telemetry.js` (`KNOWN_EVENTS`), `docs/PRIVACY.md`,
  `tests/sync_app_test.rs` (new).
- **Depends on:** S3 (`SyncHandle`, `EngineRequest`, the ports).
- **Done when:**
  - Every §19.5 command in `src/commands/sync.rs` exists, returns
    `Result<_, AppError>` and is registered; both parity tests pass;
    `state_manage_parity_test` passes.
  - The `mock_app_test` sweep passes with sync off: no key store call, and
    nothing written outside the isolation root. Destructive commands
    without a token return `needs_confirmation`.
  - No folder or database I/O before 10 s after setup. Time to first
    window is unchanged within noise (startup log timestamps).
  - The exit flush respects 1.5 s, and 500 ms on the updater path,
    before `release_kill_on_exit_job`.
  - `save_config` three-way merge: a stale whole-config save doesn't
    revert a field another window or sync changed (test).
  - Telemetry props match §16 exactly (a unit test inspects the call
    sites). Tray status line and **Sync now** work.
  - Gate: `sync_available()` is true in debug builds and on the `dev`
    channel only, until the owner flips it (one line).

### WP-S5 Extension sync

- **Files (owned):** `src/sync/sources/extensions.rs` (installs,
  settings per §12.1, keys, lists and collections modes, quotas,
  store-theme auto-install; on/off and its consent guard are WP-S3's
  `cfg` records, §19.4), `src/extensions.rs` (manifest `sync`
  field, `normalize_sync`, `config.<field>.sync`),
  `src/commands/extensions/sync.rs`, `src/commands/extensions/mod.rs`,
  `src/commands/extensions/files.rs` (notify; the per-key lock, which
  `save_extension_data` doesn't take today; the `reads` and `ext_view`
  bases by window label), `src/commands/extensions/install.rs` (uninstall
  notify; theme install from `installs`; `origin` in
  `commit_extension_install`), `src/commands/extensions/store.rs` (pass
  the store origin to the install), `src/commands/extensions/discovery.rs`
  (the per-window read in `get_extension_config` and the three-way rule
  in `save_extension_config`, §9.5),
  `ui/js/shared/extension-permissions.js` (capability, command entries,
  and its own `save_config` call moved to `baseRevision`),
  `ui/js/shared/extension-sandbox-host.js`,
  `ui/js/shared/extension-manager.js` (offer the sync host features),
  `ui/js/extension-sandbox/runtime.js`,
  `ui/js/extension-sandbox/runtime/host-api.js`,
  `ui/js/extension-sandbox/runtime/sync-api.js` (new),
  `ui/js/settings/extension-capabilities.js` (label and icon),
  `docs/EXTENSIONS.md` (a "Sync" section and the multi-instance note),
  `ui-tests/extension-sandbox/sync-api.test.js`,
  `tests/sync_extensions_test.rs` (new).
- **Depends on:** C0 (`Source`, `CaptureCtx`, `ExtRequest`, `ExtStore`);
  S3 merged for end to end; EXTENSION_VIEWS WP-V3 and WP-V4 (storage change
  events) merged first.
- **Done when:**
  - §12 holds exactly: manifest validation, limits, error codes, base
    tracking, coalesced feed, at-least-once delivery, `initial` stamps,
    schema gating, purge.
  - I25, I26, I31 and I32 pass; a different-origin install of the same
    id gets nothing (`origin_mismatch`). Sensitive keys, sensitive settings and
    credential-named record fields are never published (tests against
    `Rules`).
  - JS tests: capability gate, identity injection, `context.sync` absent
    without the feature or the grant.
  - An extension update that adds `sync` needs approval again.
  - `cargo clippy`, `cargo fmt --check` and the vitest files pass.
- **What WP-S5 built** (`src/sync/sources/extensions.rs`; the engine
  hooks are in §19.4):
  - **Records.** `installs/<kind>:<id>`, `cfg.ext/<id>/<field>`,
    `ext.<id>.k/<key>` and `ext.<id>.l.<key>/<element id>` (plus
    `~rest`, the list file's other fields) each hold one field `v`.
    Collections use the manifest's structure.
  - **What syncs here** is decided in `prepare`, per extension: a grant
    with a known origin, the origin cell agreeing (§12.3), the user's
    switch for a sideload; then for data the `sync` and `storage` grant,
    the extension switched on, its data switch on. Records that don't
    apply here (not installed, another origin, a setting the local
    manifest doesn't let sync, a key it doesn't list) get a whole-record
    pin (`not_installed`, `origin_mismatch`, `this_computer`), so the
    engine neither captures nor projects them. Lifting one projects the
    record. An extension that stops syncing here has its keys and lists
    `projected` rows cleared, so a reinstall starts from the synced
    values, not from deletes. One that starts syncing (install, grant,
    switch) projects its keys and lists, puts every record of its
    collections in the feed with its base cleared, and needs an initial
    push.
  - **The journal** (`Journal`, one per app, `journal()`): the storage
    commands note which window loaded and saved which synced key, under
    the key's lock (`extensions::data_key_lock`, which projections take
    too). `prepare` turns loads into `reads` and `ext_view` rows and
    captures each save with the saving window's base. A keys-mode window
    that saved exactly what it read gets the newer value back in the
    file. A lists file that lost elements without a save through Kage
    (a restart in between, an edit by hand) gets them back too.
  - **Origins.** `store_install` and an update note `store:<hash>`,
    `install_extension_from_path` `sideload`; `commit_extension_install`
    writes it into the grant. Grants without one (made earlier, or by
    the welcome screen) are resolved by `ExtensionHost::resolve_origins`
    (`store::backfill_grant_origins`: the first store whose catalog lists
    the id, else `sideload`); until then the extension doesn't sync.
  - **Store themes** install through `ExtensionHost::install_store_theme`
    (`install::install_synced_theme`) when the theme's store is one this
    computer uses and the user hasn't excluded it.
  - **Settings saves** (`save_extension_config`) are merged three ways
    against the copy the window last got (`merge_extension_settings`,
    §9.5).
- **Decisions WP-S5 made** where the design was open or didn't fit:
  - **Schema gating is per collection**, through the engine's
    `schema.<ns>` (§8.11), not per record: a newer schema makes the
    whole collection read-only on an older extension (pull skips it,
    push answers `newer_schema`).
  - A pushed field whose name a cell can't carry, or a `sets`/`maps`
    field of the wrong type, is refused as `bad_collection` (`split`
    would drop it otherwise).
  - `sync.description` is checked against 140 characters as written;
    the install prompt checks the resolved text.
  - Elements a projection adds to a lists file go at the end in id
    order within one batch (`project` sees no stamps); elements written
    back after a stale save go in stamp order.
  - An initial push that loses to a newer copy in the folder leaves the
    extension's base at its own value, so pull delivers the winner and a
    later edit made before that pull is a conflict.
  - `needsInitialPush` ends with the first ack after it was set.
  - A collection delete with no base kept (a rebuilt replica) deletes
    the current record.
  - A whole-record pin reason for "can't confirm the same origin" (a
    sideload with its switch off, an unknown origin) is
    `origin_mismatch` too; Settings tells them apart by the local origin.
  - Joining reads extension data only once the extension source has
    run: a keys or lists value set up by hand on both computers is
    captured as this computer's edit after the join, with the other
    value in the conflicts list.

### WP-S6 Settings → Sync, scope controls and safe saves

- **Files (owned):** `ui/js/settings/sync.js`, `ui/js/settings/sync-setup.js`,
  `ui/js/settings/sync-conflicts.js`, `ui/js/settings/sync-review.js`,
  `ui/js/settings/sync-devices.js`, `ui/js/shared/sync-scope-chip.js`,
  `ui/js/settings/sync-ext-chip.js` (new: the chip on an extension's
  settings page, §12.3), `ui/css/settings-sync.css`, `ui/settings.html`,
  `ui/js/settings/manager.js` (register the module; field-level save;
  one call into the chip, in the header order of §23),
  `ui/js/settings/base.js`, `ui/js/settings/dirty-tracker.js`,
  `ui/js/shared/config-cache.js`, `ui/js/shared/settings-renderer.js`
  (per-field control on extension settings pages), the first-party
  settings modules that render synced rows (`appearance.js`, `hotkey.js`,
  `system.js`, `notifications.js`, `speech.js`, `speech-view.js`,
  `assistant.js`, `assistant-app-modes.js`, `connection.js`,
  `connection-list.js`, `model.js`, `mcp.js`, `internal-sites.js`,
  `internal-sites-draft.js`, `permissions.js`, `tool-permissions.js`,
  `integration.js`, `shortcuts.js`, `shortcuts-view.js`, `automations.js`,
  `automation-view-helpers.js`, `actions.js`, `store.js`, `privacy.js`,
  `updates.js`, `about-backup.js`), `ui/js/store.js` (the uninstall
  note, §12.8), `ui/js/floating/window.js`
  (`saveConfig`), `ui/js/chat/agent-sessions.js` and
  `ui/js/chat/app/session-history.js` ("On your other computers",
  read-only), `locales/*/messages.json` after C0 (merging the other
  packages' fragments), `ui-tests/settings/sync*.test.js`,
  `ui-tests/settings/manager-field-save.test.js`,
  `ui-tests/shared/sync-scope-chip.test.js`.
- **Depends on:** §19.5 and §19.6 (mocked); real data after S4.
- **Done when:**
  - Every screen and state of §13 renders from mocked commands; the
    sidebar item is hidden while `sync_get_status` reports `unavailable`.
  - Controls per §9.4 on every catalog row; the `sync-paths` test passes.
  - Field-level save per §9.5. Every in-tree `save_config` caller except
    `extension-permissions.js` (S5) uses `saveConfig` (a grep test).
  - Editors keep `sync_id` when they rebuild items (tests per editor).
  - The Store's uninstall confirm (`ui/js/store.js`) adds
    `settings.sync.uninstall_note` when the extension's data syncs
    (§12.8).
  - Keyboard-only use of setup, conflicts and reviews; RTL checked in
    `ar`; no hard-coded strings; `python scripts/check_i18n.py` passes.
  - With sync off, nothing changes visually on any page.

### WP-S7 Mind Map sync participation (Kage-Extensions)

- **Files (owned):** `extensions/mindmap/manifest.json` (`sync`
  permission and block, `config.my_names.sync`,
  `config.background_organize.sync`, `sync_private_items`),
  `extensions/mindmap/lib/syncmap.js` (new leaf: state ↔ collection
  records, link ids, groups, local fields), `extensions/mindmap/lib/model.js`
  (`applyRemote`, deterministic entity dedupe), `extensions/mindmap/mind.js`
  (push after ops, `syncPending`, pull and ack under `withLock`, initial
  push, `onChanges`), `extensions/mindmap/settings.js` (sync status
  line, "Sync private items", disclosure), `extensions/mindmap/defaults.js`
  (sync batch limits), `extensions/mindmap/_locales/en/messages.json`,
  `extensions/mindmap/README.md`, `extensions/mindmap/lib/syncmap.test.js`,
  `extensions/mindmap/lib/store.js` (only if `withLock` needs the
  "send inside, await outside" push of §12.9),
  `extensions/mindmap/sync.test.js`, `test-helpers/mindmap/sync-host.mjs`
  (new fake `context.sync`), `scripts/host-capabilities.mjs` (the `sync`
  capability and `ext_sync_*` commands), `scripts/validate-manifests.mjs`
  (the `sync` block and `config.<field>.sync`), `docs/mindmap/SPEC.md`
  (§7 sync, §15 disclosure), `docs/mindmap/ARCHITECTURE.md` (the
  `syncmap` module), `docs/mindmap/OPEN_QUESTIONS.md` (Q2 answered; Q13
  wording, §12.9 item 9).
- **Depends on:** §12.2 and §12.6 only; it builds against the fake host.
  End-to-end checks after S4 and S5.
- **Done when:**
  - §12.9 behaviour; the §18.6 tests pass.
  - Mind Map works unchanged on hosts without `sync.records`.
  - `npm run check:all` passes; the leaf-import rule holds
    (`host-loader.test.js`); private items still never reach agent
    payloads (existing tests stay green).
  - It rebases on the Mind Map packages of EXTENSION_VIEWS (WP-V8),
    EXTENSION_FILE_TEXT and EXTENSION_AGENT_CALLS that merged before it,
    and owns only the sync lines in their shared files.

### Merge order

1. **C0** (WP-S1's first commit). Everyone branches from it.
2. **WP-S1**.
3. **WP-S2**.
4. **WP-S3**.
5. **WP-S4**. Sync now works on dev builds for settings, shortcuts and
   automations, steering docs, the install list and extension settings.
6. **WP-S5**, after EXTENSION_VIEWS WP-V3 and WP-V4. Extension data syncs.
7. **WP-S6**. It may merge any time after C0, because it is hidden while
   the backend reports `unavailable`; its final pass comes after S4.
8. **WP-S7**. It may merge any time after C0 (feature-detected); the
   end-to-end check comes after S5.
9. The owner runs the §18.7 matrix on at least three providers and flips
   the gate.

Chat history is built inside S3 (source and provider) and S6 (history
view). It ships with them, off by default, and may be the last piece
finished.

---

## 21. Changes to other designs

These changes are already applied in the other three documents. The
Mind Map documents in Kage-Extensions are updated by the work packages
named in the last column.

| Design | Section | Change |
|---|---|---|
| EXTENSION_AGENT_CALLS | §10, the row "The extension's own switch (Mind Map `background_organize`)" | **"This computer"** (`"sync": "machine"`), not shared. Each computer sends notes to its own agent, and consent is per computer; a shared switch would ask for consent on a computer where the user didn't turn anything on. The `machine_tag` work split (§11.3) is unchanged. |
| EXTENSION_AGENT_CALLS | §10, `machine_tag` | Keeps its own `machine-id` file. Sync's device id exists only while sync is on, and changes on a clone or rejoin, so it can't replace it. |
| EXTENSION_AGENT_CALLS | §5.13, `extension_agent_access` | Backend-owned (`preserve_backend_owned_fields`), so a stale window can't switch access back on. That design lands first (§23), so C0 adds its `sync::catalog` row: This computer. |
| EXTENSION_AGENT_CALLS | §10 | A synced edit of the active agent connection makes consent stale on every computer. |
| EXTENSION_FILE_TEXT | §10, §11.1, §11.4, §11.8 | Sync encrypts extension data, so Mind Map bodies sync and `sync_document_text` is dropped. `possible_secret` documents (item and body) stay on their computer (§11.5 here). `file_ref`s move to a local `fileRefs` field. Bodies are a `large` collection, up to 4 MiB per record, which covers `doc_max_chars` = 1,000,000. The per-extension quota (128 MiB) covers the 64 MB body budget. `file-ref.key` is listed as never synced (§4.2). |
| EXTENSION_FILE_TEXT, EXTENSION_VIEWS | §5.2 / §13.4 | A click inside an extension view counts as the host-seen click a file dialog needs, through user activation in the view page. |
| EXTENSION_VIEWS | §21, §24.5 | `extension-views.json` is listed as never synced (§4.2). `mm-view` isn't listed in the manifest's `sync` block. `mm-layout` pins sync as the `pins` collection (§12.9), not as a whole key, so two computers moving different nodes merge. Sync emits `extension_data_changed` with `source: "sync"` for keys and lists modes, through that design's coalescer. |
| EXTENSION_VIEWS | §9.4, §8.6 | The view page offers `sync.keys`, `sync.lists` and `sync.records` like the other windows. |
| Mind Map OPEN_QUESTIONS | Q2, Q13 (WP-S7) | Q2 answered: yes, through Kage folder sync; no extension-side folder access is needed. Q13: window titles are "never sent to your agent", no longer "stay on this computer". |
| Mind Map SPEC | §6 "Sync readiness", §15 (WP-S7) | Tombstones live in the host's replica; Mind Map keeps deleting records as today and pushes the deleted ids. §15's "What goes where" adds the encrypted sync folder (§12.9 item 9). |

---

## 22. Small points for the owner

None of these blocks the work. Each has the default this design uses.

1. **Which sync app (provider) in telemetry?** Not sent, to keep
   telemetry to counts. It would help tell sync-app problems apart. One
   enum prop on `sync_problem` if wanted.
2. **Start at login** stays "This computer" and can't be shared, so a
   new computer never gains a login item silently.
3. **Themes** from the store install by themselves on other computers;
   extensions and command packs are suggestions. The alternative is one
   rule for all (always ask).
4. **Retention**: conflicts 90 days; Recently deleted, join undo and
   bulk-delete copies 30 days.
5. **Chat history budget**: 200 MB per computer, 8 MiB per chat.
6. **Agent connections sync** (available on the OS they were made on,
   spawn commands reviewed per computer). The stricter alternative keeps
   them on each computer.
7. **`updates.channel`** is shared; a dev channel on one computer only is
   one "Only on this computer" click.

---

## 23. Building the four designs together

Four designs are meant to be built in the same period: this one,
[`EXTENSION_VIEWS.md`](EXTENSION_VIEWS.md),
[`EXTENSION_FILE_TEXT.md`](EXTENSION_FILE_TEXT.md) and
[`EXTENSION_AGENT_CALLS.md`](EXTENSION_AGENT_CALLS.md). Inside each
design the work packages own disjoint files. Between them they don't, so
this section is the rule. The other three point here.

### 23.1 Contracts commits

The four contracts packages land one at a time, each rebased on the one
before:

1. **WP-AC0** (agent calls): the `NotPermitted` and `InvalidInput` error
   kinds, `sensitive::secret_spans`, the `extension_agent_access` config
   field, its catalog strings and event.
2. **WP-FT-C** (file text): the wire types in `kage-core`, its catalog
   strings and event. It uses AC0's kinds and `secret_spans`.
3. **WP-V1** (views): the capability, labels, commands, events and the
   view-surface denylist (including the `agent_call_*` names). Its errors
   use AC0's kinds.
4. **C0** (this design's WP-S1): the sync skeleton, its error kinds
   (`sync_unavailable`, `quota_exceeded`, `needs_confirmation`), and
   `sync::catalog` rows for every `Config` field the three before it
   added.

Why this order: the error kinds are shared, so the design that defines
them goes first; the sync catalog needs a row for every `Config` field,
so sync goes last. A `Config` field added by any design after C0 adds its
catalog row in the same commit (§4.7).

### 23.2 Feature packages

- Inside a design, its own merge order holds.
- Dependencies across designs:
  - WP-S5 needs EXTENSION_VIEWS' WP-V3 and WP-V4 (storage change events).
  - EXTENSION_FILE_TEXT's explorer parts (C11, C12) go in WP-FT-MM3,
    after WP-V8.
  - WP-FT6 adds the file broker to the view page after WP-V6.
  - WP-AC5 reuses WP-FT6's click lease, or adds it as EXTENSION_FILE_TEXT
    §5.2 specifies if FT6 hasn't merged.
  - WP-S7 rebases on WP-V8, WP-FT-MM3 and WP-AC-MM3, whichever merged.
- **Shared files.** When packages of two designs list the same file and
  both are in flight, the earlier design in this order merges its change
  first: EXTENSION_VIEWS, EXTENSION_FILE_TEXT, EXTENSION_AGENT_CALLS,
  SYNC. The later package rebases and owns only its own lines. A design
  that isn't being built blocks nothing.
- Package names are unique across the four: `V1`…`V8`, `FT0`…`FT8`,
  `FT-C`, `FT-X1`, `FT-MM1`…`FT-MM3`, `AC0`…`AC7`, `AC-X1`,
  `AC-MM1`…`AC-MM3`, `C0`, `S1`…`S7`.

Files listed by more than one design:

| File | Packages, in merge order |
|---|---|
| `src/error.rs` | AC0, C0, S4 |
| `src/events.rs` | AC0, FT-C, V1, C0, V3 (one stale `allow`), AC3 (one stale `allow`), S4 |
| `ui/js/shared/events.js` | AC0, FT-C, V1, C0, S4 |
| `locales/*/messages.json` | AC0, FT-C, V1, C0, then later strings of every package (S6 merges sync's fragments) |
| `src/lib.rs` | V1, FT5, AC1, C0 |
| `src/main.rs` | V1, V2, V3 (one comment), FT5, AC3 (the `mod` line, window focus), C0, S4 |
| `src/state.rs` | V2, AC3, S4 |
| `src/setup.rs` | V2, S4 |
| `src/commands/registry.rs` | V1, FT5, AC3, C0, S4 |
| `src/commands/mod.rs` | AC3, C0, S4 |
| `src/commands/extensions/mod.rs` | V1, FT5, C0, S5 |
| `src/commands/extensions/files.rs` | V3, S5 |
| `src/commands/extensions/install.rs` | V2, V3 (one argument), AC3, S5 |
| `src/commands/extensions/discovery.rs` | V2, AC3, S5 |
| `src/commands/system/config_io.rs` | V3, AC4, S4 |
| `src/commands/system/lifecycle.rs`, `src/tray.rs` | AC3, S4 |
| `src/extensions.rs` | V1, FT5, AC3, S5 |
| `src/config.rs`, `src/config_export/allowlist.rs` | AC0, S3 |
| `src/config_export.rs` | V3, S3 |
| `src/config_export/tests.rs` | V3, AC4 |
| `src/config/permissions.rs` | AC0, S3 |
| `src/permission_audit/types.rs`, `tests.rs` | FT5, AC3 |
| `src/agent_sessions/*` | AC2, S3 |
| `Cargo.toml` | FT1, C0 (`zeroize`), S2 (FT1 and S2 both need `hmac`: one 0.12 line, matching `sha2`) |
| `CLAUDE.md` | V7, FT1, S3 |
| `ui/js/shared/extension-permissions.js` | V1, FT6, AC3, C0, S5 |
| `ui/js/shared/extension-sandbox-host.js` | V4, FT6, AC5, C0, S5 |
| `ui/js/shared/extension-manager.js` | V5, FT6, AC5, S5 |
| `ui/js/extension-sandbox/runtime.js`, `runtime/host-api.js` | V4, AC5, S5 |
| `ui/js/shared/settings-host-effects.js` | V5, FT6, AC6 |
| `ui/js/shared/settings-renderer.js` | FT6, S6 |
| `ui/js/settings/manager.js` | V5, FT7, AC6, S6 |
| `ui/js/settings/extension-capabilities.js` | V1, FT7, AC6, S5 |
| `ui/js/settings/tool-permissions.js` | FT7, AC6, S6 |
| `ui/js/floating/app.js`, `ui/js/floating/app/lifecycle-visibility.js`, `ui/js/chat/app/lifecycle.js` | V5, FT6 |
| `ui/js/extension-view/main.js` | V6, then FT6 and S5 (their host features, §9.4 there) |
| `ui/js/shared/telemetry.js` | V5, FT6, AC7, S4 |
| `docs/EXTENSIONS.md`, `docs/SECURITY_MODEL.md`, `docs/PRIVACY.md` | V7, FT8, AC7, S2, S4, S5 |
| `ui-tests/shared/extension-permissions.test.js`, `extension-rust-alignment.test.js`, `extension-sandbox-host*.test.js` | V1, V4, FT6, AC5, C0 (the first two), S5 |
| Mind Map `manifest.json`, `mind.js`, `settings.js`, `view.js`, `_locales/en/messages.json`, `README.md`, `docs/mindmap/*` | V8, FT-MM3, AC-MM3, S7 |
| Mind Map `lib/model.js` | FT-MM2, AC-MM2, S7 |
| Mind Map `defaults.js` | FT-MM3, AC-MM3, S7 |
| Kage-Extensions `scripts/host-capabilities.mjs` | V8, FT-X1, AC-X1, S7 |
| Kage-Extensions `scripts/validate-manifests.mjs` | V8, AC-X1, S7 |
| Kage-Extensions `web/store-core.js` | V8, FT-X1 |

### 23.3 Shared names

**Capabilities.** Each goes into `VALID_CAPABILITIES`, `CAPABILITIES`,
`CAPABILITY_ICONS`, the keys `settings.manager.cap.<name>.label` and
`.desc`, and the Kage-Extensions mirror.

| Capability | Icon | Label | Design |
|---|---|---|---|
| `views` | 🖼️ | Its own windows | EXTENSION_VIEWS |
| `file_text` | 📄 | Files you choose | EXTENSION_FILE_TEXT |
| `agent_background` | 🌙 | Background AI | EXTENSION_AGENT_CALLS |
| `sync` | 🔄 | Sync between your computers | SYNC |

**Commands extensions may call** (all others the designs add are `null`).
The sandbox host force-injects `extensionId` for every one that takes it.

| Capability | Commands |
|---|---|
| `views` | none (the capability gates host behaviour only) |
| `file_text` | `pick_file_text` (also `displayName`), `cancel_file_text`, `file_text_support` |
| `agent_background` | `agent_call_reserve`, `agent_call_ready`, `agent_call_send`, `agent_call_cancel`, `agent_call_status` |
| `sync` | `ext_sync_status`, `ext_sync_push`, `ext_sync_pull`, `ext_sync_ack` |

**Host features per window**: the table in EXTENSION_VIEWS §9.4. A view
offers everything except `agent.background`.

**Error kinds.** New: `not_permitted`, `invalid_input` (AC0);
`sync_unavailable`, `quota_exceeded`, `needs_confirmation` (C0). All four
designs use `not_permitted` for a missing grant or a turned-off
extension, and `invalid_input` for bad arguments. Reused:
`rate_limited`, `timeout`, `cancelled`, `connection_lost`, `internal`.

**i18n key prefixes.**

| Design | Prefixes |
|---|---|
| EXTENSION_VIEWS | `errors.ext_view.*`, `ext_view.*`, `settings.manager.cap.views.*`, `settings.manager.ext.views.*` |
| EXTENSION_FILE_TEXT | `errors.file_text.*`, `file_text.*`, `settings.manager.cap.file_text.*`, `settings.tool_permissions.audit.summary.extension_file_read*`, `settings.tool_permissions.audit.filter.file_read` |
| EXTENSION_AGENT_CALLS | `errors.agent_call.*`, `settings.manager.cap.agent_background.*`, `settings.manager.ext.agent.*`, `floating.agent_call.*`, `tray.agent_call_tooltip`, `settings.tool_permissions.audit.summary.extension_agent_*`, `settings.tool_permissions.audit.filter.background_ai` |
| SYNC | `settings.sync.*`, `settings.sync_scope.*`, `sync.*`, `settings.manager.cap.sync.*`, `settings.manager.ext.sync.*`, `errors.sync.*` |

**An extension's settings page.** Host blocks come in this order, each in
its own module with one call from `ui/js/settings/manager.js`:
1. capability badges (existing);
2. the sync chip (`sync-ext-chip.js`, §12.3);
3. the Background AI panel (`extension-agent-access.js`,
   EXTENSION_AGENT_CALLS §4.2);
4. the Windows block (`extension-views-block.js`, EXTENSION_VIEWS §18.4);
5. the extension's own settings.

Only this design adds a sidebar item (§13).

**State.** No design adds a managed type, so
`tests/state_manage_parity_test.rs` doesn't change:
`FeatureServices.agent_calls`, `FeatureServices.sync`, `UiState.ext_views`.

**Local files** under `<data_local>/kage/`, all never synced and never in
a backup: `extension-views.json` (views), `file-ref.key` (file text),
`agent-calls-usage.json`, `machine-id`, `ephemeral-sessions.json`,
`agent-scratch/` (agent calls), `sync/` (this design).

**Telemetry.** `extension_view_opened`, `extension_file_text`,
`extension_agent_access_changed`, `extension_agent_calls_daily`,
`extension_agent_calls_paused`, and the `sync_*` events of §16. Each goes
into `KNOWN_EVENTS` and `docs/PRIVACY.md`. The file text and agent calls
events carry `extension_id`, as the existing `extension_installed` does;
the view and sync events carry none.

---

## Appendix A: how the drafts were combined

The base is the cloud-behaviour draft: per-computer write-once files,
full merged state per computer, content-named shards, a cell per field
and scope, a strict file classifier, and the key-store rule for
ad-hoc-signed macOS builds. From the data-model draft come the base rule
for captures, the capped HLC with a per-cell floor, deterministic
conflict records, safety around stale whole-config saves, and the rules
for older readers. From the product draft come the UI words and screens,
the guards (review before it runs, mass change, config reset, churn),
field-level Settings saves, lists mode, and the join preview and undo.

| Topic | Drafts said | This design |
|---|---|---|
| Transport | Per-computer append-only op log with snapshots / full state per computer in write-once shards / per-computer state files rewritten in place | Full state per computer in write-once, content-named shards (§5). No log to compact or replay; nothing is ever modified in place. |
| Causality | Version vectors per record / up to 3 ancestor stamps per cell / one base pointer per entry | 3 ancestor stamps per cell (§7.4), taken from what the user saw (`projected`), not from the replica. No false negatives; rare false positives are dismissable. |
| Unit of merge | Record value / field cell with scope / item | Field cell with scope (§7.2). Needed for per-OS fields inside list items and for field merges in Mind Map. |
| Merge rules | `lww`/`union`/`min`/`max`/`sticky`/`object` carried in data / `lww`/`set`/`max`/`min` from local code / policies applied on conflict | Structure only: whole fields, groups, sets, maps, local fields. Every cell is "higher stamp wins", so no rule is evaluated from local code and versions can't disagree. `sticky`, `min` and `max` aren't needed once fields merge separately. |
| HLC and skew | Capped observe plus per-record floor / uncapped / uncapped | Capped at wall + 1 hour, with a per-cell floor (§7.3) |
| Who sees conflicts | Every computer / every computer / only the losing computer | Every computer. Records are synced with deterministic stamps (§10.1), so the list is the same everywhere and restore works from any computer. |
| Conflict retention | 90 days / 90 days / 30 days | 90 days; Recently deleted and join undo 30 days |
| Local state location | Config folder / local data folder / config folder | `<data_local>/kage/sync/` (non-roaming; §6.1) |
| Category switches | Per computer / shared settings / per computer | Per computer, outside `config.json` |
| Key store | Keychain always / keychain only with a stable signature, else file / keychain always | Keychain only with a stable Developer ID signature; else an owner-only file (§11.4) |
| Argon2 cost | 64 MiB, t=3 / backup's 19 MiB, t=2 / 64 MiB, t=3 | 64 MiB, t=3, stored in the key file |
| Wrapped key location | `kage-sync.json` / per-computer key files / `kage-sync.json` with a revision counter | Per-computer key files; `kage-sync.json` written once (single writer, R1) |
| Agent connections | Never sync / sync without review / sync per OS with review | Sync, available on the OS they were made on, spawn command reviewed per computer (§8.10) |
| Steering path, MCP config path | This computer / per OS / this computer and per OS | This computer: they name files Kage writes |
| User steering doc at a custom path | Synced / not synced / synced | Not synced |
| Auto-steering doc | This computer / shared / shared, silent conflicts | Shared, conflicts under "Generated by Kage" |
| Scheduled automations | Sync as-is / arrive switched off / held until approved by hash | Held until approved by hash, so a later script change is reviewed too |
| Extension settings rule | Only plain settings / everything not sensitive / everything not sensitive | Not a credential, and plain or explicitly marked `"sync"` (§12.1). Stops a synced endpoint from redirecting a local key. |
| Extension data modes | Keys, records / keys, records / files, keyed lists, records | Keys, lists, collections (§12.4–§12.6) |
| Record API | `put`/`changes` with version tokens and extension-kept cursors / `push`/`pull`/`ack` / `push`/`pull`/`ack` | `push`/`pull`/`ack`; the host tracks each record's base (`ext_base`), so extensions keep no tokens and stale pushes become conflicts |
| Record size limits | 1 MiB / 1 MB / 512 KB | 64 KiB default; 4 MiB for `large` collections (document text) |
| Per-extension quota | 50 MiB / 64 MB / 100 MB | 128 MiB (Mind Map's 64 MB body budget plus records) |
| Themes from other computers | Auto-install / auto-install / ask | Store themes auto-install; extensions and command packs are suggestions |
| Join | Per-category choice with preview / two choices / three choices with undo | Two choices, a preview with counts, local-only items always added, undo for 30 days (§8.8) |
| Backup import while syncing | Spreads, with a note / banner asking / note | Spreads, with a note in the confirm; replaced values recorded as `restore_backup` conflicts |
| Mass-change, config-reset and churn guards | – / – / yes | Yes (§8.10) |
| Stale Settings saves | Backend three-way merge / backend three-way merge / frontend field-level save | Both (§9.5) |
| Deletes and dormant computers | Horizon plus rejoin with a local archive / acks plus a coming-back rule / absorb protocol | Acks, 60-day active window, 90-day cleanup, coming-back rule into the conflicts list (§8.7). Echoed state makes absorbing a leaver unnecessary. |
| Removing an extension's data from the folder | Causal range purge, data dropped everywhere / stamp purge, local data kept / tombstone everything | Stamp purge that also turns that extension's data sync off on each computer; local data kept (§8.7) |
| Chat delete | Hide everywhere, never delete agent files remotely / delete own only / delete everywhere including the origin's agent files | Hide everywhere; only a delete on the origin deletes, as today (§4.6) |
| Chat budget | 500 MiB total / 200 MB per computer / 2 MB per chat | 200 MB per computer, 8 MiB per chat |
| Poll and timings | 60 s poll; 10 s start / 2–10–30 min; 5 s start / 5–15–30 min; 10 s start | 5–15–30 min poll plus watcher and focus wakes; 10 s start; 1.5 s exit flush (§14) |
| Telemetry provider | Sent / sent / question | Not sent (§16, §22) |
| Mind Map `background_organize` | This computer / shared with a lease / this computer | This computer; no lease; EXTENSION_AGENT_CALLS' `machine_tag` split |
| Mind Map keyphrases | Union / whole field / union | Whole field: they are re-derived on edit, and a union would collect stale ones |
| Mind Map private items | Sync, opt-out / sync / sync, opt-out | Sync encrypted; "Sync private items" (on) to keep them where they are |
| Work packages | 14 / 13 / 14 | 7, with a contracts commit first (§20) |
