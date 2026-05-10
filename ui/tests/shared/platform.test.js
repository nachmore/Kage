/**
 * Tests for platform helpers — both the ESM exports in shared/shortcuts.js
 * and the classic-script mirror in shared/platform-global.js. These are
 * parallel implementations (see the "keep in sync" headers) so we verify
 * identical behavior across both.
 */

import { readFileSync } from 'fs';
import { resolve } from 'path';

// Load the classic-script platform-global.js into the jsdom context.
function loadPlatformGlobal() {
    const src = readFileSync(
        resolve(__dirname, '../../js/shared/platform-global.js'),
        'utf-8',
    );
    new Function(src)();
}

// The ESM shortcuts.js caches isMac() on first call. We need a fresh module
// per platform permutation to re-evaluate navigator.platform.
async function importShortcutsFresh() {
    vi.resetModules();
    return await import('../../js/shared/shortcuts.js');
}

function setPlatform(value) {
    Object.defineProperty(navigator, 'platform', {
        value,
        configurable: true,
    });
}

beforeEach(() => {
    delete window.kagePlatform;
});

describe('isMac (both impls)', () => {
    it('returns true on MacIntel', async () => {
        setPlatform('MacIntel');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        expect(window.kagePlatform.isMac()).toBe(true);
        expect(shortcuts.isMac()).toBe(true);
    });

    it('returns false on Win32', async () => {
        setPlatform('Win32');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        expect(window.kagePlatform.isMac()).toBe(false);
        expect(shortcuts.isMac()).toBe(false);
    });

    it('returns false on Linux x86_64', async () => {
        setPlatform('Linux x86_64');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        expect(window.kagePlatform.isMac()).toBe(false);
        expect(shortcuts.isMac()).toBe(false);
    });
});

describe('isWindows / isLinux (both impls)', () => {
    // isWindows and isLinux gate OS-specific settings UI (mac permissions,
    // Windows clipboard history, etc.). They must be mutually exclusive and
    // together with isMac cover every platform the WebView might report.

    it('classifies Windows correctly', async () => {
        setPlatform('Win32');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        expect(window.kagePlatform.isWindows()).toBe(true);
        expect(window.kagePlatform.isMac()).toBe(false);
        expect(window.kagePlatform.isLinux()).toBe(false);
        expect(shortcuts.isWindows()).toBe(true);
        expect(shortcuts.isLinux()).toBe(false);
    });

    it('classifies macOS correctly', async () => {
        setPlatform('MacIntel');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        expect(window.kagePlatform.isWindows()).toBe(false);
        expect(window.kagePlatform.isMac()).toBe(true);
        expect(window.kagePlatform.isLinux()).toBe(false);
        expect(shortcuts.isWindows()).toBe(false);
        expect(shortcuts.isLinux()).toBe(false);
    });

    it('classifies Linux as the fallthrough', async () => {
        setPlatform('Linux x86_64');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        expect(window.kagePlatform.isWindows()).toBe(false);
        expect(window.kagePlatform.isMac()).toBe(false);
        expect(window.kagePlatform.isLinux()).toBe(true);
        expect(shortcuts.isWindows()).toBe(false);
        expect(shortcuts.isLinux()).toBe(true);
    });

    it('treats unknown platforms as Linux (most conservative)', async () => {
        setPlatform('FreeBSD');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        expect(window.kagePlatform.isLinux()).toBe(true);
        expect(shortcuts.isLinux()).toBe(true);
    });
});

describe('cmdOrCtrlPressed (both impls)', () => {
    it('accepts ctrlKey on Windows', async () => {
        setPlatform('Win32');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        const ev = { ctrlKey: true, metaKey: false };
        expect(window.kagePlatform.cmdOrCtrlPressed(ev)).toBe(true);
        expect(shortcuts.cmdOrCtrlPressed(ev)).toBe(true);
    });

    it('rejects metaKey-only on Windows (Win+key is OS-intercepted)', async () => {
        setPlatform('Win32');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        const ev = { ctrlKey: false, metaKey: true };
        expect(window.kagePlatform.cmdOrCtrlPressed(ev)).toBe(false);
        expect(shortcuts.cmdOrCtrlPressed(ev)).toBe(false);
    });

    it('rejects metaKey-only on Linux (Super+key is WM binding)', async () => {
        setPlatform('Linux x86_64');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        const ev = { ctrlKey: false, metaKey: true };
        expect(window.kagePlatform.cmdOrCtrlPressed(ev)).toBe(false);
        expect(shortcuts.cmdOrCtrlPressed(ev)).toBe(false);
    });

    it('accepts both ctrlKey and metaKey on Mac', async () => {
        setPlatform('MacIntel');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        expect(window.kagePlatform.cmdOrCtrlPressed({ ctrlKey: true, metaKey: false })).toBe(true);
        expect(window.kagePlatform.cmdOrCtrlPressed({ ctrlKey: false, metaKey: true })).toBe(true);
        expect(shortcuts.cmdOrCtrlPressed({ ctrlKey: true, metaKey: false })).toBe(true);
        expect(shortcuts.cmdOrCtrlPressed({ ctrlKey: false, metaKey: true })).toBe(true);
    });

    it('rejects when no modifier is pressed', async () => {
        setPlatform('MacIntel');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        const ev = { ctrlKey: false, metaKey: false };
        expect(window.kagePlatform.cmdOrCtrlPressed(ev)).toBe(false);
        expect(shortcuts.cmdOrCtrlPressed(ev)).toBe(false);
    });
});

describe('platformKeyLabel (both impls)', () => {
    it('returns input unchanged on non-mac', async () => {
        setPlatform('Win32');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        expect(window.kagePlatform.platformKeyLabel('Ctrl+N')).toBe('Ctrl+N');
        expect(window.kagePlatform.platformKeyLabel('Ctrl+Shift+C')).toBe('Ctrl+Shift+C');
        expect(shortcuts.platformKeyLabel('Ctrl+N')).toBe('Ctrl+N');
        expect(shortcuts.platformKeyLabel('Ctrl+Shift+C')).toBe('Ctrl+Shift+C');
    });

    it('translates Ctrl → ⌘ on mac', async () => {
        setPlatform('MacIntel');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        expect(window.kagePlatform.platformKeyLabel('Ctrl+N')).toBe('\u2318N');
        expect(shortcuts.platformKeyLabel('Ctrl+N')).toBe('\u2318N');
    });

    it('translates Shift → ⇧, Enter → ⏎, Escape → ⎋ on mac', async () => {
        setPlatform('MacIntel');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        expect(window.kagePlatform.platformKeyLabel('Ctrl+Shift+C')).toBe('\u2318\u21E7C');
        expect(window.kagePlatform.platformKeyLabel('Ctrl+Enter')).toBe('\u2318\u23CE');
        expect(window.kagePlatform.platformKeyLabel('Escape')).toBe('\u238B');
        expect(shortcuts.platformKeyLabel('Ctrl+Shift+C')).toBe('\u2318\u21E7C');
        expect(shortcuts.platformKeyLabel('Ctrl+Enter')).toBe('\u2318\u23CE');
        expect(shortcuts.platformKeyLabel('Escape')).toBe('\u238B');
    });

    it('translates Alt → ⌥, Backspace → ⌫, Tab → ⇥ on mac', async () => {
        setPlatform('MacIntel');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        expect(window.kagePlatform.platformKeyLabel('Alt+F')).toBe('\u2325F');
        expect(window.kagePlatform.platformKeyLabel('Backspace')).toBe('\u232B');
        expect(window.kagePlatform.platformKeyLabel('Tab')).toBe('\u21E5');
        expect(shortcuts.platformKeyLabel('Alt+F')).toBe('\u2325F');
        expect(shortcuts.platformKeyLabel('Backspace')).toBe('\u232B');
        expect(shortcuts.platformKeyLabel('Tab')).toBe('\u21E5');
    });

    it('passes unknown tokens through verbatim on mac', async () => {
        setPlatform('MacIntel');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        // Someone typed Win+X — we don't recognize it, leave it alone.
        expect(window.kagePlatform.platformKeyLabel('Win+X')).toBe('WinX');
        expect(shortcuts.platformKeyLabel('Win+X')).toBe('WinX');
    });

    it('treats Cmd/Super/Meta as synonyms for the ⌘ glyph on mac', async () => {
        setPlatform('MacIntel');
        loadPlatformGlobal();
        const shortcuts = await importShortcutsFresh();
        expect(window.kagePlatform.platformKeyLabel('Cmd+N')).toBe('\u2318N');
        expect(window.kagePlatform.platformKeyLabel('Super+N')).toBe('\u2318N');
        expect(window.kagePlatform.platformKeyLabel('Meta+N')).toBe('\u2318N');
        expect(shortcuts.platformKeyLabel('Cmd+N')).toBe('\u2318N');
        expect(shortcuts.platformKeyLabel('Super+N')).toBe('\u2318N');
        expect(shortcuts.platformKeyLabel('Meta+N')).toBe('\u2318N');
    });
});
