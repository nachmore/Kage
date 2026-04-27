# Extension Development Guide

This document describes how to create extensions for Kage. Extensions can add search providers (instant results in the floating window), settings pages, widget slots, and custom themes.

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
    "settingsModule": "./settings.js",
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
| `config` | No | Config schema — keys become the extension's config object. |
| `contributes` | No | What the extension provides (see below). |

### Config Schema

Each key in `config` defines a setting with a type and default value. The framework stores extension configs in `config.extensions["<id>"]` as a JSON object. Extensions read their config via the `context.config` object passed to `initialize()`.

Supported types: `"boolean"`, `"string"`, `"number"`.

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
  "permissions": ["storage", "shell", "notifications"]
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
| `storage`       | 💾  | The extension's own sandboxed data and config: `save_extension_data`, `load_extension_data`, `delete_extension_data`, `get_extension_config`, `save_extension_config`, read-only `get_config`, `save_frecency`, `load_frecency`. |
| `clipboard`     | 📋  | `read_clipboard`, `get_clipboard_history`, `paste_clipboard_item`. |
| `shell`         | 🌐  | `open_url`, `open_path`, `launch_app_by_name`, `fetch_favicon`, `fetch_link_metadata`. |
| `filesystem`    | 📂  | `pick_folder`, `scan_folder`, `execute_folder_plan`, `get_common_folders`, `search_files`, `resolve_directories`. |
| `window`        | 🪟  | Kage-owned window chrome: `resize_floating_window`, `set_floating_opacity`, `start_drag_window`, window geometry. |
| `windows`       | 🧿  | Other apps' windows: `list_open_windows`, `focus_open_window`, `get_process_name`, `get_source_window`, `get_app_icon`. |
| `notifications` | 🔔  | `notify_frontend_ready`. |
| `calendar`      | 📅  | `get_calendar_events`, `get_calendar_events_for_date`. |
| `session`       | 💬  | `list_sessions`, `load_session`, `get_current_session_id`, `get_floating_session_id`, `get_sessions_directory`. |
| `agent`         | 🤖  | LLM communication: `send_message_streaming`, `cancel_generation`, `send_steering_message`, `send_extension_tool_steering`, `extension_tool_response`, `open_chat_with_message`, `get_available_models`, `get_slash_commands`. |
| `activity`      | 📊  | `start_activity_tracker`, `stop_activity_tracker`, `get_activity_report`, `is_activity_tracker_running`. |
| `automation`    | ⚡   | `emit_automation_signal`, `list_automation_signals`, `get_power_status`. |
| `tts`           | 🔈  | `pocket_tts_test`, `pocket_tts_voices`. |

The authoritative command-to-capability mapping is in
[`ui/js/shared/extension-permissions.js`](../ui/js/shared/extension-permissions.js).
If you add a new Tauri command you want extensions to be able to
call, add it to that table under an existing capability (or propose
a new one).

### Forbidden commands

Some commands are never callable from any extension regardless of
capabilities. These include: `save_config`, `quit_app`,
`restart_app`, `execute_system_command`, install/uninstall commands,
tool-permission policy commands, `read_extension_file`,
`open_devtools`, MCP config commands, updater commands, inline
assist, the shortcut executor, and more. See
`extension-permissions.js` for the full list.

## Vendor libraries

Some extensions need a UMD/IIFE library (e.g. mathjs) that sets
globals on the sandbox window. Because the sandbox has no network
access, these can't be loaded from a CDN; and because they're not ES
modules, they can't be `import`ed from a blob URL. Instead, extensions
opt in via the `sandboxVendor` manifest field:

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

| Name   | Global set        | File                  |
|--------|-------------------|-----------------------|
| `math` | `window.math` (mathjs) | `ui/vendor/lib/math.js` |

If you need a vendor library that isn't on the list, open a PR
adding it. Third-party extensions currently can't add their own
vendor libs — only built-in extensions ship vendor bundles.

At sandbox init, the host fetches each named file and hands the
source text to the runtime, which evaluates it via an inline
`<script>` tag before any provider module loads. The global set by
the library is then visible to provider code for the lifetime of the
sandbox.

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
  granted capabilities before calling Tauri.
- `context.log` writes to the main Kage log via the bridge; it's
  a structured logger (`log.info`, `log.warn`, etc.) not a
  `console.log` replacement. Regular `console.log` inside the
  sandbox still works but goes to the sandbox iframe's dev
  console, not the main one.
- You cannot attach event listeners to the parent window or
  manipulate parent DOM. `document.*` inside the sandbox exists
  but it's the sandbox's own empty document — writes have no
  visible effect.

## Search Provider API

A search provider is an ES module that default-exports a class:

```js
export default class MySearchProvider {
    /**
     * Called once when the extension loads.
     * @param {object} context - { invoke, config }
     *   invoke: Tauri invoke function for IPC
     *   config: The extension's config object (from manifest defaults or user overrides)
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
     * Called after match(). Results are merged in.
     * @param {string} query
     * @returns {Promise<SearchResult[]>}
     */
    async matchAsync(query) {
        return [];
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
| `custom` | Extension handles it | `data: any` |

### Score Guidelines

| Range | Use for |
|-------|---------|
| 90-100 | Exact/high-confidence matches (colors, URLs) |
| 80-89 | Strong matches (system commands, shortcuts) |
| 70-79 | Good matches (app names, command names) |
| 60-69 | Partial matches |
| < 60 | Weak/fuzzy matches |

## Settings Provider API

A settings provider is an ES module that default-exports a class. The
provider runs inside the extension sandbox and **never touches the host
DOM**. Instead, it returns a JSON schema describing the settings UI, and
the host renders it. Action buttons route back through the provider as
RPC calls.

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

    /** Declarative description of the settings UI. */
    getSettings() {
        return {
            description: 'Optional text shown below the header.',
            sections: [
                {
                    label: 'Display',                  // optional
                    controls: [
                        { type: 'checkbox', id: 'enabled', label: 'Enable', default: true,
                          description: 'Turn the extension on or off.' },
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
     * `{ values: <canonical map> }`.
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

| Type       | Props                                                                 |
|------------|-----------------------------------------------------------------------|
| `checkbox` | `id`, `label`, `description?`, `default?`, `showWhen?`                |
| `text`     | `id`, `label`, `description?`, `default?`, `placeholder?`, `maxWidth?`, `showWhen?` |
| `number`   | `id`, `label`, `description?`, `default?`, `min?`, `max?`, `step?`, `maxWidth?`, `showWhen?` |
| `select`   | `id`, `label`, `description?`, `default?`, `options: [{value, label}]`, `maxWidth?`, `showWhen?` |
| `range`    | `id`, `label`, `description?`, `default?`, `min`, `max`, `step?`, `unit?`, `showWhen?` |
| `action`   | `id`, `label` (button text), `description?`, `action` (RPC name), `variant?: 'default'|'danger'|'primary'`, `confirm?`, `showWhen?` |
| `info`     | `label?`, `html` (sanitized), `showWhen?`                             |

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
| `download`            | `filename`, `content`, `mime?`                             | —                            |
| `pick_file`           | `accept?` (e.g. `.json`), `action?` (routed back)          | `onFileSelected`             |
| `play_timer_sound`    | `soundId`, `customPath?`, `repeats?`                       | —                            |

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

The icon field is rendered as **plain text** (typically an emoji).
SVG and HTML are not supported — the sandbox security model doesn't
let the host trust icon markup. Use a representative emoji or a short
text label.

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
- `data-*` attributes other than `data-ext-action`.
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
