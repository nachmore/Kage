/**
 * The view surface of ExtensionSandbox (docs/design/EXTENSION_VIEWS.md
 * §8.3–§8.8, §10): the iframe's attributes, the srcdoc and init message,
 * the navigation detector, `view-request` handling, the view-surface
 * denylist, storage change forwarding and the view RPCs. Like
 * extension-sandbox-host.test.js, this drives the host with a fake port
 * instead of a real null-origin iframe.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
    ExtensionSandbox,
    ExtensionSandboxPool,
    HOST_FEATURE_DATE_PICKER,
    HOST_FEATURE_VIEWS,
    hostFeaturesFor,
    SURFACE_HIDDEN,
    SURFACE_VIEW,
    VIEW_MOUNT_TIMEOUT_MS,
} from '../../ui/js/shared/extension-sandbox-host.js';
import { VIEW_CSP, VIEW_IFRAME_ALLOW } from '../../ui/js/shared/extension-view-policy.js';

const VIEWS = [
    { id: 'explorer', title: 'Explorer', module: './explorer.js' },
    { id: 'board', module: './board.js' },
];

function viewSpec(overrides = {}) {
    return {
        extensionId: 'mindmap',
        capabilities: ['storage', 'views', 'urls'],
        config: {},
        sources: { view: 'export default class {}', sourcePaths: { view: './explorer.js' } },
        hostFeatures: [HOST_FEATURE_VIEWS],
        surface: SURFACE_VIEW,
        viewId: 'explorer',
        views: VIEWS,
        viewTitle: 'Explorer',
        extensionName: 'Mind Map',
        viewTheme: { scheme: 'dark', tokens: { 'kage-bg': '#1e1e1e' } },
        viewCss: ['.x { color: red }'],
        ...overrides,
    };
}

function makeView({ spec = {}, hooks = {}, rawInvoke = vi.fn().mockResolvedValue(null) } = {}) {
    const viewHooks = {
        setTitle: vi.fn(),
        close: vi.fn(),
        onNavigated: vi.fn(),
        hasUserActivation: () => true,
        ...hooks,
    };
    const sb = new ExtensionSandbox(viewSpec({ viewHooks, ...spec }), rawInvoke, document.body);
    const sent = [];
    sb._port = { postMessage: (m) => sent.push(m), close: () => {} };
    sb._ready = true;
    return { sb, sent, rawInvoke, hooks: viewHooks };
}

function request(sb, kind, fields = {}, id = 1) {
    return sb._handleViewRequest({ type: 'view-request', id, kind, ...fields });
}

afterEach(() => {
    vi.restoreAllMocks();
    document.body.innerHTML = '';
});

describe('view surface: construction', () => {
    it('is the hidden surface unless asked', () => {
        const sb = new ExtensionSandbox({ extensionId: 'x', capabilities: [] }, vi.fn(), document.body);
        expect(sb.surface).toBe(SURFACE_HIDDEN);
        expect(new ExtensionSandbox({ extensionId: 'x', surface: 'other' }, vi.fn(), document.body).surface).toBe(
            SURFACE_HIDDEN
        );
    });

    it('needs a view id the manifest declares', () => {
        expect(() => new ExtensionSandbox(viewSpec({ viewId: 'missing' }), vi.fn(), document.body)).toThrow(
            /view id the manifest declares/
        );
        expect(() => new ExtensionSandbox(viewSpec({ viewId: undefined }), vi.fn(), document.body)).toThrow();
        expect(() => new ExtensionSandbox(viewSpec({ views: 'nope' }), vi.fn(), document.body)).toThrow();
    });

    it('offers the views host feature only with the views capability', () => {
        expect(hostFeaturesFor([HOST_FEATURE_VIEWS], ['views'])).toEqual([HOST_FEATURE_VIEWS]);
        expect(hostFeaturesFor([HOST_FEATURE_VIEWS, HOST_FEATURE_DATE_PICKER], ['calendar'])).toEqual([
            HOST_FEATURE_DATE_PICKER,
        ]);
        const { sb } = makeView({ spec: { capabilities: ['storage'] } });
        expect(sb.hostFeatures).toEqual([]);
    });

    it('drops CSS over the size limits', () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const big = 'a'.repeat(256 * 1024 + 1);
        const { sb } = makeView({ spec: { viewCss: ['.a{}', big, 42, '.b{}'] } });
        expect(sb._viewCss).toEqual(['.a{}', '.b{}']);
    });
});

describe('view surface: the iframe', () => {
    it('has exactly sandbox="allow-scripts", the allow denials, no-referrer and a title', () => {
        const { sb } = makeView();
        const f = sb._createIframe();
        expect(f.getAttribute('sandbox')).toBe('allow-scripts');
        expect(f.getAttribute('sandbox').split(/\s+/)).toEqual(['allow-scripts']);
        expect(f.getAttribute('allow')).toBe(VIEW_IFRAME_ALLOW);
        expect(f.getAttribute('referrerpolicy')).toBe('no-referrer');
        expect(f.hasAttribute('aria-hidden')).toBe(false);
        expect(f.title).toBeTruthy();
        expect(f.style.pointerEvents).not.toBe('none');
        expect(f.style.opacity).toBe('');
        expect(f.dataset.extensionId).toBe('mindmap');
        expect(f.dataset.viewId).toBe('explorer');
    });

    it('leaves the hidden iframe as it was', () => {
        const sb = new ExtensionSandbox({ extensionId: 'x', capabilities: [] }, vi.fn(), document.body);
        const f = sb._createIframe();
        expect(f.getAttribute('sandbox')).toBe('allow-scripts');
        expect(f.getAttribute('aria-hidden')).toBe('true');
        expect(f.hasAttribute('allow')).toBe(false);
        expect(f.style.opacity).toBe('0');
        expect(f.style.pointerEvents).toBe('none');
    });
});

/** start() with the iframe load, handshake and init stubbed out. */
async function startWithStubs(sb, initResult = { hasView: true }) {
    vi.stubGlobal(
        'fetch',
        vi.fn(async (p) => ({ ok: true, status: 200, text: async () => `/* ${p} */\n` }))
    );
    const make = sb._createIframe.bind(sb);
    sb._createIframe = () => {
        const f = make();
        Object.defineProperty(f, 'contentWindow', { value: { postMessage: vi.fn() } });
        return f;
    };
    sb._waitForIframeLoad = async () => {};
    sb._waitForReady = async () => {
        sb._ready = true;
    };
    let init = null;
    sb._rpcRaw = vi.fn(async (msg) => {
        init = msg;
        return { ok: true, result: initResult };
    });
    await sb.start();
    return init;
}

describe('view surface: start', () => {
    it('writes the view srcdoc with the CSP first and sends the view init', async () => {
        const { sb } = makeView();
        sb._port = null;
        sb._ready = false;
        sb.i18nLanguage = 'ar';
        sb.i18nRtl = true;
        const init = await startWithStubs(sb);
        const html = sb._iframe.srcdoc;
        expect(html).toMatch(/^<!DOCTYPE html><html lang="ar" dir="rtl"><head><meta http-equiv="Content-Security-Policy"/);
        expect(html).toContain(VIEW_CSP);
        expect(html).toContain('--kage-bg: #1e1e1e');
        expect(init).toMatchObject({
            type: 'init',
            surface: 'view',
            hostFeatures: [HOST_FEATURE_VIEWS],
            view: {
                id: 'explorer',
                css: ['.x { color: red }'],
                links: true,
                theme: { scheme: 'dark', tokens: { 'kage-bg': '#1e1e1e' } },
            },
        });
        expect(init.sources.view).toBe('export default class {}');
        expect(sb.hasView).toBe(true);
        sb.destroy();
    });

    it('sends links: false without the urls capability', async () => {
        const { sb } = makeView({ spec: { capabilities: ['storage', 'views'] } });
        sb._port = null;
        sb._ready = false;
        const init = await startWithStubs(sb, { hasView: false });
        expect(init.view.links).toBe(false);
        expect(sb.hasView).toBe(false);
        sb.destroy();
    });

    it('a second iframe load tears the sandbox down and reports it', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const { sb, hooks } = makeView();
        sb._port = null;
        sb._ready = false;
        await startWithStubs(sb);
        const iframe = sb._iframe;
        expect(iframe.isConnected).toBe(true);
        iframe.dispatchEvent(new Event('load'));
        expect(sb._destroyed).toBe(true);
        expect(iframe.isConnected).toBe(false);
        expect(hooks.onNavigated).toHaveBeenCalledTimes(1);
        iframe.dispatchEvent(new Event('load'));
        expect(hooks.onNavigated).toHaveBeenCalledTimes(1);
    });

    it('the hidden surface keeps its srcdoc and has no navigation detector', async () => {
        const sb = new ExtensionSandbox({ extensionId: 'x', capabilities: [] }, vi.fn(), document.body);
        const init = await startWithStubs(sb, {});
        expect(sb._iframe.srcdoc).toMatch(/^<!DOCTYPE html><html><head><meta charset="UTF-8"><\/head><body><script>/);
        expect(sb._iframe.srcdoc).not.toContain('Content-Security-Policy');
        expect(init).not.toHaveProperty('surface');
        expect(init).not.toHaveProperty('view');
        sb._iframe.dispatchEvent(new Event('load'));
        expect(sb._destroyed).toBe(false);
        sb.destroy();
    });
});

describe('view-request', () => {
    it('set-title cleans and caps the subtitle, null clears it', async () => {
        const { sb, sent, hooks } = makeView();
        const rlo = String.fromCharCode(0x202e);
        await request(sb, 'set-title', { text: ` Bin${rlo}go ` });
        expect(hooks.setTitle).toHaveBeenLastCalledWith('Bingo');
        expect(sent.at(-1)).toEqual({ type: 'view-response', id: 1, result: 'Bingo' });
        await request(sb, 'set-title', { text: 'x'.repeat(100) }, 2);
        expect(hooks.setTitle).toHaveBeenLastCalledWith('x'.repeat(80));
        await request(sb, 'set-title', { text: null }, 3);
        expect(hooks.setTitle).toHaveBeenLastCalledWith(null);
        await request(sb, 'set-title', { text: '   ' }, 4);
        expect(hooks.setTitle).toHaveBeenLastCalledWith(null);
    });

    it('set-title refuses a non-string and is limited to 10 per second', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const { sb, sent, hooks } = makeView();
        await request(sb, 'set-title', { text: 42 });
        expect(sent[0]).toMatchObject({ id: 1, errorKind: 'invalid_input', errorKey: 'errors.ext_view.text_invalid' });
        expect(hooks.setTitle).not.toHaveBeenCalled();
        for (let i = 0; i < 10; i++) await request(sb, 'set-title', { text: `t${i}` }, 10 + i);
        await request(sb, 'set-title', { text: 'one too many' }, 99);
        expect(hooks.setTitle).toHaveBeenCalledTimes(10);
        expect(sent.at(-1)).toMatchObject({ id: 99, errorKind: 'rate_limited', errorKey: 'errors.ext_view.too_fast' });
    });

    it('logs a rate-limit refusal once until that kind is allowed again', async () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        let now = 1_000_000;
        vi.spyOn(Date, 'now').mockImplementation(() => now);
        const { sb } = makeView();
        for (let i = 0; i < 15; i++) await request(sb, 'set-title', { text: 't' }, i + 1);
        const limited = () => warn.mock.calls.filter((c) => /set-title refused/.test(c[0])).length;
        expect(limited()).toBe(1);
        now += 1000;
        await request(sb, 'set-title', { text: 't' }, 50);
        for (let i = 0; i < 12; i++) await request(sb, 'set-title', { text: 't' }, 60 + i);
        expect(limited()).toBe(2);
    });

    it('close calls the page hook', async () => {
        const { sb, sent, hooks } = makeView();
        await request(sb, 'close');
        expect(hooks.close).toHaveBeenCalledTimes(1);
        expect(sent[0]).toEqual({ type: 'view-response', id: 1, result: null });
    });

    it('open-view opens a declared view with source view', async () => {
        const rawInvoke = vi.fn().mockResolvedValue({ label: 'ext-view:mindmap:board', reused: true });
        const { sb, sent } = makeView({ rawInvoke });
        await request(sb, 'open-view', { view: 'board', params: { focus: 'o' } });
        expect(rawInvoke).toHaveBeenCalledWith('ext_view_open', {
            extensionId: 'mindmap',
            viewId: 'board',
            params: { focus: 'o' },
            source: 'view',
        });
        expect(sent[0]).toEqual({ type: 'view-response', id: 1, result: { reused: true } });
    });

    it('open-view runs the §6.2 checks, then activation, then the rate limit', async () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        let active = false;
        const { sb, sent, rawInvoke } = makeView({ hooks: { hasUserActivation: () => active } });
        await request(sb, 'open-view', { view: 'nope' }, 1);
        expect(sent.at(-1)).toMatchObject({ errorKind: 'invalid_input', errorKey: 'errors.ext_view.unknown_view' });
        expect(warn.mock.calls.at(-1)[0]).toMatch(/^\[ext mindmap\] open_view refused: /);
        await request(sb, 'open-view', { view: 'board', params: [1] }, 2);
        expect(sent.at(-1)).toMatchObject({ errorKey: 'errors.ext_view.params_invalid' });
        await request(sb, 'open-view', { view: 'board' }, 3);
        expect(sent.at(-1)).toMatchObject({ errorKind: 'not_permitted', errorKey: 'errors.ext_view.needs_user_action' });
        expect(rawInvoke).not.toHaveBeenCalled();
        active = true;
        await request(sb, 'open-view', { view: 'board' }, 4);
        expect(rawInvoke).toHaveBeenCalledTimes(1);
        await request(sb, 'open-view', { view: 'board' }, 5);
        expect(sent.at(-1)).toMatchObject({ id: 5, errorKind: 'rate_limited' });
        expect(rawInvoke).toHaveBeenCalledTimes(1);
    });

    it('open-view needs the views grant and host feature', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const { sb, sent, rawInvoke } = makeView({ spec: { capabilities: ['storage'] } });
        await request(sb, 'open-view', { view: 'board' });
        expect(sent[0]).toMatchObject({ errorKind: 'not_permitted', errorKey: 'errors.ext_view.opener_refused' });
        expect(rawInvoke).not.toHaveBeenCalled();
    });

    it('compose checks the text, activation and rate, then calls ext_view_compose', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const { sb, sent, rawInvoke } = makeView();
        await request(sb, 'compose', { text: `ls${String.fromCharCode(27)}[2J` }, 1);
        expect(sent.at(-1)).toMatchObject({ errorKind: 'invalid_input', errorKey: 'errors.ext_view.text_invalid' });
        await request(sb, 'compose', { text: 'x'.repeat(2001) }, 2);
        expect(sent.at(-1)).toMatchObject({ errorKey: 'errors.ext_view.text_invalid' });
        await request(sb, 'compose', { text: 'mm? tell me about Bingo' }, 3);
        expect(rawInvoke).toHaveBeenLastCalledWith('ext_view_compose', { text: 'mm? tell me about Bingo' });
        await request(sb, 'compose', { text: 'again' }, 4);
        await request(sb, 'compose', { text: 'third' }, 5);
        expect(rawInvoke).toHaveBeenCalledTimes(2);
        expect(sent.at(-1)).toMatchObject({ id: 5, errorKind: 'rate_limited' });
    });

    it('copy-text allows 8192 characters and calls ext_view_copy_text', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const { sb, sent, rawInvoke } = makeView();
        await request(sb, 'copy-text', { text: 'x'.repeat(8193) }, 1);
        expect(sent.at(-1)).toMatchObject({ errorKey: 'errors.ext_view.text_invalid' });
        await request(sb, 'copy-text', { text: '/path/to/doc.md' }, 2);
        expect(rawInvoke).toHaveBeenCalledWith('ext_view_copy_text', { text: '/path/to/doc.md' });
        expect(sent.at(-1)).toEqual({ type: 'view-response', id: 2, result: null });
    });

    it('copy-text without activation is refused', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const { sb, sent, rawInvoke } = makeView({ hooks: { hasUserActivation: () => false } });
        await request(sb, 'copy-text', { text: 'hi' });
        expect(sent[0]).toMatchObject({ errorKind: 'not_permitted' });
        expect(rawInvoke).not.toHaveBeenCalled();
    });

    it("open-settings opens this extension's own page", async () => {
        const { sb, rawInvoke } = makeView();
        await request(sb, 'open-settings', { section: 'agents' });
        expect(rawInvoke).toHaveBeenCalledWith('open_settings_window', { section: 'ext-mindmap' });
    });

    it('page hooks replace the default commands', async () => {
        const openView = vi.fn().mockResolvedValue({ reused: false });
        const compose = vi.fn();
        const { sb, rawInvoke } = makeView({ hooks: { openView, compose } });
        await request(sb, 'open-view', { view: 'explorer' }, 1);
        await request(sb, 'compose', { text: 'hi' }, 2);
        expect(openView).toHaveBeenCalledWith('explorer', null);
        expect(compose).toHaveBeenCalledWith('hi');
        expect(rawInvoke).not.toHaveBeenCalled();
    });

    it('forwards a backend AppError kind and key', async () => {
        const rawInvoke = vi.fn().mockRejectedValue({
            kind: 'not_permitted',
            key: 'errors.ext_view.not_focused',
            message: "The extension window isn't in front.",
        });
        const { sb, sent } = makeView({ rawInvoke });
        await request(sb, 'copy-text', { text: 'hi' });
        expect(sent[0]).toEqual({
            type: 'view-response',
            id: 1,
            error: "The extension window isn't in front.",
            errorKind: 'not_permitted',
            errorKey: 'errors.ext_view.not_focused',
        });
    });

    it('ignores unknown kinds, bad ids and requests on the hidden surface', async () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const { sb, sent } = makeView();
        await request(sb, 'pop-up', {});
        await request(sb, 'close', {}, 'one');
        expect(sent).toEqual([]);
        expect(warn).toHaveBeenCalledWith(expect.stringMatching(/unknown view request 'pop-up'/));

        const hidden = new ExtensionSandbox({ extensionId: 'x', capabilities: ['views'] }, vi.fn(), document.body);
        const hiddenSent = [];
        hidden._port = { postMessage: (m) => hiddenSent.push(m), close() {} };
        hidden._ready = true;
        hidden._onPortMessage({ data: { type: 'view-request', id: 1, kind: 'close' } });
        await Promise.resolve();
        expect(hiddenSent).toEqual([]);
    });

    it('arrives through the port and is not answered after destroy', async () => {
        let release;
        const close = vi.fn(() => new Promise((r) => (release = r)));
        const { sb, sent } = makeView({ hooks: { close } });
        sb._onPortMessage({ data: { type: 'view-request', id: 7, kind: 'close' } });
        expect(close).toHaveBeenCalledTimes(1);
        sb.destroy();
        release();
        await new Promise((r) => setTimeout(r, 0));
        expect(sent.filter((m) => m.type === 'view-response')).toEqual([]);
    });
});

describe('view-surface denylist', () => {
    it('refuses window-bound commands in a view, whatever the grant', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const rawInvoke = vi.fn().mockResolvedValue('ok');
        const { sb, sent } = makeView({ rawInvoke, spec: { capabilities: ['views', 'agent', 'storage'] } });
        await sb._handleInvoke({ id: 1, command: 'send_message_streaming', args: {} });
        expect(rawInvoke).not.toHaveBeenCalled();
        expect(sent[0].error).toMatch(/isn't available in an extension window/);
        await sb._handleInvoke({ id: 2, command: 'open_chat_with_message', args: { message: 'x' } });
        expect(rawInvoke).toHaveBeenCalledWith('open_chat_with_message', { message: 'x' });
    });

    it('allows them on the hidden surface', async () => {
        const rawInvoke = vi.fn().mockResolvedValue('ok');
        const sb = new ExtensionSandbox({ extensionId: 'x', capabilities: ['agent'] }, rawInvoke, document.body);
        sb._port = { postMessage: () => {}, close() {} };
        sb._ready = true;
        await sb._handleInvoke({ id: 1, command: 'send_message_streaming', args: {} });
        expect(rawInvoke).toHaveBeenCalledWith('send_message_streaming', {});
    });
});

describe('storage-changed', () => {
    it('reaches the sandbox only with storage, cleaned', () => {
        const { sb, sent } = makeView();
        expect(
            sb.notifyStorageChanged([
                { key: 'mm-meta', op: 'save', source: 'window', window: 'floating', self: false },
            ])
        ).toBe(true);
        expect(sent[0]).toEqual({
            type: 'storage-changed',
            changes: [{ key: 'mm-meta', op: 'save', self: false, source: 'window' }],
        });
        expect(sb.notifyStorageChanged([])).toBe(false);
        expect(sb.notifyStorageChanged([{ key: 'k', op: 'nope' }])).toBe(false);

        const { sb: noStorage, sent: none } = makeView({ spec: { capabilities: ['views'] } });
        expect(noStorage.notifyStorageChanged([{ key: 'k', op: 'save', source: 'window' }])).toBe(false);
        expect(none).toEqual([]);
    });

    it('works on the hidden surface and stops after destroy', () => {
        const sb = new ExtensionSandbox({ extensionId: 'x', capabilities: ['storage'] }, vi.fn(), document.body);
        const sent = [];
        sb._port = { postMessage: (m) => sent.push(m), close() {} };
        sb._ready = true;
        expect(sb.notifyStorageChanged([{ key: 'k', op: 'delete', source: 'sync', self: true }])).toBe(true);
        expect(sent[0].changes).toEqual([{ key: 'k', op: 'delete', self: true, source: 'sync' }]);
        sb.destroy();
        expect(sb.notifyStorageChanged([{ key: 'k', op: 'save', source: 'window' }])).toBe(false);
    });

    it('the pool routes a batch to the matching sandbox only', () => {
        const pool = new ExtensionSandboxPool(vi.fn());
        const posts = {};
        for (const id of ['mindmap', 'todos']) {
            const sb = new ExtensionSandbox({ extensionId: id, capabilities: ['storage'] }, vi.fn(), document.body);
            posts[id] = [];
            sb._port = { postMessage: (m) => posts[id].push(m), close() {} };
            sb._ready = true;
            pool._sandboxes.set(id, sb);
        }
        expect(pool.notifyStorageChanged('mindmap', [{ key: 'mm-meta', op: 'save', source: 'window' }])).toBe(true);
        expect(pool.notifyStorageChanged('other', [{ key: 'k', op: 'save', source: 'window' }])).toBe(false);
        expect(posts.mindmap).toHaveLength(1);
        expect(posts.todos).toEqual([]);
    });
});

describe('view RPCs', () => {
    it('mountView, reopenView, setViewVisibility and pingView call the runtime', () => {
        const { sb } = makeView();
        const call = vi.spyOn(sb, 'call').mockResolvedValue(true);
        sb.mountView({ params: { focus: 'o' }, source: 'chat', reloaded: true });
        expect(call).toHaveBeenLastCalledWith(
            'viewMount',
            { params: { focus: 'o' }, source: 'chat', reloaded: true },
            { timeoutMs: VIEW_MOUNT_TIMEOUT_MS }
        );
        sb.mountView({ source: 'deep-link' }, { timeoutMs: 50 });
        expect(call).toHaveBeenLastCalledWith(
            'viewMount',
            { params: null, source: 'launcher', reloaded: false },
            { timeoutMs: 50 }
        );
        sb.reopenView({ params: { focus: 'e1' }, source: 'launcher' });
        expect(call).toHaveBeenLastCalledWith('viewReopen', { params: { focus: 'e1' }, source: 'launcher' });
        sb.setViewVisibility(0);
        expect(call).toHaveBeenLastCalledWith('viewVisibility', { visible: false });
        sb.pingView();
        expect(call).toHaveBeenLastCalledWith('viewPing', {}, { timeoutMs: 5000 });
    });

    it('setViewTheme sends only a changed, sanitised snapshot', async () => {
        const { sb } = makeView();
        const call = vi.spyOn(sb, 'call').mockResolvedValue(true);
        expect(await sb.setViewTheme({ scheme: 'dark', tokens: { 'kage-bg': '#1e1e1e' } })).toBe(false);
        expect(call).not.toHaveBeenCalled();
        expect(
            await sb.setViewTheme({ scheme: 'light', tokens: { 'kage-bg': '#fafafa', 'kage-text': 'url(x)' } })
        ).toBe(true);
        expect(call).toHaveBeenCalledWith('viewTheme', {
            theme: { scheme: 'light', fontSize: 14, reducedMotion: false, tokens: { 'kage-bg': '#fafafa' } },
        });
        expect(sb.viewTheme.scheme).toBe('light');
    });

    it('are refused on the hidden surface', () => {
        const sb = new ExtensionSandbox({ extensionId: 'x', capabilities: [] }, vi.fn(), document.body);
        expect(() => sb.mountView()).toThrow(/only for view sandboxes/);
        expect(() => sb.pingView()).toThrow(/only for view sandboxes/);
        return expect(sb.setViewTheme({})).rejects.toThrow(/only for view sandboxes/);
    });

    it('hasUserActivation uses the hook and fails closed when it throws', () => {
        const { sb } = makeView({
            hooks: {
                hasUserActivation: () => {
                    throw new Error('boom');
                },
            },
        });
        expect(sb.hasUserActivation()).toBe(false);
        const { sb: truthy } = makeView({ hooks: { hasUserActivation: () => 'yes' } });
        expect(truthy.hasUserActivation()).toBe(false);
    });
});
