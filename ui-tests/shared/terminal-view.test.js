/**
 * The xterm.js view (shared/terminal-view.js): loaded lazily, once; LTR and
 * labelled; keys typed into it never reach the window's own shortcuts
 * while it takes input, and once input is off it lets go of the keyboard;
 * its colours follow theme changes; input can be switched off.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
    _setTerminalFactoryForTests,
    createTerminalView,
    ensureXterm,
} from '../../ui/js/shared/terminal-view.js';

const flush = () => new Promise((r) => setTimeout(r, 0));

let terms;
function fakeTerminal(opts) {
    const term = {
        opts,
        cols: 80,
        rows: opts.rows,
        options: { theme: opts.theme },
        dataHandler: null,
        open: vi.fn((el) => {
            // Stands in for xterm's hidden textarea, which takes the keys.
            const ta = document.createElement('textarea');
            el.append(ta);
            term.textarea = ta;
        }),
        write: vi.fn(),
        focus: vi.fn(() => term.textarea?.focus()),
        blur: vi.fn(() => term.textarea?.blur()),
        dispose: vi.fn(),
        loadAddon: vi.fn(),
        keyHandler: null,
        attachCustomKeyEventHandler(fn) {
            term.keyHandler = fn;
        },
        onData(handler) {
            term.dataHandler = handler;
            return { dispose() {} };
        },
    };
    terms.push(term);
    return term;
}

beforeEach(() => {
    terms = [];
});

afterEach(() => {
    _setTerminalFactoryForTests(null);
    delete window.Terminal;
    document.head.innerHTML = '';
    document.body.innerHTML = '';
    document.body.className = '';
});

describe('ensureXterm', () => {
    it('injects xterm and its addons once, in order, with its stylesheet', async () => {
        const first = ensureXterm();
        const second = ensureXterm();
        expect(second).toBe(first);
        const scripts = Array.from(document.head.querySelectorAll('script[data-kage-xterm]'));
        expect(scripts.map((s) => s.getAttribute('src'))).toEqual([
            'vendor/lib/xterm/xterm.js',
            'vendor/lib/xterm/addon-fit.js',
            'vendor/lib/xterm/addon-web-links.js',
        ]);
        expect(scripts.every((s) => s.async === false)).toBe(true);
        expect(document.head.querySelectorAll('link[data-kage-xterm]').length).toBe(1);
        window.Terminal = function Terminal() {};
        for (const s of scripts) s.dispatchEvent(new Event('load'));
        expect(await first).toBe(true);
        // Loaded: no more tags.
        expect(await ensureXterm()).toBe(true);
        expect(document.head.querySelectorAll('script[data-kage-xterm]').length).toBe(3);
    });

    it('a failed load resolves false and can be retried', async () => {
        const p = ensureXterm();
        const [script] = document.head.querySelectorAll('script[data-kage-xterm]');
        script.dispatchEvent(new Event('error'));
        expect(await p).toBe(false);
        expect(document.head.querySelectorAll('script[data-kage-xterm]').length).toBe(0);
        ensureXterm();
        expect(document.head.querySelectorAll('script[data-kage-xterm]').length).toBe(3);
    });
});

describe('createTerminalView', () => {
    beforeEach(() => _setTerminalFactoryForTests(fakeTerminal));

    it('is LTR, labelled, and set up without window reports', () => {
        const box = document.createElement('div');
        document.body.append(box);
        createTerminalView(box, { rows: 10, ariaLabel: 'Sign-in terminal' });
        expect(box.dir).toBe('ltr');
        expect(box.getAttribute('aria-label')).toBe('Sign-in terminal');
        const { opts } = terms[0];
        expect(opts.rows).toBe(10);
        expect(opts.scrollback).toBe(1000);
        expect(opts.allowProposedApi).toBe(false);
        expect(opts.windowOptions).toEqual({});
        expect(opts.linkHandler.allowNonHttpProtocols).toBe(false);
        expect(terms[0].open).toHaveBeenCalledWith(box);
    });

    it("keeps Escape and the window's shortcuts inside the terminal", () => {
        const box = document.createElement('div');
        document.body.append(box);
        createTerminalView(box, {});
        const seen = vi.fn();
        document.addEventListener('keydown', seen);
        try {
            terms[0].textarea.dispatchEvent(
                new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })
            );
            terms[0].textarea.dispatchEvent(
                new KeyboardEvent('keydown', { key: ',', metaKey: true, bubbles: true })
            );
            expect(seen).not.toHaveBeenCalled();
            // Outside the terminal, keys still reach the window.
            document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
            expect(seen).toHaveBeenCalledTimes(1);
        } finally {
            document.removeEventListener('keydown', seen);
        }
    });

    it('re-applies its theme when the window switches dark/light', async () => {
        const box = document.createElement('div');
        document.body.append(box);
        createTerminalView(box, {});
        const before = terms[0].options.theme;
        document.body.classList.add('dark-theme');
        await flush();
        expect(terms[0].options.theme).not.toBe(before);
        expect(terms[0].options.theme.background).toBe('rgba(0, 0, 0, 0)');
    });

    it('passes typing on until input is switched off; disposes cleanly', () => {
        const box = document.createElement('div');
        document.body.append(box);
        const onData = vi.fn();
        const view = createTerminalView(box, { onData });
        terms[0].dataHandler('a');
        view.setInputEnabled(false);
        terms[0].dataHandler('b');
        expect(onData.mock.calls).toEqual([['a']]);
        expect(terms[0].options.disableStdin).toBe(true);
        view.write('hello');
        expect(terms[0].write).toHaveBeenCalledWith('hello');
        view.dispose();
        expect(terms[0].dispose).toHaveBeenCalled();
        view.write('late');
        expect(terms[0].write).toHaveBeenCalledTimes(1);
        expect(box.children.length).toBe(0);
    });

    it('with input off it gives the keyboard back: focus, Esc and Tab', () => {
        const box = document.createElement('div');
        document.body.append(box);
        const view = createTerminalView(box, {});
        const windowKeys = vi.fn();
        document.addEventListener('keydown', windowKeys);
        try {
            view.focus();
            expect(document.activeElement).toBe(terms[0].textarea);
            const esc = () =>
                terms[0].textarea.dispatchEvent(
                    new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })
                );
            esc();
            expect(windowKeys).not.toHaveBeenCalled();
            // xterm handles keys (and so swallows Tab) only while input is on.
            expect(terms[0].keyHandler(new KeyboardEvent('keydown', { key: 'Tab' }))).toBe(true);

            view.setInputEnabled(false);
            expect(terms[0].blur).toHaveBeenCalled();
            expect(box.contains(document.activeElement)).toBe(false);
            expect(terms[0].keyHandler(new KeyboardEvent('keydown', { key: 'Tab' }))).toBe(false);
            esc();
            expect(windowKeys).toHaveBeenCalledTimes(1);
        } finally {
            document.removeEventListener('keydown', windowKeys);
        }
    });

    it('switching input off while focus is elsewhere leaves focus alone', () => {
        const box = document.createElement('div');
        const other = document.createElement('input');
        document.body.append(box, other);
        const view = createTerminalView(box, {});
        other.focus();
        view.setInputEnabled(false);
        expect(terms[0].blur).not.toHaveBeenCalled();
        expect(document.activeElement).toBe(other);
    });
});
