/**
 * Tests for config-cache: the per-window memoisation layer for get_config.
 *
 * The contract:
 *   - first call invokes the backend, caches the result;
 *   - subsequent calls return clones (no shared-reference mutation hazard);
 *   - concurrent first-time callers share one in-flight invoke;
 *   - config_updated event invalidates the cache;
 *   - invoke errors don't poison the cache (next call retries).
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';

// The module installs a global Tauri-event listener at first use. Each
// test starts with a fresh module registry + a freshly-stubbed
// __TAURI__.event.listen to avoid cross-test contamination.
let listenCb = null;
let listenCallCount = 0;

beforeEach(async () => {
    vi.resetModules();
    listenCb = null;
    listenCallCount = 0;
    globalThis.window = globalThis.window || globalThis;
    window.__TAURI__ = {
        event: {
            listen: vi.fn(async (name, cb) => {
                listenCallCount += 1;
                if (name === 'config_updated') {
                    listenCb = cb;
                }
                return () => {}; // unlisten fn
            }),
        },
    };
});

async function loadModule() {
    return await import('../../ui/js/shared/config-cache.js');
}

describe('config-cache', () => {
    it('first call invokes the backend; subsequent calls reuse the cached value', async () => {
        const { getConfig } = await loadModule();
        const invoke = vi.fn().mockResolvedValue({ hotkey: { key: 'Space' } });

        const a = await getConfig(invoke);
        const b = await getConfig(invoke);

        expect(invoke).toHaveBeenCalledTimes(1);
        expect(a).toEqual({ hotkey: { key: 'Space' } });
        expect(b).toEqual({ hotkey: { key: 'Space' } });
    });

    it('returns a fresh clone each call so mutations do not leak', async () => {
        const { getConfig } = await loadModule();
        const invoke = vi.fn().mockResolvedValue({
            ui: { theme: 'dark' },
            shortcuts: [{ name: 'a' }],
        });

        const a = await getConfig(invoke);
        a.ui.theme = 'light';
        a.shortcuts.push({ name: 'b' });

        const b = await getConfig(invoke);
        // b reflects the original cached payload, not a's mutations.
        expect(b.ui.theme).toBe('dark');
        expect(b.shortcuts).toEqual([{ name: 'a' }]);
        // And b is itself isolated from a.
        expect(b).not.toBe(a);
        expect(b.ui).not.toBe(a.ui);
    });

    it('concurrent first-time callers share a single in-flight invoke', async () => {
        const { getConfig } = await loadModule();
        let resolveInvoke;
        const invoke = vi.fn(() => new Promise((res) => { resolveInvoke = res; }));

        const p1 = getConfig(invoke);
        const p2 = getConfig(invoke);
        const p3 = getConfig(invoke);

        // All three started before the invoke resolves — only one IPC.
        expect(invoke).toHaveBeenCalledTimes(1);
        resolveInvoke({ ok: true });

        const [a, b, c] = await Promise.all([p1, p2, p3]);
        // Each caller still gets its own clone.
        expect(a).toEqual({ ok: true });
        expect(a).not.toBe(b);
        expect(b).not.toBe(c);
    });

    it('config_updated event invalidates the cache', async () => {
        const { getConfig } = await loadModule();
        const invoke = vi.fn()
            .mockResolvedValueOnce({ generation: 1 })
            .mockResolvedValueOnce({ generation: 2 });

        const first = await getConfig(invoke);
        expect(first.generation).toBe(1);

        // Simulate the backend broadcasting config_updated.
        expect(typeof listenCb).toBe('function');
        listenCb({ payload: null });

        const second = await getConfig(invoke);
        expect(second.generation).toBe(2);
        expect(invoke).toHaveBeenCalledTimes(2);
    });

    it('an invoke error is not cached — the next call retries', async () => {
        const { getConfig } = await loadModule();
        const invoke = vi.fn()
            .mockRejectedValueOnce(new Error('transient backend error'))
            .mockResolvedValueOnce({ ui: { theme: 'system' } });

        await expect(getConfig(invoke)).rejects.toThrow('transient backend error');
        const result = await getConfig(invoke);
        expect(result.ui.theme).toBe('system');
        expect(invoke).toHaveBeenCalledTimes(2);
    });

    it('explicit invalidateConfig forces a re-fetch on next read', async () => {
        const { getConfig, invalidateConfig } = await loadModule();
        const invoke = vi.fn()
            .mockResolvedValueOnce({ generation: 1 })
            .mockResolvedValueOnce({ generation: 2 });

        await getConfig(invoke);
        invalidateConfig();
        const second = await getConfig(invoke);

        expect(second.generation).toBe(2);
        expect(invoke).toHaveBeenCalledTimes(2);
    });

    it('installs the config_updated listener exactly once across many reads', async () => {
        const { getConfig } = await loadModule();
        const invoke = vi.fn().mockResolvedValue({});

        await getConfig(invoke);
        await getConfig(invoke);
        await getConfig(invoke);

        // listenCallCount tracks total listen() invocations, including any
        // for non-config_updated events (none here). One config_updated
        // listener installed regardless of how many getConfig calls fire.
        expect(listenCallCount).toBe(1);
    });

    // --- onConfigChange: the race-proof subscription path ---------------
    //
    // These pin the invariant that fixed the "new shortcut doesn't show up
    // until restart" bug: a subscriber that reads config inside its handler
    // must observe the FRESH value, because the cache is invalidated before
    // any subscriber runs. If someone refactors the ordering, these fail.

    it('onConfigChange subscriber sees fresh config (cache invalidated first)', async () => {
        const { getConfig, onConfigChange } = await loadModule();
        const invoke = vi
            .fn()
            .mockResolvedValueOnce({ generation: 1 })
            .mockResolvedValueOnce({ generation: 2 });

        // Prime the cache at generation 1.
        expect((await getConfig(invoke)).generation).toBe(1);

        // A subscriber that re-reads config when notified must see gen 2,
        // proving the cache was cleared before the subscriber ran. The
        // listener dispatches subscribers fire-and-forget (the cache is
        // already invalidated synchronously by then), so we capture and
        // await the subscriber's own work to observe its result.
        let subscriberWork = null;
        onConfigChange(() => {
            subscriberWork = getConfig(invoke);
        });

        expect(typeof listenCb).toBe('function');
        listenCb({ payload: null });

        const seen = await subscriberWork;
        expect(seen.generation).toBe(2);
    });

    it('onConfigChange installs the listener even with no prior getConfig', async () => {
        const { onConfigChange } = await loadModule();
        let fired = false;
        onConfigChange(() => {
            fired = true;
        });
        // The owning config_updated listener must exist now, not only after
        // a getConfig call.
        expect(typeof listenCb).toBe('function');
        listenCb({ payload: null });
        expect(fired).toBe(true);
    });

    it('onConfigChange returns an unsubscribe that stops further notifications', async () => {
        const { onConfigChange } = await loadModule();
        let count = 0;
        const off = onConfigChange(() => {
            count += 1;
        });
        listenCb({ payload: null });
        off();
        listenCb({ payload: null });
        expect(count).toBe(1);
    });

    it('one subscriber throwing does not stop the others', async () => {
        const { onConfigChange } = await loadModule();
        const calls = [];
        onConfigChange(() => {
            calls.push('a');
            throw new Error('boom');
        });
        onConfigChange(() => {
            calls.push('b');
        });
        // Should not throw out of the listener despite subscriber a.
        expect(() => listenCb({ payload: null })).not.toThrow();
        expect(calls).toEqual(['a', 'b']);
    });
});
