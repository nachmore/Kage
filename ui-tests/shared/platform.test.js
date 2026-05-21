/**
 * Tests for platform helpers exported from `shared/shortcuts.js`.
 *
 * shortcuts.js exports `isMac`, `isWindows`, `isLinux`,
 * `cmdOrCtrlPressed`, and `platformKeyLabel`. These are used everywhere
 * in the UI to make the keyboard surface feel native on each OS.
 *
 * (Earlier revisions of the codebase also kept a parallel
 * `shared/platform-global.js` classic-script mirror so non-module
 * windows could call the same helpers via `window.kagePlatform`. We
 * deleted it once every window switched to ES modules — see the
 * single-module-loading-style refactor.)
 */

import { describe, it, expect, vi } from 'vitest';

// shortcuts.js caches isMac() on first call. We need a fresh module
// per platform permutation to re-evaluate navigator.platform.
async function importShortcutsFresh() {
    vi.resetModules();
    return await import('../../ui/js/shared/shortcuts.js');
}

function setPlatform(value) {
    Object.defineProperty(navigator, 'platform', {
        value,
        configurable: true,
    });
}

describe('isMac', () => {
    it('returns true on MacIntel', async () => {
        setPlatform('MacIntel');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.isMac()).toBe(true);
    });

    it('returns false on Win32', async () => {
        setPlatform('Win32');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.isMac()).toBe(false);
    });

    it('returns false on Linux x86_64', async () => {
        setPlatform('Linux x86_64');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.isMac()).toBe(false);
    });
});

describe('isWindows / isLinux', () => {
    // isWindows and isLinux gate OS-specific settings UI (mac permissions,
    // Windows clipboard history, etc.). They must be mutually exclusive and
    // together with isMac cover every platform the WebView might report.

    it('classifies Windows correctly', async () => {
        setPlatform('Win32');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.isWindows()).toBe(true);
        expect(shortcuts.isMac()).toBe(false);
        expect(shortcuts.isLinux()).toBe(false);
    });

    it('classifies macOS correctly', async () => {
        setPlatform('MacIntel');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.isWindows()).toBe(false);
        expect(shortcuts.isMac()).toBe(true);
        expect(shortcuts.isLinux()).toBe(false);
    });

    it('classifies Linux as the fallthrough', async () => {
        setPlatform('Linux x86_64');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.isWindows()).toBe(false);
        expect(shortcuts.isMac()).toBe(false);
        expect(shortcuts.isLinux()).toBe(true);
    });

    it('treats unknown platforms as Linux (most conservative)', async () => {
        setPlatform('FreeBSD');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.isLinux()).toBe(true);
    });
});

describe('cmdOrCtrlPressed', () => {
    it('accepts ctrlKey on Windows', async () => {
        setPlatform('Win32');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.cmdOrCtrlPressed({ ctrlKey: true, metaKey: false })).toBe(true);
    });

    it('rejects metaKey-only on Windows (Win+key is OS-intercepted)', async () => {
        setPlatform('Win32');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.cmdOrCtrlPressed({ ctrlKey: false, metaKey: true })).toBe(false);
    });

    it('rejects metaKey-only on Linux (Super+key is WM binding)', async () => {
        setPlatform('Linux x86_64');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.cmdOrCtrlPressed({ ctrlKey: false, metaKey: true })).toBe(false);
    });

    it('accepts both ctrlKey and metaKey on Mac', async () => {
        setPlatform('MacIntel');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.cmdOrCtrlPressed({ ctrlKey: true, metaKey: false })).toBe(true);
        expect(shortcuts.cmdOrCtrlPressed({ ctrlKey: false, metaKey: true })).toBe(true);
    });

    it('rejects when no modifier is pressed', async () => {
        setPlatform('MacIntel');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.cmdOrCtrlPressed({ ctrlKey: false, metaKey: false })).toBe(false);
    });
});

describe('platformKeyLabel', () => {
    it('returns input unchanged on non-mac', async () => {
        setPlatform('Win32');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.platformKeyLabel('Ctrl+N')).toBe('Ctrl+N');
        expect(shortcuts.platformKeyLabel('Ctrl+Shift+C')).toBe('Ctrl+Shift+C');
    });

    it('translates Ctrl → ⌘ on mac', async () => {
        setPlatform('MacIntel');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.platformKeyLabel('Ctrl+N')).toBe('\u2318N');
    });

    it('translates Shift → ⇧, Enter → ⏎, Escape → ⎋ on mac', async () => {
        setPlatform('MacIntel');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.platformKeyLabel('Ctrl+Shift+C')).toBe('\u2318\u21E7C');
        expect(shortcuts.platformKeyLabel('Ctrl+Enter')).toBe('\u2318\u23CE');
        expect(shortcuts.platformKeyLabel('Escape')).toBe('\u238B');
    });

    it('translates Alt → ⌥, Backspace → ⌫, Tab → ⇥ on mac', async () => {
        setPlatform('MacIntel');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.platformKeyLabel('Alt+F')).toBe('\u2325F');
        expect(shortcuts.platformKeyLabel('Backspace')).toBe('\u232B');
        expect(shortcuts.platformKeyLabel('Tab')).toBe('\u21E5');
    });

    it('passes unknown tokens through verbatim on mac', async () => {
        setPlatform('MacIntel');
        const shortcuts = await importShortcutsFresh();
        // Someone typed Win+X — we don't recognize it, leave it alone.
        expect(shortcuts.platformKeyLabel('Win+X')).toBe('WinX');
    });

    it('treats Cmd/Super/Meta as synonyms for the ⌘ glyph on mac', async () => {
        setPlatform('MacIntel');
        const shortcuts = await importShortcutsFresh();
        expect(shortcuts.platformKeyLabel('Cmd+N')).toBe('\u2318N');
        expect(shortcuts.platformKeyLabel('Super+N')).toBe('\u2318N');
        expect(shortcuts.platformKeyLabel('Meta+N')).toBe('\u2318N');
    });
});
