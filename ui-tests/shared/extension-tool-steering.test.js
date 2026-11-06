/**
 * Extension tool steering stays in sync with the extension tools after
 * startup: a setting that turns an extension's agent tools on (Stocks'
 * "Allow tools for agents"), an extension enabled, installed, disabled or
 * removed. The steering must be re-sent once per real change and never for a
 * refresh that found the same tools.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
    ExtensionManager,
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
        expect(h.steeringSends()).toHaveLength(0); // no tools yet: nothing to say

        h.appConfig.extensions.stocks.allowAgentTools = true;
        await h.configUpdated();
        await settle();
        expect(h.steeringSends()).toHaveLength(1);
        const [, args] = h.steeringSends()[0];
        expect(args.sessionId).toBe('sess-1');
        expect(args.toolSteering).toContain('get_quote');
        expect(args.toolSteering).toContain('replaces any extension tool list');

        // Another config save that leaves the tools as they were (or the
        // same toggle applied again).
        await h.configUpdated();
        await settle();
        expect(h.steeringSends()).toHaveLength(1);
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

        h.appConfig.extensions.stocks.allowAgentTools = true;
        await h.configUpdated();
        h.appConfig.extensions.stocks.allowAgentTools = false;
        await h.configUpdated();
        await settle();
        expect(h.steeringSends()).toHaveLength(0);
    });

    it('a failed send is retried on the next change', async () => {
        const h = makeHarness({ allowAgentTools: false });
        await h.mgr.getToolDefinitions();
        await h.controller.startSteering();
        h.hostInvoke.mockRejectedValueOnce(new Error('ipc down'));

        h.appConfig.extensions.stocks.allowAgentTools = true;
        await h.configUpdated();
        await settle();
        expect(h.steeringSends()).toHaveLength(1); // failed

        await h.controller.sendSteering(); // any later refresh
        expect(h.steeringSends()).toHaveLength(2);
        expect(h.steeringSends()[1][1].toolSteering).toContain('get_quote');
    });

    it('stopSteering() stops re-sends', async () => {
        const h = makeHarness({ allowAgentTools: false });
        await h.mgr.getToolDefinitions();
        await h.controller.startSteering();
        h.controller.stopSteering();

        h.appConfig.extensions.stocks.allowAgentTools = true;
        await h.configUpdated();
        await settle();
        expect(h.steeringSends()).toHaveLength(0);
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
            getSessionId: () => 's',
            extensionManager: {
                buildToolSteeringBlock: vi.fn(async () => blocks.shift() ?? 'v2'),
            },
        });
        const p1 = c.sendSteering();
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
