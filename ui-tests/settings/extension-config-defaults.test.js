/**
 * The runtime and the Settings window see the same extension config.
 *
 * Both resolve it with resolveExtensionConfig (ui/js/shared/extension-config.js):
 * manifest `config` defaults overlaid with config.extensions[id], plus the
 * host's `enabled`. Covered here end to end:
 *   - the runtime sandbox's boot config and every onConfigUpdate payload,
 *   - the Settings page's values and the settings provider's own config,
 *   - a Settings Save, which stores only overrides and only this page's keys,
 *   - the header's Disable/Enable switch, which owns `enabled`.
 *
 * Regression: Math's manifest default precision is 2 but its control said 0.
 * With config.extensions.math = {}, the runtime returned `{}` as-is (so the
 * extension fell back to its own code default) while Settings showed 0.
 */

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';

const MATH = Object.freeze({
    id: 'math',
    name: 'Math',
    version: '1.0.1',
    permissions: [],
    config: {
        enabled: { type: 'boolean', default: true },
        precision: { type: 'number', default: 2 },
        auto_copy: { type: 'boolean', default: true },
        thousands_separator: { type: 'boolean', default: false },
    },
    contributes: { searchProvider: './search.js', settingsProvider: './settings.js' },
});

/** Math's settings page as shipped: its precision control disagrees with the manifest. */
const mathSchema = () => ({
    sections: [
        {
            controls: [
                { type: 'checkbox', id: 'enabled', label: 'Enable', default: true },
                { type: 'number', id: 'precision', label: 'Precision', default: 0, min: -1, max: 15 },
                { type: 'checkbox', id: 'auto_copy', label: 'Auto copy', default: true },
                { type: 'checkbox', id: 'thousands_separator', label: 'Thousands', default: false },
            ],
        },
    ],
});

const appConfig = (math, states = { math: true }) => ({
    extensions: math === undefined ? {} : { math },
    extension_states: states,
    extension_grants: { math: { granted: [], version: '1.0.1' } },
});

/** Load `manifest` into a runtime ExtensionManager with a stubbed sandbox pool. */
async function loadRuntime(manifest, config) {
    let current = structuredClone(config);
    const invoke = vi.fn(async (cmd) => (cmd === 'get_config' ? structuredClone(current) : undefined));
    const mgr = new ExtensionManager(invoke);
    mgr._configCache = structuredClone(config);
    mgr._fetchProviderSources = async () => ({ searchProvider: 'export default class {}' });
    mgr._fetchVendorSources = async () => undefined;
    mgr._fetchExtensionLocale = async () => ({ catalog: {}, fallback: {}, language: 'en', rtl: false });
    mgr._loadExtensionCss = async () => {};
    const sandbox = { updateConfig: vi.fn(async () => {}), clearMascotActivity: vi.fn() };
    mgr._pool = { load: vi.fn(async () => sandbox) };
    await mgr._loadExtension({ manifest, enabled: true });
    return {
        mgr,
        sandbox,
        bootConfig: mgr._pool.load.mock.calls[0][0].config,
        setBackend: (next) => {
            current = structuredClone(next);
        },
    };
}

let manager;
let backend;
let saved;

beforeAll(async () => {
    setupTauriMock({
        get_config: async () => structuredClone(backend),
        save_config: async ({ config }) => {
            saved.push(structuredClone(config));
            backend = structuredClone(config);
        },
    });
    manager = await import('../../ui/js/settings/manager.js');
});

afterAll(() => {
    teardownTauriMock();
});

beforeEach(() => {
    saved = [];
    document.body.innerHTML = '<div id="settingsModules"></div>';
    // The renderer warns about Math's mismatched control default.
    vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
    vi.restoreAllMocks();
});

/** Build Math's Settings module the way the Settings window does. */
async function loadSettings(config, schema = mathSchema()) {
    const sandbox = {
        hasSettings: true,
        call: vi.fn(async (method) => (method === 'getSettings' ? schema : undefined)),
        updateConfig: vi.fn(async () => {}),
    };
    const pool = { load: vi.fn(async () => sandbox), unload: vi.fn() };
    const mod = await manager.buildSandboxedSettingsModule({
        invoke: undefined,
        pool,
        manifest: MATH,
        capabilities: [],
        settingsProviderSource: '',
        currentConfig: structuredClone(config),
    });
    return { mod, sandbox, bootConfig: pool.load.mock.calls[0][0].config };
}

describe('runtime and Settings agree', () => {
    it('Math with saved {}: precision 2 for the runtime, the settings provider, and the page', async () => {
        const config = appConfig({});
        const runtime = await loadRuntime(MATH, config);
        const settings = await loadSettings(config);
        settings.mod.load(structuredClone(config));

        const expected = { precision: 2, auto_copy: true, thousands_separator: false, enabled: true };
        expect(runtime.bootConfig).toEqual(expected);
        expect(settings.bootConfig).toEqual(expected);
        expect(settings.mod._rendered.currentValues()).toEqual(expected);
        // The settings provider already has these, so load() doesn't resend them.
        expect(settings.sandbox.updateConfig).not.toHaveBeenCalled();
        // Dev aid: the control's own default is flagged.
        expect(console.warn).toHaveBeenCalledWith(expect.stringMatching(/'math'.*'precision'/));
    });

    it('a saved override reaches onConfigUpdate and the Settings page alike', async () => {
        const runtime = await loadRuntime(MATH, appConfig(undefined));
        const settings = await loadSettings(appConfig(undefined));

        const next = appConfig({ precision: 4 });
        runtime.setBackend(next);
        await runtime.mgr.onConfigUpdate();
        settings.mod.load(structuredClone(next));

        const expected = { precision: 4, auto_copy: true, thousands_separator: false, enabled: true };
        expect(runtime.sandbox.updateConfig).toHaveBeenLastCalledWith(expected);
        expect(settings.mod._rendered.currentValues()).toEqual(expected);
        // The settings provider's context.config follows too.
        expect(settings.sandbox.updateConfig).toHaveBeenCalledTimes(1);
        expect(settings.sandbox.updateConfig).toHaveBeenLastCalledWith(expected);
    });

    it('onConfigUpdate carries enabled from the host switch, not from saved config', async () => {
        const runtime = await loadRuntime(MATH, appConfig({ enabled: true }));
        expect(runtime.bootConfig.enabled).toBe(true);

        runtime.setBackend(appConfig({ enabled: true, precision: 3 }, { math: false }));
        await runtime.mgr.onConfigUpdate();
        expect(runtime.sandbox.updateConfig).toHaveBeenLastCalledWith({
            precision: 3,
            auto_copy: true,
            thousands_separator: false,
            enabled: false,
        });
    });
});

describe('Settings Save', () => {
    async function mount(config) {
        backend = structuredClone(config);
        const settings = await loadSettings(config);
        const mgr = new manager.SettingsManager();
        mgr.registerModule(settings.mod);
        mgr.render();
        await mgr.load();
        manager.setManagerHandle(() => mgr);
        manager.registerManagerActions();
        return { mgr, ...settings };
    }

    const ctrl = (id) => document.getElementById(`ext-ctrl-math-${id}`);

    function setChecked(id, checked) {
        ctrl(id).checked = checked;
        ctrl(id).dispatchEvent(new Event('change', { bubbles: true }));
    }

    function setNumber(id, value) {
        ctrl(id).value = String(value);
        ctrl(id).dispatchEvent(new Event('input', { bubbles: true }));
    }

    it('stores only overrides and keeps saved keys the page has no control for', async () => {
        // What an older host wrote: every control's value, a stale `enabled`,
        // and a scratch value; plus a key with no control.
        const { mgr } = await mount(
            appConfig({
                precision: 2,
                auto_copy: true,
                thousands_separator: false,
                enabled: false,
                legacy_key: 'keep',
            })
        );
        // The stale `enabled: false` is ignored: the page follows extension_states.
        expect(ctrl('enabled').checked).toBe(true);

        setChecked('thousands_separator', true);
        expect(mgr.isDirty('ext-math')).toBe(true);
        expect(await mgr.save()).toBe(true);

        const out = saved.at(-1);
        expect(out.extensions.math).toEqual({ legacy_key: 'keep', thousands_separator: true });
        expect(out.extension_states.math).toBe(true);
    });

    it('drops the entry when nothing differs from the defaults any more', async () => {
        const { mgr } = await mount(appConfig({ precision: 5 }));
        expect(ctrl('precision').value).toBe('5');
        setNumber('precision', 2);
        expect(await mgr.save()).toBe(true);
        expect(saved.at(-1).extensions).not.toHaveProperty('math');
    });

    it('the header switch owns enabled: the hidden control follows it and only extension_states is saved', async () => {
        const { mgr, mod } = await mount(appConfig({ precision: 3 }));
        expect(document.getElementById('ext-row-math-enabled').style.display).toBe('none');

        document.getElementById('ext-toggle-btn-math').click();
        expect(document.getElementById('ext-enabled-math').value).toBe('false');
        expect(mod._rendered.currentValues().enabled).toBe(false);
        expect(mgr.isDirty('ext-math')).toBe(true);

        expect(await mgr.save()).toBe(true);
        const out = saved.at(-1);
        expect(out.extension_states.math).toBe(false);
        expect(out.extensions.math).toEqual({ precision: 3 });
    });
});
