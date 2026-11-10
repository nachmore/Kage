/**
 * `context.agent` (docs/design/EXTENSION_AGENT_CALLS.md §3.4, §12): the
 * call sequence over the `agent_call_*` commands, a builder that gives
 * the turn back, an abort at each stage, the one retry after user
 * activity, errors that keep `kind` and `key`, and `context.agent` only
 * with the `agent.background` host feature.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import {
    agentWireRequest,
    createAgentApi,
} from '../../ui/js/extension-sandbox/runtime/agent-api.js';
import {
    HOST_FEATURE_AGENT_BACKGROUND,
    RUNTIME_SOURCE_PATHS,
} from '../../ui/js/shared/extension-sandbox-host.js';

const UI_ROOT = resolve(__dirname, '../../ui');

function deferred() {
    let resolve;
    let reject;
    const promise = new Promise((res, rej) => {
        resolve = res;
        reject = rej;
    });
    return { promise, resolve, reject };
}

/** An AppError as `invoke()` rejects with it. */
function appError(kind, key) {
    return Object.assign(new Error(`message for ${key}`), {
        kind,
        key: `errors.agent_call.${key}`,
    });
}

const REPLY = {
    call_id: 'c1',
    text: '{"ok":true}',
    json: { ok: true },
    prompt_chars: 120,
    response_chars: 11,
    duration_ms: 900,
    redactions: 1,
};

/**
 * A fake `invoke`. `answers[command]` is a function of the args (sync or
 * async), a value, or an Error to throw. Reservations get c1, c2, …
 */
function makeApi(answers = {}) {
    let n = 0;
    const calls = [];
    const invoke = vi.fn(async (command, args) => {
        calls.push([command, args]);
        const a = answers[command];
        if (typeof a === 'function') return a(args);
        if (a instanceof Error) throw a;
        if (a !== undefined) return a;
        switch (command) {
            case 'agent_call_reserve':
                n += 1;
                return { call_id: `c${n}`, position: 1 };
            case 'agent_call_ready':
                return { call_id: args.args.call_id, send_by: '2026-11-09T10:00:10Z' };
            case 'agent_call_send':
                return { ...REPLY, call_id: args.args.call_id };
            default:
                return null;
        }
    });
    const agent = createAgentApi({ invoke });
    const commands = () => calls.map(([c]) => c);
    const cancels = () => calls.filter(([c]) => c === 'agent_call_cancel').map(([, a]) => a);
    return { agent, invoke, calls, commands, cancels };
}

const REQUEST = { label: 'Organize 2 notes', instructions: 'Sort these.', input: { notes: [] } };

describe('context.agent.complete', () => {
    it('reserves, waits for the turn, builds, sends, and returns the reply', async () => {
        const order = [];
        const { agent, calls } = makeApi({
            agent_call_ready: (a) => {
                order.push('ready');
                return { call_id: a.args.call_id, send_by: 'soon' };
            },
        });
        const build = vi.fn(() => {
            order.push('build');
            return { ...REQUEST, maxResponseChars: 4000, timeoutMs: 30_000 };
        });
        const result = await agent.complete(build, {
            purpose: 'organize',
            dedupeKey: 'organize',
        });
        expect(order).toEqual(['ready', 'build']);
        expect(calls).toEqual([
            [
                'agent_call_reserve',
                { args: { purpose: 'organize', when: 'idle', dedupe_key: 'organize' } },
            ],
            ['agent_call_ready', { args: { call_id: 'c1' } }],
            [
                'agent_call_send',
                {
                    args: {
                        call_id: 'c1',
                        request: {
                            label: 'Organize 2 notes',
                            instructions: 'Sort these.',
                            input: { notes: [] },
                            max_response_chars: 4000,
                            timeout_ms: 30_000,
                        },
                    },
                },
            ],
        ]);
        expect(result).toEqual({
            callId: 'c1',
            text: '{"ok":true}',
            json: { ok: true },
            promptChars: 120,
            responseChars: 11,
            durationMs: 900,
            redactions: 1,
        });
    });

    it('takes a request object, an async builder, and when: now', async () => {
        const { agent, calls } = makeApi();
        await agent.complete(REQUEST, { purpose: 'organize', when: 'now' });
        expect(calls[0][1]).toEqual({ args: { purpose: 'organize', when: 'now' } });
        expect(calls[2][1].args.request).toEqual(REQUEST);
        await agent.complete(async () => REQUEST, { purpose: 'organize' });
        expect(calls.at(-1)[1].args.request).toEqual(REQUEST);
    });

    it('a builder returning null gives the turn back and resolves null', async () => {
        for (const nothing of [null, undefined]) {
            const { agent, commands, cancels } = makeApi();
            expect(await agent.complete(() => nothing, { purpose: 'organize' })).toBeNull();
            expect(commands()).toEqual([
                'agent_call_reserve',
                'agent_call_ready',
                'agent_call_cancel',
            ]);
            expect(cancels()).toEqual([{ args: { call_id: 'c1' } }]);
        }
    });

    it('a builder that throws, or builds a bad request, cancels and rejects', async () => {
        const boom = new Error('store not loaded');
        const a = makeApi();
        await expect(
            a.agent.complete(
                () => {
                    throw boom;
                },
                { purpose: 'organize' }
            )
        ).rejects.toBe(boom);
        expect(a.cancels()).toEqual([{ args: { call_id: 'c1' } }]);
        const b = makeApi();
        await expect(
            b.agent.complete(() => ({ input: 'no instructions' }), { purpose: 'organize' })
        ).rejects.toMatchObject({ kind: 'invalid_input' });
        expect(b.commands()).not.toContain('agent_call_send');
        expect(b.cancels()).toEqual([{ args: { call_id: 'c1' } }]);
    });

    it('checks a request object and the options before it reserves', async () => {
        const { agent, invoke } = makeApi();
        const bad = [
            [REQUEST, {}],
            [REQUEST, { purpose: '' }],
            [REQUEST, { purpose: 'organize', when: 'soon' }],
            [REQUEST, { purpose: 'organize', dedupeKey: 3 }],
            [null, { purpose: 'organize' }],
            ['text', { purpose: 'organize' }],
            [{ instructions: 1 }, { purpose: 'organize' }],
            [{ instructions: 'x', label: {} }, { purpose: 'organize' }],
            [{ instructions: 'x', timeoutMs: 'soon' }, { purpose: 'organize' }],
            [{ instructions: 'x', maxResponseChars: Number.NaN }, { purpose: 'organize' }],
        ];
        for (const [build, options] of bad) {
            await expect(agent.complete(build, options)).rejects.toMatchObject({
                kind: 'invalid_input',
            });
        }
        expect(invoke).not.toHaveBeenCalled();
    });

    it('rejects with the AppError kind and key at every stage', async () => {
        const stages = [
            ['agent_call_reserve', appError('rate_limited', 'duplicate')],
            ['agent_call_ready', appError('not_permitted', 'consent_stale')],
            ['agent_call_send', appError('internal', 'invalid_response')],
        ];
        for (const [command, err] of stages) {
            const { agent, cancels } = makeApi({ [command]: err });
            await expect(agent.complete(REQUEST, { purpose: 'organize' })).rejects.toMatchObject({
                kind: err.kind,
                key: err.key,
                message: err.message,
            });
            // The host has already ended the call; nothing to cancel.
            expect(cancels()).toEqual([]);
        }
    });

    it('retries once, in a new reservation, after cancelled_user_activity', async () => {
        let sends = 0;
        const { agent, calls } = makeApi({
            agent_call_send: (a) => {
                sends += 1;
                if (sends === 1) throw appError('cancelled', 'cancelled_user_activity');
                return { ...REPLY, call_id: a.args.call_id };
            },
        });
        const build = vi.fn(() => REQUEST);
        const result = await agent.complete(build, { purpose: 'organize', when: 'now' });
        expect(result.callId).toBe('c2');
        expect(build).toHaveBeenCalledTimes(2);
        const reserves = calls.filter(([c]) => c === 'agent_call_reserve');
        expect(reserves.map(([, a]) => a.args.when)).toEqual(['now', 'now']);
    });

    it('retries only once, and not at all with retryOnActivity: false', async () => {
        const stopped = appError('cancelled', 'cancelled_user_activity');
        const a = makeApi({ agent_call_send: stopped });
        await expect(a.agent.complete(REQUEST, { purpose: 'organize' })).rejects.toBe(stopped);
        expect(a.commands().filter((c) => c === 'agent_call_send')).toHaveLength(2);
        const b = makeApi({ agent_call_send: stopped });
        await expect(
            b.agent.complete(REQUEST, { purpose: 'organize', retryOnActivity: false })
        ).rejects.toBe(stopped);
        expect(b.commands().filter((c) => c === 'agent_call_send')).toHaveLength(1);
        // Other failures are never retried.
        const c = makeApi({ agent_call_send: appError('timeout', 'timeout') });
        await expect(c.agent.complete(REQUEST, { purpose: 'organize' })).rejects.toMatchObject({
            key: 'errors.agent_call.timeout',
        });
        expect(c.commands().filter((x) => x === 'agent_call_send')).toHaveLength(1);
    });
});

describe('context.agent.complete with a signal', () => {
    const CANCELLED = { kind: 'cancelled', key: 'errors.agent_call.cancelled_by_extension' };

    it('an aborted signal sends nothing', async () => {
        const { agent, invoke } = makeApi();
        const ctl = new AbortController();
        ctl.abort();
        await expect(
            agent.complete(REQUEST, { purpose: 'organize', signal: ctl.signal })
        ).rejects.toMatchObject(CANCELLED);
        expect(invoke).not.toHaveBeenCalled();
    });

    it('an abort while reserving cancels the call as soon as its id arrives', async () => {
        const reserved = deferred();
        const { agent, commands, cancels } = makeApi({
            agent_call_reserve: () => reserved.promise,
        });
        const ctl = new AbortController();
        const done = agent.complete(REQUEST, { purpose: 'organize', signal: ctl.signal });
        await Promise.resolve();
        ctl.abort();
        await expect(done).rejects.toMatchObject(CANCELLED);
        expect(cancels()).toEqual([]);
        reserved.resolve({ call_id: 'late', position: 1 });
        await vi.waitFor(() => expect(cancels()).toEqual([{ args: { call_id: 'late' } }]));
        expect(commands()).not.toContain('agent_call_ready');
    });

    it('an abort while waiting for the turn cancels the call', async () => {
        const turn = deferred();
        const { agent, commands, cancels } = makeApi({ agent_call_ready: () => turn.promise });
        const ctl = new AbortController();
        const build = vi.fn(() => REQUEST);
        const done = agent.complete(build, { purpose: 'organize', signal: ctl.signal });
        await vi.waitFor(() => expect(commands()).toContain('agent_call_ready'));
        ctl.abort();
        await expect(done).rejects.toMatchObject(CANCELLED);
        expect(cancels()).toEqual([{ args: { call_id: 'c1' } }]);
        // The host rejects the wait; nothing else happens.
        turn.reject(appError('cancelled', 'cancelled_by_extension'));
        await Promise.resolve();
        expect(build).not.toHaveBeenCalled();
        expect(commands()).not.toContain('agent_call_send');
    });

    it('an abort while building cancels the call and sends nothing', async () => {
        const built = deferred();
        const { agent, commands, cancels } = makeApi();
        const ctl = new AbortController();
        const done = agent.complete(() => built.promise, {
            purpose: 'organize',
            signal: ctl.signal,
        });
        await vi.waitFor(() => expect(commands()).toContain('agent_call_ready'));
        await Promise.resolve();
        ctl.abort();
        await expect(done).rejects.toMatchObject(CANCELLED);
        built.resolve(REQUEST);
        await Promise.resolve();
        expect(cancels()).toEqual([{ args: { call_id: 'c1' } }]);
        expect(commands()).not.toContain('agent_call_send');
    });

    it('an abort while the agent runs cancels it, once, and is not retried', async () => {
        const reply = deferred();
        const { agent, commands, cancels } = makeApi({ agent_call_send: () => reply.promise });
        const ctl = new AbortController();
        const done = agent.complete(REQUEST, { purpose: 'organize', signal: ctl.signal });
        await vi.waitFor(() => expect(commands()).toContain('agent_call_send'));
        ctl.abort();
        ctl.signal.dispatchEvent(new Event('abort'));
        await expect(done).rejects.toMatchObject(CANCELLED);
        reply.reject(appError('cancelled', 'cancelled_user_activity'));
        await Promise.resolve();
        expect(cancels()).toEqual([{ args: { call_id: 'c1' } }]);
        expect(commands().filter((c) => c === 'agent_call_reserve')).toHaveLength(1);
    });

    it('an abort after the reply sends nothing', async () => {
        const { agent, cancels } = makeApi();
        const ctl = new AbortController();
        await agent.complete(REQUEST, { purpose: 'organize', signal: ctl.signal });
        ctl.abort();
        expect(cancels()).toEqual([]);
    });
});

describe('context.agent.status and the wire request', () => {
    it('status() asks agent_call_status with no args of its own', async () => {
        const status = { available: true, access: 'on', machine_tag: 'abc' };
        const { agent, invoke } = makeApi({ agent_call_status: status });
        expect(Object.isFrozen(agent)).toBe(true);
        expect(await agent.status()).toEqual(status);
        expect(invoke).toHaveBeenCalledWith('agent_call_status', {});
    });

    it('maps the request to snake_case and clamps the two numbers', () => {
        expect(agentWireRequest({ instructions: 'x' })).toEqual({ instructions: 'x' });
        expect(
            agentWireRequest({
                instructions: 'x',
                label: null,
                input: 'text',
                schema: { type: 'object' },
                maxResponseChars: 99_999,
                timeoutMs: 1,
            })
        ).toEqual({
            instructions: 'x',
            input: 'text',
            schema: { type: 'object' },
            max_response_chars: 32_000,
            timeout_ms: 5_000,
        });
        expect(
            agentWireRequest({ instructions: 'x', max_response_chars: 0.4, timeout_ms: 999_999 })
        ).toEqual({ instructions: 'x', max_response_chars: 1, timeout_ms: 180_000 });
        expect(agentWireRequest({ instructions: 'x', maxResponseChars: 1234.6 })).toEqual({
            instructions: 'x',
            max_response_chars: 1235,
        });
    });
});

// --- In the runtime -------------------------------------------------------------

class TextBlob {
    constructor(parts = [], opts = {}) {
        this.text = parts.map(String).join('');
        this.type = opts.type || '';
    }
}

const realBlob = globalThis.Blob;
const realCreate = URL.createObjectURL;

beforeAll(() => {
    globalThis.Blob = TextBlob;
    URL.createObjectURL = (blob) =>
        `data:text/javascript;base64,${Buffer.from(blob.text).toString('base64')}`;
});

afterAll(() => {
    globalThis.Blob = realBlob;
    URL.createObjectURL = realCreate;
});

const SEARCH_SOURCE = `
export default class Search {
    initialize(context) { globalThis.__probe.context = context; }
    match() { return []; }
}`;

async function waitFor(pred, what) {
    for (let i = 0; i < 200; i++) {
        const v = pred();
        if (v) return v;
        await new Promise((r) => setTimeout(r, 5));
    }
    throw new Error(`timed out waiting for ${what}`);
}

async function boot(hostFeatures) {
    globalThis.__probe = { context: null };
    vi.resetModules();
    await import('../../ui/js/extension-sandbox/runtime.js');
    const channel = new MessageChannel();
    const host = channel.port1;
    const inbox = [];
    host.onmessage = (ev) => inbox.push(ev.data);
    window.dispatchEvent(
        new MessageEvent('message', { data: { type: 'handshake' }, ports: [channel.port2] })
    );
    await waitFor(() => inbox.find((m) => m.type === 'ready'), 'ready');
    host.postMessage({
        type: 'init',
        config: {},
        sharedSources: {},
        hostFeatures,
        sources: { searchProvider: SEARCH_SOURCE },
    });
    await waitFor(() => inbox.find((m) => m.type === 'init-ack'), 'init-ack');
    return { host, inbox, context: globalThis.__probe.context };
}

describe('context.agent in the runtime', () => {
    it('is undefined without the host feature', async () => {
        const rt = await boot(['views', 'files.pick_text']);
        expect(rt.context.agent).toBeUndefined();
        expect(rt.context.hostFeatures).not.toContain('agent.background');
        rt.host.close();
    });

    it('is there with it, and its calls go to the host as invokes with kind and key', async () => {
        const rt = await boot([HOST_FEATURE_AGENT_BACKGROUND]);
        expect(rt.context.hostFeatures).toContain('agent.background');
        expect(Object.isFrozen(rt.context.agent)).toBe(true);
        const done = rt.context.agent.complete(REQUEST, { purpose: 'organize' });
        const sent = await waitFor(() => rt.inbox.find((m) => m.type === 'invoke'), 'invoke');
        expect(sent).toMatchObject({
            command: 'agent_call_reserve',
            args: { args: { purpose: 'organize', when: 'idle' } },
        });
        rt.host.postMessage({
            type: 'invoke-response',
            id: sent.id,
            error: 'Background AI is off for Mind Map.',
            errorKind: 'not_permitted',
            errorKey: 'errors.agent_call.access_off',
        });
        await expect(done).rejects.toMatchObject({
            kind: 'not_permitted',
            key: 'errors.agent_call.access_off',
            message: 'Background AI is off for Mind Map.',
        });
        rt.host.close();
    });
});

describe('the agent.background host feature', () => {
    it('is assembled into the sandbox runtime before runtime.js', () => {
        const at = RUNTIME_SOURCE_PATHS.indexOf('js/extension-sandbox/runtime/agent-api.js');
        expect(at).toBeGreaterThanOrEqual(0);
        expect(at).toBeLessThan(RUNTIME_SOURCE_PATHS.indexOf('js/extension-sandbox/runtime.js'));
    });

    it('is offered by the windows with long-lived instances, never by a view', () => {
        const files = {
            'js/shared/extension-manager.js': /\.\.\.SYNC_HOST_FEATURES,\s*HOST_FEATURE_AGENT_BACKGROUND,/,
            'js/settings/main.js': /SETTINGS_HOST_FEATURES = \[[^\]]*HOST_FEATURE_AGENT_BACKGROUND/,
        };
        for (const [p, re] of Object.entries(files)) {
            expect(readFileSync(resolve(UI_ROOT, p), 'utf8'), p).toMatch(re);
        }
        const view = readFileSync(resolve(UI_ROOT, 'js/extension-view/main.js'), 'utf8');
        expect(view).not.toMatch(/'agent\.background'|HOST_FEATURE_AGENT_BACKGROUND/);
    });
});
