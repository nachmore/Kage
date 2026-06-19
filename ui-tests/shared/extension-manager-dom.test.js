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

describe('ExtensionManager.reload serialization', () => {
    // Build a manager whose invoke('list_extensions') we can stall, so we can
    // overlap reload() calls deterministically.
    function makeReloadHarness() {
        let releaseList;
        const listGate = new Promise((r) => { releaseList = r; });
        const counts = { get_config: 0, list_extensions: 0 };
        const invoke = async (cmd) => {
            counts[cmd] = (counts[cmd] || 0) + 1;
            if (cmd === 'get_config') return { extension_states: {}, extensions: {} };
            if (cmd === 'list_extensions') {
                await listGate; // block until the test releases it
                return [];
            }
            return undefined;
        };
        const mgr = new ExtensionManager(invoke);
        return { mgr, counts, releaseList };
    }

    it('does not run two reloads concurrently; coalesces a burst into one rerun', async () => {
        const { mgr, counts, releaseList } = makeReloadHarness();

        // Fire three reloads while the first is stalled on list_extensions.
        const p1 = mgr.reload();
        const p2 = mgr.reload();
        const p3 = mgr.reload();

        // Let the first reload advance past its get_config await to the
        // stalled list_extensions call.
        await new Promise((r) => setTimeout(r, 0));

        // Only the first has entered the body (one list_extensions call so far);
        // p2/p3 collapsed into a single pending rerun.
        expect(counts.list_extensions).toBe(1);

        releaseList();
        await Promise.all([p1, p2, p3]);

        // The in-flight run plus exactly ONE trailing rerun for the burst.
        expect(counts.list_extensions).toBe(2);
        expect(mgr._reloadInFlight).toBeNull();
    });

    it('a fresh reload after settling runs normally', async () => {
        const { mgr, releaseList } = makeReloadHarness();
        releaseList(); // don't stall this time
        await mgr.reload();
        expect(mgr._reloadInFlight).toBeNull();
        await mgr.reload(); // second, independent reload
        expect(mgr._reloadInFlight).toBeNull();
    });
});

describe('ExtensionManager keyword gate (matchAll / matchAllAsync)', () => {
    // Build a manager holding several search extensions at once. Each spec
    // entry: { id, keywords?: [{keyword,...}], matchRows?: [...] }.
    function makeManagerWithSearchExtensions(specs) {
        const mgr = new ExtensionManager(async () => undefined);
        mgr._configCache = { extension_states: {}, extension_grants: {}, extensions: {} };
        for (const spec of specs) {
            mgr._configCache.extension_states[spec.id] = true;
            const sandbox = stubSandbox({
                getKeywords: () => spec.keywords ?? [],
                match: () => spec.matchRows ?? [{ id: spec.id + ':row', label: spec.id }],
                matchAsync: () => spec.matchRows ?? [{ id: spec.id + ':row', label: spec.id }],
            });
            sandbox.hasSearch = true;
            // Track how often the per-keystroke methods are invoked.
            const realCall = sandbox.call.bind(sandbox);
            sandbox.callCounts = { match: 0, matchAsync: 0, getKeywords: 0 };
            sandbox.call = (method, params) => {
                if (method in sandbox.callCounts) sandbox.callCounts[method]++;
                return realCall(method, params);
            };
            mgr.extensions.set(spec.id, {
                manifest: { id: spec.id, name: spec.id, icon: '🔌', permissions: [] },
                i18n: { catalog: {}, fallback: {} },
                capabilities: [],
                sandbox,
            });
        }
        return mgr;
    }

    const KW = (keyword, acceptsArgs = true) => ({
        keyword,
        labelKey: 'k.label',
        descriptionKey: 'k.desc',
        acceptsArgs,
    });

    it('calls a keyword extension only when its keyword is committed', async () => {
        const mgr = makeManagerWithSearchExtensions([
            { id: 'calendar', keywords: [KW('cal'), KW('cal-refresh', false)] },
        ]);
        const cal = mgr.extensions.get('calendar').sandbox;

        // Incomplete prefix → no match() call (hint handles it elsewhere).
        await mgr.matchAll('ca');
        expect(cal.callCounts.match).toBe(0);

        // Exact keyword → called.
        await mgr.matchAll('cal');
        expect(cal.callCounts.match).toBe(1);

        // Keyword + space (args) → called.
        await mgr.matchAll('cal tomorrow');
        expect(cal.callCounts.match).toBe(2);

        // Unrelated query → not called.
        await mgr.matchAll('weather');
        expect(cal.callCounts.match).toBe(2);
    });

    it('does not fire on a bare prefix of an args keyword (whole-word gate)', async () => {
        const mgr = makeManagerWithSearchExtensions([{ id: 'bookmarks', keywords: [KW('bm')] }]);
        const bm = mgr.extensions.get('bookmarks').sandbox;
        // "bmfoo" begins with "bm" but is NOT the whole word — must not fire.
        await mgr.matchAll('bmfoo');
        expect(bm.callCounts.match).toBe(0);
        await mgr.matchAll('bm foo');
        expect(bm.callCounts.match).toBe(1);
    });

    it('always calls a content matcher (no registered keywords)', async () => {
        const mgr = makeManagerWithSearchExtensions([{ id: 'math', keywords: [] }]);
        const math = mgr.extensions.get('math').sandbox;
        await mgr.matchAll('2+2');
        await mgr.matchAll('anything at all');
        expect(math.callCounts.match).toBe(2);
    });

    it('gates each extension independently in a mixed roster', async () => {
        const mgr = makeManagerWithSearchExtensions([
            { id: 'calendar', keywords: [KW('cal')] },
            { id: 'math', keywords: [] }, // content matcher
            { id: 'spotify', keywords: [KW('sp')] },
        ]);
        const results = await mgr.matchAll('cal');
        // calendar matched (keyword), math always runs, spotify gated out.
        const ids = results.map((r) => r._extensionId);
        expect(ids).toContain('calendar');
        expect(ids).toContain('math');
        expect(ids).not.toContain('spotify');
        expect(mgr.extensions.get('spotify').sandbox.callCounts.match).toBe(0);
    });

    it('matchAllAsync applies the same gate', async () => {
        const mgr = makeManagerWithSearchExtensions([
            { id: 'calendar', keywords: [KW('cal')] },
        ]);
        const cal = mgr.extensions.get('calendar').sandbox;
        await mgr.matchAllAsync('ca');
        expect(cal.callCounts.matchAsync).toBe(0);
        await mgr.matchAllAsync('cal');
        expect(cal.callCounts.matchAsync).toBe(1);
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
