# Extension Development Guide

This document describes how to create extensions for Kiro Assistant. Extensions can add search providers (instant results in the floating window), settings pages, widget slots, and custom themes.

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
| `config` | No | Config schema — keys become the extension's config object. |
| `contributes` | No | What the extension provides (see below). |

### Config Schema

Each key in `config` defines a setting with a type and default value. The framework stores extension configs in `config.extensions["<id>"]` as a JSON object. Extensions read their config via the `context.config` object passed to `initialize()`.

Supported types: `"boolean"`, `"string"`, `"number"`.

### Contributes

| Key | Description |
|-----|-------------|
| `searchProvider` | Path to an ES module that default-exports a search provider class. |
| `settingsModule` | Path to a script that defines a `SettingsModule` subclass. |
| `css` | Array of CSS file paths to load. |
| `widgets` | Array of widget contributions (see Widget Slots below). |
| `themes` | For theme type only — `{ "dark": "./dark.json", "light": "./light.json" }`. |

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
| `custom` | Extension handles it | `data: any` |

### Score Guidelines

| Range | Use for |
|-------|---------|
| 90-100 | Exact/high-confidence matches (colors, URLs) |
| 80-89 | Strong matches (system commands, shortcuts) |
| 70-79 | Good matches (app names, command names) |
| 60-69 | Partial matches |
| < 60 | Weak/fuzzy matches |

## Settings Module API

A settings module is a regular script (not ES module) that defines a class extending `SettingsModule` and registers it on `window`:

```js
class MyExtSettingsModule extends SettingsModule {
    constructor() {
        super('my-extension', 'My Extension', '🔧');
        // Optional description shown below the title
        this.description = 'A short description of what this extension does.';
    }

    /**
     * Return the HTML for the settings content (no header — the framework renders that).
     */
    renderContent() {
        return `
            ${this.createCheckboxRow('Some Option', 'Description of the option.', 'myExtSomeOption', true)}
            ${this.createControlRow('Text Setting', 'Description.',
                '<input type="text" class="setting-input" id="myExtText" value="">'
            )}
        `;
    }

    // Fallback for non-extension contexts
    render() { return this.renderContent(); }

    load(config) {
        const ext = (config.extensions && config.extensions['my-extension']) || {};
        const el = document.getElementById('myExtSomeOption');
        if (el) el.checked = ext.some_option !== false;
        const text = document.getElementById('myExtText');
        if (text) text.value = ext.another_option || '';
    }

    save(config) {
        if (!config.extensions) config.extensions = {};
        config.extensions['my-extension'] = {
            some_option: document.getElementById('myExtSomeOption')?.checked ?? true,
            another_option: document.getElementById('myExtText')?.value || '',
        };
    }

    // Optional
    validate() { return { valid: true }; }
    initialize() { }
    destroy() { }
}

// IMPORTANT: Register on window so the dynamic loader can find it
window.MyExtSettingsModule = MyExtSettingsModule;
```

### Helper Methods (from SettingsModule base class)

- `createCheckboxRow(label, description, checkboxId, defaultChecked)` — checkbox with label
- `createControlRow(label, description, controlHtml)` — label + arbitrary control HTML

## Widget Slots

Extensions can contribute persistent UI widgets to specific slots:

| Slot | Location |
|------|----------|
| `floating-bottom` | Below the input/suggestions area in the floating window |
| `floating-status` | Small status indicator area |

Widget module contract (ES module):

```js
export default class MyWidget {
    mount(container, context) { }    // Called when widget should render
    onConfigUpdate(config) { }       // Called on config changes
    unmount() { }                    // Called on disable/unload
}
```

## Theme Format

Themes override CSS variables defined in `shared-kiro-tokens.css`:

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
    "kiro-accent": "#FF6B6B",
    "kiro-accent-hover": "#FF8E8E",
    "kiro-bg": "#1a1a2e",
    "kiro-text": "#e0e0e0"
  }
}
```

Only override the variables you want to change. Unspecified variables fall back to the built-in defaults. See `ui/css/shared-kiro-tokens.css` for all available variables.

## Installation

Extensions are distributed as `.zip` files. The zip should contain either:
- Files directly at the root (including `manifest.json`), or
- A single subdirectory containing `manifest.json`

Install methods:
1. **Store** — browse and install from the Extension Store
2. **Local file** — install from a `.zip` file or directory path
3. **Manual** — place the extension directory in `<config_dir>/kiro-assistant/extensions/<id>/`

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
