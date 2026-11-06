/**
 * The context-menu window: renders each menu it's sent, reports its size
 * so the backend can fit and place the window, and answers the requesting
 * window with the choice. Protocol: ui/js/shared/popup-menu.js.
 */

import { EVT } from '../shared/events.js';
import { DISMISSED, defaultTextMenuItems, normalizeMenuItems } from '../shared/popup-menu.js';
import { PopupMenuView } from '../shared/popup-menu-view.js';
import { isMac } from '../shared/shortcuts.js';

/** Body padding around the menu (room for its shadow), per side, CSS px. */
export const MENU_PADDING = 8;
/** A blur this long without the focus coming back dismisses the menu. */
export const BLUR_DISMISS_MS = 150;

/**
 * @param {object} deps
 * @param {Function} deps.invoke
 * @param {Function} deps.listen    global event listen
 * @param {Function} deps.emitTo    (label, event, payload) => Promise
 * @param {object}   deps.appWindow this window (hide, listen)
 * @param {HTMLElement} deps.root   the `.menu` element
 * @param {Function} deps.t         i18n lookup
 * @param {() => boolean} [deps.devMode]
 * @param {() => boolean} [deps.isRtl]
 * @param {boolean} [deps.mac]      macOS (Ctrl+click is a right click)
 */
export function startMenuWindow({
    invoke,
    listen,
    emitTo,
    appWindow,
    root,
    t,
    devMode = () => false,
    isRtl = () => false,
    mac = isMac(),
}) {
    let current = null; // render payload on screen (or about to be)
    let lastToken = 0;
    let answered = false;
    let blurTimer = null;

    const cancelBlur = () => {
        clearTimeout(blurTimer);
        blurTimer = null;
    };

    async function answer(action) {
        if (!current || answered) return;
        answered = true;
        cancelBlur();
        const answering = current;
        const { token, owner, context } = answering;
        try {
            await emitTo(owner, EVT.CONTEXT_MENU_ACTION, {
                token,
                action,
                context: context ?? null,
            });
        } catch (e) {
            console.warn('[context-menu] failed to send the choice:', e);
        }
        // A newer menu may have arrived while the answer was in flight
        // (the caller opened another one); hiding now would hide that.
        if (current !== answering || current.token !== token) return;
        // The hide carries its token: the backend serialises it with
        // presenting and ignores it once a newer menu is up, so a hide
        // still in flight when the next menu shows can't take that down.
        try {
            await invoke('hide_context_menu', { token });
        } catch (e) {
            console.warn('[context-menu] hide_context_menu failed:', e);
            // Never leave a finished menu on screen.
            if (current === answering) {
                try {
                    await appWindow.hide();
                } catch {}
            }
        }
    }

    const view = new PopupMenuView(root, {
        t,
        onChoose: (id) => answer(id),
        onDismiss: () => answer(DISMISSED),
        mac,
    });

    async function render(payload) {
        // The pushed event and the boot-time pull can both deliver a menu;
        // render each token once, and never an older one.
        if (!payload || typeof payload.token !== 'number' || payload.token <= lastToken) return;
        lastToken = payload.token;
        current = payload;
        answered = false;
        cancelBlur();
        const items = Array.isArray(payload.items)
            ? normalizeMenuItems(payload.items)
            : defaultTextMenuItems({ devMode: devMode() });
        const { width, height } = view.show(items, { keyboard: !!payload.keyboard });
        try {
            await invoke('context_menu_ready', {
                token: payload.token,
                width: width + 2 * MENU_PADDING,
                height: height + 2 * MENU_PADDING,
                rtl: !!isRtl(),
            });
        } catch (e) {
            // Never leave the caller waiting on a menu that can't show.
            console.error('[context-menu] context_menu_ready failed:', e);
            if (current === payload) await answer(DISMISSED);
        }
    }

    listen(EVT.CONTEXT_MENU_RENDER, (event) => render(event?.payload));

    // Cancelled on focus so a quick re-show (which re-focuses) isn't
    // dismissed by the previous blur's stale timer.
    appWindow.listen('tauri://focus', cancelBlur);
    appWindow.listen('tauri://blur', () => {
        cancelBlur();
        const blurred = current;
        blurTimer = setTimeout(() => {
            blurTimer = null;
            if (current === blurred) answer(DISMISSED);
        }, BLUR_DISMISS_MS);
    });

    document.addEventListener('keydown', (e) => {
        if (current && !answered) view.handleKey(e);
    });
    document.addEventListener('contextmenu', (e) => e.preventDefault());

    // A menu requested while this window was still loading.
    Promise.resolve(invoke('get_pending_context_menu'))
        .then(render)
        .catch(() => {});

    return { render, answer, view };
}
