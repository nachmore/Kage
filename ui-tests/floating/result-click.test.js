/**
 * Tests for click-to-execute on floating unified-search result rows.
 *
 * Before this, unified results (apps, URLs, slash commands like /agent) were
 * keyboard-only — clicking a row did nothing. renderUnifiedResults now accepts
 * an onItemClick callback and wires each row's .onclick to it, passing the
 * result object by value so reused DOM nodes don't fire stale handlers.
 */

import { describe, it, expect, vi } from 'vitest';
import { renderUnifiedResults } from '../../ui/js/floating/search-unified.js';

function makeContainer() {
    const el = document.createElement('div');
    document.body.appendChild(el);
    return el;
}

const RESULTS = [
    { id: 'slash:/agent', type: 'slash', label: '/agent', description: 'Select an agent' },
    { id: 'app:Calc', type: 'app', label: 'Calculator', description: 'app' },
];

describe('renderUnifiedResults click wiring', () => {
    it('invokes onItemClick with the clicked result', async () => {
        const container = makeContainer();
        const onItemClick = vi.fn();
        await renderUnifiedResults(RESULTS, container, [], () => {}, onItemClick);

        const items = container.querySelectorAll('.app-suggestion-item');
        expect(items).toHaveLength(2);

        items[0].click();
        expect(onItemClick).toHaveBeenCalledTimes(1);
        expect(onItemClick).toHaveBeenCalledWith(RESULTS[0]);

        items[1].click();
        expect(onItemClick).toHaveBeenCalledTimes(2);
        expect(onItemClick).toHaveBeenLastCalledWith(RESULTS[1]);
    });

    it('passes the correct result even after a re-render reuses DOM nodes', async () => {
        const container = makeContainer();
        const onItemClick = vi.fn();
        const matches = [];

        await renderUnifiedResults(RESULTS, container, matches, () => {}, onItemClick);
        // Re-render with the same keys (node reuse path) but updated objects.
        const updated = [
            { id: 'slash:/agent', type: 'slash', label: '/agent', description: 'changed' },
            { id: 'app:Calc', type: 'app', label: 'Calculator', description: 'app' },
        ];
        await renderUnifiedResults(updated, container, matches, () => {}, onItemClick);

        container.querySelector('.app-suggestion-item').click();
        // Must fire exactly once (no stacked handler from the first render) and
        // carry the NEW result object, not the stale one.
        expect(onItemClick).toHaveBeenCalledTimes(1);
        expect(onItemClick).toHaveBeenCalledWith(updated[0]);
    });

    it('is a no-op when no onItemClick is provided (keyboard-only mode)', async () => {
        const container = makeContainer();
        await renderUnifiedResults(RESULTS, container, [], () => {});
        const item = container.querySelector('.app-suggestion-item');
        // No handler wired — clicking must not throw.
        expect(() => item.click()).not.toThrow();
        expect(item.onclick).toBeNull();
    });
});
