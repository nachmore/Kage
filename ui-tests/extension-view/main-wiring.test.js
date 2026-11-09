/**
 * `ui/js/extension-view/main.js` only wires real modules into `page.js`.
 * These tests pin that wiring: what the page offers a view, and what it
 * expects from the modules other work packages own (EXTENSION_VIEWS.md
 * WP-V6, "Interfaces the view page uses").
 *
 * The second block checks those modules' behaviour where the page relies
 * on it. It runs once they exist (WP-V3's extension-data-events.js and
 * WP-V4's extension-view-policy.js and view surface), so a merge that
 * changes a signature fails here instead of in a running window.
 */

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { repoRoot } from './fixture.js';

const MAIN = readFileSync(path.join(repoRoot, 'ui/js/extension-view/main.js'), 'utf8');

function importedNames(from) {
    const re = new RegExp(
        `import\\s*\\{([^}]*)\\}\\s*from\\s*'${from.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&')}'`
    );
    const m = re.exec(MAIN);
    return m
        ? m[1]
              .split(',')
              .map((s) => s.trim())
              .filter(Boolean)
              .sort()
        : null;
}

describe('main.js wiring', () => {
    it('installs the link handler like every window', () => {
        expect(MAIN).toMatch(/\binitLinkHandler\(invoke\)/);
    });

    it('takes the policy helpers, the data events and the sandbox from their owners', () => {
        expect(importedNames('../shared/extension-view-policy.js')).toEqual([
            'isSafeTokenValue',
            'normalizeViews',
            'sanitizeViewTitle',
        ]);
        expect(importedNames('../shared/extension-data-events.js')).toEqual([
            'installExtensionDataEvents',
        ]);
        expect(importedNames('../shared/extension-sandbox-host.js')).toEqual(['ExtensionSandbox']);
        expect(importedNames('./page.js')).toEqual(['createViewPage']);
    });

    it('offers views and storage change events, never background agent calls (§9.4)', () => {
        const m = /const OFFERED_HOST_FEATURES = \[([^\]]*)\]/.exec(MAIN);
        expect(m).not.toBeNull();
        const offered = m[1]
            .split(',')
            .map((s) => s.trim().replace(/^'|'$/g, ''))
            .filter(Boolean);
        expect(offered).toEqual(expect.arrayContaining(['views', 'storage.change_events']));
        expect(offered).not.toContain('agent.background');
        expect(offered).not.toContain('date-picker');
    });
});

// Glob imports: an empty result while the module isn't there yet, instead of
// a transform error for the whole file.
const policyModule = Object.values(
    import.meta.glob('../../ui/js/shared/extension-view-policy.js')
)[0];
const dataEventsModule = Object.values(
    import.meta.glob('../../ui/js/shared/extension-data-events.js')
)[0];
const dependenciesMerged = !!policyModule && !!dataEventsModule;

describe.skipIf(!dependenciesMerged)('the modules main.js wires in', () => {
    it('normalizeViews(raw, name, icon, context) returns the view declarations', async () => {
        const { normalizeViews } = await policyModule();
        const decls = normalizeViews(
            [
                { id: 'explorer', title: 'Explorer', module: './explorer.js', css: ['./a.css'] },
                { id: 'Bad Id', module: './x.js' },
                { id: 'board', module: './board.js' },
            ],
            'Mind Map',
            '🧠',
            'mindmap'
        );
        expect(decls.map((d) => d.id)).toEqual(['explorer', 'board']);
        expect(decls[0]).toMatchObject({
            title: 'Explorer',
            module: './explorer.js',
            css: ['./a.css'],
        });
        expect(decls[1].title).toBe('Mind Map');
        expect(decls[1].icon).toBe('🧠');
    });

    it('sanitizeViewTitle(text, max) strips control and bidi characters and caps the length', async () => {
        const { sanitizeViewTitle } = await policyModule();
        expect(sanitizeViewTitle('Bin\u202Ego\u0007', 80)).toBe('Bingo');
        expect(Array.from(sanitizeViewTitle('x'.repeat(100), 60))).toHaveLength(60);
    });

    it('isSafeTokenValue(name, value) accepts colours and font stacks, refuses injection', async () => {
        const { isSafeTokenValue } = await policyModule();
        expect(isSafeTokenValue('kage-bg', '#1a202c')).toBe(true);
        expect(isSafeTokenValue('kage-accent-glow', 'rgba(49, 151, 149, 0.3)')).toBe(true);
        expect(
            isSafeTokenValue(
                'kage-font',
                '-apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif'
            )
        ).toBe(true);
        expect(isSafeTokenValue('kage-bg', 'url(https://example.com/x)')).toBe(false);
        expect(isSafeTokenValue('kage-bg', '#fff; } body { display: none')).toBe(false);
    });

    it('installExtensionDataEvents and the view surface exist', async () => {
        const { installExtensionDataEvents } = await dataEventsModule();
        expect(typeof installExtensionDataEvents).toBe('function');
        const { ExtensionSandbox } = await import('../../ui/js/shared/extension-sandbox-host.js');
        expect(typeof ExtensionSandbox.prototype.notifyStorageChanged).toBe('function');
    });
});
