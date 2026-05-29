/**
 * Tests for ui/js/shared/i18n.js — frontend translation runtime.
 *
 * What we're locking down:
 *   - `t()` returns a localised string; falls back to EN, then to literal key.
 *   - `{name}` substitution preserves unsupplied placeholders verbatim
 *     (so dev sees the bug instead of getting truncated text).
 *   - ICU plural / select expansions match `Intl.PluralRules` for languages
 *     with non-trivial categories (Russian few/many/other, Polish few/many,
 *     Arabic zero/two/few).
 *   - `=N` exact-match arms win over CLDR categories.
 *   - `applyStaticTranslations` rewrites every `data-i18n*` attribute in a
 *     given root, idempotently.
 *   - The active language flag drives `<html dir="rtl">` / `body.classList`
 *     so RTL CSS rules engage.
 */

import { beforeEach, describe, expect, it } from 'vitest';
import {
    t,
    tHtml,
    formatMessage,
    applyStaticTranslations,
    initI18n,
    activeLanguage,
    isRtl,
    isMachineTranslated,
} from '../../ui/js/shared/i18n.js';

/**
 * The module reads its catalog through an injected `invoke`; mock it.
 */
function mockInvoke(payload) {
    return async (cmd) => {
        if (cmd === 'get_i18n_catalog') return payload;
        throw new Error(`unexpected invoke: ${cmd}`);
    };
}

describe('frontend i18n', () => {
    beforeEach(() => {
        // Vitest doesn't reset module state between tests in the same file.
        // Each describe block reloads via initI18n() with its own payload —
        // initI18n is idempotent (caches the first promise) so we have to
        // bust the cached load between tests by reloading the module.
        // Vitest's `vi.resetModules()` would be cleaner but the module
        // itself is the surface under test, so we accept the cache-aware
        // structure of the test instead and order assertions accordingly.
    });

    it('initialises and returns the active language', async () => {
        // Use a fresh module import for this test so the cache is empty.
        const mod = await import('../../ui/js/shared/i18n.js?fresh1');
        const payload = {
            language: 'ja',
            rtl: false,
            machine_translated: true,
            catalog: { 'common.ok': { message: 'OK' } },
            fallback: { 'common.ok': { message: 'OK' } },
        };
        await mod.initI18n(mockInvoke(payload));
        expect(mod.activeLanguage()).toBe('ja');
        expect(mod.isMachineTranslated()).toBe(true);
        expect(mod.isRtl()).toBe(false);
    });

    it('flips html dir to rtl for RTL languages', async () => {
        const mod = await import('../../ui/js/shared/i18n.js?fresh2');
        await mod.initI18n(
            mockInvoke({
                language: 'ar',
                rtl: true,
                machine_translated: false,
                catalog: { hello: { message: 'مرحبا' } },
                fallback: { hello: { message: 'hello' } },
            })
        );
        expect(document.documentElement.getAttribute('dir')).toBe('rtl');
        expect(document.documentElement.getAttribute('lang')).toBe('ar');
        expect(mod.isRtl()).toBe(true);
    });

    it('falls back to EN when the active catalog is missing a key', async () => {
        const mod = await import('../../ui/js/shared/i18n.js?fresh3');
        await mod.initI18n(
            mockInvoke({
                language: 'ja',
                rtl: false,
                machine_translated: false,
                catalog: {},
                fallback: { 'errors.unknown': { message: 'Unknown error' } },
            })
        );
        expect(mod.t('errors.unknown')).toBe('Unknown error');
    });

    it('falls back to the literal key when EN is also missing it', async () => {
        const mod = await import('../../ui/js/shared/i18n.js?fresh4');
        await mod.initI18n(
            mockInvoke({
                language: 'en',
                rtl: false,
                machine_translated: false,
                catalog: {},
                fallback: {},
            })
        );
        expect(mod.t('does.not.exist')).toBe('does.not.exist');
    });
});

describe('formatMessage — simple substitution', () => {
    it('replaces {name} placeholders', () => {
        expect(formatMessage('hello {name}', { name: 'World' }, 'en')).toBe('hello World');
    });

    it('preserves unsupplied placeholders verbatim', () => {
        // The dev-time signal: the bug is visible, not silently truncated.
        expect(formatMessage('hello {name}', {}, 'en')).toBe('hello {name}');
    });

    it('coerces non-string values', () => {
        expect(formatMessage('count: {n}', { n: 42 }, 'en')).toBe('count: 42');
        expect(formatMessage('flag: {b}', { b: true }, 'en')).toBe('flag: true');
    });

    it('returns the template unchanged when there are no braces', () => {
        expect(formatMessage('plain text', {}, 'en')).toBe('plain text');
    });
});

describe('formatMessage — ICU plural', () => {
    it('selects English `one` vs `other`', () => {
        const tpl = '{count, plural, one {1 chat} other {# chats}}';
        expect(formatMessage(tpl, { count: 1 }, 'en')).toBe('1 chat');
        expect(formatMessage(tpl, { count: 5 }, 'en')).toBe('5 chats');
        expect(formatMessage(tpl, { count: 0 }, 'en')).toBe('0 chats');
    });

    it('honours =N exact-match arms before CLDR categories', () => {
        const tpl = '{count, plural, =0 {none} one {# item} other {# items}}';
        expect(formatMessage(tpl, { count: 0 }, 'en')).toBe('none');
        expect(formatMessage(tpl, { count: 1 }, 'en')).toBe('1 item');
        expect(formatMessage(tpl, { count: 7 }, 'en')).toBe('7 items');
    });

    it('handles Russian few/many/other categories', () => {
        // Russian: 1 → one; 2-4 → few; 5-20 → many; 21 → one; 22 → few; etc.
        const tpl = '{n, plural, one {# книга} few {# книги} many {# книг} other {# книг}}';
        expect(formatMessage(tpl, { n: 1 }, 'ru')).toBe('1 книга');
        expect(formatMessage(tpl, { n: 2 }, 'ru')).toBe('2 книги');
        expect(formatMessage(tpl, { n: 5 }, 'ru')).toBe('5 книг');
        expect(formatMessage(tpl, { n: 22 }, 'ru')).toBe('22 книги');
    });

    it('handles Polish few/many', () => {
        const tpl = '{n, plural, one {# plik} few {# pliki} many {# plików} other {# plików}}';
        expect(formatMessage(tpl, { n: 1 }, 'pl')).toBe('1 plik');
        expect(formatMessage(tpl, { n: 3 }, 'pl')).toBe('3 pliki');
        expect(formatMessage(tpl, { n: 7 }, 'pl')).toBe('7 plików');
    });

    it('handles Arabic zero/two/few/many/other', () => {
        const tpl =
            '{n, plural, zero {لا توجد كتب} one {كتاب واحد} two {كتابان} few {# كتب} many {# كتاباً} other {# كتاب}}';
        expect(formatMessage(tpl, { n: 0 }, 'ar')).toBe('لا توجد كتب');
        expect(formatMessage(tpl, { n: 1 }, 'ar')).toBe('كتاب واحد');
        expect(formatMessage(tpl, { n: 2 }, 'ar')).toBe('كتابان');
        expect(formatMessage(tpl, { n: 5 }, 'ar')).toBe('5 كتب');
    });

    it('falls back to `other` when no other arm matches', () => {
        const tpl = '{n, plural, other {fallback}}';
        expect(formatMessage(tpl, { n: 1 }, 'en')).toBe('fallback');
        expect(formatMessage(tpl, { n: 5 }, 'en')).toBe('fallback');
    });

    it('substitutes # for the count value inside a plural arm', () => {
        const tpl = '{n, plural, one {1 chat} other {# chats}}';
        expect(formatMessage(tpl, { n: 42 }, 'en')).toBe('42 chats');
    });

    it('expands nested {name} inside a plural arm', () => {
        const tpl = '{n, plural, one {1 chat with {who}} other {# chats with {who}}}';
        expect(formatMessage(tpl, { n: 1, who: 'Bot' }, 'en')).toBe('1 chat with Bot');
        expect(formatMessage(tpl, { n: 3, who: 'Bot' }, 'en')).toBe('3 chats with Bot');
    });
});

describe('formatMessage — ICU select', () => {
    it('picks the matching arm', () => {
        const tpl = '{role, select, admin {Administrator} user {Member} other {Guest}}';
        expect(formatMessage(tpl, { role: 'admin' }, 'en')).toBe('Administrator');
        expect(formatMessage(tpl, { role: 'user' }, 'en')).toBe('Member');
    });

    it('falls back to `other` for unknown values', () => {
        const tpl = '{role, select, admin {Administrator} other {Guest}}';
        expect(formatMessage(tpl, { role: 'something_else' }, 'en')).toBe('Guest');
    });

    it('coerces non-string values', () => {
        const tpl = '{n, select, 0 {none} 1 {one} other {many}}';
        expect(formatMessage(tpl, { n: 0 }, 'en')).toBe('none');
        expect(formatMessage(tpl, { n: 1 }, 'en')).toBe('one');
        expect(formatMessage(tpl, { n: 99 }, 'en')).toBe('many');
    });
});

describe('applyStaticTranslations', () => {
    it('rewrites data-i18n textContent', async () => {
        const mod = await import('../../ui/js/shared/i18n.js?fresh-static');
        await mod.initI18n(
            mockInvoke({
                language: 'en',
                rtl: false,
                machine_translated: false,
                catalog: { 'common.ok': { message: 'OK' } },
                fallback: { 'common.ok': { message: 'OK' } },
            })
        );
        const root = document.createElement('div');
        const btn = document.createElement('button');
        btn.setAttribute('data-i18n', 'common.ok');
        root.appendChild(btn);
        mod.applyStaticTranslations(root);
        expect(btn.textContent).toBe('OK');
    });

    it('rewrites data-i18n-placeholder, -title, -aria-label, -alt', async () => {
        const mod = await import('../../ui/js/shared/i18n.js?fresh-attrs');
        await mod.initI18n(
            mockInvoke({
                language: 'en',
                rtl: false,
                machine_translated: false,
                catalog: {
                    'common.search': { message: 'Search' },
                    'common.help': { message: 'Help' },
                    'common.icon_alt': { message: 'icon' },
                },
                fallback: {
                    'common.search': { message: 'Search' },
                    'common.help': { message: 'Help' },
                    'common.icon_alt': { message: 'icon' },
                },
            })
        );
        const root = document.createElement('div');
        root.innerHTML = `
            <input data-i18n-placeholder="common.search">
            <button data-i18n-title="common.help">?</button>
            <a data-i18n-aria-label="common.help">link</a>
            <img data-i18n-alt="common.icon_alt">
        `;
        mod.applyStaticTranslations(root);
        expect(root.querySelector('input').placeholder).toBe('Search');
        expect(root.querySelector('button').title).toBe('Help');
        expect(root.querySelector('a').getAttribute('aria-label')).toBe('Help');
        expect(root.querySelector('img').alt).toBe('icon');
    });

    it('substitutes data-i18n-args JSON for placeholders', async () => {
        const mod = await import('../../ui/js/shared/i18n.js?fresh-args');
        await mod.initI18n(
            mockInvoke({
                language: 'en',
                rtl: false,
                machine_translated: false,
                catalog: { greeting: { message: 'Hello {name}' } },
                fallback: { greeting: { message: 'Hello {name}' } },
            })
        );
        const root = document.createElement('div');
        const span = document.createElement('span');
        span.setAttribute('data-i18n', 'greeting');
        span.setAttribute('data-i18n-args', '{"name":"Ada"}');
        root.appendChild(span);
        mod.applyStaticTranslations(root);
        expect(span.textContent).toBe('Hello Ada');
    });

    describe('tHtml — auto-escape vars for HTML contexts', () => {
        it('escapes HTML special characters in interpolated vars', async () => {
            const mod = await import('../../ui/js/shared/i18n.js?tHtml-basic');
            await mod.initI18n(
                mockInvoke({
                    language: 'en',
                    rtl: false,
                    catalog: {
                        wrap: { message: 'Hello {who}' },
                    },
                    fallback: {},
                })
            );
            // The vars must be HTML-escaped so a session named "Foo & <bar>"
            // can't break out of its container.
            expect(mod.tHtml('wrap', { who: 'Foo & <bar>' })).toBe(
                'Hello Foo &amp; &lt;bar&gt;'
            );
        });

        it('does NOT escape the catalog template — _html keys keep their markup', async () => {
            const mod = await import('../../ui/js/shared/i18n.js?tHtml-template');
            await mod.initI18n(
                mockInvoke({
                    language: 'en',
                    rtl: false,
                    catalog: {
                        link: {
                            message: 'Install <code>{package}</code> to continue.',
                        },
                    },
                    fallback: {},
                })
            );
            // The <code> from the template survives, but the {package} var is
            // escaped so a malicious package name cannot inject its own tags.
            expect(
                mod.tHtml('link', { package: '<script>alert(1)</script>' })
            ).toBe(
                'Install <code>&lt;script&gt;alert(1)&lt;/script&gt;</code> to continue.'
            );
        });

        it('does not affect plain t() — vars there pass through verbatim', async () => {
            const mod = await import('../../ui/js/shared/i18n.js?tHtml-plain');
            await mod.initI18n(
                mockInvoke({
                    language: 'en',
                    rtl: false,
                    catalog: {
                        wrap: { message: 'Hello {who}' },
                    },
                    fallback: {},
                })
            );
            // Plain t() is for confirm()/alert()/textContent — escaping there
            // would render &amp; literally.
            expect(mod.t('wrap', { who: 'Foo & Bar' })).toBe('Hello Foo & Bar');
        });

        it('escapes vars inside plural arms', async () => {
            const mod = await import('../../ui/js/shared/i18n.js?tHtml-plural');
            await mod.initI18n(
                mockInvoke({
                    language: 'en',
                    rtl: false,
                    catalog: {
                        items: {
                            message:
                                '{count, plural, one {1 item by {who}} other {# items by {who}}}',
                        },
                    },
                    fallback: {},
                })
            );
            expect(mod.tHtml('items', { count: 3, who: '<x>' })).toBe(
                '3 items by &lt;x&gt;'
            );
        });
    });
});
