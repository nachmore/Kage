/**
 * Right-click menu for the floating window, shown in the cached popup
 * window (ui/js/shared/popup-menu.js has the protocol).
 *
 * - On an app / file / folder result row: that result's menu (Open, Show
 *   in Finder / Open file location, Copy, Uninstall, …), built from what
 *   the backend says the result supports on this OS.
 * - On a misspelled word in the launcher input: the spell checker's
 *   suggestions and Learn spelling above the text menu (spell-menu.js).
 * - Anywhere else, or for any other kind of row: the text menu (Cut, Copy,
 *   Paste, Settings, Close, and Inspect in dev mode), as always.
 * - Keyboard: Shift+F10 or the Menu key opens the selected result's menu
 *   (Windows/Linux; macOS has no such convention).
 *
 * Tauri readiness:
 *   `window.__TAURI__` is injected by the runtime (`withGlobalTauri: true`),
 *   but in release builds the module bundle can hit the parser before the
 *   injection lands — destructuring `__TAURI__` at module top-level then
 *   throws and aborts the whole `<script type="module">` graph, taking
 *   `main.js` (and `notify_frontend_ready`) down with it. Defer all
 *   `__TAURI__` access until `waitForTauri` confirms it's there.
 */

import { waitForTauri } from '../shared/tauri-init.js';
import { DISMISSED, createPopupMenuClient, isMenuKey } from '../shared/popup-menu.js';
import { buildResultMenuItems, resultMenuTarget } from '../shared/result-menu.js';
import { isMac } from '../shared/shortcuts.js';
import {
    SPELL_LOOKUP_TIMEOUT_MS,
    cleanSuggestions,
    findSpellWord,
    inputOffsetAtPoint,
    replaceWord,
    spellChoice,
    spellMenuItems,
    withTimeout,
} from '../shared/spell-menu.js';

waitForTauri(({ invoke, appWindow, listen }) => {
    initContextMenu(invoke, appWindow, listen);
});

/** After a keyboard-opened menu, the browser may still fire `contextmenu`. */
const KEYBOARD_CONTEXTMENU_SWALLOW_MS = 600;

function initContextMenu(invoke, appWindow, listen) {
    const menu = createPopupMenuClient({ invoke, listen });
    const mac = isMac();
    // Dev mode adds Inspect to the text menu; the spelling menu lists the
    // text items itself, so it needs to know too. Checked once.
    let devMode = false;
    Promise.resolve(invoke('is_dev_mode'))
        .then((v) => {
            devMode = !!v;
        })
        .catch(() => {});
    // Menus being prepared or shown. `window._contextMenuOpen` (read by the
    // floating window's blur handler) stays true while any is, so a menu
    // replacing another doesn't briefly let the launcher hide.
    let busy = 0;
    const hold = () => {
        busy++;
        window._contextMenuOpen = true;
    };
    const release = () => {
        busy = Math.max(0, busy - 1);
        window._contextMenuOpen = busy > 0 || menu.isOpen();
    };
    let swallowUntil = 0;

    /**
     * A point in this window's CSS px → screen coordinates in this window's
     * logical units (position ÷ scale + offset). The backend multiplies a
     * keyboard anchor by this window's scale again on Windows/Linux, so a
     * mixed-DPI setup still lands on the right monitor.
     */
    async function screenPoint(clientX, clientY) {
        const pos = await appWindow.outerPosition();
        const scale = await appWindow.scaleFactor();
        return { x: pos.x / scale + clientX, y: pos.y / scale + clientY };
    }

    /** Show `result`'s menu; false if it has none (caller falls back). */
    async function openResultMenu(result, point, keyboard) {
        const app = window._floatingApp;
        const target = resultMenuTarget(result);
        if (!app || !target) return false;
        hold();
        try {
            let info;
            try {
                info = await invoke('launcher_result_actions', target);
            } catch (e) {
                console.warn('[context-menu] no result menu:', e);
                return false;
            }
            const items = buildResultMenuItems(info);
            if (!items.length) return false;
            app.selectResultRow?.(result);
            const choice = await menu.open({ ...point, items, context: target, keyboard });
            window._contextMenuOpen = false;
            if (choice.action !== DISMISSED) {
                await app.runResultAction(target, info, choice.action);
            }
            return true;
        } finally {
            release();
        }
    }

    /**
     * Start looking up the right-clicked word in the launcher input, if
     * the click was on one: `{ input, range, lookup }`, where `lookup`
     * resolves to the suggestions when the word is misspelled, else (or
     * if the backend takes too long) `null`. Synchronous up to the
     * invoke, so it reads the selection as it was at the click.
     */
    function startSpellLookup(e) {
        const input = e.target?.closest?.('.input-box');
        if (!input || typeof input.value !== 'string' || input.spellcheck === false) return null;
        const range = findSpellWord({
            text: input.value,
            selectionStart: input.selectionStart,
            selectionEnd: input.selectionEnd,
            pointOffset: inputOffsetAtPoint(input, e.clientX, e.clientY),
        });
        if (!range) return null;
        const asked = Promise.resolve(invoke('spell_suggestions', { word: range.word })).then(
            (r) => (r?.misspelled ? cleanSuggestions(r.suggestions) : null)
        );
        return { input, range, lookup: withTimeout(asked, SPELL_LOOKUP_TIMEOUT_MS, null) };
    }

    /** Show the spelling menu; false if the word is fine (caller falls back). */
    async function openSpellMenu(spell, point) {
        const suggestions = await spell.lookup;
        if (!suggestions) return false;
        hold();
        try {
            const items = spellMenuItems(suggestions, { devMode });
            const choice = await menu.open({ ...point, items });
            window._contextMenuOpen = false;
            const pick = spellChoice(choice.action, suggestions);
            if (pick?.replace !== undefined) {
                replaceWord(spell.input, spell.range, pick.replace);
            } else if (pick?.learn) {
                await learnSpelling(spell);
            } else {
                await runTextAction(choice.action);
            }
            return true;
        } finally {
            release();
        }
    }

    async function learnSpelling({ input, range }) {
        try {
            await invoke('learn_spelling', { word: range.word });
        } catch (e) {
            console.warn('[context-menu] learn_spelling failed:', e);
            return;
        }
        // WebKit re-checks text when it's edited, not when the dictionary
        // changes: put the same word back so its underline goes.
        replaceWord(input, range, range.word);
    }

    async function openTextMenu(point) {
        hold();
        try {
            const choice = await menu.open(point);
            window._contextMenuOpen = false;
            await runTextAction(choice.action);
        } finally {
            release();
        }
    }

    document.addEventListener('contextmenu', async (e) => {
        e.preventDefault();
        if (Date.now() < swallowUntil) return;
        // Guard the blur handler before the first await: the click that
        // opens the menu can blur this window.
        hold();
        try {
            // Before the first await: the lookup runs alongside the
            // position query, and reads the selection as of the click.
            const spell = startSpellLookup(e);
            const point = await screenPoint(e.clientX, e.clientY);
            const row = e.target?.closest?.('.app-suggestion-item');
            if (row?._kageResult && (await openResultMenu(row._kageResult, point, false))) {
                return;
            }
            if (spell && (await openSpellMenu(spell, point))) return;
            await openTextMenu(point);
        } catch (err) {
            console.error('Failed to show context menu:', err);
        } finally {
            release();
        }
    });

    // Capture phase, so it runs before the input's own key handling.
    document.addEventListener(
        'keydown',
        async (e) => {
            if (!isMenuKey(e, { mac })) return;
            const app = window._floatingApp;
            const idx = app?.selectedIndex ?? -1;
            const result = idx >= 0 ? app?.currentMatches?.[idx] : null;
            const row =
                app?.elements?.appSuggestions?.querySelectorAll('.app-suggestion-item')[idx];
            // Nothing selected that has a menu: the browser's own
            // contextmenu event brings up the text menu.
            if (!row || !resultMenuTarget(result)) return;
            e.preventDefault();
            e.stopPropagation();
            swallowUntil = Date.now() + KEYBOARD_CONTEXTMENU_SWALLOW_MS;
            hold();
            try {
                // Under the row, at its start edge (the backend aligns an
                // RTL menu's right edge there).
                const rect = row.getBoundingClientRect();
                const rtl = document.documentElement.getAttribute('dir') === 'rtl';
                const point = await screenPoint(
                    rtl ? rect.right - 12 : rect.left + 12,
                    rect.bottom
                );
                await openResultMenu(result, point, true);
            } catch (err) {
                console.error('Failed to show result menu:', err);
            } finally {
                release();
            }
        },
        true
    );

    async function runTextAction(action) {
        switch (action) {
            case 'cut':
                document.querySelector('.input-box')?.focus();
                document.execCommand('cut');
                break;
            case 'copy':
                if (window.getSelection().toString()) {
                    document.execCommand('copy');
                } else {
                    const responseEl = document.getElementById('responseText');
                    if (responseEl?.textContent) {
                        navigator.clipboard.writeText(responseEl.textContent).catch(() => {});
                    }
                }
                break;
            case 'paste':
                try {
                    const text = await invoke('read_clipboard');
                    const input = document.querySelector('.input-box');
                    if (input && text) {
                        input.focus();
                        const start = input.selectionStart;
                        const end = input.selectionEnd;
                        input.value =
                            input.value.substring(0, start) + text + input.value.substring(end);
                        input.selectionStart = input.selectionEnd = start + text.length;
                        input.dispatchEvent(new Event('input', { bubbles: true }));
                    }
                } catch (e) {
                    console.error('Paste failed:', e);
                }
                break;
            case 'settings':
                try {
                    await invoke('open_settings_window');
                    await appWindow.hide();
                } catch (e) {
                    console.error(e);
                }
                break;
            case 'close':
                await appWindow.hide();
                break;
            case 'inspect':
                try {
                    await invoke('open_devtools');
                } catch (e) {
                    console.error(e);
                }
                break;
            default:
                // DISMISSED: menu closed without a choice.
                break;
        }
    }
}
