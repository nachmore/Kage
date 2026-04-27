/**
 * Tests for the sandbox runtime's relative-import rewriter.
 * This is the pure regex-based text transform that lets
 * `import from './cache.js'` survive the blob-URL execution model.
 */

import { describe, it, expect } from 'vitest';
import { rewriteRelativeImportsWith } from '../../js/extension-sandbox/runtime.js';

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

    it('rewrites ../ parent-directory specifiers', () => {
        const src = "import { X } from '../util.js';";
        const out = rewriteRelativeImportsWith(src, mapOf({
            '../util.js': 'blob:null/up',
        }));
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
        // This is a known limitation of the regex approach — but we
        // should at least verify the happy case: a string literal that
        // doesn't start with `import` isn't touched.
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
});
