/**
 * Generic popup (right-click) menu — the caller side, plus the pure item
 * helpers the menu window shares. Any window can use it: describe the
 * items, get back the chosen item id (or DISMISSED) with your context.
 *
 * Protocol (src/commands/window/context_menu.rs):
 *   caller  → show_context_menu { x, y, menu: { items, context, keyboard } } → token
 *   backend → menu window: context_menu_render { token, owner, items, context, keyboard }
 *   menu    → context_menu_ready { token, width, height, rtl }   (sized, placed, shown)
 *   menu    → owner: context_menu_action { token, action, context }
 *
 * Item: { id, labelKey? | label?, labelVars?, shortcut?, enabled?, danger? }
 *       or { separator: true }. `items` omitted → the default text menu.
 */

import { EVT } from './events.js';

/** `action` when the menu closed without a choice. */
export const DISMISSED = 'dismissed';

export const SEPARATOR = Object.freeze({ separator: true });

const ID_RE = /^[A-Za-z0-9_.:-]{1,64}$/;
const KEY_RE = /^[a-z0-9_.]{1,128}$/;

/** Selectable: not a separator, not disabled. */
export function isActionable(item) {
    return !!item && !item.separator && item.enabled !== false;
}

/**
 * Drop malformed items (no valid id, no label), default `enabled` to true,
 * and collapse separators so none leads, trails or doubles up. Mirrors the
 * backend's `sanitize_menu_items` so both sides agree on what renders.
 */
export function normalizeMenuItems(items) {
    const out = [];
    for (const raw of Array.isArray(items) ? items : []) {
        if (!raw || typeof raw !== 'object') continue;
        if (raw.separator) {
            if (out.length && !out[out.length - 1].separator) out.push({ separator: true });
            continue;
        }
        if (typeof raw.id !== 'string' || !ID_RE.test(raw.id)) continue;
        const labelKey =
            typeof raw.labelKey === 'string' && KEY_RE.test(raw.labelKey) ? raw.labelKey : null;
        const label = typeof raw.label === 'string' && raw.label.trim() ? raw.label.trim() : null;
        if (!labelKey && !label) continue;
        const item = { id: raw.id, enabled: raw.enabled !== false, danger: !!raw.danger };
        if (labelKey) item.labelKey = labelKey;
        if (label) item.label = label;
        if (raw.labelVars && typeof raw.labelVars === 'object') item.labelVars = raw.labelVars;
        if (typeof raw.shortcut === 'string' && raw.shortcut.trim()) {
            item.shortcut = raw.shortcut.trim();
        }
        out.push(item);
    }
    while (out.length && out[out.length - 1].separator) out.pop();
    return out;
}

/**
 * The text-editing menu shown when nothing more specific was right-clicked.
 * Same items, order and labels the menu always had; Inspect only in dev mode.
 */
export function defaultTextMenuItems({ devMode = false } = {}) {
    const item = (id) => ({ id, labelKey: `context_menu.${id}`, enabled: true });
    const items = [
        item('cut'),
        item('copy'),
        item('paste'),
        SEPARATOR,
        item('settings'),
        item('close'),
    ];
    if (devMode) items.push(SEPARATOR, item('inspect'));
    return items.map((i) => (i.separator ? { separator: true } : i));
}

/** Index of the first selectable item, or -1. */
export function firstActionable(items) {
    return items.findIndex(isActionable);
}

/** Index of the last selectable item, or -1. */
export function lastActionable(items) {
    for (let i = items.length - 1; i >= 0; i--) if (isActionable(items[i])) return i;
    return -1;
}

/**
 * The selectable item `delta` (+1 / -1) steps from `current`, wrapping and
 * skipping separators and disabled items. From -1 (nothing highlighted),
 * down goes to the first item and up to the last. -1 if none is selectable.
 */
export function stepHighlight(items, current, delta) {
    const n = items.length;
    if (!n || firstActionable(items) < 0) return -1;
    if (current < 0 || current >= n)
        return delta >= 0 ? firstActionable(items) : lastActionable(items);
    let i = current;
    for (let step = 0; step < n; step++) {
        i = (i + (delta >= 0 ? 1 : -1) + n) % n;
        if (isActionable(items[i])) return i;
    }
    return current;
}

/**
 * Whether `e` is the keyboard "open the context menu" gesture: the Menu
 * key, or Shift+F10 (Windows/Linux). macOS has no such convention.
 */
export function isMenuKey(e, { mac = false } = {}) {
    if (mac || !e) return false;
    if (e.key === 'ContextMenu') return true;
    return e.key === 'F10' && !!e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey;
}

/**
 * Caller-side client. `open()` resolves to `{ action, context }` once the
 * user picks an item (`action` = its id) or the menu closes
 * (`action` = DISMISSED). Opening a new menu dismisses the previous one.
 */
export function createPopupMenuClient({ invoke, listen }) {
    let pending = null; // { token, resolve, context }
    let early = []; // actions that arrived before show_context_menu returned

    const settle = (action, context) => {
        const p = pending;
        pending = null;
        early = [];
        p?.resolve({ action, context: context ?? p.context ?? null });
    };

    listen(EVT.CONTEXT_MENU_ACTION, (event) => {
        const payload = event?.payload;
        if (!payload || typeof payload !== 'object') return;
        if (!pending) return;
        if (pending.token == null) {
            early.push(payload);
            return;
        }
        if (payload.token !== pending.token) return;
        settle(typeof payload.action === 'string' ? payload.action : DISMISSED, payload.context);
    });

    return {
        isOpen: () => pending !== null,

        async open({ x, y, items, context = null, keyboard = false } = {}) {
            if (pending) settle(DISMISSED);
            const menu = { context, keyboard: !!keyboard };
            if (items) menu.items = normalizeMenuItems(items);
            const done = new Promise((resolve) => {
                pending = { token: null, resolve, context };
            });
            const mine = pending;
            try {
                const token = await invoke('show_context_menu', {
                    x: Math.round(x),
                    y: Math.round(y),
                    menu,
                });
                if (pending === mine) {
                    mine.token = token;
                    const hit = early.find((p) => p.token === token);
                    early = [];
                    if (hit) settle(hit.action || DISMISSED, hit.context);
                }
            } catch (e) {
                console.error('[popup-menu] show_context_menu failed:', e);
                if (pending === mine) settle(DISMISSED);
            }
            return done;
        },
    };
}
