/**
 * Tests for the host-side shared-module discovery that lets extensions
 * use `import from './sibling.js'` inside sandboxed provider files.
 *
 * The sandbox runtime's rewrite step runs inside the iframe and needs
 * the real DOM for Blob/URL — that's covered by manual QA. Here we
 * cover the host-side scanning that populates the sharedSources bag.
 */

import { describe, it, expect } from 'vitest';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';

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
