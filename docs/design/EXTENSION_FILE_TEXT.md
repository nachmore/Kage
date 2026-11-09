# Design: reading a file the user picks (extension file text)

Status: design, ready to build. Nothing here is implemented yet.

This document specifies a host API that lets an extension ask the user to
pick one file and get its text back. The first user is Mind Map, which
adds documents to the user's map.

Read first:
- `docs/SECURITY_MODEL.md` (the sandbox, capabilities, backups).
- `docs/EXTENSIONS.md` (manifest, `invoke()`, host features, errors).
- `docs/I18N.md` (keys, `t!`, `AppError::keyed`).
- `src/commands/folder_tools.rs` (`pick_folder`: the existing native
  dialog path).
- `docs/design/EXTENSION_AGENT_CALLS.md` (the sibling design for
  background agent calls; this one shares its error kinds and some files).
- Mind Map: `Kage-Extensions/docs/mindmap/SPEC.md` (§4, §6, §7, §15) and
  `OPEN_QUESTIONS.md` (Q4).

## 0. Decisions this design builds on

These were made by the owner and are not open:

- Mind Map reads file contents on capture. The user picks a file each
  time. Kage extracts text from txt, md, pdf and docx, with a size cap.
  A capability gates it.
- Mind Map gets a full-window pan/zoom explorer (a new extension "view"
  window, designed separately).
- Background AI organising in Mind Map is off by default (see the agent
  calls design). Private items are never sent.
- Mind Map actions stay separate from Todos. Unassigned actions are owned
  by "me".
- Mind Map's other defaults stay: agent tools on, fetch link titles on,
  record source window on.
- Kage will get a folder-based sync (Dropbox, iCloud Drive, OneDrive or
  any folder). Settings, shortcuts and automations, and extensions with
  their data sync by default (each extension opts in). Every setting has
  one shared value, which can be overridden per OS or for this machine
  only. Conflicts: newest change wins per item, and the loser is kept in
  a conflicts list in Settings → Sync. Sync must reuse the backup's
  sensitive and credential rules and never write a credential in plain
  text to the sync folder.

## 1. Summary

An extension that holds the new `file_text` capability can ask Kage to
open the native file dialog. If the user picks a file, Kage:

1. Checks that it is a regular file of a supported type and at most
   32 MB.
2. Reads it in the main process (so the OS grants access to the file the
   user chose), with a timeout for files still downloading from a cloud
   drive.
3. Parses it in a separate, short-lived worker process
   (`kage-file-text`) with memory, CPU and time limits. The worker gets
   only the bytes, never a path.
4. Normalises the text, caps it (default 100,000 characters, at most
   1,000,000), and flags text that looks like it holds a password or key.
5. Returns the text with a small set of metadata: file name, extension,
   format, size, modified time, title, page count, a SHA-256 of the
   bytes, and an opaque per-machine `file_ref`. **Never the path.**
6. Writes one audit entry and counts the read for telemetry (counts
   only).

The dialog opens straight away when the request follows a click the host
saw (a launcher Enter, a widget or toolbar button, a settings action).
Otherwise Kage first shows a small "ask bar" the user must click.

Formats: plain text, Markdown, PDF and Word (`.docx`) in the first
release. Web pages (`.html`), PowerPoint (`.pptx`) and Rich Text
(`.rtf`) follow in a second package on the same API.

Kage stores nothing from the file. What the extension keeps is ordinary
extension data, backed up and synced under the existing rules.

### Non-goals

- Reading any file the user didn't pick in that dialog, or a folder.
- Remembering a file and reading it again later without a new dialog.
- Returning bytes, images or the path.
- OCR of scanned pages.
- Multiple files at once, drag and drop (see §14).
- Opening the file afterwards (that is the `launch` capability's job).

## 2. Decisions made here

| Topic | Decision | Why |
|---|---|---|
| Capability | New `file_text`, separate from `filesystem` | `filesystem` is discovery (paths, scans). This one returns content but no paths. Users should see the difference at install. |
| Formats | P0: text, Markdown, PDF, DOCX. P1: HTML, PPTX, RTF. Not supported: `.doc`, spreadsheets, `.odt`, `.epub`, Apple iWork, images, email files | HTML and PPTX reuse the P0 machinery (tokenizer, OOXML reader). RTF is TextEdit's default on macOS, so it's worth a small in-house parser. The rest need large or immature parsers. |
| Path | Never returned | It exposes the user name and folder layout (the reason `get_user_info` is blocked), is machine-specific under sync, and isn't needed: `file_ref` covers "same file again", `sha256` covers "same content". |
| Where parsing runs | A sidecar binary, `kage-file-text`, one process per file | Untrusted files. A pure-Rust parser can still loop forever, recurse until the stack overflows, or allocate until the process aborts. In a child process that costs one error message, not the app. |
| Who reads the file | The main process | On macOS the user's pick grants Kage access to that file, also in protected folders. A child process may not get that grant. Passing bytes keeps the worker free of file access. |
| Dialog | `tauri-plugin-dialog` (already a dependency), every time, no remembered folder | Same path as `pick_folder`. The OS remembers the last folder per app. Kage stores no path. |
| Dialog text | Host-written: "{extension}: choose a file to read" | No extension-supplied text in a trusted OS dialog. |
| Without a click | Host "ask bar" with **Choose file…** and **Not now** | An agent tool call or a timer can't pop a dialog on its own. |
| New settings | None | Each read needs the user to pick a file. Disabling or uninstalling the extension stops it. No config field means nothing to migrate, back up or sync. |
| PDF permission bits | Ignored; a `restricted` warning is returned | The user owns the file and uses the text privately. A PDF that needs a password to open is refused. |
| Text format | Plain text with light Markdown for structure (`#` headings, `- ` list items, tab-separated table cells) | Keeps headings and lists, which Mind Map's extractor uses ("Action items"). |
| Error kinds | Reuse `not_permitted` and `invalid_input` from the agent calls design | No new kinds. Extensions branch on `key`. |

## 3. API surface

### 3.1 Capability

| Name | Icon | Label | Description |
|---|---|---|---|
| `file_text` | 📄 | Files you choose | Ask you to pick a file, then read its text (plain text, Markdown, PDF, Word, …). It gets only the file you pick, each time, never its folder or any other file. |

Added to `VALID_CAPABILITIES` (`src/extensions.rs`), `CAPABILITIES`
(`ui/js/shared/extension-permissions.js`), `CAPABILITY_ICONS` and the
`settings.manager.cap.file_text.*` keys
(`ui/js/settings/extension-capabilities.js`), and the Kage-Extensions
mirror (`scripts/host-capabilities.mjs`, `web/store-core.js`).

### 3.2 Manifest

No new manifest fields. An extension declares the capability:

```json
{ "permissions": ["storage", "file_text"] }
```

On a host without the capability, `normalize_permissions` drops the
unknown name with a warning, and the extension loads without it.

### 3.3 Host feature

`files.pick_text`. Offered by every window that hosts extension sandboxes
and can show the ask bar: floating, chat, settings, and extension view
windows (`ext-view:*`, [`EXTENSION_VIEWS.md`](EXTENSION_VIEWS.md) §9.4). Filtered by `hostFeaturesFor` with
`HOST_FEATURE_CAPABILITY['files.pick_text'] = 'file_text'`, so it appears
in `context.hostFeatures` only for extensions that hold the capability.

### 3.4 Runtime API in the sandbox

`context.files` exists only when `context.hostFeatures` includes
`files.pick_text`. It is a thin wrapper over the commands in §3.5, in a
new self-contained runtime part,
`ui/js/extension-sandbox/runtime/files-api.js`, appended to
`RUNTIME_SOURCE_PATHS`.

```js
const has = (f) => Array.isArray(context.hostFeatures) && context.hostFeatures.includes(f);

if (has('files.pick_text')) {
    // What this Kage can read: { formats: [{ id, extensions }], max_input_bytes,
    // max_chars, default_max_chars }
    const support = await context.files.support();

    try {
        const file = await context.files.pickText({
            formats: ['text', 'markdown', 'pdf', 'docx'], // optional; default: all this Kage reads
            maxChars: 200000,                              // optional; default 100,000; 1–1,000,000
            signal,                                        // optional AbortSignal
        });
        if (!file) return; // the user cancelled, or said "Not now"
        // file: FileText (§3.6)
    } catch (err) {
        // err.message is localised. Branch on err.key, e.g.
        // 'errors.file_text.too_large', 'errors.file_text.encrypted'.
    }
}
```

`pickText()` calls `pick_file_text` with `args: { formats, max_chars }`
(`maxChars` becomes `max_chars`). Aborting `signal` calls
`cancel_file_text`. It resolves `null` on cancel. `support()` calls
`file_text_support`.

Extensions may also call the commands through `context.invoke`. The
Kage-Extensions permission check finds `invoke('…')` call sites only, so
code that uses `context.files` adds the existing hint comment
`// permissions: file_text`.

Call it from a click handler (`execute`, `onWidgetAction`,
`onToolbarClick`, `onResultAction`, `runSettingsAction`, `onDateSelected`)
so the dialog opens at once (§5.2). Don't call it from agent tools: the
user sees the ask bar instead, and the tool's 60-second limit will often
run out before they finish.

### 3.5 Tauri commands callable by extensions

| Command | Capability | Args (JS names) | Returns |
|---|---|---|---|
| `pick_file_text` | `file_text` | `{ extensionId, displayName, args: { formats?, max_chars? } }` | `FileText \| null` |
| `cancel_file_text` | `file_text` | `{ extensionId }` | `bool` (something was cancelled) |
| `file_text_support` | `file_text` | `{}` | `FileTextSupport` |

The sandbox host force-injects `extensionId` (`pick_file_text` and
`cancel_file_text` join `IDENTITY_SCOPED_COMMANDS`) and, for
`pick_file_text`, `displayName` (the manifest name, already resolved by
`extension-manager/i18n.js`). Values
the sandbox sent are overwritten. Args are wrapped in `args`, like
`oauth_loopback_*`.

```rust
// src/commands/extensions/file_text.rs
#[tauri::command]
pub async fn pick_file_text<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::WebviewWindow<R>,    // the calling window: dialog parent, progress events
    features: tauri::State<'_, FeatureServices>,
    extension_id: String,
    display_name: String,
    args: Option<PickFileTextArgs>,
) -> Result<Option<FileText>, AppError>;

#[tauri::command]
pub async fn cancel_file_text(extension_id: String) -> Result<bool, AppError>;

#[tauri::command]
pub fn file_text_support() -> Result<FileTextSupport, AppError>;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PickFileTextArgs {
    #[serde(default)] pub formats: Option<Vec<Format>>,   // 1–7 unique
    #[serde(default)] pub max_chars: Option<u32>,         // 1..=1_000_000
}

#[derive(Serialize)]
pub struct FileTextSupport {
    pub formats: Vec<FormatInfo>,       // only what this build reads
    pub max_input_bytes: u64,           // 33_554_432
    pub max_chars: u32,                 // 1_000_000
    pub default_max_chars: u32,         // 100_000
}
pub struct FormatInfo { pub id: Format, pub extensions: Vec<&'static str> }
```

`Format` serialises lowercase: `text`, `markdown`, `pdf`, `docx`, `pptx`,
`html`, `rtf`. It lives in `kage-core` (§5.6) so the worker and the app
share it.

| Format | Extensions | Package |
|---|---|---|
| `text` | `txt`, `text`, `log`, `csv`, `tsv` | P0 |
| `markdown` | `md`, `markdown`, `mdown`, `mkd` | P0 |
| `pdf` | `pdf` | P0 |
| `docx` | `docx` | P0 |
| `html` | `html`, `htm`, `xhtml` | P1 |
| `pptx` | `pptx` | P1 |
| `rtf` | `rtf` | P1 |

`pick_file_text` and `cancel_file_text` are long-running or stateful.
Tauri does not drop a running command when its webview goes away, so the
sandbox host calls `cancel_file_text` when it destroys a sandbox with a
request in flight (§3.9), the same way it handles OAuth listeners.

The three commands are not in the mock-app sweep's denylist. With empty
args, `pick_file_text` fails on the missing `extensionId`; with an id,
it fails the grant check (§5.1 step 2) before any dialog. A test asserts
both (§12).

### 3.6 The result

```rust
#[derive(Serialize)]
pub struct FileText {
    pub name: String,                 // file name only, NFC, cleaned (§5.10), ≤255 chars
    pub extension: String,            // lowercase, without the dot
    pub format: Format,               // what the content turned out to be
    pub size: u64,                    // bytes
    pub modified_at: Option<i64>,     // ms since epoch (UTC), when the OS reports it
    pub title: Option<String>,        // document metadata or first heading, ≤200 chars
    pub text: String,
    pub chars: u32,                   // Unicode scalar values in `text`
    pub truncated: bool,              // the document had more text than `max_chars`
    pub total_chars: Option<u32>,     // full length, when the parser knows it
    pub pages: Option<Pages>,         // PDF pages / PPTX slides: { count, read }
    pub page_starts: Option<Vec<u32>>,// char offset in `text` where each read page starts
    pub warnings: Vec<Warning>,
    pub sha256: String,               // lowercase hex of the file's bytes
    pub file_ref: String,             // "fr1_" + base64url, opaque (§5.11)
}
```

`Warning` (snake_case strings):

| Warning | Meaning |
|---|---|
| `pages_skipped` | More than 2,000 pages or 1,000 slides; the rest were not read. |
| `no_text` | Almost no text found (for a PDF: likely a scan). `text` may be empty. |
| `replaced_characters` | Some bytes couldn't be decoded and became U+FFFD. |
| `legacy_encoding` | Plain text wasn't UTF-8 or UTF-16; Kage guessed the encoding. |
| `restricted` | The PDF asks readers not to copy its text. |
| `hidden_text_skipped` | Text marked hidden in the document was left out. |
| `possible_secret` | The text looks like it contains a password, token or key (§5.12). |

Field names stay clear of the host's secret-name heuristic (no `author`,
`token`, `key`, `session`), so an extension can store the object as is.

### 3.7 Text conventions per format

All formats end up as one string with `\n` line breaks, after the
normalisation in §5.10.

- **Text**: as decoded. Title: none.
- **Markdown**: as decoded, markup kept. Title: the YAML front matter's
  `title:`, else the first level-1 heading (`# …`, or a line over `===`)
  outside code fences.
- **PDF**: page by page in content order, a blank line between pages.
  Multi-column pages may interleave columns. Title: the document's
  `/Title`.
- **DOCX**: paragraphs separated by a blank line. Heading styles 1–3 (and
  `Title`) become `#`, `##`, `###`. List paragraphs start with `- `.
  Table cells are separated by a tab, rows by a line break. Text boxes
  inline. Footnotes and endnotes at the end under `Notes`. Skipped:
  comments, headers and footers, deleted tracked changes, field codes,
  hidden text. Title: `dc:title`, else the first heading.
- **PPTX**: each visible slide starts with `## Slide N` (or `## ` and the
  slide's title placeholder text). Speaker notes follow under `Notes:`.
  Hidden slides are skipped. Title: `dc:title`, else the first slide's
  title.
- **HTML**: visible text. Skipped: `script`, `style`, `noscript`,
  `template`, `svg`, `head` (except `title`), elements with the `hidden`
  attribute or an inline `display:none`. `h1`–`h3` become headings, `li`
  becomes `- `. Nothing is fetched. Title: `<title>`.
- **RTF**: body text. Skipped: font, colour and style tables, pictures,
  objects, headers, footers, field instructions (field results kept).
  Title: `\title` in `\info`.

### 3.8 Errors and i18n keys

Kinds: `not_permitted` and `invalid_input` are the two `ErrorKind`
variants the agent calls design adds (`NotPermitted`, `InvalidInput`).
Its WP-AC0 is the first contracts commit of the four designs (SYNC §23),
so this design only uses them. The rest reuse existing kinds.
All keys live under `errors.file_text.` in `locales/en/messages.json`.

| Key (`errors.file_text.…`) | Kind | Raised by | English message |
|---|---|---|---|
| `not_granted` | not_permitted | Rust | {extension} doesn't have permission to read files you choose. |
| `extension_disabled` | not_permitted | Rust | {extension} is turned off. |
| `invalid_formats` | invalid_input | Rust | Unknown file type requested. Kage reads: {formats}. |
| `invalid_max_chars` | invalid_input | Rust | The character limit must be between 1 and {max}. |
| `busy` | rate_limited | Rust, JS host (ask bar already showing) | Kage is already reading a file. Try again when it's done. |
| `too_soon` | rate_limited | Rust | Wait a moment before choosing another file. |
| `declined` | rate_limited | JS host | You chose "Not now". Try again in a moment. |
| `no_window` | cancelled | JS host | Open Kage to choose a file. |
| `unavailable` | internal | Rust | File reading isn't available in this copy of Kage. |
| `dialog_failed` | internal | Rust | The file picker couldn't open. |
| `not_a_file` | invalid_input | Rust | {name} isn't a regular file. |
| `unsupported` | invalid_input | Rust | Kage can't read .{ext} files here. It reads: {formats}. |
| `wrong_content` | invalid_input | Rust | {name} doesn't contain what its .{ext} name says. |
| `too_large` | invalid_input | Rust | {name} is {size}. Kage reads files up to {limit}. |
| `read_failed` | internal | Rust | Couldn't read {name}. |
| `not_downloaded` | timeout | Rust | {name} didn't finish downloading. Try again when it's on this computer. |
| `encrypted` | invalid_input | Rust | {name} is password-protected. Remove the password and try again. |
| `legacy_format` | invalid_input | Rust | {name} uses an old format Kage can't read. Save it as .docx or PDF and try again. |
| `not_text` | invalid_input | Rust | {name} doesn't look like a text file. |
| `malformed` | invalid_input | Rust | {name} looks damaged and couldn't be read. |
| `too_complex` | invalid_input | Rust | {name} is too complex to read safely. |
| `timeout` | timeout | Rust | Reading {name} took too long, so Kage stopped. |
| `cancelled` | cancelled | Rust | Reading {name} was cancelled. |
| `worker_failed` | internal | Rust | The file reader stopped unexpectedly. |

`{extension}` is the injected display name. `{name}` is the cleaned file
name. `{ext}` is the file's extension without the dot (a separate name, so
it can't be confused with `{extension}`). `{size}` and `{limit}` are
formatted by the host (`48.2 MB`). `{formats}` is a list of extensions
(`.txt, .md, .pdf, .docx`), which needs no translation. Errors the JS
host raises carry `errorKind` and `errorKey` the same way Rust errors do,
so extensions can branch on them.

Rust-side UI strings, also in `locales/en/messages.json`:
- `file_text.dialog.title`: "{extension}: choose a file to read"
- `file_text.dialog.filter`: "Supported files"

### 3.9 Sandbox host changes (`ui/js/shared/extension-sandbox-host.js`)

Kept small. The logic lives in a new module,
`ui/js/shared/extension-file-text.js` (§5.1–5.3), so the host file doesn't
grow much.

- `pick_file_text` and `cancel_file_text` join `IDENTITY_SCOPED_COMMANDS`
  (`extensionId`). `pick_file_text` also gets `displayName` from the spec.
- `call(method, params, { userGesture: true })`: sets the sandbox's click
  lease (§5.2) before posting, and marks the RPC's timer as pausable.
- `consumeClickLease()`, `pauseGestureTimers()`, `resumeGestureTimers()`.
- `_handleInvoke`: after the capability, argument and rate checks,
  `pick_file_text` is handed to `FileTextBroker.request(this, args)`
  instead of the raw invoke.
- `destroy()`: if this sandbox has a request in flight, call
  `cancel_file_text` and remove its ask bar.
- `HOST_FEATURE_FILE_TEXT = 'files.pick_text'` with its capability in
  `HOST_FEATURE_CAPABILITY`.
- `SandboxSpec.displayName`, set by `ExtensionManager` and the settings
  pool.

`validateExtensionInvokeArgs` gains a branch for `pick_file_text`: `args`
is absent or an object with only `formats` (array of 1–7 known ids, no
duplicates) and `max_chars` (integer 1–1,000,000). Anything else is
refused before Rust sees it. Rust checks again.

### 3.10 Events

| Event | Payload | Audience |
|---|---|---|
| `file_text_progress` | `{ extension_id, phase, file? }` | The calling window only |

`phase` is `dialog_closed` (with `picked: bool`), `reading`,
`downloading`, `extracting` or `done`. `file` is the cleaned file name.
Emitted by Rust to the window that called `pick_file_text`
(`event_targets::emit_to_self`). Sandboxes never see it; the host uses it
for the window guard and the progress chip. The name is a constant in
`src/events.rs` and `ui/js/shared/events.js` (WP-FT-C), so
`events-and-windows.test.js` keeps the two sides equal.

## 4. What the user sees

### 4.1 Install and upgrade prompt

The capability shows in the install prompt and on welcome cards like any
other: "📄 Files you choose — Ask you to pick a file, then read its text
… never its folder or any other file." An existing extension that adds it
is held for approval (Store → Approve), as for any added capability.
Until then `files.pick_text` is missing from `context.hostFeatures`, and
`context.files` is `undefined`.

### 4.2 The file dialog

- The native dialog of the OS, every time.
- Title: "Mind Map: choose a file to read". macOS shows it as the panel's
  message line; Windows and Linux show it in the title bar.
- One filter, "Supported files", listing the requested formats'
  extensions. No "All files" entry.
- No starting folder from Kage. The OS opens where it last was.
- From a chat, settings or view window the dialog is attached to that
  window. From the launcher the launcher hides first and comes back after
  (§5.3).

### 4.3 The ask bar (when there was no click)

When the request doesn't follow a click the host saw (§5.2), the calling
window shows a bar at the top, rendered by the host:

> 🧠 **Mind Map** wants to read a file you choose. **[Choose file…]**
> **[Not now]**

- The extension's icon and name come from its manifest, via the host.
- **Choose file…** opens the dialog. **Not now**, Escape, or 60 seconds
  without an answer resolves the request with `null`. After **Not now**
  the same sandbox can't ask again for 30 seconds (`declined`).
- One bar per window. A second request while one is showing fails with
  `busy`.
- If the window is hidden (a sandbox in a hidden chat window, or the
  launcher while it's closed), the request fails with `no_window`. Kage
  never pops a window open for this.
- `role="alertdialog"`, focus moves to **Choose file…**, keyboard
  reachable. Logical CSS properties, so it mirrors in RTL languages.

### 4.4 Progress chip

If reading takes more than 400 ms, the calling window shows a chip:
"Reading Q3 plan.pdf… **Cancel**", or "Downloading Q3 plan.pdf… **Cancel**"
while a cloud drive fetches it. **Cancel** calls `cancel_file_text`. The
chip goes away on `done`. File names are shown in `<bdi>` with
`dir="auto"`, so a right-to-left name can't reorder the text around it.

### 4.5 Audit viewer and capability pill

- Settings → Extensions → the extension: the pill "📄 Files you choose".
- Settings → Tool permissions → Audit log: one row per file read, with a
  new filter option, "File reads":
  - 📄 "{extension} read {file} ({format}, {size}, {chars} characters)"
  - 📄 "{extension} couldn't read {file}", with the outcome code
    (`too_large`, `malformed`, …) as a muted tag

  The log already has "Clear". It is on this machine only (§5.16).

### 4.6 No new setting

There is no switch for this feature. Each read needs the user to pick the
file in the OS dialog, so there is no standing access to turn off. To
stop an extension asking, the user disables or uninstalls it. If
fine-grained grants arrive (SECURITY_MODEL "Known gaps"), `file_text` is
a natural first capability to make revocable on its own.

### 4.7 UI i18n keys

All in `locales/en/messages.json`:

- `settings.manager.cap.file_text.label` ("Files you choose"),
  `settings.manager.cap.file_text.desc`
- `file_text.ask.message` ("{extension} wants to read a file you
  choose."), `file_text.ask.choose` ("Choose file…"),
  `file_text.ask.dismiss` ("Not now")
- `file_text.progress.reading` ("Reading {file}…"),
  `file_text.progress.downloading` ("Downloading {file}…"),
  `file_text.progress.cancel` ("Cancel")
- `settings.tool_permissions.audit.summary.extension_file_read`,
  `settings.tool_permissions.audit.summary.extension_file_read_failed`,
  `settings.tool_permissions.audit.filter.file_read`
- `errors.file_text.declined`, `errors.file_text.no_window` (raised by the
  JS host, listed in §3.8)

## 5. Host behaviour

### 5.1 Life of a request

In the window's JS host (`FileTextBroker`, one per window):

1. The sandbox posts `invoke('pick_file_text', { args })`.
2. The sandbox host runs `decideInvoke`, the argument check and the
   invoke rate limit, as for every command.
3. If the window isn't visible: reject with `no_window`.
4. If the sandbox holds a click lease, consume it. Otherwise: if a
   **Not now** cooldown is running, reject with `declined`; else show the
   ask bar and wait. **Not now** or timeout → resolve `null`.
5. Pause the timers of this sandbox's click RPCs (§5.2).
6. Call the window's dialog guard `begin()` (§5.3).
7. Invoke Rust `pick_file_text` with the forced `extensionId` and
   `displayName`.
8. On `dialog_closed`, call the guard's `end()`. Show the chip on
   `reading`, `downloading` or `extracting` after 400 ms.
9. When Rust returns or throws: hide the chip, call `end()` if it wasn't
   called, resume the timers, post the result to the sandbox.

In Rust (`pick_file_text`), in this order:

1. Validate `extension_id` (`extensions::validate_extension_id`). Clean
   `display_name` (§5.10, at most 64 chars; empty → the id).
2. Check the grant: `config.extension_grants[id].granted` contains
   `file_text`, and the extension is enabled. Else `not_granted` or
   `extension_disabled`. This is defence in depth: the JS host already
   refused.
3. Validate `args`. Default formats: every format this build reads.
   Default `max_chars`: 100,000.
4. Find the worker next to the current executable (§5.5). Missing →
   `unavailable`, before the user picks anything.
5. Take the single slot: one request at a time in the whole app (`busy`),
   and at least 3 seconds since this extension's last dialog closed
   (`too_soon`).
6. Open the dialog (§5.3). Emit `dialog_closed`. No file, or the request
   was cancelled while the dialog was open → release the slot, return
   `Ok(None)`.
7. Check the pick (§5.4): a regular file, an extension in the requested
   formats, at most 32 MB. No read happens before these pass.
8. Emit `reading` (or `downloading`, §8). Read the bytes with a 60-second
   timeout.
9. Compute `sha256` and `file_ref` (§5.11).
10. Emit `extracting`. Run the worker (§5.5) with a 30-second deadline and
    the request's cancel flag.
11. Build `FileText`: clean the name and title, add `possible_secret` if
    §5.12 matches.
12. Append the audit entry, track telemetry, log one line, emit `done`,
    release the slot, return.

Any error after step 6 also writes the audit entry (with the error key)
and emits `done`.

`cancel_file_text` sets the cancel flag of the extension's request if it
is the one in the slot. During the dialog the flag is checked when the
dialog closes (an open native dialog can't be closed from code). During
the read the result is dropped. During extraction the worker is killed.

### 5.2 The click lease

The host knows which RPCs it sends because the user did something. Those
call sites pass `{ userGesture: true }` to `ExtensionSandbox.call()`:

| Call site | RPC |
|---|---|
| `extension-manager/search.js` `executeResult` | `execute` |
| `extension-manager/ui.js` | `onWidgetAction`, `onToolbarClick`, `onResultAction` |
| `extension-manager/dates.js` | `onDateSelected` |
| `settings-renderer.js` | `runSettingsAction` |
| `settings-host-effects.js` | `onFileSelected` |
| View page (`ui/js/extension-view/main.js`) | none: see the view rule below |

Such a call gives the sandbox a lease: one dialog, within 10 seconds.
`pick_file_text` consumes it. Nothing else grants one: not `match`,
`matchAsync`, `getTools`, tool calls from the agent, widget refreshes,
formatter passes or timers. The sandbox can't fake a lease, because the
host only sets it on its own outgoing calls.

While a `pick_file_text` request from a sandbox is in flight, the timers of
that sandbox's `userGesture` RPCs are paused and later resumed with the
time they had left. So a launcher `execute()` that awaits the dialog
doesn't hit the 10-second RPC timeout while the user browses folders.

**Extension view windows.** A view draws its own buttons inside its
iframe, so the host never sends it a click RPC. Instead, the view page
grants the lease when `pick_file_text` arrives from the view sandbox and
the "recent user activation" rule of
[`EXTENSION_VIEWS.md`](EXTENSION_VIEWS.md) §13.4 holds: the view window
is focused and `navigator.userActivation.isActive` is true in the view
page. A click inside the iframe activates the page as well (activation
propagates to ancestor frames), and script can't fake it. On an engine
without `navigator.userActivation`, the view gets the ask bar.

### 5.3 Dialogs per window

The dialog guard is a pair of callbacks a window registers with its
`FileTextBroker`.

- **Floating launcher and inline assist** (overlay windows). `begin()`
  sets `_nativeDialogOpen` (the blur handler in
  `ui/js/floating/app/lifecycle-visibility.js` returns early while it is
  set) and hides the window. Rust opens the dialog with no parent, after
  `setup::activate_app` (as `pick_folder` does), so it gets keyboard
  focus. `end()` shows the launcher again through its normal show path,
  sets `_suppressBlurHideUntil = now + 1500`, and clears the flag. The
  extension's reply (`display`, …) then shows in the launcher as usual.

  Why hide: the launcher is always on top and hides when it loses focus.
  A dialog attached to it would sit on a small input bar (a sheet on
  macOS), and a free dialog can end up under it on Windows and Linux.
- **Chat, settings and view windows.** No-op guard. Rust attaches the
  dialog to the calling window (`set_parent`): a sheet on macOS, an owned
  modal dialog on Windows, a transient dialog on Linux.

Rust picks the mode from `window.label()`: `window_labels::FLOATING` and
`INLINE_ASSIST` mean "no parent", everything else "parent".

```rust
let mut dialog = app.dialog().file()
    .set_title(t!("file_text.dialog.title", "extension" => &display_name))
    .add_filter(t!("file_text.dialog.filter"), &extensions); // requested formats only
if attach { dialog = dialog.set_parent(&window); } else { crate::setup::activate_app(&app); }
let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_file()).await?;
```

### 5.4 Reading the file

All in `src/file_text/read.rs`, on a blocking thread. The platform parts
(the non-blocking open and the cloud-placeholder check in §8) go through a
new `src/os/file_probe.rs` with per-OS files in `src/os/{windows,macos,linux}/`,
as `docs/OS_ABSTRACTION_GUIDE.md` requires:

1. `std::fs::metadata` (follows links): must be a regular file. A
   directory (macOS packages like `.rtfd`), FIFO, socket or device →
   `not_a_file`.
2. The extension (lowercase) must belong to a requested format →
   `unsupported`. The Linux GTK dialog lets users type any path, so the
   filter alone isn't enough.
3. `len() > 32 MiB` → `too_large`, without reading.
4. Open. On Unix, open with `O_NONBLOCK`, `fstat` again (must still be a
   regular file), then clear `O_NONBLOCK`. This keeps a FIFO swapped in
   after step 1 from blocking the read.
5. Read through `take(MAX_INPUT_BYTES + 1)`; more than the cap →
   `too_large`.
6. Timeout: 60 seconds for the whole read. On timeout the thread is left
   to finish on its own and its bytes are dropped → `not_downloaded`.
7. `modified_at` from the metadata, when the OS gives it.

The name returned is the final path component only. The path itself
stays in local variables of this function and the `file_ref` input.

### 5.5 The worker process

A new workspace crate, `kage-file-text/`, is both a library (the parsers,
for tests and fuzzing) and a binary (the worker). It depends on
`kage-core` for the wire types and on the parser crates (§6). The app
crate does not link the parsers.

The worker links `kage-core` with `default-features = false`. Its `os`
feature, on by default for the app and the other sidecars, gates the OS
layer and everything built on it. Without it, the worker loads no
AppKit, UI Automation or `dirs`: on macOS it links only `libSystem` and
`libiconv`.

**Provisioning.** Like the MCP sidecar: `build.rs` calls
`provision_sidecar("kage-file-text")` on every platform (with
`rerun-if-changed` on `kage-file-text/src` and its `Cargo.toml`, not on
`fuzz/`, which keeps a target dir and corpora), and `tauri.conf.json` and
`tauri.macos.conf.json` list `src-tauri/binaries/kage-file-text` in
`externalBin` (the macOS file replaces the base list, so both need it).
On macOS tauri-bundler signs it with the app's entitlements, as it does
the other sidecars.

**Finding it.** The sibling of `std::env::current_exe()` only
(`kage-file-text`, `.exe` on Windows). Never `PATH`: a planted binary must
not get the user's files.

**Spawning** (`src/file_text/worker.rs`, on a blocking thread):

- `Command::new(path).arg("--v1")`, `env_clear()` (keeping only
  `SystemRoot` on Windows), current directory set to the temp dir, all
  three stdio piped, `CREATE_NO_WINDOW` on Windows.
- Not `login_env::apply`: this is Kage's own binary.
- One thread writes the request (§5.6) and closes stdin. One reads stdout
  through `take(max_chars * 4 + 1 MiB)`. One drains stderr through
  `take(16 KiB)` (debug log, English, never shown).
- The parent polls `try_wait()` every 25 ms until the worker exits, the
  30-second deadline passes, or the cancel flag is set. The last two kill
  it.
- Exit status 0 with a valid response → result. A killed worker →
  `timeout` or `cancelled`. Any other exit (abort, signal, stack
  overflow, out of memory) → `too_complex`. Output over the cap →
  `worker_failed`.

**Inside the worker** (`kage-file-text/src/main.rs`), before reading
stdin:

- A counting global allocator with a 768 MiB cap. Over the cap, the
  allocation fails and Rust aborts the worker.
- Unix: `setrlimit(RLIMIT_CPU)`, 30 s soft and 31 s hard, and
  `RLIMIT_CORE` 0, so a crash never writes the document into a core
  file. Linux also `RLIMIT_AS` (2 GiB), `PR_SET_NO_NEW_PRIVS`, and
  `PR_SET_PDEATHSIG(SIGKILL)` so it dies with Kage. The death signal
  fires when the thread that spawned the worker exits, so the app spawns
  it and waits for it on the same thread.
- Windows: create a Job Object with a 1 GiB process memory limit, an
  active-process limit of 1 (it can't start programs) and death on an
  unhandled exception, and assign itself to it. The error mode turns
  off crash dialogs. It is also inside Kage's kill-on-exit job, so it
  dies with Kage.
- macOS doesn't enforce `RLIMIT_AS`, so there the allocator cap is the
  memory limit. `RLIMIT_CPU` still applies.
- Each limit is best effort: one that fails is noted on stderr and the
  worker goes on. The deadline and the allocator cap still hold.
- The panic hook prints only where a panic happened, never its message,
  which could quote the file.
- Read the request frame (header ≤ 4 KiB, payload ≤ 32 MiB). A bad frame
  or an unknown `v` → `Err { code: Malformed }`. Any argument other than
  `--v1` → usage on stderr, exit status 2.
- Run the parser on a thread with a 64 MiB stack, inside
  `catch_unwind`. A panic → `malformed`.
- Write one JSON response to stdout, exit 0.

The worker needs no files and no network. Further sandboxing (seccomp,
the macOS sandbox profile, a Windows AppContainer) is listed in §14.

Why not the main binary in a helper mode (as `/capture-hotkey` does on
Windows)? The parsers would bloat the app, the allocator cap would apply
to the app too, and on Linux every start would load the GTK and WebKit
libraries the app links. Why not in-process with `catch_unwind`? A stack
overflow or allocation failure aborts the whole app, and an endless loop
can't be stopped.

### 5.6 Wire protocol

Types in `kage-core/src/file_text_wire.rs` (serde only), shared by the app
and the worker, like `CalendarEvent` is for the calendar helper.

Request on stdin: `u32` LE header length, the header as JSON, `u64` LE
payload length, the payload bytes. `write_request` and `read_request` in
the same module write and read this frame, so both sides use one copy.
They refuse a header over 4 KiB or a payload over 32 MiB before
allocating. `WORKER_BIN_NAME` (`kage-file-text`), `WORKER_FLAG` (`--v1`)
and `WIRE_VERSION` (1) live there too.

```rust
pub struct WorkerRequest {
    pub v: u32,                    // 1
    pub formats: Vec<Format>,      // what the caller accepts
    pub extension: String,         // the file's extension, for detection
    pub max_chars: u32,
    pub max_pages: u32,            // 2_000
    pub max_slides: u32,           // 1_000
}

#[serde(tag = "status", rename_all = "snake_case")]
pub enum WorkerResponse {
    Ok {
        format: Format,
        text: String,
        chars: u32,
        truncated: bool,
        total_chars: Option<u32>,
        title: Option<String>,
        pages: Option<Pages>,
        page_starts: Option<Vec<u32>>,
        warnings: Vec<Warning>,
    },
    Err { code: WorkerErrorCode, detail: String },  // detail: English, for the log only
}

pub enum WorkerErrorCode {  // maps 1:1 to errors.file_text.* keys
    Unsupported, WrongContent, Encrypted, LegacyFormat, NotText, Malformed, TooComplex,
}
```

A `v` the worker doesn't know → `Err { code: Malformed }` and the parent
reports `worker_failed`. The worker and app always ship together, so this
only guards against a broken install.

### 5.7 Limits

| Limit | Value | Where |
|---|---|---|
| Input file | 32 MiB, refused above | Parent, before reading |
| Output text | 100,000 chars by default; the extension may ask 1–1,000,000 | Worker, then parent re-checks |
| PDF pages read | 2,000 (then `pages_skipped`) | Worker |
| PPTX slides read | 1,000 | Worker |
| ZIP entries (DOCX, PPTX) | 20,000 in the central directory | Worker |
| One XML part, inflated | 64 MiB, counted while inflating | Worker |
| All parts, inflated | 256 MiB | Worker |
| XML / RTF nesting | 256 levels | Worker |
| Title / name | 200 / 255 chars | Worker / parent |
| Worker memory | 768 MiB allocator cap; 1 GiB (Windows job), 2 GiB address space (Linux) | Worker |
| Worker time | 30 s wall clock (parent kills), 30 s CPU (Unix) | Both |
| File read | 60 s | Parent |
| Worker stdout | `max_chars * 4 + 1 MiB` | Parent |
| Click lease | 10 s, one dialog | JS host |
| Ask bar | 60 s; 30 s cooldown after **Not now** | JS host |
| Requests in flight | 1 in the whole app | Rust |
| Between dialogs | 3 s per extension | Rust |

The constants live in `kage-core/src/file_text_wire.rs` so both sides and
the tests use one copy. The three JS host limits (click lease, ask bar,
cooldown) live in `ui/js/shared/extension-file-text.js`.

### 5.8 Format detection

In the worker, from the bytes, checked against the extension:

| Content | Detected as |
|---|---|
| `%PDF-` in the first 1,024 bytes | `pdf` |
| ZIP (`PK\x03\x04`) whose `[Content_Types].xml` names a WordprocessingML main part | `docx` |
| ZIP whose `[Content_Types].xml` names a PresentationML main part | `pptx` |
| `{\rtf` at the start (after whitespace) | `rtf` |
| OLE compound file (`D0 CF 11 E0 A1 B1 1A E1`) | `encrypted` if it holds an `EncryptedPackage` stream name, else `legacy_format` |
| Anything else that decodes as text (§5.9) | `text`, `markdown` or `html`, by extension |

If the detected format isn't the one the extension implies, the request
fails with `wrong_content`. `text` and `markdown` count as one family.
Three details:
- An OLE file is `encrypted` or `legacy_format` whatever its extension.
- For `text`, `markdown` and `html` the PDF header must be at offset 0,
  so a note that mentions `%PDF-` near its top is still text.
- A ZIP's entry count is read from its end record (and the ZIP64 one)
  before the archive is parsed, so a huge directory is refused
  (`too_complex`) without being built.

### 5.9 Per-format extraction rules

**Text and Markdown**
- BOM first (UTF-8, UTF-16 LE/BE). Then UTF-16 without a BOM if the NUL
  pattern says so; this comes before UTF-8, because UTF-16 text in the
  ASCII range is also valid UTF-8. Then strict UTF-8. Then UTF-8 with a
  few bad sequences (at most one per 20 multi-byte characters, or one
  cut-off character at the end), decoded with U+FFFD
  (`replaced_characters`), so one bad byte doesn't turn a UTF-8 file
  into a legacy guess. Then `chardetng` picks an encoding and
  `encoding_rs` decodes it (`legacy_encoding`).
- More than 1% control characters in the first 8 KiB → `not_text`.
  Tab, line breaks, form feed, vertical tab and ESC (colour codes in
  logs) don't count.
- The whole file is decoded (at most 32 MiB), and the normaliser counts
  past `max_chars`, so `total_chars` is exact.

**DOCX and PPTX** (`kage-file-text/src/ooxml/`)
- Open the ZIP from memory with the `zip` crate. More than 20,000 entries
  → `too_complex`. Encrypted entries → `encrypted`.
- Read only the parts needed: `[Content_Types].xml`, the main part,
  its `.rels`, `docProps/core.xml`, footnotes and endnotes (DOCX), the
  slides and notes reached through `ppt/_rels/presentation.xml.rels` in
  `p:sldIdLst` order (PPTX). Media and everything else are never inflated.
  Overlapping-entry bombs don't help an attacker, because only these few
  entries are read and the total inflate cap applies.
- Each part is inflated through a counting reader (64 MiB per part,
  256 MiB in all). A part that declares a size and inflates past it →
  `malformed`.
- XML with `quick-xml` in streaming mode. Elements are matched by local
  name, so Strict OOXML namespaces work too. `quick-xml` doesn't load
  DTDs or external entities and doesn't expand custom entities, so there is
  no XXE and no "billion laughs". Depth over 256 → `too_complex`.
- DOCX: `w:t` text, `w:tab` → tab, `w:br`/`w:cr` → line break, `w:p` →
  paragraph, `w:tc`/`w:tr` → tab/row. `w:pStyle` of `Title` or
  `Heading1`–`3` → `#`–`###`. `w:numPr` → `- `. Skip `w:delText`,
  `w:instrText`, runs with `w:vanish` (`hidden_text_skipped`), comments,
  headers, footers. In `mc:AlternateContent` read `mc:Choice` only, so
  text isn't doubled.
- PPTX: `a:t` text, `a:p` → line break, `a:br` → line break, the
  `title`/`ctrTitle` placeholder → the slide heading, `show="0"` slides
  skipped.

**PDF** (`kage-file-text/src/pdf/`)
- Parse from memory with `pdf-extract` 0.12.1, chosen in WP-FT0, set up
  as §6.2 says. Page by page, so the page cap and `max_chars` stop work
  early, and `page_starts` can be recorded. A file `lopdf` 0.42 can't
  load gets one repair try through `lopdf` 0.47 (§6.2).
- A document that needs a user password → `encrypted`. One with only an
  owner password (permission bits) is decrypted with the empty password
  and read (`restricted`).
- The crate has no inflate cap, so the worker sets one while loading
  (§6.2): image streams are emptied, and so is any Flate stream that
  inflates past 32 MiB. The allocator cap stays the backstop.
- Before §5.10: right-to-left lines go back to logical order, and Latin
  ligatures, Arabic presentation forms and Kangxi radicals get NFKC
  (§6.2).
- A Type0 font with no ToUnicode map and an Adobe CJK ordering gets one
  from Adobe's tables (§6.2).
- Fewer than 16 characters per page on average → `no_text`.
- Info `/Title` → `title`.

**HTML** (P1, `kage-file-text/src/html/`)
- Encoding: BOM, then a `<meta charset>` prescan of the first 1,024 bytes,
  then UTF-8, then Windows-1252 (the HTML standard's fallback).
- `html5ever`'s tokenizer with a small sink, no tree builder: linear time,
  no DOM in memory. A stack of open "skipped" elements handles
  `script`/`style`/`hidden`; mis-nested markup can only make it skip too
  little or too much text, never loop.

**RTF** (P1, `kage-file-text/src/rtf/`)
- An in-house tokenizer of about 300 lines: groups, control words with
  numeric parameters, `\'hh` bytes decoded with the `\ansicpg` code page
  (via `encoding_rs`), `\uN` with `\ucN` fallback skipping, `\bin N`
  skipped (capped by the input length). Group depth over 256 →
  `too_complex`. WP-FT4 may use the `rtf-parser` crate instead if it passes
  the same tests and fuzzing.

### 5.10 Text normalisation and truncation

In the worker, after extraction, for every format, in this order:

1. Unicode NFC.
2. `\r\n` and `\r` → `\n`. U+0085, U+2028 and U+2029 → `\n`.
3. Remove: NUL and other C0 controls except tab and `\n`; DEL; C1 controls;
   U+FEFF; noncharacters; Unicode tag characters (U+E0000–U+E007F, used to
   hide instructions from people but not from AI models); the bidi
   overrides LRO and RLO (U+202D, U+202E). Embeddings, isolates and the
   LRM/RLM marks stay, because right-to-left documents use them.
4. Trim trailing spaces on each line. Collapse three or more line breaks
   into two. No blank lines at the start, no whitespace at the end.
5. If the text is longer than `max_chars` (counted in Unicode scalar
   values), cut it. Prefer the last paragraph break in the final 5%, then
   the last sentence end, then the last whitespace (all three within
   that final 5%), then a hard cut. Set `truncated`.

Parsers feed their text through one `Normalizer`
(`kage-file-text/src/normalize.rs`), which also records `page_starts`
and tells a parser when it can stop. NFC runs on each piece a parser
pushes, so parsers push whole pages or paragraphs, not word fragments.

File names and titles get the same treatment, plus: every bidi control
removed, whitespace collapsed to single spaces, one line, trimmed, capped.
The display name injected by the host gets it too.

`chars` counts scalar values. JavaScript's `text.length` counts UTF-16
units and can be larger for emoji and rare scripts. Extensions should use
`chars` from the result.

### 5.11 `file_ref` and `sha256`

- `sha256`: of the bytes read, in the parent (`sha2` is already a
  dependency). The same content gives the same hash on every machine.
- `file_ref`: `"fr1_"` + base64url of the first 16 bytes of
  HMAC-SHA256(key, `"v1\0" + extension_id + "\0" + canonical path in
  NFC`). The key is 32 random bytes in
  `<data_local_dir>/kage/file-ref.key` (owner-only, created on first use,
  replaced if malformed), resolved through `crate::paths`.
- Properties: stable for the same file at the same place on this machine;
  different for each extension, so two extensions can't link their
  records; meaningless on another machine; not reversible to the path.
  Renaming or moving the file changes it. Losing the key changes every
  ref; only "same file again" detection suffers.
- The `hmac` crate (RustCrypto, MIT or Apache-2.0) matches the existing
  `sha2`.

### 5.12 Secret-shaped text

After extraction, the parent runs the backup classifier's `SECRET_SHAPE`
over the text: `!sensitive::secret_spans(&text).is_empty()` → add
`possible_secret`. `secret_spans` is the function the agent calls design's WP-AC0
adds to `src/config_export/sensitive.rs`, before this design's contracts
land (SYNC §23). `TOKEN_RUN` is not used: in documents it matches too many
ordinary ids and hashes. Nothing is removed. The extension decides what to
do (Mind Map makes the item private, §11.7). The match itself is never
logged or returned.

### 5.13 Audit log

One new `AuditEvent` variant in `src/permission_audit/types.rs`, written
when a picked file has been handled, with any outcome. Not written when the
user cancels the dialog.

```rust
ExtensionFileRead {
    extension_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file_name: Option<String>,    // cleaned name; None if it looks like a secret
    format: Option<String>,       // detected format, when known
    bytes: u64,
    chars: u32,                   // 0 on failure
    truncated: bool,
    outcome: String,              // "ok", or the error key's last segment ("too_large", …)
    duration_ms: u64,
},
```

`summary()` gets an English arm. Older builds skip unknown lines. The
viewer row and filter are in §4.5. File names are in this log because it
answers "what did my extensions read?". It stays on this machine and can
be cleared.

### 5.14 Telemetry

Counts only. No names, titles, text or hashes. Added to `docs/PRIVACY.md`
and to `KNOWN_EVENTS` (advisory).

| Event | Props | When |
|---|---|---|
| `extension_file_text` | `extension_id`, `format`, `outcome` (`ok`, `cancelled`, or the error key's last segment), `size_bucket` (`<100KB`, `<1MB`, `<10MB`, `≤32MB`), `chars_bucket` (`<10k`, `<100k`, `<1M`, `1M`), `ms_bucket` (`<500`, `<2000`, `<10000`, `10000+`) | A picked file was handled |

Through `telemetry::track`, a no-op without consent.

### 5.15 Logging

`log::info!` one line per request: extension id, detected format, size
bucket, char count, outcome key, duration. Never the file name, the path,
the title or any text, in any mode. `AppError`'s `Display` substitutes
params (including `{name}`), so code in `src/file_text/` logs `err.key`,
never `{}` of the error. A unit test captures the log and checks for the
fixture's file name.

### 5.16 What is stored where

| Data | Location | Backed up | Synced |
|---|---|---|---|
| The extracted text | Nowhere in the host. Returned to the extension. | — | — |
| What the extension saves | The extension's data | As extension data (§9) | If the extension opts in (§10) |
| `file-ref.key` | `<data_local_dir>/kage/file-ref.key` (owner-only) | No | No (machine-only) |
| Audit entries | `<config_dir>/kage/permission-audit.jsonl` | No (already excluded) | No |
| Last-dialog time, the slot | Memory only | — | — |

No new `Config` field.

## 6. Crates and licences

Kage is Apache-2.0. Only permissive licences (MIT, Apache-2.0, BSD,
Zlib, Unicode) are allowed in the worker's tree. WP-FT0 found one
exception, `option-ext` (MPL-2.0), through `kage-core` → `dirs`, not
from a parser; since WP-FT1 the worker links `kage-core` without `dirs`
(§5.5), so its tree has none. WP-FT0 checked the
planned tree with `cargo deny check licenses` (§6.1).

| Need | Choice | Licence | Notes |
|---|---|---|---|
| ZIP | `zip` (already used, 8.x, `deflate` only) | MIT | Same version as the app. |
| XML | `quick-xml` (already in the lock file) | MIT | Streaming reader, no DTD processing. |
| PDF | `pdf-extract` `=0.12.1`, on `lopdf` 0.42 | MIT | Chosen in WP-FT0 (§6.2). Pin the exact version: the worker uses its public `output_doc_page`, `OutputDev` and `PlainTextOutput`. |
| PDF repair | `lopdf` 0.47, `default-features = false` | MIT | Only when `lopdf` 0.42 can't load a file: 0.47 rebuilds a broken cross-reference table, and the worker saves the result for `pdf-extract` (§6.2). |
| Stream cap | `flate2` (already in the tree through `zip` and `lopdf`) | MIT or Apache-2.0 | Counts a stream's inflated size (§6.2). |
| Right-to-left lines | `unicode-bidi` (new) | MIT or Apache-2.0 | Visual to logical order for PDF lines (§6.2). |
| CJK fonts without a ToUnicode map | Adobe `cmap-resources`: the `Adobe-Japan1-UCS2`, `Adobe-Korea1-UCS2`, `Adobe-GB1-UCS2` and `Adobe-CNS1-UCS2` CMaps, turned into compact tables at build time | BSD-3-Clause | §6.2. The licence text ships with the other notices. |
| HTML | `html5ever` tokenizer | MIT or Apache-2.0 | Already in the lock file through `wry`. |
| Decoding | `encoding_rs` (already in the lock file) | (Apache-2.0 or MIT) and BSD-3-Clause | WHATWG encodings. |
| Encoding detection | `chardetng` (new) | Apache-2.0 or MIT | Firefox's detector. |
| NFC | `icu_normalizer` (already used for backup names) | Unicode-3.0 | |
| HMAC | `hmac` (new, app crate) | MIT or Apache-2.0 | Matches `sha2` 0.10. |
| RTF | in-house (default), or `rtf-parser` | — / MIT | Decided in FT4. |

Rejected:
- `lopdf` alone (0.42 or 0.47) as the PDF extractor: in WP-FT0 it read no
  text from TeX and groff manuals, from text in Form XObjects, or from
  most CJK and Indic files, and it breaks lines only at `T*` and `ET`
  (§6.1).
- `pdfium-render` + PDFium: best PDF quality, but a native C++ library to
  ship and sign per platform. Kept as the upgrade path if users report
  poor PDF text (§14).
- MuPDF bindings (AGPL), Poppler (GPL): licence.
- `docx-rs`, `dotext` and similar: little gained over `zip` + `quick-xml`,
  and less control over limits.
- `html2text`: builds a full DOM; the tokenizer is enough.

### 6.1 WP-FT0: the PDF spike

**Corpus.** 2,438 files. None are checked in.
- 1,016 real-world PDFs: the PDFs shipped with macOS and the installed
  apps on the test machine (licence agreements in 40 languages,
  regulatory labels, manuals, TeX and groff documentation, icons and
  other vector art), and the PDFs in the local cargo registry's test
  fixtures. Deduplicated by hash. 191 of the macOS and app files have
  20 or more words.
- 129 generated documents with known text: the repo's own docs and its
  32 locale catalogs, written by Quartz (CoreText: 1 to 3 columns,
  slides, scans, 2,100 pages, 30 MB with images), Chrome's print to PDF
  (Skia), Inkscape (cairo), the CUPS text filter, PDFKit (filled form
  fields) and `lopdf`'s encryption (RC4 40 and 128, AES 128 and 256; with
  a user password, and with only an owner password that forbids copying).
- 43 hand-written edge cases: bombs (content, image, object stream and
  ToUnicode streams of 0.5 to 1 GiB), cycles (`/Parent`, `/Kids`,
  references, a Form XObject that draws itself), nesting 300 and 100,000
  deep, 2,001 and 20,000 pages, broken, missing and shifted xref tables,
  truncation, garbage, unusual filters, WinAnsi, `/Differences`, Type3,
  Type0 with and without ToUnicode, a predefined CJK CMap, a page of 6
  million characters, an invisible OCR layer, a rotated page.
- 1,250 byte-level mutants (flips, inserts, deletes, truncation, splices,
  bad numbers and brackets) of 25 seeds, 5 of them uncompressed.

**Method.** One process per file and candidate, set up as the worker
will be: the parser on a 64 MiB stack, a counting allocator capped at
768 MiB, a 30-second deadline from the parent, `catch_unwind` per page,
at most 1,000,000 characters and 2,000 pages. Peak heap comes from the
allocator, peak RSS from `wait4`. Text quality is the F1 score of word
counts (characters for CJK and Thai) after NFKC and lower case: against
the source text for generated files, against macOS PDFKit's text for
real-world files. Apple M3 Pro, four runs at a time. Times include
process start (about 8 ms).

Candidates: `pdf-extract` 0.12.1 set up as in §6.2 ("worker"); its
whole-document `extract_text_from_mem`; `lopdf` 0.42 and 0.47 alone
(`extract_text` per page; 0.47 with its 32 MiB decompression limit).

**Robustness, time and memory** (1,188 files, without the mutants)

| | Worker | Whole document | `lopdf` 0.42 | `lopdf` 0.47 |
|---|---|---|---|---|
| Read | 1,164 | 1,134 | 1,145 | 1,164 |
| Refused: user password | 13 | 13 | 13 | 13 |
| Other load errors | 10 | 29 | 29 | 11 |
| Lost to a panic | 8 pages | 8 files | 0 | 0 |
| Killed (abort or deadline) | 1 | 4 | 1 | 0 |
| Time, median / p95 / max | 13 / 37 / 854 ms | 13 / 36 / 3,052 ms | 13 / 25 / 14,013 ms | 13 / 25 / 11,338 ms |
| Peak heap, p95 / max | 15 / 114 MiB | 15 / 511 MiB | 15 / 511 MiB | 15 / 121 MiB |

The 8 panics are all in hand-written files: a missing font or font
resources, absurd matrix numbers, deep arrays in a content stream, a
reference cycle, a predefined CJK CMap, and the ToUnicode bomb after the
cap emptied it. No real-world or generated file made `pdf-extract` panic.
The one worker kill is the Form XObject that draws itself: the stack
overflows after 68 ms, which the parent reports as `too_complex`. The
other load errors are icons and broken files with no text.

Large files, worker, peak heap at most 114 MiB:

| File | 100,000 chars (default) | 1,000,000 chars |
|---|---|---|
| Quartz, 2,100 pages, 12 MB | 0.2 s (30 pages) | 0.5 s (280 pages) |
| Chrome, 5.8 MB | 0.2 s (28 pages) | 0.8 s (321 pages) |
| Quartz, 13 pages with images, 30 MB | 40 ms (all 37,000 chars) | 40 ms |
| 20,000 tiny pages, 6 MB | 0.4 s (2,000 pages) | 0.4 s |
| One page of 6 million characters | 70 ms | 170 ms |

**Text quality** (F1 on generated documents, Quartz / Chrome)

| Script | `pdf-extract` | `lopdf` 0.47 | PDFKit |
|---|---|---|---|
| English (11 docs each) | 1.00 / 0.97 | 0.99 / 1.00 | 1.00 / 0.97 |
| Latin, Cyrillic, Greek (21 locales each) | 1.00 / 0.98 | 0.87 / 0.99 | 1.00 / 1.00 |
| Chinese | 1.00 / 1.00 | 0.12 / 0.10 | 1.00 / 1.00 |
| Japanese, Korean | 0.00 / 1.00 | 0.05 / 0.06 | 1.00 / 1.00 |
| Hebrew, Arabic, Persian, Urdu | 0.09 / 0.08; 0.69 / 0.46 after the RTL pass | 0.11 / 0.09 | 0.76 / 0.99 |
| Hindi, Bengali | 0.75 / 0.81 | 0.05 / 0.06 | 0.75 / 0.81 |
| Thai | 1.00 / 1.00 | 1.00 / 1.00 | 1.00 / 1.00 |

On those 191 real-world files, against PDFKit: `pdf-extract` 0.95 (25
files under 0.9), `lopdf` 0.81 (101 under 0.9). `lopdf` read
nothing from the TeX and groff manuals. `pdf-extract`'s low scores are
label layouts read in a different order, and Apple's Japanese, Korean,
Hebrew and Arabic licence PDFs (the gaps below).

**Mutants** (1,250 files)

| | Worker | Worker without repair | Whole document | `lopdf` 0.47 |
|---|---|---|---|---|
| Load errors | 170 | 850 | 850 | 152 |
| Files with text | 735 | 324 | 260 | 730 |
| Panics | 269 files lost a page | 95 files lost a page | 97 files lost | 0 |
| Killed (abort or deadline) | 0 | 0 | 0 | 0 |
| Slowest | 0.6 s | 45 ms | 41 ms | 41 ms |

**Hand-checked files** (23, worker output against the source or PDFKit):
- Five English documents (Chrome and Quartz reports, 2 and 3 columns,
  slides): exact text, columns in reading order, a blank line between
  paragraphs, soft line breaks kept, bullets kept as `•` where the PDF
  draws them as text.
- A groff manual (Type 1 fonts): exact. `lopdf` gave nothing.
- A regulatory label: all text, in another order than PDFKit's.
- Thai (Quartz), CUPS text, cairo (Inkscape, and a spec from the cargo
  registry), WinAnsi accents, `/Differences`: exact. CUPS pads lines with
  spaces (§5.10 trims them). cairo and Chrome keep `ﬁ` as one code point.
- Hindi (Quartz): vowel signs and repha in visual order, as in PDFKit.
- Japanese and Chinese (Chrome): right text, but some ideographs come as
  Kangxi radicals (`⾳` for `音`) and some Latin words are split
  (`Ka ge`).
- Japanese (Quartz) and Apple's Japanese licence: no text at all.
- Hebrew (Quartz): visual order; right after the RTL pass.
- Arabic (Chrome): visual order, one word per line, spaces inside words.
- AES-256 with an owner password only: same text as the plain file;
  `restricted` detected.
- A PDFKit form: page text exact; the field values are not read (by any
  candidate, nor by PDFKit's page text).
- A Quartz scan: no text, so `no_text`.

**What `pdf-extract` 0.12.1 offers, confirmed**
- Per-page API: `output_doc_page(&doc, &mut dyn OutputDev, n)` and
  `extract_text_from_mem_by_pages`. Both call `Document::get_pages()` for
  each page, which walks the whole page tree: 2,000 pages of a
  20,000-page file took 14.3 s. With the root trick in §6.2 it takes
  0.4 s.
- `PlainTextOutput::new` takes any `ConvertToFmt`, so a writer can refuse
  text past `max_chars` and stop a page early.
- `OutputDev` is a public trait: `begin_page` and `end_page` mark pages,
  and `output_character` gets each glyph with its position, so a custom
  layout is possible.
- No stream cap. `pdf-extract` pins `lopdf` 0.42, which has none (0.47 has
  `max_decompressed_size`). `LoadOptions::filter`, a `fn` called on each
  object while loading, lets the worker set one (§6.2). `lopdf` 0.42 skips
  the filter for encrypted files.
- Without the steps in §6.2: a `/Parent` cycle loops until the deadline;
  a missing `/MediaBox` panics; image XObjects are inflated on `Do` (a
  900 MiB image bomb took 454 MiB); a 1 GiB content bomb and a 900 MiB
  ToUnicode bomb hit the allocator cap.
- Encryption: `lopdf` 0.42 opens RC4 40 and 128 and AES 128 and 256
  (revisions 2 to 6). The empty user password opens owner-only files at
  load; `encryption_state.permissions()` without `COPYABLE` means
  `restricted`. With a user password, `is_encrypted()` stays true, so
  `encrypted`.
- `Document::load_mem` parses every object first, so at the default
  100,000 characters, load time is most of the cost.
- `pdf-extract` 0.12.1 builds against `lopdf` 0.47 with a one-line change
  (`get_page_content` no longer returns a `Result`). A release on a newer
  `lopdf` would make the repair step and the load filter unnecessary.

**Licences.** `cargo deny check licenses` (cargo-deny 0.20.2) on the
planned worker tree: `kage-core`, `serde`, `serde_json`, `zip`
(`deflate`), `quick-xml`, `encoding_rs`, `chardetng`, `icu_normalizer`,
`html5ever`, `unicode-bidi`, `flate2`, `libc`, `windows`, and each PDF
candidate. All targets. It passes with this configuration:

```toml
[licenses]
allow = ["MIT", "Apache-2.0", "BSD-3-Clause", "Zlib", "Unicode-3.0"]
exceptions = [{ name = "option-ext", allow = ["MPL-2.0"] }]
[licenses.private]
ignore = true
```

The exception is the one MPL-2.0 crate, through `kage-core` → `dirs` →
`dirs-sys`. `pdf-extract`'s own 78 crates are MIT, Apache-2.0,
BSD-3-Clause (`encoding_rs`), Zlib (`zlib-rs`) and Unicode-3.0;
`lopdf` 0.47 adds BSD-3-Clause (`brotli-decompressor`, `alloc-stdlib`).
FT1 dropped the exception: `dirs` is behind `kage-core`'s `os` feature,
which the worker turns off.

### 6.2 Decision: `pdf-extract`

`pdf-extract` 0.12.1 reads the most text by a wide margin (font
encodings, CID fonts, Form XObjects), and no real file made it panic.
Its weak points can all be handled in the worker without a fork. FT3
builds these into `kage-file-text/src/pdf/`:

1. **Cap streams while loading.** Load with
   `LoadOptions { filter: Some(cap_streams), .. }`. The filter empties
   image XObjects (text never needs them, and `pdf-extract` inflates them
   on `Do`) and empties any `FlateDecode` stream whose inflated size,
   counted through a capped decoder, passes 32 MiB. For anything that
   isn't a stream it returns a clone of the object: objects from object
   streams take the returned value. For a file that was encrypted, run
   the same function over `doc.objects` after loading. The bombs then
   cost at most 5 MiB and 20 ms. The allocator cap stays the backstop
   (object streams inside encrypted files).
2. **One page at a time, cheaply.** Call `get_pages()` once. Before each
   page, set the root `/Pages` node's `/Kids` to that page and `/Count`
   to 1, then call `output_doc_page(&doc, out, 1)` inside
   `catch_unwind`. A panic loses only the rest of that page.
3. **Check each page first.** Walk `/Parent` with a visited set, at most
   64 steps; a cycle skips the page. A page with no inherited
   `/MediaBox` gets `[0 0 612 792]`. A document whose pages were all
   skipped or all panicked is `malformed`.
4. **Caps while writing.** Give `PlainTextOutput` a writer that refuses
   text past the remaining `max_chars`; its error means `truncated`.
   Record `page_starts` between pages.
5. **Repair.** If `lopdf` 0.42 can't load the file, load it with `lopdf`
   0.47 (`max_decompressed_size` 32 MiB), save it to memory, and load that
   with 0.42. Take `restricted` from the 0.47 load. On the mutants this
   raised the files with text from 324 to 735.
6. **Fix the text, PDF only, before §5.10.**
   - On each line with right-to-left letters, put the right-to-left
     runs and the run order back into logical order, with
     `unicode-bidi` for the levels. Measured with a simple run reversal:
     Hebrew 0.10 → 0.95, Persian 0.08 → 0.94 and Arabic 0.15 → 0.81
     (Quartz).
   - Apply NFKC to Latin ligatures (U+FB00–U+FB06), Arabic presentation
     forms (U+FB50–U+FDFF, U+FE70–U+FEFF) and Kangxi radicals
     (U+2F00–U+2FDF) only. Chrome writes `ﬁ` and `⾳`, NFC keeps them, and
     a search for "file" or `音` would miss them.
7. **CJK fonts without a ToUnicode map.** Quartz embeds Hiragino and
   Apple SD Gothic Neo as CID fonts with the Adobe-Japan1 or Adobe-Korea1
   ordering and no ToUnicode map. Apple's own Japanese and Korean licence
   PDFs are made this way, and neither crate reads them. For a Type0 font
   without `/ToUnicode` whose `/CIDSystemInfo` names Adobe-Japan1,
   Adobe-Korea1, Adobe-GB1 or Adobe-CNS1, the worker adds a ToUnicode
   stream built from that ordering's table, for all CIDs (`/W` lists only
   glyphs with a non-default width). The spike injected such a stream
   and `pdf-extract` used it.
8. **Gaps in P0.** Not read: form field values (they live in widget
   annotations, not in page content); text in other annotations; text in
   non-embedded fonts with a predefined CJK CMap such as `UniJIS-UCS2-H`
   (`pdf-extract` panics and the page is lost). Weak: Indic vowel signs
   and repha stay in visual order, as in PDFKit. Chrome's Arabic and Urdu
   come out one word per line with spaces inside words (F1 0.25 and 0.23
   after the RTL pass), because `PlainTextOutput`'s spacing rules misread
   Chrome's glyph positions. Urdu Nastaliq from Quartz has no usable
   ToUnicode map (PDFKit fails too). A custom `OutputDev` layout is the
   fix for the spacing (§14).
9. **Dependencies.** FT1 adds `pdf-extract = "=0.12.1"`,
   `lopdf = { version = "0.47", default-features = false }`, `flate2` and
   `unicode-bidi` to `kage-file-text/Cargo.toml`. FT3 adds the Adobe CMap
   tables under `kage-file-text/src/pdf/`.

## 7. Security analysis

Against `docs/SECURITY_MODEL.md`. The sandbox, the capability check at
the port, identity injection and the fail-closed command table all apply
unchanged. The new surface is three commands under one new capability.

| Threat | Mitigation | Residual |
|---|---|---|
| An extension reads files the user didn't choose | No path input exists. Rust opens only what the OS dialog returns. No remembered grants, no folder listing, no "read again". | None known. |
| An extension learns the user name or folder layout | The path is never returned. `file_ref` is a keyed hash, different per extension. Names are cleaned. | The file name itself can be revealing ("Divorce lawyer.pdf"). The user chose to give it. |
| Two extensions link what they each read | `file_ref` is keyed per extension. `sha256` matches for the same content, but each extension already has the text. | — |
| Dialog spam, or a dialog opened by the agent through a tool | Dialogs open only after a host-seen click (one per click) or a click on the host's ask bar. One request app-wide; 3 s between dialogs; 30 s after **Not now**. | An extension can open the dialog after any of its own buttons is clicked. The title names it. |
| Phishing text in the dialog | The title is a fixed string with the manifest name. No extension text. | A misleading extension name, the same as in the install prompt. |
| Hidden window pops up a dialog | `no_window` in hidden windows. The launcher is never opened for this. | — |
| A crafted file exploits a parser | Pure-Rust parsers (no C), in a separate process with a 768 MiB allocator cap, OS memory and CPU limits, a 30 s deadline, no file access and no network need, a fixed `--v1` protocol, capped output. A crash is an error message. | A logic bug can still return wrong or partial text. Fuzzing (§12) lowers the odds. |
| ZIP bombs, overlapping entries | Only named parts are inflated; per-part and total caps counted while inflating; entry count cap. | — |
| XML attacks (XXE, entity expansion, deep nesting) | `quick-xml` doesn't resolve DTDs or external entities; depth cap. | — |
| PDF bombs (huge streams, object loops, deep nesting) | Page cap, a 32 MiB stream cap set while loading, a `/Parent` cycle check (§6.2), allocator cap, big worker stack plus depth limits, deadline. | Lost work on hostile files only. |
| Huge output floods IPC or storage | `max_chars` ≤ 1,000,000; worker stdout cap; the extension stores what it chooses. | — |
| FIFO, device or directory chosen | Regular-file check, `O_NONBLOCK` open with a second check on Unix, size check before reading. | — |
| A file still syncing from the cloud hangs Kage | The read runs off the async runtime with a 60 s timeout. The single slot is released. | The abandoned read thread finishes on its own. |
| Hidden instructions for AI in the text (prompt injection) | Unicode tag characters and bidi overrides removed; hidden DOCX/HTML text skipped. The text is still untrusted: extensions must frame it as data in prompts (Mind Map does) and escape it in HTML (the host sanitizer is a second layer). The agent calls design refuses tool use on background calls. | White-on-white or tiny text can't be detected. |
| Credentials in a chosen file | `possible_secret` from the same `SECRET_SHAPE` rules as backups. Mind Map makes such items private, and keeps their text off the sync folder (§10). | Detection is heuristic. |
| A planted worker binary | Only the sibling of the running executable, signed in the bundle on macOS. `env_clear()`. | Someone who can write to Kage's install folder already controls Kage. |
| Capability granted through a backup or sync | Grants are never imported (`config_export/consent.rs`) and never synced (SYNC §11.1). | — |
| File names in logs | Never in `app.jsonl`. Only in the local, clearable audit log. | — |

Also noted, outside this design: the settings `pick_file` host effect
(`settings-host-effects.js`) reads a whole file of any size as text and
hands it to the extension without a capability. The user's pick is the
consent there too, but it has no size cap. A separate small fix should cap
it at a few MB.

`docs/SECURITY_MODEL.md` gets: `file_text` in the capability list; a short
section "Files the user picks" (no paths, the worker, the click lease, the
ask bar); and these rows in "Summary of current guarantees": "Extension
reads a file the user didn't pick", "A crafted document crashes or
hijacks Kage", "A dialog opens without a user action".

## 8. Cross-platform notes

**Windows**
- The dialog is `IFileOpenDialog` through `tauri-plugin-dialog` (rfd).
  Owned by the calling window except for the launcher (§5.3).
- OneDrive "Files On-Demand": a file with
  `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` or `FILE_ATTRIBUTE_OFFLINE` is
  downloaded on first read. Kage emits `downloading` for it.
- Nested Job Objects work on Windows 8 and later. The worker sits inside
  Kage's kill-on-exit job and its own limit job.
- Antivirus may scan the file on open and add a delay. The 60 s read
  timeout covers it.
- The worker ships like the MCP sidecar: the NSIS installer includes
  `externalBin` entries automatically, and release signing (where
  configured) covers it with the other binaries.

**macOS**
- `NSOpenPanel` through rfd. rfd sets the title as the panel's message,
  which macOS shows. Sheets on chat, settings and view windows.
- Kage reads the file in its own process: picking it in the panel lets a
  non-sandboxed app read it without a Files and Folders prompt, even in
  Desktop, Documents, Downloads or iCloud Drive. The worker only gets
  bytes, so it needs no such access.
- iCloud Drive with "Optimise Mac Storage": a dataless file
  (`SF_DATALESS` in `st_flags`) downloads on read. Kage emits
  `downloading`.
- Packages (`.rtfd`, `.pages`) are folders and are refused as
  `not_a_file`; the filter usually hides them anyway.
- File names come back decomposed (NFD) from HFS+ volumes; §5.10 turns
  them into NFC.
- The worker lives in `Contents/MacOS` and is covered by the app's
  signature and notarisation, with the same hardened-runtime entitlements
  as the other sidecars. It needs none of them.

**Linux**
- GTK 3 file chooser through rfd, transient for the calling window. In
  Flatpak or Snap builds the desktop portal may serve the dialog; it
  returns a `/run/user/<uid>/doc/…` path that Kage can read. Paths are
  never returned, so this difference doesn't reach extensions.
- The GTK chooser accepts typed paths, so the extension check in §5.4
  matters here.
- `PR_SET_PDEATHSIG` and `RLIMIT_AS` work as described. AppImage, deb and
  rpm builds place `externalBin` next to the main binary, where §5.5
  looks.

**All platforms**
- File names in any script work; they are NFC and cleaned.
- No paths cross the API, so per-OS path handling never reaches
  extension data or sync.

## 9. Backups (`src/config_export*`)

- **No new config field.** `allowlist.rs` needs no rule, and
  `every_config_field_has_a_decision` is unaffected.
- **The grant.** `file_text` appears in `extension_grants[id].granted`
  like any capability. The settings-only allowlist checks capability names
  against `VALID_CAPABILITIES`, so it accepts `file_text` once that list
  has it. Imports never take grants from a backup (`consent.rs`). An
  extension the backup has on whose manifest asks for `file_text` stays off
  until approved on this machine. On an older Kage, a backup grant naming
  `file_text` fails the shape check and keeps that machine's value.
- **Extracted text the extension saved** is extension data: in encrypted
  full backups, never in settings-only backups. A full backup refuses an
  entry over 64 MB and more than 256 MB in all, so extensions that store
  documents need a budget. Mind Map caps document text at 64 MB in all
  (§11.8).
- **Document text and the credential rules.** We deliberately do not tell
  extensions to declare document text in `sensitiveData`. A declared file
  that the backup doesn't replace makes an import keep all of that
  extension's data and settings whole, so one secret-looking document would
  block every Mind Map restore. Document text is user content, protected by
  the backup's encryption.
- **Not in any backup**: `file-ref.key` and the audit log (device-local).
- None of the four backup commands become extension-callable.

## 10. Interaction with folder sync

Folder sync is specified in [`SYNC.md`](SYNC.md). The open choices this
section used to leave to it are now decided there.

| Item | Sync rule |
|---|---|
| `extension_grants` (including `file_text`) | This computer, as for backups. Each computer approves capabilities itself (SYNC §11.1). |
| `file-ref.key`, audit log | Not settings; never synced (SYNC §4.2). |
| What the extension saved (Mind Map documents) | Synced when the extension opts in, field by field, newest wins (SYNC §12.6). |

Consequences:

- **Everything in the sync folder is encrypted and authenticated**
  (AES-256-GCM with a key wrapped by a sync passphrase, SYNC §11). So
  document text is never in plain text in a cloud folder, for Mind Map or
  any other extension. No per-key exclude list is needed.
- **Text that looks like a credential stays on this computer anyway.**
  Sync never publishes credentials, even encrypted (SYNC §11.5). A
  document whose result carries `possible_secret` is treated the same
  way: Mind Map pushes neither its item nor its body, because the item's
  `text` holds the first 8,000 characters and may hold the secret too.
  The document exists only on the computer that read it.
- **Reading synced documents needs no `file_text`.** Computer B shows and
  searches Mind Map documents captured on computer A with only `storage`.
- **`file_ref` doesn't travel.** It is keyed per computer and per
  extension, so Mind Map keeps it in a field sync never sends (`fileRefs`,
  §11.4). Extensions use `sha256` to spot the same content on another
  computer (Mind Map does, §11.4).
- **No paths, so nothing per-OS.** A document captured on Windows is
  the same record on macOS.
- **Size.** Bodies are a `large` collection: up to 4 MiB per record,
  which covers `doc_max_chars` = 1,000,000. The per-extension sync quota
  (128 MiB) covers Mind Map's 64 MB body budget. A body that loses a
  conflict is kept whole in the conflicts list (SYNC §10.1). Mind Map
  writes each body once per revision (§11.4), so conflicts on bodies are
  rare.

## 11. Mind Map integration

This section says exactly what Mind Map does with the API. It replaces
the "Assumed" answer to OPEN_QUESTIONS Q4 (documents as references only)
and the "Document text ingestion" entry in SPEC §19. The agent route (C6:
attach a file in chat and ask the agent) stays.

Phases: **F0** ships with host P0; **F1** with the explorer and host P1;
**F2** with background organising (agent calls design).

### 11.1 Manifest, defaults, settings

```json
"permissions": ["storage", "urls", "network", "windows", "file_text"],
"config": {
  "doc_max_chars": { "type": "number", "default": 200000, "enum": [50000, 200000, 1000000] }
}
```

- `file_text` triggers the upgrade prompt for existing users. Without the
  grant, everything works as today and the file rows are hidden.
- `doc_max_chars`: "Keep up to … characters of each file". An `enum`, so
  settings-only backups may copy it.
- No document sync switch: sync encrypts everything, so bodies sync with
  the rest of Mind Map's data (§10, §11.8).
- Settings page: a **Documents** section with the action **Add a
  file…**, the setting, and "Document storage: {used} of 64 MB".
- Disclosure (settings and README) gains: "When you add a file, Kage
  reads it on this computer and gives Mind Map its text. Mind Map keeps up
  to {n} characters of it with your notes. Kage never tells Mind Map
  where the file is."
- `tools.js`, `search.js` and `settings.js` carry the hint comment
  `// permissions: file_text` where they call `context.files`.

### 11.2 Where the user adds a file

All four are clicks the host sees (C11 and C12 in the explorer through
the view rule of §5.2), so the dialog opens at once.

| # | Surface | Flow |
|---|---|---|
| C9 | Launcher `mm+` (empty) | A row "📄 Add a file…" (id `mm:file`), subtitle "Text, Markdown, PDF or Word. Kage reads it on this computer." Also for `mm+ file` and `mm add file`. `execute()` runs §11.3 and returns `display` with an op card. |
| C10 | Settings → Documents → **Add a file…** | `runSettingsAction` runs §11.3, returns `{ status }`. |
| C11 | Explorer toolbar **Add a file…** (F1) | A button in the explorer's own toolbar, inside the view. The click counts as a host-seen click through the view rule of §5.2. The new doc is selected and centred. |
| C12 | **Update from file…** on a doc's card or explorer pane (F1) | Picks again. The text replaces that doc's body (§11.4, case U). |

Agent tools never open the picker. The `capture` tool and the C6 route
are unchanged.

### 11.3 The request

```js
const support = await context.files.support();            // cached per session
const formats = support.formats.map((f) => f.id);          // F1 gains html/pptx/rtf with no code change
const file = await context.files.pickText({ formats, maxChars: config.doc_max_chars });
if (!file) return null;                                     // cancelled: no card, no write
```

- Errors: show `err.message` (already in the user's language) on the
  op card or settings status. `too_large` adds "Try a smaller copy, or
  save part of it as a PDF." `no_window` can't happen from these entry
  points.
- Validate the result before use (types, lengths, `chars` ≤ `maxChars`,
  `sha256` is 64 hex chars, `file_ref` matches `^fr1_[A-Za-z0-9_-]{22}$`).
  An invalid result is treated as an error, not stored.

### 11.4 Ingest: dedupe, item, body

New pure module `lib/files.js` decides; `mind.js` applies under the
existing lock (`store.withLock`, SPEC §7.4).

Dedupe, first match wins:

| Case | Condition | Result |
|---|---|---|
| S | An item has this `file_ref` in `fileRefs` and the same `sha256` | No write. Card: "Already in your mind map (added {date})". |
| U | An item has this `file_ref` in `fileRefs` and another `sha256`, or the user chose **Update from file…** | Re-read: new body, item re-derived, op `reread`. |
| C | An item has this `sha256` (another computer, a copy) | No new item. Add `file_ref` to its `fileRefs` (at most 8, oldest dropped). Card: "Already saved as '{title}'". |
| N | Otherwise | New doc item, op `ingest`. |

New item (fields added to SPEC §6.2):

```
Item {
  kind: 'doc', title, titleSource: 'meta' | 'auto',   // file.title if usable, else the name without extension
  text,                // the first 8,000 chars of the body (existing cap), truncated: true when longer
  source: { via: 'file', app?, window? },             // app/window per record_source_window (default on)
  file: { name, ext, format, size, modifiedAt, sha256,
          pages?: {count, read}, chars, truncated, warnings },
  fileRefs: [file_ref],                               // this computer only: never synced (§10)
  body?: { key, chars },                              // only when the text is over 8,000 chars
  private?: true,                                     // possible_secret or new_items_private
  ...the usual fields
}
```

- "Usable title": non-empty, not equal to the file name, and not a
  generator default (Mind Map strips a leading "Microsoft Word - ").
- No `path` field. The C5 flow for paths the user types stays as it is.
- `fileRefs` is a top-level field so sync can leave it out (SYNC §12.9
  lists it under `local`). A `file_ref` means nothing on another computer,
  and keeping it out of `file` means two computers adding the same
  document never conflict over it.
- Body key: `mm-body-${id}-${rev}`. Value:
  `{ schema: 1, id, rev, format, chars, text }`. A literal-prefix key, so
  `check:security` resolves it.
- Write order inside the lock: body first, then the shard, journal and
  meta. A crash after the body write leaves an unreferenced body. Item ids
  come from the replayable id pool, so a retried capture rewrites the same
  key. (Extensions can't list their keys, so an orphan from an abandoned
  capture stays; it is bounded by one body.)
- Hosts with `files.pick_text` always have `storage.atomic_write`, so
  bodies are written once, without the twin.
- New journal op kinds: `ingest` and `reread`. `reread` keeps the old body
  key in its `before` snapshot. Undo puts it back.

### 11.5 Organising a document

- `extract()` runs on the item's `text` (the first 8,000 chars), as for any
  capture: kinds, people, teams, projects, tags, acronyms, dates.
- Keyphrases come from the **whole** body: term frequencies over
  `lib.text.tokenize`, stoplist applied, top 64, stored in
  `item.keyphrases` (still "rule, re-derivable"). Headings (lines starting
  with `#`) add their terms with double weight. Budget: ≤ 60 ms for
  200,000 chars.
- Actions found in a document are always **suggestions**, even explicit
  `TODO:` and `[ ]` lines, because someone else may have written the
  document. Unassigned ones are owned by "me"; "<Name> to …" by Name. They
  appear in `mm review` with the quote and a link to the doc. Mind Map
  actions stay separate from Todos.
- `possible_secret` → the item is private (§11.7).

### 11.6 Search, snippets and the explorer

- Bodies are **not** loaded by `ready()`. The index uses title, entities,
  summary, `text` and the body keyphrases, so the load and memory budgets
  in SPEC §17 hold.
- An LRU of 8 bodies per window loads them on demand, through
  `load_extension_data`.
- Search rows for docs: "📄 Q3 plan.pdf · 12 pages · added Oct 26". When
  the hit is only in body keyphrases, the row loads the body and shows the
  best matching passage (≤ 160 chars).
- Explorer (F1): doc nodes show the 📄 icon, the name and the page count.
  The detail pane shows metadata and **Show text**, which loads the body
  and pages through it 10,000 characters at a time. `page_starts` gives
  "p. 4" labels for PDF and PPTX.

### 11.7 Agent paths and privacy

- **Tools** (on by default): `search` and `get` return doc metadata
  (`name`, `format`, `pages`, `chars`), the summary, and up to 3 passages of
  ≤ 400 chars that match the query (from the body, loaded on demand),
  within the 8 KB result cap. Never the whole body.
- **Private** documents are never sent, as for every private item.
- **`possible_secret`**: the item is saved as private and the card says:
  "This file looks like it contains a password or key, so it's private:
  your agent never sees it." with **Let my agent see it** (an undoable
  `privacy` op). With sync on it adds "It stays on this computer."
  (`files.card.text_kept_local`). **Let my agent see it** changes privacy
  only; the document still never syncs, because the warning is about the
  text, not about who reads it.
- **Background organising** (F2, off by default): a doc is eligible like a
  note. Its payload is the title, headings and the first 600 chars, framed
  as data. Never private or `possible_secret` docs.
- Window titles stay local, as today.

### 11.8 Undo, delete, export, budgets, backups and sync

- **Undo**: `ingest` → delete the item and its body key. `reread` → the
  previous body key and fields come back; the new body key is deleted.
- **Delete**: the body key stays while the delete op can still be undone.
  When the journal drops an op (200 ops / 256 KB), bodies only that op
  referenced are deleted.
- **Export JSON**: an optional `bodies: { [itemId]: text }` map. Schema
  stays 1 (additive; Import ignores unknown fields). Export Markdown: a
  "Documents" section with title, file name, pages and summary.
- **Budget**: `meta.counts.bodyBytes` (UTF-8 bytes of all bodies). New
  documents are refused above 64 MB with "Mind Map's document storage is
  full (64 MB). Delete some documents first." Settings warns at 90%. This
  keeps a full backup well under its 256 MB limit.
- **Backups**: bodies are extension data (encrypted backups only, §9).
  Mind Map doesn't add `sensitiveData`.
- **Sync**: bodies sync, encrypted, with the rest of Mind Map's data, as
  the `bodies` collection (SYNC §12.9). Two exceptions keep a document on
  this computer: its `file.warnings` include `possible_secret` (neither
  the item nor the body is pushed), or the item is private while "Sync
  private items" is off. `mm-lock` and
  the other bookkeeping keys never sync (SYNC §12.9). On another computer,
  a doc whose body isn't there (still downloading, or kept local) shows
  "The full text is on another of your computers" and searches by title,
  summary and keyphrases. Sync never gives extensions computer names
  (SYNC §12.6), so the message names none.
- **Uninstall**: the host deletes all local data, bodies included, as
  today. With sync on, the copy in the sync folder and on the other
  computers stays (SYNC §12.8).

### 11.9 What the user sees in Mind Map

- Launcher and settings rows from §11.2.
- Op card after a capture: "Added Q3 plan.pdf: 12 pages, 48,211
  characters. Found 3 people, 2 projects and 4 suggested actions."
  Truncated: "Kept the first 200,000 characters." `no_text`: "No text
  found. It may be a scanned PDF. Saved the file name only." `pages_skipped`:
  "Read the first 2,000 pages."
- New `_locales/en/messages.json` keys (EN first, then `translate.py`):
  `files.row.add`, `files.row.add_sub`, `files.card.added`,
  `files.card.truncated`, `files.card.no_text`, `files.card.pages_skipped`,
  `files.card.same`, `files.card.copy_of`, `files.card.updated`,
  `files.card.secret`, `files.action.reveal_to_agent`,
  `files.action.update`, `files.error.too_large_hint`,
  `files.error.storage_full`, `settings.documents.title`,
  `settings.documents.add`, `settings.documents.max_chars`,
  `settings.documents.usage`, `settings.disclosure.files`,
  `files.card.text_elsewhere`, `files.card.text_kept_local`.

### 11.10 Mind Map tests

- `lib/files.test.js`: result validation (bad types, oversized, bad hex,
  bad ref); dedupe cases S, U, C, N; title choice; keyphrases from a body
  (determinism, budget); passage finder; the `fileRefs` cap.
- `model.test.js`: `ingest` and `reread` apply and undo deep-equal;
  doc-derived actions are suggestions; `possible_secret` → private.
- `mind.test.js` with the fake host (a `pick_file_text` stub): body-first
  write order; crash after the body write; budget refusal; lazy body
  LRU; another window sees the doc after `syncIfStale`; delete keeps the
  body until the op leaves the journal.
- `tools.test.js`: passages within 8 KB; private and secret docs hidden.
- `search.test.js`, `settings.test.js`, `view.test.js`: rows only with the
  host feature; cancel produces nothing; error statuses; the card strings.
- With an older host (no `files.pick_text`), Mind Map behaves exactly as
  before.

## 12. Host test plan

**Worker crate (`kage-file-text`, `cargo test -p kage-file-text`)**
- Fixtures are built in the tests: DOCX and PPTX with the `zip` crate,
  small PDFs from hand-written bytes, text in several encodings. No
  third-party documents are checked in.
- Per format: the text conventions in §3.7, titles, `page_starts`,
  truncation at each boundary type, `total_chars`.
- Normalisation: each removed character class, NFC, line endings, blank
  line collapse, scalar-value counting.
- Detection: every row of §5.8, `wrong_content` for each mismatch, a
  binary renamed `.txt` → `not_text`, CFB with and without
  `EncryptedPackage`.
- Limits: a high-ratio ZIP entry, 20,001 entries, a part that inflates past
  its declared size, 300-deep XML, a PDF with 2,001 pages, a Flate bomb
  stream, a broken xref, a user-password PDF, an owner-password PDF
  (`restricted`), 300-deep RTF groups, `\bin` past the end.
- PDF cases from WP-FT0 (§6.1, §6.2): bombs in a content, image, object
  and ToUnicode stream, and one in an encrypted file; 20,000 pages (the
  first 2,000 read in under a second); a `/Parent` cycle; a Form XObject
  that draws itself (`too_complex`); a page without `/MediaBox`; an xref
  only the repair step can read; a Hebrew page; a CID font without a
  ToUnicode map; Chrome's `ﬁ` and Kangxi radicals.
- The allocator cap aborts a large allocation; `catch_unwind` maps a panic
  to `malformed`; the stack guard thread survives deep but legal input.
- A local corpus test, skipped unless `KAGE_FILE_TEXT_CORPUS` points at a
  folder: every file finishes within the deadline, never crashes the test
  process, and output is within caps. Used by FT0 and for regressions.
- Fuzzing: `cargo fuzz` targets `text`, `ooxml`, `pdf`, `html`, `rtf`,
  seeded from the fixtures. Invariants: no panic escapes, output within
  caps, each input under the deadline. A nightly CI job runs each for a
  few minutes. The targets call `extract_as`, which skips the
  `READS_P1_FORMATS` check, so FT4 can fuzz HTML and RTF before FT8
  switches them on. `fuzz/` is its own workspace; `cargo fuzz build -s
  none` works on stable Rust.

**App crate**
- Unit (`src/file_text/*`): args validation; grant and enabled checks;
  single slot and the 3 s gap; cancel during each phase; `file_ref`
  stability, per-extension difference, `fr1_` shape; key file created
  owner-only and replaced if malformed; name and display-name cleaning;
  regular-file checks (a FIFO and a directory on Unix); size refusal
  without a read; the error mapping from every `WorkerErrorCode` and exit
  kind; no file name in captured logs; `possible_secret`.
- A `DialogSource` trait (real dialog in production, a fixed path in
  tests) keeps the dialog out of tests.
- Integration `tests/file_text_test.rs` (calls `kage::paths::isolate`
  first): spawn the real worker built by `build.rs` on fixture files; a
  hanging worker (debug-only `--v1-test-hang` flag) is killed at the
  deadline; a crashing one (`--v1-test-abort`) gives `too_complex`;
  oversized stdout (`--v1-test-flood`) gives `worker_failed`. These
  flags exist only with `debug_assertions`. The worker crate's own
  process tests (`kage-file-text/tests/worker.rs`) also use
  `--v1-test-limits` (prints the limits in effect before stdin is read),
  `--v1-test-oom`, `--v1-test-panic` and `--v1-test-deep`.
- Existing gates: `command_error_type_parity_test` (all three return
  `Result<_, AppError>`), `command_permissions_parity_test` (the JS table
  has them), `user_dirs_isolation_test` (the key goes through
  `crate::paths`), the mock-app sweep (empty args error out, no dialog).
- `scripts/check_module_size.py`: every new file under 500 lines.

**JS (`ui-tests`, vitest)**
- `extension-file-text.test.js`: lease granted by each `userGesture` call
  site and consumed once; expired after 10 s; no lease from `match`,
  `getTools` or tool calls; the ask bar (choose, not now, Escape,
  timeout, cooldown, one per window); `no_window` when hidden; timers
  paused and resumed with the time left; the guard order (`begin` before
  the invoke, `end` on `dialog_closed` or on return); the chip after
  400 ms and its Cancel; `cancel_file_text` on destroy; `errorKind` and
  `errorKey` on host-raised errors.
- `extension-sandbox-host.test.js`: identity and display name are
  overwritten even when the sandbox sends them; the args validator refuses
  unknown keys, bad formats and out-of-range `max_chars`.
- `extension-permissions.test.js` and `extension-rust-alignment.test.js`:
  the capability and three commands match Rust.
- `hostFeaturesFor`: `files.pick_text` only with `file_text`.
- Runtime: `context.files` absent without the feature; `pickText` maps
  `signal` to `cancel_file_text`.
- `scripts/check_i18n.py`: all keys in all catalogs.

**Manual checklist (each of Windows 11, macOS 14 or later, Ubuntu with
GNOME and one KDE desktop)**
1. Launcher: `mm+` → Add a file… → the launcher hides, the dialog opens
   with focus, pick a PDF → the launcher comes back with the chip, then the
   card.
2. Leave the dialog open for 30 s, then pick: no timeout.
3. Chat and settings: the dialog is attached to the window.
4. A file on a cloud drive that isn't downloaded yet: "Downloading…",
   then the text, or `not_downloaded` after 60 s with the network off.
5. Cancel during extraction of a large PDF.
6. An extension calling `pickText()` from a timer: the ask bar appears;
   **Not now** blocks re-asking for 30 s.
7. A hidden chat window's sandbox asks: `no_window`, nothing pops up.
8. RTL language (Arabic): the ask bar and chip mirror; an RTL file name
   displays correctly.
9. Audit log shows the read; Clear removes it.
10. Uninstall the extension while the dialog is open: picking a file after
    that reads nothing.

## 13. Work packages

File ownership is disjoint between packages that run in parallel. Shared
contract files belong to WP-FT-C, which lands first. The order:
- FT0 (spike) and FT-C first;
- FT1 needs FT-C;
- FT2, FT3 and FT4 need FT1 and run in parallel;
- FT5 needs FT1; FT6 runs in parallel with FT5; they merge together
  (the Rust and JS capability lists must agree for
  `extension-rust-alignment.test.js`);
- FT7 needs FT5; FT8 finishes last;
- FT-X1 any time after FT-C; FT-MM1 and FT-MM2 in parallel against the
  fake host; FT-MM3 needs FT-MM1, FT-MM2 and the host packages to test
  live.

The Kage-Extensions packages carry the `FT-` prefix so they never clash
with EXTENSION_AGENT_CALLS's `AC-X1` and `AC-MM1`…`AC-MM3`.

**Building alongside the other designs** (the full plan is SYNC §23):
- The four contracts packages land one at a time, in the order AC0,
  FT-C, V1, C0. FT-C rebases on AC0 and uses its `NotPermitted`,
  `InvalidInput` and `secret_spans`; it doesn't add them.
- The other designs list some of the same files (`locales/*`,
  `src/extensions.rs`, `src/commands/registry.rs`, `src/main.rs`,
  `src/lib.rs`, `src/permission_audit/types.rs`,
  `extension-permissions.js`, `extension-sandbox-host.js`,
  `extension-manager.js`, `settings-renderer.js`,
  `settings-host-effects.js`, `extension-capabilities.js`,
  `tool-permissions.js`, `settings/manager.js`, the floating and chat
  lifecycle files, `telemetry.js`, the docs, and Mind Map's `mind.js`,
  `model.js`, `settings.js`, `view.js`, `manifest.json`, `_locales`). For
  each, the earlier design in the order EXTENSION_VIEWS,
  EXTENSION_FILE_TEXT, EXTENSION_AGENT_CALLS, SYNC merges first, and the
  later package rebases and owns only its own lines.
- When EXTENSION_VIEWS has merged, FT6 adds the file broker to the view
  page (`ui/js/extension-view/main.js`, one install call with the no-op
  dialog guard of §5.3 and the view rule of §5.2).

### Kage repo

**WP-FT0 PDF spike** (no merged code; results go into §6 of this doc)
- Run `pdf-extract` and `lopdf` on a local corpus of at least 200 PDFs
  (reports, slides exported to PDF, scans, forms, CJK and RTL documents,
  password and permission-restricted files, broken files).
- Measure: text quality on 20 hand-checked files, time, peak memory,
  panics, hangs. Confirm the per-page API and stream-cap options of the
  chosen version. Run `cargo deny check licenses` on the worker's planned
  tree.
- Accept: a written choice with numbers; licences all permissive.
- Done: `pdf-extract`, with the worker steps in §6.2. Results in §6.1.
  One licence exception, outside the parsers (§6).

**WP-FT-C Contracts** (first, small)
- Files:
  - `kage-core/src/file_text_wire.rs`, `kage-core/src/lib.rs` (mod line):
    `Format`, `Warning`, `Pages`, `WorkerRequest`, `WorkerResponse`,
    `WorkerErrorCode`, the limit constants, the extension table (with the
    `READS_P1_FORMATS` switch), and the frame helpers `write_request` and
    `read_request`;
  - `src/events.rs` and `ui/js/shared/events.js` (`FILE_TEXT_PROGRESS`);
  - `locales/en/messages.json` (every key in §3.8 and §4.7), then
    `locales/<other>/messages.json` through `scripts/translate.py`.
- Depends on: WP-AC0 merged (`NotPermitted`, `InvalidInput`,
  `secret_spans`).
- Accept: `cargo check`; `scripts/check_i18n.py` passes; wire types
  round-trip through serde in unit tests; `events-and-windows.test.js`
  passes.

**WP-FT1 Worker crate and provisioning**
- Files: `kage-file-text/**` (Cargo.toml with every dependency the later
  packages need, `src/main.rs`, `src/lib.rs` with the format dispatch,
  `src/alloc.rs`, `src/limits.rs`, `src/normalize.rs`, `src/sniff.rs`,
  `src/plain.rs`, stub `src/{ooxml,pdf,html,rtf}/mod.rs` returning
  `Unsupported`, `tests/`, `fuzz/` with all five targets, the `text` one
  real), root `Cargo.toml` (workspace member, `hmac` for the app),
  `Cargo.lock`, `build.rs`, `tauri.conf.json`, `tauri.macos.conf.json`,
  `CLAUDE.md` (the binaries section names the new sidecar). Also
  `kage-core/Cargo.toml` and `kage-core/src/lib.rs` (the `os` feature,
  §5.5) and `scripts/check_module_size.py` (the new source root).
- Accept:
  - `cargo build` stages `kage-file-text-<triple>` and Tauri copies it
    next to `kage` on all three OSes; a warm no-op build stays under a
    second;
  - text and Markdown meet §5.9 and §5.10; normalisation and detection
    tests pass; the allocator cap and OS limits are applied before stdin
    is read (tests);
  - `cargo fuzz build` works; `cargo clippy -p kage-file-text` clean;
    files under 500 lines.

**WP-FT2 OOXML** (`kage-file-text/src/ooxml/**`,
`kage-file-text/tests/ooxml_*.rs`, `kage-file-text/fuzz/fuzz_targets/ooxml.rs`)
- DOCX in P0. PPTX in the same package but reported through
  `file_text_support` only once FT8's P1 switch is flipped
  (`READS_P1_FORMATS` in the wire crate's format table, owned by FT-C,
  changed in FT8).
- Accept: §3.7 conventions; §12 limit cases; Strict OOXML; fuzz target
  runs 10 minutes clean.

**WP-FT3 PDF** (`kage-file-text/src/pdf/**`, its tests and fuzz target)
- With the FT0 choice. Page cap, `page_starts`, encryption handling,
  `no_text`, title.
- Accept: §12 PDF cases; the corpus run finishes every file within the
  deadline; fuzz target runs 10 minutes clean.

**WP-FT4 HTML and RTF** (P1: `kage-file-text/src/html/**`,
`kage-file-text/src/rtf/**`, their tests and fuzz targets)
- Accept: §3.7 conventions; §12 cases; fuzz targets run 10 minutes clean.

**WP-FT5 Host commands** (`src/file_text/{mod,dialog,read,worker,file_ref,post}.rs`,
`src/commands/extensions/file_text.rs`, `src/commands/extensions/mod.rs`,
`src/commands/registry.rs`, `src/main.rs` and `src/lib.rs` (mod lines),
`src/extensions.rs` (`VALID_CAPABILITIES`),
`src/os/file_probe.rs`, `src/os/{windows,macos,linux}/file_probe.rs`,
`src/os/mod.rs` (mod line), `src/permission_audit/types.rs`,
`src/permission_audit/tests.rs`, `tests/file_text_test.rs`)
- §3.5, §5.1, §5.3–5.5, §5.11–5.15.
- Accept: every app-crate test in §12; the existing gates pass; no file
  name in any log line; `cargo clippy` clean.

**WP-FT6 Host JS** (`ui/js/shared/extension-file-text.js`,
`ui/js/extension-sandbox/runtime/files-api.js`,
`ui/js/shared/extension-sandbox-host.js`,
`ui/js/shared/extension-permissions.js`,
`ui/js/shared/extension-url-policy.js`,
`ui/js/shared/extension-manager.js`,
`ui/js/shared/extension-manager/{search,ui,dates}.js`,
`ui/js/shared/settings-renderer.js`, `ui/js/shared/settings-host-effects.js`,
`ui/js/floating/app.js`, `ui/js/floating/app/lifecycle-visibility.js`,
`ui/js/chat/app/lifecycle.js`, `ui/js/shared/telemetry.js`
(`KNOWN_EVENTS`), `ui/css/shared-components.css`,
`ui-tests/shared/extension-file-text.test.js` and the updates to the
existing tests named in §12)
- §3.4, §3.9, §4.3, §4.4, §5.1 (JS part), §5.2, §5.3 (JS part).
- Accept: every JS test in §12; manual steps 1, 2, 6, 7, 8 on one OS with a
  test extension.

**WP-FT7 Settings UI** (`ui/js/settings/manager.js` (display name and
host feature for the settings pool), `ui/js/settings/tool-permissions.js`
(filter option and one delegate call), `ui/js/settings/audit-file-reads.js`
(new: row rendering), `ui/js/settings/extension-capabilities.js`, settings
tests)
- Accept: rows and filter render for both outcomes; the pill shows with
  its tooltip; `tool-permissions.js` doesn't grow by more than 15 lines.

**WP-FT8 Docs and release** (`docs/EXTENSIONS.md`, `docs/SECURITY_MODEL.md`,
`docs/PRIVACY.md`, and the P1 format switch in
`kage-core/src/file_text_wire.rs` once FT2 and FT4 are done)
- EXTENSIONS.md: the capability row; a section "Reading a file the user
  picks" (§3.4–3.8 in user terms); the host-feature row; the two error
  kinds in "Errors from `invoke()`"; the `// permissions:` hint.
- SECURITY_MODEL.md: §7's additions. PRIVACY.md: the telemetry event and
  "what Kage keeps".
- Accept: links resolve; the docs match the merged behaviour; manual
  checklist complete on all three OSes.

### Kage-Extensions repo

**WP-FT-X1 Host table sync** (`scripts/host-capabilities.mjs`,
`web/store-core.js`, their tests)
- Mirror the capability (icon, label, description) and the three
  commands.
- Accept: `check-host-sync` reports no drift; store tests pass.

**WP-FT-MM1 Files leaf** (`extensions/mindmap/lib/files.js`,
`extensions/mindmap/lib/files.test.js`)
- Pure: `validateFileText`, `dedupeDecision`, `docTitle`,
  `bodyKeyphrases`, `findPassages`, `addRef`.
- Imports nothing (leaf rule); dependencies come in through `lib`.
- Accept: the `files.test.js` cases in §11.10; deterministic.

**WP-FT-MM2 Model** (`extensions/mindmap/lib/model.js`,
`extensions/mindmap/lib/journal.js`, their tests)
- `file` and `body` fields and their validation; `applyIngest`,
  `applyReread`; undo; doc actions as suggestions; body cleanup when the
  journal trims.
- Accept: the model cases in §11.10; existing tests unchanged.

**WP-FT-MM3 Integration** (`extensions/mindmap/mind.js`, `search.js`,
`settings.js`, `tools.js`, `view.js`, `defaults.js`, `styles.css`,
`manifest.json`, `_locales/en/messages.json`, `README.md`, their tests,
`Kage-Extensions/docs/mindmap/{SPEC,ARCHITECTURE,OPEN_QUESTIONS}.md`;
for F1 also `explorer.js` and `explorer-panel.js`)
- §11.1–11.9 for F0. The explorer parts (C11, C12, the doc detail pane)
  are F1: once EXTENSION_VIEWS' WP-V8 has merged the explorer, this
  package adds them to `explorer.js` and `explorer-panel.js` (only those
  lines), using `lib/files.js`. WP-V8 merges first and doesn't depend on
  this design.
- Q4 rewritten to "Decided"; SPEC §4 gains C9–C12, §6.2 the new fields,
  §7.1 the body keys, §7.2 the limits, §15 the disclosure, §18 the
  feature row; the §19 entry removed.
- Accept: `check:all` green; the §11.10 cases pass; with the host packages
  merged, manual steps 1–5 work with Mind Map; with an older host, Mind
  Map behaves exactly as before.

## 14. Later

- Several files in one dialog (at most 10), sharing the slot and limits.
- Dropping files on a host-owned drop zone in the view window (a drop is
  as clear a choice as a pick).
- OCR for scanned PDFs and images through the OS (Vision on macOS,
  `Windows.Media.Ocr` on Windows), still in the worker.
- More formats: `.odt`/`.odp` (same ZIP + XML path), `.epub`, `.eml`,
  spreadsheets (`calamine`, MIT) as tab-separated text.
- PDFium for better PDF text, if users report poor results.
- A PDF layout of our own (a custom `pdf-extract` `OutputDev`: lines by
  baseline, spaces from glyph gaps, direction-aware), for Chrome's Arabic
  and Urdu and its split words (§6.2).
- Drop the `lopdf` 0.47 repair step and the load filter once
  `pdf-extract` is released on `lopdf` 0.47 or later (§6.1).
- `reveal_picked_file(file_ref)`: the host keeps a machine-only map from
  ref to path and reveals the file in Finder or Explorer on a click. Needs
  its own consent story.
- Stronger worker sandboxing: seccomp on Linux, a sandbox profile on
  macOS, an AppContainer or low-integrity token on Windows.
- Fine-grained grants, so a user can keep an extension but revoke
  `file_text`.
- A `list_extension_data_keys` command, so extensions can sweep orphans.
- Cap the settings `pick_file` host effect (§7).
