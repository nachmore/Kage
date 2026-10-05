/**
 * Extension CSS is injected into the trusted host document, so
 * `_loadExtensionCss` filters it rule by rule (UI-redress protection for
 * the permission prompt). These tests drive the real loader end to end
 * and assert on the <style> it injects.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';

const EXT_ID = 'acme';
const SCOPE = `[data-ext-widget-key^="${EXT_ID}:"]`;

async function loadCss(css, id = EXT_ID) {
    const mgr = new ExtensionManager(async (cmd) => {
        if (cmd === 'read_extension_file') return css;
        return undefined;
    });
    await mgr._loadExtensionCss(id, { contributes: { css: ['./styles.css'] } });
    const style = document.querySelector(`style[data-ext-css="${id}"]`);
    return style ? style.textContent : null;
}

// Normalise whitespace so assertions don't depend on the serializer's
// exact indentation of nested rules.
const flat = (s) => s.replace(/\s+/g, ' ');

beforeEach(() => {
    // jsdom gives the inert createHTMLDocument() no stylesheet support
    // (`el.sheet` is null), so parse in the live document instead. The
    // filter only reads the CSSOM; where the sheet lives doesn't matter.
    vi.spyOn(document.implementation, 'createHTMLDocument').mockReturnValue(document);
    // jsdom has no CSS.escape; the loader only uses it on the extension id.
    if (!globalThis.CSS?.escape) vi.stubGlobal('CSS', { escape: (s) => String(s) });
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    vi.spyOn(console, 'log').mockImplementation(() => {});
    document.body.innerHTML = `
        <div class="permission-modal-overlay" id="permissionModal">
            <div class="modal-inner"><button class="deny">Deny</button></div>
        </div>
        <div class="ext-widget" data-ext-widget-key="${EXT_ID}:bar"><span class="acme-row"></span></div>`;
});

afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    document.head.querySelectorAll('style').forEach((el) => el.remove());
    document.body.innerHTML = '';
});

describe('extension CSS filter: nesting', () => {
    it('keeps nested rules whose resolved selector misses the modal', async () => {
        // `& button` used to be probed as-is (`&` = :scope = the modal) and
        // matched the modal's own buttons, dropping legitimate styling.
        const out = flat(await loadCss('.acme-card { color: red; & button { color: blue; } }'));
        expect(out).toContain('.acme-card');
        expect(out).toContain('& button');
        expect(out).toContain('color: blue');
    });

    it('keeps relative nested rules and nested @media blocks', async () => {
        const out = flat(
            await loadCss('.acme-card { span { color: green; } @media (min-width: 1px) { i { color: red; } } }')
        );
        expect(out).toContain('color: green');
        expect(out).toContain('color: red');
    });

    it('still drops nested rules that resolve onto the modal', async () => {
        const out = flat(await loadCss('.modal-inner { color: red; & .deny { opacity: 0; } }'));
        // The parent itself matches the modal subtree, so the whole block goes.
        expect(out).not.toContain('opacity');
    });

    it('drops a nested rule that resolves to body::after', async () => {
        const out = flat(await loadCss('body { &::after { content: "x"; } }'));
        expect(out).not.toContain('content');
    });

    it('treats a top-level & as the document root', async () => {
        const out = flat(await loadCss('& { color: red; } .acme-ok { color: blue; }'));
        expect(out).not.toContain('color: red');
        expect(out).toContain('.acme-ok');
    });
});

describe('extension CSS filter: document root targets', () => {
    it.each([
        'body { opacity: 0; }',
        'html { filter: blur(4px); }',
        ':root { --kage-accent: transparent; }',
        'body::after { content: "Allow"; }',
        'html::before { content: ""; }',
        ':is(body)::after { content: ""; }',
        '.acme-ok, body { color: red; }',
    ])('drops %s', async (css) => {
        const out = flat(await loadCss(css));
        expect(out.trim()).toBe('');
    });

    it('keeps rules that only use body as an ancestor (theme hooks)', async () => {
        const out = flat(await loadCss('body.light-theme .acme-card { color: black; }'));
        expect(out).toContain('body.light-theme .acme-card');
    });
});

describe('extension CSS filter: overlays', () => {
    it.each([
        '.acme-overlay { position: fixed; inset: 0; }',
        '.acme-overlay { position: fixed; }',
        '.acme-overlay { position: sticky; top: 0; z-index: 5; }',
        '.acme-overlay { position: var(--p); }',
        '.acme-overlay { position: absolute; z-index: 99999; }',
        '.acme-overlay { z-index: var(--z); }',
        '.acme-overlay { z-index: calc(1e9); }',
    ])('drops %s', async (css) => {
        const out = flat(await loadCss(css));
        expect(out.trim()).toBe('');
    });

    it('keeps ordinary positioning and small z-indexes', async () => {
        const out = flat(
            await loadCss(
                '.acme-bar { position: relative; } .acme-progress { position: absolute; left: 0; top: 0; z-index: 2; } .acme-head { position: sticky; }'
            )
        );
        expect(out).toContain('.acme-bar');
        expect(out).toContain('.acme-progress');
        expect(out).toContain('.acme-head');
    });

    it('allows fixed/sticky positioning scoped to the extension widget host', async () => {
        const out = flat(
            await loadCss(
                `${SCOPE} .acme-menu { position: fixed; top: 0; z-index: 3; } ${SCOPE} { & .acme-head { position: sticky; top: 0; } }`
            )
        );
        expect(out).toContain('.acme-menu');
        expect(out).toContain('.acme-head');
    });

    it('keeps the z-index cap even inside the extension container', async () => {
        // A fixed box escapes its container visually, so the cap is the
        // part no selector scope can relax.
        const out = flat(await loadCss(`${SCOPE} .acme-menu { position: fixed; z-index: 10001; }`));
        expect(out).not.toContain('.acme-menu');
    });

    it('does not accept another extension, :not(), or sibling escapes as scoping', async () => {
        const out = flat(
            await loadCss(
                [
                    '[data-ext-widget-key^="other:"] .x1 { position: fixed; }',
                    `:not(${SCOPE}) .x2 { position: fixed; }`,
                    `${SCOPE} ~ .x3 { position: fixed; }`,
                    `.acme-ok, ${SCOPE} .x4 { position: fixed; }`,
                ].join('\n')
            )
        );
        for (const cls of ['.x1', '.x2', '.x3', '.x4']) expect(out).not.toContain(cls);
    });

    it('strips overlay declarations from @keyframes frames', async () => {
        const out = flat(
            await loadCss('@keyframes acme-pop { to { position: fixed; z-index: 99999; opacity: 1; } }')
        );
        expect(out).toContain('acme-pop');
        expect(out).toContain('opacity: 1');
        expect(out).not.toContain('z-index');
        expect(out).not.toContain('position');
    });
});

describe('extension CSS filter: state-dependent selectors', () => {
    // The live-DOM probe runs once at load, while the modal is idle and the
    // theme/language are whatever they happen to be. Selectors that only
    // match later must still be judged as if they match.
    it.each([
        'button:hover { opacity: 0; }',
        'button:not(:not(:hover)) { opacity: 0; }',
        '.light-theme::after { content: "Allow"; }',
        '.dark-theme { direction: rtl; }',
        '[lang="ar"] { direction: ltr; }',
        ':lang(en) { --kage-accent: transparent; }',
    ])('drops %s', async (css) => {
        const out = flat(await loadCss(css));
        expect(out.trim()).toBe('');
    });

    it('keeps state selectors confined to extension markup', async () => {
        const out = flat(
            await loadCss(
                '.acme-card button:hover { color: red; } body.light-theme .acme-row:not(.done) { color: blue; }'
            )
        );
        expect(out).toContain('.acme-card button:hover');
        expect(out).toContain('.acme-row:not(.done)');
    });
});

describe('extension CSS filter: modal ancestors', () => {
    it('drops rules on a container of the modal (inherited props reach it)', async () => {
        document.body.innerHTML = `
            <div class="chat-main"><div class="permission-modal-overlay chat-scoped" id="permissionModal">
                <button class="deny">Deny</button>
            </div></div>`;
        const out = flat(
            await loadCss('.chat-main { direction: rtl; } .acme-ok { color: red; }')
        );
        expect(out).not.toContain('direction');
        expect(out).toContain('.acme-ok');
    });
});

describe('extension CSS filter: widget scoping', () => {
    it('accepts a widget-specific key prefix as the extension container', async () => {
        const out = flat(
            await loadCss(
                '[data-ext-widget-key^="acme:bar"] .acme-menu { position: fixed; top: 0; } [data-ext-widget-key="acme:bar"] .acme-tip { position: fixed; }'
            )
        );
        expect(out).toContain('.acme-menu');
        expect(out).toContain('.acme-tip');
    });
});

describe('extension CSS filter: existing protections', () => {
    it('drops @import and rules naming the permission UI', async () => {
        const out = flat(
            await loadCss('@import url("https://evil.example/x.css"); .permission-modal { display: none; } .acme-ok { color: red; }')
        );
        expect(out).not.toContain('@import');
        expect(out).not.toContain('permission');
        expect(out).toContain('.acme-ok');
    });

    it('strips non-data url() beacons but keeps data: URIs', async () => {
        const out = flat(
            await loadCss(
                '.acme-a { background: url("https://evil.example/b.png"); color: red; } .acme-b { background-image: url("data:image/png;base64,AAAA"); }'
            )
        );
        expect(out).not.toContain('evil.example');
        expect(out).toContain('color: red');
        expect(out).toContain('data:image/png');
    });
});
