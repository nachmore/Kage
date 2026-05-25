/**
 * Settings window entry point.
 *
 * Single ES module that wires together every settings module and the
 * surrounding chrome (theme init, console interception, navigation
 * listeners, hot-reload of extension settings, etc.).
 *
 * Loaded from `ui/settings.html` as:
 *     <script type="module" src="js/settings/main.js"></script>
 */

import './actions.js'; // installs the delegated dispatcher on import

import {
    SettingsManager,
    addExtensionSidebarItem,
    buildSandboxedSettingsModule,
    createSandboxPool,
    normalizeExtensionPermissions,
    registerManagerActions,
    setManagerHandle,
} from './manager.js';

// Theme + platform helpers (used by individual modules and by the
// settings window's own keyboard shortcut + initial paint).
import { applyTheme, initThemeListener, loadAndApplyTheme } from '../shared/theme.js';
import { cmdOrCtrlPressed } from '../shared/shortcuts.js';
import { isMacOS } from '../shared/mac-permissions.js';

// First-party settings modules (registration order matches the sidebar
// in settings.html — see the comments in that file for the contract).
import { AppearanceSettingsModule } from './appearance.js';
import { HotkeySettingsModule } from './hotkey.js';
import { SystemSettingsModule } from './system.js';
import { MacPermissionsSettingsModule } from './permissions.js';
import { AssistantSettingsModule } from './assistant.js';
import { ConnectionSettingsModule } from './connection.js';
import { OllamaSettingsModule } from './ollama.js';
import { ModelSettingsModule } from './model.js';
import { McpSettingsModule } from './mcp.js';
import { ToolPermissionsSettingsModule } from './tool-permissions.js';
import { IntegrationSettingsModule } from './integration.js';
import { ShortcutsSettingsModule } from './shortcuts.js';
import { AutomationsSettingsModule } from './automations.js';
import { SpeechSettingsModule } from './speech.js';
import { StoreSettingsModule } from './store.js';
import { PrivacySettingsModule } from './privacy.js';
import { UpdatesSettingsModule } from './updates.js';
import { AboutSettingsModule } from './about.js';

let settingsManager = null;

// --- Console interception ---------------------------------------------------
//
// Mirror console output into the backend so settings-window logs show
// up alongside the rest of the app's logs. This used to be inlined in
// settings.html as a classic script — moving it here so it runs under
// the same module loader as everything else.
(function installConsoleIntercept() {
    function fmt(v) {
        if (v === null || v === undefined) return String(v);
        if (v instanceof Error) return v.message + '\n' + (v.stack || '');
        if (typeof v === 'object') {
            try {
                return JSON.stringify(v);
            } catch {
                return String(v);
            }
        }
        return String(v);
    }
    const orig = {
        log: console.log.bind(console),
        warn: console.warn.bind(console),
        error: console.error.bind(console),
        debug: console.debug.bind(console),
    };
    function send(level, args) {
        const fn = window.__TAURI__?.core?.invoke;
        if (fn) {
            fn('app_log_write', {
                level,
                source: 'settings',
                msg: args.map(fmt).join(' '),
            }).catch(() => {});
        }
    }
    console.log = (...a) => {
        orig.log(...a);
        send('info', a);
    };
    console.warn = (...a) => {
        orig.warn(...a);
        send('warn', a);
    };
    console.error = (...a) => {
        orig.error(...a);
        send('error', a);
    };
    console.debug = (...a) => {
        orig.debug(...a);
        send('debug', a);
    };
})();

// --- Window-level keyboard shortcuts ---------------------------------------
//
// Ctrl/⌘+W hides the settings window (matches the floating + chat windows).
document.addEventListener('keydown', (e) => {
    if (cmdOrCtrlPressed(e) && e.key === 'w') {
        e.preventDefault();
        if (window.__TAURI__) {
            window.__TAURI__.webviewWindow.getCurrentWebviewWindow().hide();
        }
    }
});

// --- Initial theme paint ----------------------------------------------------
//
// The settings window picks up the user's chosen theme as soon as Tauri
// is ready. Subsequent config changes flow through `loadAndApplyTheme`
// invoked by the appearance module.
(async function paintInitialTheme() {
    function waitForTauri(cb) {
        if (window.__TAURI__?.core) cb();
        else setTimeout(() => waitForTauri(cb), 50);
    }
    waitForTauri(async () => {
        initThemeListener();
        try {
            await loadAndApplyTheme(window.__TAURI__.core.invoke);
        } catch (e) {
            console.warn('[settings] initial theme apply failed:', e);
            applyTheme('system');
        }
    });
})();

// --- Cross-window navigation -----------------------------------------------
//
// The floating window (and others) emit these events so users can
// jump directly into a specific settings section.
(function listenForNavigation() {
    function waitForTauri(cb) {
        if (window.__TAURI__?.event) cb();
        else setTimeout(() => waitForTauri(cb), 50);
    }
    waitForTauri(() => {
        window.__TAURI__.event.listen('navigate_settings_section', (event) => {
            const section = event.payload;
            if (section && settingsManager) {
                settingsManager.switchSection(section);
            }
        });
        window.__TAURI__.event.listen('navigate_settings_subsection', (event) => {
            const sub = event.payload;
            if (sub) {
                // Small delay to let the section switch render first
                setTimeout(() => {
                    document.dispatchEvent(new CustomEvent('settings-subsection', { detail: sub }));
                }, 100);
            }
        });
    });
})();

// --- Boot -------------------------------------------------------------------

window.addEventListener('DOMContentLoaded', async () => {
    settingsManager = new SettingsManager();
    setManagerHandle(() => settingsManager);
    registerManagerActions();

    const invoke = window.__TAURI__.core.invoke;

    // Register core modules (order matches sidebar)
    settingsManager.registerModule(new AppearanceSettingsModule());
    settingsManager.registerModule(new HotkeySettingsModule());
    settingsManager.registerModule(new SystemSettingsModule());
    // macOS-only: privacy/TCC permissions pane. The sidebar item for this
    // module is hidden by default in settings.html and revealed here.
    if (isMacOS()) {
        settingsManager.registerModule(new MacPermissionsSettingsModule());
        const sidebarItem = document.getElementById('macPermissionsSidebarItem');
        if (sidebarItem) sidebarItem.classList.remove('hidden');
    }
    settingsManager.registerModule(new AssistantSettingsModule());
    settingsManager.registerModule(new ConnectionSettingsModule());
    settingsManager.registerModule(new OllamaSettingsModule());
    settingsManager.registerModule(new ModelSettingsModule());
    settingsManager.registerModule(new McpSettingsModule());
    settingsManager.registerModule(new ToolPermissionsSettingsModule());
    settingsManager.registerModule(new IntegrationSettingsModule());
    settingsManager.registerModule(new ShortcutsSettingsModule());
    settingsManager.registerModule(new AutomationsSettingsModule());
    settingsManager.registerModule(new SpeechSettingsModule());
    settingsManager.registerModule(new StoreSettingsModule());

    // Settings-window-local sandbox pool. Separate from the floating/chat
    // windows' pools because each window has its own document; sandboxes
    // are tied to the document they mount into.
    const sandboxPool = createSandboxPool(invoke);

    // Preload current config so we can hand each sandbox the right initial values.
    let currentConfig = {};
    try {
        currentConfig = await invoke('get_config');
    } catch (e) {
        console.warn('Failed to preload config:', e);
    }

    // Read all user-installed extensions once — we'll iterate over both
    // bundled and user-installed lists with a single loader path.
    let installedUser = [];
    try {
        installedUser = await invoke('list_extensions');
    } catch {}

    // Helper: resolve granted capabilities for an extension. Bundled ones
    // get what their manifest declares (implicit grant); user-installed
    // ones get whatever `extension_grants[id].granted` says. See
    // docs/SECURITY_MODEL.md for the install-time grant story.
    function resolveCaps(manifest, bundled) {
        const requested = normalizeExtensionPermissions(manifest.permissions, manifest.id);
        if (bundled) return requested;
        const record = currentConfig.extension_grants?.[manifest.id];
        if (!record) return [];
        const grantedSet = new Set(normalizeExtensionPermissions(record.granted, manifest.id));
        return requested.filter((cap) => grantedSet.has(cap));
    }

    async function loadSandboxedSettings({ manifest, sourceCode, bundled }) {
        const capabilities = resolveCaps(manifest, bundled);
        const mod = await buildSandboxedSettingsModule({
            pool: sandboxPool,
            manifest,
            capabilities,
            settingsProviderSource: sourceCode,
            currentConfig,
        });
        settingsManager.registerModule(mod);
        addExtensionSidebarItem(mod.id, manifest.icon || '📦', manifest.name);
    }

    // 1. Load bundled extension settings.
    try {
        const resp = await fetch('extensions/bundled.json');
        if (resp.ok) {
            const bundledList = await resp.json();
            for (const entry of bundledList) {
                try {
                    const manifestResp = await fetch(`extensions/${entry.id}/manifest.json`);
                    if (!manifestResp.ok) continue;
                    const manifest = await manifestResp.json();
                    const settingsPath = manifest.contributes?.settingsProvider;
                    if (!settingsPath) continue; // extension has no settings UI
                    const srcResp = await fetch(
                        `extensions/${entry.id}/${settingsPath.replace('./', '')}`
                    );
                    if (!srcResp.ok) throw new Error(`HTTP ${srcResp.status}`);
                    const sourceCode = await srcResp.text();
                    await loadSandboxedSettings({ manifest, sourceCode, bundled: true });
                } catch (e) {
                    console.warn(`Failed to load bundled extension settings '${entry.id}':`, e);
                }
            }
        }
    } catch (e) {
        console.warn('Failed to load bundled.json:', e);
    }

    // 2. Load user-installed extension settings.
    try {
        const bundledIds = new Set();
        try {
            const resp = await fetch('extensions/bundled.json');
            if (resp.ok) (await resp.json()).forEach((e) => bundledIds.add(e.id));
        } catch {}

        for (const item of installedUser) {
            if (bundledIds.has(item.manifest.id)) continue;
            const manifest = item.manifest;
            const settingsPath = manifest.contributes?.settingsProvider;
            if (!settingsPath) continue;
            try {
                const sourceCode = await invoke('read_extension_file', {
                    extensionId: manifest.id,
                    kind: 'extension',
                    filePath: settingsPath.replace('./', ''),
                });
                await loadSandboxedSettings({ manifest, sourceCode, bundled: false });
            } catch (e) {
                console.warn(`Failed to load user extension settings '${manifest.id}':`, e);
            }
        }
    } catch (e) {
        console.warn('Failed to load user extensions for settings:', e);
    }

    // About section — ordering here must match the sidebar in
    // ui/settings.html (Privacy → Updates → About). Several bits of
    // settings machinery key off registration order: the first module
    // is the default-visible section, extension module sidebar entries
    // are inserted relative to these, and switchSection() relies on
    // the one-to-one mapping between sidebar data-section IDs and the
    // module IDs registered here.
    settingsManager.registerModule(new PrivacySettingsModule());
    settingsManager.registerModule(new UpdatesSettingsModule());
    settingsManager.registerModule(new AboutSettingsModule());

    // Render and load
    settingsManager.render();
    await settingsManager.load();

    // Apply URL-param navigation. open_settings_window encodes
    // section/subsection as query params when building a fresh
    // settings window, since the alternative (emit a Tauri event
    // immediately after window.show()) races against the new
    // webview's JS boot — the listener isn't attached yet when the
    // event fires, and it gets dropped. URL params survive that
    // race because they're attached to the initial document URL.
    try {
        const params = new URLSearchParams(window.location.search);
        const section = params.get('section');
        const subSection = params.get('subsection');
        if (section) {
            settingsManager.switchSection(section);
        }
        if (subSection) {
            // Same delay as the event-channel path so the section
            // switch above paints before the subsection scroll runs.
            setTimeout(() => {
                document.dispatchEvent(
                    new CustomEvent('settings-subsection', { detail: subSection })
                );
            }, 100);
        }
    } catch (e) {
        console.warn('[settings] URL-param navigation failed:', e);
    }

    // Listen for extension install/uninstall — hot-load new settings modules
    const { listen } = window.__TAURI__.event;
    listen('extensions_changed', async () => {
        console.log('[Settings] extensions_changed — checking for new modules');
        try {
            const userExts = await invoke('list_extensions');
            // Refresh config so grants are current
            try {
                currentConfig = await invoke('get_config');
            } catch {}

            const bundledIds = new Set();
            try {
                const resp = await fetch('extensions/bundled.json');
                if (resp.ok) (await resp.json()).forEach((e) => bundledIds.add(e.id));
            } catch {}

            let added = false;
            for (const item of userExts) {
                if (bundledIds.has(item.manifest.id)) continue;
                const manifest = item.manifest;
                if (!manifest.contributes?.settingsProvider) continue;

                const existingMod = settingsManager.modules.find(
                    (m) => m._extensionId === manifest.id
                );
                if (existingMod) {
                    if (existingMod._extensionVersion === manifest.version) continue;
                    console.log(
                        `[Settings] Updating '${manifest.id}' from ${existingMod._extensionVersion} to ${manifest.version}`
                    );
                    const idx = settingsManager.modules.indexOf(existingMod);
                    if (idx !== -1) settingsManager.modules.splice(idx, 1);
                    const sidebarItem = document.querySelector(
                        `.sidebar-item[data-section="${existingMod.id}"]`
                    );
                    if (sidebarItem) sidebarItem.remove();
                    try {
                        existingMod.destroy?.();
                    } catch {}
                    sandboxPool.unload(manifest.id);
                }

                try {
                    const settingsPath = manifest.contributes.settingsProvider.replace('./', '');
                    const sourceCode = await invoke('read_extension_file', {
                        extensionId: manifest.id,
                        kind: 'extension',
                        filePath: settingsPath,
                    });
                    const capabilities = resolveCaps(manifest, false);
                    const mod = await buildSandboxedSettingsModule({
                        pool: sandboxPool,
                        manifest,
                        capabilities,
                        settingsProviderSource: sourceCode,
                        currentConfig,
                    });
                    const insertIdx = Math.max(0, settingsManager.modules.length - 2);
                    settingsManager.modules.splice(insertIdx, 0, mod);
                    addExtensionSidebarItem(mod.id, manifest.icon || '📦', manifest.name);
                    added = true;
                    console.log(
                        `[Settings] Hot-loaded extension settings: ${manifest.id} v${manifest.version}`
                    );
                } catch (e) {
                    console.warn(`[Settings] Failed to hot-load '${manifest.id}':`, e);
                }
            }

            // Remove modules for uninstalled extensions
            const installedIds = new Set(userExts.map((e) => e.manifest.id));
            const toRemove = settingsManager.modules.filter(
                (m) =>
                    m._extensionId &&
                    !bundledIds.has(m._extensionId) &&
                    !installedIds.has(m._extensionId)
            );
            for (const mod of toRemove) {
                const idx = settingsManager.modules.indexOf(mod);
                if (idx !== -1) {
                    settingsManager.modules.splice(idx, 1);
                    const sidebarItem = document.querySelector(
                        `.sidebar-item[data-section="${mod.id}"]`
                    );
                    if (sidebarItem) sidebarItem.remove();
                    try {
                        mod.destroy?.();
                    } catch {}
                    sandboxPool.unload(mod._extensionId);
                    added = true;
                    console.log(`[Settings] Removed uninstalled extension: ${mod._extensionId}`);
                }
            }

            if (added) {
                settingsManager.render();
                await settingsManager.load();
            }
        } catch (e) {
            console.warn('[Settings] Failed to reload extensions:', e);
        }
    });
});
