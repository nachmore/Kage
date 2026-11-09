/**
 * Tests for executeResult's handling of extension-returned actions:
 *
 * - `open_extension_settings` deep-links the settings window to the
 *   extension's own page (used by Spotify's "connect before a Client ID is
 *   saved" flow). The settings target is derived from the host-stamped
 *   `_extensionId`, never from anything the extension returns, so a
 *   malicious provider can't navigate the user to an arbitrary section.
 * - `open_url` / `open_path` (Bookmarks, GitHub) run only with the
 *   capability the command needs (`urls` / `launch`) and the bridge's
 *   argument policy, exactly as if the extension had invoked the command
 *   itself. `open_url` takes http, https and mailto links only.
 * - `send_prompt` is an alias of `prompt`.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

let executeResult;
let ExtensionManager;

beforeEach(async () => {
    vi.resetModules();
    vi.doMock('../../ui/js/shared/commands.js', () => ({ executeCommand: vi.fn() }));
    vi.doMock('../../ui/js/shared/shortcuts.js', () => ({ buildShortcutCommand: vi.fn() }));
    vi.doMock('../../ui/js/shared/kage-log.js', () => ({
        kageLog: { info: vi.fn(), warn: vi.fn() },
    }));
    vi.doMock('../../ui/js/shared/search-engine.js', () => ({ recordSelection: vi.fn() }));
    ({ executeResult } = await import('../../ui/js/shared/result-executor.js'));
    ({ ExtensionManager } = await import('../../ui/js/shared/extension-manager.js'));
});

afterEach(() => {
    vi.restoreAllMocks();
});

function makeExtensionManager(action) {
    return { executeResult: vi.fn(async () => action) };
}

describe('executeResult — open_extension_settings action', () => {
    it('opens settings deep-linked to the extension section', async () => {
        const invoke = vi.fn(async () => {});
        const result = { id: 'spotify:connect', _extensionId: 'spotify', data: { id: 'connect' } };

        const out = await executeResult(result, 'sp connect', {
            invoke,
            extensionManager: makeExtensionManager({ type: 'open_extension_settings' }),
        });

        expect(out.handled).toBe(true);
        expect(invoke).toHaveBeenCalledWith('open_settings_window', {
            section: 'ext-spotify',
        });
    });

    it('derives the section from the host-stamped id, ignoring extension-supplied targets', async () => {
        const invoke = vi.fn(async () => {});
        const result = { id: 'evil:x', _extensionId: 'evil', data: { id: 'x' } };

        // A hostile extension trying to steer navigation elsewhere.
        await executeResult(result, 'x', {
            invoke,
            extensionManager: makeExtensionManager({
                type: 'open_extension_settings',
                section: 'privacy',
                value: 'tool-permissions',
            }),
        });

        expect(invoke).toHaveBeenCalledWith('open_settings_window', {
            section: 'ext-evil',
        });
    });

    it('a failed open does not reject the whole execute', async () => {
        const invoke = vi.fn(async () => {
            throw new Error('window create failed');
        });
        const result = { id: 'spotify:connect', _extensionId: 'spotify', data: { id: 'connect' } };
        vi.spyOn(console, 'warn').mockImplementation(() => {});

        const out = await executeResult(result, 'sp connect', {
            invoke,
            extensionManager: makeExtensionManager({ type: 'open_extension_settings' }),
        });
        expect(out.handled).toBe(true);
    });
});

/**
 * A real ExtensionManager with one loaded extension whose execute() returns
 * `action`, so the capability gate under test is the production one.
 */
function managerWith({ action, capabilities = [], enabled = true, id = 'bookmarks' }) {
    const mgr = new ExtensionManager(vi.fn(async () => null));
    mgr._configCache = { extension_states: { [id]: enabled } };
    mgr.extensions.set(id, {
        manifest: { id },
        capabilities,
        sandbox: {
            hasSearch: true,
            call: vi.fn(async (method) => (method === 'execute' ? action : null)),
        },
    });
    return mgr;
}

async function run(action, { capabilities = [], enabled = true, ctx = {} } = {}) {
    const invoke = vi.fn(async () => {});
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const extensionManager = managerWith({ action, capabilities, enabled });
    const result = { id: 'bm:open:docs', _extensionId: 'bookmarks', data: { open: 'x' } };
    const out = await executeResult(result, 'bm docs', { invoke, extensionManager, ...ctx });
    return { out, invoke, warn };
}

describe('executeResult — open_url action', () => {
    it('opens an http(s) link for an extension holding urls, then hides', async () => {
        const { out, invoke } = await run(
            { type: 'open_url', value: 'https://example.com/docs?q=1' },
            { capabilities: ['storage', 'urls'] }
        );
        expect(invoke).toHaveBeenCalledWith('open_url', { url: 'https://example.com/docs?q=1' });
        expect(out).toEqual({ handled: true, action: 'hide' });
    });

    it('sends the parsed, normalised URL and allows mailto', async () => {
        const spaced = await run(
            { type: 'open_url', value: '  http://Example.com/a b ' },
            { capabilities: ['urls'] }
        );
        expect(spaced.invoke).toHaveBeenCalledWith('open_url', { url: 'http://example.com/a%20b' });
        const mail = await run(
            { type: 'open_url', value: 'mailto:dana@example.com?subject=Hi there' },
            { capabilities: ['urls'] }
        );
        expect(mail.invoke).toHaveBeenCalledWith('open_url', {
            url: 'mailto:dana@example.com?subject=Hi%20there',
        });
    });

    it('refuses without the urls capability', async () => {
        const { out, invoke, warn } = await run(
            { type: 'open_url', value: 'https://example.com' },
            { capabilities: ['storage', 'launch'] }
        );
        expect(invoke).not.toHaveBeenCalled();
        expect(out).toEqual({ handled: true });
        expect(warn.mock.calls.flat().join(' ')).toMatch(/missing capability 'urls'/);
    });

    it('refuses other schemes, relative links and junk, even with urls and launch', async () => {
        const values = [
            'javascript:alert(1)',
            'file:///etc/passwd',
            'tel:+15555550100',
            'ms-settings:privacy',
            'spotify:track:1',
            'data:text/html,hi',
            'example.com',
            'www.example.com',
            '/relative/path',
            'https://',
            '',
            '   ',
            42,
            null,
            { toString: () => 'https://example.com' },
        ];
        for (const value of values) {
            const { out, invoke } = await run(
                { type: 'open_url', value },
                { capabilities: ['urls', 'launch'] }
            );
            expect(invoke, String(value)).not.toHaveBeenCalled();
            expect(out).toEqual({ handled: true });
        }
    });

    it('refuses for a disabled extension', async () => {
        const { invoke } = await run(
            { type: 'open_url', value: 'https://example.com' },
            { capabilities: ['urls'], enabled: false }
        );
        expect(invoke).not.toHaveBeenCalled();
    });

    it('fails closed when the manager cannot check capabilities', async () => {
        const invoke = vi.fn(async () => {});
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const out = await executeResult(
            { id: 'x', _extensionId: 'x' },
            'x',
            { invoke, extensionManager: makeExtensionManager({ type: 'open_url', value: 'https://example.com' }) }
        );
        expect(invoke).not.toHaveBeenCalled();
        expect(out).toEqual({ handled: true });
    });

    it('a failed open neither rejects nor hides', async () => {
        const invoke = vi.fn(async () => {
            throw new Error('no browser');
        });
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const out = await executeResult(
            { id: 'bm:open:docs', _extensionId: 'bookmarks' },
            'bm docs',
            {
                invoke,
                extensionManager: managerWith({
                    action: { type: 'open_url', value: 'https://example.com' },
                    capabilities: ['urls'],
                }),
            }
        );
        expect(invoke).toHaveBeenCalledTimes(1);
        expect(out).toEqual({ handled: true });
    });
});

describe('executeResult — open_path action', () => {
    it('opens a path for an extension holding launch, then hides', async () => {
        const { out, invoke } = await run(
            { type: 'open_path', value: '~/Documents/plan.pdf' },
            { capabilities: ['launch'] }
        );
        expect(invoke).toHaveBeenCalledWith('open_path', { path: '~/Documents/plan.pdf' });
        expect(out).toEqual({ handled: true, action: 'hide' });
    });

    it('refuses without launch (urls is not enough) and for an empty value', async () => {
        const noLaunch = await run(
            { type: 'open_path', value: '/tmp/plan.pdf' },
            { capabilities: ['urls', 'storage'] }
        );
        expect(noLaunch.invoke).not.toHaveBeenCalled();
        expect(noLaunch.warn.mock.calls.flat().join(' ')).toMatch(/missing capability 'launch'/);
        const empty = await run({ type: 'open_path', value: ' ' }, { capabilities: ['launch'] });
        expect(empty.invoke).not.toHaveBeenCalled();
    });
});

describe('executeResult — prompt actions', () => {
    it('send_prompt is an alias of prompt', async () => {
        for (const type of ['prompt', 'send_prompt']) {
            const onPrompt = vi.fn(async () => {});
            const { invoke } = await run({ type, value: 'Summarise my notes' }, { ctx: { onPrompt } });
            expect(onPrompt).toHaveBeenCalledWith('Summarise my notes');
            expect(invoke).not.toHaveBeenCalled();
        }
    });
});

/**
 * `open_view` (docs/design/EXTENSION_VIEWS.md §6): a result row opens one of
 * the extension's own views. The host checks run first (the window offers
 * `views`, the extension holds it, the manifest declares the view, the
 * params are valid); then `ext_view_open` with the row's source.
 */
describe('executeResult — open_view action', () => {
    const MANIFEST = {
        id: 'mindmap',
        name: 'Mind Map',
        permissions: ['storage', 'views'],
        contributes: {
            views: [{ id: 'explorer', title: 'Explorer', module: './explorer.js' }],
        },
    };
    const OPENED = { label: 'ext-view:mindmap:explorer', reused: false };

    function viewManager({
        action,
        capabilities = ['storage', 'views'],
        enabled = true,
        hostFeatures = ['views'],
        resultSource,
        manifest = MANIFEST,
        onBeforeOpenView,
    }) {
        const mgr = new ExtensionManager(vi.fn(async () => null), {
            hostFeatures,
            resultSource,
            onBeforeOpenView,
        });
        mgr._configCache = { extension_states: { mindmap: enabled } };
        mgr.extensions.set('mindmap', {
            manifest,
            capabilities,
            sandbox: {
                hasSearch: true,
                call: vi.fn(async (method) => (method === 'execute' ? action : null)),
            },
        });
        return mgr;
    }

    async function openRun(action, opts = {}, { invokeImpl, row } = {}) {
        const invoke = vi.fn(
            invokeImpl || (async (cmd) => (cmd === 'ext_view_open' ? OPENED : undefined))
        );
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const onDisplay = vi.fn();
        const extensionManager = viewManager({ action, ...opts });
        const result = row || { id: 'mm:open', _extensionId: 'mindmap', data: { node: 'n1' } };
        const out = await executeResult(result, 'mm explore', {
            invoke,
            extensionManager,
            onDisplay,
        });
        return { out, invoke, warn, onDisplay };
    }

    const viewCalls = (invoke) => invoke.mock.calls.filter(([cmd]) => cmd === 'ext_view_open');

    it('opens the view from the launcher, and the launcher stays', async () => {
        const { out, invoke, onDisplay } = await openRun({
            type: 'open_view',
            view: 'explorer',
            params: { focus: 'n1' },
        });
        expect(viewCalls(invoke)).toEqual([
            [
                'ext_view_open',
                {
                    extensionId: 'mindmap',
                    viewId: 'explorer',
                    params: { focus: 'n1' },
                    source: 'launcher',
                },
            ],
        ]);
        // No 'hide': the launcher keeps its response area and clears its input.
        expect(out).toEqual({ handled: true });
        expect(onDisplay).not.toHaveBeenCalled();
    });

    it("opens from a chat window with source 'chat' and null params", async () => {
        const { invoke } = await openRun(
            { type: 'open_view', view: 'explorer' },
            { resultSource: 'chat' }
        );
        expect(viewCalls(invoke)[0][1]).toEqual({
            extensionId: 'mindmap',
            viewId: 'explorer',
            params: null,
            source: 'chat',
        });
    });

    it('refuses without the views capability and says why', async () => {
        const { out, invoke, warn, onDisplay } = await openRun(
            { type: 'open_view', view: 'explorer' },
            { capabilities: ['storage'] }
        );
        expect(viewCalls(invoke)).toEqual([]);
        expect(out).toEqual({ handled: true });
        expect(warn.mock.calls.flat().join(' ')).toMatch(
            /\[ext mindmap\] open_view refused: .*'views' capability/
        );
        expect(onDisplay).toHaveBeenCalledWith('errors.ext_view.not_granted');
    });

    it('refuses a view the manifest does not declare', async () => {
        for (const view of ['atlas', '', 'Explorer', 42, null, '../explorer']) {
            const { invoke, onDisplay } = await openRun({ type: 'open_view', view });
            expect(viewCalls(invoke), String(view)).toEqual([]);
            expect(onDisplay).toHaveBeenCalledWith('errors.ext_view.unknown_view');
        }
    });

    it("refuses in a window that doesn't offer views", async () => {
        const { invoke, onDisplay } = await openRun(
            { type: 'open_view', view: 'explorer' },
            { hostFeatures: ['date-picker'] }
        );
        expect(viewCalls(invoke)).toEqual([]);
        expect(onDisplay).toHaveBeenCalledWith('errors.ext_view.opener_refused');
    });

    it('refuses params that are not small, plain JSON', async () => {
        const bad = [
            [1, 2],
            'focus',
            { a: { b: { c: { d: { e: 1 } } } } },
            { big: 'x'.repeat(5000) },
            { n: Number.NaN },
            { when: new Date(0) },
        ];
        for (const params of bad) {
            const { invoke, onDisplay } = await openRun({
                type: 'open_view',
                view: 'explorer',
                params,
            });
            expect(viewCalls(invoke)).toEqual([]);
            expect(onDisplay.mock.calls[0][0]).toMatch(
                /^errors\.ext_view\.params_(invalid|too_large)$/
            );
        }
    });

    it('refuses for a disabled extension', async () => {
        const { invoke, onDisplay } = await openRun(
            { type: 'open_view', view: 'explorer' },
            { enabled: false }
        );
        expect(viewCalls(invoke)).toEqual([]);
        expect(onDisplay).toHaveBeenCalledWith('errors.ext_view.disabled');
    });

    it("names the extension from the row's host stamp, never from the effect", async () => {
        const { invoke } = await openRun({
            type: 'open_view',
            view: 'explorer',
            extensionId: 'other',
            extension_id: 'other',
            source: 'settings',
        });
        expect(viewCalls(invoke)[0][1]).toMatchObject({
            extensionId: 'mindmap',
            source: 'launcher',
        });
    });

    it("shows the AppError's message when Rust refuses, and never rejects", async () => {
        const { out, onDisplay, warn } = await openRun(
            { type: 'open_view', view: 'explorer' },
            {},
            {
                invokeImpl: async (cmd) => {
                    if (cmd !== 'ext_view_open') return undefined;
                    throw {
                        kind: 'rate_limited',
                        key: 'errors.ext_view.too_many_open',
                        message: 'Too many extension windows are open (limit 4).',
                    };
                },
            }
        );
        expect(out).toEqual({ handled: true });
        expect(onDisplay).toHaveBeenCalledWith('Too many extension windows are open (limit 4).');
        expect(warn.mock.calls.flat().join(' ')).toMatch(/\[ext mindmap\] open_view failed/);
    });

    it('runs the window hook only once the checks pass, before the open', async () => {
        const order = [];
        const onBeforeOpenView = vi.fn(() => order.push('before'));
        const invokeImpl = async (cmd) => {
            if (cmd === 'ext_view_open') order.push('open');
            return OPENED;
        };
        await openRun({ type: 'open_view', view: 'explorer' }, { onBeforeOpenView }, { invokeImpl });
        expect(order).toEqual(['before', 'open']);

        const refused = vi.fn();
        await openRun(
            { type: 'open_view', view: 'nope' },
            { onBeforeOpenView: refused },
            { invokeImpl }
        );
        expect(refused).not.toHaveBeenCalled();
    });

    it('fails closed when the manager cannot open views', async () => {
        const invoke = vi.fn(async () => {});
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const out = await executeResult({ id: 'x', _extensionId: 'x' }, 'x', {
            invoke,
            extensionManager: makeExtensionManager({ type: 'open_view', view: 'explorer' }),
        });
        expect(invoke).not.toHaveBeenCalled();
        expect(out).toEqual({ handled: true });
        expect(warn.mock.calls.flat().join(' ')).toMatch(/open_view refused/);
    });
});
