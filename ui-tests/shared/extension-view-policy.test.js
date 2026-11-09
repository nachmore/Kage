/**
 * Host-side rules for extension views (ui/js/shared/extension-view-policy.js,
 * docs/design/EXTENSION_VIEWS.md §4, §6.2, §8.3, §8.4, §11, §13.3).
 *
 * The normalisation and params cases come from the fixture Rust's
 * tests/extension_view_policy_test.rs reads too, so both sides agree.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it, vi } from 'vitest';
import {
    buildViewSrcdoc,
    createRateWindow,
    DEFAULT_MIN_SIZE,
    DEFAULT_SIZE,
    DEFAULT_SIZE_MAX,
    DEFAULT_SIZE_MIN,
    escapeInlineScript,
    isBundlePath,
    isSafeTokenValue,
    isValidViewId,
    MAX_COMPOSE_CHARS,
    MAX_COPY_CHARS,
    MAX_CSS_FILE_BYTES,
    MAX_CSS_FILES,
    MAX_CSS_TOTAL_BYTES,
    MAX_ICON_CHARS,
    MAX_PARAMS_BYTES,
    MAX_PARAMS_DEPTH,
    MAX_TITLE_CHARS,
    MAX_VIEW_ID_LEN,
    MAX_VIEWS,
    MIN_SIZE_FLOOR,
    normalizeViewSource,
    normalizeViews,
    readViewTheme,
    sanitizeStorageChanges,
    sanitizeViewTheme,
    sanitizeViewTitle,
    themeStyleText,
    VIEW_CSP,
    VIEW_IFRAME_ALLOW,
    VIEW_IFRAME_SANDBOX,
    VIEW_SOURCES,
    VIEW_THEME_TOKENS,
    validateOpenViewEffect,
    validateViewParams,
    viewTextError,
    viewThemesEqual,
} from '../../ui/js/shared/extension-view-policy.js';
import { copyTextError, MAX_COPY_LENGTH } from '../../ui/js/shared/settings-schema.js';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const fixture = JSON.parse(
    readFileSync(path.join(repoRoot, 'tests/fixtures/extension-views-policy.json'), 'utf8')
);
const RLO = String.fromCharCode(0x202e);
const LRI = String.fromCharCode(0x2066);

const silent = { warn: null };

describe('normalizeViews mirrors src/extensions/views.rs (shared fixture)', () => {
    for (const c of fixture.normalize) {
        it(c.case, () => {
            expect(normalizeViews(c.views, fixture.name, fixture.icon, 'fixture', silent)).toEqual(
                c.expected
            );
        });
    }
});

describe('validateViewParams mirrors validate_params (shared fixture)', () => {
    for (const c of fixture.params) {
        it(c.case, () => {
            const got = validateViewParams(c.params);
            if (c.error === null) expect(got).toBeNull();
            else expect(got?.key).toBe(c.error);
        });
    }
});

/** `pub const NAME: usize = <expr>;` from a Rust file, evaluated. */
function rustUsize(file, name) {
    const text = readFileSync(path.join(repoRoot, file), 'utf8');
    const m = text.match(new RegExp(`pub const ${name}: (?:usize|u64) = ([^;]+);`));
    if (!m) throw new Error(`${name} not found in ${file}`);
    return Function(`return (${m[1]});`)();
}

function rustSize(file, name) {
    const text = readFileSync(path.join(repoRoot, file), 'utf8');
    const m = text.match(
        new RegExp(`pub const ${name}: ViewSize = ViewSize \\{\\s*width: (\\d+),\\s*height: (\\d+),?\\s*\\}`)
    );
    if (!m) throw new Error(`${name} not found in ${file}`);
    return { width: Number(m[1]), height: Number(m[2]) };
}

describe('limits match Rust', () => {
    const file = 'src/extensions/views.rs';
    it('numbers', () => {
        expect(MAX_VIEWS).toBe(rustUsize(file, 'MAX_VIEWS'));
        expect(MAX_VIEW_ID_LEN).toBe(rustUsize(file, 'MAX_VIEW_ID_LEN'));
        expect(MAX_TITLE_CHARS).toBe(rustUsize(file, 'MAX_TITLE_CHARS'));
        expect(MAX_ICON_CHARS).toBe(rustUsize(file, 'MAX_ICON_CHARS'));
        expect(MAX_CSS_FILES).toBe(rustUsize(file, 'MAX_CSS_FILES'));
        expect(MAX_CSS_FILE_BYTES).toBe(rustUsize(file, 'MAX_CSS_FILE_BYTES'));
        expect(MAX_CSS_TOTAL_BYTES).toBe(rustUsize(file, 'MAX_CSS_TOTAL_BYTES'));
        expect(MAX_PARAMS_BYTES).toBe(rustUsize(file, 'MAX_PARAMS_BYTES'));
        expect(MAX_PARAMS_DEPTH).toBe(rustUsize(file, 'MAX_PARAMS_DEPTH'));
    });

    it('sizes', () => {
        expect(DEFAULT_SIZE).toEqual(rustSize(file, 'DEFAULT_SIZE'));
        expect(DEFAULT_SIZE_MIN).toEqual(rustSize(file, 'DEFAULT_SIZE_MIN'));
        expect(DEFAULT_SIZE_MAX).toEqual(rustSize(file, 'DEFAULT_SIZE_MAX'));
        expect(DEFAULT_MIN_SIZE).toEqual(rustSize(file, 'DEFAULT_MIN_SIZE'));
        expect(MIN_SIZE_FLOOR).toEqual(rustSize(file, 'MIN_SIZE_FLOOR'));
    });

    it('view sources match ext_views::VIEW_SOURCES', () => {
        const text = readFileSync(path.join(repoRoot, 'src/ext_views.rs'), 'utf8');
        const body = text.match(/pub const VIEW_SOURCES: &\[&str\] = &\[([\s\S]*?)\];/)[1];
        const rust = [...body.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
        expect(VIEW_SOURCES).toEqual(rust);
        for (const s of VIEW_SOURCES) expect(normalizeViewSource(s)).toBe(s);
        for (const s of [undefined, null, '', 'Chat', 'deep-link', 42]) {
            expect(normalizeViewSource(s)).toBe('launcher');
        }
    });
});

describe('normalizeViews details', () => {
    it('warns about bad entries unless silenced', () => {
        const warn = vi.fn();
        normalizeViews([{ id: 'BAD', module: './v.js' }, 'x'], 'n', 'i', 'demo', { warn });
        expect(warn).toHaveBeenCalledTimes(2);
        expect(warn.mock.calls[0][0]).toMatch(/Extension 'demo': view 0 is invalid/);
        const quiet = vi.spyOn(console, 'warn').mockImplementation(() => {});
        normalizeViews([{ id: 'BAD', module: './v.js' }], 'n', 'i', 'demo', silent);
        expect(quiet).not.toHaveBeenCalled();
        quiet.mockRestore();
    });

    it('reads own properties only', () => {
        const entry = Object.create({ id: 'inherited', module: './v.js' });
        expect(normalizeViews([entry], 'n', 'i', 'x', silent)).toEqual([]);
    });

    it('is idempotent on its own output', () => {
        const once = normalizeViews(
            [{ id: 'a', module: './a.js', title: 'A', icon: '🗺️', css: ['./a.css'] }],
            'n',
            '🧠',
            'x',
            silent
        );
        expect(normalizeViews(once, 'n', '🧠', 'x', silent)).toEqual(once);
    });

    it('id and path helpers', () => {
        expect(isValidViewId('explorer')).toBe(true);
        expect(isValidViewId('a'.repeat(33))).toBe(false);
        expect(isValidViewId('map:view')).toBe(false);
        expect(isValidViewId(undefined)).toBe(false);
        expect(isBundlePath('./view.js', '.js')).toBe(true);
        for (const bad of ['', './', '.js', './.js', '../v.js', 'v.js?x', 'v.js#a', 'v%2e.js', 'C:/v.js']) {
            expect(isBundlePath(bad, '.js'), bad).toBe(false);
        }
        expect(isBundlePath(`vi${String.fromCharCode(10)}ew.js`, '.js')).toBe(false);
    });
});

describe('validateViewParams (JS-only values)', () => {
    it('refuses values JSON or Rust can not carry', () => {
        class Point {}
        for (const bad of [
            { a: undefined },
            { a: Number.NaN },
            { a: Number.POSITIVE_INFINITY },
            { a: () => 1 },
            { a: new Date(0) },
            { a: new Map() },
            { a: new Point() },
            { a: 1n },
            { a: String.fromCharCode(0xd800) },
            { [String.fromCharCode(0xdc00)]: 1 },
            new Point(),
        ]) {
            expect(validateViewParams(bad)?.key).toBe('errors.ext_view.params_invalid');
        }
    });

    it('accepts undefined and null as no params', () => {
        expect(validateViewParams(undefined)).toBeNull();
        expect(validateViewParams(null)).toBeNull();
        expect(validateViewParams(Object.create(null))).toBeNull();
    });

    it('stops on a cycle', () => {
        const a = {};
        a.self = a;
        expect(validateViewParams(a)?.key).toBe('errors.ext_view.params_invalid');
    });

    it('measures UTF-8 bytes of compact JSON', () => {
        // {"s":"…"} is 8 bytes plus the string; é is 2 bytes.
        const fits = { s: 'é'.repeat((MAX_PARAMS_BYTES - 8) / 2) };
        expect(validateViewParams(fits)).toBeNull();
        const over = { s: `${'é'.repeat((MAX_PARAMS_BYTES - 8) / 2)}x` };
        const bad = validateViewParams(over);
        expect(bad).toMatchObject({
            ok: false,
            kind: 'invalid_input',
            key: 'errors.ext_view.params_too_large',
            vars: { size: String(MAX_PARAMS_BYTES + 1), limit: String(MAX_PARAMS_BYTES) },
        });
    });
});

describe('validateOpenViewEffect', () => {
    const views = [{ id: 'explorer', module: './explorer.js' }];
    const ok = { hostFeatures: ['views'], capabilities: ['storage', 'views'], views };

    it('passes a declared view with valid params', () => {
        expect(
            validateOpenViewEffect({ type: 'open_view', view: 'explorer', params: { focus: 'e1' } }, ok)
        ).toEqual({ ok: true, view: 'explorer', params: { focus: 'e1' } });
        expect(validateOpenViewEffect({ type: 'open_view', view: 'explorer' }, ok)).toEqual({
            ok: true,
            view: 'explorer',
            params: null,
        });
    });

    it('checks in order: host feature, grant, declared view, params', () => {
        const effect = { type: 'open_view', view: 'nope', params: [] };
        expect(validateOpenViewEffect(effect, { ...ok, hostFeatures: [], capabilities: [] })).toMatchObject({
            ok: false,
            kind: 'not_permitted',
            key: 'errors.ext_view.opener_refused',
        });
        expect(
            validateOpenViewEffect(effect, { ...ok, capabilities: ['storage'], extensionName: 'Mind Map' })
        ).toMatchObject({
            kind: 'not_permitted',
            key: 'errors.ext_view.not_granted',
            vars: { extension: 'Mind Map' },
        });
        expect(validateOpenViewEffect(effect, { ...ok, extensionId: 'mindmap' })).toMatchObject({
            kind: 'invalid_input',
            key: 'errors.ext_view.unknown_view',
            vars: { extension: 'mindmap', view: 'nope' },
        });
        expect(
            validateOpenViewEffect({ type: 'open_view', view: 'explorer', params: [] }, ok)
        ).toMatchObject({ kind: 'invalid_input', key: 'errors.ext_view.params_invalid' });
    });

    it('accepts a Set of capabilities and raw manifest views', () => {
        expect(
            validateOpenViewEffect(
                { type: 'open_view', view: 'explorer' },
                { hostFeatures: ['views'], capabilities: new Set(['views']), views }
            ).ok
        ).toBe(true);
    });

    it('refuses anything that is not an open_view effect', () => {
        for (const bad of [null, 'open_view', { type: 'open_url', view: 'explorer' }, []]) {
            expect(validateOpenViewEffect(bad, ok).ok).toBe(false);
        }
        expect(validateOpenViewEffect({ type: 'open_view', view: 42 }, ok).key).toBe(
            'errors.ext_view.unknown_view'
        );
        // A view the manifest lists past the fourth entry isn't declared.
        const many = ['a', 'b', 'c', 'd', 'e'].map((id) => ({ id, module: './v.js' }));
        expect(validateOpenViewEffect({ type: 'open_view', view: 'e' }, { ...ok, views: many }).ok).toBe(
            false
        );
        expect(validateOpenViewEffect({ type: 'open_view', view: 'd' }, { ...ok, views: many }).ok).toBe(
            true
        );
    });
});

describe('sanitizeViewTitle', () => {
    it('removes control and bidi-override characters, trims and caps', () => {
        expect(sanitizeViewTitle(`  Bin${RLO}go${String.fromCharCode(7)}  `)).toBe('Bingo');
        expect(sanitizeViewTitle(`${LRI}${RLO}\n`)).toBe('');
        expect(sanitizeViewTitle('é'.repeat(100))).toBe('é'.repeat(MAX_TITLE_CHARS));
        expect(sanitizeViewTitle('🗺️'.repeat(50), 80)).toBe(Array.from('🗺️'.repeat(50)).slice(0, 80).join(''));
        expect(sanitizeViewTitle(42)).toBe('');
        expect(sanitizeViewTitle(null)).toBe('');
    });
});

describe('theme tokens', () => {
    it('allows hex, rgb[a], hsl[a], named colours and plain font stacks', () => {
        for (const v of ['#fff', '#1e1e1eff', 'rgba(49, 151, 149, 0.3)', 'rgb(1 2 3 / 50%)', 'hsl(180deg 50% 40%)', 'transparent', ' #1a202c ']) {
            expect(isSafeTokenValue('kage-bg', v), v).toBe(true);
        }
        expect(
            isSafeTokenValue(
                'kage-font',
                '\n        -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif'
            )
        ).toBe(true);
        expect(isSafeTokenValue('kage-font-mono', "ui-monospace, 'SF Mono', Menlo, monospace")).toBe(true);
    });

    it('rejects url(), ;, }, expression(), over-long values, unknown names and non-strings', () => {
        for (const v of [
            'url(https://example.com/x.png)',
            '#fff; background: red',
            'red}body{display:none',
            'expression(alert(1))',
            'rgb(var(--x), 0, 0)',
            'rgb(1,2,3) url(x)',
            '#ggg',
            '',
            `#${'a'.repeat(250)}`,
            'a'.repeat(201),
        ]) {
            expect(isSafeTokenValue('kage-bg', v), v).toBe(false);
        }
        expect(isSafeTokenValue('kage-font', 'Roboto; color: red')).toBe(false);
        expect(isSafeTokenValue('kage-font', 'url(x)')).toBe(false);
        expect(isSafeTokenValue('kage-font', 'a\\62 c')).toBe(false);
        expect(isSafeTokenValue('kage-font', 'x'.repeat(201))).toBe(false);
        expect(isSafeTokenValue('kage-bg-other', '#fff')).toBe(false);
        expect(isSafeTokenValue('kage-bg', 42)).toBe(false);
    });

    it('sanitizeViewTheme keeps only allowed tokens with safe values, frozen', () => {
        const theme = sanitizeViewTheme({
            scheme: 'light',
            fontSize: 15,
            reducedMotion: true,
            tokens: { 'kage-bg': '#fafafa', 'kage-text': 'url(x)', 'kage-other': '#000' },
        });
        expect(theme).toEqual({
            scheme: 'light',
            fontSize: 15,
            reducedMotion: true,
            tokens: { 'kage-bg': '#fafafa' },
        });
        expect(Object.isFrozen(theme)).toBe(true);
        expect(Object.isFrozen(theme.tokens)).toBe(true);
        expect(sanitizeViewTheme(null)).toEqual({
            scheme: 'dark',
            fontSize: 14,
            reducedMotion: false,
            tokens: {},
        });
        expect(sanitizeViewTheme({ scheme: 'neon', fontSize: 'big' }).scheme).toBe('dark');
        expect(sanitizeViewTheme({ fontSize: 500 }).fontSize).toBe(72);
    });

    it('themeStyleText writes :root custom properties, scheme and font size', () => {
        expect(
            themeStyleText({ scheme: 'dark', fontSize: 14, tokens: { 'kage-bg': '#1e1e1e', 'kage-text': '#eee' } })
        ).toBe(':root { --kage-bg: #1e1e1e; --kage-text: #eee; color-scheme: dark; font-size: 14px }');
        // Re-sanitises whatever it gets.
        expect(themeStyleText({ tokens: { 'kage-bg': '#fff}' } })).not.toContain('}}');
    });

    it('readViewTheme reads the allowed tokens from a computed style', () => {
        const values = { '--kage-bg': ' #1a202c', '--kage-accent': '#319795', '--kage-link': 'url(x)' };
        const style = { getPropertyValue: (n) => values[n] ?? '' };
        const theme = readViewTheme(style, { scheme: 'dark', fontSize: 13, reducedMotion: false });
        expect(theme.tokens).toEqual({ 'kage-bg': '#1a202c', 'kage-accent': '#319795' });
        expect(theme.fontSize).toBe(13);
        expect(VIEW_THEME_TOKENS).toHaveLength(20);
    });

    it('viewThemesEqual compares snapshots', () => {
        const a = sanitizeViewTheme({ tokens: { 'kage-bg': '#000' } });
        expect(viewThemesEqual(a, sanitizeViewTheme({ tokens: { 'kage-bg': '#000' } }))).toBe(true);
        expect(viewThemesEqual(a, sanitizeViewTheme({ tokens: { 'kage-bg': '#111' } }))).toBe(false);
        expect(viewThemesEqual(a, sanitizeViewTheme({ tokens: { 'kage-bg': '#000' }, reducedMotion: true }))).toBe(
            false
        );
    });
});

describe('the view iframe', () => {
    it('CSP: no network, no frames, objects, forms or base', () => {
        expect(VIEW_CSP).toBe(
            [
                "default-src 'none'",
                "script-src 'unsafe-inline' 'unsafe-eval' blob:",
                'worker-src blob:',
                'child-src blob:',
                "style-src 'unsafe-inline' blob:",
                'img-src blob: data:',
                'font-src blob: data:',
                'media-src blob: data:',
                'connect-src blob: data:',
                "frame-src 'none'",
                "object-src 'none'",
                "base-uri 'none'",
                "form-action 'none'",
            ].join('; ')
        );
        expect(VIEW_CSP).not.toMatch(/https?:|\*/);
    });

    it('sandbox and allow attributes', () => {
        expect(VIEW_IFRAME_SANDBOX).toBe('allow-scripts');
        for (const f of ['camera', 'microphone', 'geolocation', 'clipboard-read', 'clipboard-write', 'display-capture', 'fullscreen', 'payment', 'usb', 'serial', 'hid', 'bluetooth', 'midi', 'screen-wake-lock', 'web-share', 'autoplay']) {
            expect(VIEW_IFRAME_ALLOW).toContain(`${f} 'none'`);
        }
    });

    it('buildViewSrcdoc puts the CSP meta first', () => {
        const html = buildViewSrcdoc({
            runtimeSource: 'console.log(1)',
            lang: 'ar',
            dir: 'rtl',
            theme: { scheme: 'light', tokens: { 'kage-bg': '#fff' } },
        });
        expect(html.startsWith('<!DOCTYPE html><html lang="ar" dir="rtl"><head><meta http-equiv="Content-Security-Policy"')).toBe(true);
        expect(html).toContain(`content="${VIEW_CSP}"`);
        expect(html.indexOf('Content-Security-Policy')).toBeLessThan(html.indexOf('<style'));
        expect(html.indexOf('Content-Security-Policy')).toBeLessThan(html.indexOf('<script'));
        expect(html).toContain('<style id="kage-view-theme">:root { --kage-bg: #fff; color-scheme: light;');
        expect(html).toContain('<script>console.log(1)</script>');

        const doc = new DOMParser().parseFromString(html, 'text/html');
        expect(doc.head.firstElementChild.getAttribute('http-equiv')).toBe('Content-Security-Policy');
        expect(doc.head.firstElementChild.getAttribute('content')).toBe(VIEW_CSP);
        expect(doc.documentElement.getAttribute('dir')).toBe('rtl');
    });

    it('buildViewSrcdoc sanitises lang and dir and keeps the script closed', () => {
        const html = buildViewSrcdoc({
            runtimeSource: 'var s = "</script><img src=x>";',
            lang: 'en"><script>',
            dir: 'sideways',
        });
        expect(html).toContain('<html lang="en" dir="ltr">');
        const doc = new DOMParser().parseFromString(html, 'text/html');
        expect(doc.querySelectorAll('script')).toHaveLength(1);
        expect(doc.querySelector('img')).toBeNull();
        expect(escapeInlineScript('a</SCRIPT>b')).toBe('a<\\/script>b');
    });
});

describe('view text limits', () => {
    it('copyTextError takes a max', () => {
        expect(copyTextError('x'.repeat(MAX_COPY_LENGTH + 1))).toMatch(/at most 2048/);
        expect(copyTextError('x'.repeat(MAX_COPY_LENGTH + 1), { max: 8192 })).toBeNull();
        expect(copyTextError('x'.repeat(8193), { max: 8192 })).toMatch(/at most 8192/);
        // A bad max falls back to the default.
        expect(copyTextError('x'.repeat(MAX_COPY_LENGTH + 1), { max: -1 })).toMatch(/at most 2048/);
    });

    it('viewTextError applies the copy rules with the view limits', () => {
        expect(viewTextError('hello\nworld', MAX_COMPOSE_CHARS)).toBeNull();
        expect(viewTextError('x'.repeat(MAX_COMPOSE_CHARS + 1), MAX_COMPOSE_CHARS)).toMatch(/at most 2000/);
        expect(viewTextError('x'.repeat(MAX_COPY_CHARS), MAX_COPY_CHARS)).toBeNull();
        expect(viewTextError(`a${RLO}b`, MAX_COPY_CHARS)).toMatch(/plain text/);
        expect(viewTextError('', MAX_COPY_CHARS)).toMatch(/empty/);
        expect(viewTextError(7, MAX_COPY_CHARS)).toMatch(/string/);
    });
});

describe('createRateWindow', () => {
    it('allows max per window and recovers as the window slides', () => {
        let now = 0;
        const rate = createRateWindow(2, 1000, () => now);
        expect(rate.take()).toBe(true);
        expect(rate.take()).toBe(true);
        expect(rate.take()).toBe(false);
        now = 999;
        expect(rate.take()).toBe(false);
        now = 1000;
        expect(rate.take()).toBe(true);
    });
});

describe('sanitizeStorageChanges', () => {
    it('keeps key, op, self and source; drops the window label and bad entries', () => {
        expect(
            sanitizeStorageChanges([
                { key: 'mm-meta', op: 'save', source: 'window', window: 'floating', self: true },
                { key: 'mm-layout', op: 'delete', source: 'sync' },
                { key: 'anything', op: 'bulk', source: 'import' },
                { key: '../x', op: 'save', source: 'window' },
                { key: 'a', op: 'rename' },
                { key: 3, op: 'save' },
                null,
            ])
        ).toEqual([
            { key: 'mm-meta', op: 'save', self: true, source: 'window' },
            { key: 'mm-layout', op: 'delete', self: false, source: 'sync' },
            { key: '*', op: 'bulk', self: false, source: 'import' },
        ]);
        expect(sanitizeStorageChanges('nope')).toEqual([]);
        expect(sanitizeStorageChanges([{ key: 'k', op: 'save', source: 'elsewhere' }])[0].source).toBe(
            'window'
        );
    });

    it('collapses an oversized batch to one bulk change', () => {
        const many = Array.from({ length: 65 }, (_, i) => ({ key: `k${i}`, op: 'save', source: 'sync' }));
        expect(sanitizeStorageChanges(many)).toEqual([
            { key: '*', op: 'bulk', self: false, source: 'sync' },
        ]);
    });
});
