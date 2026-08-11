/**
 * Tests for the global link-click interceptor.
 *
 * The key invariant (the fix for the "links open twice" bug): the delegated
 * click listener is installed on `document` exactly ONCE per window, even if
 * initLinkHandler is called more than once. A second listener would make every
 * link open twice — stopPropagation only stops node-to-node bubbling, not two
 * sibling listeners on the same node.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';

async function loadFresh() {
    vi.resetModules();
    return await import('../../ui/js/shared/link-handler.js');
}

function clickAnchor(href) {
    const a = document.createElement('a');
    a.setAttribute('href', href);
    document.body.appendChild(a);
    // Bubbling click so the delegated document listener sees it.
    a.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    a.remove();
}

describe('link-handler', () => {
    beforeEach(() => {
        document.body.innerHTML = '';
    });

    it('opens an external URL exactly once per click', async () => {
        const { initLinkHandler } = await loadFresh();
        const invoke = vi.fn(() => Promise.resolve());
        initLinkHandler(invoke);

        clickAnchor('https://example.com/');

        expect(invoke).toHaveBeenCalledTimes(1);
        expect(invoke).toHaveBeenCalledWith('open_url', { url: 'https://example.com/' });
    });

    it('does NOT double-open when initLinkHandler is called twice', async () => {
        const { initLinkHandler } = await loadFresh();
        const invoke = vi.fn(() => Promise.resolve());
        // Simulate a double init (double module eval / re-init / refactor).
        initLinkHandler(invoke);
        initLinkHandler(invoke);

        clickAnchor('https://example.com/');

        // Still exactly one open_url — the second init reused the listener.
        expect(invoke).toHaveBeenCalledTimes(1);
    });

    it('a second init refreshes the invoke reference', async () => {
        const { initLinkHandler } = await loadFresh();
        const first = vi.fn(() => Promise.resolve());
        const second = vi.fn(() => Promise.resolve());
        initLinkHandler(first);
        initLinkHandler(second);

        clickAnchor('https://example.com/');

        // The live listener closed over the module-level _invoke, which the
        // second init updated — so the newest invoke is used, exactly once.
        expect(second).toHaveBeenCalledTimes(1);
        expect(first).not.toHaveBeenCalled();
    });

    it('routes mailto: links through open_url once', async () => {
        const { initLinkHandler } = await loadFresh();
        const invoke = vi.fn(() => Promise.resolve());
        initLinkHandler(invoke);

        clickAnchor('mailto:someone@example.com');

        expect(invoke).toHaveBeenCalledTimes(1);
        expect(invoke).toHaveBeenCalledWith('open_url', { url: 'mailto:someone@example.com' });
    });

    it('ignores anchors with no href or href="#"', async () => {
        const { initLinkHandler } = await loadFresh();
        const invoke = vi.fn(() => Promise.resolve());
        initLinkHandler(invoke);

        clickAnchor('#');
        const bare = document.createElement('a');
        document.body.appendChild(bare);
        bare.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
        bare.remove();

        expect(invoke).not.toHaveBeenCalled();
    });
});
