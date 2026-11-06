import { afterEach, describe, expect, it, vi } from 'vitest';
import {
    registerSharedModules,
    rewriteRelativeImportsWith,
    importFromSource,
} from '../../ui/js/extension-sandbox/runtime.js';
import { collectExtensionModules, extensionModuleImports } from '../../ui/js/shared/extension-modules.js';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';

afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
});

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

    it('isolates cycle members and their dependents while building healthy modules', async () => {
        const mgr = new ExtensionManager(async () => undefined);
        const files = {
            './a.js': "export * from './b.js';",
            './b.js': "import './a.js';",
            './consumer.js': "import './a.js';",
            './good.js': 'export const good = 1;',
        };
        mgr._fetchText = vi.fn(async (_id, path) => files[path]);
        const shared = await mgr._fetchSharedSources({
            searchProvider: "import './a.js'; import './consumer.js';",
            toolProvider: "import './good.js';",
        }, 'test');
        const capture = captureBlobs();
        const failures = new Map();
        const blobs = registerSharedModules(shared, new Map(), failures);
        expect([...blobs.keys()]).toEqual(['./good.js']);
        expect([...failures.keys()].sort()).toEqual(['./a.js', './b.js', './consumer.js']);
        await expect(importFromSource(
            "import './consumer.js'; export default class {}",
            'searchProvider', './search.js', blobs, failures
        )).rejects.toThrow("searchProvider './search.js': Extension module './consumer.js': Extension module import cycle: ./a.js -> ./b.js -> ./a.js");
        expect(URL.createObjectURL).toHaveBeenCalledTimes(1);
        expect(capture.revokeObjectURL).not.toHaveBeenCalled();
        expect(mgr._fetchText).toHaveBeenCalledTimes(4);
    });

    it('reports self-import cycles clearly', () => {
        captureBlobs();
        const failures = new Map();
        const blobs = registerSharedModules({ './a.js': "import './a.js';" }, new Map(), failures);
        expect(blobs.size).toBe(0);
        expect(failures.get('./a.js').message).toContain('./a.js -> ./a.js');
    });

    it.each([
        ['lexer error', 'export const broken = "unterminated;', 'Parse error'],
        ['rejected specifier', "import '../../escape.js';", 'Extension module path escapes bundle root'],
    ])('isolates a shared-file %s and names it in the dependent provider error', async (_case, source, error) => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const files = { './bad.js': source, './good.js': 'export const good = 1;' };
        const shared = await collectExtensionModules({
            searchProvider: "import './bad.js';",
            toolProvider: "import './good.js';",
        }, async (path) => files[path], { searchProvider: './search.js', toolProvider: './tools.js' });
        expect(warn).toHaveBeenCalledWith(
            "Failed to scan extension module './bad.js':", expect.any(Error)
        );
        captureBlobs();
        const failures = new Map();
        const blobs = registerSharedModules(shared, new Map(), failures);
        expect([...blobs.keys()]).toEqual(['./good.js']);
        await expect(importFromSource(
            "import './bad.js'; export default class {}",
            'searchProvider', './search.js', blobs, failures
        )).rejects.toThrow(`searchProvider './search.js': Extension module './bad.js': ${error}`);
    });

    it('continues discovering healthy providers when another entry source is malformed', async () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const shared = await collectExtensionModules({
            searchProvider: 'export default class { label() { return "unterminated; } }',
            toolProvider: "import './good.js'; export default class {}",
        }, async () => 'export const good = 1;', {
            searchProvider: './providers/search.js', toolProvider: './tools.js',
        });
        expect(Object.keys(shared)).toEqual(['./good.js']);
        expect(warn).toHaveBeenCalledWith(
            "Failed to scan extension module './providers/search.js':", expect.any(Error)
        );
        await expect(importFromSource(
            'export default class { label() { return "unterminated; } }',
            'searchProvider', './providers/search.js', new Map(), new Map()
        )).rejects.toThrow("searchProvider './providers/search.js': Parse error");
    });

    it('skips all imports of a rejected source before IPC and continues other sources', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const fetch = vi.fn(async () => 'export const good = 1;');
        const shared = await collectExtensionModules({
            searchProvider: "import './skipped.js'; import '../../escape.js';",
            toolProvider: "import './good.js';",
        }, fetch, { searchProvider: './search.js', toolProvider: './tools.js' });
        expect(Object.keys(shared)).toEqual(['./good.js']);
        expect(fetch).toHaveBeenCalledExactlyOnceWith('./good.js');
    });

    it('names missing shared files without preventing independent modules from loading', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const shared = await collectExtensionModules({
            searchProvider: "import './missing.js';",
            toolProvider: "import './good.js';",
        }, async (path) => {
            if (path === './missing.js') throw new Error('not found');
            return 'export const good = 1;';
        });
        captureBlobs();
        const failures = new Map();
        const blobs = registerSharedModules(shared, new Map(), failures);
        expect([...blobs.keys()]).toEqual(['./good.js']);
        await expect(importFromSource(
            "import './missing.js';", 'searchProvider', './search.js', blobs, failures
        )).rejects.toThrow("Missing extension module './missing.js' imported by './search.js'");
    });
});
