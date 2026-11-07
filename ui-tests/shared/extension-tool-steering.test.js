/**
 * Extension tool steering stays in sync with the extension tools after
 * startup: a setting that turns an extension's agent tools on (Stocks'
 * "Allow tools for agents"), an extension enabled, installed, disabled or
 * removed. The window re-publishes once per real change, never for a
 * refresh that found the same tools, and nothing after stopSteering(). (The
 * backend attaches what was published to the window's next user prompt.)
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
    ExtensionManager,
    formatToolParam,
    formatToolSteeringBlock,
} from '../../ui/js/shared/extension-manager.js';
import {
    ExtensionToolController,
    STEERING_REFRESH_DEBOUNCE_MS,
} from '../../ui/js/shared/extension-tool-controller.js';

const QUOTE_TOOL = {
    name: 'get_quote',
    description: 'Latest price for a ticker',
    parameters: { symbol: { type: 'string', description: 'Ticker' } },
};

/** A tools-capable sandbox whose getTools() follows its config. */
function stocksSandbox() {
    const sandbox = {
        hasTools: true,
        config: {},
        updateConfig: vi.fn(async (cfg) => {
            sandbox.config = cfg || {};
        }),
        call: vi.fn(async (method) => {
            if (method === 'getTools') return sandbox.config.allowAgentTools ? [QUOTE_TOOL] : [];
            return undefined;
        }),
        destroy: vi.fn(),
    };
    return sandbox;
}

/**
 * A manager with one loaded "stocks" extension, backed by an app config the
 * test mutates the way Settings would before emitting config_updated.
 */
function makeHarness({ allowAgentTools = false } = {}) {
    const appConfig = {
        extension_states: { stocks: true },
        extension_grants: {},
        extensions: { stocks: { allowAgentTools } },
    };
    const manifest = { id: 'stocks', name: 'Stocks', version: '1.0.0', icon: '📈' };
    const sandbox = stocksSandbox();
    sandbox.config = { allowAgentTools };
    const backend = vi.fn(async (cmd) => {
        if (cmd === 'get_config') return structuredClone(appConfig);
        if (cmd === 'list_extensions') {
            return [{ manifest, enabled: appConfig.extension_states.stocks !== false }];
        }
        return undefined;
    });
    const mgr = new ExtensionManager(backend);
    mgr._configCache = structuredClone(appConfig);
    mgr.extensions.set('stocks', { manifest, capabilities: [], sandbox });
    const hostInvoke = vi.fn().mockResolvedValue(undefined);
    const controller = new ExtensionToolController({
        invoke: hostInvoke,
        getSessionId: () => 'sess-1',
        extensionManager: mgr,
        permissionModal: { showForExtensionTool: vi.fn() },
    });
    const steeringSends = () =>
        hostInvoke.mock.calls.filter(([cmd]) => cmd === 'send_extension_tool_steering');
    /** What the window does on config_updated (chat/main.js, floating). */
    const configUpdated = async () => {
        await mgr.onConfigUpdate();
        await mgr.reload();
    };
    return { appConfig, sandbox, mgr, controller, hostInvoke, steeringSends, configUpdated };
}

/** Let the debounce elapse and the resulting send finish. */
async function settle() {
    await vi.advanceTimersByTimeAsync(STEERING_REFRESH_DEBOUNCE_MS + 10);
}

describe('ExtensionManager.onToolDefinitionsChanged', () => {
    it('fires when getTools() answers differently, not on an unchanged refresh', async () => {
        const { mgr, sandbox } = makeHarness();
        await mgr.getToolDefinitions();
        const listener = vi.fn();
        mgr.onToolDefinitionsChanged(listener);

        await mgr.getToolDefinitions();
        expect(listener).not.toHaveBeenCalled();

        sandbox.config = { allowAgentTools: true };
        await mgr.getToolDefinitions();
        expect(listener).toHaveBeenCalledTimes(1);
        expect(listener.mock.calls[0][0][0].tools).toEqual([QUOTE_TOOL]);

        await mgr.getToolDefinitions();
        expect(listener).toHaveBeenCalledTimes(1);
    });

    it('a slow stale result cannot overwrite a newer one', async () => {
        const { mgr, sandbox } = makeHarness();
        let releaseSlow;
        const slowGate = new Promise((r) => {
            releaseSlow = r;
        });
        sandbox.call.mockImplementationOnce(async () => {
            await slowGate;
            return [];
        });
        const slow = mgr.getToolDefinitions(); // started first, answers "no tools"
        sandbox.config = { allowAgentTools: true };
        await mgr.getToolDefinitions(); // started second, sees the tool
        releaseSlow();
        await slow;
        expect(mgr.getToolDefinitionsCached()[0]?.tools).toEqual([QUOTE_TOOL]);
    });

    it('reload() refreshes the definitions so a disabled extension drops out', async () => {
        const { mgr, appConfig } = makeHarness({ allowAgentTools: true });
        await mgr.getToolDefinitions();
        const listener = vi.fn();
        mgr.onToolDefinitionsChanged(listener);

        appConfig.extension_states.stocks = false;
        await mgr.reload();
        expect(listener).toHaveBeenCalledTimes(1);
        expect(listener.mock.calls[0][0]).toEqual([]);
    });

    it('unsubscribe stops notifications', async () => {
        const { mgr, sandbox } = makeHarness();
        const listener = vi.fn();
        const off = mgr.onToolDefinitionsChanged(listener);
        off();
        sandbox.config = { allowAgentTools: true };
        await mgr.getToolDefinitions();
        expect(listener).not.toHaveBeenCalled();
    });
});

describe('extension tool steering follows tool changes', () => {
    beforeEach(() => {
        vi.useFakeTimers();
    });
    afterEach(() => {
        vi.useRealTimers();
    });

    it('setting toggled on → steering re-sent once; saved again unchanged → not re-sent', async () => {
        const h = makeHarness({ allowAgentTools: false });
        await h.mgr.getToolDefinitions(); // initialize() primes the cache
        await h.controller.startSteering();
        // No tools yet: the window publishes that once ("no tools" is never
        // sent to a session that had none), so the backend doesn't steer
        // its sessions with another window's tools.
        expect(h.steeringSends()).toHaveLength(1);
        expect(h.steeringSends()[0][1]).toEqual({ toolSteering: '' });

        h.appConfig.extensions.stocks.allowAgentTools = true;
        await h.configUpdated();
        await settle();
        expect(h.steeringSends()).toHaveLength(2);
        const [, args] = h.steeringSends()[1];
        expect(args).toEqual({ toolSteering: expect.stringContaining('get_quote') });
        expect(args.toolSteering).toContain('replaces any extension tool list');

        // Another config save that leaves the tools as they were (or the
        // same toggle applied again).
        await h.configUpdated();
        await settle();
        expect(h.steeringSends()).toHaveLength(2);
    });

    it('setting toggled off → an empty block is sent once (backend withdraws the tools)', async () => {
        const h = makeHarness({ allowAgentTools: true });
        await h.mgr.getToolDefinitions();
        await h.controller.startSteering();
        expect(h.steeringSends()).toHaveLength(1);

        h.appConfig.extensions.stocks.allowAgentTools = false;
        await h.configUpdated();
        await settle();
        expect(h.steeringSends()).toHaveLength(2);
        expect(h.steeringSends()[1][1].toolSteering).toBe('');

        await h.configUpdated();
        await settle();
        expect(h.steeringSends()).toHaveLength(2);
    });

    it('on then off within the quiet period sends nothing', async () => {
        const h = makeHarness({ allowAgentTools: false });
        await h.mgr.getToolDefinitions();
        await h.controller.startSteering();
        expect(h.steeringSends()).toHaveLength(1); // the initial "no tools"

        h.appConfig.extensions.stocks.allowAgentTools = true;
        await h.configUpdated();
        h.appConfig.extensions.stocks.allowAgentTools = false;
        await h.configUpdated();
        await settle();
        expect(h.steeringSends()).toHaveLength(1);
    });

    it('a failed send is retried on the next change', async () => {
        const h = makeHarness({ allowAgentTools: false });
        await h.mgr.getToolDefinitions();
        await h.controller.startSteering(); // the initial "no tools"
        h.hostInvoke.mockRejectedValueOnce(new Error('ipc down'));

        h.appConfig.extensions.stocks.allowAgentTools = true;
        await h.configUpdated();
        await settle();
        expect(h.steeringSends()).toHaveLength(2); // failed

        await h.controller.sendSteering(); // any later refresh
        expect(h.steeringSends()).toHaveLength(3);
        expect(h.steeringSends()[2][1].toolSteering).toContain('get_quote');
    });

    it('stopSteering() stops re-sends', async () => {
        const h = makeHarness({ allowAgentTools: false });
        await h.mgr.getToolDefinitions();
        await h.controller.startSteering();
        h.controller.stopSteering();
        expect(h.steeringSends()).toHaveLength(1); // the initial "no tools"

        h.appConfig.extensions.stocks.allowAgentTools = true;
        await h.configUpdated();
        await settle();
        expect(h.steeringSends()).toHaveLength(1);
    });

    it('stopSteering() while a publication is being built publishes nothing', async () => {
        const h = makeHarness({ allowAgentTools: true });
        let release;
        const gate = new Promise((r) => {
            release = r;
        });
        h.sandbox.call.mockImplementationOnce(async () => {
            await gate;
            return [QUOTE_TOOL];
        });
        const started = h.controller.startSteering();
        h.controller.stopSteering();
        release();
        await started;
        expect(h.steeringSends()).toHaveLength(0);
    });

    it('a change queued behind a stopped publication is dropped', async () => {
        const h = makeHarness({ allowAgentTools: false });
        await h.mgr.getToolDefinitions();
        let release;
        h.hostInvoke.mockImplementationOnce(
            () =>
                new Promise((r) => {
                    release = r;
                })
        );
        const started = h.controller.startSteering(); // its invoke hangs
        for (let i = 0; i < 100 && !release; i++) await Promise.resolve();
        expect(h.steeringSends()).toHaveLength(1);
        // A real change, so only the lifetime check can hold it back.
        h.sandbox.config = { allowAgentTools: true };
        h.controller.sendSteering(); // queued rerun
        h.controller.stopSteering();
        release();
        await started;
        expect(h.steeringSends()).toHaveLength(1); // only the one already sent
    });

    it('startSteering() after stopSteering() publishes afresh', async () => {
        const h = makeHarness({ allowAgentTools: false });
        await h.mgr.getToolDefinitions();
        await h.controller.startSteering();
        h.controller.stopSteering();
        await h.controller.startSteering();
        expect(h.steeringSends()).toHaveLength(2);
        expect(h.mgr._toolDefsListeners.size).toBe(1);
    });

    it('startSteering() twice subscribes once', async () => {
        const h = makeHarness({ allowAgentTools: false });
        await h.mgr.getToolDefinitions();
        await h.controller.startSteering();
        await h.controller.startSteering();
        expect(h.mgr._toolDefsListeners.size).toBe(1);
    });
});

describe('ExtensionToolController.sendSteering is single-flight', () => {
    it('overlapping calls coalesce into one trailing rerun, in order', async () => {
        let releaseFirst;
        const gate = new Promise((r) => {
            releaseFirst = r;
        });
        const blocks = ['v1', 'v2'];
        const invoke = vi.fn(async () => {
            if (invoke.mock.calls.length === 1) await gate;
        });
        const c = new ExtensionToolController({
            invoke,
            extensionManager: {
                buildToolSteeringBlock: vi.fn(async () => blocks.shift() ?? 'v2'),
            },
        });
        const p1 = c.startSteering();
        const p2 = c.sendSteering();
        const p3 = c.sendSteering();
        releaseFirst();
        await Promise.all([p1, p2, p3]);
        expect(invoke.mock.calls.map(([, a]) => a.toolSteering)).toEqual(['v1', 'v2']);
    });
});

describe('steering can not be driven by a misbehaving extension', () => {
    beforeEach(() => {
        vi.useFakeTimers();
    });
    afterEach(() => {
        vi.useRealTimers();
    });

    it('a getTools() that answers differently on every call does not loop', async () => {
        const h = makeHarness({ allowAgentTools: true });
        let n = 0;
        h.sandbox.call.mockImplementation(async (method) =>
            method === 'getTools' ? [{ ...QUOTE_TOOL, description: `price now ${n++}` }] : undefined
        );
        await h.mgr.getToolDefinitions();
        await h.controller.startSteering();
        const sendsAfterStart = h.steeringSends().length;
        const callsAfterStart = n;
        for (let i = 0; i < 10; i++) await settle();
        // At most the one cached re-render of the start-up fetch's change;
        // no further getTools() calls and no stream of agent turns.
        expect(h.steeringSends().length - sendsAfterStart).toBeLessThanOrEqual(1);
        expect(n).toBe(callsAfterStart);
    });

    it('cyclic or BigInt tool data neither throws nor polls forever', async () => {
        const h = makeHarness({ allowAgentTools: true });
        const tool = { name: 'q', description: 'd', parameters: { n: { type: 'bigint', default: 10n } } };
        tool.self = tool;
        h.sandbox.call.mockImplementation(async (method) => (method === 'getTools' ? [tool] : undefined));
        await h.mgr.getToolDefinitions();
        await h.controller.startSteering();
        const calls = h.sandbox.call.mock.calls.length;
        for (let i = 0; i < 10; i++) await settle();
        expect(h.sandbox.call.mock.calls.length).toBe(calls);
        expect(h.steeringSends()).toHaveLength(1);
        expect(h.steeringSends()[0][1].toolSteering).toContain('n (bigint, default: 10)');
    });

    it('a failed getTools() keeps the last tools instead of withdrawing them', async () => {
        const h = makeHarness({ allowAgentTools: true });
        await h.mgr.getToolDefinitions();
        await h.controller.startSteering();
        expect(h.steeringSends()).toHaveLength(1);
        const listener = vi.fn();
        h.mgr.onToolDefinitionsChanged(listener);

        h.sandbox.call.mockRejectedValue(new Error('RPC timeout'));
        await h.configUpdated();
        await h.controller.sendSteering(); // a fresh fetch fails too
        await settle();

        expect(h.mgr.getToolDefinitionsCached()[0]?.tools).toEqual([QUOTE_TOOL]);
        expect(listener).not.toHaveBeenCalled();
        expect(h.steeringSends()).toHaveLength(1);
    });
});

describe('formatToolSteeringBlock', () => {
    it('is empty without tools and tolerates malformed sandbox data', () => {
        expect(formatToolSteeringBlock([])).toBe('');
        expect(formatToolSteeringBlock(null)).toBe('');
        expect(formatToolSteeringBlock([{ extensionId: 'x', tools: [null, 3] }])).toBe('');
        const block = formatToolSteeringBlock([
            {
                extensionId: 'x',
                extensionName: 'X',
                extensionIcon: '🧩',
                tools: [null, { name: 't', description: 'does t', parameters: { a: null } }],
            },
        ]);
        expect(block).toContain('  - t: does t');
        expect(block).toContain('a (undefined)');
    });
});

describe('formatToolParam', () => {
    it('renders plain declarations exactly as before', () => {
        expect(
            formatToolParam('limit', { type: 'number', description: 'Max items', default: 10 })
        ).toBe('limit (number, default: 10) — Max items');
        expect(formatToolParam('symbol', { type: 'string', description: 'Ticker' })).toBe(
            'symbol (string) — Ticker'
        );
        expect(formatToolParam('flag', { type: 'boolean', default: false })).toBe(
            'flag (boolean, default: false)'
        );
        expect(formatToolParam('a', null)).toBe('a (undefined)');
        expect(formatToolParam('b', 'string')).toBe('b (undefined)');
    });

    it('lists enum values', () => {
        expect(
            formatToolParam('range', {
                type: 'string',
                enum: ['1d', '5d', '1mo', '1y'],
                default: '1d',
                description: 'Chart range',
            })
        ).toBe('range (string, one of: 1d|5d|1mo|1y, default: 1d) — Chart range');
        expect(formatToolParam('n', { type: 'number', enum: [1, 2.5, -3] })).toBe(
            'n (number, one of: 1|2.5|-3)'
        );
    });

    it('shows the item type and item values of an array', () => {
        expect(
            formatToolParam('kinds', {
                type: 'array',
                items: { type: 'string', enum: ['note', 'link', 'doc', 'action', 'meeting'] },
                description: 'Only these kinds',
            })
        ).toBe('kinds (array of string, one of: note|link|doc|action|meeting) — Only these kinds');
        expect(formatToolParam('terms', { type: 'array', items: { type: 'string' } })).toBe(
            'terms (array of string)'
        );
        // Values put on the array instead of its items still reach the agent.
        expect(formatToolParam('tags', { type: 'array', enum: ['a', 'b'] })).toBe(
            'tags (array, one of: a|b)'
        );
        // items on a non-array type is ignored.
        expect(formatToolParam('q', { type: 'string', items: { type: 'number' } })).toBe(
            'q (string)'
        );
    });

    it('marks required parameters, only for a literal true', () => {
        expect(
            formatToolParam('query', {
                type: 'string',
                required: true,
                description: 'The question, verbatim',
            })
        ).toBe('query (string, required) — The question, verbatim');
        expect(formatToolParam('k', { type: 'string', required: 'yes' })).toBe('k (string)');
        expect(
            formatToolParam('kinds', {
                type: 'array',
                items: { type: 'string', enum: ['note', 'doc'] },
                required: true,
                default: 'note',
            })
        ).toBe('kinds (array of string, one of: note|doc, required, default: note)');
    });

    it('falls back to the plain form for malformed declarations, never throwing', () => {
        const cases = [
            [{ type: 'string', enum: 'a|b' }, 'x (string)'],
            [{ type: 'string', enum: [] }, 'x (string)'],
            [{ type: 'string', enum: ['a', { b: 1 }] }, 'x (string)'],
            [{ type: 'number', enum: [1, Number.NaN] }, 'x (number)'],
            [{ type: 'string', enum: ['a', null] }, 'x (string)'],
            [{ type: 'string', enum: [, 'a'] }, 'x (string)'], // sparse
            [{ type: 'array', items: null }, 'x (array)'],
            [{ type: 'array', items: 'string' }, 'x (array)'],
            [{ type: 'array', items: { type: { toString: 1 } } }, 'x (array)'],
            [{ type: 'array', items: { type: 'string', enum: 7 } }, 'x (array of string)'],
            [{ type: 7 }, 'x (7)'],
        ];
        for (const [spec, want] of cases) {
            expect(formatToolParam('x', spec)).toBe(want);
        }
        const hostile = structuredClone({ type: 'array', items: { type: 'string', enum: ['a'] } });
        Object.defineProperty(hostile, 'default', { value: { toString: 1, valueOf: 1 } });
        expect(() => formatToolParam('x', hostile)).not.toThrow();
    });

    it('caps long enums and counts the rest', () => {
        const values = Array.from({ length: 20 }, (_, i) => `v${i}`);
        expect(formatToolParam('x', { type: 'string', enum: values })).toBe(
            `x (string, one of: ${values.slice(0, 12).join('|')}|… (+8 more))`
        );
        const twelve = values.slice(0, 12);
        expect(formatToolParam('x', { type: 'string', enum: twelve })).toBe(
            `x (string, one of: ${twelve.join('|')})`
        );
    });

    it('quotes values that would blur the list', () => {
        expect(
            formatToolParam('x', { type: 'string', enum: ['a b', '', 'x|y', 'p(q)', 'ok', 'last week'] })
        ).toBe('x (string, one of: "a b"|""|"x|y"|"p(q)"|ok|"last week")');
    });

    it('is what the steering block prints for each parameter', () => {
        const block = formatToolSteeringBlock([
            {
                extensionId: 'mindmap',
                extensionName: 'Mind Map',
                extensionIcon: '🧠',
                tools: [
                    {
                        name: 'search',
                        description: 'Search saved notes',
                        parameters: {
                            query: { type: 'string', required: true, description: 'Verbatim' },
                            kinds: {
                                type: 'array',
                                items: { type: 'string', enum: ['note', 'doc'] },
                            },
                            limit: { type: 'number', default: 8 },
                        },
                    },
                ],
            },
        ]);
        expect(block).toContain(
            '    Parameters: query (string, required) — Verbatim; ' +
                'kinds (array of string, one of: note|doc); limit (number, default: 8)\n'
        );
    });
});

describe('tool definitions stay usable with hostile or churning extensions', () => {
    it('unprintable sandbox values neither throw nor block tool calls', async () => {
        const h = makeHarness({ allowAgentTools: true });
        const hostile = structuredClone({ name: { toString: 1 }, description: 'd' });
        h.sandbox.call.mockImplementation(async (method) =>
            method === 'getTools' ? [hostile, QUOTE_TOOL] : undefined
        );
        expect(() => formatToolSteeringBlock([{ extensionId: 'x', tools: [hostile] }])).not.toThrow();
        await expect(h.mgr.getToolDefinitions()).resolves.toHaveLength(1);
        const block = await h.mgr.buildToolSteeringBlock();
        expect(block).toContain('get_quote');
    });

    it('order of loading does not change the steering text', async () => {
        const a = makeHarness({ allowAgentTools: true });
        const other = stocksSandbox();
        other.config = { allowAgentTools: true };
        a.mgr.extensions.set('aaa', {
            manifest: { id: 'aaa', name: 'A', version: '1', icon: '🅰️' },
            capabilities: [],
            sandbox: other,
        });
        a.appConfig.extension_states.aaa = true;
        a.mgr._configCache.extension_states.aaa = true;
        const before = await a.mgr.buildToolSteeringBlock();
        // Reload moves 'stocks' to the end of the Map.
        const entry = a.mgr.extensions.get('stocks');
        a.mgr.extensions.delete('stocks');
        a.mgr.extensions.set('stocks', entry);
        expect(await a.mgr.buildToolSteeringBlock()).toBe(before);
    });

    it('keeps last tools for a few failures, not for an unloaded extension or forever', async () => {
        const h = makeHarness({ allowAgentTools: true });
        await h.mgr.getToolDefinitions();
        h.sandbox.call.mockRejectedValue(new Error('busy'));
        for (let i = 0; i < 5; i++) {
            expect(await h.mgr.getToolDefinitions()).toHaveLength(1);
        }
        expect(await h.mgr.getToolDefinitions()).toHaveLength(0);

        const g = makeHarness({ allowAgentTools: true });
        await g.mgr.getToolDefinitions();
        g.sandbox.call.mockImplementation(async () => {
            g.mgr.extensions.delete('stocks'); // unloaded mid-call
            throw new Error('sandbox destroyed');
        });
        expect(await g.mgr.getToolDefinitions()).toHaveLength(0);
    });
});
