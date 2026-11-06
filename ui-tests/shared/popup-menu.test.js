/**
 * Generic popup menu: item normalisation, keyboard navigation helpers, the
 * menu-key gesture, the caller-side client protocol, the menu window's
 * rendering/keyboard view, and the menu window's request/answer loop.
 */

import { describe, it, expect, vi } from 'vitest';
import {
    DISMISSED,
    createPopupMenuClient,
    defaultTextMenuItems,
    firstActionable,
    isMenuKey,
    lastActionable,
    normalizeMenuItems,
    stepHighlight,
} from '../../ui/js/shared/popup-menu.js';
import {
    PopupMenuView,
    isPrimaryClick,
    renderMenuItems,
} from '../../ui/js/shared/popup-menu-view.js';
import { startMenuWindow, MENU_PADDING } from '../../ui/js/context-menu/menu-window.js';
import { EVT } from '../../ui/js/shared/events.js';

const t = (key, vars) => (vars?.name ? `${key}(${vars.name})` : key);
const flush = () => new Promise((r) => setTimeout(r, 0));

describe('normalizeMenuItems', () => {
    it('drops malformed items and collapses separators', () => {
        const items = normalizeMenuItems([
            { separator: true },
            { id: 'open', labelKey: 'result_menu.open' },
            { separator: true },
            { separator: true },
            { id: '', labelKey: 'x.y' },
            { id: 'bad id', labelKey: 'x.y' },
            { id: 'nolabel' },
            { id: 'raw', label: '  Raw  ', shortcut: ' Enter ' },
            null,
            { separator: true },
        ]);
        expect(items).toEqual([
            { id: 'open', labelKey: 'result_menu.open', enabled: true, danger: false },
            { separator: true },
            { id: 'raw', label: 'Raw', shortcut: 'Enter', enabled: true, danger: false },
        ]);
    });

    it('keeps enabled/danger flags', () => {
        const [a, b] = normalizeMenuItems([
            { id: 'a', label: 'A', enabled: false },
            { id: 'b', label: 'B', danger: true },
        ]);
        expect(a.enabled).toBe(false);
        expect(b.danger).toBe(true);
    });

    it('returns [] for garbage', () => {
        expect(normalizeMenuItems(undefined)).toEqual([]);
        expect(normalizeMenuItems('nope')).toEqual([]);
    });
});

describe('defaultTextMenuItems', () => {
    const shape = (items) => items.map((i) => (i.separator ? '-' : i.id));

    it('is the classic text menu', () => {
        expect(shape(defaultTextMenuItems())).toEqual([
            'cut',
            'copy',
            'paste',
            '-',
            'settings',
            'close',
        ]);
        expect(defaultTextMenuItems()[0].labelKey).toBe('context_menu.cut');
    });

    it('adds Inspect in dev mode', () => {
        expect(shape(defaultTextMenuItems({ devMode: true })).slice(-2)).toEqual(['-', 'inspect']);
    });
});

describe('keyboard navigation helpers', () => {
    const items = [
        { id: 'a', enabled: false },
        { id: 'b' },
        { separator: true },
        { id: 'c' },
        { id: 'd', enabled: false },
    ];

    it('finds the first and last selectable items', () => {
        expect(firstActionable(items)).toBe(1);
        expect(lastActionable(items)).toBe(3);
    });

    it('steps over separators and disabled items, wrapping', () => {
        expect(stepHighlight(items, -1, 1)).toBe(1);
        expect(stepHighlight(items, -1, -1)).toBe(3);
        expect(stepHighlight(items, 1, 1)).toBe(3);
        expect(stepHighlight(items, 3, 1)).toBe(1);
        expect(stepHighlight(items, 1, -1)).toBe(3);
    });

    it('returns -1 when nothing is selectable', () => {
        expect(stepHighlight([{ separator: true }, { id: 'x', enabled: false }], -1, 1)).toBe(-1);
        expect(stepHighlight([], 0, 1)).toBe(-1);
    });
});

describe('isMenuKey', () => {
    it('recognises the Menu key and Shift+F10 off macOS', () => {
        expect(isMenuKey({ key: 'ContextMenu' })).toBe(true);
        expect(isMenuKey({ key: 'F10', shiftKey: true })).toBe(true);
        expect(isMenuKey({ key: 'F10' })).toBe(false);
        expect(isMenuKey({ key: 'F10', shiftKey: true, ctrlKey: true })).toBe(false);
        expect(isMenuKey({ key: 'a', shiftKey: true })).toBe(false);
    });

    it('is off on macOS (no convention)', () => {
        expect(isMenuKey({ key: 'ContextMenu' }, { mac: true })).toBe(false);
        expect(isMenuKey({ key: 'F10', shiftKey: true }, { mac: true })).toBe(false);
    });
});

function fakeTauri({ token = 7 } = {}) {
    const listeners = {};
    const listen = vi.fn((event, cb) => {
        (listeners[event] ||= []).push(cb);
        return Promise.resolve(() => {});
    });
    const emit = (event, payload) => (listeners[event] || []).forEach((cb) => cb({ payload }));
    let next = token;
    const invoke = vi.fn(async (cmd) => {
        if (cmd === 'show_context_menu') return next++;
        return null;
    });
    return { listen, emit, invoke };
}

describe('createPopupMenuClient', () => {
    it('sends the request and resolves with the matching choice', async () => {
        const { listen, emit, invoke } = fakeTauri();
        const client = createPopupMenuClient({ invoke, listen });
        const done = client.open({
            x: 10.4,
            y: 20.6,
            items: [{ id: 'open', labelKey: 'result_menu.open' }],
            context: { kind: 'app', target: 'Slack' },
            keyboard: true,
        });
        expect(client.isOpen()).toBe(true);
        await flush();
        expect(invoke).toHaveBeenCalledWith('show_context_menu', {
            x: 10,
            y: 21,
            menu: {
                context: { kind: 'app', target: 'Slack' },
                keyboard: true,
                items: [{ id: 'open', labelKey: 'result_menu.open', enabled: true, danger: false }],
            },
        });
        // Another menu's answer is ignored.
        emit(EVT.CONTEXT_MENU_ACTION, { token: 99, action: 'open' });
        emit(EVT.CONTEXT_MENU_ACTION, {
            token: 7,
            action: 'open',
            context: { kind: 'app', target: 'Slack' },
        });
        await expect(done).resolves.toEqual({
            action: 'open',
            context: { kind: 'app', target: 'Slack' },
        });
        expect(client.isOpen()).toBe(false);
    });

    it('omits items for the default text menu', async () => {
        const { listen, invoke } = fakeTauri();
        const client = createPopupMenuClient({ invoke, listen });
        client.open({ x: 1, y: 2 });
        await flush();
        expect(invoke.mock.calls[0][1].menu).toEqual({ context: null, keyboard: false });
    });

    it('dismisses the previous menu when a new one opens', async () => {
        const { listen, invoke } = fakeTauri();
        const client = createPopupMenuClient({ invoke, listen });
        const first = client.open({ x: 0, y: 0 });
        await flush();
        client.open({ x: 0, y: 0 });
        await expect(first).resolves.toMatchObject({ action: DISMISSED });
    });

    it('resolves as dismissed if the menu cannot be shown', async () => {
        const listen = vi.fn();
        const invoke = vi.fn(async () => {
            throw new Error('no window');
        });
        const client = createPopupMenuClient({ invoke, listen });
        vi.spyOn(console, 'error').mockImplementation(() => {});
        await expect(client.open({ x: 0, y: 0 })).resolves.toMatchObject({ action: DISMISSED });
    });

    it('keeps an answer that arrives before the token does', async () => {
        const listeners = {};
        const listen = (e, cb) => (listeners[e] = cb);
        let release;
        const invoke = vi.fn(
            () =>
                new Promise((r) => {
                    release = r;
                })
        );
        const client = createPopupMenuClient({ invoke, listen });
        const done = client.open({ x: 0, y: 0 });
        listeners[EVT.CONTEXT_MENU_ACTION]({ payload: { token: 3, action: 'copy' } });
        release(3);
        await expect(done).resolves.toMatchObject({ action: 'copy' });
    });
});

describe('PopupMenuView', () => {
    function setup(items, opts) {
        const root = document.createElement('div');
        document.body.appendChild(root);
        const onChoose = vi.fn();
        const onDismiss = vi.fn();
        const view = new PopupMenuView(root, { t, onChoose, onDismiss });
        view.show(items, opts);
        return { root, view, onChoose, onDismiss };
    }
    const key = (k) => new KeyboardEvent('keydown', { key: k, cancelable: true });
    const items = [
        { id: 'open', labelKey: 'result_menu.open', shortcut: 'Enter' },
        { separator: true },
        { id: 'gone', label: 'Gone', enabled: false },
        { id: 'uninstall', labelKey: 'result_menu.uninstall', labelVars: { name: 'Slack' }, danger: true },
    ];

    it('renders labels, hints, separators and flags as text', () => {
        const root = document.createElement('div');
        renderMenuItems(root, [{ id: 'x', label: '<b>not html</b>' }, ...items], t);
        const rows = root.querySelectorAll('.menu-item');
        expect(rows).toHaveLength(4);
        expect(rows[0].textContent).toBe('<b>not html</b>');
        expect(root.querySelector('b')).toBeNull();
        expect(rows[1].querySelector('.menu-shortcut').textContent).toBe('Enter');
        expect(rows[1].querySelector('.menu-shortcut').getAttribute('dir')).toBe('ltr');
        expect(rows[2].getAttribute('aria-disabled')).toBe('true');
        expect(rows[3].classList.contains('danger')).toBe(true);
        expect(rows[3].textContent).toBe('result_menu.uninstall(Slack)');
        expect(root.querySelectorAll('[role="separator"]')).toHaveLength(1);
    });

    it('highlights the first item only when opened from the keyboard', () => {
        expect(setup(items, { keyboard: false }).view.highlighted).toBe(-1);
        expect(setup(items, { keyboard: true }).view.highlighted).toBe(0);
    });

    it('navigates with arrows, skipping disabled items, and chooses with Enter', () => {
        const { view, onChoose } = setup(items, { keyboard: true });
        view.handleKey(key('ArrowDown'));
        expect(view.highlighted).toBe(3);
        view.handleKey(key('ArrowDown'));
        expect(view.highlighted).toBe(0);
        view.handleKey(key('End'));
        view.handleKey(key('Enter'));
        expect(onChoose).toHaveBeenCalledWith('uninstall');
    });

    it('Escape dismisses; Enter with nothing highlighted does nothing', () => {
        const { view, onChoose, onDismiss } = setup(items, { keyboard: false });
        view.handleKey(key('Enter'));
        expect(onChoose).not.toHaveBeenCalled();
        view.handleKey(key('Escape'));
        expect(onDismiss).toHaveBeenCalled();
    });

    it('chooses only on a primary-button press', () => {
        const { root, onChoose } = setup(items, {});
        const open = root.querySelector('[data-action="open"]');
        const press = (init) =>
            open.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true, ...init }));
        press({ button: 2 }); // right click
        press({ button: 1 }); // middle click
        expect(onChoose).not.toHaveBeenCalled();
        // The right/middle press still doesn't steal focus or start a drag.
        const right = new MouseEvent('mousedown', { bubbles: true, cancelable: true, button: 2 });
        open.dispatchEvent(right);
        expect(right.defaultPrevented).toBe(true);
        press({ button: 0, ctrlKey: true }); // Ctrl+click off macOS is a click
        expect(onChoose).toHaveBeenCalledTimes(1);
        press({ button: 0 });
        expect(onChoose).toHaveBeenCalledTimes(2);
        expect(onChoose).toHaveBeenLastCalledWith('open');
    });

    it('treats Ctrl+click as a right click on macOS', () => {
        const root = document.createElement('div');
        const onChoose = vi.fn();
        const view = new PopupMenuView(root, { t, onChoose, onDismiss: vi.fn(), mac: true });
        view.show(items, {});
        const open = root.querySelector('[data-action="open"]');
        open.dispatchEvent(
            new MouseEvent('mousedown', { bubbles: true, button: 0, ctrlKey: true })
        );
        expect(onChoose).not.toHaveBeenCalled();
        open.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, button: 0 }));
        expect(onChoose).toHaveBeenCalledWith('open');
    });

    it('isPrimaryClick', () => {
        expect(isPrimaryClick({ button: 0 })).toBe(true);
        expect(isPrimaryClick({ button: 2 })).toBe(false);
        expect(isPrimaryClick({ button: 1 })).toBe(false);
        expect(isPrimaryClick({ button: 0, ctrlKey: true })).toBe(true);
        expect(isPrimaryClick({ button: 0, ctrlKey: true }, { mac: true })).toBe(false);
        expect(isPrimaryClick(null)).toBe(false);
    });

    it('ignores clicks on disabled items', () => {
        const { root, onChoose } = setup(items, {});
        const disabled = root.querySelector('[data-action="gone"]');
        disabled.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
        expect(onChoose).not.toHaveBeenCalled();
        root.querySelector('[data-action="open"]').dispatchEvent(
            new MouseEvent('mousedown', { bubbles: true })
        );
        expect(onChoose).toHaveBeenCalledWith('open');
    });
});

describe('startMenuWindow', () => {
    function setup({ pending = null, readyFails = false, emitTo: customEmitTo = null } = {}) {
        const listeners = {};
        const winListeners = {};
        const listen = (e, cb) => (listeners[e] = cb);
        const invoke = vi.fn(async (cmd) => {
            if (cmd === 'get_pending_context_menu') return pending;
            if (cmd === 'context_menu_ready') {
                if (readyFails) throw new Error('boom');
                return true;
            }
            return null;
        });
        const emitTo = customEmitTo || vi.fn(async () => {});
        const appWindow = {
            hide: vi.fn(async () => {}),
            listen: (e, cb) => (winListeners[e] = cb),
        };
        const root = document.createElement('div');
        document.body.appendChild(root);
        root.getBoundingClientRect = () => ({ width: 151.2, height: 99.5 });
        const win = startMenuWindow({
            invoke,
            listen,
            emitTo,
            appWindow,
            root,
            t,
            devMode: () => false,
            isRtl: () => true,
            mac: false,
        });
        return { win, listeners, winListeners, invoke, emitTo, appWindow, root };
    }
    const payload = (token, extra = {}) => ({
        token,
        owner: 'floating',
        items: [{ id: 'open', labelKey: 'result_menu.open' }],
        context: { kind: 'app', target: 'Slack' },
        keyboard: false,
        ...extra,
    });

    it('renders a pushed menu and reports its padded size', async () => {
        const { listeners, invoke, root } = setup();
        await listeners[EVT.CONTEXT_MENU_RENDER]({ payload: payload(5) });
        expect(root.querySelectorAll('.menu-item')).toHaveLength(1);
        expect(invoke).toHaveBeenCalledWith('context_menu_ready', {
            token: 5,
            width: 152 + 2 * MENU_PADDING,
            height: 100 + 2 * MENU_PADDING,
            rtl: true,
        });
    });

    it('renders the default text menu when no items are sent', async () => {
        const { listeners, root } = setup();
        await listeners[EVT.CONTEXT_MENU_RENDER]({ payload: payload(5, { items: null }) });
        expect([...root.querySelectorAll('.menu-item')].map((e) => e.dataset.action)).toEqual([
            'cut',
            'copy',
            'paste',
            'settings',
            'close',
        ]);
    });

    it('picks up a menu requested while it was loading, once', async () => {
        const { invoke, listeners } = setup({ pending: payload(9) });
        await flush();
        await listeners[EVT.CONTEXT_MENU_RENDER]({ payload: payload(9) });
        const readies = invoke.mock.calls.filter(([c]) => c === 'context_menu_ready');
        expect(readies).toHaveLength(1);
    });

    it('answers the owner with the choice and context, then hides', async () => {
        const { listeners, win, emitTo, appWindow } = setup();
        await listeners[EVT.CONTEXT_MENU_RENDER]({ payload: payload(5) });
        await win.answer('open');
        await win.answer('open'); // answered once only
        expect(emitTo).toHaveBeenCalledTimes(1);
        expect(emitTo).toHaveBeenCalledWith('floating', EVT.CONTEXT_MENU_ACTION, {
            token: 5,
            action: 'open',
            context: { kind: 'app', target: 'Slack' },
        });
        expect(appWindow.hide).toHaveBeenCalled();
    });

    it("an answer still in flight doesn't hide the menu that replaced it", async () => {
        // present token 1 → choose → (emit pending) present token 2 → emit
        // settles: token 2 must stay on screen.
        let releaseEmit;
        const emitTo = vi.fn(
            () =>
                new Promise((resolve) => {
                    releaseEmit = resolve;
                })
        );
        const { listeners, win, appWindow, root } = setup({ emitTo });
        await listeners[EVT.CONTEXT_MENU_RENDER]({ payload: payload(1) });
        const answering = win.answer('open');
        await listeners[EVT.CONTEXT_MENU_RENDER]({
            payload: payload(2, { items: [{ id: 'reveal', label: 'Reveal' }] }),
        });
        releaseEmit();
        await answering;
        expect(emitTo).toHaveBeenCalledWith(
            'floating',
            EVT.CONTEXT_MENU_ACTION,
            expect.objectContaining({ token: 1, action: 'open' })
        );
        expect(appWindow.hide).not.toHaveBeenCalled();
        expect([...root.querySelectorAll('.menu-item')].map((e) => e.dataset.action)).toEqual([
            'reveal',
        ]);
        // The new menu still answers — and hides — normally.
        const second = win.answer('reveal');
        releaseEmit();
        await second;
        expect(emitTo).toHaveBeenLastCalledWith(
            'floating',
            EVT.CONTEXT_MENU_ACTION,
            expect.objectContaining({ token: 2, action: 'reveal' })
        );
        expect(appWindow.hide).toHaveBeenCalledTimes(1);
    });

    it('dismisses on blur unless the focus comes back', async () => {
        vi.useFakeTimers();
        try {
            const { listeners, winListeners, emitTo } = setup();
            await listeners[EVT.CONTEXT_MENU_RENDER]({ payload: payload(5) });
            winListeners['tauri://blur']();
            winListeners['tauri://focus']();
            await vi.advanceTimersByTimeAsync(500);
            expect(emitTo).not.toHaveBeenCalled();
            winListeners['tauri://blur']();
            await vi.advanceTimersByTimeAsync(500);
            expect(emitTo).toHaveBeenCalledWith(
                'floating',
                EVT.CONTEXT_MENU_ACTION,
                expect.objectContaining({ token: 5, action: DISMISSED })
            );
        } finally {
            vi.useRealTimers();
        }
    });

    it("a stale blur can't dismiss the next menu", async () => {
        vi.useFakeTimers();
        try {
            const { listeners, winListeners, emitTo } = setup();
            await listeners[EVT.CONTEXT_MENU_RENDER]({ payload: payload(5) });
            winListeners['tauri://blur']();
            await listeners[EVT.CONTEXT_MENU_RENDER]({ payload: payload(6) });
            await vi.advanceTimersByTimeAsync(500);
            expect(emitTo).not.toHaveBeenCalled();
        } finally {
            vi.useRealTimers();
        }
    });

    it('tells the caller the menu was dismissed if it cannot be shown', async () => {
        vi.spyOn(console, 'error').mockImplementation(() => {});
        const { listeners, emitTo } = setup({ readyFails: true });
        await listeners[EVT.CONTEXT_MENU_RENDER]({ payload: payload(5) });
        expect(emitTo).toHaveBeenCalledWith(
            'floating',
            EVT.CONTEXT_MENU_ACTION,
            expect.objectContaining({ action: DISMISSED })
        );
    });
});
