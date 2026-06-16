/**
 * DOM-surface tests for the ExtensionManager's widget/toolbar/formatter
 * path. We can't easily spin up a real iframe sandbox in jsdom, so we
 * stub the sandbox pool and extensions map directly and exercise the
 * host-side rendering logic.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';

function stubSandbox(responses = {}) {
    return {
        hasSearch: false,
        hasTools: false,
        hasTriggers: false,
        hasSettings: false,
        hasToolbar: false,
        hasFormatter: false,
        widgetIds: [],
        async call(method, params) {
            const r = responses[method];
            if (typeof r === 'function') return r(params);
            return r;
        },
        updateConfig: vi.fn(),
        destroy: vi.fn(),
    };
}

function makeManagerWithExtension({ extensionId, manifest, sandbox, enabled = true }) {
    const mgr = new ExtensionManager(async () => undefined);
    mgr._configCache = {
        extension_states: { [extensionId]: enabled },
        extension_grants: {},
        extensions: {},
    };
    mgr.extensions.set(extensionId, {
        manifest,
        basePath: null,
        userInstalled: false,
        capabilities: manifest.permissions || [],
        sandbox,
    });
    return mgr;
}

describe('ExtensionManager toolbar', () => {
    it('collects and returns buttons from enabled extensions', async () => {
        const sandbox = stubSandbox({
            getToolbarButtons: () => [
                { id: 'todos-summary', icon: '✅', tooltip: 'Show summary' },
            ],
        });
        sandbox.hasToolbar = true;

        const mgr = makeManagerWithExtension({
            extensionId: 'todos',
            manifest: { id: 'todos', name: 'Todos', permissions: [] },
            sandbox,
        });

        await mgr._refreshToolbarButtons();
        const buttons = mgr.getToolbarButtons();
        expect(buttons.length).toBe(1);
        expect(buttons[0].id).toBe('todos-summary');
        expect(buttons[0].extensionId).toBe('todos');
        expect(typeof buttons[0].onClick).toBe('function');
    });

    it('routes onClick through the sandbox with safe context', async () => {
        const seen = [];
        const sandbox = stubSandbox({
            getToolbarButtons: () => [{ id: 'go', icon: '▶', tooltip: 'Go' }],
            onToolbarClick: (p) => {
                seen.push(p);
                return { host: { type: 'set_chat_input', value: 'hello' } };
            },
        });
        sandbox.hasToolbar = true;

        const mgr = makeManagerWithExtension({
            extensionId: 'x',
            manifest: { id: 'x', name: 'X', permissions: [] },
            sandbox,
        });
        await mgr._refreshToolbarButtons();
        const btn = mgr.getToolbarButtons()[0];

        const result = await btn.onClick({
            input: 'current',
            messages: [{ role: 'user', content: 'hi' }],
        });

        expect(seen.length).toBe(1);
        expect(seen[0].buttonId).toBe('go');
        expect(seen[0].context.input).toBe('current');
        expect(seen[0].context.messages).toEqual([{ role: 'user', content: 'hi' }]);
        expect(result).toEqual({ host: { type: 'set_chat_input', value: 'hello' } });
    });

    it('returns [] when extension is disabled', async () => {
        const sandbox = stubSandbox({
            getToolbarButtons: () => [{ id: 'x', icon: 'x', tooltip: 'x' }],
        });
        sandbox.hasToolbar = true;

        const mgr = makeManagerWithExtension({
            extensionId: 'todos',
            manifest: { id: 'todos', name: 'Todos', permissions: [] },
            sandbox,
            enabled: false,
        });
        await mgr._refreshToolbarButtons();
        expect(mgr.getToolbarButtons().length).toBe(0);
    });
});

describe('ExtensionManager renderResult (custom renderer)', () => {
    it('injects sanitized HTML into the provided element', async () => {
        const sandbox = stubSandbox({
            renderCustom: () => ({
                html: '<div class="app-name">Title</div><script>evil()</script>',
                className: 'my-result',
            }),
        });
        sandbox.hasSearch = true;

        const mgr = makeManagerWithExtension({
            extensionId: 'cal',
            manifest: { id: 'cal', name: 'Cal', permissions: [] },
            sandbox,
        });

        const result = { id: 'cal:1', _extensionId: 'cal', label: 'x' };
        await mgr.prefetchCustomRender([result]);

        const el = document.createElement('div');
        const handled = mgr.renderResult(result, el);
        expect(handled).toBe(true);
        expect(el.querySelector('.app-name')?.textContent).toBe('Title');
        expect(el.innerHTML).not.toContain('<script');
        expect(el.classList.contains('my-result')).toBe(true);
    });

    it('returns false (falls back to default) when nothing cached', () => {
        const sandbox = stubSandbox();
        sandbox.hasSearch = true;
        const mgr = makeManagerWithExtension({
            extensionId: 'x',
            manifest: { id: 'x', name: 'X', permissions: [] },
            sandbox,
        });
        const el = document.createElement('div');
        expect(mgr.renderResult({ id: 'x:1', _extensionId: 'x' }, el)).toBe(false);
    });

    it('returns false when the extension has no search provider', () => {
        const sandbox = stubSandbox();
        sandbox.hasSearch = false;
        const mgr = makeManagerWithExtension({
            extensionId: 'x',
            manifest: { id: 'x', name: 'X', permissions: [] },
            sandbox,
        });
        const el = document.createElement('div');
        expect(mgr.renderResult({ id: 'x:1', _extensionId: 'x' }, el)).toBe(false);
    });

    it('wires data-ext-action buttons to sandbox RPCs', async () => {
        let calls = [];
        const sandbox = stubSandbox({
            renderCustom: () => ({
                html: '<button data-ext-action="do">Go</button>',
            }),
            onResultAction: (p) => { calls.push(p); return {}; },
        });
        sandbox.hasSearch = true;

        const mgr = makeManagerWithExtension({
            extensionId: 'x',
            manifest: { id: 'x', name: 'X', permissions: [] },
            sandbox,
        });

        const result = { id: 'x:1', _extensionId: 'x' };
        await mgr.prefetchCustomRender([result]);
        const el = document.createElement('div');
        mgr.renderResult(result, el);

        const btn = el.querySelector('button[data-ext-action="do"]');
        expect(btn).toBeTruthy();
        btn.click();
        await new Promise(r => setTimeout(r, 0));
        expect(calls.length).toBe(1);
        expect(calls[0].actionId).toBe('do');
    });
});

describe('ExtensionManager formatMessage', () => {
    it('replaces container HTML with sanitized formatter output', async () => {
        const sandbox = stubSandbox({
            formatMessage: ({ html }) => ({
                html: html + '<p class="ext-annotation">hello<script>evil()</script></p>',
            }),
        });
        sandbox.hasFormatter = true;

        const mgr = makeManagerWithExtension({
            extensionId: 'lp',
            manifest: { id: 'lp', name: 'Link Preview', permissions: [] },
            sandbox,
        });

        const container = document.createElement('div');
        container.innerHTML = '<p>original</p>';
        await mgr.formatMessage(container, { streaming: false, role: 'assistant' });
        expect(container.textContent).toContain('original');
        expect(container.querySelector('.ext-annotation')?.textContent).toBe('hello');
        expect(container.innerHTML).not.toContain('<script');
    });

    it('skips formatter when extension is disabled', async () => {
        const sandbox = stubSandbox({
            formatMessage: () => ({ html: 'should not appear' }),
        });
        sandbox.hasFormatter = true;

        const mgr = makeManagerWithExtension({
            extensionId: 'lp',
            manifest: { id: 'lp', name: 'Link Preview', permissions: [] },
            sandbox,
            enabled: false,
        });
        const container = document.createElement('div');
        container.innerHTML = '<p>ok</p>';
        await mgr.formatMessage(container, { streaming: false });
        expect(container.innerHTML).toContain('<p>ok</p>');
        expect(container.innerHTML).not.toContain('should not appear');
    });
});

describe('ExtensionManager widget render', () => {
    // Build a manager with one mounted widget controller wired to a stub
    // sandbox, returning the host element and the renderWidget call count.
    function makeMountedWidget(renderImpl) {
        const calls = { renderWidget: 0 };
        const sandbox = stubSandbox({
            renderWidget: (params) => {
                calls.renderWidget++;
                return renderImpl(params);
            },
        });
        sandbox.widgetIds = ['w'];
        const mgr = makeManagerWithExtension({
            extensionId: 'ext',
            manifest: { id: 'ext', name: 'Ext', permissions: [] },
            sandbox,
        });
        const host = document.createElement('div');
        if (!mgr._widgetInstances) mgr._widgetInstances = new Map();
        mgr._widgetInstances.set('ext:w', {
            extensionId: 'ext',
            widgetId: 'w',
            slot: 'floating-bottom',
            host,
            renderInFlight: false,
            consecutiveFailures: 0,
            tripped: false,
            destroyed: false,
            refreshIntervalMs: 60_000,
            lastSuccessRenderAt: 0,
        });
        return { mgr, host, calls };
    }

    beforeEach(() => {
        // Default to "visible" so individual tests opt into the hidden state.
        window._kageFloatingHidden = false;
    });

    it('renderAllWidgets paints mounted widgets', async () => {
        const { mgr, host } = makeMountedWidget(() => ({ html: '<span>hi</span>' }));
        mgr.renderAllWidgets();
        // renderAllWidgets is fire-and-forget; flush microtasks.
        await new Promise((r) => setTimeout(r, 0));
        expect(host.innerHTML).toContain('hi');
        expect(host.style.display).not.toBe('none');
    });

    it('skips rendering while the floating window is hidden', async () => {
        const { mgr, host, calls } = makeMountedWidget(() => ({ html: '<span>hi</span>' }));
        window._kageFloatingHidden = true;
        mgr.renderAllWidgets();
        await new Promise((r) => setTimeout(r, 0));
        expect(calls.renderWidget).toBe(0);
        expect(host.innerHTML).toBe('');
    });

    it('catches up the render once the window becomes visible', async () => {
        const { mgr, host, calls } = makeMountedWidget(() => ({ html: '<span>late</span>' }));
        // Hidden: a render is requested but skipped (e.g. mounted via a
        // hot-update while the launcher was closed).
        window._kageFloatingHidden = true;
        mgr.renderAllWidgets();
        await new Promise((r) => setTimeout(r, 0));
        expect(calls.renderWidget).toBe(0);

        // Shown: the catch-up render now paints.
        window._kageFloatingHidden = false;
        mgr.renderAllWidgets();
        await new Promise((r) => setTimeout(r, 0));
        expect(calls.renderWidget).toBe(1);
        expect(host.innerHTML).toContain('late');
    });
});
