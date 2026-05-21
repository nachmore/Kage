/**
 * Tests for `_fetchVendorSources` — the host-side loader that resolves
 * allow-listed UMD/IIFE vendor libs (e.g. mathjs) declared by an
 * extension's `sandboxVendor` manifest field.
 *
 * The runtime-side script-tag injection runs inside the iframe and
 * isn't exercised here (needs a real iframe); manual QA covers it.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';

let origFetch;

beforeEach(() => {
    origFetch = global.fetch;
});
afterEach(() => {
    global.fetch = origFetch;
});

/** Make a fetch() that serves known URLs from a map and 404s others. */
function fakeFetch(files) {
    return vi.fn(async (url) => {
        if (Object.prototype.hasOwnProperty.call(files, url)) {
            return { ok: true, text: async () => files[url] };
        }
        return { ok: false, status: 404, text: async () => '' };
    });
}

describe('_fetchVendorSources', () => {
    it('returns undefined when manifest has no sandboxVendor', async () => {
        const mgr = new ExtensionManager(async () => undefined);
        const out = await mgr._fetchVendorSources({ id: 'test' });
        expect(out).toBeUndefined();
    });

    it('returns undefined when sandboxVendor is an empty array', async () => {
        const mgr = new ExtensionManager(async () => undefined);
        const out = await mgr._fetchVendorSources({ id: 'test', sandboxVendor: [] });
        expect(out).toBeUndefined();
    });

    it('fetches allow-listed vendor (math) and returns a name→source map', async () => {
        global.fetch = fakeFetch({
            'vendor/lib/math.js': '/* mathjs UMD */ self.math = {};',
        });
        const mgr = new ExtensionManager(async () => undefined);
        const out = await mgr._fetchVendorSources({
            id: 'math',
            sandboxVendor: ['math'],
        });
        expect(out).toBeDefined();
        expect(out.math).toContain('self.math');
    });

    it('drops unknown vendor names with a console.warn', async () => {
        const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
        global.fetch = fakeFetch({});
        const mgr = new ExtensionManager(async () => undefined);
        const out = await mgr._fetchVendorSources({
            id: 'evil',
            sandboxVendor: ['../../secret.js'],
        });
        expect(out).toBeUndefined();
        expect(warnSpy).toHaveBeenCalled();
        warnSpy.mockRestore();
    });

    it('ignores non-string entries without crashing', async () => {
        global.fetch = fakeFetch({
            'vendor/lib/math.js': '/* mathjs */',
        });
        const mgr = new ExtensionManager(async () => undefined);
        const out = await mgr._fetchVendorSources({
            id: 'test',
            sandboxVendor: [null, 42, 'math', {}],
        });
        expect(out).toBeDefined();
        expect(Object.keys(out)).toEqual(['math']);
    });

    it('skips vendors whose fetch fails (network error)', async () => {
        const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
        global.fetch = vi.fn(async () => {
            throw new Error('network down');
        });
        const mgr = new ExtensionManager(async () => undefined);
        const out = await mgr._fetchVendorSources({
            id: 'math',
            sandboxVendor: ['math'],
        });
        expect(out).toBeUndefined();
        expect(warnSpy).toHaveBeenCalled();
        warnSpy.mockRestore();
    });

    it('skips vendors whose fetch returns non-ok', async () => {
        const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
        global.fetch = vi.fn(async () => ({ ok: false, status: 500, text: async () => '' }));
        const mgr = new ExtensionManager(async () => undefined);
        const out = await mgr._fetchVendorSources({
            id: 'math',
            sandboxVendor: ['math'],
        });
        expect(out).toBeUndefined();
        expect(warnSpy).toHaveBeenCalled();
        warnSpy.mockRestore();
    });
});
