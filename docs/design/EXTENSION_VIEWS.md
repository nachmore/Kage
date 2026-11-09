# Extension views: full windows that an extension draws

Status: design, ready to build. Nothing here is implemented yet.

This document specifies a new extension surface, the **view**. A view is
an on-demand Kage window whose content is drawn by an extension: for
example Mind Map's full-window pan/zoom explorer. It keeps the existing
sandbox model (a sandboxed iframe without `allow-same-origin`, one
`MessagePort`, host-side capability checks) and adds one capability,
`views`.

It also specifies **storage change events**, which every extension
instance gets, so a view and the extension's other instances (floating,
chat, settings) stay in step through shared storage.

Read first: [`docs/EXTENSIONS.md`](../EXTENSIONS.md),
[`docs/SECURITY_MODEL.md`](../SECURITY_MODEL.md),
[`docs/I18N.md`](../I18N.md). The last section,
[Mind Map integration](#24-mind-map-integration), says exactly what Mind
Map (`Kage-Extensions`, `extensions/mindmap/`) does with this API.

---

## Contents

1. [Goals and non-goals](#1-goals-and-non-goals)
2. [Terms](#2-terms)
3. [Overview](#3-overview)
4. [Manifest: `contributes.views`](#4-manifest-contributesviews)
5. [The `views` capability](#5-the-views-capability)
6. [Opening a view](#6-opening-a-view)
7. [Window lifecycle](#7-window-lifecycle)
8. [The view page and its sandbox](#8-the-view-page-and-its-sandbox)
9. [Runtime API in the sandbox](#9-runtime-api-in-the-sandbox)
10. [Talking to the extension's other instances](#10-talking-to-the-extensions-other-instances)
11. [Theming](#11-theming)
12. [i18n and RTL](#12-i18n-and-rtl)
13. [Input: pointer, wheel, keyboard, clipboard, drops](#13-input-pointer-wheel-keyboard-clipboard-drops)
14. [Performance](#14-performance)
15. [Focus and blur, and the floating launcher](#15-focus-and-blur-and-the-floating-launcher)
16. [Tauri commands, events and i18n keys](#16-tauri-commands-events-and-i18n-keys)
17. [Security analysis](#17-security-analysis)
18. [UX](#18-ux)
19. [Cross-platform notes](#19-cross-platform-notes)
20. [Backups](#20-backups)
21. [Folder sync](#21-folder-sync)
22. [Test plan](#22-test-plan)
23. [Work packages](#23-work-packages)
24. [Mind Map integration](#24-mind-map-integration)
25. [Open questions and later work](#25-open-questions-and-later-work)
26. [Alternatives considered](#26-alternatives-considered)

---

## 1. Goals and non-goals

Goals:
- An extension can declare one or more full windows and draw them itself:
  DOM, Canvas 2D, SVG, pointer, wheel and keyboard events.
- The same trust model as every other contribution point. The extension
  never reaches Tauri. Every `invoke()` goes through the host's capability
  check.
- Views open only from a user action: a launcher result, a formatter card
  button, a widget button, a toolbar button, a settings action, or the
  host's own "Open" button.
- Windows are on demand (like Settings and the Store), cached while
  closed, and remember their size and position per view, per machine.
- A view and the extension's other instances share data through
  extension storage, with change events so nobody has to poll.
- Kage's theme, language and text direction reach the view.
- Opening a view from the launcher doesn't dismiss the launcher.

Non-goals (v1):
- Views inside the floating window or chat window (panels, tabs, docks).
- Transparent, frameless, always-on-top or click-through windows.
- Several windows of the same view at once. Each view is a singleton.
- Loading remote content. Views can't reach the network at all.
- Opening a view from outside Kage (deep links, other apps) or from an
  agent tool call without a click.
- Extension-supplied images or fonts from the bundle (see §25).

## 2. Terms

| Term | Meaning |
|---|---|
| **View** | One entry in `contributes.views`: an id, a title and a module. |
| **View window** | The Tauri window that shows a view. Label `ext-view:<extensionId>:<viewId>`. |
| **View page** | The first-party page loaded in a view window: `ui/extension-view.html`. Trusted. |
| **View sandbox** | The visible sandboxed iframe inside the view page that runs the view module. Untrusted. |
| **Host bar** | The thin strip at the top of the view page, drawn by Kage, outside the iframe. |
| **Opener** | The window whose host code asked to open the view (floating, main, `chat-*`, settings, or another view of the same extension). |
| **Instance** | One sandbox of an extension in one window. An extension has one instance per window that loads it. A view window adds one more. |

## 3. Overview

```
 floating / chat / settings window (first-party)
 ┌─────────────────────────────────────────────┐
 │ hidden sandbox iframe (extension instance)  │  user clicks "Open explorer"
 │   execute() → {type:'open_view', ...}       │──┐
 └─────────────────────────────────────────────┘  │ host validates, then
                                                  ▼ invoke('ext_view_open')
                                   Rust: validate manifest + grant + enabled,
                                   build or show window "ext-view:mindmap:explorer",
                                   place it, remember params
                                                  │
 view window (first-party view page)              ▼
 ┌─────────────────────────────────────────────┐
 │ host bar: 🧠 Mind Map · Explorer   ⟳ ⚙ ✕    │  Kage-drawn, outside the iframe
 ├─────────────────────────────────────────────┤
 │ visible sandbox iframe (view instance)      │  canvas, pointer, wheel, keys
 │   mount(root, {params})                     │
 │   context.invoke(...) ── MessagePort ──►    │  same capability checks
 │   context.storage.onChanged(...)            │◄── extension_data_changed
 └─────────────────────────────────────────────┘
```

Everything that decides what an extension may do stays in first-party code:
the view page's `ExtensionSandbox` (JS) and the Rust commands. The view
sandbox only draws and asks.

## 4. Manifest: `contributes.views`

```json
{
  "permissions": ["storage", "views"],
  "contributes": {
    "views": [
      {
        "id": "explorer",
        "title": "__MSG_view.explorer.title__",
        "icon": "🗺️",
        "module": "./explorer.js",
        "css": ["./explorer.css"],
        "defaultSize": { "width": 1200, "height": 800 },
        "minSize": { "width": 640, "height": 420 }
      }
    ]
  }
}
```

Field rules. The host applies them in Rust (`src/extensions/views.rs`)
and mirrors them in JS (`ui/js/shared/extension-view-policy.js`). A bad
entry is dropped with a warning in the log and console, like an unknown
capability. A bad `views` value never makes the manifest unreadable: Rust
keeps it as raw JSON (`Option<serde_json::Value>`, the same pattern as
`sensitiveData`) and normalises it on read.

| Field | Required | Rule |
|---|---|---|
| `views` | no | Array. At most **4** entries; the rest are ignored. |
| `id` | yes | `^[a-z][a-z0-9-]{0,31}$`, unique within the extension. |
| `title` | yes | 1–60 characters after `__MSG_*__` resolution. Control and bidi-override characters are removed. Missing or empty: the extension's name. |
| `icon` | no | One emoji, at most 8 characters. Default: the extension's icon. |
| `module` | yes | A bundle-relative `.js` path, with the same path rules as other providers. It may import other bundle files (same loader as other providers). |
| `css` | no | At most 8 bundle-relative `.css` files, each ≤ 256 KB, total ≤ 1 MB. Injected into the view document only, never into host windows. |
| `defaultSize` | no | Logical pixels. Width clamped to 480–3840, height to 360–2160. Default 1100 × 760. |
| `minSize` | no | Logical pixels. Clamped to 320 × 240 up to `defaultSize`. Default 480 × 360. |

Anything else in an entry is ignored. In particular a manifest can't ask
for position, always-on-top, transparency, no frame, full screen, or
"skip taskbar".

The Rust `ExtensionContributes` struct gains `views` so the field survives
the manifest round trip through `list_extensions` (the same reason
`mascot_activities` is modelled there). The round-trip test in
`src/extensions.rs` adds `views` to its key list.

## 5. The `views` capability

| Capability | Icon | Grants |
|---|---|---|
| `views` | 🖼️ | Show the windows the manifest declares in `contributes.views`, drawn by the extension itself. |

- Added to `VALID_CAPABILITIES` (`src/extensions.rs`) and `CAPABILITIES`
  (`ui/js/shared/extension-permissions.js`). The alignment test keeps the
  two lists equal.
- Install prompt label: "Its own windows". Description: "Show windows the
  extension draws itself, such as a map or a board. What you type in those
  windows goes to the extension. Kage labels them with the extension's
  name."
- No Tauri command maps to `views`. The capability gates host behaviour:
  the `open_view` action and effect, the `views` host feature, and
  `ext_view_open` (which re-checks the grant in Rust).
- An extension that adds `views` in an update runs without it until the
  user approves (existing behaviour: extras are dropped until
  re-approval). Its `open_view` actions are refused and `views` is absent
  from `context.hostFeatures`.
- What the view instance may `invoke()` is the extension's normal grant,
  minus the view-surface denylist (§8.6). `views` doesn't widen anything
  else.

Why a new capability rather than reusing `window` (Kage window chrome):
until now no extension code paints directly to the user or receives raw
keystrokes. Everything else is HTML the host sanitises and injects. A
window the extension draws is a new kind of trust (it can imitate other
UI, §17), so the user should see it at install time.

## 6. Opening a view

### 6.1 Entry points

| Where the click happens | What the extension returns | Host code that opens |
|---|---|---|
| Launcher or chat result row (Enter or click) | `execute()` → `{ type: 'open_view', view, params? }` | `ui/js/shared/result-executor.js` |
| `data-ext-action` button in a formatter card or custom result row | `onResultAction()` → `{ host: { type: 'open_view', view, params? } }` | `ui/js/shared/extension-manager/ui.js` |
| Widget button | `onAction()` → `{ host: { type: 'open_view', ... }, rerender? }` | `ui/js/shared/extension-manager/ui.js` |
| Chat toolbar button | `onClick()` → `{ host: { type: 'open_view', ... } }` | `ui/js/shared/toolbar-host-effects.js` |
| Extension settings action | `runAction()` → `{ host: { type: 'open_view', ... } }` | `ui/js/shared/settings-host-effects.js` |
| Host "Open" button in Settings → the extension's page | none (host-rendered) | `ui/js/settings/manager.js` |
| Inside a view | `context.view.open(viewId, params?)` (another view of the same extension) | view page |

Today `onResultAction`'s return value is ignored. It becomes honoured for
`host.type === 'open_view'` only. Widget actions keep `rerender` and gain
`host`.

All of these run inside the handling of a user's click or key press, so
"views only open from a user action" holds without a separate gesture
token. The extension can delay its answer by at most the RPC timeout
(10 s). There is deliberately no `context.views.open()` callable from any
provider at any time: a tool provider or a timer can't pop windows.

### 6.2 The effect

```js
{ type: 'open_view', view: 'explorer', params: { focus: 'e1a2b3c4d5e6f7g' } }
```

- `view`: a view id the manifest declares.
- `params`: optional. A JSON object (plain objects, arrays, strings,
  finite numbers, booleans, null), at most 4096 bytes serialised and
  nesting depth ≤ 4. It reaches the view as `params` (§9). It never goes
  into a URL.

Host-side checks before `ext_view_open`, in this order. A failure is
logged as `[ext <id>] open_view refused: <reason>` and nothing opens.
1. The window offers the `views` host feature.
2. The extension holds `views` in this window's grant.
3. The manifest declares `view`.
4. `params` passes the shape and size rules.

Rust then repeats checks 2–4 against the installed manifest and
`config.extension_grants` (authoritative), and also checks:
- the extension is installed and enabled;
- the opener's label is `floating`, `main`, `chat-*`, `settings` or
  `ext-view:*` (context menu, welcome, store and inline assist never open
  views);
- an opener that is itself a view window opens only views of its own
  extension.

### 6.3 What happens

New window:
1. Opener host code validates (§6.2). The floating window also arms its
   blur suppression (§15.2).
2. `invoke('ext_view_open', { extensionId, viewId, params, source })`.
3. Rust validates, computes the placement (§7.5), builds the window hidden,
   stores the boot record (extension, view, params, `open_seq = 1`,
   source), shows and focuses the window, returns
   `{ label, reused: false }`.
4. The view page boots: `ext_view_boot` returns the boot record, derived
   from the calling window's label. The page cannot ask for another
   extension's boot record.
5. The view page loads the manifest, grant, config, locale and view
   sources, creates the view sandbox, and calls `viewMount(params)`.
6. When `mount` resolves, the host hides its loading state.

Window already exists (visible or cached hidden):
1. Steps 1–2 as above.
2. Rust bumps `open_seq`, stores the params, shows and focuses the window.
   If the page has booted, Rust emits `ext_view_reopen
   { params, open_seq, source }` to that window. If it hasn't booted yet,
   the page picks up the newest params from `ext_view_boot`.
3. The view page calls `viewReopen(params)`, which reaches `onOpen`.

Failure: `ext_view_open` rejects with an `AppError` (§16). The launcher
shows its localised message as a reply; other openers log it, and settings
shows it on the action row.

## 7. Window lifecycle

### 7.1 Labels

- `window_labels::EXT_VIEW_PREFIX = "ext-view:"`, with
  `ext_view_label(ext, view)`, `parse_ext_view_label(label) ->
  Option<(&str, &str)>` and `is_ext_view_label(label)`. The JS mirror lives
  in `ui/js/shared/window-labels.js`. Never build or match the prefix
  inline.
- Both parts are validated before a label is built (extension id rules as
  today, view id rule §4), so a label can't contain another separator.

### 7.2 Window shape

Built with `WebviewWindowBuilder` in `src/ext_views.rs`, like Settings and
the Store:
- URL `extension-view.html` with no query string. Identity comes from the
  label.
- Native decorations, resizable, can minimise and maximise, native full
  screen allowed.
- Not always-on-top, shown in the taskbar and Dock.
- `min_inner_size` from `minSize`.
- `.visible(false)` until placed, then shown.
- `.guard_new_windows(&app)` (new-window requests denied or forwarded, as
  for other on-demand windows).
- Background colour from the current theme (dark `#1e1e1e`, light
  `#fafafa`), to avoid a white flash.
- Initial title `Kage`. The view page sets the real title within its first
  frame (§18.2).
- The Tauri drag-and-drop handler stays on (the default), and the view page
  ignores its events (§13.5).

On macOS it is a normal `NSWindow`, not an `NSPanel` (§15.1). View windows
are never converted with `overlay_windows::configure`, so the overlay
invariants (never destroy) don't apply. Destroying them is safe.

### 7.3 Singleton, limits and caching

- One window per `(extension, view)`. Opening again focuses it and
  delivers new params (§6.3).
- At most **4** view windows exist at once, visible or cached, and at
  most **2** of them hidden. Each one is its own WebView (on Windows a
  WebView2 controller with its own renderer process, tens of MB each, the
  cost that keeps Settings and the Store out of the startup window list),
  so a cache of hidden windows has to stay small. When a 5th window is
  requested, or a 3rd window is hidden, Rust destroys the least recently
  hidden one. If all 4 are visible, `ext_view_open` fails with
  `errors.ext_view.too_many_open`.
- **Close hides.** `CloseRequested` is already turned into a hide by
  `setup::handle_window_close`. For view windows Rust also:
  - emits `ext_view_visibility { visible: false }` to the window (the page
    calls `onHide`);
  - starts a teardown timer: after **10 minutes** hidden, the window is
    destroyed. A reopen within that time is instant and keeps in-memory
    state. A generation counter cancels the timer on reopen.
  - saves geometry (§7.4).

  A view closed while it is in native full screen on macOS is destroyed
  instead of hidden: hiding a full-screen window leaves an empty
  full-screen Space behind. The next open builds it again and restores
  the last windowed geometry (§7.4 saves nothing while full screen).
- **Destroyed immediately** when:
  - the extension is disabled (`set_extension_enabled(false)`) or
    uninstalled (`uninstall_extension`);
  - its grant no longer includes `views` (`remove_extension_grant`, or
    `commit_extension_install` recording a smaller grant);
  - on Windows, its WebView2 process fails (`webview_recovery`'s
    `ProcessFailed` path destroys view windows instead of running its
    recovery, so a crashing view can't loop). On macOS and Linux a dead
    content process leaves a blank stage; the watchdog banner (§8.7)
    offers Reload.

  Rust does this in `ext_views::close_for_extension(app, id)`, called from
  those commands.
- **Reloaded in place** by the view page (the sandbox is torn down and
  booted again; the window stays) when:
  - the extension's version changes (`extensions_changed`);
  - the grant changes but still includes `views`;
  - the UI language changes (`config_updated`);
  - the user presses Reload in the host bar.

  If, after an update, the manifest no longer declares the view, the page
  shows the "can't open" state (§18.3) with a Close button instead of
  vanishing.

The view's in-memory state is lost on reload and teardown. A view must
keep anything worth keeping in extension storage (§10).

### 7.4 Size and position persistence

- Per `(extension, view)`, per machine, in
  `<data_local_dir>/kage/extension-views.json`, resolved through
  `crate::paths::data_local_dir()` (never `dirs::*`, so tests isolate).
- Written with `crate::atomic_write`, owner-only. Debounced: on `Moved` /
  `Resized` (500 ms trailing) and on hide. Nothing is saved while
  minimised or in native full screen.
- Physical pixels plus the monitor it was on:

  ```json
  {
    "version": 1,
    "views": {
      "mindmap/explorer": {
        "x": 312, "y": 140, "width": 2400, "height": 1600,
        "maximized": false,
        "monitor": { "name": "DISPLAY2", "x": 0, "y": 0, "width": 5120, "height": 2880, "scale": 2.0 },
        "saved_at": "2026-11-09T10:00:00Z"
      }
    }
  }
  ```

- At most 64 entries; the oldest `saved_at` goes first. An unreadable file
  is ignored and rewritten on the next save.
- Uninstalling an extension removes its entries. Settings offers "Reset
  window size and position" (`ext_view_reset_geometry`).
- This file is not part of `Config`, not in any backup, and never synced
  (§20, §21). Window geometry is device-local, the same as the chat
  window's geometry, which import already keeps local.

### 7.5 Placement, multiple monitors and DPI

A pure function in `src/ext_views/geometry.rs`, unit-tested with fake
monitor lists:

```rust
pub fn place(
    saved: Option<&SavedGeometry>,
    monitors: &[MonitorInfo],       // name, physical rect, work area, scale
    opener: Option<&OpenerInfo>,    // kind (floating | other), physical rect, monitor
    cursor_monitor: Option<usize>,
    decl: &ViewDecl,                // defaultSize, minSize (logical)
    can_position: bool,             // false on Wayland
) -> Placement                      // physical size, Option<physical position>, maximized
```

1. **Saved and still valid.** A monitor with the same name and the same
   physical rect exists, and the saved rect overlaps its work area by at
   least half its own area: use the saved rect, shrunk to fit the work
   area. Restore `maximized` after showing.
2. **Otherwise** pick the target monitor: the opener's monitor, else the
   cursor's, else the primary.
   - Size: the saved logical size if there is one (converted with the
     target's scale), else `defaultSize`. Clamped to 90% of the work area
     and at least `minSize`.
   - Position: if the opener is the floating launcher and the view fits
     below it (launcher bottom + 16 px + height ≤ work-area bottom), centre
     it horizontally under the launcher, 16 px below. That keeps the
     launcher from covering it (§15). Otherwise centre it in the work
     area.
3. **Wayland** (`can_position == false`): size only. The compositor places
   the window.

DPI:
- Storing physical pixels together with the monitor's rect and scale avoids
  the ambiguity of logical positions across monitors with different
  scales.
- Moving a window between monitors changes `devicePixelRatio` inside the
  view. The page needs no help from the host; the view handles it (§14).

## 8. The view page and its sandbox

### 8.1 Files

- `ui/extension-view.html`: loads `shared-kage-tokens.css`,
  `shared-components.css`, `shared-rtl.css`, `extension-view.css`, and
  `js/extension-view/main.js`. It installs the shared link handler (as
  every window must).
- `ui/js/extension-view/main.js`: boot and event wiring.
- `ui/js/extension-view/chrome.js`: host bar, loading, error and
  not-responding states.
- `ui/js/extension-view/watchdog.js`: ping/pong timer (pure, testable).

The view page is first-party and trusted. Its Tauri permissions come from a
separate capability file, `capabilities/extension-view.json`, with window
pattern `ext-view:*`. That file lists only what the page uses: `core:default`,
`core:event:default`, `core:event:allow-listen`, `core:window:default`,
`core:window:allow-close`, `core:window:allow-hide`,
`core:window:allow-show`, `core:window:allow-set-focus`,
`core:window:allow-set-title`, `core:window:allow-is-visible`,
`core:webview:default`. No global shortcuts, shell, dialog, updater,
notification or deep-link permissions. (App commands from
`generate_handler!` stay callable from every first-party window, as
today; the iframe can't reach any of them.)

### 8.2 Layout

```
┌──────────────────────────────────────────────┐
│ host bar (32 px, Kage tokens)                │  <header class="kv-bar">
├──────────────────────────────────────────────┤
│ status banner (only when needed)             │  <div class="kv-banner">
├──────────────────────────────────────────────┤
│                                              │
│ stage: the view iframe, 100% × rest          │  <main class="kv-stage"><iframe>
│ (loading / error panels sit over it)         │
└──────────────────────────────────────────────┘
```

The iframe is laid out below the host bar, so nothing it draws can cover
the bar. The bar is not part of the iframe's document.

### 8.3 The iframe

Created by `ExtensionSandbox` with a new option `surface: 'view'`
(`ui/js/shared/extension-sandbox-host.js`). Differences from the hidden
surface:

| | Hidden (today) | View |
|---|---|---|
| Size and position | 1 px, off the layout, `opacity:0`, `pointer-events:none` | Fills the stage, visible, takes pointer and keyboard input |
| `aria-hidden` | `true` | not set; `title` = `t('ext_view.iframe_label', {view, extension})` |
| `sandbox` | `allow-scripts` | `allow-scripts` (exactly; tested) |
| `allow` | not set | explicit denials: `camera 'none'; microphone 'none'; geolocation 'none'; clipboard-read 'none'; clipboard-write 'none'; display-capture 'none'; fullscreen 'none'; payment 'none'; usb 'none'; serial 'none'; hid 'none'; bluetooth 'none'; midi 'none'; screen-wake-lock 'none'; web-share 'none'; autoplay 'none'` |
| `referrerpolicy` | not set | `no-referrer` |
| srcdoc head | charset | charset, CSP `<meta>` (§8.4), `<html lang dir>`, theme `<style>` |
| Providers loaded | everything the manifest contributes | only the view module and what it imports |

Missing sandbox tokens are deliberate: no `allow-same-origin` (null origin,
no Tauri), no `allow-popups`, `allow-forms`, `allow-modals` (no
`alert`/`confirm`/`prompt` look-alikes), `allow-top-navigation*`,
`allow-pointer-lock`, `allow-downloads` or `allow-presentation`.

### 8.4 Content Security Policy

The view srcdoc starts with:

```
default-src 'none';
script-src 'unsafe-inline' 'unsafe-eval' blob:;
worker-src blob:; child-src blob:;
style-src 'unsafe-inline' blob:;
img-src blob: data:; font-src blob: data:; media-src blob: data:;
connect-src blob: data:;
frame-src 'none'; object-src 'none';
base-uri 'none'; form-action 'none'
```

- `'unsafe-inline'` runs the inlined runtime. `blob:` loads extension
  modules. `'unsafe-eval'` keeps `runSandboxed` working (its worker
  evaluates the serialised function). None of these widen what the
  extension can reach, because the frame is already untrusted.
- The point is **no network**: no `fetch`, XHR, WebSocket, EventSource,
  beacon or remote `<img>`/CSS/font. A view gets remote data only through
  host commands its grant allows (`fetch_link_metadata`, `fetch_favicon`
  under `network`).
- CSP doesn't cover WebRTC. Before it loads the view module, the view
  runtime deletes `RTCPeerConnection`, `webkitRTCPeerConnection` and
  `RTCDataChannel` from the view document's global object. `frame-src
  'none'` stops the module from getting fresh copies from a new frame,
  and workers have no WebRTC.
- Hidden sandboxes keep their current srcdoc (no CSP). Adding the same
  policy there is a follow-up (§25), because existing extensions may rely
  on direct requests.
- **What this does and doesn't buy.** The same extension's hidden
  instances (floating, chat, settings) share its storage and can still
  reach the network directly. So an extension can still send out what its
  view shows or what the user types into it, by writing it to storage for
  a hidden instance to send. Until hidden sandboxes get the same policy,
  the view's CSP is defence in depth, not a boundary: it stops a view from
  loading remote code, images or fonts, and from exfiltrating on its own.
  §17.2 and SECURITY_MODEL say so.

### 8.5 Boot sequence (view page)

1. `initLinkHandler()`, apply theme (`loadAndApplyTheme`), load the Kage
   catalog, apply static translations and RTL.
2. `ext_view_boot` → `{ extension_id, view_id, params, open_seq, source,
   reloaded }`. Errors show the matching error state.
3. `list_extensions` → find the manifest. `get_config` → the grant
   (requested ∩ granted, the same rule Settings uses) and the effective
   extension config (`resolveExtensionConfig`).
4. Read the view's module (`read_extension_file`), its imports
   (`fetchSharedSourcesViaInvoke`), its CSS files, the extension's locale
   (`fetchExtensionLocaleViaInvoke`) and apply `applyManifestI18n`.
5. Compute theme tokens (§11).
6. Create `ExtensionSandbox({ surface: 'view', ... })` with sources
   `{ view: <module source>, sourcePaths: { view: <path> } }`, the CSS
   text, the theme and the boot params. Start it.
7. RPC `viewMount({ params, source, reloaded })`. Show the loading state
   until it resolves, up to **10 s**; then show the error state with
   Reload.
8. Set the window title (§18.2). Focus the iframe.
9. Start the watchdog (§8.7). Subscribe to `ext_view_reopen`,
   `ext_view_visibility`, `config_updated`, `extensions_changed`,
   `extension_data_changed`, and window focus.

### 8.6 Port protocol additions

Host → sandbox (RPCs, answered with `rpc-response`):

| Method | Params | Calls in the view module |
|---|---|---|
| `viewMount` | `{ params, source, reloaded }` | `mount(root, {params, source, reloaded})` |
| `viewReopen` | `{ params, source }` | `onOpen({params, source})` |
| `viewVisibility` | `{ visible }` | `onShow()` / `onHide()` |
| `viewTheme` | `{ theme }` | updates the theme `<style>`, then `onThemeChange(theme)` |
| `viewPing` | `{}` | nothing (the runtime answers) |

Host → sandbox (fire-and-forget message, **every surface**, not only
views):

| Type | Payload |
|---|---|
| `storage-changed` | `{ changes: [{ key, op, self, source }] }` (§10) |

Sandbox → host (new message type `view-request`, view surface only;
ignored elsewhere):

| `kind` | Fields | Host checks, then does |
|---|---|---|
| `set-title` | `text` (string or null) | ≤ 80 chars after removing control/bidi characters; ≤ 10 per second. Sets the subtitle in the bar and the OS title. |
| `close` | none | Closes (hides) the window. |
| `open-view` | `view`, `params` | §6.2 checks; recent user activation (§13.4); ≤ 1 per second. `ext_view_open` with `source: 'view'`. |
| `compose` | `text` | `ext_view_compose` (§16). |
| `copy-text` | `text` | `ext_view_copy_text` (§16). |
| `open-settings` | none | `open_settings_window({ section: 'ext-<id>' })`. The id is the host's, never the sandbox's. |

Every field is type-checked. Unknown kinds are ignored with a warning.

**View-surface denylist.** `decideInvoke(command, held, { surface: 'view' })`
also refuses these commands in a view window, because they act on the
calling window and assume it is the launcher or a chat window:

`send_message_streaming`, `cancel_generation`, `send_steering_message`,
`extension_tool_response`, `get_window_session`, `resize_floating_window`,
`set_floating_opacity`, `apply_chat_window_size`, `save_window_position`,
`save_chat_window_geometry`, `notify_frontend_ready`.

(For example, a prompt sent from a view window would route its tool
permission requests to a window with no permission UI, and the agent would
wait forever.) The reason text says so. `open_chat_with_message` (opens the
main chat) stays allowed under `agent`.

The denylist also holds EXTENSION_AGENT_CALLS' five commands
(`agent_call_reserve`, `agent_call_ready`, `agent_call_send`,
`agent_call_cancel`, `agent_call_status`). A view is torn down 10 minutes
after it is hidden, so background agent work belongs to the extension's
long-lived instances; a view that wants it asks them through storage.
V1 adds these entries; the names are fixed by EXTENSION_AGENT_CALLS §3.5,
and listing a command that doesn't exist yet is harmless.

### 8.7 Watchdog

- While the window is visible, the page sends `viewPing` every 5 s.
- After 3 missed answers (15 s), the banner says "Explorer isn't
  responding." with Reload and Close.
- An answer clears the banner.
- If the sandbox shares the page's main thread and freezes it, the page
  can't show the banner. The native close button still works (Rust handles
  `CloseRequested`), and the next open of a window whose last ping failed
  reloads the sandbox.
- The watchdog never kills the view on its own.

### 8.8 Navigation guard for the iframe

A sandboxed iframe can still navigate itself (`location.href = …`). On
macOS and Linux the Rust navigation guard sees and cancels iframe
navigations away from the app origin. On Windows it sees only main-frame
navigations (`docs/SECURITY_MODEL.md`, "Links and navigation"). Without a
check, a view could show a real web page inside a Kage window. So:
- The view page listens for the iframe element's `load` event. The first
  one is the srcdoc. **Any later `load` means the frame navigated.** The
  page then destroys the sandbox at once and shows "Explorer tried to show
  a web page, so Kage stopped it." with Reload.
- The runtime also cancels clicks on `<a href>` inside the view (capture
  phase on `window`). With the `urls` capability it routes `http(s)` and
  `mailto` links through `invoke('open_url')`. Without `urls`, links do
  nothing and a warning is logged. Unlike host-rendered extension HTML,
  the extension controls this document and can fake clicks, so views get
  no free link-opening.

## 9. Runtime API in the sandbox

### 9.1 View module contract

`contributes.views[].module` default-exports a class:

```js
export default class ExplorerView {
    /** Same context as every provider, plus context.view (below). */
    initialize(context) {}

    /**
     * Render into `root` (a full-size element the runtime created in this
     * document). Resolve when the first frame is drawn. Rejecting or
     * taking more than 10 s shows the host's error state.
     * @param {HTMLElement} root
     * @param {{ params: object|null, source: string, reloaded: boolean }} info
     */
    async mount(root, info) {}

    /** Opened again while loaded (new params, e.g. a new focus). */
    onOpen({ params, source }) {}

    /** The window was shown / hidden. Stop animation loops on hide. */
    onShow() {}
    onHide() {}

    /** Kage's theme changed (also available as context.view.theme). */
    onThemeChange(theme) {}

    /** The user saved this extension's settings. */
    onConfigUpdate(config) {}

    destroy() {}
}
```

`source` is one of `launcher`, `chat`, `formatter`, `widget`, `toolbar`,
`settings`, `view`. It's informational.

### 9.2 `context.view` (view surface only)

```js
context.view = {
    id: 'explorer',
    theme,                      // frozen snapshot, replaced on change (§11)
    setTitle(text | null),      // subtitle in the host bar and OS title
    close(),                    // hide this window
    open(viewId, params?),      // another view of this extension; needs a recent click or key press
    compose(text),              // put text in the launcher's input and show the launcher; never sends
    copyText(text),             // copy plain text; needs a recent click or key press
    openSettings(),             // this extension's settings page
};
```

All methods return a Promise that rejects with an `Error` carrying
`kind`/`key` when the host refuses (same shape as `invoke()` errors). In
other surfaces `context.view` is `undefined`.

### 9.3 `context.storage.onChanged` (every surface)

```js
const off = context.storage.onChanged((changes) => {
    // changes: [{ key: 'mm-meta', op: 'save' | 'delete' | 'bulk', self: false, source: 'window' }]
});
off(); // unsubscribe
```

Details in §10.

### 9.4 Feature detection

| Feature | In `context.hostFeatures` when | Meaning |
|---|---|---|
| `views` | the window can open views **and** the extension holds `views` | `open_view` actions and effects work here; in a view, `context.view` exists. |
| `storage.change_events` | always (on hosts with this design) | `context.storage.onChanged` delivers changes. |

`views` is added to `HOST_FEATURE_CAPABILITY` (`views` → `views`) and
offered by the floating, chat, settings and view windows.
`storage.change_events` joins `HOST_FEATURES` in
`ui/js/extension-sandbox/runtime/host-api.js`.

The view page offers the host features the four designs add to the
other windows, except `agent.background`, so a module behaves the same
in a view:

| Feature (design) | Floating | Chat | Settings | View |
|---|---|---|---|---|
| `views` (this design) | yes | yes | yes | yes |
| `storage.change_events` (this design) | yes | yes | yes | yes |
| `files.pick_text` (EXTENSION_FILE_TEXT §3.3) | yes | yes | yes | yes |
| `sync.keys`, `sync.lists`, `sync.records` (SYNC §12.3) | yes | yes | yes | yes |
| `agent.background` (EXTENSION_AGENT_CALLS §3.3) | yes | yes | yes | **no** (§8.6) |
| `date-picker` (existing, floating only today) | yes | no | no | no |

Each design's package that adds a feature also adds it to the view
page's offered list (`ui/js/extension-view/main.js`), after this
design's V6 has merged.

An extension can tell "old host" from "not approved yet": a host with
`storage.change_events` but without `views` supports views, and the user
hasn't granted them.

## 10. Talking to the extension's other instances

The owner's decision: shared storage plus change events. There is no
direct message bus between instances (§26).

### 10.1 Change events

- `save_extension_data` and `delete_extension_data` gain injected
  `app: AppHandle` and `window: WebviewWindow` parameters (invisible to
  callers). After the write is on disk (the rename has happened) they
  record `{ extension_id, key, op, window: <caller label> }` in a
  coalescer.
- The coalescer (`src/extension_data_events.rs`) batches per extension for
  **100 ms**. A later change to the same key replaces the earlier one. A
  batch over **64** keys becomes one `{ key: '*', op: 'bulk' }`. It emits
  `extension_data_changed` with `event_targets::emit_to_extension_hosts`
  (new helper: `floating`, `main`, `chat-*`, `settings`, `ext-view:*`):

  ```json
  { "extension_id": "mindmap",
    "changes": [ { "key": "mm-meta", "op": "save", "source": "window", "window": "floating" } ] }
  ```

- Other sources use the same path:
  - a committed backup import (`import_config_bundle`): one `bulk` entry
    per extension whose data it wrote, `source: "import"`;
  - the planned folder sync: `source: "sync"` (§21);
  - uninstall: `bulk`, `source: "uninstall"`, though the instances are
    torn down anyway.
- Each window's host code (`ui/js/shared/extension-data-events.js`,
  installed by the extension manager, the settings window and the view
  page) forwards a batch only to the sandbox whose extension id matches,
  and only if it holds `storage`. It adds
  `self = (change.window === myLabel)` and drops the label before
  forwarding.
- Events carry key names only, never data. An instance re-reads what it
  needs.

### 10.2 Guarantees and non-guarantees

- Delivered after the data is readable.
- Per key, the last change in a batch wins. Order across keys within a
  batch follows write order.
- **Hints, not a log.** A window that was reloading or not yet booted
  misses events. Instances must still validate with their own revision
  markers (Mind Map: `mm-meta.rev`) when they start or come back from
  hidden.
- `self: true` means this window's sandbox wrote it. One sandbox per
  extension per window makes that exact.
- No events for other extensions' keys, and no event for config
  (`save_extension_config` already broadcasts `config_updated`).

### 10.3 Patterns

| Need | Use |
|---|---|
| The launcher captured something; the open view should show it | Launcher writes data; the view gets `onChanged` and re-reads. |
| The launcher wants the view to focus on X | `open_view` with params; the view gets `onOpen`. No disk round trip. |
| The view wants the user to ask the agent | `context.view.compose(text)`. The user sees it and presses Enter. |
| Two instances must not both do a one-shot job | The existing lease pattern in extension data (EXTENSIONS.md, "multiple instances"). |
| Per-computer view state (camera, zoom) | A key of its own that the manifest's `sync` block doesn't list (§21). |

## 11. Theming

The view page computes a token snapshot from its own document after
applying the theme (`getComputedStyle(document.documentElement)`):

```js
theme = {
    scheme: 'dark' | 'light',
    fontSize: 14,              // config.ui.font_size
    reducedMotion: false,      // prefers-reduced-motion in the view page
    tokens: {
        'kage-bg': '#1e1e1e', 'kage-bg-elevated': '…', 'kage-bg-input': '…',
        'kage-text': '…', 'kage-text-bright': '…', 'kage-text-muted': '…',
        'kage-border': '…', 'kage-border-subtle': '…',
        'kage-accent': '…', 'kage-accent-hover': '…', 'kage-accent-active': '…',
        'kage-accent-light': '…', 'kage-accent-glow': '…',
        'kage-link': '…', 'kage-success': '…', 'kage-warning': '…',
        'kage-danger': '…', 'kage-error': '…',
        'kage-font': '…', 'kage-font-mono': '…',
    },
}
```

- Only the tokens listed here, by name. Each value is checked by
  `isSafeTokenValue`: at most 200 characters, and for colours one of hex,
  `rgb[a]()`, `hsl[a]()` or a named colour; for fonts only letters,
  digits, spaces, commas, quotes and hyphens. Anything else (a theme
  extension's `url(…)`, a `;` or `}`) drops that token. Theme extensions
  are untrusted too.
- The runtime writes the tokens into the view document as
  `:root { --kage-bg: …; …; color-scheme: dark; font-size: 14px }`, so
  extension CSS can use `var(--kage-bg)`. Canvas code reads
  `context.view.theme.tokens`.
- Changes (Settings → Appearance, OS dark mode under "system", a theme
  extension) reach the page through `config_updated` and the theme
  module's OS listener. The page recomputes and sends `viewTheme` only
  when the snapshot changed.
- `prefers-color-scheme` inside the iframe follows the OS, not Kage. Views
  should use `theme.scheme`.

## 12. i18n and RTL

- The view sandbox gets the same i18n payload as other sandboxes:
  `context.i18n.t(key, vars)`, `language()`, `isRtl()`, the matched
  `_locales/<lang>/messages.json` plus the EN fallback.
- View titles may be `__MSG_key__` tokens, resolved with the manifest
  (`applyManifestI18n`).
- The runtime sets `<html lang="…" dir="ltr|rtl">` in the view document.
  The view page sets its own `dir` through the shared i18n code, so the
  host bar flips (`shared-rtl.css` plus `extension-view.css` rules).
- A language change reloads the view in place (§7.3), so every string
  comes from the new catalog. Views must restore their state from storage
  on `mount`.
- Canvas text: set `ctx.direction = context.i18n.isRtl() ? 'rtl' : 'ltr'`
  and mirror layout where it carries meaning. Keep inherently left-to-right
  content (URLs, ids, hotkeys) `dir="ltr"` in DOM parts.
- Logs stay English (`context.log.*`), as everywhere.
- Host strings (bar, states, prompts, errors) use Kage's catalog. Keys are
  in §16.4.

## 13. Input: pointer, wheel, keyboard, clipboard, drops

### 13.1 Pointer and wheel

- Native DOM events inside the iframe: `pointerdown/move/up`,
  `setPointerCapture`, `wheel`, `dblclick`, `contextmenu`. No host
  translation.
- Register `wheel` with `{ passive: false }` on the drawing surface only,
  and call `preventDefault()` there, so the page never scrolls or zooms.
- Pinch to zoom differs by engine:
  - WebView2 (Windows) reports a trackpad pinch as `wheel` with
    `ctrlKey: true`;
  - WebKit (macOS, Linux) may send non-standard `gesturestart` /
    `gesturechange` / `gestureend` events with `scale`, instead of or as
    well as ctrl + wheel.

  Handle both, and ignore one kind while the other is active in the same
  gesture. A view's `preventDefault()` on these events stops browser zoom
  inside the iframe. The view page blocks zoom on its own document (a
  `user-scalable=no` viewport and a ctrl + wheel guard), so a pinch over
  the host bar doesn't zoom the page either.
- Use pointer events for dragging. HTML5 drag-and-drop isn't available in
  views (§13.5).
- Right-click: draw your own menu in the view. The host's context-menu
  window is not available to views.

### 13.2 Keyboard and focus

- When the window gains focus, the page focuses the iframe. The runtime
  then focuses its document so keys reach the view without a click.
- Keys typed in the view go to the view. Kage's global hotkeys still work
  (they are OS-level), so the launcher hotkey shows the launcher over the
  view.
- Reserved: **⌘W / Ctrl+W** close the window, like Settings and chat. The
  runtime catches it in a capture listener and sends `view-request
  { kind: 'close' }`. This is a convenience, not a security control: the
  native close button, Alt+F4 and the host bar's Close always work and
  don't depend on extension-realm code.
- Esc is not reserved. Views use it as they like.
- Tab can leave the iframe to the host bar's buttons, as in any page.
- macOS ⌘C/⌘V/⌘X/⌘A/⌘Z come from the app's Edit menu, so they work in
  text the view renders and in its inputs.

### 13.3 Clipboard

- The iframe has no clipboard permission (`clipboard-read 'none';
  clipboard-write 'none'`).
- `context.view.copyText(text)` → `ext_view_copy_text`. The text must pass
  the settings copy rules (`copyTextError` in
  `ui/js/shared/settings-schema.js`, given a `max` option) with a limit of
  **8192** characters: plain text, no control or bidi-override
  characters. It also needs a recent user activation (§13.4) and the view
  window must be focused. At most 2 per second.
- Reading the clipboard still needs the `clipboard` capability
  (`read_clipboard`).

### 13.4 "Recent user activation"

The view page accepts `open-view`, `compose` and `copy-text` only when:
- the view window is focused (checked in Rust for `compose` and
  `copy-text`); and
- `navigator.userActivation.isActive` is true in the view page, where the
  engine supports it. Activation in a child frame propagates to its
  ancestors. Where the API is missing, focus plus the rate limit is the
  rule.

None of these actions can do harm without the user seeing it (a window
opens, text appears in the launcher, the clipboard changes), so focus and
rate limits are enough.

The same rule stands in for the host-seen click that
EXTENSION_FILE_TEXT §5.2 needs before it opens a file dialog. A view
draws its own buttons, so the host never sends it a click RPC. When
`pick_file_text` arrives from the view sandbox, the view page grants the
click lease if the window is focused and `navigator.userActivation.isActive`
is true. A script in the iframe can't produce user activation, so it
can't open a dialog on its own. Where the API is missing, the view gets
EXTENSION_FILE_TEXT's ask bar instead (one extra click), not a weaker
check.

### 13.5 Files dropped on a view

- Tauri's native drag-and-drop handler stays on for view windows. It
  intercepts OS file drops before the page sees them, so dropped files
  never reach the iframe as `File` objects.
- The view page ignores the resulting `tauri://drag-drop` events: paths
  never reach the extension.
- The manual test matrix checks this on every platform. If a platform
  turns out to pass file drops through to the page, the fallback is a
  transparent host-owned layer over the stage, shown on
  `tauri://drag-enter` and hidden on leave or drop, so the drop lands on
  the host document and is discarded.
- Reading a file's contents remains the job of the file-reading host API
  (separate design: the user picks a file each time, gated by a
  capability). A future "drop to capture" would route through that API,
  not through HTML5 drops.

## 14. Performance

The host's part:
- Each view has its own window and WebView, so a heavy view doesn't share
  the launcher's sandboxes' event loop (the cluster note in EXTENSIONS.md
  is about iframes in one page). WebView2 and WKWebView normally give each
  top-level WebView its own renderer process; the test plan checks that
  the launcher stays responsive under a busy view.
- Teardown after 10 minutes hidden returns the memory.
- `onHide` lets the view stop work while hidden. The page doesn't rely on
  `document.visibilityState`, which not every engine updates when a native
  window hides.
- The invoke rate limit (100/s per sandbox) applies to the view instance
  as to any other.

Guidance for view authors (also in EXTENSIONS.md):
- **Size the canvas to the viewport, not the world.** Backing store =
  CSS size × `devicePixelRatio`, at most 8192 px per side and 32
  megapixels. Pan and zoom with a transform. Never allocate a canvas for
  the whole map.
- **Redraw on demand.** Keep a dirty flag and request one
  `requestAnimationFrame` when something changed. No continuous loop when
  idle; none at all after `onHide`.
- **Cull and index.** Keep a spatial index (uniform grid or quadtree) for
  visibility and hit tests. Draw only what intersects the viewport.
- **Level of detail.** Below a zoom threshold draw dots or plain shapes
  without text. Cache `measureText` results. Avoid `shadowBlur`, `filter`
  and `getImageData` in hot paths.
- **Heavy compute off the main thread.** Use `context.runSandboxed`
  (terminable worker, hard timeout) for layout passes. Cap a pass at a few
  hundred milliseconds and keep the last good result on timeout.
- **DPI changes.** On `resize` and on a change of
  `matchMedia('(resolution: <dpr>dppx)')`, re-read `devicePixelRatio` and
  resize the backing store.
- `OffscreenCanvas` in a worker is optional: feature-detect it (older
  WebKitGTK may lack it).
- Data loads cost IPC: load once in `mount`, then update from change
  events.

## 15. Focus and blur, and the floating launcher

### 15.1 macOS: a normal window, not a panel

The launcher, inline assist and context menu are non-activating `NSPanel`s
(`src/overlay_windows.rs`), so they appear over other apps' full-screen
Spaces without activating Kage. A view is a **normal `NSWindow`**:
- it shows in ⌘Tab and the Dock (Kage switches to the Regular activation
  policy while one is visible), can be minimised, moved between Spaces and
  put in native full screen;
- it can be destroyed safely (panels can't, because of the class swap);
- a large work surface shouldn't float above every app.

Trade-off: opening a view from the launcher while it sits over another
app's full-screen Space activates Kage and moves the user to a desktop
Space, as for Settings. The launcher joins all Spaces, so it follows.

Activation: `set_focus` activates the app. On recent macOS versions with
cooperative activation this is allowed because the user just acted in a
Kage window (the key panel or a Kage window).

Activation policy: `setup::update_activation_policy` counts a window as
"real" through a new predicate `is_real_window_label(label)`: today's list
(main, settings, store, welcome) plus `is_ext_view_label`.

Closing: `setup::handle_window_close` currently hides the window and then
hides the whole app (`hide_macos_app`). For a view window, Rust hides the
app only if no other real window is still visible, so closing a view
leaves the user in their open chat or settings window. Other windows keep
today's behaviour.

### 15.2 The launcher stays when a view opens

Today the launcher hides on any blur (`lifecycle-visibility.js`). A view
opening takes focus, so the launcher would vanish together with its input
and response. Two rules prevent that:

**Rule A, the moment of opening.** When the launcher opens a view, it sets
`this._suppressBlurHideUntil = Date.now() + 1500` (the result menu uses
the same mechanism) and `this._viewCompanion = true` before calling
`ext_view_open`. Building a new WebView window can take a few hundred
milliseconds; the suppression covers it. The launcher clears its input as
after any executed result, but keeps its response area.

**Rule B, moving focus between the launcher and views.** In the blur
handler, after the existing guards and before hiding:

```js
const { view_focused } = await invoke('ext_view_focus_state').catch(() => ({ view_focused: false }));
if (view_focused) { this._viewCompanion = true; return; }   // focus went to a view: stay
```

`ext_view_focus_state` answers at once with `false` when no view window is
visible, so the common blur isn't slowed down. When one is visible it
waits up to 120 ms for a view to report focus (the order of the two focus
events isn't guaranteed).

The launcher then still has to hide when the user leaves Kage from the
view. Rust handles a view's `Focused(false)`: after 150 ms, if neither the
launcher nor any view window is focused, it emits `ext_view_focus_lost` to
the launcher. The launcher runs its normal hide path if
`_viewCompanion` is set and it isn't focused. `_viewCompanion` is cleared
whenever the launcher hides or shows.

The decision is a pure function, unit-tested in Rust and JS:

```
should_release_launcher(focused_labels) = !focused_labels.any(l => l == "floating" || is_ext_view_label(l))
```

Resulting behaviour:

| User does | Launcher |
|---|---|
| Opens a view from the launcher | stays, above the view (it's a floating panel / always-on-top) |
| Clicks the view, then the launcher, then the view | stays |
| Clicks another app (from the launcher or from the view) | hides |
| Clicks the chat or settings window | hides (as today) |
| Presses Esc, or runs a hide action | hides (as today) |

Placement (§7.5) puts a new view below the launcher when it fits, so the
launcher doesn't cover it.

### 15.3 Windows

- The launcher is an always-on-top window. A normal view window opened
  from it gets the foreground because Kage owns the foreground at that
  moment. Rules A and B work unchanged (`WM_KILLFOCUS` → `tauri://blur`).
- The view window isn't owned by the launcher, so it doesn't minimise with
  it and isn't forced above or below it.
- Alt+F4 closes (hides) the view.

### 15.4 Linux

- Same rules. On Wayland, focus-stealing prevention may show the new
  window without focus (with an attention hint). Then the launcher doesn't
  blur and simply stays.

## 16. Tauri commands, events and i18n keys

Every command returns `Result<T, AppError>` (enforced by
`tests/command_error_type_parity_test.rs`). Every command is mapped to
`null` in `COMMAND_CAPABILITIES`: extensions never call them, only
first-party host code does (`tests/command_permissions_parity_test.rs`
forces the entry). Commands live in `src/commands/extensions/views.rs`,
are registered in `src/commands/registry.rs`, and call into
`src/ext_views.rs`.

### 16.1 Commands

```rust
#[tauri::command]
pub async fn ext_view_open<R: Runtime>(
    extension_id: String, view_id: String,
    params: Option<serde_json::Value>, source: Option<String>,
    window: WebviewWindow<R>, app: AppHandle<R>,
    ui: State<'_, UiState>, features: State<'_, FeatureServices>,
) -> Result<ExtViewOpened, AppError>;      // { label: String, reused: bool }

#[tauri::command]
pub async fn ext_view_boot<R: Runtime>(window: WebviewWindow<R>, ui: State<'_, UiState>,
    features: State<'_, FeatureServices>) -> Result<ExtViewBoot, AppError>;
    // { extension_id, view_id, params: Option<Value>, open_seq: u64, source: String, reloaded: bool }

#[tauri::command]
pub async fn ext_view_compose<R: Runtime>(text: String, window: WebviewWindow<R>,
    app: AppHandle<R>, ui: State<'_, UiState>) -> Result<(), AppError>;

#[tauri::command]
pub async fn ext_view_copy_text<R: Runtime>(text: String, window: WebviewWindow<R>,
    ui: State<'_, UiState>) -> Result<(), AppError>;

#[tauri::command]
pub async fn ext_view_focus_state<R: Runtime>(app: AppHandle<R>)
    -> Result<ExtViewFocusState, AppError>;  // { view_focused: bool }

#[tauri::command]
pub async fn ext_view_list<R: Runtime>(app: AppHandle<R>, ui: State<'_, UiState>)
    -> Result<Vec<ExtViewInfo>, AppError>;   // [{ extension_id, view_id, visible, focused }]

#[tauri::command]
pub async fn ext_view_reset_geometry(extension_id: String, view_id: Option<String>)
    -> Result<u32, AppError>;                // entries removed
```

| Command | Caller | Errors (`kind` / `key` / params) |
|---|---|---|
| `ext_view_open` | floating, main, `chat-*`, settings, `ext-view:*` pages | `not_permitted` `errors.ext_view.opener_refused` · `invalid_input` `errors.ext_view.unknown_extension` `{id}` · `not_permitted` `errors.ext_view.disabled` `{extension}` · `not_permitted` `errors.ext_view.not_granted` `{extension}` · `invalid_input` `errors.ext_view.unknown_view` `{extension, view}` · `not_permitted` `errors.ext_view.wrong_extension` · `invalid_input` `errors.ext_view.params_invalid` `{reason}` · `invalid_input` `errors.ext_view.params_too_large` `{size, limit}` · `rate_limited` `errors.ext_view.too_many_open` `{limit}` · `internal` `errors.window.create_failed` `{reason}` (existing) |
| `ext_view_boot` | view page | `not_permitted` `errors.ext_view.not_a_view_window` · and, re-checked on every boot, `unknown_extension`, `disabled`, `not_granted`, `unknown_view` |
| `ext_view_compose` | view page | `not_a_view_window` · `not_permitted` `errors.ext_view.not_focused` · `invalid_input` `errors.ext_view.text_invalid` `{reason}` · `rate_limited` `errors.ext_view.too_fast` |
| `ext_view_copy_text` | view page | the same, plus `internal` `errors.ext_view.clipboard_failed` |
| `ext_view_focus_state` | floating | none (always `Ok`) |
| `ext_view_list` | settings | none |
| `ext_view_reset_geometry` | settings | `internal` `errors.ext_view.geometry_write_failed` `{reason}` |

`not_permitted` and `invalid_input` are the two `ErrorKind`s
EXTENSION_AGENT_CALLS' WP-AC0 adds. AC0 lands before this design's V1
(SYNC §23), and EXTENSION_FILE_TEXT uses the same kinds for the same
cases (a missing grant, a turned-off extension, bad arguments), so
callers branch the same way on every design's errors.

Notes:
- `ext_view_open` records telemetry `extension_view_opened
  { source, reused }` (strings and 0/1 only).
- `ext_view_compose` accepts at most 2000 characters, plain text (same
  character rules as copy). It shows the launcher (at its usual place) and
  emits `floating_compose { text }` to it. The launcher replaces its input,
  selects it and does **not** send.
- `ext_view_open`'s `source` is checked against the closed list in §9.1;
  anything else becomes `launcher`.

### 16.2 Rust state

`UiState` gains `ext_views: ext_views::Registry` (a `Mutex` inside), so no
new managed type is needed (`tests/state_manage_parity_test.rs` stays as
it is). The registry holds, per label:
- `extension_id`, `view_id`;
- `params`, `open_seq`, `source`, `booted`;
- `hidden_gen` (teardown timer token), `last_hidden_at`;
- pending geometry;
- rate-limit timestamps for compose and copy.

### 16.3 Events

Added to `src/events.rs` and `ui/js/shared/events.js` (the parity test
checks both).

| Event | Target | Payload |
|---|---|---|
| `ext_view_reopen` | that view window | `{ params, open_seq, source }` |
| `ext_view_visibility` | that view window | `{ visible }` |
| `ext_view_focus_lost` | floating | `null` |
| `floating_compose` | floating | `{ text }` |
| `extension_data_changed` | extension hosts (floating, main, `chat-*`, settings, `ext-view:*`) | `{ extension_id, changes: [{ key, op, source, window? }] }` |

`event_targets.rs` gains `emit_to_extension_hosts` and
`emit_to_ext_view(label)`. No new event uses a plain `emit`.

### 16.4 i18n keys (EN, `locales/en/messages.json`)

Backend errors:

| Key | EN |
|---|---|
| `errors.ext_view.opener_refused` | This window can't open extension windows. |
| `errors.ext_view.unknown_extension` | No extension called {id} is installed. |
| `errors.ext_view.disabled` | {extension} is turned off. |
| `errors.ext_view.not_granted` | {extension} needs your approval to open its own windows. |
| `errors.ext_view.unknown_view` | {extension} has no window called {view}. |
| `errors.ext_view.wrong_extension` | An extension window can only open windows of the same extension. |
| `errors.ext_view.params_invalid` | The window's start data isn't valid: {reason} |
| `errors.ext_view.params_too_large` | The window's start data is too large ({size} bytes; the limit is {limit}). |
| `errors.ext_view.too_many_open` | Too many extension windows are open (limit {limit}). Close one and try again. |
| `errors.ext_view.not_a_view_window` | This isn't an extension window. |
| `errors.ext_view.not_focused` | The extension window isn't in front. |
| `errors.ext_view.text_invalid` | The text can't be used: {reason} |
| `errors.ext_view.too_fast` | Too many requests. Try again in a moment. |
| `errors.ext_view.clipboard_failed` | Couldn't copy to the clipboard. |
| `errors.ext_view.geometry_write_failed` | Couldn't save window positions: {reason} |

Frontend:

| Key | EN |
|---|---|
| `ext_view.window_title` | {view} — {extension} |
| `ext_view.window_title_sub` | {subtitle} — {view} — {extension} |
| `ext_view.iframe_label` | {view}, drawn by {extension} |
| `ext_view.badge` | Extension |
| `ext_view.badge_tip` | This window is drawn by the {extension} extension. |
| `ext_view.bar.reload` | Reload |
| `ext_view.bar.settings` | Extension settings |
| `ext_view.bar.close` | Close |
| `ext_view.loading` | Opening {view}… |
| `ext_view.state.load_failed` | {view} couldn't open. |
| `ext_view.state.not_responding` | {view} isn't responding. |
| `ext_view.state.navigated` | {view} tried to show a web page, so Kage stopped it. |
| `ext_view.state.removed` | {extension} no longer has this window. |
| `ext_view.state.updated` | Updated to {version}. |
| `settings.manager.cap.views.label` | Its own windows |
| `settings.manager.cap.views.desc` | Show windows the extension draws itself, such as a map or a board. What you type in those windows goes to the extension. Kage labels them with the extension's name. |
| `settings.manager.ext.views.heading` | Windows |
| `settings.manager.ext.views.open` | Open |
| `settings.manager.ext.views.reset` | Reset window size and position |
| `settings.manager.ext.views.reset_done` | Window size and position reset. |

All 30 other catalogs come from `scripts/translate.py`;
`scripts/check_i18n.py` must pass.

## 17. Security analysis

Measured against [`docs/SECURITY_MODEL.md`](../SECURITY_MODEL.md).

### 17.1 Existing guarantees

| Guarantee | With views |
|---|---|
| Extension calls an IPC command it didn't declare | Unchanged. The view page runs the same `ExtensionSandbox._handleInvoke`: `decideInvoke`, `validateExtensionInvokeArgs`, rate limit, identity injection. Plus the view-surface denylist (§8.6). |
| "Never allowed" commands | Unchanged. All new commands are `null`. |
| Reaching `window.__TAURI__` | Unchanged. Null-origin iframe; tested that `sandbox` is exactly `allow-scripts`. |
| Reading files outside its directory | Unchanged. The view page reads sources through `read_extension_file`, path-bounded. |
| New command silently exposed | Unchanged (parity test). |
| Install without approval; silent capability growth | `views` follows the same grant and re-approval rules. Rust re-checks the grant on every `ext_view_open` and `ext_view_boot`. |
| A Kage window turned into a remote page | The iframe can't navigate the top window (no `allow-top-navigation`). Self-navigation is cancelled by the Rust guard on macOS/Linux and detected and torn down on every platform (§8.8). The view page itself has the link handler and the guard like any window. |
| Opening the browser without `urls` | Links in a view need `urls` (§8.8). There is no free user-click path in views, because the extension can fake clicks in its own document. |
| Backups leak or grant | No new config fields. Geometry isn't in backups. Grants are never imported (§20). |

### 17.2 New threats

| Threat | Mitigation |
|---|---|
| **Imitating Kage UI** (a fake permission prompt, a fake "enter your backup passphrase", a fake login) | The host bar is drawn by Kage outside the iframe and always names the extension. The OS title always ends with the extension's name (the extension sets only a subtitle). Kage **never** asks for a passphrase, password or permission inside a view window; permission prompts appear in the launcher, chat or Settings. EXTENSIONS.md and the install prompt say so. Views open only on a user action. Windows can't be frameless, transparent, always-on-top, element-full-screen or smaller than `minSize` ≥ 320 × 240, and the extension can't choose where they appear. |
| **Capturing keystrokes** | Only what the user types into the view window, which is labelled. A view gets no input from other windows. OS-level hotkeys still work. No pointer lock. |
| **Exfiltration over the network** | From the view itself: CSP allows no `connect-src`, `img-src`, `font-src` or `media-src` outside `blob:`/`data:`, no frames, no forms, and the runtime removes WebRTC (§8.4). Network only through granted host commands. **Residual:** the extension's hidden instances share its storage and still have direct network access (§25), so the CSP is defence in depth until hidden sandboxes get it too. |
| **Pop-ups, modals, downloads, devices** | Sandbox tokens not granted; `allow` denies camera, microphone, geolocation, clipboard, display capture, full screen, payment, USB/serial/HID/Bluetooth/MIDI, wake lock, web share and autoplay. |
| **Window spam / DoS** | Opening needs a user action; one window per view; at most 4 view windows, 2 of them hidden; at most 4 views per extension; reopen rate limits inside a view. A busy view can't block the native close. The watchdog offers Reload. Teardown frees memory. A crashing renderer is destroyed, not reloaded in a loop. |
| **Identity spoofing between extensions** | Rust derives identity from the window label it created. `ext_view_boot` takes no id argument. A view window can only open views of its own extension. Storage events are forwarded only to the matching extension's sandbox. |
| **Window-bound commands misused from a view** | View-surface denylist (§8.6). |
| **Clipboard abuse** | Host-written only; plain text; length cap; focus and activation checks; rate limit (§13.3). |
| **Launcher input injection** | `compose` only fills the input. It never sends. Plain text, length cap, focus, rate limit. The user sees it. |
| **Theme values as an injection channel** | Allowlisted token names and validated values (§11). |
| **Params as an injection channel** | JSON only, size and depth capped, never in a URL, delivered over the port. |
| **File paths or contents through drops** | Native handler intercepts drops; the page ignores them (§13.5). |
| **Data events as a side channel** | Key names of the extension's own keys only, to its own sandboxes only. No data in the event. |
| **View geometry revealing monitor setup** | Stored locally only, never synced, backed up or sent. |

### 17.3 SECURITY_MODEL.md changes (WP-V7)

- "Which contribution points are sandboxed": add "View ✅ Yes — a visible
  sandboxed iframe in its own window; CSP blocks the view's own network
  access (the extension's hidden instances still have it, see Known
  gaps); the host bar and title are Kage's".
- "Capabilities": add `views` to the summary list.
- "Links and navigation": add the view rule (links need `urls`;
  self-navigation detection on every platform).
- "Summary of current guarantees": add rows for UI imitation, network from
  views, and window spam.

## 18. UX

### 18.1 Install and approval

- The install prompt lists 🖼️ **Its own windows** with the description in
  §16.4. The welcome screen's capability pills show it too (same table).
- Updating to a version that adds `views` shows the usual "new permission"
  badge in the Store's Approve flow. Until then the extension's
  `open_view` entries are refused, and well-behaved extensions hide them
  (§9.4).

### 18.2 The window

- **OS title:** `t('ext_view.window_title', { view, extension })`, for
  example "Explorer — Mind Map". With a subtitle from `setTitle`:
  "Bingo — Explorer — Mind Map".
- **Host bar** (32 px, Kage tokens, flips in RTL):
  - start side: the view icon, the extension name in bold, "·", the view
    title, the subtitle in muted text;
  - end side: an "Extension" badge (tooltip `ext_view.badge_tip`),
    ⟳ Reload, ⚙ Extension settings, ✕ Close.
  - All buttons are keyboard accessible and labelled.
- The host bar is always shown (§25 asks whether a full-screen mode may
  hide it).

### 18.3 States

| State | Shown by the host |
|---|---|
| Loading | Centred spinner and "Opening Explorer…" until `mount` resolves (≤ 10 s). |
| Can't open | "Explorer couldn't open." with Reload and Extension settings. The detail goes to the log. Same panel for "not approved" (`errors.ext_view.not_granted`, with an Approve button that opens the Store's Approve flow), "turned off", and "no longer has this window" (with Close). |
| Not responding | Banner: "Explorer isn't responding." with Reload and Close. |
| Stopped navigation | "Explorer tried to show a web page, so Kage stopped it." with Reload. |
| Updated in place | "Updated to 1.2.0." in the bar for 3 s. |

### 18.4 Settings

- **Settings → the extension's page:** the capability badges show 🖼️. A
  host-rendered **Windows** block lists each declared view with **Open**,
  and one **Reset window size and position** button. It lives in its own
  module, `ui/js/settings/extension-views-block.js`, called with one line
  from `manager.js`. The page header's host blocks come in a fixed order
  shared by the four designs: capability badges, the sync chip (SYNC
  §12.3), the Background AI panel (EXTENSION_AGENT_CALLS §4.2), this
  Windows block, then the extension's own settings.
- No global setting. The grant is the switch: an extension without
  `views` has no windows.

### 18.5 Indicators

- A visible view appears in the Dock or taskbar and in ⌘Tab / Alt+Tab as a
  Kage window with its title.
- No tray item, no badge on the launcher.

### 18.6 Telemetry

- `extension_view_opened { source, reused }`. Add it to
  `docs/PRIVACY.md` and to `KNOWN_EVENTS` in `ui/js/shared/telemetry.js`.
  No extension id, no params, no titles.

## 19. Cross-platform notes

| Topic | macOS | Windows | Linux |
|---|---|---|---|
| Window type | normal `NSWindow` (§15.1) | normal top-level window | normal toplevel |
| Activation | Regular policy while a view is visible | n/a | n/a |
| Over a full-screen app | moves to a desktop Space | n/a | depends on the WM |
| Close | hides; hides the app only if no other real window is visible | hides | hides |
| Iframe self-navigation | cancelled by the Rust guard, and detected (§8.8) | **not** seen by the guard; detected and torn down (§8.8) | cancelled by the guard, and detected |
| Pinch | `gesture*` events | `wheel` + `ctrlKey` | `gesture*` events (WebKitGTK) |
| Positioning | restored | restored | restored on X11; size only on Wayland |
| DPI | per-monitor scale, physical storage (§7.5) | per-monitor DPI v2, physical storage | as reported by GTK |
| Copy shortcuts | Edit menu key equivalents | WebView2 native | WebKitGTK native |
| File drops | native handler intercepts | native handler intercepts | native handler intercepts |
| `navigator.userActivation` | WebKit supports it on current macOS; else focus + rate limit | supported | depends on the WebKitGTK version; else focus + rate limit |

Also:
- The updater counts a visible view as "the user is working"
  (`updater::state::is_user_facing_label` includes `ext-view:*`), so a
  silent install doesn't restart Kage under an open view.
- Windows Job Object and process cleanup are unchanged. Views are
  webviews, not processes Kage spawns.

## 20. Backups

Interaction with `src/config_export.rs` and `src/config_export/*`:
- **No new `Config` fields.** The allowlist
  (`every_config_field_has_a_decision`) and `config_migrations` are
  untouched. Any later field (say, a configurable teardown time) needs
  `#[serde(default)]` and an allowlist decision (a number in range would
  be copied).
- **Grants.** `views` is stored in `extension_grants` like any capability.
  A settings-only backup may copy the grant record (as today), but no
  import ever takes grants (`consent.rs`). An extension that a backup
  switches on, with `views` not granted here, stays off until approved.
- **Geometry** (`extension-views.json`) lives outside the backed-up set
  (config, steering docs, `extension-data/`), so it's in no backup. A test
  asserts that a full encrypted bundle never contains it.
- **Data written by views** is ordinary extension data: in the encrypted
  backup, never in the settings-only one. `sensitiveData` and the safety
  net apply unchanged.
- **Import while a view is open.** The import journal already keeps a
  file that changed after it was staged (a view wrote it again). After a
  committed import, `import_config_bundle` emits `extension_data_changed`
  `bulk` with `source: "import"` per extension whose data it wrote, so
  open views (and every other instance) re-read.

## 21. Folder sync

Folder sync is specified in [`SYNC.md`](SYNC.md) (a user-chosen folder,
everything in it encrypted; settings, shortcuts, extensions and their
data by default, each extension opting in; per-OS and per-computer
overrides; newest change wins per field, with a conflicts list in
Settings → Sync). What this design needs from it, and what SYNC
decided:
- **Geometry is never synced.** It's device-local by nature, not even a
  per-OS value. SYNC §4.2 lists `extension-views.json` as never synced
  (on macOS it sits next to `config.json` in the same folder).
- **Grants are never synced**, the same rule as backups (SYNC §11.1). An
  extension that arrives on a computer through sync must go through that
  computer's approval before `views` works.
- **View data follows the extension's opt-in.** What a view writes is
  extension data. It syncs only if the manifest's `sync` block lists it
  (SYNC §12.2): a key in `keys` or `lists`, or records the extension
  pushes into a collection.
- **Per-computer view state.** Camera, zoom and panel state shouldn't
  follow the user to another screen size. An extension keeps them in a
  key its `sync` block doesn't list. Mind Map: `mm-view` isn't listed;
  `mm-layout` pins sync as the `pins` collection, so two computers moving
  different nodes merge (SYNC §12.9, §24.5 here).
- **Remote changes reach open views.** Two paths, depending on the sync
  mode (SYNC §12.4–§12.6):
  - keys and lists modes: sync writes the extension-data file and emits
    `extension_data_changed` with `source: "sync"`, also after a restore
    from the conflicts list;
  - collections mode (Mind Map): sync never writes the extension's files.
    It sends `extension_sync_changes`; whichever instance pulls (the view
    too, since it offers `sync.records`, §9.4) applies the change under
    the extension's own lock and saves, and every other instance, the
    view included, gets an ordinary `extension_data_changed`.

  A view must never assume its in-memory copy is current. It re-reads on
  events and resolves with its own revisions.
- **Conflicts.** "Newest wins per field" happens at the sync layer. A view
  that is open while its data is replaced just sees a change event.

## 22. Test plan

Rule: never run the full `cargo test` locally (the mock-app sweep can
touch real config without isolation). Run the targeted commands below. CI
runs everything.

### 22.1 Rust unit tests

- `extensions::views`: normalisation (valid entry, bad id, duplicate id,
  more than 4 views, bad module path, title fallback and stripping,
  size clamps, `css` limits, a non-array `views` that leaves the manifest
  readable); manifest round trip keeps `views`; `VALID_CAPABILITIES`
  includes `views`.
- `window_labels`: build/parse/match for `ext-view:` labels; rejection of
  ids that would inject a separator; `non_chat_labels_are_not_chat`
  includes a view label.
- `ext_views::geometry`: `place()` for saved-and-valid, monitor gone,
  monitor resized, different scale, 90% clamp, `minSize` floor, below the
  launcher when it fits, centred when not, Wayland. Load/save round trip;
  corrupt file; LRU at 64; removal by extension.
- `ext_views` registry: open → boot → reopen (`open_seq`, params,
  `booted`); reopen before boot keeps the newest params; teardown token
  invalidated by reopen; the cap of 4 (and of 2 hidden) evicts the least
  recently hidden; all visible → `too_many_open`; a close in native full
  screen destroys instead of hiding (`should_destroy_on_close`).
- Validation: `validate_open_request(manifest, grant, enabled, opener,
  view_id, params)` for every error key in §16.1; params depth and size.
- `should_release_launcher` truth table.
- `setup::is_real_window_label`; the macOS close rule as a pure function
  (`should_hide_app_on_close(closing, visible_real_windows)`).
- `updater::state`: view labels gate updates.
- `extension_data_events` coalescer: batching within 100 ms, last op wins
  per key, overflow to `bulk`, per-extension separation, `window` label
  kept.
- `files.rs`: save and delete still atomic and owner-only; a change is
  recorded only after a successful write.

Commands: `cargo test --lib extensions::`, `cargo test --lib
window_labels::`, `cargo test --lib ext_views::`, `cargo test --lib
extension_data_events::`, `cargo test --lib setup::`, `cargo test --lib
updater::`.

### 22.2 Rust integration tests

- `tests/extension_views_test.rs` (new; calls `kage::paths::isolate(<tempdir>)`
  first): with a mock app and a fake installed extension:
  - open with a grant creates `ext-view:<id>:<view>` and returns
    `reused: false`; a second open returns `reused: true` and emits
    `ext_view_reopen`;
  - open without a grant → `errors.ext_view.not_granted`; disabled →
    `disabled`; undeclared view → `unknown_view`; from the `store` label →
    `opener_refused`; from another extension's view → `wrong_extension`;
  - `ext_view_boot` from a non-view window → `not_a_view_window`;
  - disable and uninstall destroy the window; uninstall removes geometry;
  - a full encrypted export doesn't contain `extension-views.json`.
- The existing parity tests pass unchanged in shape:
  `command_error_type_parity_test`, `command_permissions_parity_test`,
  `state_manage_parity_test`, `user_dirs_isolation_test`,
  `config_serde_default_parity_test`.
- `mock_app_test`'s command sweep (CI) calls the new commands with bad
  arguments without panicking.

### 22.3 JS tests (vitest, `ui-tests/`)

- `shared/extension-view-policy.test.js`: view normalisation mirrors Rust
  (shared fixture file); `validateOpenViewEffect`; params rules;
  `sanitizeViewTitle`; `isSafeTokenValue` (rejects `url(`, `;`, `}`,
  `expression(`, over-long values); the CSP string; `buildViewSrcdoc`
  puts the CSP meta first.
- `shared/extension-sandbox-host-view.test.js` (jsdom): view surface
  iframe has `sandbox="allow-scripts"` exactly, the `allow` denials,
  `referrerpolicy`, no `aria-hidden`, a `title`; `view-request` validation
  and rate limits; unknown kinds ignored; a second iframe `load` tears the
  sandbox down and reports `navigated`; `storage-changed` reaches only the
  matching sandbox and only with `storage`.
- `shared/extension-permissions.test.js`: `views` in `CAPABILITIES`;
  every new command is `null`; the view-surface denylist refuses its
  commands only on the view surface.
- `shared/extension-sandbox-host-api.test.js`: `HOST_FEATURES` includes
  `storage.change_events`; `hostFeaturesFor(['views'], caps)` needs
  `views`.
- `shared/extension-sandbox-runtime-assembly.test.js`: the runtime
  assembles with `runtime/view-api.js`.
- Runtime tests: `context.view` exists only on the view surface;
  `mount(root, info)` is called once with params; `onOpen`, `onShow` /
  `onHide`, `onThemeChange` delivery; theme `<style>` update; link clicks
  routed (with `urls`) or dropped (without); ⌘W / Ctrl+W posts `close`;
  `storage.onChanged` subscribe, unsubscribe, `self` flag.
- `shared/result-executor-extension-actions.test.js`: `open_view` refused without the
  capability or for an undeclared view; with both, `ext_view_open` is
  invoked with `source: 'launcher'` / `'chat'`.
- Host effects: `open_view` from `onResultAction`, widget `onAction`,
  toolbar `onClick` and settings `runAction`.
- `floating/blur-view-companion.test.js`: Rule A arms suppression before
  the invoke; Rule B keeps the launcher when `view_focused`;
  `ext_view_focus_lost` hides only with `_viewCompanion` set and the
  launcher unfocused.
- `extension-view/*.test.js`: boot order; error states for each boot
  error; watchdog (3 misses → banner, an answer clears it); reload on
  version, grant and language change; close on "view removed"; title
  composition; the drag-drop events ignored.
- `shared/link-handler.test.js`: `extension-view.html` installs the link
  handler (add it to the list of windows).
- `python scripts/check_i18n.py` passes.

Commands: `cd ui-tests && npx vitest run <files above>`.

### 22.4 Manual matrix (macOS, Windows, Linux X11, Linux Wayland)

1. Open a view from: a launcher result, a formatter card in the launcher,
   a formatter card in chat, a widget, a toolbar button, a settings
   action, the Settings "Open" button. Each opens the same singleton.
2. The launcher stays when the view opens from it; focus between launcher
   and view keeps it; clicking another app hides it; Esc hides it.
3. Reopen with new params while open re-focuses the view's content.
4. Close (✕, ⌘W / Ctrl+W, Alt+F4) hides. Reopen within 10 minutes is
   instant. After 10 minutes it rebuilds.
5. Two monitors with different scales: move the view to the second,
   close, reopen (same place); unplug the second, reopen (centred on the
   active monitor); maximise, close, reopen (maximised).
6. macOS: ⌘Tab shows Kage while a view is visible and not after it
   closes; over a full-screen app; closing a view with chat open leaves
   chat in front.
7. Theme switch, OS dark mode under "system", a theme extension: live
   update. Language switch to Arabic: the view reloads, host bar and view
   mirrored.
8. Extension update while open → reload in place; disable → window gone;
   grant removed → window gone.
9. Hostile view fixture (an extension in the test helpers):
   `location.href = 'https://example.com'` → stopped on every platform;
   `fetch('https://example.com')` → CSP error; `<img src=https://…>` →
   blocked; `alert()` → nothing; `window.open` → nothing; a file dropped →
   nothing reaches the extension; `while(true){}` → native close works,
   reopen recovers; spamming `open-view` → rate-limited; sending
   `send_message_streaming` from the view → refused.
10. Performance: Mind Map explorer at 2,000 visible nodes, pan and zoom at
    ≥ 50 fps on a mid-range laptop; the launcher stays responsive while
    the view runs a layout pass.

## 23. Work packages

WP-V1 lands first. It fixes every contract (names, keys, labels, command
signatures) so the rest can run in parallel on disjoint files. Each later
package codes against this document.

**Building alongside the other designs** (the full plan is SYNC §23):
- The four contracts packages land one at a time, in the order AC0
  (EXTENSION_AGENT_CALLS), FT-C (EXTENSION_FILE_TEXT), V1, C0 (SYNC). V1
  rebases on FT-C and uses AC0's `not_permitted` and `invalid_input`
  error kinds (§16.1).
- Files that packages of another design also list (`src/extensions.rs`,
  `src/commands/registry.rs`, `src/commands/extensions/{mod,files,install,discovery}.rs`,
  `src/state.rs`, `src/main.rs`, `src/setup.rs`, `src/lib.rs`,
  `src/commands/system/config_io.rs`, `extension-permissions.js`,
  `extension-sandbox-host.js`, `extension-manager.js`, the runtime files,
  `settings-host-effects.js`, `settings/manager.js`, the floating and
  chat lifecycle files, `telemetry.js`, `locales/*`, the docs, and Mind
  Map's `manifest.json`, `mind.js`, `view.js`, `search.js`,
  `settings.js`, `_locales`, `README.md`, the Kage-Extensions scripts):
  this design is first in the order EXTENSION_VIEWS, EXTENSION_FILE_TEXT,
  EXTENSION_AGENT_CALLS, SYNC, so its package merges first when both are
  in flight, and the later package rebases and owns only its own lines.

| WP | Title | Depends on | Repo |
|---|---|---|---|
| V1 | Contracts and skeleton | none | Kage |
| V2 | Rust window lifecycle and commands | V1 | Kage |
| V3 | Storage change events | V1 | Kage |
| V4 | Sandbox view surface and runtime API | V1 | Kage |
| V5 | Openers in existing windows, launcher focus rules | V1 (codes against V4's API) | Kage |
| V6 | The view page | V1 (codes against V2, V3, V4) | Kage |
| V7 | Docs | V1; final pass after V2–V6 | Kage |
| V8 | Mind Map explorer | V1; integration after V4–V6 | Kage-Extensions |

### WP-V1 Contracts and skeleton

- **Files (owned):**
  - `src/extensions.rs` (manifest `views` field, `VALID_CAPABILITIES`,
    `mod views;`)
  - `src/extensions/views.rs` (new: `ViewDecl`, `normalize_views`,
    limits, params validation)
  - `src/window_labels.rs`, `ui/js/shared/window-labels.js`
  - `src/events.rs`, `ui/js/shared/events.js`,
    `ui-tests/shared/events-and-windows.test.js` (the `ext-view:` label
    helpers)
  - `src/event_targets.rs` (`emit_to_extension_hosts` and
    `emit_to_ext_view`, complete: both are a few lines, and V2 and V3
    need them in parallel)
  - `src/lib.rs` (`pub mod ext_views; pub mod extension_data_events;`)
  - `src/ext_views.rs` (new, skeleton: types `ExtViewOpened`,
    `ExtViewBoot`, `ExtViewFocusState`, `ExtViewInfo`, `Registry`; stub
    bodies that return errors, never `todo!()`)
  - `src/extension_data_events.rs` (new, skeleton: `Change`, `Batch`
    types)
  - `src/commands/extensions/mod.rs` (`mod views; pub use views::*;`)
  - `src/commands/extensions/views.rs` (new: the seven command signatures
    from §16.1, each returning `errors.ext_view.not_a_view_window` or an
    empty result)
  - `src/commands/registry.rs` (register the seven commands)
  - `ui/js/shared/extension-permissions.js` (`views` capability, `null`
    entries, view-surface denylist and the `surface` option of
    `decideInvoke`)
  - `ui-tests/shared/extension-permissions.test.js`,
    `ui-tests/shared/extension-rust-alignment.test.js`
  - `locales/en/messages.json` and the other catalogs (all keys of §16.4,
    then `scripts/translate.py`)
- **Done when:**
  - `cargo test --lib extensions::` and `cargo test --lib window_labels::`
    pass; `cargo test --test command_permissions_parity_test` and
    `--test command_error_type_parity_test` pass.
  - `cd ui-tests && npx vitest run shared/extension-permissions.test.js
    shared/extension-rust-alignment.test.js shared/events-and-windows.test.js`
    passes.
  - `python scripts/check_i18n.py` passes.
  - `cargo clippy` and `cargo fmt --check` are clean.

### WP-V2 Rust window lifecycle and commands

- **Files (owned):** `src/ext_views.rs` (bodies),
  `src/ext_views/geometry.rs` (new), `src/ext_views/registry.rs` (new),
  `src/commands/extensions/views.rs` (bodies),
  `src/commands/extensions/install.rs` (close on uninstall, drop
  geometry, close on a smaller grant),
  `src/commands/extensions/discovery.rs` (close on disable),
  `src/setup.rs` (`is_real_window_label`, macOS close rule, visibility
  event and teardown timer on close), `src/main.rs` (view `Moved` /
  `Resized` / `Focused(false)` / `Destroyed` handling),
  `src/state.rs` (`UiState.ext_views`), `src/updater/state.rs`,
  `src/webview_recovery.rs` and `src/webview_recovery/process_failures.rs`
  (destroy view windows on a WebView2 process failure),
  `capabilities/extension-view.json` (new),
  `tests/extension_views_test.rs` (new).
- **Done when:**
  - `cargo test --lib ext_views::`, `cargo test --lib setup::`,
    `cargo test --lib updater::` pass.
  - `cargo test --test extension_views_test` passes (isolated).
  - `cargo test --test state_manage_parity_test` passes.
  - Telemetry `extension_view_opened` fires once per open.
  - `cargo clippy` and `cargo fmt --check` are clean.
  - A dev build opens a stub `ext-view:` window that shows the view page.

### WP-V3 Storage change events

- **Files (owned):** `src/extension_data_events.rs` (bodies),
  `src/commands/extensions/files.rs` (inject `app`/`window`, record
  changes after successful writes; keep the pure helpers its unit tests
  call, and keep `extension_id` as the first argument, because
  `extension-rust-alignment.test.js` reads the signature),
  `src/commands/system/config_io.rs` (emit `bulk` after a committed
  import), `ui/js/shared/extension-data-events.js` (new:
  `installExtensionDataEvents({ listen, myLabel, deliver })`, adds `self`,
  filters by extension id), `ui-tests/shared/extension-data-events.test.js`
  (new).
- **Done when:**
  - `cargo test --lib extension_data_events::` and `cargo test --lib
    commands::extensions::` pass.
  - The vitest file passes.
  - A save in one window produces one batch in another within 150 ms
    (manual, dev build).
  - `cargo clippy` and `cargo fmt --check` are clean.

### WP-V4 Sandbox view surface and runtime API

- **Files (owned):** `ui/js/shared/extension-sandbox-host.js`
  (`surface: 'view'`, view RPCs, `view-request` handling hooks,
  `notifyStorageChanged`, `HOST_FEATURE_VIEWS`, navigation detector),
  `ui/js/extension-sandbox/runtime.js` (view provider, `context.view`,
  `context.storage`), `ui/js/extension-sandbox/runtime/host-api.js`
  (`storage.change_events`), `ui/js/extension-sandbox/runtime/view-api.js`
  (new, self-contained), `ui/js/shared/extension-view-policy.js` (new),
  `ui/js/shared/settings-schema.js` (`copyTextError` gains `max`),
  tests: `ui-tests/shared/extension-view-policy.test.js`,
  `ui-tests/shared/extension-sandbox-host-view.test.js`,
  `ui-tests/shared/extension-sandbox-host-api.test.js`,
  `ui-tests/shared/extension-sandbox-runtime-assembly.test.js`,
  `ui-tests/shared/extension-view-runtime.test.js`.
- **Done when:**
  - All listed vitest files pass, plus the existing
    `extension-sandbox-*.test.js`.
  - Hidden-surface behaviour is unchanged (existing tests untouched and
    green).
  - The view-surface srcdoc carries the CSP meta first.

### WP-V5 Openers in existing windows, launcher focus rules

- **Files (owned):** `ui/js/shared/result-executor.js`,
  `ui/js/shared/extension-host-effects.js` (new: `runOpenViewEffect`),
  `ui/js/shared/extension-manager.js` (`extensionViewDenial`, data-event
  wiring, `views` feature), `ui/js/shared/extension-manager/ui.js`
  (`host` effects from `onResultAction` and widget actions),
  `ui/js/shared/settings-host-effects.js`,
  `ui/js/shared/toolbar-host-effects.js`, `ui/js/shared/telemetry.js`,
  `ui/js/floating/app.js`, `ui/js/floating/app/lifecycle-visibility.js`
  (Rules A and B, `ext_view_focus_lost`, `floating_compose`),
  `ui/js/chat/app/lifecycle.js`, `ui/js/settings/main.js` (data events,
  `views` feature for settings sandboxes),
  `ui/js/settings/extension-views-block.js` (new: the Windows block),
  `ui/js/settings/manager.js` (one call into it, in the header order of
  §18.4), tests:
  `ui-tests/shared/result-executor-extension-actions.test.js`,
  `ui-tests/shared/extension-host-effects.test.js` (new),
  `ui-tests/floating/blur-view-companion.test.js` (new),
  `ui-tests/settings/extension-views-block.test.js` (new).
- **Done when:**
  - The listed vitest files pass, plus the existing floating, chat and
    settings suites.
  - Manual: matrix items 1–3 on one platform with a stub view.

### WP-V6 The view page

- **Files (owned):** `ui/extension-view.html`,
  `ui/js/extension-view/main.js`, `ui/js/extension-view/chrome.js`,
  `ui/js/extension-view/watchdog.js`, `ui/css/extension-view.css`,
  `ui-tests/extension-view/*.test.js` (new),
  `ui-tests/shared/link-handler.test.js` (add the page).
- **Done when:**
  - The vitest files pass.
  - Manual: matrix items 4–9 on macOS and Windows with the hostile
    fixture.
  - The page passes the RTL check (Arabic) and both themes.

### WP-V7 Docs

- **Files (owned):** `docs/EXTENSIONS.md` (new "Views" section: manifest,
  capability, provider contract, `context.view`, `context.storage`,
  `open_view` action and `host` effects, performance guidance, the
  "Kage never asks for secrets in a view" note; feature-detection rows;
  capability table row), `docs/SECURITY_MODEL.md` (§17.3),
  `docs/PRIVACY.md` (telemetry event), `CLAUDE.md` (one paragraph under
  Frontend: view windows are on-demand, labelled `ext-view:*`, normal
  windows on macOS, never overlays).
- **Done when:** the docs match what V2–V6 shipped (reviewed against the
  code), and links resolve.

### WP-V8 Mind Map explorer (Kage-Extensions)

See §24.9.

## 24. Mind Map integration

What Mind Map (`extensions/mindmap/` in Kage-Extensions) does with this
API. The existing SPEC and ARCHITECTURE stay valid. The map card in the
launcher and chat stays; the explorer is the full-window companion that
SPEC §19 deferred ("Pan/zoom canvas: needs a host view surface").

### 24.1 Manifest

```json
{
  "permissions": ["storage", "urls", "network", "windows", "views"],
  "contributes": {
    "searchProvider": "./search.js",
    "toolProvider": "./tools.js",
    "messageFormatters": "./formatter.js",
    "settingsProvider": "./settings.js",
    "css": ["./styles.css"],
    "views": [
      {
        "id": "explorer",
        "title": "__MSG_view.explorer.title__",
        "icon": "🗺️",
        "module": "./explorer.js",
        "css": ["./explorer.css"],
        "defaultSize": { "width": 1200, "height": 800 },
        "minSize": { "width": 640, "height": 420 }
      }
    ]
  }
}
```

- No `config` changes. The owner's defaults stay: agent tools on, fetch
  link titles on, record source window on.
- Adding `views` means existing users approve once (Store → Approve). Until
  then nothing explorer-related shows (§24.8).

### 24.2 Ways in

All of them only when `context.hostFeatures.includes('views')`.

| Where | What |
|---|---|
| Launcher | `mm map [focus]` keeps its rows and adds a first row "🗺️ Open in explorer". `execute()` → `{ type: 'open_view', view: 'explorer', params: { focus } }`. New phrase `mm explore [focus]` does the same directly. |
| Map card (formatter) | A header button "⤢ Explorer" (`data-ext-action="mm:b<blk>:explore"`). `onResultAction` returns `{ host: { type: 'open_view', view: 'explorer', params: { focus: <card's current ref> } } }`. |
| Item card (formatter) | "Explore" next to "Map": the same with `focus: <itemId>`. |
| Settings | Action "Open the map explorer" → `{ host: { type: 'open_view', view: 'explorer' } }`. The host's Windows block offers Open and Reset too. |

`params`: `{ focus?: 'o' | <itemId> | <entityId>, query?: string }`.
`query` is at most 200 characters of filter syntax (for example
`kind:doc @bingo`), shown in the explorer's search box and applied as a
lens.

### 24.3 Modules

New files, following ARCHITECTURE §1 (entries import anything, composers
import only leaves, leaves import nothing):

| File | Role | Contents |
|---|---|---|
| `explorer.js` | entry (view module) | The class from §9.1. Wires `getMind(context)`, the scene, the renderer, the panel, storage events and `mm-view`. |
| `explorer-canvas.js` | composer | Canvas renderer, hit testing, pointer/wheel/keyboard input → intents (`select`, `move`, `pin`, `focus`, `expand`, `pan`, `zoom`). Imports `lib/camera.js`, `lib/spatial.js`. |
| `explorer-panel.js` | composer | DOM side panel (details, actions, search, lenses) and the accessible node list. Gets `view.js` helpers (labels, icons, dates, escaping) injected by `explorer.js`, because a composer can't import `view.js`. |
| `lib/camera.js` | leaf | `createCamera`, `worldToScreen`, `screenToWorld`, `zoomAt(cam, factor, sx, sy, {min:0.1, max:4})`, `fit(cam, bounds, viewport, pad)`, `panBy`. Pure. |
| `lib/scene.js` | leaf | `buildScene(port, {focus, hops:2, cap:300, pins, rtl, lens})` → `{nodes, edges, bounds}`; `expand(scene, ref, port, {cap:60, max:2000})`; `diffScene(prev, next)`; deterministic seed positions. |
| `lib/spatial.js` | leaf | Uniform grid: `createGrid(cell)`, `insert`, `remove`, `query(rect)`, `hit(x, y)`. |
| `lib/layout.js` | leaf | `relax(data)`: a self-contained force relaxation (no closures, no imports) suitable for `runSandboxed`. Pinned nodes fixed. Seeded by id, so results are stable. |
| `explorer.css` | view CSS | `ex-` prefixed classes; uses `var(--kage-*)`. |

`mind.js` gains one read method on its `DataPort`:
`port.links(ref, {limit})` → `{ node: NodeVM, links: [{ ref, role, style,
w }] }`. It uses the same sources as `graph.neighborhood` (mentions,
co-occurrence, children, related) without lane paging.

### 24.4 What the explorer draws

- **Nodes:** entities (pill with type icon: 👤 👥 📁 #️⃣ 🔤 🏢) and items
  (card with kind icon: 📝 🔗 📄 🗓️ ☐ ☑ ❔). Suggested items and inferred
  links are dashed with ❔. Done actions are muted. Private items show 🔒
  and stay local. A 📌 marks a pinned node.
- **Edges:** solid for mentions and parent/child, dashed for suggested and
  BM25-related (top 3), thin co-occurrence edges whose opacity follows the
  weight.
- **Start:** the focus (from params, else `mm-view.focus`, else the
  overview `o`), 2 hops, at most 300 nodes. Seed positions follow the map
  card's lanes so the two feel alike: entities above, actions on the start
  side, docs and links on the end side, notes and meetings below (mirrored
  in RTL). Pinned nodes keep their saved positions. One `relax` pass in
  `runSandboxed` (≤ 300 iterations, 250 ms timeout; on timeout the seeds
  stay).
- **Growth:** expanding a node adds up to 60 neighbours. The scene holds at
  most 2,000 nodes. Beyond that a toast says "The map is full. Collapse
  something or refocus."
- **Level of detail:** below zoom 0.35 dots only; 0.35–0.7 icon and a
  12-character label; above 0.7 the 24-character label; above 1.2 also the
  sub line (kind · date · source app).
- **Theme:** every colour from `context.view.theme.tokens`; redraw on
  `onThemeChange`.

### 24.5 Interactions and data

| Input | Effect | Data |
|---|---|---|
| Drag the background, Space + drag, arrow keys with nothing selected | pan | `mm-view` camera (debounced 2 s) |
| Wheel | zoom at the cursor, or pan (toolbar toggle "Scroll zooms / Scroll pans"; default pan on macOS, zoom elsewhere) | `mm-view.scroll` |
| ⌘/Ctrl + wheel, pinch, `+` / `−`, `0` (fit) | zoom | `mm-view` |
| Click a node | select; the panel shows it (`port.item(id)` or the entity summary) | none |
| Drag a node | move and pin | `mm-layout` |
| `P` or the panel's Pin button | pin / unpin | `mm-layout` |
| Double-click or Enter | focus there (history ≤ 20; Back, Home in the toolbar) | `mm-view.focus` |
| Space or the node's ＋ handle | expand neighbours | none |
| Panel: Done, Reopen, Accept, Dismiss, Private | `mind.setStatus` / `accept` / `dismiss` / `setPrivate`, with an Undo toast (`mind.undo`) | normal Mind Map keys, journaled |
| Panel: Open | links → `invoke('open_url')`; paths → `context.view.copyText(path)` ("Path copied"), since Mind Map has no `launch`; notes → full text in the panel | none |
| Panel: Ask | `context.view.compose('mm? ' + t('explorer.ask_about', { title }))`; disabled for private items | none |
| `/` or ⌘/Ctrl+F | search box: `mind.parse` + `mind.find`; Enter focuses the first hit | none |
| Lens chips | kinds, "My actions" (owner `me`), "Show done" | `mm-view.lens` |
| Arrow keys with a selection | move the selection to the nearest node in that direction | none |
| Esc | clear the selection or close the search | none |
| ⌘/Ctrl+Z | undo the last layout move (local stack, ≤ 50, this session only) | `mm-layout` |
| `context.view.setTitle` | the focus name ("Bingo") | none |

Keys:
- **`mm-layout`** (new): `{ schema: 1, rev, txn, pins: { [ref]: { x, y,
  at } } }`. At most 5,000 pins. Written under `store.withLock()` with the
  usual twin or atomic write. Not in the journal: layout moves have their
  own local undo. With Kage sync on, each pin syncs as one record of the
  `pins` collection (SYNC §12.9), so pins moved on two computers merge.
  "Reset map layout" in settings deletes it after a confirm; with sync on
  the confirm says it resets the layout on all your computers. Export JSON
  gains `layout.pins`; import restores them.
- **`mm-view`** (the key SPEC §7.1 reserved; per computer, not listed in
  the manifest's `sync` block, so it never syncs): `{ schema: 1, camera: { x, y, zoom }, focus, history, lens,
  scroll, panelOpen, updatedAt }`. Only the explorer writes it, last write
  wins, no lock. Debounced to one write per 2 s.

Change events:
- The explorer subscribes with `context.storage.onChanged`. On a change it
  didn't make (`self: false`) to `mm-meta` or `mm-layout`, it runs
  `mind.syncIfStale(0)`, rebuilds the scene for the current node set
  (positions kept), and redraws once.
- Mind Map's launcher, chat and settings instances also subscribe when
  `storage.change_events` is present. A change to `mm-meta` makes the next
  `match()` sync first, so a capture in the explorer shows in the launcher
  at once. The 1.5 s `syncIfStale` stays as a backstop.

Launcher to explorer:
- With the explorer open, choosing "Open in explorer" for another entity in
  the launcher delivers `onOpen({ params: { focus } })`. The explorer
  re-centres (animated unless `theme.reducedMotion`), pushes history and
  updates the title.

### 24.6 Rules that carry over

- **Private items** are drawn locally only. They never go into `compose`
  text or an agent prompt. Copying is allowed (it's the user's own
  clipboard).
- **Actions stay Mind Map's** and are separate from Todos. The explorer
  only uses Mind Map actions. "My actions" means `owner: me`, and
  unassigned actions are owned by "me".
- **Background organising** (separate host API, off by default) is shown,
  not driven: inferred entities and links appear dashed with ❔ and can be
  accepted or dismissed in the panel. If the organiser publishes a status,
  a small "Organising in the background" chip appears in the toolbar.
- **File contents on capture** (EXTENSION_FILE_TEXT): the explorer's
  "＋ Add" box captures text through `mind.capture(text, {via:
  'explorer'})`. EXTENSION_FILE_TEXT's WP-FT-MM3 adds an **Add a file…**
  button to the explorer's toolbar (its C11) after WP-V8 merges. It shows
  only when `files.pick_text` is in `context.hostFeatures`, and a click
  on it counts as a host-seen click through §13.4. It hands the
  extracted text to the same ingest path as the launcher.

### 24.7 Accessibility, i18n, motion

- A parallel, focusable `<ul role="listbox">` mirrors the visible nodes in
  reading order (focus, then lanes), with an `aria-live` line announcing
  the selection. A "List" toggle shows it as the main view, matching the
  map card's list mode.
- Every string uses `context.i18n.t`. New keys under `explorer.*` and
  `view.explorer.title` in `_locales/en/messages.json`; `check:i18n`
  passes. Labels in RTL use `ctx.direction = 'rtl'`; the panel moves to
  the start side.
- No animation when `theme.reducedMotion` is true; `relax` results are
  applied without tweening.

### 24.8 Degradation

| Host | Explorer |
|---|---|
| Has `views` | everything in this section |
| Has `storage.change_events`, no `views` (new host, not approved) | No explorer rows or buttons. Settings shows "Approve Mind Map's new permission in the Store to use the map explorer." |
| Neither (older host) | No explorer rows, buttons or hint. The map card works as today. |

### 24.9 WP-V8 for Mind Map

- **Files (owned):** `extensions/mindmap/manifest.json`, `explorer.js`,
  `explorer-canvas.js`, `explorer-panel.js`, `explorer.css`,
  `lib/camera.js`, `lib/scene.js`, `lib/spatial.js`, `lib/layout.js` and
  their tests; `mind.js` (`port.links`, `mm-layout` and `mm-view`
  read/write, change-event subscription); `search.js` (explorer rows,
  `mm explore`, `onResultAction` → `open_view`); `view.js` (the map-card
  and item-card buttons, `explore` in `parseAction`); `settings.js` (open
  and reset actions, approval hint); `_locales/en/messages.json`;
  `host-loader.test.js` (new entry and composers); `README.md`;
  `docs/mindmap/SPEC.md` §9 and §19, `docs/mindmap/ARCHITECTURE.md` §2–§5,
  `docs/mindmap/OPEN_QUESTIONS.md` Q7 (mark "decided: build it");
  `scripts/host-capabilities.mjs` and `web/store-core.js` (mirror the
  `views` capability: icon, label, description; no commands map to it),
  `scripts/validate-manifests.mjs` and its test (`contributes.views`:
  the §4 rules, module and CSS paths exist and start with `./`). Without
  the mirror, `validate-manifests` rejects `views` as an unknown
  capability.
- The explorer's "Add a file…" button and doc detail pane are not part
  of this package. EXTENSION_FILE_TEXT's WP-FT-MM3 adds them after this
  package merges (§24.6).
- **Can start before the host work:** `lib/camera.js`, `lib/scene.js`,
  `lib/spatial.js`, `lib/layout.js`, `port.links`, with tests.
- **Budgets** (10k items, mid-range laptop):

  | Path | Budget |
  |---|---|
  | `mount` to first frame, warm data | ≤ 300 ms after `ready()` |
  | `buildScene`, 2,000 nodes | ≤ 50 ms |
  | Frame, 2,000 nodes and 4,000 edges visible | ≤ 8 ms |
  | `relax` pass | ≤ 250 ms (hard timeout) |
  | Change event to redraw | ≤ 100 ms |
  | Explorer memory | ≤ 80 MB on top of the WebView baseline |

- **Done when:**
  - `npm test` passes in Kage-Extensions, including `check:security`,
    `check:permissions`, `check:i18n`, `check:defaults` and
    `host-loader.test.js`.
  - New tests pass: camera round trips and zoom-at-point; scene
    determinism (same input, same positions) and RTL mirroring; spatial
    query and hit; `relax` survives `Function.toString()` and runs in a
    plain worker; explorer acceptance in jsdom with a recording 2D-context
    stub (open with `focus`, pin persists to `mm-layout`, a non-self
    `mm-meta` change triggers one redraw, a private node never reaches
    `compose`, `onOpen` re-centres).
  - Manual on a Kage build with V1–V6: matrix item 10, the launcher-to-
    explorer focus flow, and pinning on one window showing after reopen.

## 25. Open questions and later work

Open questions (assumed answers in bold; each is a small change):
1. May a view hide the host bar while the window is in native full
   screen? **No.** The bar is the identity anchor.
2. Teardown after **10 minutes** hidden. Shorter saves memory; longer
   makes reopen instant more often.
3. Mind Map's default scroll behaviour: **pan on macOS, zoom elsewhere**,
   with a toolbar toggle.
4. Should a view be allowed to open from an agent tool call (no click)?
   **No.** The tool can return a card with an "Open explorer" button
   instead.

Later work, not in v1:
- The same CSP for hidden sandboxes (blocks direct network requests from
  every extension; audit store extensions first).
- Bundle assets for views (images, fonts) through a host-read, size-capped
  `blob:` URL map.
- Several windows of one view (`instances: "multiple"`).
- A view docked inside the chat window.
- "Drop a file to capture" through the file-reading host API.

Observed host issues, out of scope (flag only):
- `setup::update_activation_policy`'s real-window list doesn't include
  `chat-*` windows, so a visible peer chat window alone may not keep Kage
  in ⌘Tab.
- Closing Settings or the Store on macOS hides the whole app, including an
  open chat window. This design changes that for views only.
- Hidden sandbox iframes can make direct network requests to servers that
  allow any origin, whatever their grant (see the first later-work item).

## 26. Alternatives considered

| Alternative | Why not |
|---|---|
| The extension sends draw commands; the host draws on its own canvas | Slow (every frame crosses the port), limits what views can do, and doesn't remove the imitation risk, because the host would still draw whatever it's told. |
| A view as an `NSPanel` on macOS | Would show over full-screen apps, but has no ⌘Tab or Dock presence, can't be destroyed after the class swap, and floats above every app. |
| Reuse the `window` capability | `window` is about Kage's own chrome. A window the extension draws is a new trust step the user should see. |
| A direct message bus between instances (`context.instances.post`) | Two sources of truth, lost on reload, and no help with sync. Storage plus change events covers the needs (§10.3), and `open_view` params cover intent handoff. |
| Geometry in `config.json` | Needs allowlist and import decisions, and the sync design would have to exclude it field by field. It's device-local data, so it lives in its own local file. |
| `context.views.open()` callable anywhere | A tool provider or a timer could pop windows. Declarative effects tie opening to a user action without a gesture token. |
| HTML5 file drops into views | Would hand file contents to the extension without the file-reading capability. |
