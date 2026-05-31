/**
 * Tests for the host-side shared-module discovery that lets extensions
 * use `import from './sibling.js'` inside sandboxed provider files.
 *
 * The sandbox runtime's rewrite step runs inside the iframe and needs
 * the real DOM for Blob/URL — that's covered by manual QA. Here we
 * cover the host-side scanning that populates the sharedSources bag.
 */

import { describe, it, expect, vi } from 'vitest';
import {
    ExtensionManager,
    fetchSharedSourcesViaInvoke,
} from '../../ui/js/shared/extension-manager.js';
import { buildSandboxedSettingsModule } from '../../ui/js/settings/manager.js';

/** Build a manager with stubbed fetch methods so we can drive the scanner. */
function makeManager(fs) {
    const mgr = new ExtensionManager(async () => undefined);
    mgr._fetchText = async (_id, rel) => {
        const key = rel.replace(/^\.\//, '');
        return fs[key] ?? null;
    };
    return mgr;
}

describe('_fetchSharedSources', () => {
    it('returns undefined when no relative imports are referenced', async () => {
        const mgr = makeManager({
            'search.js': 'export default class { foo() { return 1 } }',
        });
        const sources = { searchProvider: 'export default class { foo() { return 1 } }' };
        const out = await mgr._fetchSharedSources(sources, 'test-ext');
        expect(out).toBeUndefined();
    });

    it('discovers one-level relative imports and fetches their source', async () => {
        const mgr = makeManager({
            'cache.js': 'export const C = 1;',
        });
        const sources = {
            searchProvider: `
                import { C } from './cache.js';
                export default class {}
            `,
        };
        const out = await mgr._fetchSharedSources(sources, 'test-ext');
        expect(out).toBeDefined();
        expect(out['./cache.js']).toContain('export const C = 1');
    });

    it('follows transitive imports (shared module importing another shared module)', async () => {
        const mgr = makeManager({
            'a.js': "import { B } from './b.js'; export const A = B + 1;",
            'b.js': 'export const B = 42;',
        });
        const sources = {
            searchProvider: "import { A } from './a.js'; export default class {}",
        };
        const out = await mgr._fetchSharedSources(sources, 'test-ext');
        expect(Object.keys(out).sort()).toEqual(['./a.js', './b.js']);
    });

    it('discovers imports from multiple entry points and dedupes', async () => {
        const mgr = makeManager({
            'cache.js': 'export const C = 1;',
        });
        const sources = {
            searchProvider: "import { C } from './cache.js'; export default class {}",
            widgets: {
                'w1': "import { C } from './cache.js'; export default class {}",
                'w2': "import { C } from './cache.js'; export default class {}",
            },
        };
        const out = await mgr._fetchSharedSources(sources, 'test-ext');
        expect(Object.keys(out)).toEqual(['./cache.js']);
    });

    it('ignores imports from dependencies that cannot be fetched', async () => {
        const mgr = makeManager({
            'real.js': 'export const X = 1;',
        });
        const sources = {
            searchProvider: `
                import { X } from './real.js';
                import { Y } from './missing.js';
                export default class {}
            `,
        };
        const out = await mgr._fetchSharedSources(sources, 'test-ext');
        // The missing file simply isn't in the output; the sandbox runtime
        // logs + skips when the import fails to resolve at blob-build time.
        expect(out['./real.js']).toBeDefined();
        expect(out['./missing.js']).toBeUndefined();
    });

    it('matches both `import X from "./path"` and bare `import "./path"` forms', async () => {
        const mgr = makeManager({
            'a.js': 'export const A = 1;',
            'b.js': 'console.log("side effect");',
        });
        const sources = {
            searchProvider: `
                import { A } from './a.js';
                import './b.js';
                export default class {}
            `,
        };
        const out = await mgr._fetchSharedSources(sources, 'test-ext');
        expect(out['./a.js']).toBeDefined();
        expect(out['./b.js']).toBeDefined();
    });

    it('does not try to resolve non-relative specifiers (bare imports)', async () => {
        const mgr = makeManager({});
        const sources = {
            searchProvider: `
                import foo from 'some-bare-module';
                import bar from 'other/bare';
                export default class {}
            `,
        };
        const out = await mgr._fetchSharedSources(sources, 'test-ext');
        expect(out).toBeUndefined();
    });
});

/**
 * `fetchSharedSourcesViaInvoke` is the standalone shape of the
 * scanner — same logic as `_fetchSharedSources` but takes the IPC
 * `invoke` directly, so the settings window (which builds its own
 * sandbox in `buildSandboxedSettingsModule`) can resolve relative
 * imports without going through the full ExtensionManager class.
 *
 * The Spotify install bug landed because this code path didn't
 * exist — the settings sandbox had no way to ship sibling modules
 * to the iframe, so `import * as auth from './auth.js'` threw
 * "Failed to resolve module specifier" and the settings sidebar
 * never gained a Spotify entry.
 */
describe('fetchSharedSourcesViaInvoke (settings-side scanner)', () => {
    /** Build an `invoke` stub that serves a virtual filesystem. */
    function makeInvoke(fs) {
        return async (cmd, args) => {
            if (cmd !== 'read_extension_file') return null;
            const key = args.filePath;
            return fs[key] ?? null;
        };
    }

    it('walks relative imports the same way the manager class does', async () => {
        const invoke = makeInvoke({
            'auth.js': "import { B } from './shared.js'; export const auth = B;",
            'shared.js': 'export const B = 7;',
        });
        const out = await fetchSharedSourcesViaInvoke(invoke, 'spotify', {
            settingsProvider: "import * as auth from './auth.js'; export default class {}",
        });
        expect(out).toBeDefined();
        expect(Object.keys(out).sort()).toEqual(['./auth.js', './shared.js']);
    });

    it('returns undefined when the entry source has no relative imports', async () => {
        const invoke = vi.fn();
        const out = await fetchSharedSourcesViaInvoke(invoke, 'plain', {
            settingsProvider: 'export default class {}',
        });
        expect(out).toBeUndefined();
        // Defence in depth — the scanner should never invoke for a
        // module that doesn't ask for sibling resolution.
        expect(invoke).not.toHaveBeenCalled();
    });

    it('skips siblings whose IPC fetch fails — the runtime warns at load', async () => {
        // Spotify's settings flow used to hit this when read_extension_file
        // was throwing for a missing sibling. We collapse the throw to a
        // logged-and-skipped outcome so other modules still resolve.
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const invoke = async (cmd, args) => {
            if (cmd !== 'read_extension_file') return null;
            if (args.filePath === 'broken.js') throw new Error('forbidden');
            return 'export const X = 1;';
        };
        const out = await fetchSharedSourcesViaInvoke(invoke, 'ext', {
            settingsProvider: "import './broken.js'; import { X } from './ok.js'; export default class {}",
        });
        expect(out['./ok.js']).toBeDefined();
        expect(out['./broken.js']).toBeUndefined();
        warn.mockRestore();
    });
});

/**
 * `buildSandboxedSettingsModule` is the boot path the settings
 * window uses for every extension that contributes a settingsProvider.
 * The Spotify regression here was a missing argument: the function
 * was building the sandbox spec without `sharedSources`, so the
 * iframe runtime had nothing to register for sibling modules. The
 * fix is one line, but the failure mode is silent — the test below
 * keeps it that way.
 */
describe('buildSandboxedSettingsModule', () => {
    /**
     * Build a fake sandbox + pool that record what spec was passed.
     * `getSettings` returns a minimal valid schema; `hasSettings`
     * reports true; the rest of the surface is never reached.
     */
    function makePoolStub() {
        const calls = [];
        const fakeSandbox = {
            hasSettings: true,
            call: vi.fn().mockResolvedValue({ sections: [] }),
        };
        const pool = {
            load: vi.fn(async (spec) => {
                calls.push(spec);
                return fakeSandbox;
            }),
            unload: vi.fn(),
        };
        return { pool, calls, fakeSandbox };
    }

    /**
     * `invoke` stub that:
     *   - serves `read_extension_file` from a virtual fs
     *   - returns a minimal `read_extension_locale` payload so the
     *     manifest token resolver is happy
     */
    function makeInvoke(fs, locale = {}) {
        return vi.fn(async (cmd, args) => {
            if (cmd === 'read_extension_file') {
                const key = args.filePath;
                return fs[key] ?? null;
            }
            if (cmd === 'read_extension_locale') {
                return locale;
            }
            return null;
        });
    }

    it('plumbs sharedSources through to pool.load when settings.js imports siblings', async () => {
        const { pool, calls } = makePoolStub();
        const invoke = makeInvoke({
            'auth.js': 'export const auth = 1;',
        });
        const settingsSrc =
            "import * as auth from './auth.js'; export default class { initialize() {} validate() { return { valid: true } } save() {} load() {} }";

        await buildSandboxedSettingsModule({
            invoke,
            pool,
            manifest: { id: 'spotify', name: 'Spotify', type: 'extension', version: '0.1.0' },
            capabilities: ['storage', 'urls', 'oauth'],
            settingsProviderSource: settingsSrc,
            currentConfig: {},
        });

        expect(pool.load).toHaveBeenCalled();
        const spec = calls[0];
        expect(
            spec.sharedSources,
            'sharedSources must be in the sandbox spec; without it sibling imports throw "Failed to resolve module specifier"'
        ).toBeDefined();
        expect(spec.sharedSources['./auth.js']).toBe('export const auth = 1;');
    });

    it('omits sharedSources when settings.js has no relative imports', async () => {
        const { pool, calls } = makePoolStub();
        const invoke = makeInvoke({});
        const settingsSrc =
            'export default class { initialize() {} validate() { return { valid: true } } save() {} load() {} }';

        await buildSandboxedSettingsModule({
            invoke,
            pool,
            manifest: { id: 'simple', name: 'Simple', type: 'extension', version: '1.0.0' },
            capabilities: [],
            settingsProviderSource: settingsSrc,
            currentConfig: {},
        });

        const spec = calls[0];
        // Cleaner spec when there's nothing to share — guards against
        // an over-eager scanner shipping empty-bag sharedSources.
        expect(spec.sharedSources).toBeUndefined();
    });
});
