/**
 * Background agent calls at the sandbox bridge
 * (docs/design/EXTENSION_AGENT_CALLS.md §3.8, §12): the identity the host
 * injects, `when: 'now'` and the click lease, call tracking and the cancel
 * on destroy, and the 64 KB pre-check before `agent_call_send`.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { decideInvoke } from '../../ui/js/shared/extension-permissions.js';
import {
    AGENT_SEND_MAX_JSON_CHARS,
    agentCallId,
    agentReserveArgs,
    agentSendRefusal,
    ExtensionSandbox,
    IDENTITY_SCOPED_COMMANDS,
} from '../../ui/js/shared/extension-sandbox-host.js';

const ALL = [
    'agent_call_reserve',
    'agent_call_ready',
    'agent_call_send',
    'agent_call_cancel',
    'agent_call_status',
];

const live = [];

afterEach(() => {
    for (const sb of live.splice(0)) sb.destroy();
    vi.useRealTimers();
    vi.restoreAllMocks();
});

function deferred() {
    let resolve;
    let reject;
    const promise = new Promise((res, rej) => {
        resolve = res;
        reject = rej;
    });
    return { promise, resolve, reject };
}

/** An AppError as the frontend sees it. */
function appError(kind, key) {
    return Object.assign(new Error(key), { kind, key: `errors.agent_call.${key}` });
}

/**
 * A sandbox on a fake port. `answers[command]` is a function of the args,
 * a value, or an Error to throw. Reservations get ids c1, c2, …
 */
function agentSandbox({ capabilities = ['agent_background'], answers = {} } = {}) {
    let n = 0;
    const rawInvoke = vi.fn(async (command, args) => {
        const a = answers[command];
        if (typeof a === 'function') return a(args);
        if (a instanceof Error) throw a;
        if (a !== undefined) return a;
        if (command === 'agent_call_reserve') {
            n += 1;
            return { call_id: `c${n}`, position: 1 };
        }
        return null;
    });
    const sb = new ExtensionSandbox(
        { extensionId: 'mindmap', capabilities, config: {}, sources: {} },
        rawInvoke,
        document.body
    );
    live.push(sb);
    const sent = [];
    sb._port = { postMessage: (m) => sent.push(m), close: () => {} };
    sb._ready = true;
    let id = 0;
    const inv = (command, args) => {
        id += 1;
        return sb._handleInvoke({ id, command, args });
    };
    const cancels = () =>
        rawInvoke.mock.calls.filter(([c]) => c === 'agent_call_cancel').map(([, a]) => a);
    const whens = () =>
        rawInvoke.mock.calls.filter(([c]) => c === 'agent_call_reserve').map(([, a]) => a.args.when);
    // An RPC the host sends because of a click (EXTENSION_FILE_TEXT §5.2).
    const click = () => sb.call('execute', {}, { userGesture: true, timeoutMs: 0 }).catch(() => {});
    return { sb, sent, rawInvoke, inv, cancels, whens, click };
}

const cancelOf = (callId) => ({ extensionId: 'mindmap', args: { call_id: callId } });

describe('identity and grant', () => {
    it('force-injects the calling extension for all five commands', async () => {
        const { rawInvoke, inv } = agentSandbox();
        for (const command of ALL) {
            await inv(command, { extensionId: 'other', args: { call_id: 'x', purpose: 'organize' } });
        }
        expect(rawInvoke).toHaveBeenCalledTimes(ALL.length);
        for (const [command, args] of rawInvoke.mock.calls) {
            expect(args.extensionId, command).toBe('mindmap');
        }
        for (const command of ALL) expect(IDENTITY_SCOPED_COMMANDS[command]).toBe('extensionId');
    });

    it('needs the agent_background grant, and never runs in a view', async () => {
        const { rawInvoke, sent, inv } = agentSandbox({ capabilities: ['agent', 'storage'] });
        for (const command of ALL) await inv(command, { args: {} });
        expect(rawInvoke).not.toHaveBeenCalled();
        expect(sent).toHaveLength(ALL.length);
        for (const msg of sent) expect(msg.error).toMatch(/missing capability 'agent_background'/);
        for (const command of ALL) {
            const held = new Set(['agent_background']);
            expect(decideInvoke(command, held, { surface: 'view' }).allow, command).toBe(false);
        }
    });
});

describe("when: 'now' and the click lease", () => {
    it("goes as 'idle' without a click", async () => {
        const { rawInvoke, inv } = agentSandbox();
        await inv('agent_call_reserve', {
            args: { purpose: 'organize', when: 'now', dedupe_key: 'k' },
        });
        expect(rawInvoke).toHaveBeenLastCalledWith('agent_call_reserve', {
            extensionId: 'mindmap',
            args: { purpose: 'organize', when: 'idle', dedupe_key: 'k' },
        });
    });

    it('passes once per click, and leaves the lease for a file dialog', async () => {
        const { sb, inv, whens, click } = agentSandbox();
        click();
        await inv('agent_call_reserve', { args: { purpose: 'organize', when: 'now' } });
        expect(sb.hasClickLease()).toBe(true);
        // A second reservation from the same click waits for idle.
        await inv('agent_call_reserve', { args: { purpose: 'organize', when: 'now' } });
        click();
        await inv('agent_call_reserve', { args: { purpose: 'organize', when: 'now' } });
        // An idle call never takes the click's turn.
        click();
        await inv('agent_call_reserve', { args: { purpose: 'organize' } });
        await inv('agent_call_reserve', { args: { purpose: 'organize', when: 'now' } });
        expect(whens()).toEqual(['now', 'idle', 'now', 'idle', 'now']);
    });

    it("a refused reservation leaves the click's one unused", async () => {
        let first = true;
        const { inv, whens, click } = agentSandbox({
            answers: {
                agent_call_reserve: () => {
                    if (!first) return { call_id: 'c9', position: 1 };
                    first = false;
                    throw appError('rate_limited', 'duplicate');
                },
            },
        });
        click();
        await inv('agent_call_reserve', { args: { purpose: 'organize', when: 'now' } });
        await inv('agent_call_reserve', { args: { purpose: 'organize', when: 'now' } });
        expect(whens()).toEqual(['now', 'now']);
    });

    it('a lease ends after 10 s, and an RPC without a gesture grants none', async () => {
        vi.useFakeTimers();
        const { sb, inv, whens, click } = agentSandbox();
        sb.call('match', {}, { timeoutMs: 0 }).catch(() => {});
        await inv('agent_call_reserve', { args: { purpose: 'organize', when: 'now' } });
        click();
        vi.advanceTimersByTime(10_001);
        await inv('agent_call_reserve', { args: { purpose: 'organize', when: 'now' } });
        expect(whens()).toEqual(['idle', 'idle']);
    });

    it('rebuilds the args, so no other shape reaches serde as now', async () => {
        const { rawInvoke, inv, click } = agentSandbox();
        const last = () => rawInvoke.mock.calls.at(-1)[1].args;
        click();
        await inv('agent_call_reserve', {
            args: { purpose: 'organize', when: { now: null }, extra: 1 },
        });
        expect(last()).toEqual({ purpose: 'organize', when: 'idle' });
        await inv('agent_call_reserve', { args: ['organize', 'now'] });
        expect(last()).toEqual({ purpose: undefined, when: 'idle' });
        await inv('agent_call_reserve', { args: { purpose: 'organize', dedupeKey: 'x' } });
        expect(last()).toEqual({ purpose: 'organize', when: 'idle', dedupe_key: 'x' });
        expect(agentReserveArgs(null, true)).toEqual({ purpose: undefined, when: 'now' });
        expect(agentReserveArgs({ purpose: 'p', when: 'now' }, 'yes')).toEqual({
            purpose: 'p',
            when: 'idle',
        });
    });
});

describe('call tracking', () => {
    it('destroy() cancels the calls the extension still holds, as that extension', async () => {
        const { sb, inv, cancels } = agentSandbox();
        await inv('agent_call_reserve', { args: { purpose: 'organize' } });
        await inv('agent_call_reserve', { args: { purpose: 'organize' } });
        expect([...sb._agentCalls]).toEqual(['c1', 'c2']);
        sb.destroy();
        expect(cancels()).toEqual([cancelOf('c1'), cancelOf('c2')]);
        sb.destroy();
        expect(cancels()).toHaveLength(2);
    });

    it('drops a call once its send is answered, its turn refused, or it is cancelled', async () => {
        const { sb, inv, cancels } = agentSandbox({
            answers: {
                agent_call_ready: (a) => {
                    if (a.args.call_id === 'c2') throw appError('not_permitted', 'consent_stale');
                    return { call_id: a.args.call_id, send_by: '2026-11-09T10:00:10Z' };
                },
                agent_call_send: (a) => ({ call_id: a.args.call_id, text: 'ok' }),
            },
        });
        for (let i = 0; i < 3; i++) await inv('agent_call_reserve', { args: { purpose: 'organize' } });
        await inv('agent_call_ready', { args: { call_id: 'c1' } });
        expect(sb._agentCalls.has('c1')).toBe(true);
        await inv('agent_call_send', { args: { call_id: 'c1', request: { instructions: 'x' } } });
        await inv('agent_call_ready', { args: { call_id: 'c2' } });
        await inv('agent_call_cancel', { args: { call_id: 'c3' } });
        expect([...sb._agentCalls]).toEqual([]);
        sb.destroy();
        // Only the extension's own cancel went out.
        expect(cancels()).toEqual([cancelOf('c3')]);
    });

    it('keeps a call sent before its turn, and drops one Rust finished', async () => {
        const { sb, inv } = agentSandbox({
            answers: {
                agent_call_send: (a) => {
                    const key = a.args.call_id === 'c1' ? 'not_ready' : 'invalid_response';
                    throw appError('internal', key);
                },
            },
        });
        await inv('agent_call_reserve', { args: { purpose: 'organize' } });
        await inv('agent_call_reserve', { args: { purpose: 'organize' } });
        await inv('agent_call_send', { args: { call_id: 'c1', request: { instructions: 'x' } } });
        await inv('agent_call_send', { args: { call_id: 'c2', request: { instructions: 'x' } } });
        expect([...sb._agentCalls]).toEqual(['c1']);
    });

    it('ends a call itself when the IPC refused a send Rust never saw', async () => {
        const { sb, sent, inv, cancels } = agentSandbox({
            answers: {
                agent_call_send: new Error('invalid args `args` for command `agent_call_send`'),
            },
        });
        await inv('agent_call_reserve', { args: { purpose: 'organize' } });
        await inv('agent_call_send', { args: { call_id: 'c1', request: {} } });
        expect(sent.at(-1).error).toMatch(/invalid args/);
        expect(cancels()).toEqual([cancelOf('c1')]);
        expect(sb._agentCalls.size).toBe(0);
    });

    it('never cancels an id it did not reserve', async () => {
        const { inv, cancels } = agentSandbox({ answers: { agent_call_send: new Error('nope') } });
        await inv('agent_call_send', { args: { call_id: 'someone-else', request: {} } });
        expect(cancels()).toEqual([]);
    });

    it('cancels a reservation that lands after destroy()', async () => {
        const later = deferred();
        const { sb, sent, inv, cancels } = agentSandbox({
            answers: { agent_call_reserve: () => later.promise },
        });
        const pending = inv('agent_call_reserve', { args: { purpose: 'organize' } });
        sb.destroy();
        later.resolve({ call_id: 'late', position: 1 });
        await pending;
        expect(cancels()).toEqual([cancelOf('late')]);
        // The extension never hears of the id.
        expect(sent.filter((m) => m.type === 'invoke-response')).toEqual([]);
    });

    it('cancels every sandbox’s calls when the page goes away', async () => {
        const { sb, inv, cancels } = agentSandbox();
        await inv('agent_call_reserve', { args: { purpose: 'organize' } });
        window.dispatchEvent(new Event('pagehide'));
        expect(cancels()).toEqual([cancelOf('c1')]);
        expect(sb._agentCalls.size).toBe(0);
    });

    it('reads the call id from call_id or callId', () => {
        expect(agentCallId({ args: { call_id: 'a' } })).toBe('a');
        expect(agentCallId({ args: { callId: 'b' } })).toBe('b');
        expect(agentCallId({ args: { call_id: 3 } })).toBeNull();
        expect(agentCallId({ args: 'x' })).toBeNull();
        expect(agentCallId(undefined)).toBeNull();
    });
});

describe('the 64 KB pre-check', () => {
    it('refuses a bigger send before Rust sees it, and ends its turn', async () => {
        const { sb, sent, rawInvoke, inv, cancels } = agentSandbox();
        await inv('agent_call_reserve', { args: { purpose: 'organize' } });
        const request = { instructions: 'x', input: 'y'.repeat(70_000) };
        await inv('agent_call_send', { args: { call_id: 'c1', request } });
        expect(rawInvoke.mock.calls.some(([c]) => c === 'agent_call_send')).toBe(false);
        expect(sent.at(-1)).toMatchObject({
            type: 'invoke-response',
            errorKind: 'invalid_input',
            errorKey: 'errors.agent_call.prompt_too_large',
        });
        expect(cancels()).toEqual([cancelOf('c1')]);
        expect(sb._agentCalls.size).toBe(0);
    });

    it('forwards a send at the limit, counting characters, not bytes', async () => {
        const { rawInvoke, inv } = agentSandbox();
        const wrap = (input) => ({ args: { call_id: 'c1', request: { instructions: '', input } } });
        const shell = JSON.stringify({ ...wrap(''), extensionId: 'mindmap' }).length;
        // Three bytes a character in UTF-8, one code unit here.
        const input = '漢'.repeat(AGENT_SEND_MAX_JSON_CHARS - shell);
        await inv('agent_call_send', wrap(input));
        expect(rawInvoke.mock.calls.filter(([c]) => c === 'agent_call_send')).toHaveLength(1);
        const over = { ...wrap(`${input}x`), extensionId: 'mindmap' };
        expect(agentSendRefusal(over)).toMatchObject({
            key: 'errors.agent_call.prompt_too_large',
            vars: { size: AGENT_SEND_MAX_JSON_CHARS + 1, limit: AGENT_SEND_MAX_JSON_CHARS },
        });
    });

    it('refuses a send that cannot be JSON', () => {
        expect(agentSendRefusal({ args: { request: { input: 1n } } })).toMatchObject({
            kind: 'invalid_input',
            key: 'errors.agent_call.field_invalid',
            vars: { field: 'request' },
        });
        expect(agentSendRefusal({ args: { request: { instructions: 'ok' } } })).toBeNull();
    });
});
