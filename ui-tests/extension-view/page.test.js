/**
 * The extension view page's controller (docs/design/EXTENSION_VIEWS.md §8,
 * §6.3, §7.3, §10, §11, §13): boot order, every boot error's panel, the
 * reload-in-place triggers, the view's requests and the input guards.
 * Every dependency is a fake; the sandbox is a stand-in that records RPCs.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { hostFeaturesFor } from '../../ui/js/shared/extension-sandbox-host.js';
import {
    CSS_FILE_MAX_BYTES,
    createViewPage,
    grantedCapabilities,
    MOUNT_TIMEOUT_MS,
    normalizeViewSource,
    OPEN_VIEW_PER_SECOND,
    recentUserActivation,
    SET_TITLE_PER_SECOND,
} from '../../ui/js/extension-view/page.js';
import { fakeT, mountViewHtml } from './fixture.js';

const LABEL = 'ext-view:mindmap:explorer';

const MANIFEST = Object.freeze({
    id: 'mindmap',
    name: 'Mind Map',
    version: '1.0.0',
    icon: '🧠',
    permissions: ['storage', 'views'],
    config: { scrollZooms: { type: 'boolean', default: false } },
    contributes: {
        views: [
            {
                id: 'explorer',
                title: 'Explorer',
                icon: '🗺️',
                module: './explorer.js',
                css: ['./explorer.css'],
            },
            { id: 'board', title: 'Board', module: './board.js' },
        ],
    },
});

const CONFIG = Object.freeze({
    ui: { theme: 'dark', font_size: 14 },
    extension_states: {},
    extension_grants: { mindmap: { granted: ['storage', 'views'], version: '1.0.0' } },
    extensions: {},
});

/** Stand-in for `ExtensionSandbox` with surface 'view'. */
class FakeSandbox {
    constructor(spec, container, h) {
        this.spec = spec;
        this.container = container;
        this.h = h;
        this.calls = [];
        this.storage = [];
        this.configs = [];
        this.destroyed = false;
        this.started = false;
        this.onViewRequest = null;
        this.onNavigated = null;
    }

    async start() {
        this.h.order.push('sandbox.start');
        if (this.h.state.startGate) await this.h.state.startGate;
        if (this.h.state.failStart) throw new Error('did not signal ready');
        this.iframe = document.createElement('iframe');
        this.container.appendChild(this.iframe);
        this.started = true;
    }

    call(method, params, opts) {
        this.calls.push({ method, params, opts });
        if (method !== 'viewPing') this.h.order.push(`rpc:${method}`);
        const handler = this.h.state.rpc[method];
        return handler
            ? Promise.resolve().then(() => handler(params, opts, this))
            : Promise.resolve(null);
    }

    rpcs(method) {
        return this.calls.filter((c) => c.method === method);
    }

    notifyStorageChanged(changes) {
        this.storage.push(changes);
    }

    updateConfig(config) {
        this.configs.push(config);
    }

    destroy() {
        this.destroyed = true;
        this.iframe?.remove();
    }

    /** What the sandbox host does with a type-checked `view-request`. */
    request(req) {
        return this.onViewRequest(req);
    }
}

/** The JS mirror of `normalize_views`, reduced to what the page relies on. */
function fakeNormalizeViews(raw, name, icon) {
    if (!Array.isArray(raw)) return [];
    const out = [];
    for (const e of raw.slice(0, 4)) {
        if (!e || typeof e !== 'object') continue;
        if (!/^[a-z][a-z0-9-]{0,31}$/.test(e.id ?? '') || typeof e.module !== 'string') continue;
        if (out.some((v) => v.id === e.id)) continue;
        out.push({
            id: e.id,
            title: e.title || name,
            icon: e.icon || icon,
            module: e.module,
            css: Array.isArray(e.css) ? e.css : [],
        });
    }
    return out;
}

function fakeSanitize(text, max) {
    const clean = String(text)
        .replace(/[\u0000-\u001f\u007f-\u009f\u202A-\u202E\u2066-\u2069]/g, '')
        .trim();
    return Array.from(clean).slice(0, max).join('');
}

function harness(overrides = {}) {
    const state = {
        manifest: structuredClone(MANIFEST),
        enabled: true,
        config: structuredClone(CONFIG),
        boot: {
            extension_id: 'mindmap',
            view_id: 'explorer',
            params: { focus: 'o' },
            open_seq: 1,
            source: 'launcher',
            reloaded: false,
        },
        bootError: null,
        files: {
            'explorer.js': 'export default class Explorer {}',
            'explorer.css': '.ex { color: red }',
            'board.js': 'export default class Board {}',
        },
        locale: {},
        focused: true,
        activation: { isActive: true },
        visible: true,
        failStart: false,
        rpc: {},
        tokens: { '--kage-bg': '#1e1e1e', '--kage-text': '#e2e8f0' },
        ...overrides,
    };
    const h = { state, order: [], sandboxes: [], listeners: {}, configHandlers: [] };
    h.invoke = vi.fn(async (cmd, args) => {
        h.order.push(cmd);
        switch (cmd) {
            case 'ext_view_boot':
                if (state.bootError) throw state.bootError;
                return structuredClone(state.boot);
            case 'list_extensions':
                return state.manifest
                    ? [{ manifest: structuredClone(state.manifest), enabled: state.enabled }]
                    : [];
            case 'read_extension_locale':
                return state.locale[args.language] ?? null;
            case 'read_extension_file':
                if (Object.hasOwn(state.files, args.filePath)) return state.files[args.filePath];
                throw { kind: 'not_found', key: 'errors.passthrough', message: 'missing' };
            case 'ext_view_open':
                return { label: `ext-view:mindmap:${args.viewId}`, reused: false };
            case 'ext_view_compose':
            case 'ext_view_copy_text':
            case 'open_settings_window':
            case 'open_store_window':
                return null;
            default:
                throw new Error(`unhandled invoke ${cmd}`);
        }
    });
    h.listen = vi.fn(async (event, handler) => {
        h.order.push(`listen:${event}`);
        (h.listeners[event] ||= []).push(handler);
        return () => {};
    });
    h.emit = (event, payload) => {
        for (const handler of h.listeners[event] || []) handler({ event, payload });
    };
    h.appWindow = {
        label: overrides.label ?? LABEL,
        close: vi.fn(async () => {}),
        setTitle: vi.fn(async () => {}),
        isVisible: vi.fn(async () => state.visible),
        onFocusChanged: vi.fn(async (handler) => {
            h.focusHandler = handler;
            return () => {};
        }),
    };
    h.win = {
        getComputedStyle: () => ({ getPropertyValue: (name) => state.tokens[name] ?? '' }),
        MutationObserver: window.MutationObserver,
        addEventListener: (...args) => window.addEventListener(...args),
    };
    h.deps = {
        invoke: h.invoke,
        listen: h.listen,
        appWindow: h.appWindow,
        t: fakeT,
        win: h.win,
        initI18n: vi.fn(async () => h.order.push('initI18n')),
        applyStaticTranslations: vi.fn(() => h.order.push('applyStaticTranslations')),
        applyTheme: vi.fn(async () => h.order.push('applyTheme')),
        initThemeListener: vi.fn(),
        getConfig: vi.fn(async () => structuredClone(state.config)),
        onConfigChange: vi.fn((handler) => {
            h.configHandlers.push(handler);
            return () => {};
        }),
        createSandbox: vi.fn((spec, container) => {
            h.order.push('createSandbox');
            const sb = new FakeSandbox(spec, container, h);
            h.sandboxes.push(sb);
            return sb;
        }),
        installDataEvents: vi.fn((opts) => {
            h.dataEvents = opts;
            return () => {};
        }),
        policy: {
            normalizeViews: vi.fn(fakeNormalizeViews),
            sanitizeViewTitle: vi.fn(fakeSanitize),
            isSafeTokenValue: vi.fn((_name, value) => !/url\(|[;{}]/i.test(value)),
        },
        hostFeatures: ['views', 'storage.change_events'],
        userActivation: () => state.activation,
        hasFocus: () => state.focused,
        ...(overrides.deps || {}),
    };
    h.page = createViewPage(h.deps);
    Object.defineProperty(h, 'sandbox', { get: () => h.sandboxes.at(-1) });
    h.configChanged = async () => {
        for (const handler of h.configHandlers) await handler();
        await settle();
    };
    return h;
}

/** Let promise chains and zero-delay timers run. */
async function settle() {
    for (let i = 0; i < 10; i++) await vi.advanceTimersByTimeAsync(0);
}

const text = (sel) => document.querySelector(sel)?.textContent;
const visible = (sel) => !document.querySelector(sel).hidden;
const stateActions = () =>
    [...document.querySelectorAll('.kv-state-actions button')].map((b) =>
        b.getAttribute('data-kv-action')
    );

beforeEach(() => {
    vi.useFakeTimers();
    mountViewHtml();
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    vi.spyOn(console, 'error').mockImplementation(() => {});
});

afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
    document.body.innerHTML = '';
});

describe('boot (§8.5)', () => {
    it('runs the steps in order and mounts with the boot record', async () => {
        const h = harness();
        await h.page.start();

        const at = (step) => h.order.indexOf(step);
        expect(at('applyTheme')).toBeLessThan(at('initI18n'));
        expect(at('initI18n')).toBeLessThan(at('applyStaticTranslations'));
        // Subscribed before the first boot, so no reopen is lost.
        expect(at('listen:ext_view_reopen')).toBeLessThan(at('ext_view_boot'));
        expect(at('listen:ext_view_visibility')).toBeLessThan(at('ext_view_boot'));
        expect(at('applyStaticTranslations')).toBeLessThan(at('ext_view_boot'));
        expect(at('ext_view_boot')).toBeLessThan(at('list_extensions'));
        expect(at('list_extensions')).toBeLessThan(at('read_extension_locale'));
        expect(at('read_extension_locale')).toBeLessThan(at('read_extension_file'));
        expect(at('read_extension_file')).toBeLessThan(at('createSandbox'));
        expect(at('createSandbox')).toBeLessThan(at('sandbox.start'));
        expect(at('sandbox.start')).toBeLessThan(at('rpc:viewMount'));

        expect(h.invoke).toHaveBeenCalledWith('read_extension_file', {
            extensionId: 'mindmap',
            kind: 'extension',
            filePath: 'explorer.js',
        });
        const [mount] = h.sandbox.rpcs('viewMount');
        expect(mount.params).toEqual({
            params: { focus: 'o' },
            source: 'launcher',
            reloaded: false,
        });
        expect(mount.opts).toEqual({ timeoutMs: MOUNT_TIMEOUT_MS });
        expect(MOUNT_TIMEOUT_MS).toBe(10_000);

        expect(visible('.kv-loading')).toBe(false);
        expect(visible('.kv-state')).toBe(false);
        expect(document.activeElement).toBe(h.sandbox.iframe);
        expect(h.deps.installDataEvents).toHaveBeenCalledTimes(1);
        expect(h.deps.onConfigChange).toHaveBeenCalledTimes(1);
        expect(h.appWindow.onFocusChanged).toHaveBeenCalledTimes(1);

        // The watchdog starts with the first ping, untimed.
        const [ping] = h.sandbox.rpcs('viewPing');
        expect(ping.opts).toEqual({ timeoutMs: 0 });
    });

    it('builds the view sandbox from the manifest, the grant and the config', async () => {
        const h = harness();
        await h.page.start();
        const [[spec, container]] = h.deps.createSandbox.mock.calls;
        expect(container).toBe(document.querySelector('.kv-frame'));
        expect(spec).toMatchObject({
            extensionId: 'mindmap',
            surface: 'view',
            viewId: 'explorer',
            capabilities: ['storage', 'views'],
            sources: {
                view: 'export default class Explorer {}',
                sourcePaths: { view: './explorer.js' },
            },
            viewCss: ['.ex { color: red }'],
            i18nLanguage: 'en',
            i18nRtl: false,
            iframeTitle: 'ext_view.iframe_label {"view":"Explorer","extension":"Mind Map"}',
        });
        expect(spec.config).toEqual({ scrollZooms: false, enabled: true });
        expect(spec.hostFeatures).toEqual(
            hostFeaturesFor(['views', 'storage.change_events'], ['storage', 'views'])
        );
        expect(spec.theme).toEqual({
            scheme: 'dark',
            fontSize: 14,
            reducedMotion: false,
            tokens: { 'kage-bg': '#1e1e1e', 'kage-text': '#e2e8f0' },
        });
        expect(h.sandbox.iframe.getAttribute('title')).toBe(spec.iframeTitle);
        // Hooks are on the instance before start().
        expect(typeof h.sandbox.onViewRequest).toBe('function');
        expect(typeof h.sandbox.onNavigated).toBe('function');
        expect(h.deps.policy.normalizeViews).toHaveBeenCalledWith(
            MANIFEST.contributes.views,
            'Mind Map',
            '🧠',
            'mindmap'
        );
    });

    it('shows the identity and sets the OS title (§18.2)', async () => {
        const h = harness();
        await h.page.start();
        expect(text('.kv-bar-icon')).toBe('🗺️');
        expect(text('.kv-bar-ext')).toBe('Mind Map');
        expect(text('.kv-bar-view')).toBe('Explorer');
        const title = 'ext_view.window_title {"view":"Explorer","extension":"Mind Map"}';
        expect(document.title).toBe(title);
        expect(h.appWindow.setTitle).toHaveBeenLastCalledWith(title);
    });

    it('resolves a __MSG_ title and name from the extension catalog', async () => {
        const manifest = structuredClone(MANIFEST);
        manifest.name = '__MSG_manifest.name__';
        manifest.contributes.views[0].title = '__MSG_view.explorer.title__';
        const h = harness({
            manifest,
            locale: {
                en: {
                    'manifest.name': { message: 'Carte mentale' },
                    'view.explorer.title': { message: '  Explorateur\u202E  ' },
                },
            },
        });
        await h.page.start();
        expect(text('.kv-bar-ext')).toBe('Carte mentale');
        expect(text('.kv-bar-view')).toBe('Explorateur');
    });

    it('caps a resolved title at 60 characters', async () => {
        const manifest = structuredClone(MANIFEST);
        manifest.contributes.views[0].title = '__MSG_t__';
        const h = harness({ manifest, locale: { en: { t: { message: 'x'.repeat(90) } } } });
        await h.page.start();
        expect(text('.kv-bar-view')).toBe('x'.repeat(60));
    });

    it('a label that is not a view label never boots', async () => {
        const h = harness({ label: 'settings' });
        await h.page.start();
        expect(h.invoke).not.toHaveBeenCalledWith('ext_view_boot');
        expect(visible('.kv-state')).toBe(true);
        expect(stateActions()).toEqual(['close']);
    });

    it('a boot record for another window is refused', async () => {
        const h = harness();
        h.state.boot.extension_id = 'other';
        await h.page.start();
        expect(h.deps.createSandbox).not.toHaveBeenCalled();
        expect(text('.kv-state-text')).toMatch(/^ext_view\.state\.load_failed/);
    });
});

describe('boot errors (§16.1, §18.3)', () => {
    const appError = (kind, key, message) => ({ kind, key, message });
    const cases = [
        ['errors.ext_view.not_granted', 'not_permitted', ['approve', 'settings']],
        ['errors.ext_view.disabled', 'not_permitted', ['settings']],
        ['errors.ext_view.unknown_view', 'invalid_input', ['close']],
        ['errors.ext_view.unknown_extension', 'invalid_input', ['close']],
        ['errors.ext_view.not_a_view_window', 'not_permitted', ['close']],
        ['errors.window.create_failed', 'internal', ['reload', 'settings']],
    ];
    for (const [key, kind, actions] of cases) {
        it(`${key} → its panel`, async () => {
            const h = harness({ bootError: appError(kind, key, `localised ${key}`) });
            await h.page.start();
            expect(h.deps.createSandbox).not.toHaveBeenCalled();
            expect(visible('.kv-state')).toBe(true);
            expect(visible('.kv-loading')).toBe(false);
            expect(stateActions()).toEqual(actions);
            const message = text('.kv-state-text');
            if (actions.includes('reload') || key.endsWith('not_a_view_window')) {
                expect(message).toMatch(/^ext_view\.state\.load_failed/);
            } else {
                expect(message).toBe(`localised ${key}`);
            }
            // The log gets kind and key, in English.
            expect(console.warn).toHaveBeenCalledWith(expect.stringContaining(key));
        });
    }

    it('the bar names the extension even when boot fails', async () => {
        const h = harness({
            bootError: appError('not_permitted', 'errors.ext_view.disabled', 'off'),
        });
        await h.page.start();
        expect(text('.kv-bar-ext')).toBe('mindmap');
        expect(document.title).toBe(
            'ext_view.window_title {"view":"explorer","extension":"mindmap"}'
        );
    });

    it('Approve opens the Store, where the card offers it', async () => {
        const h = harness({
            bootError: appError('not_permitted', 'errors.ext_view.not_granted', 'needs approval'),
        });
        await h.page.start();
        document.querySelector('.kv-state [data-kv-action="approve"]').click();
        await settle();
        expect(h.invoke).toHaveBeenCalledWith('open_store_window', { tab: 'extensions' });
    });
});

describe('panels the page decides itself', () => {
    it('no such extension', async () => {
        const h = harness({ manifest: null });
        await h.page.start();
        expect(text('.kv-state-text')).toBe('errors.ext_view.unknown_extension {"id":"mindmap"}');
        expect(stateActions()).toEqual(['close']);
    });

    it('turned off', async () => {
        const h = harness();
        h.state.config.extension_states.mindmap = false;
        await h.page.start();
        expect(text('.kv-state-text')).toBe('errors.ext_view.disabled {"extension":"Mind Map"}');
        expect(stateActions()).toEqual(['settings']);
    });

    it('not approved: no views in the grant', async () => {
        const h = harness();
        h.state.config.extension_grants.mindmap.granted = ['storage'];
        await h.page.start();
        expect(text('.kv-state-text')).toBe('errors.ext_view.not_granted {"extension":"Mind Map"}');
        expect(stateActions()).toEqual(['approve', 'settings']);
    });

    it('the view is no longer declared', async () => {
        const h = harness();
        h.state.manifest.contributes.views = [{ id: 'board', module: './board.js' }];
        await h.page.start();
        expect(text('.kv-state-text')).toBe('ext_view.state.removed {"extension":"Mind Map"}');
        expect(stateActions()).toEqual(['close']);
        document.querySelector('.kv-state [data-kv-action="close"]').click();
        expect(h.appWindow.close).toHaveBeenCalled();
    });

    it("a module that can't be read", async () => {
        const h = harness();
        delete h.state.files['explorer.js'];
        await h.page.start();
        expect(h.deps.createSandbox).not.toHaveBeenCalled();
        expect(text('.kv-state-text')).toBe('ext_view.state.load_failed {"view":"Explorer"}');
        expect(stateActions()).toEqual(['reload', 'settings']);
    });

    it("a sandbox that doesn't start", async () => {
        const h = harness({ failStart: true });
        await h.page.start();
        expect(h.sandbox.destroyed).toBe(true);
        expect(text('.kv-state-text')).toBe('ext_view.state.load_failed {"view":"Explorer"}');
    });

    it('a sandbox that throws while being built', async () => {
        const h = harness({
            deps: {
                createSandbox: () => {
                    throw new Error('bad spec');
                },
            },
        });
        await h.page.start();
        expect(text('.kv-state-text')).toBe('ext_view.state.load_failed {"view":"Explorer"}');
    });

    it('mount rejects or times out (10 s)', async () => {
        const h = harness({
            rpc: {
                viewMount: () => {
                    throw new Error("RPC 'viewMount' timed out after 10000ms");
                },
            },
        });
        await h.page.start();
        expect(h.sandbox.destroyed).toBe(true);
        expect(visible('.kv-state')).toBe(true);
        expect(stateActions()).toEqual(['reload', 'settings']);
        expect(h.sandbox.rpcs('viewPing')).toHaveLength(0);
    });

    it('Reload from a panel boots again', async () => {
        const h = harness({ failStart: true });
        await h.page.start();
        h.state.failStart = false;
        document.querySelector('.kv-state [data-kv-action="reload"]').click();
        await settle();
        expect(h.sandboxes).toHaveLength(2);
        expect(h.sandbox.rpcs('viewMount')).toHaveLength(1);
        expect(visible('.kv-state')).toBe(false);
    });

    it('CSS over 256 KiB, or over 1 MiB in total, is left out', async () => {
        const manifest = structuredClone(MANIFEST);
        manifest.contributes.views[0].css = Array.from({ length: 6 }, (_, i) => `./c${i}.css`);
        const big = 'a'.repeat(CSS_FILE_MAX_BYTES + 1);
        const fit = 'b'.repeat(250 * 1024);
        const files = {
            'explorer.js': 'export default class Explorer {}',
            'c0.css': big,
            'c1.css': fit,
            'c2.css': fit,
            'c3.css': fit,
            'c4.css': fit,
            'c5.css': 'small',
        };
        const h = harness({ manifest, files });
        await h.page.start();
        const css = h.sandbox.spec.viewCss;
        // c0 too big; c1–c4 make 1000 KiB; c5 still fits; nothing over 1 MiB.
        expect(css).toEqual([fit, fit, fit, fit, 'small']);
    });
});

describe('reopen and visibility (§6.3, §7.3)', () => {
    it('delivers a newer reopen and ignores stale ones', async () => {
        const h = harness();
        await h.page.start();
        h.emit('ext_view_reopen', { params: { focus: 'e1' }, open_seq: 2, source: 'chat' });
        h.emit('ext_view_reopen', { params: { focus: 'old' }, open_seq: 2, source: 'chat' });
        h.emit('ext_view_reopen', { params: { focus: 'older' }, open_seq: 1, source: 'chat' });
        await settle();
        const reopens = h.sandbox.rpcs('viewReopen');
        expect(reopens.map((c) => c.params)).toEqual([{ params: { focus: 'e1' }, source: 'chat' }]);
        // Already visible: no visibility RPC.
        expect(h.sandbox.rpcs('viewVisibility')).toHaveLength(0);
    });

    it('reads an unknown source as launcher', async () => {
        expect(normalizeViewSource('deep-link')).toBe('launcher');
        expect(normalizeViewSource('widget')).toBe('widget');
        const h = harness();
        await h.page.start();
        h.emit('ext_view_reopen', { params: null, open_seq: 2, source: 'evil' });
        await settle();
        expect(h.sandbox.rpcs('viewReopen')[0].params).toEqual({
            params: null,
            source: 'launcher',
        });
    });

    it('queues a reopen that arrives while loading, delivered after mount', async () => {
        const h = harness();
        let releaseMount;
        h.state.rpc.viewMount = () =>
            new Promise((resolve) => {
                releaseMount = resolve;
            });
        const started = h.page.start();
        await settle();
        expect(releaseMount).toBeTypeOf('function');
        h.emit('ext_view_reopen', { params: { focus: 'early' }, open_seq: 1, source: 'chat' });
        h.emit('ext_view_reopen', { params: { focus: 'new' }, open_seq: 3, source: 'chat' });
        h.emit('ext_view_reopen', { params: { focus: 'mid' }, open_seq: 2, source: 'chat' });
        expect(h.sandbox.rpcs('viewReopen')).toHaveLength(0);
        releaseMount();
        await started;
        await settle();
        const reopens = h.sandbox.rpcs('viewReopen');
        expect(reopens.map((c) => c.params.params)).toEqual([{ focus: 'new' }]);
    });

    it('hide and show reach the view and pause the watchdog', async () => {
        const h = harness();
        await h.page.start();
        h.emit('ext_view_visibility', { visible: false });
        h.emit('ext_view_visibility', { visible: false });
        await settle();
        expect(h.sandbox.rpcs('viewVisibility').map((c) => c.params)).toEqual([{ visible: false }]);
        const pings = h.sandbox.rpcs('viewPing').length;
        await vi.advanceTimersByTimeAsync(30_000);
        expect(h.sandbox.rpcs('viewPing')).toHaveLength(pings);

        h.emit('ext_view_visibility', { visible: true });
        await settle();
        expect(h.sandbox.rpcs('viewVisibility').at(-1).params).toEqual({ visible: true });
        h.emit('ext_view_visibility', { visible: 'yes' });
        await settle();
        expect(h.sandbox.rpcs('viewVisibility')).toHaveLength(2);
    });

    it('a reopen after a hide shows the view, then delivers the params', async () => {
        const h = harness();
        await h.page.start();
        h.emit('ext_view_visibility', { visible: false });
        h.emit('ext_view_reopen', { params: { focus: 'x' }, open_seq: 2, source: 'launcher' });
        await settle();
        const names = h.sandbox.calls
            .map((c) => c.method)
            .filter((m) => m === 'viewVisibility' || m === 'viewReopen');
        expect(names).toEqual(['viewVisibility', 'viewVisibility', 'viewReopen']);
        expect(h.sandbox.rpcs('viewVisibility').at(-1).params).toEqual({ visible: true });
    });

    it('mounted while hidden: tells the view at once', async () => {
        const h = harness({ visible: false });
        await h.page.start();
        await settle();
        expect(h.sandbox.rpcs('viewVisibility').map((c) => c.params)).toEqual([{ visible: false }]);
        expect(h.sandbox.rpcs('viewPing')).toHaveLength(0);
    });

    it('reopening a window whose last ping failed reloads the sandbox', async () => {
        const h = harness({ rpc: { viewPing: () => new Promise(() => {}) } });
        await h.page.start();
        await vi.advanceTimersByTimeAsync(5_000); // one miss
        const first = h.sandbox;
        h.emit('ext_view_visibility', { visible: false });
        h.emit('ext_view_reopen', { params: { focus: 'y' }, open_seq: 2, source: 'launcher' });
        await settle();
        expect(first.destroyed).toBe(true);
        expect(h.sandboxes).toHaveLength(2);
        expect(h.invoke.mock.calls.filter(([c]) => c === 'ext_view_boot')).toHaveLength(2);
        expect(first.rpcs('viewReopen')).toHaveLength(0);
    });

    it('a reopen while a panel shows tries again', async () => {
        const h = harness({ failStart: true });
        await h.page.start();
        h.state.failStart = false;
        h.state.boot.open_seq = 2;
        h.emit('ext_view_reopen', { params: { focus: 'z' }, open_seq: 2, source: 'launcher' });
        await settle();
        expect(h.sandboxes).toHaveLength(2);
        expect(h.sandbox.rpcs('viewMount')).toHaveLength(1);
    });
});

describe('watchdog banner (§8.7)', () => {
    it('3 missed pings show "isn\'t responding"; an answer clears it', async () => {
        let answer;
        const h = harness({
            rpc: {
                viewPing: () =>
                    new Promise((resolve) => {
                        answer = resolve;
                    }),
            },
        });
        await h.page.start();
        await vi.advanceTimersByTimeAsync(10_000);
        expect(visible('.kv-banner')).toBe(false);
        await vi.advanceTimersByTimeAsync(5_000);
        expect(visible('.kv-banner')).toBe(true);
        expect(text('.kv-banner-text')).toBe('ext_view.state.not_responding {"view":"Explorer"}');
        expect(
            [...document.querySelectorAll('.kv-banner button')].map((b) =>
                b.getAttribute('data-kv-action')
            )
        ).toEqual(['reload', 'close']);
        // The watchdog never kills the view.
        expect(h.sandbox.destroyed).toBe(false);
        answer('pong');
        await settle();
        expect(visible('.kv-banner')).toBe(false);
    });
});

describe('reload in place (§7.3)', () => {
    it('an extension update reloads and says "Updated to"', async () => {
        const h = harness();
        await h.page.start();
        const first = h.sandbox;
        h.state.manifest.version = '1.2.0';
        h.state.boot.reloaded = true;
        h.emit('extensions_changed', null);
        await settle();
        expect(first.destroyed).toBe(true);
        expect(h.sandboxes).toHaveLength(2);
        expect(h.sandbox.rpcs('viewMount')[0].params.reloaded).toBe(true);
        expect(visible('.kv-bar-notice')).toBe(true);
        expect(text('.kv-bar-notice')).toBe('ext_view.state.updated {"version":"1.2.0"}');
        await vi.advanceTimersByTimeAsync(3_000);
        expect(visible('.kv-bar-notice')).toBe(false);
    });

    it("another extension's change leaves the view alone", async () => {
        const h = harness();
        await h.page.start();
        h.emit('extensions_changed', null);
        await settle();
        expect(h.sandboxes).toHaveLength(1);
    });

    it('a changed declaration at the same version reloads without the note', async () => {
        const h = harness();
        await h.page.start();
        h.state.manifest.contributes.views[0].css = [];
        h.emit('extensions_changed', null);
        await settle();
        expect(h.sandboxes).toHaveLength(2);
        expect(visible('.kv-bar-notice')).toBe(false);
    });

    it('an update that drops the view shows "no longer has this window" with Close', async () => {
        const h = harness();
        await h.page.start();
        h.state.manifest.version = '2.0.0';
        h.state.bootError = {
            kind: 'invalid_input',
            key: 'errors.ext_view.unknown_view',
            message: 'Mind Map has no window called explorer.',
        };
        h.emit('extensions_changed', null);
        await settle();
        expect(text('.kv-state-text')).toBe('ext_view.state.removed {"extension":"Mind Map"}');
        expect(stateActions()).toEqual(['close']);
    });

    it('uninstalled while open: says so', async () => {
        const h = harness();
        await h.page.start();
        h.state.manifest = null;
        h.emit('extensions_changed', null);
        await settle();
        expect(h.sandbox.destroyed).toBe(true);
        expect(text('.kv-state-text')).toBe('errors.ext_view.unknown_extension {"id":"mindmap"}');
    });

    it('a grant change that keeps views reloads; one that drops it shows the approval panel', async () => {
        const h = harness();
        h.state.manifest.permissions = ['storage', 'views', 'urls'];
        await h.page.start();
        h.state.config.extension_grants.mindmap.granted = ['storage', 'views', 'urls'];
        await h.configChanged();
        expect(h.sandboxes).toHaveLength(2);
        expect(h.sandbox.spec.capabilities).toEqual(['storage', 'views', 'urls']);

        h.state.config.extension_grants.mindmap.granted = ['storage'];
        await h.configChanged();
        expect(h.sandbox.destroyed).toBe(true);
        expect(stateActions()).toEqual(['approve', 'settings']);

        // Approved again: the page loads by itself.
        h.state.config.extension_grants.mindmap.granted = ['storage', 'views'];
        await h.configChanged();
        expect(h.sandboxes).toHaveLength(3);
        expect(visible('.kv-state')).toBe(false);
    });

    it('turned off shows the panel; turned on again loads', async () => {
        const h = harness();
        await h.page.start();
        h.state.config.extension_states.mindmap = false;
        await h.configChanged();
        expect(h.sandbox.destroyed).toBe(true);
        expect(stateActions()).toEqual(['settings']);
        h.state.config.extension_states.mindmap = true;
        await h.configChanged();
        expect(h.sandboxes).toHaveLength(2);
    });

    it("the extension's settings reach onConfigUpdate without a reload", async () => {
        const h = harness();
        await h.page.start();
        await h.configChanged(); // nothing changed
        expect(h.sandbox.configs).toHaveLength(0);
        h.state.config.extensions.mindmap = { scrollZooms: true };
        await h.configChanged();
        expect(h.sandboxes).toHaveLength(1);
        expect(h.sandbox.configs).toEqual([{ scrollZooms: true, enabled: true }]);
    });

    it('a language change reloads so every string is new (§12)', async () => {
        const h = harness();
        await h.page.start();
        document.dispatchEvent(new CustomEvent('kage:i18n-changed'));
        await settle();
        expect(h.sandboxes).toHaveLength(2);
        expect(h.sandboxes[0].destroyed).toBe(true);
    });

    it('the bar Reload button reloads', async () => {
        const h = harness();
        await h.page.start();
        document.querySelector('.kv-bar [data-kv-action="reload"]').click();
        await settle();
        expect(h.sandboxes).toHaveLength(2);
    });

    it('a second reload supersedes one still loading', async () => {
        const h = harness();
        await h.page.start();
        let release;
        h.state.rpc.viewMount = () =>
            new Promise((resolve) => {
                release = resolve;
            });
        document.querySelector('.kv-bar [data-kv-action="reload"]').click();
        await settle();
        const slow = h.sandbox;
        h.state.rpc.viewMount = null;
        document.querySelector('.kv-bar [data-kv-action="reload"]').click();
        await settle();
        expect(slow.destroyed).toBe(true);
        release();
        await settle();
        expect(h.sandbox).not.toBe(slow);
        expect(visible('.kv-loading')).toBe(false);
        expect(h.sandbox.destroyed).toBe(false);
    });
});

describe('requests from the view (§8.6, §13.4)', () => {
    async function mounted(overrides) {
        const h = harness(overrides);
        await h.page.start();
        return h;
    }

    it('set-title: cleaned subtitle in the bar and the OS title; null clears it', async () => {
        const h = await mounted();
        await h.sandbox.request({ kind: 'set-title', text: ' Bingo\u202E\u0007 ' });
        expect(text('.kv-bar-sub')).toBe('Bingo');
        const title =
            'ext_view.window_title_sub {"subtitle":"Bingo","view":"Explorer","extension":"Mind Map"}';
        expect(document.title).toBe(title);
        expect(h.appWindow.setTitle).toHaveBeenLastCalledWith(title);

        await h.sandbox.request({ kind: 'set-title', text: 'y'.repeat(100) });
        expect(text('.kv-bar-sub')).toBe('y'.repeat(80));

        await h.sandbox.request({ kind: 'set-title', text: null });
        expect(visible('.kv-bar-sub')).toBe(false);
        expect(document.title).toBe(
            'ext_view.window_title {"view":"Explorer","extension":"Mind Map"}'
        );
    });

    it('set-title: at most 10 a second, strings only', async () => {
        expect(SET_TITLE_PER_SECOND).toBe(10);
        const h = await mounted();
        for (let i = 0; i < 10; i++) await h.sandbox.request({ kind: 'set-title', text: `t${i}` });
        await expect(
            h.sandbox.request({ kind: 'set-title', text: 'eleven' })
        ).rejects.toMatchObject({ kind: 'rate_limited', key: 'errors.ext_view.too_fast' });
        await vi.advanceTimersByTimeAsync(1_000);
        await h.sandbox.request({ kind: 'set-title', text: 'later' });
        expect(text('.kv-bar-sub')).toBe('later');
        await expect(h.sandbox.request({ kind: 'set-title', text: 42 })).rejects.toMatchObject({
            kind: 'invalid_input',
            key: 'errors.ext_view.text_invalid',
        });
    });

    it('close hides the window through CloseRequested', async () => {
        const h = await mounted();
        await expect(h.sandbox.request({ kind: 'close' })).resolves.toBeNull();
        expect(h.appWindow.close).toHaveBeenCalledTimes(1);
    });

    it('open-view: another view of the same extension, source view', async () => {
        const h = await mounted();
        const result = await h.sandbox.request({
            kind: 'open-view',
            view: 'board',
            params: { a: 1 },
        });
        expect(result).toEqual({ label: 'ext-view:mindmap:board', reused: false });
        expect(h.invoke).toHaveBeenCalledWith('ext_view_open', {
            extensionId: 'mindmap',
            viewId: 'board',
            params: { a: 1 },
            source: 'view',
        });
    });

    it('open-view: needs focus and a recent click or key press', async () => {
        const h = await mounted();
        h.state.activation = { isActive: false };
        await expect(h.sandbox.request({ kind: 'open-view', view: 'board' })).rejects.toMatchObject(
            { kind: 'not_permitted', key: 'errors.ext_view.needs_user_action' }
        );
        h.state.activation = { isActive: true };
        h.state.focused = false;
        await expect(h.sandbox.request({ kind: 'open-view', view: 'board' })).rejects.toMatchObject(
            { key: 'errors.ext_view.needs_user_action' }
        );
        expect(h.invoke).not.toHaveBeenCalledWith('ext_view_open', expect.anything());
        expect(console.warn).toHaveBeenCalledWith(
            expect.stringContaining('[ext mindmap] open_view refused:')
        );
    });

    it('open-view: without navigator.userActivation, focus is the rule', async () => {
        const h = await mounted({ activation: undefined });
        await h.sandbox.request({ kind: 'open-view', view: 'board' });
        expect(h.invoke).toHaveBeenCalledWith('ext_view_open', expect.anything());
    });

    it('open-view: at most one a second', async () => {
        expect(OPEN_VIEW_PER_SECOND).toBe(1);
        const h = await mounted();
        await h.sandbox.request({ kind: 'open-view', view: 'board' });
        await expect(h.sandbox.request({ kind: 'open-view', view: 'board' })).rejects.toMatchObject(
            { kind: 'rate_limited', key: 'errors.ext_view.too_fast' }
        );
        await vi.advanceTimersByTimeAsync(1_000);
        await h.sandbox.request({ kind: 'open-view', view: 'board' });
        expect(h.invoke.mock.calls.filter(([c]) => c === 'ext_view_open')).toHaveLength(2);
    });

    it('open-view: an undeclared view or non-object params are refused here', async () => {
        const h = await mounted();
        await expect(
            h.sandbox.request({ kind: 'open-view', view: 'settings' })
        ).rejects.toMatchObject({ kind: 'invalid_input', key: 'errors.ext_view.unknown_view' });
        await vi.advanceTimersByTimeAsync(1_000);
        await expect(
            h.sandbox.request({ kind: 'open-view', view: 'board', params: [1, 2] })
        ).rejects.toMatchObject({ kind: 'invalid_input', key: 'errors.ext_view.params_invalid' });
        expect(h.invoke).not.toHaveBeenCalledWith('ext_view_open', expect.anything());
    });

    it('compose and copy-text go to Rust with the text; activation first', async () => {
        const h = await mounted();
        await h.sandbox.request({ kind: 'compose', text: 'mm? about Bingo' });
        await h.sandbox.request({ kind: 'copy-text', text: '/tmp/notes.md' });
        expect(h.invoke).toHaveBeenCalledWith('ext_view_compose', { text: 'mm? about Bingo' });
        expect(h.invoke).toHaveBeenCalledWith('ext_view_copy_text', { text: '/tmp/notes.md' });

        h.state.activation = { isActive: false };
        await expect(h.sandbox.request({ kind: 'copy-text', text: 'x' })).rejects.toMatchObject({
            key: 'errors.ext_view.needs_user_action',
        });
        h.state.activation = { isActive: true };
        await expect(h.sandbox.request({ kind: 'compose', text: 5 })).rejects.toMatchObject({
            key: 'errors.ext_view.text_invalid',
        });
    });

    it("a Rust refusal reaches the view as Rust's AppError", async () => {
        const h = await mounted();
        const refusal = { kind: 'not_permitted', key: 'errors.ext_view.not_focused', message: 'x' };
        h.invoke.mockImplementationOnce(async () => {
            throw refusal;
        });
        await expect(h.sandbox.request({ kind: 'copy-text', text: 'x' })).rejects.toBe(refusal);
    });

    it("open-settings opens this extension's page, whatever the view says", async () => {
        const h = await mounted();
        await h.sandbox.request({ kind: 'open-settings', section: 'ext-other' });
        expect(h.invoke).toHaveBeenCalledWith('open_settings_window', { section: 'ext-mindmap' });
    });

    it('unknown kinds are ignored with a warning', async () => {
        const h = await mounted();
        await expect(h.sandbox.request({ kind: 'resize', width: 10 })).resolves.toBeNull();
        expect(console.warn).toHaveBeenCalledWith(expect.stringContaining("'resize'"));
    });

    it('a request from a replaced sandbox is refused', async () => {
        const h = await mounted();
        const old = h.sandbox;
        document.querySelector('.kv-bar [data-kv-action="reload"]').click();
        await settle();
        await expect(old.request({ kind: 'close' })).rejects.toMatchObject({
            key: 'errors.ext_view.not_a_view_window',
        });
        expect(h.appWindow.close).not.toHaveBeenCalled();
    });

    it('the bar settings button opens the same page', async () => {
        const h = await mounted();
        document.querySelector('.kv-bar [data-kv-action="settings"]').click();
        await settle();
        expect(h.invoke).toHaveBeenCalledWith('open_settings_window', { section: 'ext-mindmap' });
    });
});

describe('navigation (§8.8)', () => {
    it('a frame that navigated is stopped and the page says so', async () => {
        const h = harness();
        await h.page.start();
        h.sandbox.onNavigated();
        expect(h.sandbox.destroyed).toBe(true);
        expect(text('.kv-state-text')).toBe('ext_view.state.navigated {"view":"Explorer"}');
        expect(stateActions()).toEqual(['reload']);
        // It waits for the user: an extension update doesn't restart it.
        h.state.manifest.version = '1.0.1';
        h.emit('extensions_changed', null);
        await settle();
        expect(h.sandboxes).toHaveLength(1);
    });

    it("a replaced sandbox's report is ignored", async () => {
        const h = harness();
        await h.page.start();
        const old = h.sandbox;
        document.querySelector('.kv-bar [data-kv-action="reload"]').click();
        await settle();
        old.onNavigated();
        expect(visible('.kv-state')).toBe(false);
        expect(h.sandbox.destroyed).toBe(false);
    });
});

describe('storage change events (§10.1)', () => {
    it('installs the shared listener with this window label', async () => {
        const h = harness();
        await h.page.start();
        expect(h.dataEvents.listen).toBe(h.listen);
        expect(h.dataEvents.myLabel).toBe(LABEL);
    });

    it("forwards this extension's batches, only with storage", async () => {
        const h = harness();
        await h.page.start();
        const changes = [{ key: 'mm-meta', op: 'save', self: false, source: 'window' }];
        h.dataEvents.deliver('mindmap', changes);
        h.dataEvents.deliver('other', changes);
        h.dataEvents.deliver({ extension_id: 'mindmap', changes });
        h.dataEvents.deliver('mindmap', []);
        expect(h.sandbox.storage).toEqual([changes, changes]);
    });

    it('drops batches without the storage grant', async () => {
        const h = harness();
        h.state.manifest.permissions = ['views'];
        h.state.config.extension_grants.mindmap.granted = ['views'];
        await h.page.start();
        h.dataEvents.deliver('mindmap', [{ key: 'k', op: 'save', self: false, source: 'window' }]);
        expect(h.sandbox.storage).toEqual([]);
    });

    it('drops batches while no sandbox runs', async () => {
        const h = harness({ failStart: true });
        await h.page.start();
        h.dataEvents.deliver('mindmap', [{ key: 'k', op: 'save', self: false, source: 'window' }]);
        expect(h.sandbox.storage).toEqual([]);
    });
});

describe('theme (§11)', () => {
    it('sends viewTheme only when the snapshot changed', async () => {
        const h = harness();
        await h.page.start();
        await settle();
        await h.configChanged();
        expect(h.sandbox.rpcs('viewTheme')).toHaveLength(0);

        h.state.tokens['--kage-bg'] = '#fafafa';
        document.body.classList.add('light-theme');
        await h.configChanged();
        const sent = h.sandbox.rpcs('viewTheme');
        expect(sent).toHaveLength(1);
        expect(sent[0].params.theme).toMatchObject({
            scheme: 'light',
            tokens: { 'kage-bg': '#fafafa' },
        });
        document.body.classList.remove('light-theme');
    });

    it('notices a theme applied outside config_updated (the OS listener)', async () => {
        const h = harness();
        await h.page.start();
        await settle();
        h.state.tokens['--kage-accent'] = '#ff0000';
        document.body.classList.add('light-theme');
        await settle();
        expect(h.sandbox.rpcs('viewTheme')).toHaveLength(1);
        document.body.classList.remove('light-theme');
    });

    it('a theme change while the sandbox starts still reaches it', async () => {
        let open;
        const h = harness({
            startGate: new Promise((resolve) => {
                open = resolve;
            }),
        });
        const started = h.page.start();
        await settle();
        h.state.tokens['--kage-bg'] = '#123456';
        document.body.classList.add('light-theme');
        await settle();
        expect(h.sandbox.rpcs('viewTheme')).toHaveLength(0);
        open();
        await started;
        await settle();
        const sent = h.sandbox.rpcs('viewTheme');
        expect(sent).toHaveLength(1);
        expect(sent[0].params.theme.tokens['kage-bg']).toBe('#123456');
        document.body.classList.remove('light-theme');
    });

    it('a font size change counts', async () => {
        const h = harness();
        await h.page.start();
        h.state.config.ui.font_size = 18;
        await h.configChanged();
        expect(h.sandbox.rpcs('viewTheme').at(-1).params.theme.fontSize).toBe(18);
    });

    it('an unsafe token value never reaches the view', async () => {
        const h = harness();
        h.state.tokens['--kage-link'] = 'url(https://example.com/beacon)';
        await h.page.start();
        expect(h.sandbox.spec.theme.tokens).not.toHaveProperty('kage-link');
    });
});

describe('input on the host page (§13)', () => {
    it('ctrl + wheel never zooms the page; a plain wheel is left alone', async () => {
        const h = harness();
        await h.page.start();
        const zoom = new WheelEvent('wheel', { ctrlKey: true, cancelable: true });
        window.dispatchEvent(zoom);
        expect(zoom.defaultPrevented).toBe(true);
        const scroll = new WheelEvent('wheel', { cancelable: true });
        window.dispatchEvent(scroll);
        expect(scroll.defaultPrevented).toBe(false);
    });

    it('a drop never navigates the page and nothing reaches the view', async () => {
        const h = harness();
        await h.page.start();
        for (const type of ['dragover', 'drop']) {
            const e = new Event(type, { cancelable: true, bubbles: true });
            document.querySelector('.kv-stage').dispatchEvent(e);
            expect(e.defaultPrevented).toBe(true);
        }
        // The page never subscribes to Tauri's drag-drop events (§13.5).
        const events = h.listen.mock.calls.map(([event]) => event);
        expect(events.some((e) => /drag|drop/.test(e))).toBe(false);
        expect(h.sandbox.calls.map((c) => c.method)).not.toContain(expect.stringMatching(/drop/));
        expect(h.sandbox.storage).toEqual([]);
    });

    it('Ctrl+W on the host page closes the window', async () => {
        const h = harness();
        await h.page.start();
        const e = new KeyboardEvent('keydown', { key: 'w', ctrlKey: true, cancelable: true });
        document.dispatchEvent(e);
        expect(e.defaultPrevented).toBe(true);
        expect(h.appWindow.close).toHaveBeenCalledTimes(1);
        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'w' }));
        document.dispatchEvent(
            new KeyboardEvent('keydown', { key: 'w', ctrlKey: true, shiftKey: true })
        );
        expect(h.appWindow.close).toHaveBeenCalledTimes(1);
    });

    it('window focus goes to the view, but not away from a bar button', async () => {
        const h = harness();
        await h.page.start();
        document.body.focus();
        h.sandbox.iframe.blur();
        h.focusHandler({ payload: true });
        expect(document.activeElement).toBe(h.sandbox.iframe);

        const reload = document.querySelector('.kv-bar [data-kv-action="reload"]');
        reload.focus();
        h.focusHandler({ payload: true });
        expect(document.activeElement).toBe(reload);
        h.focusHandler({ payload: false });
        expect(document.activeElement).toBe(reload);
    });
});

describe('recentUserActivation (§13.4)', () => {
    it("needs focus, then the engine's activation where it reports one", () => {
        expect(recentUserActivation(false, { isActive: true })).toBe(false);
        expect(recentUserActivation(true, { isActive: true })).toBe(true);
        expect(recentUserActivation(true, { isActive: false })).toBe(false);
        expect(recentUserActivation(true, undefined)).toBe(true);
        expect(recentUserActivation(true, {})).toBe(true);
        expect(recentUserActivation(false, undefined)).toBe(false);
    });
});

describe('grantedCapabilities', () => {
    it('is what the manifest asks for, cut down to the grant', () => {
        const manifest = { id: 'x', permissions: ['storage', 'views', 'urls'] };
        expect(
            grantedCapabilities(manifest, {
                extension_grants: { x: { granted: ['views', 'storage', 'clipboard'] } },
            })
        ).toEqual(['storage', 'views']);
        expect(grantedCapabilities(manifest, { extension_grants: {} })).toEqual([]);
        expect(grantedCapabilities(manifest, {})).toEqual([]);
    });
});
