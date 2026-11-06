/**
 * Tests for the sandbox runtime's relative-import rewriter.
 * This is the source-range transform that lets
 * `import from './cache.js'` survive the blob-URL execution model.
 */

import { describe, it, expect } from 'vitest';
import { rewriteRelativeImportsWith } from '../../ui/js/extension-sandbox/runtime.js';

function mapOf(obj) {
    return new Map(Object.entries(obj));
}

describe('rewriteRelativeImportsWith', () => {
    it('passes source through unchanged when the blob map is empty', () => {
        const src = "import { C } from './cache.js'; export default class {}";
        expect(rewriteRelativeImportsWith(src, new Map())).toBe(src);
    });

    it('rewrites named imports to the mapped blob URL', () => {
        const src = "import { C } from './cache.js';\nexport default class {}";
        const out = rewriteRelativeImportsWith(src, mapOf({
            './cache.js': 'blob:null/abc',
        }));
        expect(out).toContain("from 'blob:null/abc'");
        expect(out).not.toContain("'./cache.js'");
    });

    it('rewrites default imports', () => {
        const src = "import Cache from './cache.js';";
        const out = rewriteRelativeImportsWith(src, mapOf({
            './cache.js': 'blob:null/abc',
        }));
        expect(out).toBe("import Cache from 'blob:null/abc';");
    });

    it('rewrites namespace imports', () => {
        const src = "import * as cache from './cache.js';";
        const out = rewriteRelativeImportsWith(src, mapOf({
            './cache.js': 'blob:null/abc',
        }));
        expect(out).toContain("'blob:null/abc'");
    });

    it('rewrites side-effect (bare) imports', () => {
        const src = "import './sideeffect.js';\nconsole.log(1);";
        const out = rewriteRelativeImportsWith(src, mapOf({
            './sideeffect.js': 'blob:null/xyz',
        }));
        expect(out).toContain("import 'blob:null/xyz'");
    });

    it('rewrites double-quoted specifiers', () => {
        const src = 'import { C } from "./cache.js";';
        const out = rewriteRelativeImportsWith(src, mapOf({
            './cache.js': 'blob:null/abc',
        }));
        expect(out).toContain('from "blob:null/abc"');
    });

    it('rewrites ../ specifiers relative to a nested importer inside the bundle', () => {
        const src = "import { X } from '../util.js';";
        const out = rewriteRelativeImportsWith(src, mapOf({
            './util.js': 'blob:null/up',
        }), './nested/cache.js');
        expect(out).toContain("'blob:null/up'");
    });

    it('leaves unknown relative imports alone', () => {
        const src = "import { X } from './missing.js';";
        const out = rewriteRelativeImportsWith(src, mapOf({
            './cache.js': 'blob:null/abc',
        }));
        expect(out).toBe(src); // no change
    });

    it('leaves bare-module imports alone', () => {
        const src = "import lodash from 'lodash';\nimport { k } from 'kage/lib';";
        const out = rewriteRelativeImportsWith(src, mapOf({
            './cache.js': 'blob:null/abc',
        }));
        expect(out).toBe(src);
    });

    it('rewrites multiple imports in the same file', () => {
        const src = [
            "import { A } from './a.js';",
            "import { B } from './b.js';",
            "import './c.js';",
        ].join('\n');
        const out = rewriteRelativeImportsWith(src, mapOf({
            './a.js': 'blob:null/a',
            './b.js': 'blob:null/b',
            './c.js': 'blob:null/c',
        }));
        expect(out).toContain("'blob:null/a'");
        expect(out).toContain("'blob:null/b'");
        expect(out).toContain("'blob:null/c'");
        expect(out).not.toContain("'./a.js'");
        expect(out).not.toContain("'./b.js'");
        expect(out).not.toContain("'./c.js'");
    });

    it('does not rewrite relative paths in string literals that look like imports', () => {
        const src = [
            "const x = \"from './cache.js' later\";",
            "const y = 'not an import ./cache.js in a string';",
        ].join('\n');
        const out = rewriteRelativeImportsWith(src, mapOf({
            './cache.js': 'blob:null/abc',
        }));
        expect(out).toBe(src);
    });

    it('handles imports spanning multiple lines (named destructuring)', () => {
        const src = `import {
    initCache,
    getEvents,
    invalidate,
} from './cache.js';`;
        const out = rewriteRelativeImportsWith(src, mapOf({
            './cache.js': 'blob:null/abc',
        }));
        expect(out).toContain("'blob:null/abc'");
        expect(out).not.toContain("'./cache.js'");
    });

    it('handles compact imports, comments, string export names, and re-exports', () => {
        const src = [
            'import{ "name" as name }/* comment */from/* gap */"./cache.js";',
            "export { name as renamed } from './cache.js';",
            "export * from './cache.js';",
            "export * as cache from './cache.js';",
            "import/* gap */'./cache.js';",
        ].join('\n');
        const out = rewriteRelativeImportsWith(src, mapOf({ './cache.js': 'blob:null/final' }));
        expect(out.match(/blob:null\/final/g)).toHaveLength(5);
        expect(out).not.toContain('./cache.js');
    });

    it('rewrites quoted dynamic imports and escaped specifiers without evaluating expressions', () => {
        const src = [
            "const a = import(/* gap */'./cache.js');",
            'const b = import("./cache\\u002ejs", { with: { type: "javascript" } });',
            "const c = import('./' + name);",
            'const d = import(`./${name}.js`);',
            'const e = import(`./cache.js`);',
        ].join('\n');
        const out = rewriteRelativeImportsWith(src, mapOf({ './cache.js': 'blob:null/final' }));
        expect(out.match(/blob:null\/final/g)).toHaveLength(2);
        expect(out).toContain("import('./' + name)");
        expect(out).toContain('import(`./${name}.js`)');
        expect(out).toContain('import(`./cache.js`)');
    });

    it('leaves comments, strings, template prose, regexes, and object methods unchanged', () => {
        const src = [
            "// import './cache.js';",
            "/* export * from './cache.js'; */",
            'const text = "import { C } from \'./cache.js\'";',
            'const template = `export * from "./cache.js"`;',
            'const regex = /import\\(".\\/cache.js"\\)/;',
            'const obj = { import(path) { return path; } };',
            'obj.import("./cache.js");',
            'const meta = import.meta.url;',
        ].join('\n');
        expect(rewriteRelativeImportsWith(src, mapOf({ './cache.js': 'blob:null/final' }))).toBe(src);
    });

    it('rejects traversal outside the extension root', () => {
        expect(() => rewriteRelativeImportsWith("import '../other.js'", mapOf({
            './cache.js': 'blob:null/final',
        }))).toThrow('escapes bundle root');
    });
});
