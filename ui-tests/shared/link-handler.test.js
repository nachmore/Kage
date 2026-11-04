/**
 * Tests for the global link-click interceptor.
 *
 * The key invariant (the fix for the "links open twice" bug): the delegated
 * click listener is installed on `document` exactly ONCE per window, even if
 * initLinkHandler is called more than once. A second listener would make every
 * link open twice — stopPropagation only stops node-to-node bubbling, not two
 * sibling listeners on the same node.
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

// Each test gets a fresh module instance; the previous one's document
// listeners must go, or its handler claims every click first.
let _loaded = null;

async function loadFresh() {
    _loaded?.disposeLinkHandler();
    vi.resetModules();
    _loaded = await import('../../ui/js/shared/link-handler.js');
    return _loaded;
}

afterEach(() => {
    _loaded?.disposeLinkHandler();
    _loaded = null;
});

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

    it('cancels activation of SVG anchors whose only URL is xlink:href', async () => {
        // Graphviz emits <a xlink:href="javascript:…">; getAttribute('href')
        // misses the namespaced attribute, which used to skip preventDefault
        // and let the javascript: URL run.
        const { initLinkHandler } = await loadFresh();
        const invoke = vi.fn(() => Promise.resolve());
        initLinkHandler(invoke);

        const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
        const a = document.createElementNS('http://www.w3.org/2000/svg', 'a');
        a.setAttributeNS('http://www.w3.org/1999/xlink', 'xlink:href', 'javascript:alert(1)');
        svg.appendChild(a);
        document.body.appendChild(svg);
        const ev = new MouseEvent('click', { bubbles: true, cancelable: true });
        a.dispatchEvent(ev);
        svg.remove();

        expect(ev.defaultPrevented).toBe(true);
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

    it('strips target=_blank so "#" cannot open a native new window', async () => {
        // Regression: the sanitizer forces target=_blank on http(s) links; a
        // neutralized anchor that kept it opened http://tauri.localhost/# in
        // the browser next to the real URL.
        const { neutralizeLinks } = await loadFresh();
        const root = container(
            '<a href="https://example.com/x" target="_blank" rel="noopener noreferrer">link</a>'
        );
        neutralizeLinks(root);
        const a = root.querySelector('a');
        expect(a.hasAttribute('target')).toBe(false);
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

describe('classifyLinkHref', () => {
    it('sorts hrefs into kage / external / none', async () => {
        const { classifyLinkHref } = await loadFresh();
        expect(classifyLinkHref('https://developer.spotify.com/dashboard')).toBe('external');
        expect(classifyLinkHref('http://example.com/')).toBe('external');
        expect(classifyLinkHref('mailto:a@b.com')).toBe('external');
        // The engine's URL parser is case-insensitive and trims; ours must
        // be too, or the click is cancelled but never opened.
        expect(classifyLinkHref('HTTPS://EXAMPLE.COM/')).toBe('external');
        expect(classifyLinkHref('  https://example.com/')).toBe('external');
        expect(classifyLinkHref('kage:store')).toBe('kage');
        expect(classifyLinkHref('KAGE:settings/updates')).toBe('kage');

        for (const href of [
            '',
            null,
            undefined,
            '#',
            '#section',
            'javascript:alert(1)',
            ' JavaScript:alert(1)',
            'file:///etc/passwd',
            'data:text/html,<script>alert(1)</script>',
            'blob:tauri://localhost/x',
            'spotify:track:1',
            'vscode://file/x',
            'tel:+15555550100',
            'ftp://h/f',
            '/relative/path',
            'settings.html',
        ]) {
            expect(classifyLinkHref(href), String(href)).toBe('none');
        }
    });
});

describe('link-handler — every link in every window', () => {
    function makeAnchor(attrs, parent = document.body) {
        const a = document.createElement('a');
        for (const [k, v] of Object.entries(attrs)) a.setAttribute(k, v);
        a.textContent = 'link';
        parent.appendChild(a);
        return a;
    }

    function click(el) {
        const ev = new MouseEvent('click', { bubbles: true, cancelable: true });
        el.dispatchEvent(ev);
        return ev;
    }

    beforeEach(() => {
        document.body.innerHTML = '';
    });

    it('a plain extension setup link (no target) opens in the browser, never navigates the window', async () => {
        // The reported bug: Spotify's settings info block renders
        // <a href="https://developer.spotify.com/dashboard"> with no target,
        // and the Settings window had no handler, so the click navigated the
        // Settings webview itself.
        const { initLinkHandler } = await loadFresh();
        const invoke = vi.fn(() => Promise.resolve());
        initLinkHandler(invoke);

        const info = document.createElement('div');
        info.className = 'setting-info-body';
        info.innerHTML =
            'Create one at <a href="https://developer.spotify.com/dashboard">developer.spotify.com/dashboard</a>.';
        document.body.appendChild(info);
        const ev = click(info.querySelector('a'));

        expect(ev.defaultPrevented).toBe(true);
        expect(invoke).toHaveBeenCalledTimes(1);
        expect(invoke).toHaveBeenCalledWith('open_url', {
            url: 'https://developer.spotify.com/dashboard',
        });
    });

    it("target=_blank links open once — tauri-plugin-shell's injected opener never fires", async () => {
        const { initLinkHandler } = await loadFresh();
        const invoke = vi.fn(() => Promise.resolve());
        initLinkHandler(invoke);

        // Same predicate as tauri-plugin-shell's init-iife.js: a body click
        // listener that opens target=_blank http(s)/mailto/tel links itself.
        const shellOpen = vi.fn();
        const shellListener = (e) => {
            const a = e.target.closest('a');
            if (
                a &&
                a.href !== '' &&
                ['http://', 'https://', 'mailto:', 'tel:'].some((p) => a.href.startsWith(p)) &&
                a.target === '_blank'
            ) {
                shellOpen(a.href);
                e.preventDefault();
            }
        };
        document.body.addEventListener('click', shellListener);
        try {
            const a = makeAnchor({ href: 'https://ollama.com/download', target: '_blank' });
            click(a);
            expect(shellOpen).not.toHaveBeenCalled();
            expect(invoke).toHaveBeenCalledTimes(1);
            expect(invoke).toHaveBeenCalledWith('open_url', { url: 'https://ollama.com/download' });
        } finally {
            document.body.removeEventListener('click', shellListener);
        }
    });

    it('a click another handler already claimed (preventDefault) is not opened again', async () => {
        // e.g. an element's own click listener that calls open_url itself.
        const { initLinkHandler } = await loadFresh();
        const invoke = vi.fn(() => Promise.resolve());
        initLinkHandler(invoke);

        const a = makeAnchor({ href: 'https://example.com/' });
        const own = vi.fn((e) => e.preventDefault());
        a.addEventListener('click', own);
        click(a);

        expect(own).toHaveBeenCalledTimes(1);
        expect(invoke).not.toHaveBeenCalled();
    });

    it('dangerous and non-web schemes are cancelled, never opened', async () => {
        const { initLinkHandler } = await loadFresh();
        const invoke = vi.fn(() => Promise.resolve());
        initLinkHandler(invoke);

        for (const href of [
            'javascript:alert(1)',
            'file:///etc/passwd',
            'spotify:track:1',
            'tel:+15555550100',
            'settings.html',
        ]) {
            const ev = click(makeAnchor({ href }));
            expect(ev.defaultPrevented, href).toBe(true);
        }
        expect(invoke).not.toHaveBeenCalled();
    });

    it('opens mixed-case http links instead of silently dropping them', async () => {
        const { initLinkHandler } = await loadFresh();
        const invoke = vi.fn(() => Promise.resolve());
        initLinkHandler(invoke);

        click(makeAnchor({ href: 'HTTPS://Example.com/Docs' }));

        expect(invoke).toHaveBeenCalledWith('open_url', { url: 'HTTPS://Example.com/Docs' });
    });

    it('middle-click opens the link; right-click is left alone', async () => {
        const { initLinkHandler } = await loadFresh();
        const invoke = vi.fn(() => Promise.resolve());
        initLinkHandler(invoke);

        const a = makeAnchor({ href: 'https://example.com/mid' });
        const right = new MouseEvent('auxclick', { bubbles: true, cancelable: true, button: 2 });
        a.dispatchEvent(right);
        expect(right.defaultPrevented).toBe(false);
        expect(invoke).not.toHaveBeenCalled();

        const middle = new MouseEvent('auxclick', { bubbles: true, cancelable: true, button: 1 });
        a.dispatchEvent(middle);
        expect(middle.defaultPrevented).toBe(true);
        expect(invoke).toHaveBeenCalledTimes(1);
        expect(invoke).toHaveBeenCalledWith('open_url', { url: 'https://example.com/mid' });
    });

    it('kage: links route internally instead of leaving the app', async () => {
        const { initLinkHandler } = await loadFresh();
        const invoke = vi.fn(() => Promise.resolve());
        initLinkHandler(invoke);

        click(makeAnchor({ href: 'kage:settings/updates' }));
        await Promise.resolve();

        expect(invoke).toHaveBeenCalledWith('open_settings_window', { section: 'updates' });
        expect(invoke).not.toHaveBeenCalledWith('open_url', expect.anything());
    });
});

describe('every Kage window installs the link handler', () => {
    // A window that forgets initLinkHandler navigates itself to any plain
    // link the user clicks (the Settings window did). The Rust navigation
    // guard cancels that, but only hands the URL to the browser on Windows,
    // so on macOS/Linux this handler is what makes the link work at all.
    const ENTRY_POINTS = {
        main: 'ui/js/chat/main.js',
        floating: 'ui/js/floating/main.js',
        'inline-assist': 'ui/js/inline-assist.js',
        settings: 'ui/js/settings/main.js',
        store: 'ui/js/store.js',
        welcome: 'ui/welcome.html',
        'context-menu': 'ui/context-menu.html',
    };

    for (const [label, file] of Object.entries(ENTRY_POINTS)) {
        it(`${label} (${file})`, () => {
            const src = readFileSync(path.join(repoRoot, file), 'utf8');
            expect(src).toMatch(
                /import\s*\{[^}]*\binitLinkHandler\b[^}]*\}\s*from\s*'[^']*link-handler\.js'/
            );
            expect(src).toMatch(/\binitLinkHandler\(invoke\)/);
        });
    }

    it('covers every window in the capability file', () => {
        // New window → add its entry point above (and call initLinkHandler).
        const caps = JSON.parse(
            readFileSync(path.join(repoRoot, 'capabilities/default.json'), 'utf8')
        );
        expect([...caps.windows].sort()).toEqual(Object.keys(ENTRY_POINTS).sort());
    });
});
