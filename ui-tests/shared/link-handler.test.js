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

describe('neutralizeLinks', () => {
    function container(html) {
        const div = document.createElement('div');
        div.innerHTML = html;
        return div;
    }

    it('moves external hrefs to data-href and blanks href (kills native nav)', async () => {
        const { neutralizeLinks } = await loadFresh();
        const root = container('<a href="https://example.com/x">link</a>');
        neutralizeLinks(root);
        const a = root.querySelector('a');
        // href="#" means WebView2 has nothing to navigate to; the URL lives on
        // data-href for the click handler.
        expect(a.getAttribute('href')).toBe('#');
        expect(a.getAttribute('data-href')).toBe('https://example.com/x');
    });

    it('neutralizes mailto: and kage: too', async () => {
        const { neutralizeLinks } = await loadFresh();
        const root = container(
            '<a href="mailto:a@b.com">m</a><a href="kage:settings">s</a>'
        );
        neutralizeLinks(root);
        const [m, s] = root.querySelectorAll('a');
        expect(m.getAttribute('data-href')).toBe('mailto:a@b.com');
        expect(m.getAttribute('href')).toBe('#');
        expect(s.getAttribute('data-href')).toBe('kage:settings');
        expect(s.getAttribute('href')).toBe('#');
    });

    it('leaves in-page (#anchor) and unknown-scheme links alone', async () => {
        const { neutralizeLinks } = await loadFresh();
        const root = container('<a href="#section">jump</a><a href="ftp://h/f">ftp</a>');
        neutralizeLinks(root);
        const [jump, ftp] = root.querySelectorAll('a');
        expect(jump.getAttribute('href')).toBe('#section');
        expect(jump.hasAttribute('data-href')).toBe(false);
        expect(ftp.getAttribute('href')).toBe('ftp://h/f');
        expect(ftp.hasAttribute('data-href')).toBe(false);
    });

    it('is idempotent — a second pass does not re-wrap', async () => {
        const { neutralizeLinks } = await loadFresh();
        const root = container('<a href="https://example.com/">l</a>');
        neutralizeLinks(root);
        neutralizeLinks(root);
        const a = root.querySelector('a');
        expect(a.getAttribute('data-href')).toBe('https://example.com/');
        expect(a.getAttribute('href')).toBe('#');
    });

    it('a click on a neutralized link opens exactly once via data-href', async () => {
        const { initLinkHandler, neutralizeLinks } = await loadFresh();
        const invoke = vi.fn(() => Promise.resolve());
        initLinkHandler(invoke);

        const a = document.createElement('a');
        a.setAttribute('href', 'https://example.com/deep');
        document.body.appendChild(a);
        neutralizeLinks(document.body);
        a.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
        a.remove();

        expect(invoke).toHaveBeenCalledTimes(1);
        expect(invoke).toHaveBeenCalledWith('open_url', { url: 'https://example.com/deep' });
    });
});
