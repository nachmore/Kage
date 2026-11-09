/**
 * The `open_view` host effect (docs/design/EXTENSION_VIEWS.md §6) and the
 * windows that open views with it: `runOpenViewEffect` itself, then the
 * extension manager's paths (a result-row or formatter-card button's
 * `onResultAction`, a widget's `onAction`, a toolbar `onClick`), the
 * settings `runAction` path, the launcher-focus helper and the storage
 * change events each window forwards to its sandboxes (§10.1).
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
    isOpenViewEffect,
    openViewDenial,
    runOpenViewEffect,
} from '../../ui/js/shared/extension-host-effects.js';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';
import { runSettingsHostSideEffect } from '../../ui/js/shared/settings-host-effects.js';
import { renderSchema } from '../../ui/js/shared/settings-renderer.js';
import { runToolbarHostEffect } from '../../ui/js/shared/toolbar-host-effects.js';

const VIEWS = [
    { id: 'explorer', title: 'Explorer', module: './explorer.js' },
    { id: 'board', title: 'Board', module: './board.js' },
];
const MANIFEST = {
    id: 'mindmap',
    name: 'Mind Map',
    permissions: ['storage', 'views'],
    contributes: { views: VIEWS },
};
const OPENED = { label: 'ext-view:mindmap:explorer', reused: false };
const EFFECT = { type: 'open_view', view: 'explorer', params: { focus: 'n1' } };

let warn;
beforeEach(() => {
    warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    document.body.innerHTML = '';
});
afterEach(() => {
    vi.restoreAllMocks();
    delete window.__TAURI__;
    document.body.innerHTML = '';
});

function fakeInvoke(impl) {
    return vi.fn(impl || (async (cmd) => (cmd === 'ext_view_open' ? OPENED : undefined)));
}

const viewCalls = (invoke) => invoke.mock.calls.filter(([cmd]) => cmd === 'ext_view_open');

const allowAll = (effect) =>
    openViewDenial(effect, {
        hostFeatures: ['views'],
        capabilities: ['views'],
        views: VIEWS,
        extensionId: 'mindmap',
    });

describe('runOpenViewEffect', () => {
    it('opens with the host id, the view, the params and a known source', async () => {
        const invoke = fakeInvoke();
        const out = await runOpenViewEffect(EFFECT, {
            extensionId: 'mindmap',
            source: 'widget',
            invoke,
            check: allowAll,
        });
        expect(out).toEqual({ ok: true, label: OPENED.label, reused: false });
        expect(viewCalls(invoke)).toEqual([
            [
                'ext_view_open',
                {
                    extensionId: 'mindmap',
                    viewId: 'explorer',
                    params: { focus: 'n1' },
                    source: 'widget',
                },
            ],
        ]);
    });

    it("turns an unknown source into 'launcher'", async () => {
        const invoke = fakeInvoke();
        await runOpenViewEffect(EFFECT, {
            extensionId: 'mindmap',
            source: 'deep-link',
            invoke,
            check: allowAll,
        });
        expect(viewCalls(invoke)[0][1].source).toBe('launcher');
    });

    it('reports reused: true when Rust showed an existing window', async () => {
        const invoke = fakeInvoke(async () => ({ label: OPENED.label, reused: true }));
        const out = await runOpenViewEffect(EFFECT, {
            extensionId: 'mindmap',
            source: 'chat',
            invoke,
            check: allowAll,
        });
        expect(out.reused).toBe(true);
    });

    it('refuses, logs and never opens when the check refuses', async () => {
        const invoke = fakeInvoke();
        const beforeOpen = vi.fn();
        const out = await runOpenViewEffect(
            { type: 'open_view', view: 'atlas' },
            { extensionId: 'mindmap', source: 'launcher', invoke, check: allowAll, beforeOpen }
        );
        expect(out).toMatchObject({
            ok: false,
            kind: 'invalid_input',
            key: 'errors.ext_view.unknown_view',
            message: 'errors.ext_view.unknown_view',
        });
        expect(invoke).not.toHaveBeenCalled();
        expect(beforeOpen).not.toHaveBeenCalled();
        expect(warn.mock.calls.flat().join(' ')).toMatch(
            /\[ext mindmap\] open_view refused: the manifest declares no view "atlas"/
        );
    });

    it('fails closed without an extension id, a check, an invoke or an open_view effect', async () => {
        const invoke = fakeInvoke();
        const cases = [
            [EFFECT, { source: 'launcher', invoke, check: allowAll }],
            [EFFECT, { extensionId: '', source: 'launcher', invoke, check: allowAll }],
            [EFFECT, { extensionId: 'mindmap', source: 'launcher', invoke }],
            [EFFECT, { extensionId: 'mindmap', source: 'launcher', check: allowAll }],
            [{ type: 'open_url', view: 'explorer' }, { extensionId: 'mindmap', invoke, check: () => null }],
            [null, { extensionId: 'mindmap', invoke, check: () => null }],
        ];
        for (const [effect, opts] of cases) {
            const out = await runOpenViewEffect(effect, opts);
            expect(out.ok).toBe(false);
        }
        expect(invoke).not.toHaveBeenCalled();
    });

    it('treats a check that throws or answers junk as a refusal', async () => {
        const invoke = fakeInvoke();
        for (const check of [
            () => {
                throw new Error('boom');
            },
            () => true,
            () => 'no',
            () => ({ reason: 'missing key' }),
        ]) {
            const out = await runOpenViewEffect(EFFECT, { extensionId: 'mindmap', invoke, check });
            expect(out).toMatchObject({ ok: false, key: 'errors.ext_view.opener_refused' });
        }
        expect(invoke).not.toHaveBeenCalled();
    });

    it("passes Rust's AppError through without throwing", async () => {
        const invoke = fakeInvoke(async () => {
            throw { kind: 'not_permitted', key: 'errors.ext_view.not_granted', message: 'Approve it' };
        });
        const out = await runOpenViewEffect(EFFECT, {
            extensionId: 'mindmap',
            invoke,
            check: allowAll,
        });
        expect(out).toMatchObject({
            ok: false,
            kind: 'not_permitted',
            key: 'errors.ext_view.not_granted',
            message: 'Approve it',
        });
    });

    it('runs beforeOpen just before the open, and survives it throwing', async () => {
        const order = [];
        const invoke = fakeInvoke(async () => {
            order.push('open');
            return OPENED;
        });
        const out = await runOpenViewEffect(EFFECT, {
            extensionId: 'mindmap',
            invoke,
            check: allowAll,
            beforeOpen: () => {
                order.push('before');
                throw new Error('hook failed');
            },
        });
        expect(order).toEqual(['before', 'open']);
        expect(out.ok).toBe(true);
    });

    it('isOpenViewEffect matches the type only', () => {
        expect(isOpenViewEffect({ type: 'open_view' })).toBe(true);
        expect(isOpenViewEffect({ type: 'open_url' })).toBe(false);
        expect(isOpenViewEffect(null)).toBe(false);
        expect(isOpenViewEffect('open_view')).toBe(false);
    });
});

// --- The extension manager's openers ----------------------------------------

function managerWith({
    sandbox,
    capabilities = ['storage', 'views'],
    enabled = true,
    hostFeatures = ['views'],
    resultSource,
    invoke = fakeInvoke(),
    manifest = MANIFEST,
} = {}) {
    const mgr = new ExtensionManager(invoke, { hostFeatures, resultSource });
    mgr._configCache = { extension_states: { mindmap: enabled }, extensions: {} };
    mgr.extensions.set('mindmap', { manifest, capabilities, sandbox });
    return { mgr, invoke };
}

function stubSandbox(responses = {}) {
    return {
        hasSearch: false,
        hasToolbar: false,
        hasFormatter: false,
        widgetIds: [],
        call: vi.fn(async (method, params) => {
            const r = responses[method];
            return typeof r === 'function' ? r(params) : r;
        }),
    };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe('ExtensionManager.extensionViewDenial', () => {
    it('checks loaded, enabled, the window, the grant, the view and the params, in order', () => {
        const sandbox = stubSandbox();
        const key = (opts, effect = EFFECT, id = 'mindmap') =>
            managerWith({ sandbox, ...opts }).mgr.extensionViewDenial(id, effect)?.key ?? null;
        expect(key({}, EFFECT, 'ghost')).toBe('errors.ext_view.unknown_extension');
        expect(key({ enabled: false, hostFeatures: [] })).toBe('errors.ext_view.disabled');
        expect(key({ hostFeatures: [], capabilities: [] })).toBe('errors.ext_view.opener_refused');
        expect(key({ capabilities: ['storage'] })).toBe('errors.ext_view.not_granted');
        expect(key({}, { type: 'open_view', view: 'atlas' })).toBe('errors.ext_view.unknown_view');
        expect(key({}, { type: 'open_view', view: 'explorer', params: [1] })).toBe(
            'errors.ext_view.params_invalid'
        );
        expect(key({})).toBeNull();
    });

    it("names the extension by its display name in the error's vars", () => {
        const { mgr } = managerWith({ sandbox: stubSandbox(), capabilities: [] });
        expect(mgr.extensionViewDenial('mindmap', EFFECT).vars).toEqual({ extension: 'Mind Map' });
    });
});

describe('ExtensionManager.hasViewCapableExtensions', () => {
    it('is true only for an enabled extension holding views that declares one', () => {
        const sandbox = stubSandbox();
        expect(managerWith({ sandbox }).mgr.hasViewCapableExtensions()).toBe(true);
        expect(managerWith({ sandbox, enabled: false }).mgr.hasViewCapableExtensions()).toBe(false);
        expect(
            managerWith({ sandbox, capabilities: ['storage'] }).mgr.hasViewCapableExtensions()
        ).toBe(false);
        expect(
            managerWith({
                sandbox,
                manifest: { ...MANIFEST, contributes: { views: [{ id: 'Bad!' }] } },
            }).mgr.hasViewCapableExtensions()
        ).toBe(false);
        expect(new ExtensionManager(fakeInvoke()).hasViewCapableExtensions()).toBe(false);
    });
});

describe('open_view from onResultAction (formatter cards and custom result rows)', () => {
    it("a formatter card's button opens the view with source 'formatter'", async () => {
        const sandbox = stubSandbox({
            formatMessage: ({ html }) => ({
                html: `${html}<button data-ext-action="explore">Explore</button>`,
            }),
            onResultAction: () => ({ host: EFFECT }),
        });
        sandbox.hasFormatter = true;
        const { mgr, invoke } = managerWith({ sandbox });
        const container = document.createElement('div');
        container.innerHTML = '<p>map</p>';
        document.body.appendChild(container);
        await mgr.formatMessage(container, { streaming: false, role: 'assistant' });
        container.querySelector('[data-ext-action="explore"]').click();
        await flush();
        expect(viewCalls(invoke)).toEqual([
            [
                'ext_view_open',
                {
                    extensionId: 'mindmap',
                    viewId: 'explorer',
                    params: { focus: 'n1' },
                    source: 'formatter',
                },
            ],
        ]);
    });

    it("a custom result row's button opens with the window's result source", async () => {
        const sandbox = stubSandbox({
            renderCustom: () => ({ html: '<button data-ext-action="open">Open</button>' }),
            onResultAction: () => ({ host: { type: 'open_view', view: 'board' } }),
        });
        sandbox.hasSearch = true;
        const { mgr, invoke } = managerWith({ sandbox, resultSource: 'chat' });
        const result = { id: 'mm:1', _extensionId: 'mindmap' };
        await mgr.prefetchCustomRender([result]);
        const el = document.createElement('div');
        mgr.renderResult(result, el);
        el.querySelector('[data-ext-action="open"]').click();
        await flush();
        expect(viewCalls(invoke)[0][1]).toEqual({
            extensionId: 'mindmap',
            viewId: 'board',
            params: null,
            source: 'chat',
        });
    });

    it('ignores other host effects and answers without one, as before', async () => {
        const answers = [{}, { host: { type: 'open_url', value: 'https://example.com' } }, null];
        for (const answer of answers) {
            const sandbox = stubSandbox({ onResultAction: () => answer });
            const { mgr, invoke } = managerWith({ sandbox });
            const root = document.createElement('div');
            root.innerHTML = '<button data-ext-action="go">Go</button>';
            mgr._wireExtActionsFor('mindmap', root, 'formatter');
            root.querySelector('button').click();
            await flush();
            expect(invoke).not.toHaveBeenCalled();
        }
    });

    it('refuses an open_view from an extension without the grant', async () => {
        const sandbox = stubSandbox({ onResultAction: () => ({ host: EFFECT }) });
        const { mgr, invoke } = managerWith({ sandbox, capabilities: ['storage'] });
        const root = document.createElement('div');
        root.innerHTML = '<button data-ext-action="go">Go</button>';
        mgr._wireExtActionsFor('mindmap', root, 'formatter');
        root.querySelector('button').click();
        await flush();
        expect(invoke).not.toHaveBeenCalled();
        expect(warn.mock.calls.flat().join(' ')).toMatch(/\[ext mindmap\] open_view refused/);
    });
});

describe('open_view from a widget action', () => {
    it("opens with source 'widget' and still honours rerender", async () => {
        const sandbox = stubSandbox({
            onWidgetAction: () => ({ host: EFFECT, rerender: true }),
            renderWidget: () => ({ html: '<span>ok</span>' }),
        });
        sandbox.widgetIds = ['w1'];
        const { mgr, invoke } = managerWith({ sandbox });
        const host = document.createElement('div');
        document.body.appendChild(host);
        const controller = {
            extensionId: 'mindmap',
            widgetId: 'w1',
            slot: 'floating-bottom',
            host,
            timer: null,
            destroyed: false,
            refreshIntervalMs: 0,
            renderInFlight: false,
            consecutiveFailures: 0,
            tripped: false,
            lastSuccessRenderAt: 0,
            paintedSignature: null,
        };
        await mgr._runWidgetAction(controller, 'explore');
        expect(viewCalls(invoke)[0][1]).toMatchObject({ viewId: 'explorer', source: 'widget' });
        expect(sandbox.call.mock.calls.some(([m]) => m === 'renderWidget')).toBe(true);
    });
});

describe('open_view from a toolbar button', () => {
    it("the manager opens it with source 'toolbar' and drops the effect", async () => {
        const sandbox = stubSandbox({
            getToolbarButtons: () => [{ id: 'map', icon: '🗺️', tooltip: 'Explorer' }],
            onToolbarClick: () => ({ host: EFFECT, note: 'kept' }),
        });
        sandbox.hasToolbar = true;
        const { mgr, invoke } = managerWith({ sandbox });
        await mgr._refreshToolbarButtons();
        const [button] = mgr.getToolbarButtons();
        const out = await button.onClick({ input: '', messages: [] });
        expect(out).toEqual({ note: 'kept' });
        expect(viewCalls(invoke)[0][1]).toEqual({
            extensionId: 'mindmap',
            viewId: 'explorer',
            params: { focus: 'n1' },
            source: 'toolbar',
        });
    });

    it('other toolbar effects still reach the window applier', async () => {
        const sandbox = stubSandbox({
            onToolbarClick: () => ({ host: { type: 'set_chat_input', value: 'hi' } }),
        });
        sandbox.hasToolbar = true;
        const { mgr, invoke } = managerWith({ sandbox });
        const out = await mgr.runToolbarClick('mindmap', 'x', {});
        expect(out).toEqual({ host: { type: 'set_chat_input', value: 'hi' } });
        expect(invoke).not.toHaveBeenCalled();
    });

    it('the toolbar applier ignores an open_view quietly', () => {
        const input = document.createElement('input');
        runToolbarHostEffect({ type: 'open_view', view: 'explorer' }, { input });
        expect(input.value).toBe('');
        expect(warn).not.toHaveBeenCalled();
    });
});

// --- Settings ---------------------------------------------------------------

function settingsRenderer({ capabilities = ['storage', 'views'], hostFeatures = ['views'] } = {}) {
    return {
        extensionId: 'mindmap',
        manifest: MANIFEST,
        sandbox: {
            capabilities: new Set(capabilities),
            hostFeatures,
            i18nCatalog: {},
            i18nFallback: {},
        },
    };
}

describe('open_view from a settings action', () => {
    it("opens with source 'settings' for the renderer's extension", async () => {
        const invoke = fakeInvoke();
        window.__TAURI__ = { core: { invoke } };
        const setStatus = vi.fn();
        await runSettingsHostSideEffect(settingsRenderer(), EFFECT, { setStatus });
        expect(viewCalls(invoke)).toEqual([
            [
                'ext_view_open',
                {
                    extensionId: 'mindmap',
                    viewId: 'explorer',
                    params: { focus: 'n1' },
                    source: 'settings',
                },
            ],
        ]);
        expect(setStatus).not.toHaveBeenCalled();
    });

    it('a missing grant says so on the action row', async () => {
        const invoke = fakeInvoke();
        window.__TAURI__ = { core: { invoke } };
        const setStatus = vi.fn();
        await runSettingsHostSideEffect(
            settingsRenderer({ capabilities: ['storage'], hostFeatures: [] }),
            EFFECT,
            { setStatus }
        );
        expect(invoke).not.toHaveBeenCalled();
        expect(setStatus).toHaveBeenCalledWith('❌ errors.ext_view.not_granted');
    });

    it("a window that doesn't offer views refuses", async () => {
        const invoke = fakeInvoke();
        window.__TAURI__ = { core: { invoke } };
        const setStatus = vi.fn();
        await runSettingsHostSideEffect(settingsRenderer({ hostFeatures: [] }), EFFECT, {
            setStatus,
        });
        expect(invoke).not.toHaveBeenCalled();
        expect(setStatus).toHaveBeenCalledWith('❌ errors.ext_view.opener_refused');
    });

    it("Rust's refusal shows on the action row", async () => {
        window.__TAURI__ = {
            core: {
                invoke: fakeInvoke(async () => {
                    throw { kind: 'not_permitted', message: 'Mind Map is turned off.' };
                }),
            },
        };
        const setStatus = vi.fn();
        await runSettingsHostSideEffect(settingsRenderer(), EFFECT, { setStatus });
        expect(setStatus).toHaveBeenCalledWith('❌ Mind Map is turned off.');
    });

    it("works end to end from a rendered settings action's runAction", async () => {
        const invoke = fakeInvoke();
        window.__TAURI__ = { core: { invoke } };
        const sandbox = {
            capabilities: new Set(['storage', 'views']),
            hostFeatures: ['views'],
            call: vi.fn(async (method) =>
                method === 'runSettingsAction' ? { status: 'Opening…', host: EFFECT } : undefined
            ),
        };
        const container = document.createElement('div');
        document.body.appendChild(container);
        const inst = renderSchema({
            extensionId: 'mindmap',
            container,
            sandbox,
            manifest: MANIFEST,
            schema: {
                sections: [
                    {
                        controls: [
                            { type: 'action', id: 'open', label: 'Open', action: 'openExplorer' },
                        ],
                    },
                ],
            },
        });
        inst.load({});
        container.querySelector('#ext-ctrl-mindmap-open').click();
        await vi.waitFor(() => expect(viewCalls(invoke)).toHaveLength(1));
        expect(viewCalls(invoke)[0][1].source).toBe('settings');
    });
});

// --- Storage change events (§10.1) ------------------------------------------

describe('ExtensionManager storage change events', () => {
    async function wired({ windowLabel = 'floating' } = {}) {
        let handler = null;
        const listen = vi.fn(async (event, fn) => {
            expect(event).toBe('extension_data_changed');
            handler = fn;
            return () => {};
        });
        const invoke = vi.fn(async (cmd) => {
            if (cmd === 'get_config') return {};
            if (cmd === 'list_extensions') return [];
            return undefined;
        });
        const mgr = new ExtensionManager(invoke, { listen, windowLabel });
        await mgr.initialize();
        const sandboxes = {
            mindmap: { capabilities: new Set(['storage']), notifyStorageChanged: vi.fn() },
            todos: { capabilities: new Set(['storage']), notifyStorageChanged: vi.fn() },
            nostore: { capabilities: new Set([]), notifyStorageChanged: vi.fn() },
        };
        for (const [id, sb] of Object.entries(sandboxes)) mgr._pool._sandboxes.set(id, sb);
        return { mgr, listen, fire: (payload) => handler({ payload }), sandboxes };
    }

    it('installs one listener and forwards a batch only to that extension, with self', async () => {
        const { mgr, listen, fire, sandboxes } = await wired();
        await mgr.initialize();
        expect(listen).toHaveBeenCalledTimes(1);
        fire({
            extension_id: 'mindmap',
            changes: [
                { key: 'mm-meta', op: 'save', source: 'window', window: 'floating' },
                { key: 'mm-view', op: 'delete', source: 'window', window: 'settings' },
            ],
        });
        expect(sandboxes.mindmap.notifyStorageChanged).toHaveBeenCalledWith([
            { key: 'mm-meta', op: 'save', self: true, source: 'window' },
            { key: 'mm-view', op: 'delete', self: false, source: 'window' },
        ]);
        expect(sandboxes.todos.notifyStorageChanged).not.toHaveBeenCalled();
    });

    it('skips a sandbox without storage and drops malformed batches', async () => {
        const { fire, sandboxes } = await wired();
        fire({ extension_id: 'nostore', changes: [{ key: 'k', op: 'save', source: 'window' }] });
        fire({ extension_id: 'mindmap', changes: [{ key: '../x', op: 'save', source: 'window' }] });
        fire({ extension_id: 'ghost', changes: [{ key: 'k', op: 'save', source: 'window' }] });
        fire(null);
        expect(sandboxes.nostore.notifyStorageChanged).not.toHaveBeenCalled();
        expect(sandboxes.mindmap.notifyStorageChanged).not.toHaveBeenCalled();
    });

    it('does nothing without a listen function (no Tauri)', async () => {
        const mgr = new ExtensionManager(vi.fn(async () => []));
        await mgr.initialize();
        expect(mgr._dataEventsInstalled).toBe(false);
    });
});
