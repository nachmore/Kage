/**
 * The theme snapshot a view gets (docs/design/EXTENSION_VIEWS.md §11):
 * only the listed tokens, each value checked, and a cheap equality so the
 * page sends `viewTheme` only on a real change.
 */

import { describe, expect, it, vi } from 'vitest';
import {
    computeThemeSnapshot,
    DEFAULT_FONT_SIZE,
    readThemeSnapshot,
    sameThemeSnapshot,
    VIEW_THEME_TOKENS,
} from '../../ui/js/extension-view/theme-snapshot.js';

function styleOf(values) {
    return { getPropertyValue: (name) => values[name] ?? '' };
}

const allow = () => true;

describe('computeThemeSnapshot', () => {
    it('lists exactly the §11 tokens', () => {
        expect(VIEW_THEME_TOKENS).toEqual([
            'kage-bg',
            'kage-bg-elevated',
            'kage-bg-input',
            'kage-text',
            'kage-text-bright',
            'kage-text-muted',
            'kage-border',
            'kage-border-subtle',
            'kage-accent',
            'kage-accent-hover',
            'kage-accent-active',
            'kage-accent-light',
            'kage-accent-glow',
            'kage-link',
            'kage-success',
            'kage-warning',
            'kage-danger',
            'kage-error',
            'kage-font',
            'kage-font-mono',
        ]);
    });

    it('reads the listed tokens only, trimmed and with whitespace collapsed', () => {
        const snap = computeThemeSnapshot({
            style: styleOf({
                '--kage-bg': ' #1a202c',
                '--kage-font': '\n        -apple-system, "Segoe UI",\n    sans-serif',
                '--kage-gradient': 'linear-gradient(red, blue)',
                '--kage-term-bg': 'rgba(0,0,0,.2)',
            }),
            scheme: 'dark',
            fontSize: 15,
            reducedMotion: false,
            isSafeTokenValue: allow,
        });
        expect(snap).toEqual({
            scheme: 'dark',
            fontSize: 15,
            reducedMotion: false,
            tokens: { 'kage-bg': '#1a202c', 'kage-font': '-apple-system, "Segoe UI", sans-serif' },
        });
    });

    it('drops a value isSafeTokenValue refuses, asking with the token name', () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const isSafe = vi.fn((name, value) => !/url\(|[;{}]/.test(value));
        const snap = computeThemeSnapshot({
            style: styleOf({
                '--kage-bg': 'url(https://example.com/x.png)',
                '--kage-text': '#fff; } body { display:none',
                '--kage-accent': '#319795',
            }),
            scheme: 'dark',
            isSafeTokenValue: isSafe,
        });
        expect(snap.tokens).toEqual({ 'kage-accent': '#319795' });
        expect(isSafe).toHaveBeenCalledWith('kage-bg', 'url(https://example.com/x.png)');
        expect(warn).toHaveBeenCalledTimes(2);
        warn.mockRestore();
    });

    it('fails closed when the checker throws or answers anything but true', () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const style = styleOf({ '--kage-bg': '#000', '--kage-text': '#fff' });
        expect(
            computeThemeSnapshot({
                style,
                scheme: 'dark',
                isSafeTokenValue: () => {
                    throw new Error('boom');
                },
            }).tokens
        ).toEqual({});
        expect(
            computeThemeSnapshot({ style, scheme: 'dark', isSafeTokenValue: () => 'yes' }).tokens
        ).toEqual({});
        warn.mockRestore();
    });

    it('normalises scheme, font size and motion', () => {
        const base = { style: styleOf({}), isSafeTokenValue: allow };
        expect(computeThemeSnapshot({ ...base, scheme: 'light' }).scheme).toBe('light');
        expect(computeThemeSnapshot({ ...base, scheme: 'sepia' }).scheme).toBe('dark');
        expect(computeThemeSnapshot({ ...base }).fontSize).toBe(DEFAULT_FONT_SIZE);
        expect(computeThemeSnapshot({ ...base, fontSize: 'x' }).fontSize).toBe(DEFAULT_FONT_SIZE);
        expect(computeThemeSnapshot({ ...base, fontSize: 200 }).fontSize).toBe(32);
        expect(computeThemeSnapshot({ ...base, fontSize: 13.6 }).fontSize).toBe(14);
        expect(computeThemeSnapshot({ ...base, reducedMotion: 1 }).reducedMotion).toBe(true);
    });

    it('survives a missing style', () => {
        const snap = computeThemeSnapshot({ style: null, scheme: 'dark', isSafeTokenValue: allow });
        expect(snap.tokens).toEqual({});
    });
});

describe('readThemeSnapshot', () => {
    it('reads the body (light overrides live on body.light-theme), the scheme and motion', () => {
        document.body.className = 'light-theme';
        const getComputedStyle = vi.fn(() => styleOf({ '--kage-bg': '#f7fafc' }));
        const win = {
            getComputedStyle,
            matchMedia: (q) => ({ matches: q === '(prefers-reduced-motion: reduce)' }),
        };
        const snap = readThemeSnapshot({
            doc: document,
            win,
            config: { ui: { font_size: 16 } },
            isSafeTokenValue: allow,
        });
        expect(getComputedStyle).toHaveBeenCalledWith(document.body);
        expect(snap).toEqual({
            scheme: 'light',
            fontSize: 16,
            reducedMotion: true,
            tokens: { 'kage-bg': '#f7fafc' },
        });
        document.body.className = '';
    });

    it('is dark without the light-theme class', () => {
        document.body.className = 'dark-theme';
        const snap = readThemeSnapshot({
            doc: document,
            win: { getComputedStyle: () => styleOf({}) },
            isSafeTokenValue: allow,
        });
        expect(snap.scheme).toBe('dark');
        expect(snap.reducedMotion).toBe(false);
        document.body.className = '';
    });
});

describe('sameThemeSnapshot', () => {
    const a = { scheme: 'dark', fontSize: 14, reducedMotion: false, tokens: { 'kage-bg': '#000' } };

    it('compares every field and token', () => {
        expect(sameThemeSnapshot(a, structuredClone(a))).toBe(true);
        expect(sameThemeSnapshot(a, { ...a, scheme: 'light' })).toBe(false);
        expect(sameThemeSnapshot(a, { ...a, fontSize: 15 })).toBe(false);
        expect(sameThemeSnapshot(a, { ...a, reducedMotion: true })).toBe(false);
        expect(sameThemeSnapshot(a, { ...a, tokens: { 'kage-bg': '#111' } })).toBe(false);
        expect(sameThemeSnapshot(a, { ...a, tokens: {} })).toBe(false);
        expect(
            sameThemeSnapshot(a, { ...a, tokens: { 'kage-bg': '#000', 'kage-text': '#fff' } })
        ).toBe(false);
        expect(sameThemeSnapshot(null, a)).toBe(false);
        expect(sameThemeSnapshot(null, null)).toBe(true);
    });
});
