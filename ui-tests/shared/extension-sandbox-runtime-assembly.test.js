/**
 * The sandbox host inlines the runtime into a srcdoc iframe as a CLASSIC
 * script (null-origin iframes can't load ES modules from the app origin).
 * The runtime sources are authored as ES modules for unit-testability, so
 * the host strips module syntax and concatenates them in dependency order.
 *
 * This test assembles the srcdoc payload exactly the way the host does and
 * asserts it parses as a classic script. Regression guard for the bug where
 * runtime.js grew `import` statements that the old single-file fetch +
 * single-export strip left in place — every sandbox iframe then died on
 * `SyntaxError: Cannot use import statement outside a module` at startup.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
    RUNTIME_SOURCE_PATHS,
    stripModuleSyntax,
} from '../../ui/js/shared/extension-sandbox-host.js';

const UI_ROOT = resolve(__dirname, '../../ui');

function assembleRuntime() {
    return RUNTIME_SOURCE_PATHS.map((p) =>
        stripModuleSyntax(readFileSync(resolve(UI_ROOT, p), 'utf8'))
    ).join('\n;\n');
}

describe('sandbox runtime srcdoc assembly', () => {
    it('leaves no module syntax behind', () => {
        const joined = assembleRuntime();
        expect(joined).not.toMatch(/^import\s/m);
        expect(joined).not.toMatch(/^export\s/m);
    });

    it('parses as a classic (non-module) script', () => {
        // new Function() compiles in a non-module context — exactly the
        // constraint the srcdoc <script> tag imposes.
        expect(() => new Function(assembleRuntime())).not.toThrow();
    });

    it('every listed source file exists', () => {
        for (const p of RUNTIME_SOURCE_PATHS) {
            expect(() => readFileSync(resolve(UI_ROOT, p), 'utf8'), p).not.toThrow();
        }
    });

    it('assembles with the view surface (runtime/view-api.js) before runtime.js', () => {
        const view = RUNTIME_SOURCE_PATHS.indexOf('js/extension-sandbox/runtime/view-api.js');
        const runtime = RUNTIME_SOURCE_PATHS.indexOf('js/extension-sandbox/runtime.js');
        expect(view).toBeGreaterThanOrEqual(0);
        expect(view).toBeLessThan(runtime);
        const joined = assembleRuntime();
        expect(joined).toContain('function createViewSurface(');
        expect(joined).toContain('function createStorageEvents(');
    });

    it('every import of runtime.js is listed before it', () => {
        const runtime = readFileSync(resolve(UI_ROOT, 'js/extension-sandbox/runtime.js'), 'utf8');
        const imports = [...runtime.matchAll(/^import\s[^\n]*from '([^']+)';$/gm)].map((m) =>
            resolve(UI_ROOT, 'js/extension-sandbox', m[1])
        );
        const listed = RUNTIME_SOURCE_PATHS.map((p) => resolve(UI_ROOT, p));
        const at = listed.indexOf(resolve(UI_ROOT, 'js/extension-sandbox/runtime.js'));
        for (const file of imports) {
            expect(listed.indexOf(file), file).toBeGreaterThanOrEqual(0);
            expect(listed.indexOf(file), file).toBeLessThan(at);
        }
    });

    it('top-level names are unique across the parts', () => {
        const names = [];
        for (const p of RUNTIME_SOURCE_PATHS) {
            const text = stripModuleSyntax(readFileSync(resolve(UI_ROOT, p), 'utf8'));
            for (const m of text.matchAll(/^(?:async\s+)?(?:function|const|let|var|class)\s+([A-Za-z_$][\w$]*)/gm)) {
                names.push(m[1]);
            }
        }
        const dupes = names.filter((n, i) => names.indexOf(n) !== i);
        expect(dupes).toEqual([]);
    });
});
