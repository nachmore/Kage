import { afterEach, describe, expect, it, vi } from 'vitest';
import {
    registerSharedModules,
    rewriteRelativeImportsWith,
} from '../../ui/js/extension-sandbox/runtime.js';
import { extensionModuleImports } from '../../ui/js/shared/extension-modules.js';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';

afterEach(() => vi.unstubAllGlobals());

// Inspect the exact text supplied to each blob without requiring a browser's
// module loader. Any link to an intermediate/raw/revoked blob fails this walk.
function captureBlobs() {
    const textByUrl = new Map();
    const revokeObjectURL = vi.fn();
    class TextBlob {
        constructor(parts) { this.text = parts.join(''); }
    }
    vi.stubGlobal('Blob', TextBlob);
    vi.stubGlobal('URL', {
        createObjectURL: vi.fn((blob) => {
            const url = `blob:null/final-${textByUrl.size}`;
            textByUrl.set(url, blob.text);
            return url;
        }),
        revokeObjectURL,
    });
    return { textByUrl, revokeObjectURL };
}

async function buildGraph(files, entry) {
    const mgr = new ExtensionManager(async () => undefined);
    mgr._fetchText = vi.fn(async (_id, path) => files[path] ?? null);
    const shared = await mgr._fetchSharedSources({ searchProvider: entry }, 'test');
    const capture = captureBlobs();
    const map = registerSharedModules(shared, new Map());
    const rewrittenEntry = rewriteRelativeImportsWith(entry, map);
    const visited = new Set();
    const walk = (source) => {
        for (const { specifier } of extensionModuleImports(source)) {
            expect(specifier).toMatch(/^blob:null\/final-/);
            expect([...map.values()]).toContain(specifier);
            expect(capture.textByUrl.has(specifier)).toBe(true);
            if (visited.has(specifier)) continue;
            visited.add(specifier);
            walk(capture.textByUrl.get(specifier));
        }
    };
    walk(rewrittenEntry);
    expect(visited.size).toBe(Object.keys(files).length);
    expect(capture.textByUrl.size).toBe(map.size);
    expect(capture.revokeObjectURL).not.toHaveBeenCalled();
    return { ...capture, map, shared };
}

describe('shared module graph', () => {
    it('rewrites a three-level graph leaf-first, independent of discovery order', async () => {
        const { map } = await buildGraph({
            './client.js': "import { symbols } from './symbols.js'; export const client = symbols;",
            './symbols.js': "import { defaults } from './defaults.js'; export const symbols = defaults;",
            './defaults.js': 'export const defaults = 42;',
        }, "import { client } from './client.js'; export default class {}");
        expect([...map.keys()]).toEqual(['./defaults.js', './symbols.js', './client.js']);
    });

    it('rewrites a diamond with a single final blob for the shared dependency', async () => {
        const { map, textByUrl } = await buildGraph({
            './client.js': "import './symbols.js'; import './provider.js';",
            './symbols.js': "import './defaults.js'; export const symbol = 'x';",
            './provider.js': "import './symbols.js'; export const provider = 'y';",
            './defaults.js': 'export const defaults = 1;',
        }, "import './client.js'; export default class {}");
        expect(textByUrl.get(map.get('./provider.js'))).toContain(map.get('./symbols.js'));
        expect(textByUrl.get(map.get('./client.js'))).toContain(map.get('./symbols.js'));
    });

    it('discovers and rewrites re-exports transitively, including namespace re-exports', async () => {
        await buildGraph({
            './index.js': "export { value } from './symbols.js'; export * as defaults from './defaults.js';",
            './symbols.js': "export * from './defaults.js';",
            './defaults.js': 'export const value = 42;',
        }, "import { value } from './index.js'; export default class {}");
    });

    it('resolves subdirectories relative to the importing file, including parent paths within the root', async () => {
        await buildGraph({
            './lib/client.js': "export * from './symbols.js';",
            './lib/symbols.js': "export * from '../defaults.js';",
            './defaults.js': 'export const value = 1;',
        }, "import './lib/client.js'; export default class {}");
    });

    it('builds final dependencies for quoted dynamic imports made after initialization', async () => {
        await buildGraph({
            './client.js': "export const load = () => import('./symbols.js');",
            './symbols.js': "export { value } from './defaults.js';",
            './defaults.js': 'export const value = 42;',
        }, "import './client.js'; export default class {}");
    });

    it('detects cycles before allocating or revoking any blob, with the full cycle path', async () => {
        const mgr = new ExtensionManager(async () => undefined);
        const files = {
            './a.js': "export * from './b.js';",
            './b.js': "import './a.js';",
        };
        mgr._fetchText = vi.fn(async (_id, path) => files[path]);
        const shared = await mgr._fetchSharedSources({ searchProvider: "import './a.js';" }, 'test');
        const capture = captureBlobs();
        expect(() => registerSharedModules(shared, new Map())).toThrow(
            'Extension module import cycle: ./a.js -> ./b.js -> ./a.js'
        );
        expect(URL.createObjectURL).not.toHaveBeenCalled();
        expect(capture.revokeObjectURL).not.toHaveBeenCalled();
        expect(mgr._fetchText).toHaveBeenCalledTimes(2);
    });

    it('reports self-import cycles clearly', () => {
        captureBlobs();
        expect(() => registerSharedModules({ './a.js': "import './a.js';" }, new Map()))
            .toThrow('./a.js -> ./a.js');
    });
});
