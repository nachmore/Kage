# Extension Development Guide

This document describes how to create extensions for Kage. Extensions can add search providers (instant results in the floating window), settings pages, widget slots, and custom themes.

> **Where extensions live**
>
> Built-in extensions (math, calendar, window-walker) ship inside the
> Kage binary and live under [`ui/extensions/`](../ui/extensions). All
> *installable* extensions and themes live in a separate repository,
> [**Kage-Extensions**](https://github.com/nachmore/Kage-Extensions),
> which CI builds and publishes to a static catalog at
> <https://nachmore.github.io/Kage-Extensions/>. Kage's store window
> reads that catalog by default.
>
> If you're adding a new community extension, fork Kage-Extensions and
> follow the [`CONTRIBUTING.md`](https://github.com/nachmore/Kage-Extensions/blob/main/CONTRIBUTING.md)
> there. This document is the authoritative spec for the manifest
> format, capability list, and provider APIs — Kage-Extensions
> references back here.

## Extension Types

| Type | Description | Manifest `type` |
|------|-------------|-----------------|
| Extension | Adds search results, settings, widgets | `"extension"` |
| Theme | Custom color scheme | `"theme"` |
| Commands | Bundle of quick command shortcuts | `"commands"` |

## Directory Structure

An extension is a directory containing a `manifest.json` and its associated files:

```
my-extension/
├── manifest.json       # Required — extension metadata and contribution points
├── search.js           # Optional — search provider (ES module, default export)
├── settings.js         # Optional — settings page module
└── styles.css          # Optional — additional CSS
```

## Manifest Format

```json
{
  "id": "my-extension",
  "name": "My Extension",
  "version": "1.0.0",
  "type": "extension",
  "description": "A short description of what this extension does.",
  "icon": "🔧",
  "author": "Your Name",

  "permissions": ["storage", "clipboard"],

  "config": {
    "some_option": { "type": "boolean", "default": true },
    "another_option": { "type": "string", "default": "hello" }
  },

  "contributes": {
    "searchProvider": "./search.js",
    "settingsProvider": "./settings.js",
    "css": ["./styles.css"],
    "widgets": [
      {
        "id": "my-widget",
        "slot": "floating-bottom",
        "module": "./widget.js"
      }
    ]
  }
}
```

### Manifest Fields

| Field | Required | Description |
|-------|----------|-------------|
| `id` | Yes | Unique identifier (lowercase, hyphens). Used as directory name. |
| `name` | Yes | Display name shown in settings and store. |
| `version` | Yes | Semver version string. |
| `type` | Yes | One of: `"extension"`, `"theme"`, `"commands"`. |
| `description` | No | Short description (shown in store and settings header). |
| `icon` | No | Emoji icon for the extension. |
| `author` | No | Author name. |
| `permissions` | **Yes** | Capabilities the extension needs. See [Permissions](#permissions). Use `[]` if the extension needs nothing. Omitting this field means the extension gets zero capabilities and every `invoke()` will be rejected. |
| `sandboxVendor` | No | Array of allow-listed vendor library names to load into the sandbox before provider code runs. See [Vendor libraries](#vendor-libraries). Only built-in extensions may use this today. |
| `config` | No | Config schema: your settings and their defaults, the only place defaults are declared. Keys become the extension's config object. See [Config Schema](#config-schema). |
| `contributes` | No | What the extension provides (see below). |

### Config Schema

Each key in `config` defines a setting with a type and default value.
The manifest's `config` block is **the only place an extension's setting
defaults are declared**. Everything else reads them from here: your
providers' `context.config`, the Settings page, and anything the host
itself reads.

Supported types: `"boolean"`, `"string"`, `"number"`.

Hosts that work as described below list `config.manifest_defaults` in
[`context.hostFeatures`](#feature-detection). Older hosts hand your code
the saved object as it is, or the manifest defaults only when nothing was
saved at all, so most of what follows doesn't hold there. Kage has no way
for an extension to declare a minimum host version, so while you support
older hosts, keep your code fallbacks and keep each settings control's
`default` equal to the manifest's (see [Defaults](#defaults)), or branch
on `context.hostFeatures`.

**What your code sees.** On hosts with `config.manifest_defaults`,
`context.config` (and every `onConfigUpdate` payload) is the *effective*
config: the manifest defaults, overlaid with whatever the user saved.
Every declared key is always present, even one added in a later version
for a user who saved settings long ago, and one no settings control
shows. On these hosts, code fallbacks like `config.x ?? 2` are only
needed for keys the manifest doesn't declare; older hosts still need
them. The host reads saved values as the declared type: `"5"` for a
`number` key becomes `5`, and a value that can't be read as one (a
hand-edited config, or a key whose type you changed) is ignored, so the
default applies. Saved keys the manifest doesn't declare are passed
through unchanged.

**What gets stored.** On hosts with `config.manifest_defaults`,
`config.extensions["<id>"]` holds only the user's overrides: the values
that differ from the manifest default. Saving your Settings page writes
only the keys its controls own. Other saved keys are kept (see
[Scratch fields and what gets saved](#scratch-fields-and-what-gets-saved)).
When you ship a new default, it reaches every user with no stored value
for that setting. Older hosts stored every control's value on Save, so a
user who saved your page there has the old default stored as their own
value, and keeps it until they next save that page on a current host
(Save then removes a stored value equal to the *new* default, not one
equal to the old). `get_extension_config` (capability `storage`) returns
the stored object as-is, so use `context.config` to read your settings.

**`enabled` is the host's.** `config.enabled` is always present and is
whether the user has your extension switched on, which they change with
the Disable/Enable button in its Settings header. A saved `enabled` value
is ignored, and the host never stores one. A disabled extension that
stays loaded (the user switched it off in this session) gets an
`onConfigUpdate` with `enabled: false`, so you can stop background work
there. You don't need an "Enable" checkbox on your settings page. If you
have one (older hosts show it), current hosts hide it and keep its value
equal to the header's state, so `showWhen: { id: 'enabled', … }` follows
the header. You don't need to declare `enabled` in `config` either.

Older hosts saved such a checkbox's value. When Kage loads its config, a
saved `enabled: false` (the user unticked your checkbox) switches the
extension off with the header's switch, so the user's choice still holds,
and a saved `true` is dropped. **If your `enabled` control meant
something other than "this extension is on"**, such as one feature of
it, this is a breaking change: current hosts hide the control, ignore its
value, and may switch your extension off. Give that setting another id.

### Contributes

| Key | Description |
|-----|-------------|
| `searchProvider` | Path to an ES module that default-exports a search provider class (runs sandboxed). |
| `settingsProvider` | Path to an ES module that default-exports a settings provider class. Extensions declare their settings UI as a JSON schema; the host renders it. Runs sandboxed. See [Settings Provider API](#settings-provider-api). |
| `css` | Array of CSS file paths to load. |
| `widgets` | Array of widget contributions (see Widget Slots below). |
| `themes` | For theme type only — `{ "dark": "./dark.json", "light": "./light.json" }`. |
| `toolbarButtons` | Path to an ES module that default-exports a toolbar button provider. |
| `messageFormatters` | Path to an ES module that default-exports a message formatter. |
| `toolProvider` | Path to an ES module that default-exports a tool provider (exposes tools to the LLM). |
| `triggerProvider` | Path to an ES module that default-exports a trigger provider (emits automation signals). |
| `mascotActivities` | Array of generic mascot activity names the extension may hint — `"music"`, `"meeting"`, `"timer"`. See [Mascot activity hints](#mascot-activity-hints). |

## Permissions

Extensions run in **sandboxed iframes** with no access to
`window.__TAURI__` or the parent DOM. Every Tauri IPC command they
can call is mapped to a **capability**; the manifest declares the
capabilities the extension wants, and the host enforces them at
the message-port boundary — not in JS the extension could tamper
with.

### Install-time grant

When the user installs an extension, Kage shows a prompt listing
each requested capability with its icon, label, and description.
The user either approves the whole set or cancels the install. The
approved set is stored in config under `extension_grants[<id>]`
and consulted by the sandbox host on every `invoke()`.

If an extension updates and the new version requests capabilities
that weren't previously approved, those extra capabilities are
silently dropped until the user re-approves — the extension loads
but those particular IPC calls fail with a capability-missing
error.

### Declaring permissions

Request what you need in the manifest:

```json
{
  "permissions": ["storage", "urls", "notifications"]
}
```

Extensions that need nothing should include an empty list:

```json
{
  "permissions": []
}
```

This makes intent explicit and lets the settings UI show "🔒 No
capabilities" on the extension's page, which is a positive signal
for users.

### Capability reference

| Capability      | Icon | Grants access to… |
|-----------------|------|-------------------|
| `storage`       | 💾  | The extension's own sandboxed data and config: `save_extension_data`, `load_extension_data`, `delete_extension_data`, `get_extension_config`, `save_extension_config`. |
| `clipboard`     | 📋  | `read_clipboard`, `get_clipboard_history`. Read-only — `paste_clipboard_item` (write + synthesised Ctrl+V into the focused app) is host-only. |
| `urls`          | 🔗  | `open_url` for web links and a small allowlist of safe schemes (http, https, mailto, tel, sms, facetime, x-apple.systempreferences, ms-settings, prefs). Custom app URI schemes (`spotify://`, `vscode://`, etc.) are explicitly rejected — they need `launch`. |
| `launch`        | 🚀  | `open_path` (open arbitrary files in their default OS handler) and `launch_app_by_name`. Distinct from `urls` because the blast radius is "execute external code on the user's behalf." Only request this when an extension genuinely needs to start programs. |
| `network`       | 📡  | Outbound HTTP from the Rust runtime: `fetch_favicon`, `fetch_link_metadata`. Bypasses CORS and can reach intranet endpoints, so it's a separate capability from `urls`. |
| `oauth`         | 🔐  | One-shot loopback HTTP listener for OAuth callbacks: `oauth_loopback_start`, `oauth_loopback_await`, `oauth_loopback_cancel`. See [OAuth loopback](#oauth-loopback). |
| `filesystem`    | 📂  | Discovery only: `pick_folder`, `scan_folder`, `get_common_folders`, `search_files`, `resolve_directories`. `execute_folder_plan` (move/rename/trash) is host-only. |
| `window`        | 🪟  | Kage-owned window chrome: `resize_floating_window`, `set_floating_opacity`, `start_drag_window`, `save_window_position`, `save_chat_window_geometry`, `apply_chat_window_size`. |
| `windows`       | 🧿  | Other apps' windows: `list_open_windows`, `get_window_icons`, `focus_open_window`, `get_process_name`, `get_source_window`, `get_app_icon`. |
| `notifications` | 🔔  | `notify_frontend_ready`. |
| `calendar`      | 📅  | `get_calendar_events`, `get_calendar_events_for_date`, `get_calendar_busy_days` (per-day counts). Also unlocks the host [date picker](#date-picker). |
| `session`       | 💬  | `list_sessions`, `load_session`, `get_window_session`, `get_session_stream_snapshot`, `get_sessions_directory`. |
| `agent`         | 🤖  | LLM communication: `send_message_streaming`, `cancel_generation`, `send_steering_message`, `extension_tool_response`, `open_chat_with_message`, `get_available_models`, `get_slash_commands`. |
| `activity`      | 📊  | `start_activity_tracker`, `stop_activity_tracker`, `get_activity_report`, `is_activity_tracker_running`. |
| `automation`    | ⚡   | `emit_automation_signal`, `list_automation_signals`, `get_power_status`. |
| `tts`           | 🔈  | `pocket_tts_test`, `pocket_tts_voices`. |

The authoritative command-to-capability mapping is in
[`ui/js/shared/extension-permissions.js`](../ui/js/shared/extension-permissions.js).
If you add a new Tauri command you want extensions to be able to
call, add it to that table under an existing capability (or propose
a new one).

### Other apps' windows

With the `windows` capability, `invoke('list_open_windows')` resolves
to an array of other apps' top-level windows (Kage's own excluded),
front to back on Windows and macOS:

```js
{
    title: 'Quarterly plan.docx',  // the app's name when titles are unavailable (below)
    process_name: 'Microsoft Word',
    handle: 123456789,             // opaque; pass to focus_open_window / get_window_icons
    minimized: false,
}
```

- `minimized` is `true` for a window in the Dock / taskbar. Minimized
  windows are **listed, not filtered**: if you offer a "hide
  minimized" option, filter on this field yourself. Focusing one with
  `focus_open_window` restores it.
- `minimized` is `false` whenever the host can't tell, so `false`
  means "not known to be minimized":
  - **Windows** — always accurate.
  - **macOS** — needs the Accessibility permission. Without it the list
    holds on-screen windows only (minimized ones are missing, not
    flagged), and no permission prompt is shown. With it, minimized
    windows are listed after the on-screen ones. Windows that are just
    off screen (on another Space, or of an app hidden with ⌘H) aren't
    listed either way.
  - **Linux** — not detected, always `false`.
- On macOS without the Screen Recording permission, titles are empty,
  so each app gets one entry titled with its name. That entry is
  `minimized: true` only when all of the app's listed windows are
  minimized.
- Hosts that predate the field omit it (so it reads as `undefined`)
  and don't list `windows.minimized` in
  [`context.hostFeatures`](#feature-detection). Check the feature if
  you need to tell "not minimized" from "this host doesn't say", e.g.
  to hide a "hide minimized windows" setting that would do nothing.

### Forbidden commands

Some commands are never callable from any extension regardless of
capabilities. These include: `get_config` and `save_config`,
`quit_app`, `restart_app`, `execute_system_command`,
`paste_clipboard_item` (keystroke injection), `execute_folder_plan`
(file moves/renames/trash), `send_extension_tool_steering` (the host
builds the tool-steering block from validated manifests, and the
backend replays it into every session), `notify_response_ready`,
`save_frecency` / `load_frecency`, install/uninstall commands,
tool-permission policy commands, `read_extension_file`,
`open_devtools`, MCP config commands, updater commands, inline
assist, the shortcut executor, and more. See
`extension-permissions.js` for the full list.

### OAuth loopback

The `oauth` capability gives you a one-shot HTTP listener on
`127.0.0.1` for native (PKCE) sign-in flows:

1. `invoke('oauth_loopback_start', { args: { redirect_path: '/myext/callback', timeout_secs: 300, success_label: 'My Service' } })`
   → `{ listener_id, redirect_uri, port }`. Use `redirect_uri`
   verbatim as the OAuth `redirect_uri`. `timeout_secs` defaults to
   300 and is capped at 600.
2. Open the provider's authorize URL (`open_url`, `urls` capability).
3. `invoke('oauth_loopback_await', { args: { listener_id } })` resolves
   with `{ params }` — every query parameter of the redirect (`code`,
   `state`, or `error`, …). Check `state` yourself.
4. If the user abandons the flow, call
   `invoke('oauth_loopback_cancel', { args: { listener_id } })`. It
   works whether or not an await is in flight, and it's idempotent. By
   the time it resolves the port is closed, so a consent the user gives
   later is refused instead of being told "You're authorized", and a
   pending await rejects with `cancelled`.

The listener stops after the first request to `redirect_path` (other
paths get a 404), on timeout, on cancel, or when the sandbox that
started it is torn down (your extension is disabled, updated, or
uninstalled). Closing the Settings window only hides it, so a sign-in
started there keeps running. A result that arrives before you await
is kept for a minute. `oauth_loopback_await` rejects with:

| `err.kind`  | `err.key`                       | Meaning |
|-------------|---------------------------------|---------|
| `timeout`   | `errors.passthrough`            | No redirect within `timeout_secs`. Often the provider showed its own error page (unknown client, unregistered redirect URI) and never redirected. |
| `cancelled` | `errors.passthrough`            | `oauth_loopback_cancel` stopped the listener. |
| `internal`  | `errors.oauth.unknown_listener` | Unknown id: never started, already awaited to completion, cancelled before the await, or a result left unclaimed for over a minute. |
| `internal`  | `errors.oauth.already_awaited`  | Another await on the same id is in flight. |
| `internal`  | `errors.oauth.listener_failed`  | The socket failed while waiting. |

Branch on `kind` for `timeout` and `cancelled`; their `key` is the
generic `errors.passthrough`. Their `message` is fixed English in
every language — "OAuth flow timed out before a callback arrived" and
"OAuth listener was cancelled before a callback arrived" — because
extensions written before kinds existed match on it.

**Older hosts** (no `invoke.error_kind` in
[`context.hostFeatures`](#feature-detection)) set no `kind`, report a
timeout as "OAuth listener was cancelled before a callback arrived",
and can't cancel an await that's already in flight. To support them,
match the text only when there's no `kind`:

```js
function loopbackFailure(e) {
    if (e?.kind === 'timeout' || e?.kind === 'cancelled') return e.kind;
    // Older hosts: no kind, and a timeout looked like a cancel.
    if (!e?.kind && /before a callback arrived/i.test(String(e?.message))) return 'timeout';
    return null;
}
```

## Vendor libraries

Some extensions need a UMD/IIFE library (e.g. mathjs) that exposes a
global. Because the sandbox has no network access, these can't be
loaded from a CDN. Extensions declare what they need via the
`sandboxVendor` manifest field:

```json
{
  "id": "math",
  "sandboxVendor": ["math"]
}
```

The names are looked up against an **allow-list** in
`ui/js/shared/extension-manager.js` (`SANDBOX_VENDOR_ALLOWLIST`).
Only libraries we've vetted and pre-bundled with the app are
available — extensions can't name arbitrary paths. Today the
allow-list contains:

| Name   | Global set          | File                    |
|--------|---------------------|-------------------------|
| `math` | `self.math` (mathjs) | `ui/vendor/lib/math.js` |

If you need a vendor library that isn't on the list, open a PR
adding it. Third-party extensions currently can't add their own
vendor libs — only built-in extensions ship vendor bundles.

At sandbox init, the host fetches each named file and caches its
source text. The libraries are **not loaded into the sandbox
iframe** — they're available to the [`runSandboxed` Worker
helper](#terminable-compute-with-runsandboxed), which injects them
into per-call Workers so the library runs on a thread that can be
terminated. Running vendor libraries in-thread would freeze the
whole sandbox on pathological inputs (e.g. mathjs on `2^2^2^2^2`).

### Which contribution points run in the sandbox

| Contribution point     | Runs in sandbox today? |
|------------------------|------------------------|
| `searchProvider`       | ✅ Yes |
| `toolProvider`         | ✅ Yes |
| `triggerProvider`      | ✅ Yes |
| `settingsProvider`     | ✅ Yes (declarative schema) |
| `toolbarButtons`       | ✅ Yes (declarative + RPC on-click) |
| `messageFormatters`    | ✅ Yes (HTML-in, HTML-out; host sanitizes) |
| `widgets`              | ✅ Yes (refresh-loop render with declared actions) |
| Custom `renderResult`  | ✅ Yes (sandbox returns sanitized HTML for result rows) |
| `css`                  | Trusted path (host injects scoped stylesheet) |

See [`SECURITY_MODEL.md`](./SECURITY_MODEL.md) for the plan to close
the remaining gaps.

### Sandbox behaviour you should know about

- Your provider's module runs with a **null origin**. No
  `window.__TAURI__`, no cookies, no `localStorage`, no access to
  the parent window or to other extensions.
- `context.invoke(command, args)` round-trips through a
  MessagePort. The host validates the command against your
  granted capabilities before calling Tauri. See
  [Errors from `invoke()`](#errors-from-invoke) for what a rejection
  carries.
- `context.hostFeatures` lists optional host behaviour you can
  feature-detect — see [Feature detection](#feature-detection).
- `context.mascot` lets you hint what the user is *doing* so the
  mascot can react. It's not an `invoke` and needs no capability —
  see [Mascot activity hints](#mascot-activity-hints).
- `context.formatter.requestReformat()` asks the host to re-run your
  message formatter — see [Progressive formatting](#progressive-formatting).
- `context.hostFeatures` is a frozen array of host UI features this
  window offers *your* extension (e.g. `'date-picker'`, only with the
  `calendar` grant). It's `undefined` on hosts that predate it, which is
  how you feature-detect — see [Date picker](#date-picker).
- `context.log` writes to the main Kage log via the bridge; it's
  a structured logger (`log.info`, `log.warn`, etc.) not a
  `console.log` replacement. Regular `console.log` inside the
  sandbox still works but goes to the sandbox iframe's dev
  console, not the main one.
- You cannot attach event listeners to the parent window or
  manipulate parent DOM. `document.*` inside the sandbox exists
  but it's the sandbox's own empty document — writes have no
  visible effect.
- You can't write the clipboard from the sandbox (and an RPC to the
  host would arrive after the click's user activation has expired).
  On a settings page, use a [copy action](#copy-to-clipboard) instead.

### Feature detection

`context.hostFeatures` is a frozen array of strings naming optional
host behaviour. Names are only ever added, never repurposed. Hosts
that predate the list don't set it at all, so treat a missing list as
"none of these":

```js
const has = (f) => Array.isArray(context.hostFeatures) && context.hostFeatures.includes(f);
```

| Feature                | Meaning |
|------------------------|---------|
| `settings.copy_action` | Settings `action` controls accept `copy` (see [Copy to clipboard](#copy-to-clipboard)). |
| `invoke.error_kind`    | `invoke()` rejections carry `err.kind` / `err.key` (see [Errors from `invoke()`](#errors-from-invoke)). |
| `config.manifest_defaults` | `context.config` is the manifest defaults overlaid with saved values, `config.enabled` is the host's on/off switch, saving stores only overrides, and settings controls take their defaults from the manifest (see [Config Schema](#config-schema)). |
| `windows.minimized`    | `list_open_windows` entries carry `minimized: boolean`, and minimized windows are listed and flagged rather than left out (see [Other apps' windows](#other-apps-windows)). |

### Errors from `invoke()`

A rejected `invoke()` gives you an `Error` whose `message` is
human-readable text, usually **in the user's language** — fine to log
or show, never something to match on. When the backend returned a
structured error, the `Error` also carries:

- `err.kind` — a stable category: `timeout`, `cancelled`,
  `connection_lost`, `rate_limited`, `internal`, …
- `err.key` — the stable message key, e.g.
  `errors.oauth.unknown_listener`. Free-form backend errors share the
  generic key `errors.passthrough`, so for those only `kind` tells
  them apart.

Branch on `kind` (or `key`). Errors the host raises itself (missing
capability, forbidden command, rate limit) have no `kind`. Hosts
without `invoke.error_kind` never set either field — see
[OAuth loopback](#oauth-loopback) for a fallback that works on both.

### Your extension runs as MULTIPLE instances — design for it

**This is a hard requirement, not a tip.** There is no single
"the extension process". Every Kage window that uses one of your
contribution points (floating, chat, settings, peer chat windows)
hosts its **own sandbox iframe with its own copy of your
modules**. Consequences:

- **Module-level variables are per-window.** A `let cachedToken`
  in one window is invisible to every other window. An in-flight
  promise used as a single-flight guard dedupes work inside ONE
  window only.
- **Instances race each other.** Two windows polling the same
  API will both notice an expired token within one poll cycle of
  each other and both try to refresh it concurrently. With
  providers that rotate credentials on refresh (OAuth refresh
  tokens — Spotify, Google, Microsoft all do this), the loser
  presents an already-rotated token, which the provider's reuse
  detection may answer by **revoking the whole grant**. The
  user experiences this as "the extension keeps logging me out".
- **The only shared state is `extension-data`**
  (`save_extension_data` / `load_extension_data` /
  `delete_extension_data`). It is host-side storage keyed per
  extension and visible to every instance. `localStorage` is
  unavailable in the sandbox, and module state doesn't cross
  windows — if two instances must agree on something, it goes
  through extension-data.

Rules for anything with side effects (auth flows, credential
refresh, write-behind caches, migrations):

1. **Re-read before declaring failure.** If a credential
   operation fails, reload the stored state first — another
   instance may have already rotated/renewed it. Retry with
   what's on disk before telling the user they're signed out.
2. **Serialize cross-instance side effects.** Elect a leader
   (e.g. only the floating window's widget instance refreshes
   tokens; other instances wait and re-read), or take a coarse
   mutex via extension-data (write `{holder, expires_at}`,
   others back off while it's fresh). A TTL on the lock is
   mandatory — an instance can die holding it.
3. **Never cache "am I authenticated?" in module state.**
   Another instance can sign out, sign in, or rotate at any
   moment. Read through to extension-data for anything another
   instance can change; per-instance caching is only safe for
   state that instance alone owns.
4. **Idempotency over coordination where possible.** If a side
   effect is safe to repeat (PUT-like), letting both instances
   do it beats locking. Locks are for the genuinely
   non-repeatable (token rotation, one-shot migrations).

The Spotify extension's auth module
(`Kage-Extensions/extensions/spotify/auth.js`) is the reference
implementation of these rules: single-flight within an instance
via an in-flight promise, cross-instance healing via
re-read-on-`invalid_grant`, and a persisted status marker so all
windows agree on connection state.

## Terminable compute with `runSandboxed`

JavaScript in the sandbox iframe is single-threaded. If your
extension runs a synchronous computation that takes seconds —
think `math.evaluate('2^2^2^2^2')`, a deep regex backtrack, or a
large parse — the whole iframe freezes, and because sandbox
iframes share an agent cluster in Chromium, sibling extensions
freeze too. `setTimeout` can't cancel sync work, so any in-flight
RPCs from the host to any extension hit their 10s timeout and log
scary warnings.

To protect against this, `context.runSandboxed` runs a function
inside a fresh Web Worker with a hard timeout. If the work
exceeds the deadline the Worker is terminated and the promise
rejects. The Worker has no host bridge (no `invoke`, no network),
so it's intended purely for CPU-bound work; feed it inputs via
`data`, collect the result as a serializable value.

```js
const result = await context.runSandboxed({
    vendor: ['math'],
    data: { expression: query },
    timeoutMs: 1000,
    run: (data, lib) => lib.math.evaluate(data.expression),
});
```

### API

| Option      | Type                                | Description |
|-------------|-------------------------------------|-------------|
| `run`       | `(data, lib) => any`                | Required. Serialized via `Function.toString()`, so it must be self-contained — no closures over outer variables. May return sync or async. |
| `vendor`    | `string[]`                          | Names from your manifest's `sandboxVendor` list. The Worker imports each named library before `run` executes, and `lib[name]` exposes the global the library set (e.g. `lib.math` for mathjs). |
| `data`      | any structured-cloneable            | Input passed as the first argument to `run`. |
| `timeoutMs` | `number`                            | Default `1000`. On timeout the Worker is terminated and the promise rejects. |

### When the promise rejects

| Cause                       | Error message                                          |
|-----------------------------|--------------------------------------------------------|
| Exceeded `timeoutMs`        | `runSandboxed: timed out after Nms`                    |
| `run` threw inside Worker   | `<the thrown error's message>`                         |
| `data` not structured-cloneable | `runSandboxed: failed to post data: ...`           |
| Worker script failed to parse | `runSandboxed: <worker error message>`                |

### Constraints

- `run` runs in a Worker, so **no DOM, no `context.invoke`, no
  `window.__TAURI__`, no import, no network**. If you need host
  IO, do it in the provider, then pass raw inputs into
  `runSandboxed` for the expensive step.
- `run` is serialized via `toString()`. Captured variables won't
  survive — always pass them through `data`. Anonymous arrow
  functions work; methods and bound functions don't.
- The return value is postMessaged back, so it must be
  structured-cloneable. Plain objects, arrays, numbers, strings
  are fine; BigNumber instances from mathjs aren't — convert to
  a number or string first.
- One Worker per call. Workers spin up in a few ms on modern
  Chromium, so per-keystroke use (e.g. a search provider's
  `match`) is fine. If you find yourself calling this thousands
  of times per second, do your own pooling.
- Vendor libraries you reference in `vendor` must already be
  declared in your manifest's `sandboxVendor` list. Names not
  declared at install time are silently dropped — the Worker
  just won't have that global.

## Search Provider API

A search provider is an ES module that default-exports a class:

```js
export default class MySearchProvider {
    /**
     * Called once when the extension loads.
     * @param {object} context - { invoke, config, log, runSandboxed }
     *   invoke: Tauri invoke function for IPC (host enforces capabilities)
     *   config: The extension's config: manifest defaults overlaid with the user's
     *           saved values, plus the host's `enabled` (see "Config Schema")
     *   log: Structured logger routing to the main Kage log ({ debug, info, warn, error })
     *   runSandboxed: Terminable compute helper (see "Terminable compute" above)
     */
    initialize(context) {
        this.config = context.config;
    }

    /**
     * Called when the extension's config changes (user saves settings).
     * @param {object} config - Updated config object
     */
    onConfigUpdate(config) {
        this.config = config;
    }

    /**
     * Synchronous match — called on every keystroke. Must be fast.
     * @param {string} query - Current input text (trimmed)
     * @returns {SearchResult[]} Array of results, or empty array
     */
    match(query) {
        return [];
    }

    /**
     * Async match — for expensive operations (network, crypto, etc.).
     * Called after match(). If this returns a non-empty array, those rows
     * SUPERSEDE this extension's match() rows (the "Loading…" placeholder is
     * dropped automatically — placeholder and loaded rows do NOT need matching
     * ids). Return [] to keep the placeholder, e.g. on a cache hit where
     * match() already returned the real data.
     * @param {string} query
     * @returns {Promise<SearchResult[]>}
     */
    async matchAsync(query) {
        return [];
    }

    /**
     * Optional: declare this extension's complete trigger set. The host uses
     * it to (1) gate match()/matchAsync() — only calling you when a keyword is
     * committed — and (2) show completion hints for partial prefixes. Runs in
     * the sandbox, so the list can reflect this.config (a user-renamed
     * trigger). Labels are i18n KEYS, resolved host-side against _locales.
     * Omit this method entirely if you're a content matcher that needs every
     * keystroke. @returns {KeywordDef[]}  See "Search keywords" below.
     */
    getKeywords() {
        return [
            { keyword: 'foo', labelKey: 'keyword.foo.label', descriptionKey: 'keyword.foo.desc', icon: '🦊', acceptsArgs: true },
        ];
    }

    /**
     * Called when the user selects (Enter) a result from this provider.
     * Return an action descriptor.
     * @param {SearchResult} result
     * @returns {Action}
     */
    execute(result) {
        return { type: 'copy', value: result.data.value };
    }

    /**
     * Optional: custom rendering for the suggestion item.
     * If not provided, the default label/description/icon rendering is used.
     * @param {SearchResult} result
     * @param {HTMLElement} element - The suggestion item container
     */
    renderResult(result, element) { }

    /**
     * Called when the extension is unloaded.
     */
    destroy() { }
}
```

### SearchResult Shape

```js
{
    id: string,          // Unique ID (used for frecency tracking)
    type: string,        // Extension-defined type string
    label: string,       // Primary display text
    description: string, // Secondary text (shown below label)
    icon: string,        // Emoji or icon character
    score: number,       // 0-100, used for ranking among all results
    data: any            // Extension-specific payload (passed to execute())
}
```

### Action Types

Returned by `execute()`:

| Type | Description | Fields |
|------|-------------|--------|
| `copy` | Copy value to clipboard | `value: string` |
| `open_url` | Open URL in browser | `value: string` |
| `open_path` | Open file/folder | `value: string` |
| `send_prompt` | Send to AI agent | `value: string` |
| `replace_input` | Replace the floating window input text and trigger a new search | `value: string` |
| `open_extension_settings` | Open Settings deep-linked to this extension's own settings page (the target is derived from the extension's id by the host — an extension cannot navigate to any other section) | — |
| `custom` | Extension handles it | `data: any` |

### Score Guidelines

| Range | Use for |
|-------|---------|
| 90-100 | Exact/high-confidence matches (colors, URLs) |
| 80-89 | Strong matches (system commands, shortcuts) |
| 70-79 | Good matches (app names, command names) |
| 60-69 | Partial matches |
| < 60 | Weak/fuzzy matches |

### Search keywords (`getKeywords`)

Implementing the optional `getKeywords()` declares your extension's complete
trigger set to the host. It serves **two** purposes:

1. **Gating (performance).** The host calls your `match()`/`matchAsync()` only
   when the query *commits* one of your keywords — an exact match (`cal`) or
   keyword-plus-space (`cal tomorrow`). For every other keystroke your
   provider isn't invoked at all, so it costs nothing. An extension that does
   *not* implement `getKeywords()` is treated as a **content matcher** (math,
   currency, color) and receives every keystroke as before.
2. **Discoverability (hints).** While the query is still an *incomplete
   prefix* of a keyword (`cal-ref`), the host shows a **completion hint** row
   built from the keyword's metadata. Selecting it fills the input with the
   full keyword so your real rows appear.

```js
getKeywords() {
    const trigger = (this.config.trigger || 'cal').toLowerCase(); // config-aware
    return [
        { keyword: trigger,        labelKey: 'keyword.cal.label',     descriptionKey: 'keyword.cal.desc',     icon: '📅', acceptsArgs: true  },
        { keyword: 'cal-refresh',  labelKey: 'keyword.refresh.label', descriptionKey: 'keyword.refresh.desc', icon: '🔄', acceptsArgs: false },
    ];
}
```

**KeywordDef shape:**

| Field | Type | Notes |
|-------|------|-------|
| `keyword` | string | A trigger word. Lowercased by the host for matching. |
| `labelKey` | string | i18n key for the hint's primary text. Resolved host-side against `_locales`; falls back to the bare keyword if missing. |
| `descriptionKey` | string | i18n key for the secondary text. Optional. |
| `icon` | string | Emoji/icon. Defaults to the extension's manifest icon. |
| `acceptsArgs` | boolean | `true` (default) → keyword takes arguments; a committed `keyword ` (with space) still invokes you, and selecting a hint fills `keyword + ' '`. `false` → exact-only terminal command (e.g. a refresh); the hint fills exactly `keyword`. |

**Rules — read these, they're load-bearing:**

- **Register your COMPLETE trigger set.** Because the host gates on it, any
  keyword your `match()`/`matchAsync()` answers but you *don't* register
  becomes **unreachable**. If you accept a bare `cal` *and* a longer
  `calendar`, register **both** (this differs from a pure-hint model — the
  shorter one is a real trigger, not just a prefix). Sub-commands like `bm+` /
  `todo-` are distinct whole-word keywords and must each be listed.
- **Be config-aware.** If your trigger is user-configurable, read
  `this.config` and return keywords based on it — the host re-fetches when
  config changes. If a config makes you a content matcher (e.g. an empty
  trigger that matches any word), return `[]`.
- **Hybrids stay content matchers.** If you need both fixed keywords *and*
  arbitrary content (e.g. emoji's `:shortcode:`, dev-tools' raw JSON), do
  **not** implement `getKeywords()` — you need every keystroke. You lose hints
  but keep correctness.
- **Hints fire only for an incomplete, space-free prefix.** Synonyms that
  share a `labelKey` collapse to one hint (the shortest keyword); typing a
  keyword exactly commits its whole synonym group, so no redundant hint shows.
- Selecting a hint never calls your `execute()` — it refills the input (the
  `replace_input` path) and re-runs search.
- **Labels are keys, never raw strings.** Add the keys to
  `_locales/en/messages.json` and seed the other locales (the host's
  `scripts/translate.py` walks extension catalogs). Because these keys are
  resolved host-side, not via `t()`, add an `// i18n-keys: foo.label, ...`
  comment next to `getKeywords()` so the i18n drift check doesn't flag them as
  unused.

## Settings Provider API

A settings provider is an ES module that default-exports a class. The
provider runs inside the extension sandbox and **never touches the host
DOM**. Instead, it returns a JSON schema describing the settings UI, and
the host renders it. Action buttons route back through the provider as
RPC calls.

The example's value controls are backed by this manifest `config` block
(see [Config Schema](#config-schema) and [Defaults](#defaults)):

```json
"config": {
  "show_badge": { "type": "boolean", "default": true },
  "trigger":    { "type": "string",  "default": "example" },
  "limit":      { "type": "number",  "default": 10 },
  "mode":       { "type": "string",  "default": "a" },
  "volume":     { "type": "number",  "default": 3 }
}
```

```js
export default class MyExtSettingsProvider {
    initialize(context) {
        // Same `context` every provider sees: { invoke, config, log }.
        this.config = context.config || {};
        this.invoke = context.invoke;
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    /**
     * Declarative description of the settings UI. Each value control's `id`
     * names a key in the manifest's `config` block (above), which supplies
     * its default. The `default`s below are only for hosts older than
     * `config.manifest_defaults`, and must equal the manifest's (see
     * "Defaults" below).
     */
    getSettings() {
        return {
            description: 'Optional text shown below the header.',
            sections: [
                {
                    label: 'Display',                  // optional
                    controls: [
                        { type: 'checkbox', id: 'show_badge', label: 'Show badge', default: true,
                          description: 'Show a count badge on results.' },
                        { type: 'text', id: 'trigger', label: 'Trigger word',
                          default: 'example', placeholder: 'example', maxWidth: 120 },
                        { type: 'number', id: 'limit', label: 'Max items',
                          default: 10, min: 1, max: 100 },
                        { type: 'select', id: 'mode', label: 'Mode', default: 'a',
                          options: [{ value: 'a', label: 'A' }, { value: 'b', label: 'B' }] },
                        { type: 'range', id: 'volume', label: 'Volume',
                          default: 3, min: 1, max: 10, unit: '×' },
                        { type: 'action', id: 'test', label: 'Test',
                          action: 'run_test' },               // → runAction('run_test', values)
                        { type: 'action', id: 'copy_trigger', label: 'Copy',
                          copy: { from: 'trigger' },          // host copies it, no RPC
                          action: 'copy_trigger' },           // older hosts: runAction fallback
                        { type: 'info', html: 'Sanitized HTML. <code>tags</code> allowed: a, b, br, code, em, i, li, ol, p, span, strong, ul, div.' },
                    ],
                },
            ],
        };
    }

    /** Optional validation. Called before save. */
    validate(values) {
        if (!values.trigger) return { valid: false, error: 'Trigger required' };
        return { valid: true };
    }

    /**
     * Optional normalization — canonicalize values (trim whitespace, add
     * implicit delimiters, etc.) before they are persisted. Return
     * `{ values: <canonical map> }`; keys you leave out keep their current
     * value. See "Scratch fields and what gets saved".
     */
    normalize(values) {
        return { values: { ...values, trigger: values.trigger.trim() } };
    }

    /** Called when the user clicks an action button. */
    async runAction(action, values) {
        if (action === 'run_test') {
            const result = await this.invoke('some_command', { /* ... */ });
            // Plain status string shown next to the button.
            return { status: `✅ Got ${result.count} items` };

            // Or ask the host to perform a side effect:
            // return { host: { type: 'download', filename: 'out.json', content: '...' } };
            // return { host: { type: 'pick_file', accept: '.json', action: 'import' } };
            // return { host: { type: 'play_timer_sound', soundId: 'chime', repeats: 3 } };
        }
        return {};
    }

    /**
     * Called after the user picks a file (in response to a
     * `host: { type: 'pick_file' }` effect). Return `{ status }` to show
     * feedback on the originating button.
     */
    async onFileSelected({ action, filename, content, values }) {
        if (action === 'import') {
            // ... parse, validate, save via this.invoke
            return { status: `✅ Imported from ${filename}` };
        }
        return {};
    }
}
```

### Control types

`default?` is deprecated for ids the manifest declares; see
[Defaults](#defaults).

| Type       | Props                                                                 |
|------------|-----------------------------------------------------------------------|
| `checkbox` | `id`, `label`, `description?`, `default?`, `showWhen?`                |
| `text`     | `id`, `label`, `description?`, `default?`, `placeholder?`, `maxWidth?`, `showWhen?` |
| `number`   | `id`, `label`, `description?`, `default?`, `min?`, `max?`, `step?`, `maxWidth?`, `showWhen?` |
| `select`   | `id`, `label`, `description?`, `default?`, `options: [{value, label}]`, `maxWidth?`, `showWhen?` |
| `range`    | `id`, `label`, `description?`, `default?`, `min`, `max`, `step?`, `unit?`, `showWhen?` |
| `action`   | `id`, `label` (button text), `description?`, `action` (RPC name; optional only with `copy`), `copy?`, `variant?: 'default'|'danger'|'primary'`, `confirm?`, `disabled?`, `showWhen?` |
| `info`     | `label?`, `html` (sanitized), `showWhen?`                             |

An unknown control `type` (or any other validation error) fails the
**whole schema**: your page doesn't show up in Settings at all, and a
`refresh` that returns such a schema leaves the old page in place. New
capabilities therefore arrive as optional props on existing types (like
`copy` below), which older hosts ignore, rather than as new types.

### Defaults

A value control's `id` refers to a key in your manifest's
[`config`](#config-schema) block, and that's where its default comes from.
The control shows the effective value: the manifest default, or what the
user saved instead. **A control's own `default` is deprecated.** Current
hosts ignore it for any id the manifest declares, and log a console
warning when it would show something different (text that differs only
in leading or trailing whitespace isn't flagged, since `normalize()`
often accounts for that). It is still used for:

- **ids the manifest doesn't declare**: scratch (`__`) fields, and older
  extensions that never declared their settings; and
- **older hosts**, which don't list `config.manifest_defaults` in
  [`context.hostFeatures`](#feature-detection). They show the control's
  `default` for a value that was never saved, and give your code the
  saved object without the manifest defaults merged in.

Kage has no way for an extension to declare a minimum host version, so
in practice keep every control's `default` equal to the manifest default
for as long as you support older hosts. In the Kage-Extensions repo,
`npm run check:defaults` enforces that.

Because the control always shows the effective value, a control's
`default` can no longer change how a declared value is displayed. If the
stored form of a setting isn't what the user should see (a trigger
stored with its trailing delimiter, `"w "`), store the display form and
add the rest in code, rather than computing a trimmed `default` from
`context.config`.

### Action options

- `disabled: true` renders the button greyed out and unclickable. Use it
  for actions that don't apply right now (e.g. "Sign out" when signed
  out); return a [`refresh`](#host-side-effects) effect when the state
  changes so the button updates.
- `confirm: 'Really delete?'` asks for confirmation with a native dialog
  before `runAction` runs. Ignored by copy actions.
- `variant: 'primary' | 'danger'` styles the button; the default is
  plain.
- While `runAction` runs, the button is disabled and its status line
  reads "Working…". Whatever `{ status }` you return replaces it;
  `{ error }` shows as `❌ <error>`. A thrown error shows its message.

### Copy to clipboard

The sandbox can't write the clipboard, and routing a click through
`runAction` loses the user activation the clipboard API needs. Give an
`action` a `copy` instead, and the host copies the text itself, inside
the click, then shows "Copied" next to the button:

```js
// A literal value…
{ type: 'action', id: 'copy_redirect', label: 'Copy',
  copy: { value: 'http://127.0.0.1:43821/myext/callback' },
  action: 'copy_redirect' },            // fallback for older hosts, see below
// …or the current value of another control.
{ type: 'text', id: 'api_key', label: 'API key' },
{ type: 'action', id: 'copy_key', label: 'Copy', copy: { from: 'api_key' } },
```

- `copy` needs exactly one of `value` (a string) or `from` (the id of a
  `text`, `number`, or `select` control in the same schema; it's read
  when clicked).
- The text must be plain: 1–2048 characters, no control characters
  other than tab and newlines, and no bidi-override characters. A
  literal `value` that breaks this fails schema validation; a `from`
  value that breaks it at click time shows "Couldn't copy to the
  clipboard". An empty `from` shows "Nothing to copy".
- With `copy`, current hosts never call `runAction`, and `confirm` is
  ignored. `disabled`, `variant`, and `showWhen` work as usual.

**Older hosts** (no `settings.copy_action` in
[`context.hostFeatures`](#feature-detection)) ignore `copy`, and require
`action`. Without it, they reject the whole schema. So either:

- keep an `action` on every copy control and handle it in `runAction` —
  older hosts call it on click, so return something useful such as
  `{ status: 'Copy this: http://127.0.0.1:43821/myext/callback' }`; or
- only emit copy controls when `context.hostFeatures` includes
  `settings.copy_action`, and render the text another way (an `info`
  block) otherwise.

### Scratch fields and what gets saved

When the user clicks Save, the host validates (`normalize()`, then
`validate()`) and folds the page's value controls into your stored
config:

- **Only this page's keys change** (apart from the scratch and `enabled`
  keys below, which Save always removes). Saved keys that no control on
  the page owns are kept, whether another step of a setup wizard shows
  them or no control does. A control whose value equals the manifest default
  is not stored, and removes any override it had. A value for a declared
  key is stored as the declared type. Values for ids the manifest doesn't
  declare are stored as they are. If nothing is left to store, the
  extension has no entry in `config.extensions`.
- **Ids starting with `__` are scratch fields.** They render, and their
  values reach `runAction`, `normalize()`, `validate()`, and
  `onFileSelected`, but the host never saves them, and ignores any
  saved value, so the control always starts at its `default`. Older
  hosts did save them, so every Save removes all `__` keys from your
  stored config, whether or not the page has a control for them. Don't
  store your own data under such keys; use `save_extension_data`. Use
  scratch fields for inputs that feed an action — a "paste your Client
  ID" box whose action stores the value itself — rather than settings.
- **A control with id `enabled` is the host's** (see
  [Config Schema](#config-schema)): it's hidden, holds the extension's
  on/off state, and is never saved. The header's Disable/Enable button
  is the switch. Save also removes a stored `enabled` key.
- **`normalize()` rewrites values; it can't add or remove them.** Each
  key it returns is written back into its control (so the user sees
  it) and saved from there. A key it leaves out keeps the value its
  control shows, so returning only the keys you changed is fine. Only
  ids of value controls in your schema are ever saved, so it can't add
  keys either. To keep a value out of your config, make it a scratch
  field.
- A `refresh` keeps whatever the user has typed, scratch fields
  included. A control that only the refreshed page has (the next step
  of a setup wizard) shows the user's saved value, like any control on
  the first render, so a Save from that step doesn't reset it.

### Conditional visibility

Any control can declare a `showWhen` clause that hides it until another
control has a specific value:

```js
{ type: 'text', id: 'custom_sound_path', label: 'Custom sound path',
  showWhen: { id: 'sound_id', equals: 'custom' } }
```

Also supports `{ id, oneOf: ['a', 'b'] }`.

### Host side effects

Some actions need host-side capabilities (file dialog, audio playback,
download) that the sandbox can't perform. Return a `host` object from
`runAction` to request one:

| Effect                | Fields                                                     | Callback                     |
|-----------------------|------------------------------------------------------------|------------------------------|
| `refresh`             | —                                                          | `getSettings`                |
| `download`            | `filename`, `content`, `mime?`                             | —                            |
| `pick_file`           | `accept?` (e.g. `.json`), `action?` (routed back)          | `onFileSelected`             |
| `play_timer_sound`    | `soundId`, `customPath?`, `repeats?`                       | —                            |

`refresh` re-fetches `getSettings()` and re-renders the page in place,
so an action that changed derived state (signed in/out, a step of a
setup wizard, which buttons are `disabled`, an `info` block's text)
shows up everywhere, not just in the status line. What the user has
typed is kept, scratch fields included, and the `status` you returned
alongside the effect stays on the button:

```js
return { status: '✅ Connected', host: { type: 'refresh' } };
```

Unknown effect types are ignored.

### Info block HTML sanitization

The `info` control's HTML is passed through a strict sanitizer. Allowed
tags: `a, b, br, code, div, em, i, li, ol, p, span, strong, ul`. Links
need `href` starting with `http(s):`, `mailto:`, or `#`. Unknown tags
are replaced by their text content; unknown attributes are dropped.

## Widgets

Widgets are persistent UI controlled by the extension, mounted into a
host slot. Extensions return an HTML string from `render()`; the host
sanitizes it (see [HTML sanitization](#extension-html-sanitization))
and injects it. Interactive buttons wire through declared action IDs —
there are no live callback functions crossing the sandbox boundary.

**Available slots:**

| Slot              | Location                                         |
|-------------------|--------------------------------------------------|
| `floating-bottom` | Below the offline banner, above the input area  |
| `floating-status` | Small status indicator area                      |

**Manifest:**

```json
{
  "contributes": {
    "widgets": [
      { "id": "next-meeting", "slot": "floating-bottom", "module": "./widget.js" }
    ]
  }
}
```

**Provider contract:**

```js
export default class MyWidget {
    initialize(context) {
        this.config = context.config || {};
        this.invoke = context.invoke;
    }

    onConfigUpdate(config) { this.config = config || {}; }

    /**
     * How often the host should call render() (ms). Return 0 to render
     * only on mount + config change. Typical values: 60_000 for a
     * minute-ticking overlay; 5*60_000 for a due-reminder check.
     */
    getRefreshInterval() { return 60_000; }

    /**
     * Return the widget's current state:
     *   - { html, className?, actions? }  — render a new view
     *   - null                            — hide the widget
     *
     * `actions` is an array of { id, rpc } — each `data-ext-action="<id>"`
     * element in the HTML is wired to call onAction(rpc) on click.
     */
    async render() {
        return {
            className: 'extension-bar',
            html: `<span>Hello</span>
                   <button data-ext-action="dismiss" class="extension-bar-btn">✕</button>`,
            actions: [{ id: 'dismiss', rpc: 'dismiss' }],
        };
    }

    async onAction(actionId) {
        if (actionId === 'dismiss') {
            // ... update internal state ...
            return { rerender: true }; // re-render immediately
        }
        return {};
    }

    destroy() {}
}
```

Return `{ rerender: true }` from `onAction` to force an immediate
re-render (outside the normal refresh interval).

## Toolbar Button Provider API

Toolbar buttons appear in the chat window toolbar. The extension
declares buttons as plain data and handles clicks via an RPC.

```js
export default class MyToolbarProvider {
    initialize(context) {
        this.invoke = context.invoke;
        this.config = context.config || {};
    }
    onConfigUpdate(config) { this.config = config || {}; }

    /** Declarative button definitions (no live callbacks). */
    getButtons() {
        return [
            { id: 'show-summary', icon: '✅', tooltip: 'Show summary' },
        ];
    }

    /**
     * Called when the user clicks a button.
     * @param {string} buttonId - id from getButtons()
     * @param {object} ctx - { input: string, messages: Array }
     * @returns {Promise<{ host?: object }>}
     */
    async onClick(buttonId, ctx) {
        if (buttonId === 'show-summary') {
            const data = await this.invoke('load_extension_data', { key: 'my-data' });
            return {
                host: {
                    type: 'show_ephemeral_message',
                    tag: 'summary',
                    title: '📋 Summary',
                    html: `<p>You have <strong>${data.count}</strong> things.</p>`,
                },
            };
        }
        return {};
    }

    destroy() {}
}
```

### Button icon

The icon field accepts either:

- **An emoji or short text string** — rendered as-is. Easiest, no
  styling concerns.
- **An inline SVG markup string** — rendered as a real SVG element.
  The host runs the string through the extension sanitizer's `icon`
  mode: `<svg>` and SVG-namespace children (`<path>`, `<circle>`,
  `<rect>`, `<polyline>`, etc.) survive; anything else (`<script>`,
  `<a>`, `<img>`, `on*` event handlers, `javascript:` URLs) is
  stripped. Use `currentColor` for `stroke`/`fill` so the icon
  inherits the toolbar's themed text colour.

Example:

```js
getButtons() {
    const icon = `<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M9 11l3 3L22 4"/></svg>`;
    return [{ id: 'show-summary', icon, tooltip: 'Show summary' }];
}
```

### Host effects from onClick

| Effect                   | Fields                                   | Description                                                         |
|--------------------------|------------------------------------------|---------------------------------------------------------------------|
| `set_chat_input`         | `value: string`                          | Replace the chat input text and focus it.                           |
| `append_chat_input`      | `value: string`                          | Append to the chat input (with a space separator).                  |
| `show_ephemeral_message` | `tag?`, `title?`, `html`                 | Render a sanitized ephemeral bubble in the chat. Same `tag` replaces the previous bubble from this extension. |

## Message Formatter API

A message formatter rewrites the rendered HTML of a message bubble
after markdown rendering. The extension receives the HTML as a
string, returns replacement HTML (sanitized by the host before
injection).

```js
export default class MyFormatter {
    initialize(context) {
        this.config = context.config || {};
        this.invoke = context.invoke;
    }
    onConfigUpdate(config) { this.config = config || {}; }

    /**
     * @param {string} html - serialized innerHTML of the message container
     * @param {{ streaming: boolean, role: string }} context
     * @returns {Promise<string | null>} replacement HTML, or null to skip
     */
    async format(html, context) {
        if (context.streaming) return null; // often the right call
        // parse, transform, return a new HTML string
        const doc = new DOMParser().parseFromString(
            `<!doctype html><body>${html}</body>`, 'text/html',
        );
        // ... add preview cards, annotations, etc.
        return doc.body.innerHTML;
    }

    destroy() {}
}
```

Parse with `DOMParser` — the resulting document is inert (no scripts
run) and lives entirely inside the sandbox. The host re-sanitizes
the returned string before inserting it into the live DOM.

### Progressive formatting

A formatter that needs slow data (e.g. fetching link metadata) doesn't
have to block the message on it. Return placeholder markup right away,
then call `context.formatter.requestReformat()` once the data arrives.
The host coalesces bursts and calls `format()` again on every
still-attached message it already formatted, passing the message's
*current* HTML — including your placeholders — so `format()` must be
idempotent: upgrade placeholders in place and never insert a second
copy. Like `context.mascot`, it carries no data and needs no capability.
`context.formatter` is absent on older hosts; fall back to awaiting the
data there.

Image `src` accepts `http(s)` URLs and base64 raster `data:` URIs
(PNG, GIF, JPEG, WebP, ICO; no SVG).

## Custom `renderResult`

Search providers can optionally provide custom HTML for their result
rows. The search provider returns `{ html, className? }` from
`renderCustom(result)`. The host caches the output, sanitizes it, and
injects it into the result row element.

```js
export default class MySearchProvider {
    // ... initialize, match, matchAsync, execute ...

    renderCustom(result) {
        const e = result?.data;
        if (!e) return null;
        return {
            html: `<div class="app-icon">📅</div>
                   <div class="app-info" style="flex:1">
                       <div class="app-name">${escapeHtml(e.subject)}</div>
                   </div>
                   <button data-ext-action="join:${escapeHtml(result.id)}"
                           class="extension-bar-btn">Join</button>`,
        };
    }

    // Called when a data-ext-action button in a rendered row is clicked.
    async onResultAction(actionId, ctx) {
        if (actionId.startsWith('join:')) {
            // ... look up the event, invoke open_url ...
        }
        return {};
    }
}
```

## Extension HTML sanitization

HTML strings returned by widgets, custom renderers, message
formatters, and ephemeral messages are filtered through a host-side
sanitizer before injection.

**Always stripped:**

- `<script>`, `<style>`, `<iframe>`, `<object>`, `<embed>`, `<link>`,
  `<meta>` tags (not even their text content in most cases).
- All `on*` event handler attributes (`onclick`, `onmouseover`, …).
- The `id` attribute on any element (to prevent selector collisions
  with host-owned ids).
- `data-*` attributes other than `data-ext-action` and (on a `<button>`,
  with a real `YYYY-MM-DD` value) `data-kage-date` — see
  [Date picker](#date-picker).
- URLs using `javascript:`, `vbscript:`, `data:`, `file:`, `blob:`
  schemes. Only `http(s):`, `mailto:`, and in-page `#anchors` are
  allowed on `<a href>`. `<img src>` only allows `http(s):`.
- Inline `style` declarations referring to `url(...)`, `expression()`,
  `position: fixed`, or unknown properties.

**Allowed in rich mode (widgets / formatters / ephemeral bubbles):**
block tags including `div`, `h1`–`h6`, `p`, `ul`, `ol`, `li`,
`blockquote`, `pre`, `table`, `details`, `summary`, plus inline tags,
`img`, `button`, and inline SVG (for icons).

**Allowed in inline mode (toolbar icons):** restricted subset; block
tags are replaced with their text content.

**data-ext-action attribute:** preserved on `<button>`, `<a>`,
`<span>`, `<div>`. The host wires each such element to an RPC on the
owning provider (see widget/renderResult contracts above).

**Links:** when the user clicks an `http(s)`/`mailto` link in your HTML
(widgets, formatters, result rows, settings `info` blocks), it opens
in their default browser / mail client — never inside the Kage window,
with or without `target="_blank"` (which is ignored). That needs no
capability, because the user is the one clicking. A link that is (or
sits inside) a `data-ext-action` element runs your action instead and
is not opened. Opening a URL from code (`context.invoke('open_url', …)`)
still requires `urls`. A sandbox that navigates its own iframe
(`location.href = …`) never reaches the browser.

## Tool Provider API

A tool provider exposes extension functionality to the LLM agent. When the agent needs data from an extension (e.g. calendar appointments), it calls the tool locally — no cloud round-trip needed.

```js
export default class MyToolProvider {
    /**
     * Called once when the extension loads.
     * @param {object} context - { invoke, config }
     */
    initialize(context) {
        this.invoke = context.invoke;
        this.config = context.config;
    }

    onConfigUpdate(config) {
        this.config = config;
    }

    /**
     * Return tool definitions the LLM can call.
     * Called once on load and after config changes.
     * @returns {ToolDef[]}
     */
    getTools() {
        return [
            {
                name: 'list_items',
                description: 'List items from this extension',
                parameters: {
                    limit: { type: 'number', description: 'Max items', default: 10 }
                }
            }
        ];
    }

    /**
     * Execute a tool call. Must return { result } or { error }.
     * Has a 5-second timeout — keep operations fast.
     * @param {string} toolName
     * @param {object} params
     * @returns {Promise<{result?: any, error?: string}>}
     */
    async execute(toolName, params) {
        return { result: { items: [] } };
    }

    destroy() {}
}
```

### How It Works

1. Extension loads → `getTools()` definitions are collected
2. Tool definitions are sent to the agent as a steering message
3. Agent emits a `` ```extension_tool_call``` `` fenced block when it wants to call a tool
4. Frontend detects the fence, executes the tool via `execute()`, sends result back to agent
5. Agent continues its response with the tool result data

### Manifest

Add `toolProvider` to `contributes`:

```json
{
  "contributes": {
    "toolProvider": "./tools.js"
  }
}
```

### ToolDef Shape

```js
{
    name: string,           // Tool name (used in the call)
    description: string,    // What the tool does (shown to the LLM)
    parameters: {           // Parameter definitions
        paramName: {
            type: string,       // 'string', 'number', 'boolean'
            description: string, // Shown to the LLM
            default: any        // Optional default value
        }
    }
}
```

## Trigger Provider API

A trigger provider defines signals that the extension can emit, which can be used to trigger automations. It's an ES module that default-exports a class:

```js
export default class MyTriggerProvider {
    initialize(context) {
        this.invoke = context.invoke;
    }

    onConfigUpdate(config) {}

    /**
     * Return an array of trigger definitions this extension can emit.
     * These appear in the Automations settings signal picker.
     */
    getTriggers() {
        return [
            { name: 'my-ext:something_happened', description: 'When something happens', icon: '⚡' },
            { name: 'my-ext:threshold_reached', description: 'When a threshold is reached', icon: '📊' },
        ];
    }

    destroy() {}
}
```

### Emitting Signals

When the trigger condition is met, call the `emit_automation_signal` Tauri command:

```js
this.invoke('emit_automation_signal', {
    name: 'my-ext:something_happened',
    data: { key: 'value', details: '...' }
});
```

The automation scheduler will match the signal name against configured automations and fire any that match.

### Manifest

Add `triggerProvider` to `contributes`:

```json
{
  "contributes": {
    "triggerProvider": "./triggers.js"
  }
}
```

### Built-in System Signals

These signals are always available (no extension needed):

| Signal | Description |
|--------|-------------|
| `system:clipboard_change` | Clipboard content changed |
| `system:window_focus` | A window gained focus |
| `system:idle_5m` | System idle for 5 minutes |
| `system:resume` | System resumed from sleep |

## Mascot activity hints

The Kage mascot can play **activity animations** — bopping along to
music, wearing a headset for a meeting, watching an hourglass for a
timer. Extensions tell it *what kind of thing the user is doing*; the
mascot decides what to do about it.

```js
// Something is playing → let the mascot bop. Re-assert on every refresh.
context.mascot.setActivity('music', { ttlMs: 30000 });

// Nothing is playing any more.
context.mascot.clearActivity();
```

### The vocabulary is closed and generic

| Activity | Meaning |
|----------|---------|
| `music`  | Audio is actively playing. |
| `meeting`| The user is in (or joining) a call. |
| `timer`  | A countdown / timer is running. |

The mascot must **never learn about specific extensions**. You say
`'music'`, not `'spotify'` — any number of extensions can mean the
same thing, and the mascot maps the generic activity to whatever
poses it has. Anything outside the table above is dropped with a
console warning (the same way unknown `permissions` entries are
dropped), so don't invent names: propose an addition to the
vocabulary instead.

### Declare it in the manifest

```json
{
  "contributes": {
    "mascotActivities": ["music"]
  }
}
```

The host rejects any activity the manifest didn't declare, even a
valid one. This keeps the set of things an extension can make the
mascot do auditable from the manifest alone.

### No capability needed — but the user can turn it off

`context.mascot` carries no data, never reaches Tauri, and comes from
a closed vocabulary, so it is **not** gated by a capability and does
not appear in the install-time permission prompt. It is still a
user-facing feature: **Settings → Appearance → "React to extension
activity"** (`ui.mascot_extension_hints`) disables it globally, and
hints are silently dropped while it's off. Never treat a hint as
having been applied — it is advisory, fire-and-forget, and returns
nothing.

### Hints are a lease — re-assert, don't set-and-forget

A hint expires. `ttlMs` is clamped into **15s – 10min**, defaulting to
**30s** when omitted or unusable. When the lease expires the mascot
drops the activity on its own.

This is deliberate: a crashed, hung, disabled, or hidden extension
must not be able to pin a pose forever. The correct pattern is to
re-assert the activity from the loop you already have — a widget's
`render()`, a poll tick, whatever refresh cycle already tells you the
state:

```js
export default class PlayerWidget {
    initialize(context) {
        this.ctx = context;
    }
    getRefreshInterval() {
        return 15000; // comfortably inside the 30s default lease
    }
    async render() {
        const state = await this.fetchState();
        if (state.playing) {
            this.ctx.mascot.setActivity('music', { ttlMs: 45000 });
        } else {
            this.ctx.mascot.clearActivity();
        }
        return { html: /* … */ '' };
    }
}
```

Pick a TTL comfortably longer than your refresh interval so a single
slow tick doesn't make the mascot flicker, but short enough that a
stale state corrects itself quickly.

The host also withdraws your hint for you when your sandbox is torn
down (unload, disable, uninstall, window close), when a widget's
circuit breaker trips, and when the user turns mascot hints off.

### Arbitration

Several extensions can hint at once. The mascot side owns the
arbiter — it picks a single winner by its own priority rules and
handles expiry. Your extension can't influence that beyond declaring
its activity, and shouldn't assume its hint is the one being shown.

## Date picker

The floating window has a host-owned month calendar (prev/next month,
month and year lists, Today, full keyboard support, busy-day dots). It
opens from the date in the input bar — or with **Alt+↓** (⌥↓) in the
launcher input, which opens it from the highlighted result's date when
it has one — and an extension holding the
**`calendar`** capability can open it from its own date labels and feed
it data. The popup is host UI: your code never renders into it or sees
its DOM. You take part through three narrow channels, available only
when `context.hostFeatures` includes `'date-picker'`:

```js
const canPick = Array.isArray(context.hostFeatures)
    && context.hostFeatures.includes('date-picker');
```

**1. Openers.** Mark a date label in your widget or result HTML as a
button with an ISO day:

```html
<button type="button" data-kage-date="2026-11-26">Thu, Nov 26</button>
```

The host styles it, labels it for screen readers, and opens the
calendar anchored to it on that day. Re-rendering (a widget refresh)
while the calendar is open is fine: the new button for the same date
takes over as its opener. The attribute survives
sanitization only on a `<button>` with a real date; where your extension
may not use the picker (no `calendar` grant, not a date provider, or a
window without the popup) the button is unwrapped to plain text. On an
older host (`hostFeatures` undefined) emit plain text yourself.

**2. Busy-day dots.** The search provider's optional
`getDateMarkers(start, end)` answers which days in `[start, end)`
(ISO, end exclusive — one call per visible 6-week grid) have events:

```js
async getDateMarkers(start, end) {
    try {
        // [{ date: 'YYYY-MM-DD', count }] — counts only, never titles.
        const days = await this.ctx.invoke('get_calendar_busy_days', { start, end });
        return { status: 'ok', days };
    } catch (e) {
        if (e.key === 'errors.calendar.access_denied') return { status: 'permission_denied', days: [] };
        return { status: 'error', days: [] };
    }
}
```

`days` may also be an object keyed by date, and each entry may carry up
to three `colors` (`#rgb`/`#rrggbb` hex) for per-calendar dots. The host
drops anything outside the range, clamps counts, ignores non-hex
colours, and maps `status` (`ok` / `permission_denied` / `error`) to its
OWN hint text — you can't put words in the popup. Answers are cached
for a minute per range (`error` for five seconds), so paging back and
forth is free, and while the user pages the host asks only for the
month they stop on, one lookup at a time.
`get_calendar_busy_days` takes at most 62 days and needs `calendar`.

**3. Selection.** Implementing `onDateSelected(date)` on the search
provider is what makes you a date provider. Return the launcher query
that lists that day:

```js
onDateSelected(date) {
    return { query: `cal ${date}` };
}
```

The host fills the input with it (as an undoable edit — a draft it
replaced comes back with Ctrl/⌘+Z) and runs the search, so the day's
results (or your empty state) render through your normal result rows.
The query must be a single line that starts with one of **your own**
registered keywords (`getKeywords`), or it's ignored. For a pick made
from the input bar's date, the first enabled date provider answers. An
enabled extension with `calendar` and a search provider but no
`onDateSelected` (e.g. a Calendar extension from before the picker) gets
an "update the extension" hint in the popup rather than "turn it on".

Date labels you show next to the popup should be Gregorian
(`toLocaleDateString(locale, { calendar: 'gregory', … })`): the grid is,
and a locale whose default calendar differs (fa → Persian) would
otherwise label a different-looking day than the one it highlights.

The calendar extension is the reference implementation
(`Kage-Extensions/extensions/calendar/`: `dates.js`, `search.js`,
`widget.js`).

## Theme Format

Themes override CSS variables defined in `shared-kage-tokens.css`:

```json
{
  "id": "my-theme",
  "name": "My Theme",
  "version": "1.0.0",
  "type": "theme",
  "description": "A custom color scheme.",
  "icon": "🎨",
  "author": "Your Name",
  "contributes": {
    "themes": {
      "dark": "./dark.json",
      "light": "./light.json"
    }
  }
}
```

Color file (`dark.json`):

```json
{
  "name": "My Theme Dark",
  "colors": {
    "kage-accent": "#FF6B6B",
    "kage-accent-hover": "#FF8E8E",
    "kage-bg": "#1a1a2e",
    "kage-text": "#e0e0e0"
  }
}
```

Only override the variables you want to change. Unspecified variables fall back to the built-in defaults. See `ui/css/shared-kage-tokens.css` for all available variables.

## Installation

Extensions are distributed as `.zip` files. The zip should contain either:
- Files directly at the root (including `manifest.json`), or
- A single subdirectory containing `manifest.json`

Install methods:
1. **Store** — browse and install from the Extension Store
2. **Local file** — install from a `.zip` file or directory path
3. **Manual** — place the extension directory in `<config_dir>/kage/extensions/<id>/`

## Enable / Disable

Each extension has an Enable/Disable toggle in its settings page. Disabled extensions:
- Don't appear in search results
- Settings are greyed out but visible
- Can be re-enabled without reinstalling

## Extension Lifecycle

1. App starts → `ExtensionManager.initialize()` loads all enabled extensions
2. For each extension: fetch `manifest.json`, dynamically import search provider, call `initialize(context)`
3. On every keystroke: `match(query)` called synchronously, then `matchAsync(query)` for expensive ops
4. On config change: `onConfigUpdate(config)` called with new config
5. On disable/unload: `destroy()` called
