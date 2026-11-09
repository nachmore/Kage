/**
 * The Windows block on an extension's Settings page
 * (docs/design/EXTENSION_VIEWS.md §18.4): it lists the declared views with
 * Open, has one "Reset window size and position" button, shows only for an
 * extension that holds `views`, escapes everything the manifest supplies,
 * and sits between the header blocks and the extension's own settings.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { dispatchSettingsAction } from '../../ui/js/settings/actions.js';
import {
    OPEN_VIEW_ACTION,
    openViewFromSettings,
    registerExtensionViewsActions,
    renderExtensionViewsBlock,
    RESET_GEOMETRY_ACTION,
    resetViewGeometry,
    settingsViewList,
} from '../../ui/js/settings/extension-views-block.js';

function settingsModule(overrides = {}) {
    return {
        _extensionId: 'mindmap',
        title: 'Mind Map',
        icon: '🧠',
        _capabilities: ['storage', 'views'],
        _manifest: {
            id: 'mindmap',
            name: 'Mind Map',
            contributes: {
                views: [
                    {
                        id: 'explorer',
                        title: '__MSG_view.explorer.title__',
                        icon: '🗺️',
                        module: './explorer.js',
                    },
                    { id: 'board', title: 'Board', module: './board.js' },
                    { id: 'Bad Id', title: 'Dropped', module: './x.js' },
                ],
            },
        },
        _sandbox: {
            i18nCatalog: { 'view.explorer.title': { message: 'Explorer' } },
            i18nFallback: {},
        },
        ...overrides,
    };
}

beforeEach(() => {
    document.body.innerHTML = '';
});
afterEach(() => {
    vi.restoreAllMocks();
    document.body.innerHTML = '';
});

describe('settingsViewList', () => {
    it("lists the valid views with resolved titles and the extension's icon as fallback", () => {
        expect(settingsViewList(settingsModule())).toEqual([
            { id: 'explorer', title: 'Explorer', icon: '🗺️' },
            { id: 'board', title: 'Board', icon: '🧠' },
        ]);
    });

    it('is empty without the views grant', () => {
        expect(settingsViewList(settingsModule({ _capabilities: ['storage'] }))).toEqual([]);
        expect(settingsViewList(settingsModule({ _capabilities: undefined }))).toEqual([]);
        expect(settingsViewList(settingsModule({ _capabilities: new Set(['views']) }))).toHaveLength(
            2
        );
    });

    it('is empty without declared views or an extension id', () => {
        expect(settingsViewList(settingsModule({ _manifest: { id: 'mindmap' } }))).toEqual([]);
        expect(settingsViewList(settingsModule({ _extensionId: '' }))).toEqual([]);
        expect(settingsViewList(null)).toEqual([]);
    });

    it("falls back to the extension's name when a title token doesn't resolve", () => {
        const views = settingsViewList(settingsModule({ _sandbox: {} }));
        expect(views[0].title).toBe('Mind Map');
    });

    it('strips control and bidi-override characters from titles', () => {
        const module = settingsModule({
            _sandbox: {
                i18nCatalog: { 'view.explorer.title': { message: 'Ex\u202eplo\u0007rer' } },
            },
        });
        expect(settingsViewList(module)[0].title).toBe('Explorer');
    });
});

describe('renderExtensionViewsBlock', () => {
    it('renders a heading, an Open button per view and one reset button', () => {
        document.body.innerHTML = renderExtensionViewsBlock(settingsModule());
        const block = document.querySelector('.ext-views-block');
        expect(block.dataset.extViews).toBe('mindmap');
        expect(block.querySelector('.setting-label').textContent).toBe(
            'settings.manager.ext.views.heading'
        );
        const open = [...block.querySelectorAll(`[data-action="${OPEN_VIEW_ACTION}"]`)];
        expect(open.map((b) => [b.dataset.arg, b.dataset.view])).toEqual([
            ['mindmap', 'explorer'],
            ['mindmap', 'board'],
        ]);
        expect(open.every((b) => b.type === 'button')).toBe(true);
        // Each Open button is described by its view's title.
        const described = document.getElementById(open[0].getAttribute('aria-describedby'));
        expect(described.textContent).toBe('Explorer');
        const reset = block.querySelectorAll(`[data-action="${RESET_GEOMETRY_ACTION}"]`);
        expect(reset).toHaveLength(1);
        expect(block.querySelector('[role="status"]')).toBeTruthy();
    });

    it('renders nothing for an extension without views', () => {
        expect(renderExtensionViewsBlock(settingsModule({ _capabilities: [] }))).toBe('');
    });

    it('escapes what the extension supplies', () => {
        const module = settingsModule({
            title: '<img src=x onerror=alert(1)>',
            _sandbox: {
                i18nCatalog: {
                    'view.explorer.title': { message: '<script>alert(1)</script>"><b>' },
                },
            },
        });
        document.body.innerHTML = renderExtensionViewsBlock(module);
        expect(document.querySelector('script')).toBeNull();
        expect(document.querySelector('img')).toBeNull();
        expect(document.querySelector('b')).toBeNull();
        expect(document.querySelector('.ext-views-title').textContent).toBe(
            '<script>alert(1)</script>"><b>'
        );
    });
});

describe('the block actions', () => {
    it("Open calls ext_view_open with source 'settings' and no params", async () => {
        const invoke = vi.fn(async () => ({ label: 'ext-view:mindmap:explorer', reused: false }));
        const setStatus = vi.fn();
        expect(await openViewFromSettings('mindmap', 'explorer', { invoke, setStatus })).toBe(true);
        expect(invoke).toHaveBeenCalledWith('ext_view_open', {
            extensionId: 'mindmap',
            viewId: 'explorer',
            params: null,
            source: 'settings',
        });
        expect(setStatus).toHaveBeenLastCalledWith('');
    });

    it("Open shows Rust's message when it refuses", async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const invoke = vi.fn(async () => {
            throw { kind: 'not_permitted', message: 'Mind Map is turned off.' };
        });
        const setStatus = vi.fn();
        expect(await openViewFromSettings('mindmap', 'explorer', { invoke, setStatus })).toBe(false);
        expect(setStatus).toHaveBeenLastCalledWith('Mind Map is turned off.');
    });

    it('Reset forgets every view of the extension and confirms', async () => {
        const invoke = vi.fn(async () => 2);
        const setStatus = vi.fn();
        expect(await resetViewGeometry('mindmap', { invoke, setStatus })).toBe(true);
        expect(invoke).toHaveBeenCalledWith('ext_view_reset_geometry', {
            extensionId: 'mindmap',
            viewId: null,
        });
        expect(setStatus).toHaveBeenLastCalledWith('settings.manager.ext.views.reset_done');
    });

    it('Reset shows the write error', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const invoke = vi.fn(async () => {
            throw { kind: 'internal', message: "Couldn't save window positions: disk full" };
        });
        const setStatus = vi.fn();
        expect(await resetViewGeometry('mindmap', { invoke, setStatus })).toBe(false);
        expect(setStatus).toHaveBeenLastCalledWith("Couldn't save window positions: disk full");
    });

    it('the buttons go through the settings dispatcher to the right commands', async () => {
        const invoke = vi.fn(async (cmd) => (cmd === 'ext_view_reset_geometry' ? 1 : {}));
        registerExtensionViewsActions({ invoke });
        document.body.innerHTML = renderExtensionViewsBlock(settingsModule());
        document.querySelector(`[data-view="board"]`).click();
        await vi.waitFor(() =>
            expect(invoke).toHaveBeenCalledWith('ext_view_open', {
                extensionId: 'mindmap',
                viewId: 'board',
                params: null,
                source: 'settings',
            })
        );
        document.querySelector(`[data-action="${RESET_GEOMETRY_ACTION}"]`).click();
        await vi.waitFor(() =>
            expect(document.querySelector('.ext-views-status').textContent).toBe(
                'settings.manager.ext.views.reset_done'
            )
        );
    });

    it('a button without a view id does nothing', () => {
        const invoke = vi.fn();
        registerExtensionViewsActions({ invoke });
        const el = document.createElement('button');
        dispatchSettingsAction(OPEN_VIEW_ACTION, 'mindmap', el);
        expect(invoke).not.toHaveBeenCalled();
    });
});

describe('placement on the extension page', () => {
    it('comes after the capability badges and before the extension settings', async () => {
        window.__TAURI__ = {
            core: { invoke: vi.fn(async () => ({})) },
            webviewWindow: { getCurrentWebviewWindow: () => ({ label: 'settings', close() {} }) },
            event: { listen: vi.fn(async () => () => {}) },
        };
        try {
            const { SettingsManager, SandboxedExtensionSettingsModule } = await import(
                '../../ui/js/settings/manager.js'
            );
            const container = document.createElement('div');
            container.id = 'settingsModules';
            document.body.appendChild(container);
            const base = settingsModule();
            const module = new SandboxedExtensionSettingsModule({
                extensionId: 'mindmap',
                manifest: base._manifest,
                sandbox: base._sandbox,
                rendered: { container: document.createElement('div') },
                capabilities: base._capabilities,
            });
            const mgr = new SettingsManager();
            mgr.modules = [module];
            mgr._initializeModule = () => true;
            mgr.render();
            const section = container.querySelector('[data-section-content="ext-mindmap"]');
            const badges = section.querySelector('.ext-capabilities');
            const block = section.querySelector('.ext-views-block');
            const slot = section.querySelector('#ext-sandbox-slot-mindmap');
            expect(badges && block && slot).toBeTruthy();
            const follows = (a, b) =>
                Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
            expect(follows(badges, block)).toBeTruthy();
            expect(follows(block, slot)).toBeTruthy();
            // Inside the extension's content, so turning it off dims it too.
            expect(block.closest('#ext-content-mindmap')).toBeTruthy();
        } finally {
            delete window.__TAURI__;
        }
    });
});
