/**
 * Mascot activity hints: host-side validation.
 *
 * `context.mascot.setActivity(name, { ttlMs })` in the sandbox becomes a
 * `{ type: 'mascot-activity' }` port message. The host is the authority: it
 * checks the closed vocabulary, checks the manifest declaration, honours the
 * user's `ui.mascot_extension_hints` setting, clamps the lease, and only then
 * forwards to `window.__kageMascotHint`. These tests drive `_handleMascotActivity`
 * directly — same approach as extension-sandbox-host.test.js, since a real
 * null-origin iframe isn't testable under jsdom.
 */

import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { invalidateConfig } from '../../ui/js/shared/config-cache.js';
import {
    clampMascotTtl,
    ExtensionSandbox,
    MASCOT_ACTIVITIES,
    MASCOT_TTL_DEFAULT_MS,
    MASCOT_TTL_MAX_MS,
    MASCOT_TTL_MIN_MS,
    normalizeMascotActivities,
} from '../../ui/js/shared/extension-sandbox-host.js';

/** Config returned by the stubbed `get_config`. Mutated per test. */
let configValue;
/** The `config_updated` callback config-cache registered, if any. */
let configUpdatedCb = null;

function makeSandbox({ mascotActivities, capabilities = [] } = {}) {
    const rawInvoke = vi.fn(async (command) => {
        if (command === 'get_config') return configValue;
        return {};
    });
    const sb = new ExtensionSandbox(
        {
            extensionId: 'test-ext',
            capabilities,
            config: {},
            sources: {},
            mascotActivities,
        },
        rawInvoke,
        document.body,
    );
    sb._port = { postMessage: () => {}, close: () => {} };
    sb._ready = true;
    return { sb, rawInvoke };
}

beforeEach(() => {
    configValue = { ui: { mascot_extension_hints: true } };
    invalidateConfig();
    window.__kageMascotHint = vi.fn();
    window.__kageMascotHintClear = vi.fn();
    // config-cache installs its `config_updated` listener through the Tauri
    // global; capture the callback so we can simulate a config change.
    window.__TAURI__ = {
        event: {
            listen: (_name, cb) => {
                configUpdatedCb = cb;
                return Promise.resolve(() => {});
            },
        },
    };
    vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
    vi.restoreAllMocks();
    delete window.__kageMascotHint;
    delete window.__kageMascotHintClear;
});

describe('mascot vocabulary + TTL helpers', () => {
    it('exposes exactly the agreed generic vocabulary', () => {
        expect([...MASCOT_ACTIVITIES]).toEqual(['music', 'meeting', 'timer']);
    });

    it('clamps TTLs into the lease window', () => {
        expect(clampMascotTtl(undefined)).toBe(MASCOT_TTL_DEFAULT_MS);
        expect(clampMascotTtl(0)).toBe(MASCOT_TTL_DEFAULT_MS);
        expect(clampMascotTtl(-5)).toBe(MASCOT_TTL_DEFAULT_MS);
        expect(clampMascotTtl('nonsense')).toBe(MASCOT_TTL_DEFAULT_MS);
        expect(clampMascotTtl(1_000)).toBe(MASCOT_TTL_MIN_MS);
        expect(clampMascotTtl(60_000)).toBe(60_000);
        expect(clampMascotTtl(99_999_999)).toBe(MASCOT_TTL_MAX_MS);
    });

    it('normalizes a manifest declaration, dropping unknowns and dupes', () => {
        expect(normalizeMascotActivities(['Music', ' music ', 'timer'], 'x')).toEqual([
            'music',
            'timer',
        ]);
        expect(normalizeMascotActivities(['dancing'], 'x')).toEqual([]);
        expect(normalizeMascotActivities('music', 'x')).toEqual([]);
        expect(normalizeMascotActivities(undefined, 'x')).toEqual([]);
    });
});

describe('ExtensionSandbox._handleMascotActivity', () => {
    it('forwards a declared activity with a clamped TTL and no capability', async () => {
        const { sb } = makeSandbox({ mascotActivities: ['music'], capabilities: [] });

        await sb._handleMascotActivity({ type: 'mascot-activity', activity: 'music', ttlMs: 1_000 });

        expect(window.__kageMascotHint).toHaveBeenCalledWith('test-ext', 'music', MASCOT_TTL_MIN_MS);
    });

    it('defaults the TTL when the extension omits it', async () => {
        const { sb } = makeSandbox({ mascotActivities: ['timer'] });

        await sb._handleMascotActivity({ type: 'mascot-activity', activity: 'timer' });

        expect(window.__kageMascotHint).toHaveBeenCalledWith(
            'test-ext',
            'timer',
            MASCOT_TTL_DEFAULT_MS,
        );
    });

    it('drops an activity outside the vocabulary', async () => {
        const { sb } = makeSandbox({ mascotActivities: ['music'] });

        await sb._handleMascotActivity({ type: 'mascot-activity', activity: 'dancing' });

        expect(window.__kageMascotHint).not.toHaveBeenCalled();
        expect(console.warn).toHaveBeenCalled();
    });

    it('drops a known activity the manifest did not declare', async () => {
        const { sb } = makeSandbox({ mascotActivities: ['music'] });

        await sb._handleMascotActivity({ type: 'mascot-activity', activity: 'meeting' });

        expect(window.__kageMascotHint).not.toHaveBeenCalled();
        expect(console.warn).toHaveBeenCalled();
    });

    it('drops every hint when the manifest declares nothing', async () => {
        const { sb } = makeSandbox({});

        await sb._handleMascotActivity({ type: 'mascot-activity', activity: 'music' });

        expect(window.__kageMascotHint).not.toHaveBeenCalled();
    });

    it('drops hints when the user turned mascot extension hints off', async () => {
        configValue = { ui: { mascot_extension_hints: false } };
        const { sb } = makeSandbox({ mascotActivities: ['music'] });

        await sb._handleMascotActivity({ type: 'mascot-activity', activity: 'music' });

        expect(window.__kageMascotHint).not.toHaveBeenCalled();
    });

    it('treats a missing ui.mascot_extension_hints as enabled', async () => {
        configValue = { ui: {} };
        const { sb } = makeSandbox({ mascotActivities: ['music'] });

        await sb._handleMascotActivity({ type: 'mascot-activity', activity: 'music' });

        expect(window.__kageMascotHint).toHaveBeenCalled();
    });

    it('survives a mascot that has not installed the globals yet', async () => {
        delete window.__kageMascotHint;
        delete window.__kageMascotHintClear;
        const { sb } = makeSandbox({ mascotActivities: ['music'] });

        await sb._handleMascotActivity({ type: 'mascot-activity', activity: 'music' });
        expect(() => sb.clearMascotActivity()).not.toThrow();
    });

    it('clears on activity: null', async () => {
        const { sb } = makeSandbox({ mascotActivities: ['music'] });
        await sb._handleMascotActivity({ type: 'mascot-activity', activity: 'music' });

        await sb._handleMascotActivity({ type: 'mascot-activity', activity: null });

        expect(window.__kageMascotHintClear).toHaveBeenCalledWith('test-ext');
    });

    it('does not emit a clear for an extension that never hinted', async () => {
        const { sb } = makeSandbox({ mascotActivities: ['music'] });

        await sb._handleMascotActivity({ type: 'mascot-activity', activity: null });

        expect(window.__kageMascotHintClear).not.toHaveBeenCalled();
    });

    it('clears the hint when the sandbox is destroyed', async () => {
        const { sb } = makeSandbox({ mascotActivities: ['music'] });
        await sb._handleMascotActivity({ type: 'mascot-activity', activity: 'music' });

        sb.destroy();

        expect(window.__kageMascotHintClear).toHaveBeenCalledWith('test-ext');
    });

    it('ignores hints arriving after destroy', async () => {
        const { sb } = makeSandbox({ mascotActivities: ['music'] });
        sb.destroy();
        window.__kageMascotHint.mockClear();

        await sb._handleMascotActivity({ type: 'mascot-activity', activity: 'music' });

        expect(window.__kageMascotHint).not.toHaveBeenCalled();
    });

    it('routes through the port message handler', async () => {
        const { sb } = makeSandbox({ mascotActivities: ['music'] });

        sb._onPortMessage({ data: { type: 'mascot-activity', activity: 'music', ttlMs: 60_000 } });
        // _handleMascotActivity is async (config read); let it settle.
        await new Promise((resolve) => setTimeout(resolve, 0));

        expect(window.__kageMascotHint).toHaveBeenCalledWith('test-ext', 'music', 60_000);
    });

    it('withdraws a live hint when the user turns hints off', async () => {
        const { sb } = makeSandbox({ mascotActivities: ['music'] });
        await sb._handleMascotActivity({ type: 'mascot-activity', activity: 'music' });
        expect(window.__kageMascotHint).toHaveBeenCalled();
        expect(typeof configUpdatedCb).toBe('function');

        configValue = { ui: { mascot_extension_hints: false } };
        await configUpdatedCb({});
        // The watcher re-reads config asynchronously before clearing.
        for (let i = 0; i < 5; i++) await Promise.resolve();

        expect(window.__kageMascotHintClear).toHaveBeenCalledWith('test-ext');
    });
});
