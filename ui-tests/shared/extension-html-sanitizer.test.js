import { describe, it, expect } from 'vitest';
import {
    sanitizeExtensionHtml,
    sanitizeExtensionHtmlToString,
    findExtActions,
} from '../../ui/js/shared/extension-html-sanitizer.js';

function toHtml(fragment) {
    const host = document.createElement('div');
    host.appendChild(fragment);
    return host.innerHTML;
}

describe('sanitizeExtensionHtml', () => {
    describe('script and event handler removal', () => {
        it('strips <script> tags', () => {
            const html = sanitizeExtensionHtmlToString('<p>a<script>alert(1)</script>b</p>');
            expect(html).not.toContain('<script');
        });

        it('strips onclick/onload/onmouseover attributes', () => {
            const html = sanitizeExtensionHtmlToString(
                '<button onclick="evil()" onmouseover="leak()">x</button>',
            );
            expect(html).not.toMatch(/onclick|onmouseover/i);
        });

        it('strips <style> and <iframe> tags', () => {
            const html = sanitizeExtensionHtmlToString(
                '<style>body{display:none}</style><iframe src="x"></iframe><p>ok</p>',
            );
            expect(html).not.toMatch(/<style|<iframe/i);
            expect(html).toContain('ok');
        });
    });

    describe('URL schemes', () => {
        it('drops javascript: href', () => {
            const html = sanitizeExtensionHtmlToString('<a href="javascript:evil()">bad</a>');
            expect(html).not.toContain('javascript:');
            // The <a> still renders — just without the href.
            expect(html).toContain('bad');
        });

        it('drops data: href', () => {
            const html = sanitizeExtensionHtmlToString('<a href="data:text/html,<script>evil</script>">x</a>');
            expect(html).not.toContain('data:');
        });

        it('keeps http(s), mailto, and in-page anchors', () => {
            const html = sanitizeExtensionHtmlToString(
                '<a href="https://example.com">a</a>' +
                '<a href="mailto:x@y.com">b</a>' +
                '<a href="#section">c</a>',
            );
            expect(html).toContain('https://example.com');
            expect(html).toContain('mailto:x@y.com');
            expect(html).toContain('#section');
        });

        it('drops data: img src (even text)', () => {
            const html = sanitizeExtensionHtmlToString('<img src="data:image/svg+xml,<svg onload=evil()>">');
            expect(html).not.toContain('data:');
        });

        it('keeps https img src', () => {
            const html = sanitizeExtensionHtmlToString('<img src="https://example.com/icon.png" alt="">');
            expect(html).toContain('src="https://example.com/icon.png"');
        });
    });

    describe('target=_blank safety', () => {
        it('forces rel=noopener on target=_blank links', () => {
            const html = sanitizeExtensionHtmlToString(
                '<a href="https://example.com" target="_blank">x</a>',
            );
            expect(html).toContain('target="_blank"');
            expect(html).toContain('rel="noopener noreferrer"');
        });

        it('forces target=_blank on http(s) links that did not declare it', () => {
            // Without this, clicking the link in a Tauri webview would
            // navigate the main window away from the app.
            const html = sanitizeExtensionHtmlToString(
                '<a href="https://example.com">x</a>',
            );
            expect(html).toContain('target="_blank"');
            expect(html).toContain('rel="noopener noreferrer"');
        });

        it('does not force target=_blank on in-page anchors', () => {
            const html = sanitizeExtensionHtmlToString(
                '<a href="#section">x</a>',
            );
            expect(html).toContain('href="#section"');
            expect(html).not.toContain('target="_blank"');
        });

        it('does not force target=_blank on mailto links', () => {
            const html = sanitizeExtensionHtmlToString(
                '<a href="mailto:x@y.com">x</a>',
            );
            expect(html).toContain('href="mailto:x@y.com"');
            expect(html).not.toContain('target="_blank"');
        });
    });

    describe('id and data-* stripping', () => {
        it('always strips id attributes', () => {
            const html = sanitizeExtensionHtmlToString('<div id="stealThisId">x</div>');
            expect(html).not.toContain('id=');
        });

        it('strips arbitrary data-* attributes', () => {
            const html = sanitizeExtensionHtmlToString('<div data-secret="foo" data-whatever="bar">x</div>');
            expect(html).not.toMatch(/data-(secret|whatever)/);
        });

        it('preserves data-ext-action on interactive elements', () => {
            const html = sanitizeExtensionHtmlToString(
                '<button data-ext-action="dismiss">x</button>' +
                '<a data-ext-action="more" href="#">y</a>' +
                '<span data-ext-action="inline">z</span>' +
                '<div data-ext-action="outer">w</div>',
            );
            expect(html).toContain('data-ext-action="dismiss"');
            expect(html).toContain('data-ext-action="more"');
            expect(html).toContain('data-ext-action="inline"');
            expect(html).toContain('data-ext-action="outer"');
        });

        it('drops data-ext-action on non-interactive elements', () => {
            const html = sanitizeExtensionHtmlToString('<h1 data-ext-action="weird">x</h1>');
            expect(html).not.toContain('data-ext-action');
        });
    });

    describe('style attribute filtering', () => {
        it('keeps simple color/padding declarations', () => {
            const html = sanitizeExtensionHtmlToString(
                '<div style="color: red; padding: 4px;">x</div>',
            );
            expect(html).toContain('color: red');
            expect(html).toContain('padding: 4px');
        });

        it('drops background-image with url()', () => {
            const html = sanitizeExtensionHtmlToString(
                '<div style="background-image: url(evil.png)">x</div>',
            );
            expect(html).not.toMatch(/url\s*\(/i);
        });

        it('drops expression()', () => {
            const html = sanitizeExtensionHtmlToString(
                '<div style="width: expression(evil())">x</div>',
            );
            expect(html).not.toMatch(/expression\s*\(/i);
        });

        it('drops position: fixed', () => {
            const html = sanitizeExtensionHtmlToString(
                '<div style="position: fixed; color: red;">x</div>',
            );
            expect(html).not.toContain('position');
            expect(html).toContain('color: red');
        });

        it('drops unknown properties', () => {
            const html = sanitizeExtensionHtmlToString(
                '<div style="color: red; fake-property: whatever;">x</div>',
            );
            expect(html).toContain('color: red');
            expect(html).not.toContain('fake-property');
        });
    });

    describe('tag allow-list', () => {
        it('rich mode allows block tags', () => {
            const html = sanitizeExtensionHtmlToString(
                '<div><h3>x</h3><ul><li>a</li></ul></div>',
                'rich',
            );
            expect(html).toContain('<h3>');
            expect(html).toContain('<ul>');
            expect(html).toContain('<li>');
        });

        it('inline mode strips block tags but keeps text', () => {
            const html = sanitizeExtensionHtmlToString(
                '<div><h3>heading</h3></div>',
                'inline',
            );
            // div and h3 are not in inline whitelist — replaced by text.
            expect(html).not.toContain('<h3>');
            expect(html).not.toContain('<div>');
            expect(html).toContain('heading');
        });

        it('inline mode keeps span/b/em/strong/button/a/code/img', () => {
            const html = sanitizeExtensionHtmlToString(
                '<span><b>a</b><em>b</em><code>c</code><button>d</button></span>',
                'inline',
            );
            expect(html).toContain('<span>');
            expect(html).toContain('<b>');
            expect(html).toContain('<em>');
            expect(html).toContain('<code>');
            expect(html).toContain('<button>');
        });
    });

    describe('SVG', () => {
        it('keeps basic SVG structure', () => {
            const html = sanitizeExtensionHtmlToString(
                '<svg width="16" height="16" viewBox="0 0 16 16"><path d="M0 0L16 16" fill="red"/></svg>',
            );
            expect(html).toContain('<svg');
            expect(html).toContain('<path');
            expect(html).toContain('d="M0 0L16 16"');
        });

        it('strips script inside SVG', () => {
            const html = sanitizeExtensionHtmlToString(
                '<svg><script>evil()</script><path d="M0 0"/></svg>',
            );
            expect(html).not.toContain('<script');
            expect(html).toContain('<path');
        });
    });
});

describe('findExtActions', () => {
    it('locates data-ext-action buttons', () => {
        const frag = sanitizeExtensionHtml(
            '<div><button data-ext-action="a">A</button><button data-ext-action="b">B</button></div>',
            'rich',
        );
        const host = document.createElement('div');
        host.appendChild(frag);
        const hits = findExtActions(host);
        expect(hits.length).toBe(2);
        expect(hits.map(h => h.actionId).sort()).toEqual(['a', 'b']);
    });

    it('returns empty array on static content', () => {
        const frag = sanitizeExtensionHtml('<p>hello</p>', 'rich');
        const host = document.createElement('div');
        host.appendChild(frag);
        expect(findExtActions(host)).toEqual([]);
    });
});
