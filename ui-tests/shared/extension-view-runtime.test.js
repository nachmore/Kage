/**
 * The sandbox runtime's view surface and `context.storage`, end to end
 * through the port protocol (ui/js/extension-sandbox/runtime.js,
 * runtime/view-api.js, runtime/host-api.js; docs/design/EXTENSION_VIEWS.md
 * §8.4–§8.8, §9, §10, §11, §13).
 *
 * The runtime imports extension modules from blob: URLs, which Node can't
 * import. The tests swap `Blob` and `URL.createObjectURL` for versions that
 * hand out data: URLs, so the real runtime loads real module source.
 */
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import {
    createViewSurface,
    isViewCloseShortcut,
    removeWebRtc,
    viewLinkTarget,
} from '../../ui/js/extension-sandbox/runtime/view-api.js';
import { createStorageEvents, HOST_FEATURES } from '../../ui/js/extension-sandbox/runtime/host-api.js';

class TextBlob {
    constructor(parts = [], opts = {}) {
        this.text = parts.map(String).join('');
        this.type = opts.type || '';
    }
}

const realBlob = globalThis.Blob;
const realCreate = URL.createObjectURL;

beforeAll(() => {
    globalThis.Blob = TextBlob;
    URL.createObjectURL = (blob) =>
        `data:text/javascript;base64,${Buffer.from(blob.text).toString('base64')}`;
});

afterAll(() => {
    globalThis.Blob = realBlob;
    URL.createObjectURL = realCreate;
});

const VIEW_SOURCE = `
const p = () => globalThis.__probe;
export default class ExplorerView {
    initialize(context) { p().context = context; p().calls.push(['initialize']); }
    async mount(root, info) { p().root = root; p().calls.push(['mount', root.id, info]); }
    onOpen(info) { p().calls.push(['onOpen', info]); }
    onShow() { p().calls.push(['onShow']); }
    onHide() { p().calls.push(['onHide']); }
    onThemeChange(theme) { p().calls.push(['onThemeChange', theme]); }
    onConfigUpdate(config) { p().calls.push(['onConfigUpdate', config]); }
    destroy() { p().calls.push(['destroy']); }
}`;

const SEARCH_SOURCE = `
export default class Search {
    initialize(context) { globalThis.__probe.context = context; }
    match() { return []; }
}`;

async function waitFor(pred, what = 'condition') {
    for (let i = 0; i < 200; i++) {
        const v = pred();
        if (v) return v;
        await new Promise((r) => setTimeout(r, 5));
    }
    throw new Error(`timed out waiting for ${what}`);
}

/**
 * Boot a fresh runtime over a MessageChannel and send `init`. The test
 * modules record into a fresh `globalThis.__probe`.
 */
async function boot(init) {
    globalThis.__probe = { calls: [], context: null };
    vi.resetModules();
    await import('../../ui/js/extension-sandbox/runtime.js');
    const channel = new MessageChannel();
    const host = channel.port1;
    const inbox = [];
    host.onmessage = (ev) => inbox.push(ev.data);
    window.dispatchEvent(
        new MessageEvent('message', { data: { type: 'handshake' }, ports: [channel.port2] })
    );
    await waitFor(() => inbox.find((m) => m.type === 'ready'), 'ready');
    host.postMessage({ type: 'init', config: { a: 1 }, sharedSources: {}, ...init });
    const ack = await waitFor(() => inbox.find((m) => m.type === 'init-ack'), 'init-ack');
    let nextRpc = 1;
    return {
        host,
        inbox,
        ack,
        async rpc(method, params = {}) {
            const rpcId = nextRpc++;
            host.postMessage({ type: 'rpc', rpcId, method, params });
            const res = await waitFor(
                () => inbox.find((m) => m.type === 'rpc-response' && m.rpcId === rpcId),
                `rpc ${method}`
            );
            if (res.error) throw new Error(res.error);
            return res.result;
        },
        next(type, from = 0) {
            return waitFor(() => inbox.slice(from).find((m) => m.type === type), type);
        },
        close() {
            host.close();
        },
    };
}

const THEME = {
    scheme: 'dark',
    fontSize: 14,
    reducedMotion: false,
    tokens: { 'kage-bg': '#1e1e1e', 'kage-text': '#eeeeee' },
};

function viewInit(overrides = {}) {
    return {
        surface: 'view',
        hostFeatures: ['views'],
        sources: {
            view: VIEW_SOURCE,
            searchProvider: SEARCH_SOURCE,
            sourcePaths: { view: './explorer.js' },
        },
        view: { id: 'explorer', theme: THEME, css: ['.ex-node { color: red }'], links: true },
        ...overrides,
    };
}

describe('hidden surface', () => {
    it('has context.storage but no context.view, and refuses view RPCs', async () => {
        const rt = await boot({ sources: { searchProvider: SEARCH_SOURCE, view: VIEW_SOURCE } });
        expect(rt.ack.ok).toBe(true);
        expect(rt.ack.result.hasSearch).toBe(true);
        expect(rt.ack.result.hasView).toBeUndefined();
        const ctx = globalThis.__probe.context;
        expect(ctx.view).toBeUndefined();
        expect(typeof ctx.storage.onChanged).toBe('function');
        expect(ctx.hostFeatures).toContain('storage.change_events');
        // The view module isn't loaded on the hidden surface.
        expect(globalThis.__probe.calls).toEqual([]);
        await expect(rt.rpc('viewMount', {})).rejects.toThrow(/unknown method 'viewMount'/);
        await expect(rt.rpc('viewPing')).rejects.toThrow(/unknown method/);
        rt.close();
    });
});

describe('view surface', () => {
    let rt;

    beforeAll(async () => {
        globalThis.__probe = { calls: [], context: null };
        window.RTCPeerConnection = function RTCPeerConnection() {};
        window.webkitRTCPeerConnection = function webkitRTCPeerConnection() {};
        rt = await boot(viewInit());
    });

    afterAll(() => rt.close());

    it('loads only the view module and gives it context.view', () => {
        expect(rt.ack.ok).toBe(true);
        expect(rt.ack.result).toMatchObject({ hasView: true, hasSearch: false, widgetIds: [] });
        const ctx = globalThis.__probe.context;
        expect(ctx.view.id).toBe('explorer');
        expect(Object.isFrozen(ctx.view)).toBe(true);
        expect(typeof ctx.view.setTitle).toBe('function');
        expect(ctx.view.theme).toEqual(THEME);
        expect(Object.isFrozen(ctx.view.theme)).toBe(true);
        expect(ctx.hostFeatures).toEqual(expect.arrayContaining(['views', 'storage.change_events']));
    });

    it('removed WebRTC and hides child frame windows before the module loaded', () => {
        expect(window.RTCPeerConnection).toBeUndefined();
        expect(window.webkitRTCPeerConnection).toBeUndefined();
        // Pinned: a module (strict code) can't put it back.
        expect(() => {
            window.RTCPeerConnection = function again() {};
        }).toThrow(TypeError);
        expect(window.RTCPeerConnection).toBeUndefined();
        const frame = document.createElement('iframe');
        document.body.appendChild(frame);
        expect(frame.contentWindow).toBeNull();
        expect(frame.contentDocument).toBeNull();
        frame.remove();
    });

    it('wrote the theme <style> and the view CSS', () => {
        const style = document.getElementById('kage-view-theme');
        expect(style.textContent).toBe(
            ':root { --kage-bg: #1e1e1e; --kage-text: #eeeeee; color-scheme: dark; font-size: 14px }'
        );
        const css = [...document.querySelectorAll('style[data-kage-view-css]')].map((s) => s.textContent);
        expect(css).toEqual(['.ex-node { color: red }']);
    });

    it('viewMount calls mount once with a root and the info', async () => {
        expect(await rt.rpc('viewMount', { params: { focus: 'o' }, source: 'chat', reloaded: false })).toBe(
            true
        );
        const mount = globalThis.__probe.calls.filter((c) => c[0] === 'mount');
        expect(mount).toEqual([['mount', 'kage-view-root', { params: { focus: 'o' }, source: 'chat', reloaded: false }]]);
        expect(globalThis.__probe.root.isConnected).toBe(true);
        await expect(rt.rpc('viewMount', {})).rejects.toThrow(/already mounted/);
    });

    it('viewReopen reaches onOpen', async () => {
        await rt.rpc('viewReopen', { params: { focus: 'e1' }, source: 'launcher' });
        expect(globalThis.__probe.calls.at(-1)).toEqual(['onOpen', { params: { focus: 'e1' }, source: 'launcher' }]);
    });

    it('viewVisibility calls onHide / onShow once per change', async () => {
        const before = globalThis.__probe.calls.length;
        await rt.rpc('viewVisibility', { visible: false });
        await rt.rpc('viewVisibility', { visible: false });
        await rt.rpc('viewVisibility', { visible: true });
        await rt.rpc('viewVisibility', { visible: true });
        expect(globalThis.__probe.calls.slice(before)).toEqual([['onHide'], ['onShow']]);
    });

    it('viewTheme replaces the snapshot and the <style>, then calls onThemeChange', async () => {
        const light = { scheme: 'light', fontSize: 16, tokens: { 'kage-bg': '#fafafa', 'kage-text': 'url(x)' } };
        await rt.rpc('viewTheme', { theme: light });
        const expected = { scheme: 'light', fontSize: 16, reducedMotion: false, tokens: { 'kage-bg': '#fafafa' } };
        expect(globalThis.__probe.calls.at(-1)).toEqual(['onThemeChange', expected]);
        expect(globalThis.__probe.context.view.theme).toEqual(expected);
        expect(document.getElementById('kage-view-theme').textContent).toBe(
            ':root { --kage-bg: #fafafa; color-scheme: light; font-size: 16px }'
        );
    });

    it('viewPing is answered by the runtime', async () => {
        const before = globalThis.__probe.calls.length;
        expect(await rt.rpc('viewPing')).toBe(true);
        expect(globalThis.__probe.calls.length).toBe(before);
    });

    it('onConfigUpdate reaches the view', async () => {
        await rt.rpc('onConfigUpdate', { config: { zoom: 2 } });
        expect(globalThis.__probe.calls.at(-1)).toEqual(['onConfigUpdate', { zoom: 2 }]);
    });

    it('context.view methods post view-requests and settle on the answer', async () => {
        const view = globalThis.__probe.context.view;
        const from = rt.inbox.length;
        const pending = view.setTitle('Bingo');
        const req = await rt.next('view-request', from);
        expect(req).toMatchObject({ kind: 'set-title', text: 'Bingo' });
        rt.host.postMessage({ type: 'view-response', id: req.id, result: 'Bingo' });
        await expect(pending).resolves.toBe('Bingo');

        const from2 = rt.inbox.length;
        const refused = view.copyText('x');
        const req2 = await rt.next('view-request', from2);
        expect(req2).toMatchObject({ kind: 'copy-text', text: 'x' });
        rt.host.postMessage({
            type: 'view-response',
            id: req2.id,
            error: "The extension window isn't in front.",
            errorKind: 'not_permitted',
            errorKey: 'errors.ext_view.not_focused',
        });
        await expect(refused).rejects.toMatchObject({
            message: "The extension window isn't in front.",
            kind: 'not_permitted',
            key: 'errors.ext_view.not_focused',
        });
    });

    it('open, compose, openSettings and close send their kinds', async () => {
        const view = globalThis.__probe.context.view;
        const from = rt.inbox.length;
        view.open('board', { focus: 'o' });
        view.open('board');
        view.compose('mm? Bingo');
        view.openSettings();
        view.close();
        view.setTitle();
        await waitFor(() => rt.inbox.slice(from).filter((m) => m.type === 'view-request').length === 6);
        const reqs = rt.inbox.slice(from).filter((m) => m.type === 'view-request');
        expect(reqs.map(({ id, ...r }) => r)).toEqual([
            { type: 'view-request', kind: 'open-view', view: 'board', params: { focus: 'o' } },
            { type: 'view-request', kind: 'open-view', view: 'board', params: null },
            { type: 'view-request', kind: 'compose', text: 'mm? Bingo' },
            { type: 'view-request', kind: 'open-settings' },
            { type: 'view-request', kind: 'close' },
            { type: 'view-request', kind: 'set-title', text: null },
        ]);
        expect(new Set(reqs.map((r) => r.id)).size).toBe(6);
    });

    it('a value that cannot be cloned rejects at once', async () => {
        await expect(globalThis.__probe.context.view.open('board', { f: () => 1 })).rejects.toThrow();
    });

    it('cancels link clicks and opens absolute http(s) and mailto links with urls', async () => {
        const root = globalThis.__probe.root;
        root.innerHTML =
            '<a id="web" href="https://example.com/a?b=1"><span>go</span></a>' +
            '<a id="rel" href="/settings">rel</a><a id="js" href="javascript:alert(1)">js</a>' +
            '<a id="mail" href="mailto:me@example.com">mail</a><a id="none">plain</a>';
        const from = rt.inbox.length;
        const click = (sel) => {
            const ev = new MouseEvent('click', { bubbles: true, cancelable: true });
            root.querySelector(sel).dispatchEvent(ev);
            return ev.defaultPrevented;
        };
        expect(click('#web span')).toBe(true);
        expect(click('#rel')).toBe(true);
        expect(click('#js')).toBe(true);
        expect(click('#mail')).toBe(true);
        expect(click('#none')).toBe(false);
        await waitFor(() => rt.inbox.slice(from).filter((m) => m.type === 'invoke').length === 2);
        const invokes = rt.inbox.slice(from).filter((m) => m.type === 'invoke');
        expect(invokes.map((m) => [m.command, m.args])).toEqual([
            ['open_url', { url: 'https://example.com/a?b=1' }],
            ['open_url', { url: 'mailto:me@example.com' }],
        ]);
        for (const m of invokes) rt.host.postMessage({ type: 'invoke-response', id: m.id, result: null });
    });

    it('⌘W and Ctrl+W ask the host to close; AZERTY Ctrl+Z does not', async () => {
        const from = rt.inbox.length;
        const key = (init) => {
            const ev = new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...init });
            document.body.dispatchEvent(ev);
            return ev.defaultPrevented;
        };
        expect(key({ key: 'w', code: 'KeyW', metaKey: true })).toBe(true);
        expect(key({ key: 'W', code: 'KeyW', ctrlKey: true })).toBe(true);
        expect(key({ key: 'z', code: 'KeyW', ctrlKey: true })).toBe(false);
        expect(key({ key: 'w', code: 'KeyW' })).toBe(false);
        expect(key({ key: 'w', code: 'KeyW', metaKey: true, shiftKey: true })).toBe(false);
        await waitFor(() => rt.inbox.slice(from).filter((m) => m.type === 'view-request').length === 2);
        await new Promise((r) => setTimeout(r, 20));
        const closes = rt.inbox.slice(from).filter((m) => m.type === 'view-request');
        expect(closes.map((m) => m.kind)).toEqual(['close', 'close']);
    });

    it('context.storage.onChanged delivers, unsubscribes and survives a throwing listener', async () => {
        const storage = globalThis.__probe.context.storage;
        const seen = [];
        const off = storage.onChanged((changes) => seen.push(changes));
        storage.onChanged(() => {
            throw new Error('listener bug');
        });
        const fromLog = rt.inbox.length;
        rt.host.postMessage({
            type: 'storage-changed',
            changes: [
                { key: 'mm-meta', op: 'save', self: false, source: 'window' },
                { key: 'mm-layout', op: 'delete', self: true, source: 'sync' },
                { key: 3, op: 'save' },
                { key: 'x', op: 'rename' },
            ],
        });
        await waitFor(() => seen.length === 1, 'storage change');
        expect(seen[0]).toEqual([
            { key: 'mm-meta', op: 'save', self: false, source: 'window' },
            { key: 'mm-layout', op: 'delete', self: true, source: 'sync' },
        ]);
        expect(Object.isFrozen(seen[0])).toBe(true);
        const log = await rt.next('log', fromLog);
        expect(log.msg).toMatch(/storage.onChanged listener failed: listener bug/);
        off();
        rt.host.postMessage({ type: 'storage-changed', changes: [{ key: 'k', op: 'save', source: 'window' }] });
        await new Promise((r) => setTimeout(r, 30));
        expect(seen).toHaveLength(1);
    });

    it('destroy reaches the view', async () => {
        rt.host.postMessage({ type: 'rpc', rpcId: -1, method: 'destroy', params: {} });
        await waitFor(() => globalThis.__probe.calls.some((c) => c[0] === 'destroy'), 'destroy');
    });
});

describe('view surface without urls', () => {
    it('drops link clicks and logs a warning', async () => {
        globalThis.__probe = { calls: [], context: null };
        const rt = await boot(viewInit({ view: { id: 'explorer', theme: THEME, links: false } }));
        await rt.rpc('viewMount', {});
        const root = globalThis.__probe.root;
        root.innerHTML = '<a id="web-no-urls" href="https://example.com/">go</a>';
        const from = rt.inbox.length;
        const ev = new MouseEvent('click', { bubbles: true, cancelable: true });
        root.querySelector('#web-no-urls').dispatchEvent(ev);
        expect(ev.defaultPrevented).toBe(true);
        const log = await rt.next('log', from);
        expect(log).toMatchObject({ level: 'warn' });
        expect(log.msg).toMatch(/needs the 'urls' capability/);
        expect(rt.inbox.slice(from).some((m) => m.type === 'invoke')).toBe(false);
        rt.close();
    });

    it('reports a view module that failed to load', async () => {
        globalThis.__probe = { calls: [], context: null };
        const rt = await boot(
            viewInit({ sources: { view: 'export const notDefault = 1;', sourcePaths: { view: './v.js' } } })
        );
        expect(rt.ack.result.hasView).toBe(false);
        await expect(rt.rpc('viewMount', {})).rejects.toThrow(/did not load/);
        rt.close();
    });
});

describe('view-api pieces', () => {
    it('viewLinkTarget: absolute http(s) and mailto only', () => {
        expect(viewLinkTarget('https://example.com')).toBe('https://example.com/');
        expect(viewLinkTarget(' http://a.test/x ')).toBe('http://a.test/x');
        expect(viewLinkTarget('mailto:a@b.c')).toBe('mailto:a@b.c');
        for (const bad of ['/x', 'x.html', 'javascript:alert(1)', 'data:text/html,x', 'file:///etc/hosts', 'tauri://localhost', '', null]) {
            expect(viewLinkTarget(bad), String(bad)).toBeNull();
        }
    });

    it('isViewCloseShortcut', () => {
        expect(isViewCloseShortcut({ key: 'w', metaKey: true })).toBe(true);
        expect(isViewCloseShortcut({ key: 'ц', code: 'KeyW', ctrlKey: true })).toBe(true);
        expect(isViewCloseShortcut({ key: 'w', ctrlKey: true, repeat: true })).toBe(false);
        expect(isViewCloseShortcut({ key: 'w', ctrlKey: true, altKey: true })).toBe(false);
        expect(isViewCloseShortcut(null)).toBe(false);
    });

    it('removeWebRtc works on a plain object too', () => {
        const fake = { RTCPeerConnection() {}, RTCDataChannel() {} };
        removeWebRtc(fake);
        expect(fake.RTCPeerConnection).toBeUndefined();
        expect(fake.RTCDataChannel).toBeUndefined();
        expect('webkitRTCPeerConnection' in fake).toBe(true);
        expect(fake.webkitRTCPeerConnection).toBeUndefined();
    });

    it('createViewSurface: visibility before mount only records the state', async () => {
        const doc = document.implementation.createHTMLDocument('v');
        const surface = createViewSurface({
            viewId: 'v',
            theme: {},
            links: false,
            post: () => {},
            invoke: async () => {},
            log: () => {},
            toError: (m) => new Error(m.error),
            win: window,
            doc,
        });
        const calls = [];
        surface.attach({ onHide: () => calls.push('hide'), onShow: () => calls.push('show') });
        expect(await surface.visibility({ visible: false })).toBe(false);
        expect(await surface.reopen({ params: {} })).toBe(false);
        await surface.mount({});
        expect(await surface.visibility({ visible: true })).toBe(true);
        expect(calls).toEqual(['show']);
        surface.handleResponse({ id: 999, result: 1 });
    });

    it('createStorageEvents needs a function', () => {
        const { api } = createStorageEvents({ log: () => {} });
        expect(() => api.onChanged('nope')).toThrow(TypeError);
        expect(Object.isFrozen(api)).toBe(true);
        expect(HOST_FEATURES).toContain('storage.change_events');
    });
});
