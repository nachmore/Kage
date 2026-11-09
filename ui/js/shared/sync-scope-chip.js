/**
 * The where-it-applies control on Settings rows (docs/design/SYNC.md §9.4):
 * "All computers", "Macs only", "This computer", and its menu of scope
 * actions (§9.3).
 *
 * Rows opt in with markup, so any page can use it:
 *   - `data-sync-path="ui.theme"` on a row: a control at the end of its
 *     label line (`[data-sync-chip-slot]`, else its `.setting-label`, else
 *     the row). List items use `shortcuts/<sync_id>` (a chip only when the
 *     item isn't on every computer) and per-OS fields of an item
 *     `shortcuts/<sync_id>/<field>`. `data-sync-name` names an item for
 *     the confirm text.
 *   - `data-sync-avail="shortcuts/<sync_id>"`: the item editor's
 *     "Available on" line.
 *   - `data-sync-local="sign_in"`: a sign-in field of an extension, which
 *     never syncs.
 *
 * With sync off, `sync_get_scopes` answers `sync_unavailable` and nothing
 * is drawn: no page changes. Every element this module adds carries
 * `data-sync-ui`, which the Settings dirty tracker ignores (a scope change
 * is not an unsaved edit).
 */

import { confirmDialog, alertDialog } from './confirm-dialog.js';
import { onConfigChange } from './config-cache.js';
import { errMessage } from './error-message.js';
import { EVT } from './events.js';

/** States in which sync is set up on this computer. */
const NOT_ON = new Set(['off', 'unavailable', 'setting_up']);

/** Whether a `SyncStatus` says sync is set up here (paused or locked count). */
export function syncIsOn(status) {
    return !!status && typeof status.state === 'string' && !NOT_ON.has(status.state);
}

/** This computer's OS as sync names it: `macos`, `windows` or `linux`. */
export function currentOs() {
    const p = (typeof navigator !== 'undefined' && navigator.platform) || '';
    if (/^Mac/i.test(p)) return 'macos';
    if (/^Win/i.test(p)) return 'windows';
    return 'linux';
}

/**
 * Whether two catalog paths name the same thing or one holds the other:
 * `ui.theme` / `ui.theme`, `hotkey` / `hotkey.key`, `shortcuts` /
 * `shortcuts/s1`, `shortcuts/s1` / `shortcuts/s1/path`.
 */
export function pathsOverlap(a, b) {
    if (typeof a !== 'string' || typeof b !== 'string' || !a || !b) return false;
    if (a === b) return true;
    const under = (x, y) => x.startsWith(`${y}.`) || x.startsWith(`${y}/`);
    return under(a, b) || under(b, a);
}

/** A synced value as short text for the control and its menu. */
export function formatScopeValue(value, t) {
    if (value === undefined || value === null) return '';
    if (typeof value === 'boolean') {
        return t(value ? 'settings.sync_scope.value.on' : 'settings.sync_scope.value.off');
    }
    if (typeof value === 'string' || typeof value === 'number') return String(value);
    if (Array.isArray(value)) return value.map((v) => formatScopeValue(v, t)).join(', ');
    if (typeof value === 'object' && typeof value.key === 'string') {
        // A hotkey: modifiers and key together.
        const mods = Array.isArray(value.modifiers) ? value.modifiers : [];
        return [...mods, value.key].filter(Boolean).join('+');
    }
    try {
        return JSON.stringify(value);
    } catch {
        return '';
    }
}

/** Bidi-isolate user text (FSI … PDI) so it can't reorder the sentence. */
export function isolate(text) {
    return `\u2068${text ?? ''}\u2069`;
}

function osOf(os) {
    return os === 'macos' || os === 'windows' ? os : 'other';
}

/**
 * What the control shows for one `ScopeView` on this OS, or `null` for no
 * control.
 *
 * Tones: `quiet` (shared, no overrides: an icon; the text shows on hover
 * and focus), `override` (this OS has its own value), `per_os` (per OS by
 * default), `pinned` (only this computer), `differs` (another OS has its
 * own value), `local` (never synced, static) and `held` (not synced here,
 * static).
 *
 * @param {object} view - a `ScopeView` from `sync_get_scopes`
 * @param {string} os - `macos`, `windows` or `linux`
 * @param {{ item?: boolean, t?: Function }} [opts] - `item`: a list item
 *   as a whole, which shows a chip only when it isn't on every computer
 * @returns {{ label: string, tone: string, title: string, sub?: string,
 *   interactive: boolean } | null}
 */
export function chipState(view, os, { item = false, t = (k) => k } = {}) {
    if (!view || typeof view !== 'object') return null;
    const values = view.values || {};
    const others = Array.isArray(view.others) ? view.others : [];
    if (view.rule === 'never') return null;
    if (view.rule === 'this_computer' && !view.synced) {
        const reason = view.reason_key ? t(view.reason_key) : '';
        return {
            label: t('settings.sync_scope.this_computer'),
            tone: 'local',
            title: t('settings.sync_scope.not_synced', { reason }),
            interactive: false,
        };
    }
    if (view.held) {
        return {
            label: t('settings.sync_scope.held'),
            tone: 'held',
            title: view.reason_key ? t(view.reason_key) : t('settings.sync_scope.held'),
            interactive: false,
        };
    }
    if (view.applies === 'this_computer') {
        const label = t('settings.sync_scope.this_computer');
        return { label, tone: 'pinned', title: label, interactive: true };
    }
    if (item) {
        if (view.applies !== 'os') return null;
        const label = t('settings.sync_scope.os_only', { os: osOf(os) });
        return { label, tone: 'override', title: label, interactive: true };
    }
    if (view.applies === 'os' && view.rule === 'per_os') {
        const label = t('settings.sync_scope.per_os_short');
        const title = [t('settings.sync_scope.per_os'), ...others.map((o) => otherLine(o, t))]
            .filter(Boolean)
            .join('\n');
        const out = { label, tone: 'per_os', title, interactive: true };
        // The other OS's value as a hint under the field (§4.4).
        if (others.length > 0) out.sub = others.map((o) => otherLine(o, t)).join(' · ');
        return out;
    }
    if (view.applies === 'os') {
        const label = t('settings.sync_scope.os_only', { os: osOf(os) });
        const out = { label, tone: 'override', title: label, interactive: true };
        if (values.all !== undefined) {
            out.sub = t('settings.sync_scope.all_value', {
                value: formatScopeValue(values.all, t),
            });
        }
        return out;
    }
    if (others.length > 0) {
        return {
            label: t('settings.sync_scope.differs', { os: osOf(others[0].os) }),
            tone: 'differs',
            title: others.map((o) => otherLine(o, t)).join('\n'),
            interactive: true,
        };
    }
    const label = t('settings.sync_scope.all');
    return { label, tone: 'quiet', title: label, interactive: true };
}

function otherLine(o, t) {
    return t('settings.sync_scope.other_uses', { os: osOf(o?.os), value: o?.preview ?? '' });
}

/**
 * The menu for one `ScopeView` on this OS: scope actions with their text,
 * and info lines. `noop` items are the current choice.
 *
 * @param {object} view
 * @param {string} os
 * @param {{ item?: boolean, t?: Function }} [opts]
 * @returns {Array<{ action: string, label: string, detail?: string,
 *   checked?: boolean, noop?: boolean, confirm?: boolean }
 *   | { separator: true } | { info: string }>}
 */
export function menuItems(view, os, { item = false, t = (k) => k } = {}) {
    const state = chipState(view, os, { item: false, t });
    if (!view || (!item && !state?.interactive)) return [];
    if (item) {
        if (view.held || view.rule === 'never') return [];
        const a = view.applies;
        return [
            {
                action: 'avail_all',
                label: t('settings.sync_scope.action.avail_all'),
                checked: a === 'all',
                noop: a === 'all',
            },
            {
                action: 'avail_os',
                label: t('settings.sync_scope.action.avail_os', { os: osOf(os) }),
                checked: a === 'os',
                noop: a === 'os',
            },
            {
                action: 'avail_this_computer',
                label: t('settings.sync_scope.action.avail_this_computer'),
                checked: a === 'this_computer',
                noop: a === 'this_computer',
                confirm: a !== 'this_computer',
            },
        ];
    }
    const values = view.values || {};
    const fmt = (v) => formatScopeValue(v, t);
    const mine = values[os];
    const shared = values.all;
    const items = [];
    if (state.tone === 'pinned') {
        items.push(
            {
                action: 'this_computer',
                label: t('settings.sync_scope.action.this_computer'),
                checked: true,
                noop: true,
            },
            {
                action: 'stop_this_computer',
                label: t('settings.sync_scope.action.stop_this_computer'),
            }
        );
    } else if (state.tone === 'per_os') {
        const detail = ['macos', 'windows', 'linux']
            .filter((o) => values[o] !== undefined)
            .map((o) => `${t(`sync.os.${o}`)}: ${fmt(values[o])}`)
            .join(' · ');
        items.push(
            {
                action: 'os',
                label: t('settings.sync_scope.per_os'),
                detail,
                checked: true,
                noop: true,
            },
            { action: 'all', label: t('settings.sync_scope.action.all') },
            { action: 'this_computer', label: t('settings.sync_scope.action.this_computer') }
        );
    } else if (state.tone === 'override') {
        items.push({
            action: 'os',
            label: t('settings.sync_scope.os_only', { os: osOf(os) }),
            detail: fmt(mine),
            checked: true,
            noop: true,
        });
        if (shared !== undefined) {
            items.push({
                action: 'use_shared',
                label: t('settings.sync_scope.action.use_shared', { value: fmt(shared) }),
            });
        }
        items.push(
            {
                action: 'all',
                label: t('settings.sync_scope.action.all_value', { value: fmt(mine) }),
            },
            { action: 'this_computer', label: t('settings.sync_scope.action.this_computer') }
        );
    } else {
        items.push(
            {
                action: 'all',
                label: t('settings.sync_scope.all'),
                detail: fmt(shared),
                checked: true,
                noop: true,
            },
            {
                action: 'os',
                label: t('settings.sync_scope.action.os', { os: osOf(os) }),
            },
            { action: 'this_computer', label: t('settings.sync_scope.action.this_computer') }
        );
    }
    const others = Array.isArray(view.others) ? view.others : [];
    if (others.length > 0 && state.tone !== 'per_os') {
        items.push({ separator: true });
        for (const o of others) items.push({ info: otherLine(o, t) });
    }
    return items;
}

// --- DOM -------------------------------------------------------------------

const CHIP_SELECTOR = '[data-sync-chip-for]';

/** The first match under `row` that belongs to it, not to a row inside it. */
function own(row, selector) {
    for (const el of row.querySelectorAll(selector)) {
        if (el.closest('[data-sync-path]') === row) return el;
    }
    return null;
}

function chipSlot(row) {
    return own(row, '[data-sync-chip-slot]') || own(row, '.setting-label') || row;
}

function removeChips(row) {
    const path = row.dataset.syncPath;
    for (const el of row.querySelectorAll(CHIP_SELECTOR)) {
        if (el.dataset.syncChipFor === path) el.remove();
    }
    delete row.dataset.syncApplies;
}

function rowLabel(row) {
    if (row.dataset.syncName) return row.dataset.syncName;
    const label = own(row, '.setting-label');
    const text = (label?.firstChild?.textContent || label?.textContent || '').trim();
    return text;
}

/** The menu that's open, if any (one at a time per window). */
let openMenuState = null;

function closeMenu({ refocus = false } = {}) {
    const m = openMenuState;
    openMenuState = null;
    if (!m) return;
    m.menu.remove();
    document.removeEventListener('mousedown', m.onOutside, true);
    if (refocus) m.anchor.focus?.();
}

function openMenu(anchor, items, onPick, t) {
    closeMenu();
    const menu = document.createElement('div');
    menu.className = 'sync-scope-menu';
    menu.setAttribute('role', 'menu');
    menu.dataset.syncUi = '';
    const buttons = [];
    for (const it of items) {
        if (it.separator) {
            const sep = document.createElement('div');
            sep.className = 'sync-scope-menu-sep';
            sep.setAttribute('role', 'separator');
            menu.appendChild(sep);
            continue;
        }
        if (it.info) {
            const info = document.createElement('div');
            info.className = 'sync-scope-menu-info';
            info.textContent = it.info;
            menu.appendChild(info);
            continue;
        }
        const btn = document.createElement('button');
        btn.type = 'button';
        btn.className = 'sync-scope-menu-item';
        btn.setAttribute('role', 'menuitemradio');
        btn.setAttribute('aria-checked', it.checked ? 'true' : 'false');
        btn.dataset.syncScopeAction = it.action;
        const check = document.createElement('span');
        check.className = 'sync-scope-menu-check';
        check.setAttribute('aria-hidden', 'true');
        check.textContent = it.checked ? '✓' : '';
        const label = document.createElement('span');
        label.className = 'sync-scope-menu-label';
        label.textContent = it.label;
        btn.append(check, label);
        if (it.detail) {
            const detail = document.createElement('span');
            detail.className = 'sync-scope-menu-detail';
            detail.setAttribute('dir', 'auto');
            detail.textContent = it.detail;
            btn.appendChild(detail);
        }
        btn.addEventListener('click', () => {
            closeMenu({ refocus: true });
            if (!it.noop) onPick(it);
        });
        buttons.push(btn);
        menu.appendChild(btn);
    }
    menu.addEventListener('keydown', (e) => {
        const i = buttons.indexOf(document.activeElement);
        if (e.key === 'Escape') {
            e.preventDefault();
            closeMenu({ refocus: true });
        } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
            e.preventDefault();
            const n = buttons.length;
            const next = e.key === 'ArrowDown' ? (i + 1 + n) % n : (i - 1 + n) % n;
            buttons[next]?.focus();
        } else if (e.key === 'Tab') {
            closeMenu();
        }
    });
    menu.setAttribute('aria-label', t('settings.sync_scope.menu_label'));
    document.body.appendChild(menu);
    const r = anchor.getBoundingClientRect?.() || { left: 0, right: 0, bottom: 0 };
    const rtl = document.documentElement.dir === 'rtl';
    menu.style.top = `${Math.round(r.bottom + 4)}px`;
    if (rtl) menu.style.right = `${Math.round(window.innerWidth - r.right)}px`;
    else menu.style.left = `${Math.round(r.left)}px`;
    const onOutside = (e) => {
        if (!menu.contains(e.target) && e.target !== anchor) closeMenu();
    };
    document.addEventListener('mousedown', onOutside, true);
    openMenuState = { menu, anchor, onOutside };
    (buttons.find((b) => b.getAttribute('aria-checked') === 'true') || buttons[0])?.focus();
    return menu;
}

/**
 * Add a control to every `[data-sync-path]` row under `root`, with one
 * `sync_get_scopes` call per pass. Passes run now, on `config_updated`,
 * on `sync_status_changed`, and when new rows appear (list editors
 * redraw). Returns a function that removes everything.
 *
 * @param {Element} root
 * @param {{ invoke: Function, listen: Function, t: Function, os?: string }} deps
 * @returns {() => void}
 */
export function attachScopeChips(root, { invoke, listen, t, os } = {}) {
    if (!root || typeof invoke !== 'function') return () => {};
    const st = {
        on: false,
        gen: 0,
        timer: null,
        os: os || currentOs(),
        views: new Map(),
        closed: false,
    };
    const unsubs = [];

    const clearAll = () => {
        for (const row of root.querySelectorAll('[data-sync-path]')) removeChips(row);
        for (const el of root.querySelectorAll('[data-sync-avail], [data-sync-local]')) {
            el.replaceChildren();
            el.hidden = true;
        }
    };

    const placeRow = (row) => {
        const path = row.dataset.syncPath;
        removeChips(row);
        const view = st.views.get(path);
        if (!view) return;
        const item = path.split('/').length === 2;
        const state = chipState(view, st.os, { item, t });
        if (!state) return;
        row.dataset.syncApplies = view.applies || '';
        const tag = state.interactive ? 'button' : 'span';
        const chip = document.createElement(tag);
        chip.className = `sync-scope-chip sync-scope-chip--${state.tone}`;
        chip.dataset.syncChipFor = path;
        chip.dataset.syncUi = '';
        if (state.interactive) chip.type = 'button';
        else chip.tabIndex = 0;
        chip.title = state.title;
        const name = rowLabel(row);
        chip.setAttribute(
            'aria-label',
            name ? t('settings.sync_scope.aria', { name, where: state.label }) : state.label
        );
        if (state.interactive) chip.setAttribute('aria-haspopup', 'menu');
        const icon = document.createElement('span');
        icon.className = 'sync-scope-chip-icon';
        icon.setAttribute('aria-hidden', 'true');
        icon.textContent = state.tone === 'pinned' || state.tone === 'local' ? '⌂' : '⇅';
        const label = document.createElement('span');
        label.className = 'sync-scope-chip-label';
        label.textContent = state.label;
        chip.append(icon, label);
        if (state.interactive) {
            chip.addEventListener('click', (e) => {
                // A chip can sit in a clickable row (a list item that expands).
                e.preventDefault();
                e.stopPropagation();
                const items = menuItems(view, st.os, { item, t });
                openMenu(chip, items, (it) => runAction(row, path, it), t);
            });
        }
        const slot = chipSlot(row);
        slot.appendChild(chip);
        if (state.sub) {
            const sub = document.createElement('div');
            sub.className = 'sync-scope-sub';
            sub.dataset.syncChipFor = path;
            sub.dataset.syncUi = '';
            sub.textContent = state.sub;
            if (slot === row) row.appendChild(sub);
            else slot.after(sub);
        }
    };

    const placeAvail = (el) => {
        const path = el.dataset.syncAvail;
        const view = st.views.get(path);
        el.replaceChildren();
        if (!view || view.held || view.rule === 'never') {
            el.hidden = true;
            return;
        }
        el.hidden = false;
        el.dataset.syncUi = '';
        el.classList.add('sync-avail');
        el.setAttribute('role', 'radiogroup');
        const title = document.createElement('span');
        title.className = 'sync-avail-title';
        title.id = `sync-avail-${Math.random().toString(36).slice(2, 10)}`;
        title.textContent = t('settings.sync_scope.available_on');
        el.setAttribute('aria-labelledby', title.id);
        el.appendChild(title);
        const name = `${title.id}-r`;
        for (const it of menuItems(view, st.os, { item: true, t })) {
            const label = document.createElement('label');
            label.className = 'sync-avail-option';
            const input = document.createElement('input');
            input.type = 'radio';
            input.name = name;
            input.value = it.action;
            input.checked = !!it.checked;
            input.addEventListener('change', () => {
                if (input.checked) runAction(el, path, it);
            });
            const text = document.createElement('span');
            text.textContent = it.label;
            label.append(input, text);
            el.appendChild(label);
        }
    };

    const placeLocal = (el) => {
        el.replaceChildren();
        el.hidden = false;
        el.dataset.syncUi = '';
        const chip = document.createElement('span');
        chip.className = 'sync-scope-chip sync-scope-chip--local';
        chip.tabIndex = 0;
        const text = t('settings.sync_scope.sign_in_local');
        chip.title = text;
        chip.setAttribute('aria-label', text);
        const icon = document.createElement('span');
        icon.className = 'sync-scope-chip-icon';
        icon.setAttribute('aria-hidden', 'true');
        icon.textContent = '⌂';
        const label = document.createElement('span');
        label.className = 'sync-scope-chip-label';
        label.textContent = text;
        chip.append(icon, label);
        el.appendChild(chip);
    };

    const placeAll = () => {
        if (!st.on) return;
        for (const row of root.querySelectorAll('[data-sync-path]')) placeRow(row);
        for (const el of root.querySelectorAll('[data-sync-avail]')) placeAvail(el);
        for (const el of root.querySelectorAll('[data-sync-local]')) placeLocal(el);
    };

    const runAction = async (el, path, it) => {
        if (it.confirm) {
            const nameText = el.dataset.syncName || rowLabel(el) || '';
            const ok = await confirmDialog({
                title: t('settings.sync_scope.confirm_this_computer.title'),
                message: t('settings.sync_scope.confirm_this_computer.message', {
                    name: isolate(nameText),
                }),
                confirmLabel: t('settings.sync_scope.confirm_this_computer.ok'),
                cancelLabel: t('settings.sync_scope.confirm_this_computer.cancel'),
                destructive: true,
            });
            if (!ok) {
                placeAll();
                return;
            }
        }
        try {
            const view = await invoke('sync_set_scope', { path, action: it.action });
            if (view && typeof view === 'object') st.views.set(path, view);
            placeAll();
            el.dispatchEvent(
                new CustomEvent('kage:sync-scope-changed', {
                    bubbles: true,
                    detail: { path, action: it.action },
                })
            );
        } catch (e) {
            placeAll();
            await alertDialog({
                title: t('settings.sync_scope.failed'),
                message: errMessage(e),
            });
        }
    };

    const refresh = async () => {
        if (st.closed) return;
        const gen = ++st.gen;
        const paths = new Set();
        for (const row of root.querySelectorAll('[data-sync-path]')) {
            if (row.dataset.syncPath) paths.add(row.dataset.syncPath);
        }
        for (const el of root.querySelectorAll('[data-sync-avail]')) {
            if (el.dataset.syncAvail) paths.add(el.dataset.syncAvail);
        }
        let views;
        try {
            views = await invoke('sync_get_scopes', { paths: [...paths] });
        } catch {
            // Off, locked away, or not built: nothing shows.
            if (gen !== st.gen || st.closed) return;
            st.on = false;
            st.views.clear();
            clearAll();
            return;
        }
        if (gen !== st.gen || st.closed) return;
        st.on = true;
        st.views = new Map(Object.entries(views || {}));
        placeAll();
    };

    const schedule = (ms = 150) => {
        if (st.closed) return;
        if (st.timer) clearTimeout(st.timer);
        st.timer = setTimeout(() => {
            st.timer = null;
            refresh();
        }, ms);
    };

    // New rows (a list editor redrew) and rows that now name a path (an
    // item editor opened) get their controls; our own nodes don't count.
    let observer = null;
    if (typeof MutationObserver === 'function') {
        const MARKED = '[data-sync-path], [data-sync-avail], [data-sync-local]';
        observer = new MutationObserver((records) => {
            if (!st.on) return;
            const fresh = records.some(
                (r) =>
                    r.type === 'attributes' ||
                    [...r.addedNodes].some(
                        (n) =>
                            n.nodeType === 1 &&
                            !n.hasAttribute?.('data-sync-ui') &&
                            (n.matches?.(MARKED) || n.querySelector?.(MARKED))
                    )
            );
            if (fresh) schedule(50);
        });
        observer.observe(root, {
            childList: true,
            subtree: true,
            attributes: true,
            attributeFilter: ['data-sync-path', 'data-sync-avail'],
        });
    }

    unsubs.push(onConfigChange(() => schedule()));
    if (typeof listen === 'function') {
        const p = listen(EVT.SYNC_STATUS_CHANGED, () => schedule());
        Promise.resolve(p)
            .then((un) => {
                if (typeof un !== 'function') return;
                if (st.closed) un();
                else unsubs.push(un);
            })
            .catch(() => {});
    }
    refresh();

    return () => {
        st.closed = true;
        if (st.timer) clearTimeout(st.timer);
        observer?.disconnect();
        for (const un of unsubs) {
            try {
                un();
            } catch {}
        }
        closeMenu();
        clearAll();
    };
}

/**
 * Point an item editor at the list item it edits (or at none, for a new
 * item): its `[data-sync-field="<field>"]` rows name
 * `<itemPath>/<field>`, and its `[data-sync-item-avail]` element becomes
 * the "Available on" line. A new item has no id yet, so it shows nothing.
 *
 * @param {Element} root - the editor
 * @param {string | null} itemPath - `shortcuts/<sync_id>`, or null
 * @param {string} [name] - the item's name, for the confirm text
 */
export function markSyncItem(root, itemPath, name = '') {
    if (!root) return;
    const drop = (el) => {
        for (const c of el.querySelectorAll('[data-sync-chip-for]')) c.remove();
    };
    for (const el of root.querySelectorAll('[data-sync-field]')) {
        if (itemPath) {
            el.dataset.syncPath = `${itemPath}/${el.dataset.syncField}`;
        } else if (el.dataset.syncPath) {
            drop(el);
            delete el.dataset.syncPath;
        }
    }
    for (const el of root.querySelectorAll('[data-sync-item-avail]')) {
        if (itemPath) {
            el.dataset.syncAvail = itemPath;
            el.dataset.syncName = name || '';
        } else {
            delete el.dataset.syncAvail;
            el.replaceChildren();
            el.hidden = true;
        }
    }
}

/**
 * "Changed on all your computers." with [Undo] and [Only on this computer]
 * (docs/design/SYNC.md §9.4). Hides itself after a while.
 *
 * @param {{ t: Function, onUndo: () => Promise<void>,
 *   onOnlyHere: () => Promise<void>, onError?: (e: unknown) => void }} opts
 * @returns {HTMLElement | null}
 */
export function showChangedEverywhereToast({ t, onUndo, onOnlyHere, onError }) {
    if (typeof document === 'undefined' || !document.body) return null;
    document.querySelector('.sync-toast')?.remove();
    const toast = document.createElement('div');
    toast.className = 'sync-toast';
    toast.setAttribute('role', 'status');
    toast.dataset.syncUi = '';
    const text = document.createElement('span');
    text.textContent = t('settings.sync_scope.changed_everywhere');
    toast.appendChild(text);
    const timer = setTimeout(() => toast.remove(), 10000);
    const button = (key, fn) => {
        const b = document.createElement('button');
        b.type = 'button';
        b.className = 'sync-toast-btn';
        b.textContent = t(key);
        b.addEventListener('click', async () => {
            clearTimeout(timer);
            toast.remove();
            try {
                await fn();
            } catch (e) {
                onError?.(e);
            }
        });
        toast.appendChild(b);
    };
    button('settings.sync_scope.undo', onUndo);
    button('settings.sync_scope.action.this_computer', onOnlyHere);
    document.body.appendChild(toast);
    return toast;
}

/**
 * Whether an extension's data goes to the sync folder from this computer:
 * sync on here, the Extensions category on, `sync` granted, and its data
 * switch on (docs/design/SYNC.md §12.3).
 *
 * @param {object | null} status - `SyncStatus`
 * @param {object | null} grant - `config.extension_grants[id]`
 * @param {string} extensionId
 */
export function extensionDataSyncs(status, grant, extensionId) {
    if (!syncIsOn(status)) return false;
    if (status.categories && status.categories.extensions === false) return false;
    const granted = Array.isArray(grant?.granted) ? grant.granted : [];
    if (!granted.includes('sync')) return false;
    const off = Array.isArray(status.ext_data_off) ? status.ext_data_off : [];
    return !off.includes(extensionId);
}
