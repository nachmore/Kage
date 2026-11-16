/**
 * "Sign in" on a signed-out agent error (shared/agent-sign-in.js): drawn
 * only for an error object of a signed-out kind when the backend says the
 * agent qualifies; a click starts `start_agent_sign_in` with no command of
 * its own; the terminal's keys, size, links and Stop go to the backend by
 * run id; a clean exit restarts the agent and resends once, and every other
 * ending resends nothing.
 *
 * `t()` is stubbed to return the key (plus its variables), so copy changes
 * don't break these.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../ui/js/shared/i18n.js', async (importOriginal) => {
    const real = await importOriginal();
    return {
        ...real,
        t: (key, vars) => (vars ? `${key} ${JSON.stringify(vars)}` : key),
    };
});

const {
    _resetAgentSignInForTests,
    canOfferSignIn,
    clearAgentSignInSlot,
    createDeviceFlowWatch,
    isAgentSignInActive,
    mountSignInAction,
    normalizeReconnect,
    outputSaysDeviceFlowDisabled,
    SIGN_IN_KINDS,
} = await import('../../ui/js/shared/agent-sign-in.js');
const { _setTerminalFactoryForTests } = await import('../../ui/js/shared/terminal-view.js');
const { isResendablePayload } = await import('../../ui/js/shared/error-message.js');

const flush = async (n = 6) => {
    for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 0));
};

const CTX = {
    available: true,
    reason: null,
    agent: 'Kiro CLI',
    command: 'kiro-cli login',
    terminal: true,
    timeoutMinutes: 15,
};

let terms;
function fakeTerminal(opts) {
    const term = {
        opts,
        cols: 80,
        rows: opts.rows,
        options: {},
        dataHandler: null,
        open: vi.fn((el) => {
            // Stands in for xterm's hidden textarea, which takes the keys.
            const ta = document.createElement('textarea');
            el.append(ta);
            term.textarea = ta;
        }),
        write: vi.fn(),
        focus: vi.fn(() => term.textarea?.focus()),
        blur: vi.fn(() => term.textarea?.blur()),
        dispose: vi.fn(),
        loadAddon: vi.fn(),
        onData(handler) {
            term.dataHandler = handler;
            return { dispose() {} };
        },
    };
    terms.push(term);
    return term;
}

function deferred() {
    let resolve;
    let reject;
    const promise = new Promise((res, rej) => {
        resolve = res;
        reject = rej;
    });
    return { promise, resolve, reject };
}

/** A Tauri stand-in: records invokes; each start resolves when the test says. */
function fakeTauri({ ctx = CTX, reconnect = true, stopAnswers = [] } = {}) {
    const calls = [];
    const starts = [];
    const channels = [];
    class Channel {
        constructor() {
            this.onmessage = null;
            channels.push(this);
        }
    }
    const state = { reconnect };
    const invoke = vi.fn((cmd, args) => {
        calls.push([cmd, args]);
        switch (cmd) {
            case 'agent_sign_in_context':
                return Promise.resolve(ctx);
            case 'start_agent_sign_in': {
                const d = deferred();
                starts.push({ args, ...d });
                return d.promise;
            }
            case 'reconnect_acp':
                return state.reconnect instanceof Error
                    ? Promise.reject(state.reconnect)
                    : Promise.resolve(state.reconnect);
            case 'stop_user_terminal':
                // `stopAnswers` first (false: the backend has no such run
                // yet), then true.
                {
                    const answer = stopAnswers.length ? stopAnswers.shift() : true;
                    return answer instanceof Error ? Promise.reject(answer) : Promise.resolve(answer);
                }
            default:
                return Promise.resolve(null);
        }
    });
    window.__TAURI__ = { core: { invoke, Channel } };
    const named = (name) => calls.filter(([c]) => c === name);
    return { invoke, calls, starts, channels, named, state };
}

function makeHost(overrides = {}) {
    const slot = document.createElement('div');
    slot.id = 'agentSignInSlot';
    document.body.append(slot);
    return {
        slot,
        rows: 10,
        compact: false,
        layoutChanged: vi.fn(),
        onActiveChange: vi.fn(),
        onEnded: vi.fn(),
        adoptSession: vi.fn(),
        canResend: vi.fn(() => true),
        resend: vi.fn(async () => 'sent'),
        clearError: vi.fn(),
        ...overrides,
    };
}

function actionsRow() {
    const row = document.createElement('div');
    row.className = 'chat-error-actions';
    document.body.append(row);
    return row;
}

const PAYLOAD = { kind: 'auth_required', key: 'errors.x', message: 'Signed out', sessionId: 's1' };

function click(btn, detail = 1) {
    btn.dispatchEvent(new MouseEvent('click', { bubbles: true, detail }));
}

/** Mount the button and click it; returns the panel's slot. */
async function openPanel(tauri, host, payload = PAYLOAD) {
    const btn = await mountSignInAction(actionsRow(), payload, host);
    expect(btn).toBeTruthy();
    click(btn);
    await flush();
    return btn;
}

const status = (host) => host.slot.querySelector('.agent-sign-in-status')?.textContent || '';
const buttonsIn = (host) =>
    Array.from(host.slot.querySelectorAll('.agent-sign-in-actions button')).map(
        (b) => b.className
    );

beforeEach(() => {
    terms = [];
    _setTerminalFactoryForTests(fakeTerminal);
    _resetAgentSignInForTests();
});

afterEach(() => {
    _resetAgentSignInForTests();
    _setTerminalFactoryForTests(null);
    delete window.__TAURI__;
    document.body.innerHTML = '';
});

describe('canOfferSignIn', () => {
    it('is true only for a signed-out error object', () => {
        expect(SIGN_IN_KINDS).toEqual(['auth_required', 'agent_unreachable']);
        expect(canOfferSignIn({ kind: 'auth_required' })).toBe(true);
        expect(canOfferSignIn({ kind: 'agent_unreachable' })).toBe(true);
        for (const p of [
            'Error: dispatch failure',
            null,
            undefined,
            { kind: 'connection_lost' },
            { kind: 'rate_limited' },
            { kind: 'internal' },
            { kind: ['auth_required'] },
            42,
        ]) {
            expect(canOfferSignIn(p)).toBe(false);
        }
    });
});

describe('mountSignInAction', () => {
    it('does nothing at all for a plain-string error', async () => {
        const tauri = fakeTauri();
        const row = actionsRow();
        expect(await mountSignInAction(row, 'Error: dispatch failure', makeHost())).toBeNull();
        expect(tauri.invoke).not.toHaveBeenCalled();
        expect(row.children.length).toBe(0);
    });

    it('draws no button when the backend says the agent does not qualify', async () => {
        for (const reason of ['remote', 'not_kiro', 'no_connection']) {
            _resetAgentSignInForTests();
            fakeTauri({ ctx: { available: false, reason, terminal: false } });
            const row = actionsRow();
            expect(await mountSignInAction(row, PAYLOAD, makeHost())).toBeNull();
            expect(row.querySelector('button')).toBeNull();
        }
    });

    it('draws exactly one Sign in button first in the row', async () => {
        fakeTauri();
        const row = actionsRow();
        const existing = document.createElement('button');
        existing.className = 'reconnect';
        row.append(existing);
        const host = makeHost({ buttonClass: 'chat-error-btn reconnect' });
        const btn = await mountSignInAction(row, PAYLOAD, host);
        await mountSignInAction(row, PAYLOAD, host);
        const signIn = row.querySelectorAll('.agent-sign-in-btn');
        expect(signIn.length).toBe(1);
        expect(signIn[0]).toBe(btn);
        expect(row.firstElementChild).toBe(btn);
        expect(btn.textContent).toBe('shared.agent_sign_in.btn.sign_in');
        expect(btn.classList.contains('chat-error-btn')).toBe(true);
    });

    it('caches the context', async () => {
        const tauri = fakeTauri();
        await mountSignInAction(actionsRow(), PAYLOAD, makeHost());
        await mountSignInAction(actionsRow(), PAYLOAD, makeHost());
        expect(tauri.named('agent_sign_in_context').length).toBe(1);
    });
});

describe('the run', () => {
    it('starts with only a run id, a size, the session and a channel', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        expect(tauri.starts.length).toBe(1);
        const args = tauri.starts[0].args;
        expect(Object.keys(args).sort()).toEqual(['cols', 'onOutput', 'rows', 'runId', 'sessionId']);
        expect(args.runId).toMatch(/^kage-term-[0-9a-f]{24}$/);
        expect(args.sessionId).toBe('s1');
        expect(args.onOutput).toBe(tauri.channels[0]);
        expect(args).not.toHaveProperty('command');
        expect(args).not.toHaveProperty('program');
        expect(host.onActiveChange).toHaveBeenCalledWith(true);
        expect(isAgentSignInActive()).toBe(true);
        expect(terms[0].focus).toHaveBeenCalled();
        expect(host.slot.querySelector('.agent-sign-in-cmd').textContent).toBe('kiro-cli login');
        expect(host.slot.querySelector('.agent-sign-in-term').dir).toBe('ltr');
    });

    it('ignores a double click and a second click while live', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        const btn = await mountSignInAction(actionsRow(), PAYLOAD, host);
        click(btn, 2);
        await flush();
        expect(tauri.starts.length).toBe(0);
        click(btn);
        await flush();
        click(btn);
        await flush();
        expect(tauri.starts.length).toBe(1);
    });

    it('wires output, keys, size and links to the backend by run id', async () => {
        const observers = [];
        globalThis.ResizeObserver = class {
            constructor(cb) {
                observers.push(cb);
            }
            observe() {}
            disconnect() {}
        };
        vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
        try {
            const tauri = fakeTauri();
            const host = makeHost();
            const btn = await mountSignInAction(actionsRow(), PAYLOAD, host);
            click(btn);
            await vi.advanceTimersByTimeAsync(0);
            const { runId } = tauri.starts[0].args;
            const term = terms[0];
            expect(status(host)).toBe(
                'shared.agent_sign_in.status.starting {"command":"kiro-cli login"}'
            );
            tauri.channels[0].onmessage({ seq: 0, data: '? Select login method' });
            expect(term.write).toHaveBeenCalledWith('? Select login method', expect.any(Function));
            expect(status(host)).toBe('shared.agent_sign_in.status.running');
            term.dataHandler('\u001b[B');
            expect(tauri.named('write_user_terminal')).toEqual([
                ['write_user_terminal', { runId, data: '\u001b[B' }],
            ]);
            term.opts.linkHandler.activate(null, 'https://view.awsapps.com/start');
            expect(tauri.named('open_user_terminal_link')).toEqual([
                ['open_user_terminal_link', { runId, url: 'https://view.awsapps.com/start' }],
            ]);
            // The view got wider: reported once, after the debounce.
            term.cols = 100;
            for (const cb of observers) cb();
            expect(tauri.named('resize_user_terminal').length).toBe(0);
            await vi.advanceTimersByTimeAsync(100);
            expect(tauri.named('resize_user_terminal')).toEqual([
                ['resize_user_terminal', { runId, cols: 100, rows: 10 }],
            ]);
        } finally {
            vi.useRealTimers();
            delete globalThis.ResizeObserver;
        }
    });

    it('acknowledges each output message only once xterm has written it', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        const { runId } = tauri.starts[0].args;
        const term = terms[0];
        tauri.channels[0].onmessage({ seq: 0, data: 'first' });
        tauri.channels[0].onmessage({ seq: 1, data: 'second' });
        await flush();
        // Not written yet: nothing acknowledged, so the backend holds back.
        expect(tauri.named('ack_user_terminal')).toEqual([]);
        expect(term.write.mock.calls.map((c) => c[0])).toEqual(['first', 'second']);
        term.write.mock.calls[0][1]();
        expect(tauri.named('ack_user_terminal')).toEqual([
            ['ack_user_terminal', { runId, seq: 0 }],
        ]);
        term.write.mock.calls[1][1]();
        expect(tauri.named('ack_user_terminal')).toEqual([
            ['ack_user_terminal', { runId, seq: 0 }],
            ['ack_user_terminal', { runId, seq: 1 }],
        ]);
        // A message with no text, or no usable seq, writes nothing; the
        // former is still acknowledged so the run never waits on it.
        tauri.channels[0].onmessage({ seq: 2, data: '' });
        tauri.channels[0].onmessage({ data: 'no seq' });
        tauri.channels[0].onmessage({ seq: -1, data: 'bad seq' });
        expect(tauri.named('ack_user_terminal').at(-1)).toEqual([
            'ack_user_terminal',
            { runId, seq: 2 },
        ]);
        term.write.mock.calls.at(-1)[1]();
        term.write.mock.calls.at(-2)[1]();
        expect(tauri.named('ack_user_terminal').length).toBe(3);
    });

    it('acknowledges output for a view that is gone instead of writing it', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        const { runId } = tauri.starts[0].args;
        const oldChannel = tauri.channels[0];
        click(host.slot.querySelector('.agent-sign-in-stop'));
        await flush();
        tauri.starts[0].resolve({ runId, outcome: 'cancelled', exitCode: null, signal: null });
        await flush();
        click(host.slot.querySelector('.agent-sign-in-again'));
        await flush();
        expect(tauri.starts.length).toBe(2);
        const writes = () => terms.reduce((n, term) => n + term.write.mock.calls.length, 0);
        const before = writes();
        oldChannel.onmessage({ seq: 7, data: 'late output of the first run' });
        expect(writes()).toBe(before);
        expect(tauri.named('ack_user_terminal')).toEqual([
            ['ack_user_terminal', { runId, seq: 7 }],
        ]);
    });

    it('says it is waiting after a second without output', async () => {
        vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
        try {
            fakeTauri();
            const host = makeHost();
            click(await mountSignInAction(actionsRow(), PAYLOAD, host));
            await vi.advanceTimersByTimeAsync(0);
            expect(status(host)).toMatch(/^shared\.agent_sign_in\.status\.starting/);
            await vi.advanceTimersByTimeAsync(1100);
            expect(status(host)).toBe('shared.agent_sign_in.status.running');
        } finally {
            vi.useRealTimers();
        }
    });

    it('Stop stops it; nothing restarts or resends; Sign in again starts afresh', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        const runId = tauri.starts[0].args.runId;
        click(host.slot.querySelector('.agent-sign-in-stop'));
        await flush();
        expect(tauri.named('stop_user_terminal')).toEqual([['stop_user_terminal', { runId }]]);
        expect(status(host)).toBe('shared.agent_sign_in.status.stopping');
        tauri.starts[0].resolve({ runId, outcome: 'cancelled', exitCode: null, signal: 'Terminated' });
        await flush();
        expect(status(host)).toBe('shared.agent_sign_in.status.stopped');
        expect(tauri.named('reconnect_acp').length).toBe(0);
        expect(host.resend).not.toHaveBeenCalled();
        expect(host.onEnded).toHaveBeenCalledWith(
            expect.objectContaining({ state: 'stopped', outcome: 'cancelled' })
        );
        expect(host.onActiveChange).toHaveBeenLastCalledWith(false);
        expect(isAgentSignInActive()).toBe(false);
        click(host.slot.querySelector('.agent-sign-in-again'));
        await flush();
        expect(tauri.starts.length).toBe(2);
        expect(tauri.starts[1].args.runId).not.toBe(runId);
        expect(tauri.starts[1].args.runId).toMatch(/^kage-term-[0-9a-f]{24}$/);
    });

    it('Stop before the start call (xterm still loading) never starts it', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        const btn = await mountSignInAction(actionsRow(), PAYLOAD, host);
        click(btn);
        // Same tick: startRun is waiting on ensureXterm.
        const stop = host.slot.querySelector('.agent-sign-in-stop');
        click(stop);
        expect(status(host)).toBe('shared.agent_sign_in.status.stopping');
        await flush();
        expect(tauri.starts.length).toBe(0);
        expect(tauri.named('stop_user_terminal').length).toBe(0);
        expect(status(host)).toBe('shared.agent_sign_in.status.stopped');
        expect(stop.hidden).toBe(true);
        expect(isAgentSignInActive()).toBe(false);
        expect(buttonsIn(host).some((c) => c.includes('agent-sign-in-again'))).toBe(true);
        expect(host.onEnded).toHaveBeenCalledWith(
            expect.objectContaining({ state: 'stopped', outcome: 'cancelled' })
        );
    });

    it('Stop before the backend claims the run keeps asking until it takes', async () => {
        const tauri = fakeTauri({ stopAnswers: [false, false] });
        const host = makeHost();
        await openPanel(tauri, host);
        const runId = tauri.starts[0].args.runId;
        const stop = host.slot.querySelector('.agent-sign-in-stop');
        click(stop);
        await flush();
        expect(tauri.named('stop_user_terminal').length).toBe(1);
        expect(stop.disabled).toBe(true);
        expect(status(host)).toBe('shared.agent_sign_in.status.stopping');
        await new Promise((r) => setTimeout(r, 500));
        expect(tauri.named('stop_user_terminal')).toEqual([
            ['stop_user_terminal', { runId }],
            ['stop_user_terminal', { runId }],
            ['stop_user_terminal', { runId }],
        ]);
        // Output arriving meanwhile doesn't turn "Stopping…" back.
        tauri.channels[0].onmessage({ data: 'Select a login method' });
        expect(status(host)).toBe('shared.agent_sign_in.status.stopping');
        tauri.starts[0].resolve({ runId, outcome: 'cancelled', exitCode: null, signal: 'Terminated' });
        await flush();
        expect(status(host)).toBe('shared.agent_sign_in.status.stopped');
        expect(tauri.named('stop_user_terminal').length).toBe(3);
    });

    it('a Stop that had not taken yet still wins over a clean exit: no restart, no resend', async () => {
        const tauri = fakeTauri({ stopAnswers: [false] });
        const host = makeHost();
        await openPanel(tauri, host);
        click(host.slot.querySelector('.agent-sign-in-stop'));
        await flush();
        expect(tauri.named('stop_user_terminal').length).toBe(1);
        // The login finishes before the retry asks again.
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 0, signal: null });
        await flush();
        expect(tauri.named('reconnect_acp').length).toBe(0);
        expect(host.resend).not.toHaveBeenCalled();
        expect(status(host)).toBe('shared.agent_sign_in.status.stopped');
        expect(host.onEnded).toHaveBeenCalledWith(
            expect.objectContaining({ state: 'stopped', outcome: 'cancelled' })
        );
    });

    it('a Stop whose delivery failed is not forgotten: a later clean exit still restarts nothing', async () => {
        const tauri = fakeTauri({ stopAnswers: [new Error('busy')] });
        const host = makeHost();
        await openPanel(tauri, host);
        click(host.slot.querySelector('.agent-sign-in-stop'));
        await flush();
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 0, signal: null });
        await flush();
        expect(tauri.named('reconnect_acp').length).toBe(0);
        expect(host.resend).not.toHaveBeenCalled();
        expect(status(host)).toBe('shared.agent_sign_in.status.stopped');
    });

    it('a Stop keeps being asked for after the quick tries, until it takes', async () => {
        vi.useFakeTimers();
        try {
            const tauri = fakeTauri({ stopAnswers: Array(105).fill(false) });
            const host = makeHost();
            const btn = await mountSignInAction(actionsRow(), PAYLOAD, host);
            click(btn);
            await vi.advanceTimersByTimeAsync(10);
            click(host.slot.querySelector('.agent-sign-in-stop'));
            await vi.advanceTimersByTimeAsync(100 * 200 + 6 * 2000);
            // 100 quick tries, then slower ones, until the 106th takes.
            expect(tauri.named('stop_user_terminal').length).toBe(106);
            tauri.starts[0].resolve({ outcome: 'exited', exitCode: 0, signal: null });
            await vi.advanceTimersByTimeAsync(10);
            expect(tauri.named('reconnect_acp').length).toBe(0);
            expect(host.resend).not.toHaveBeenCalled();
        } finally {
            vi.useRealTimers();
        }
    });

    it('Stop stops asking once the run ends on its own', async () => {
        const tauri = fakeTauri({ stopAnswers: Array(50).fill(false) });
        const host = makeHost();
        await openPanel(tauri, host);
        click(host.slot.querySelector('.agent-sign-in-stop'));
        await flush();
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 1, signal: null });
        await flush();
        const asked = tauri.named('stop_user_terminal').length;
        await new Promise((r) => setTimeout(r, 450));
        expect(tauri.named('stop_user_terminal').length).toBe(asked);
        expect(status(host)).toBe('shared.agent_sign_in.status.failed_exit {"code":1}');
    });

    it('a failed run hands the keyboard from the terminal to Sign in again', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        expect(document.activeElement).toBe(terms[0].textarea);
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 1, signal: null });
        await flush();
        expect(document.activeElement.classList.contains('agent-sign-in-again')).toBe(true);
        // Esc reaches the window again.
        const windowKeys = vi.fn();
        document.addEventListener('keydown', windowKeys);
        terms[0].textarea.dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })
        );
        document.removeEventListener('keydown', windowKeys);
        expect(windowKeys).toHaveBeenCalledTimes(1);
    });

    it('a run that ends while the user is elsewhere leaves focus alone', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        const other = document.createElement('input');
        document.body.append(other);
        other.focus();
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 1, signal: null });
        await flush();
        expect(document.activeElement).toBe(other);
    });

    it('exit 0: restarts, adopts the new session, resends once, clears the error', async () => {
        const tauri = fakeTauri({
            reconnect: { reconnected: true, sessionId: 's2', sessionReloaded: true },
        });
        const host = makeHost();
        await openPanel(tauri, host);
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 0, signal: null, durationMs: 9 });
        await flush();
        expect(tauri.named('reconnect_acp')).toEqual([['reconnect_acp', { sessionId: 's1' }]]);
        expect(host.adoptSession).toHaveBeenCalledWith('s2', 's1');
        expect(host.resend).toHaveBeenCalledTimes(1);
        expect(host.resend).toHaveBeenCalledWith('s2');
        expect(status(host)).toBe('shared.agent_sign_in.status.done');
        expect(host.clearError).toHaveBeenCalled();
        expect(terms[0].dispose).toHaveBeenCalled();
        expect(host.slot.querySelector('.agent-sign-in-term').hidden).toBe(true);
        expect(host.onEnded).toHaveBeenCalledWith(
            expect.objectContaining({ state: 'success', outcome: 'exited' })
        );
        expect(isAgentSignInActive()).toBe(false);
        expect(host.onActiveChange).toHaveBeenLastCalledWith(false);
    });

    it('exit 0 with a bare boolean reconnect answer resends into the same session', async () => {
        const tauri = fakeTauri({ reconnect: true });
        const host = makeHost();
        await openPanel(tauri, host);
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush();
        // Same id, still told: the restart pinned the window to it, and the
        // window keeps the session it shows if that is another one.
        expect(host.adoptSession).toHaveBeenCalledWith('s1', 's1');
        expect(host.adoptSession.mock.invocationCallOrder[0]).toBeLessThan(
            host.canResend.mock.invocationCallOrder[0]
        );
        expect(host.resend).toHaveBeenCalledWith('s1');
    });

    describe('the resend follows the turn-id rules (the window decides)', () => {
        // A host that decides as the floating and chat windows do: only the
        // failed turn `t1` the error names, only when the error says it may
        // go again, at most once.
        function turnHost(payload) {
            let resent = false;
            return makeHost({
                canResend: vi.fn(() => !resent && isResendablePayload(payload, 't1')),
                resend: vi.fn(async () => {
                    resent = true;
                    return 'sent';
                }),
            });
        }
        const OWN = { ...PAYLOAD, turnId: 't1', resendable: true };

        for (const [name, payload, expected] of [
            ['its own resendable turn is resent', OWN, 1],
            ["another turn's error resends nothing", { ...OWN, turnId: 't0' }, 0],
            ['a payload not marked resendable resends nothing', { ...OWN, resendable: false }, 0],
            ['a payload naming no turn resends nothing', { ...PAYLOAD, resendable: true }, 0],
        ]) {
            it(name, async () => {
                const tauri = fakeTauri();
                const host = turnHost(payload);
                await openPanel(tauri, host, payload);
                tauri.starts[0].resolve({ outcome: 'exited', exitCode: 0 });
                await flush();
                expect(tauri.named('reconnect_acp')).toEqual([
                    ['reconnect_acp', { sessionId: 's1' }],
                ]);
                expect(host.resend).toHaveBeenCalledTimes(expected);
                expect(status(host)).toBe(
                    expected
                        ? 'shared.agent_sign_in.status.done'
                        : 'shared.agent_sign_in.status.ready'
                );
            });
        }
    });

    it('a conversation not reopened: the message goes to the new one, and says so', async () => {
        const tauri = fakeTauri({
            reconnect: { reconnected: true, sessionId: 's9', sessionReloaded: false },
        });
        const host = makeHost();
        await openPanel(tauri, host);
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush();
        expect(host.resend).toHaveBeenCalledWith('s9');
        expect(status(host)).toBe('shared.agent_sign_in.status.done_new_session');
    });

    it('a restart that fails resends nothing; Reconnect retries then resends', async () => {
        for (const outcome of [{ reconnected: false }, new Error('agent gone')]) {
            _resetAgentSignInForTests();
            document.body.innerHTML = '';
            const tauri = fakeTauri({ reconnect: outcome });
            const host = makeHost();
            await openPanel(tauri, host);
            tauri.starts[0].resolve({ outcome: 'exited', exitCode: 0 });
            await flush();
            expect(status(host)).toMatch(/^shared\.agent_sign_in\.status\.restart_failed/);
            expect(host.resend).not.toHaveBeenCalled();
            tauri.state.reconnect = true;
            click(host.slot.querySelector('.agent-sign-in-reconnect'));
            await flush();
            expect(tauri.named('reconnect_acp').length).toBe(2);
            expect(host.resend).toHaveBeenCalledTimes(1);
        }
    });

    it('nothing to resend: ready; a resend that throws: resend_failed', async () => {
        let tauri = fakeTauri();
        let host = makeHost({ resend: vi.fn(async () => 'none') });
        await openPanel(tauri, host);
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush();
        expect(status(host)).toBe('shared.agent_sign_in.status.ready');
        expect(host.clearError).not.toHaveBeenCalled();

        _resetAgentSignInForTests();
        document.body.innerHTML = '';
        tauri = fakeTauri();
        host = makeHost({
            resend: vi.fn(async () => {
                throw new Error('busy');
            }),
        });
        await openPanel(tauri, host);
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush();
        expect(status(host)).toBe('shared.agent_sign_in.status.resend_failed {"error":"busy"}');
    });

    it('a window that cannot resend gets ready, not a send', async () => {
        const tauri = fakeTauri();
        const host = makeHost({ canResend: vi.fn(() => false) });
        await openPanel(tauri, host);
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush();
        expect(host.resend).not.toHaveBeenCalled();
        expect(status(host)).toBe('shared.agent_sign_in.status.ready');
    });

    it('exit 1: failed, read-only terminal, no restart or resend', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 1, signal: null });
        await flush();
        expect(status(host)).toBe('shared.agent_sign_in.status.failed_exit {"code":1}');
        expect(tauri.named('reconnect_acp').length).toBe(0);
        expect(host.resend).not.toHaveBeenCalled();
        expect(terms[0].options.disableStdin).toBe(true);
        expect(terms[0].dispose).not.toHaveBeenCalled();
        // Typing now goes nowhere.
        terms[0].dataHandler('x');
        expect(tauri.named('write_user_terminal').length).toBe(0);
        expect(buttonsIn(host).some((c) => c.includes('agent-sign-in-open-terminal'))).toBe(true);
        expect(buttonsIn(host).some((c) => c.includes('agent-sign-in-again'))).toBe(true);
        expect(buttonsIn(host).some((c) => c.includes('agent-sign-in-close'))).toBe(true);
    });

    it("emptying the slot after a failed run disposes that run's terminal", async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 1, signal: null });
        await flush();
        expect(terms[0].dispose).not.toHaveBeenCalled();
        expect(clearAgentSignInSlot(host.slot)).toBe(true);
        expect(terms[0].dispose).toHaveBeenCalledTimes(1);
        expect(host.slot.children.length).toBe(0);
        // Nothing left to dispose a second time.
        clearAgentSignInSlot(host.slot);
        expect(terms[0].dispose).toHaveBeenCalledTimes(1);
    });

    it('a new attempt in the same slot disposes the finished one first', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        tauri.starts[0].resolve({ outcome: 'cancelled', exitCode: null, signal: null });
        await flush();
        await openPanel(tauri, host);
        expect(terms[0].dispose).toHaveBeenCalledTimes(1);
        expect(terms.length).toBe(2);
    });

    it('a slot with a sign-in still going is left alone', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        expect(clearAgentSignInSlot(host.slot)).toBe(false);
        expect(terms[0].dispose).not.toHaveBeenCalled();
        expect(host.slot.children.length).toBeGreaterThan(0);
    });

    it('no Open in Terminal where the backend has no terminal hand-off', async () => {
        const tauri = fakeTauri({ ctx: { ...CTX, terminal: false } });
        const host = makeHost();
        await openPanel(tauri, host);
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 1 });
        await flush();
        expect(buttonsIn(host).some((c) => c.includes('agent-sign-in-open-terminal'))).toBe(false);
    });

    it('a signal ends it: failed_signal', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: null, signal: 'Killed: 9' });
        await flush();
        expect(status(host)).toBe('shared.agent_sign_in.status.failed_signal {"signal":"Killed: 9"}');
        expect(host.resend).not.toHaveBeenCalled();
    });

    it('the time limit: timed_out, no resend, onEnded says so', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        tauri.starts[0].resolve({ outcome: 'timed_out', exitCode: null, signal: 'Terminated' });
        await flush();
        expect(status(host)).toBe('shared.agent_sign_in.status.timed_out {"minutes":15}');
        expect(host.resend).not.toHaveBeenCalled();
        expect(host.onEnded).toHaveBeenCalledWith(
            expect.objectContaining({ state: 'timed_out', outcome: 'timed_out' })
        );
        expect(host.onActiveChange).toHaveBeenLastCalledWith(false);
    });

    it('a refused start: start_failed with the error text; no resend', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        tauri.starts[0].reject({ kind: 'internal', message: 'Kage is already signing in to the agent.' });
        await flush();
        expect(status(host)).toBe(
            'shared.agent_sign_in.status.start_failed {"error":"Kage is already signing in to the agent."}'
        );
        expect(host.resend).not.toHaveBeenCalled();
        expect(isAgentSignInActive()).toBe(false);
        expect(buttonsIn(host).some((c) => c.includes('agent-sign-in-open-terminal'))).toBe(true);
    });

    it('Open in Terminal hands it over with no command, then Reconnect restarts and resends', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 1 });
        await flush();
        click(host.slot.querySelector('.agent-sign-in-open-terminal'));
        await flush();
        const [[, args]] = tauri.named('agent_sign_in_in_terminal');
        expect(Object.keys(args).sort()).toEqual(['runId', 'sessionId']);
        expect(args.runId).toMatch(/^kage-term-[0-9a-f]{24}$/);
        expect(args.sessionId).toBe('s1');
        expect(status(host)).toBe('shared.agent_sign_in.status.terminal_opened');
        click(host.slot.querySelector('.agent-sign-in-reconnect'));
        await flush();
        expect(tauri.named('reconnect_acp').length).toBe(1);
        expect(host.resend).toHaveBeenCalledTimes(1);
    });

    it('Close empties the panel', async () => {
        const tauri = fakeTauri();
        const host = makeHost();
        await openPanel(tauri, host);
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 1 });
        await flush();
        click(host.slot.querySelector('.agent-sign-in-close'));
        expect(host.slot.children.length).toBe(0);
    });
});

describe('normalizeReconnect', () => {
    it('maps a bare boolean and passes ReconnectOutcome through', () => {
        expect(normalizeReconnect(true, 's1')).toEqual({
            reconnected: true,
            sessionId: 's1',
            sessionReloaded: true,
        });
        expect(normalizeReconnect(false, 's1').reconnected).toBe(false);
        expect(
            normalizeReconnect({ reconnected: true, sessionId: 's2', sessionReloaded: false }, 's1')
        ).toEqual({ reconnected: true, sessionId: 's2', sessionReloaded: false });
        expect(normalizeReconnect(undefined, 's1').reconnected).toBe(false);
    });
});

/**
 * Linux and Windows sign in with the device flow only (the backend tells
 * kiro-cli it runs remotely, so it starts no browser inside the login).
 * An organization that turned the device flow off can't sign in that way:
 * the panel says so and offers the command for the user's own terminal.
 * The messages are kiro-cli 3.3.0's own.
 */
const ORG_DISABLED = [
    'Your organization has disabled device flow sign-in. Use browser sign-in.',
    'Every available profile has device flow sign-in disabled',
    'None of the sign-in methods your organization permits work without a browser. Sign in on a local machine instead.',
];

describe('outputSaysDeviceFlowDisabled', () => {
    it("matches kiro-cli's messages, through colours, line wraps and case", () => {
        for (const msg of ORG_DISABLED) {
            expect(outputSaysDeviceFlowDisabled(msg)).toBe(true);
            expect(outputSaysDeviceFlowDisabled(`\u001b[31mError:\u001b[0m ${msg}\r\n`)).toBe(true);
        }
        expect(
            outputSaysDeviceFlowDisabled(
                'Your organization has disabled \u001b[1mdevice\u001b[22m flow\r\n  sign-in. Use browser sign-in.'
            )
        ).toBe(true);
        expect(
            outputSaysDeviceFlowDisabled('YOUR ORGANIZATION HAS DISABLED DEVICE FLOW SIGN-IN. USE BROWSER SIGN-IN.')
        ).toBe(true);
    });

    it('does not match an ordinary device-flow sign-in or other failures', () => {
        for (const text of [
            '',
            null,
            undefined,
            'Confirm the following code in the browser\r\nCode: ABCD-EFGH\r\nOpen this URL: https://view.awsapps.com/start/#/device?user_code=ABCD-EFGH',
            'Device code expired. Please try again.',
            'Already logged in, please logout with kiro-cli logout first',
            'Device flow error: access denied',
            // Printed while a sign-in is still succeeding: some profiles are
            // hidden, not the device flow turned off.
            '2 profile(s) hidden: your organization has disabled device flow sign-in for them',
            'Your organization has disabled device flow sign-in for some profiles',
        ]) {
            expect(outputSaysDeviceFlowDisabled(text)).toBe(false);
        }
    });
});

describe('createDeviceFlowWatch', () => {
    it('sees the message split across output messages, escapes and all', () => {
        const msg = `\u001b[31m${ORG_DISABLED[0]}\u001b[0m`;
        for (let cut = 1; cut < msg.length; cut += 7) {
            const watch = createDeviceFlowWatch();
            watch.push(msg.slice(0, cut));
            watch.push(msg.slice(cut));
            expect(watch.matched, `cut at ${cut}`).toBe(true);
        }
    });

    it('stays matched, keeps a bounded tail, and ignores junk', () => {
        const watch = createDeviceFlowWatch();
        watch.push(ORG_DISABLED[1]);
        watch.push('x'.repeat(100_000));
        expect(watch.matched).toBe(true);
        const other = createDeviceFlowWatch();
        for (let i = 0; i < 50; i++) other.push('y'.repeat(4096));
        other.push(null);
        other.push(42);
        expect(other.matched).toBe(false);
        expect(other.tailLength).toBeLessThanOrEqual(1024);
    });
});

describe('device flow turned off by the organization', () => {
    const DEVICE_CTX = {
        ...CTX,
        terminal: false,
        deviceFlowOnly: true,
        manualCommand: 'kiro-cli login',
    };
    const manualBox = (host) => host.slot.querySelector('.agent-sign-in-manual');

    async function runEnding(ctx, output, report) {
        const tauri = fakeTauri({ ctx });
        const host = makeHost();
        await openPanel(tauri, host);
        output.forEach((data, seq) => tauri.channels[0].onmessage({ seq, data }));
        tauri.starts[0].resolve(report);
        await flush();
        return { tauri, host };
    }

    it('says why and offers the command for their own terminal, with Copy', async () => {
        const writeText = vi.fn(() => Promise.resolve());
        Object.defineProperty(navigator, 'clipboard', {
            value: { writeText },
            configurable: true,
        });
        const logs = ['log', 'info', 'warn', 'error', 'debug'].map((m) =>
            vi.spyOn(console, m).mockImplementation(() => {})
        );
        try {
            const { tauri, host } = await runEnding(
                DEVICE_CTX,
                ['? Select login method\r\n', `\u001b[31m${ORG_DISABLED[0].slice(0, 20)}`, `${ORG_DISABLED[0].slice(20)}\u001b[0m\r\n`],
                { outcome: 'exited', exitCode: 1, signal: null }
            );
            expect(status(host)).toBe(
                'shared.agent_sign_in.status.device_flow_disabled {"agent":"Kiro CLI"}'
            );
            const box = manualBox(host);
            expect(box).toBeTruthy();
            expect(box.hidden).toBe(false);
            const code = box.querySelector('code');
            expect(code.textContent).toBe('kiro-cli login');
            expect(code.dir).toBe('ltr');
            const copy = box.querySelector('button.shell-cmd-copy');
            expect(copy.textContent).toContain('shared.shell.copy_command');
            copy.click();
            await flush();
            expect(writeText).toHaveBeenCalledWith('kiro-cli login');
            // Signing in here again would fail the same way.
            expect(buttonsIn(host).some((c) => c.includes('agent-sign-in-again'))).toBe(false);
            expect(buttonsIn(host).some((c) => c.includes('agent-sign-in-open-terminal'))).toBe(false);
            expect(buttonsIn(host).some((c) => c.includes('agent-sign-in-close'))).toBe(true);
            expect(tauri.named('reconnect_acp').length).toBe(0);
            expect(host.onEnded).toHaveBeenCalledWith(
                expect.objectContaining({ state: 'device_flow_disabled' })
            );
            expect(isAgentSignInActive()).toBe(false);
            // Signed in from their own terminal: Reconnect restarts and resends.
            click(host.slot.querySelector('.agent-sign-in-reconnect'));
            await flush();
            expect(tauri.named('reconnect_acp').length).toBe(1);
            expect(host.resend).toHaveBeenCalledTimes(1);
            // Nothing the terminal showed went to the console.
            for (const spy of logs) {
                for (const args of spy.mock.calls) {
                    expect(args.join(' ')).not.toMatch(/device flow/i);
                }
            }
        } finally {
            for (const spy of logs) spy.mockRestore();
            delete navigator.clipboard;
        }
    });

    it('still sees the message when the reply overtakes the last output', async () => {
        const tauri = fakeTauri({ ctx: DEVICE_CTX });
        const host = makeHost();
        await openPanel(tauri, host);
        tauri.channels[0].onmessage({ seq: 0, data: '? Select login method\r\n' });
        // The command's reply arrives before the message that matters.
        tauri.starts[0].resolve({ outcome: 'exited', exitCode: 1, signal: null, outputMessages: 2 });
        await flush();
        tauri.channels[0].onmessage({ seq: 1, data: `${ORG_DISABLED[0]}\r\n` });
        await flush();
        expect(status(host)).toBe(
            'shared.agent_sign_in.status.device_flow_disabled {"agent":"Kiro CLI"}'
        );
        expect(manualBox(host)?.hidden).toBe(false);
    });

    it('a clean exit still signs in, whatever was printed', async () => {
        const { tauri, host } = await runEnding(DEVICE_CTX, [ORG_DISABLED[2]], {
            outcome: 'exited',
            exitCode: 0,
            signal: null,
        });
        expect(tauri.named('reconnect_acp').length).toBe(1);
        expect(manualBox(host)?.hidden ?? true).toBe(true);
    });

    it('a failure without the message is an ordinary failure', async () => {
        const { host } = await runEnding(DEVICE_CTX, ['Device code expired. Please try again.'], {
            outcome: 'exited',
            exitCode: 1,
            signal: null,
        });
        expect(status(host)).toBe('shared.agent_sign_in.status.failed_exit {"code":1}');
        expect(manualBox(host)?.hidden ?? true).toBe(true);
        expect(buttonsIn(host).some((c) => c.includes('agent-sign-in-again'))).toBe(true);
    });

    it('Stop is Stop, even after the message', async () => {
        const { host } = await runEnding(DEVICE_CTX, [ORG_DISABLED[0]], {
            outcome: 'cancelled',
            exitCode: null,
            signal: null,
        });
        expect(status(host)).toBe('shared.agent_sign_in.status.stopped');
        expect(manualBox(host)?.hidden ?? true).toBe(true);
    });

    it('macOS (browser sign-in, Terminal hand-off) keeps Open in Terminal', async () => {
        const { host } = await runEnding(
            { ...CTX, terminal: true, deviceFlowOnly: false, manualCommand: 'kiro-cli login' },
            [ORG_DISABLED[0]],
            { outcome: 'exited', exitCode: 1, signal: null }
        );
        expect(status(host)).toBe('shared.agent_sign_in.status.failed_exit {"code":1}');
        expect(manualBox(host)?.hidden ?? true).toBe(true);
        expect(buttonsIn(host).some((c) => c.includes('agent-sign-in-open-terminal'))).toBe(true);
    });

    it("one run's output never decides the next run's ending", async () => {
        const tauri = fakeTauri({ ctx: { ...DEVICE_CTX } });
        const host = makeHost();
        await openPanel(tauri, host);
        tauri.channels[0].onmessage({ seq: 0, data: ORG_DISABLED[0] });
        tauri.starts[0].resolve({ outcome: 'timed_out', exitCode: null, signal: null });
        await flush();
        click(host.slot.querySelector('.agent-sign-in-again'));
        await flush();
        expect(tauri.starts.length).toBe(2);
        tauri.starts[1].resolve({ outcome: 'exited', exitCode: 1, signal: null });
        await flush();
        expect(status(host)).toBe('shared.agent_sign_in.status.failed_exit {"code":1}');
        expect(manualBox(host)?.hidden ?? true).toBe(true);
    });

    it('the hint says the browser does not open by itself where it is device flow only', async () => {
        const tauri = fakeTauri({ ctx: DEVICE_CTX });
        const host = makeHost();
        await openPanel(tauri, host);
        expect(host.slot.querySelector('.agent-sign-in-hint').textContent).toBe(
            'shared.agent_sign_in.hint_device_flow'
        );
        _resetAgentSignInForTests();
        document.body.innerHTML = '';
        const mac = fakeTauri({ ctx: CTX });
        const macHost = makeHost();
        await openPanel(mac, macHost);
        expect(macHost.slot.querySelector('.agent-sign-in-hint').textContent).toBe(
            'shared.agent_sign_in.hint'
        );
    });
});
