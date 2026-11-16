/**
 * Sign in under a failed floating bootstrap: after an update kiro-cli is
 * signed out and refuses `session/new` with `-32000`, which the backend
 * reports as `{ kind: 'auth_required' }`. The bootstrap's error then gets
 * Sign in (and Reconnect); a clean exit restarts the agent
 * (`reconnect_acp`, no session), bootstraps a session
 * (`switch_acp_session`) and only then sends the message that was waiting
 * on the bootstrap, exactly once, with a turn id of its own. A bootstrap
 * that still fails shows the error again, with Sign in, and sends nothing;
 * a send or a different draft since means nothing is sent; a plain-string
 * bootstrap error gets no Sign in.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { marked } from 'marked';
import { _resetAgentSignInForTests } from '../../ui/js/shared/agent-sign-in.js';
import { _setTerminalFactoryForTests } from '../../ui/js/shared/terminal-view.js';

vi.mock('../../ui/js/shared/network.js', async (importOriginal) => ({
    ...(await importOriginal()),
    checkOnError: async () => true,
    checkOnline: async () => true,
}));

let FloatingApp;
beforeAll(async () => {
    globalThis.marked = marked;
    ({ FloatingApp } = await import('../../ui/js/floating/app.js'));
});

const flush = async (n = 8) => {
    for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 0));
};
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

const CTX = {
    available: true,
    agent: 'Kiro CLI',
    command: 'kiro-cli login',
    terminal: false,
    timeoutMinutes: 15,
};
/** What `switch_acp_session` rejects with for a signed-out Kiro. */
const SIGNED_OUT = {
    kind: 'auth_required',
    key: 'errors.agent.auth_required_kiro',
    message: 'Kiro needs you to sign in. Run kiro-cli login in a terminal, then click Reconnect.',
};

function fakeTerminal(opts) {
    return {
        opts,
        cols: 80,
        rows: opts.rows,
        options: {},
        open: vi.fn(),
        write: vi.fn(),
        focus: vi.fn(),
        dispose: vi.fn(),
        loadAddon: vi.fn(),
        onData: () => ({ dispose() {} }),
    };
}

/**
 * `bootstraps`: what each `switch_acp_session` answers, in order (an Error
 * or object rejects, a string resolves); the last one repeats.
 */
function setup({ bootstraps }) {
    document.body.innerHTML = `
        <div class="speech-bubble">
            <div class="content-area" id="contentArea">
                <div class="response-text" id="responseText"></div>
            </div>
            <div class="agent-sign-in-slot" id="agentSignInSlot"></div>
        </div>`;
    const calls = [];
    const starts = [];
    let bootstrapAt = 0;
    const invoke = vi.fn(async (cmd, args) => {
        calls.push([cmd, args]);
        if (cmd === 'agent_sign_in_context') return CTX;
        if (cmd === 'start_agent_sign_in') {
            return new Promise((resolve, reject) => starts.push({ args, resolve, reject }));
        }
        if (cmd === 'reconnect_acp') {
            return { reconnected: true, sessionId: null, sessionReloaded: false };
        }
        if (cmd === 'switch_acp_session') {
            const answer = bootstraps[Math.min(bootstrapAt++, bootstraps.length - 1)];
            if (typeof answer === 'string' && answer.startsWith('s-')) return answer;
            throw answer;
        }
        if (cmd === 'handle_floating_input') return '[]';
        if (cmd === 'get_config') return { ui: { preserve_last_response: false } };
        return null;
    });
    class Channel {
        constructor() {
            this.onmessage = null;
        }
    }
    window.__TAURI__ = {
        core: { invoke, Channel },
        event: { emit: vi.fn(), listen: vi.fn(async () => () => {}) },
    };
    const appWindow = {
        show: vi.fn(async () => {}),
        setFocus: vi.fn(async () => {}),
        hide: vi.fn(async () => {}),
        listen: vi.fn(async () => () => {}),
    };
    const app = new FloatingApp(invoke, appWindow, vi.fn(async () => () => {}));
    const div = () => document.createElement('div');
    app.elements = {
        contentArea: document.getElementById('contentArea'),
        responseText: document.getElementById('responseText'),
        loadingDots: div(),
        mascotContainer: div(),
        expandBtn: div(),
        floatingStopBtn: div(),
        appSuggestions: div(),
        input: document.createElement('textarea'),
    };
    app.windowManager = {
        resizeWindow: vi.fn(async () => {}),
        resetHeightForNewMessage: vi.fn(async () => {}),
    };
    app.banner = { dismiss: vi.fn() };
    app.attachmentManager = { clear: vi.fn(), toContentBlocks: vi.fn(() => null) };
    app.stopThinking = vi.fn();
    app.startThinking = vi.fn();
    app.updateDatetimeVisibility = vi.fn();
    app._removeShellActivity = vi.fn();
    app._finishShellActivity = vi.fn();
    app._showFloatingResponseActions = vi.fn();
    app._showResponseActions = vi.fn();
    app._refreshContextUsage = vi.fn();
    app.floatingSessionId = null;
    app.isWaitingForResponse = false;
    const named = (name) => calls.filter(([c]) => c === name);
    const sends = () => named('send_message_streaming').map(([, a]) => a);
    return { app, starts, named, sends };
}

/**
 * The launch bootstrap failed with `error`; then the user types `message`
 * and presses Enter: the send retries the bootstrap, which fails again.
 */
async function failedBootstrapThenSend(app, error, message = 'hello agent') {
    app._setBootstrapFailure(error);
    app.elements.input.value = message;
    await app.sendChatMessage(message);
    // The send waits on the retry, polled every 100 ms.
    await wait(250);
    await flush();
}

const signInBtn = () => document.querySelector('#responseText .agent-sign-in-btn');

async function signInCleanly(starts) {
    signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
    await flush();
    starts[starts.length - 1].resolve({ outcome: 'exited', exitCode: 0 });
    await flush(20);
}

const status = () => document.querySelector('.agent-sign-in-status')?.textContent;

beforeEach(() => {
    _setTerminalFactoryForTests(fakeTerminal);
    _resetAgentSignInForTests();
    window._kageFloatingHidden = false;
});

afterEach(() => {
    _resetAgentSignInForTests();
    _setTerminalFactoryForTests(null);
    delete window.__TAURI__;
    delete window._kageFloatingHidden;
    document.body.innerHTML = '';
});

describe('floating: a signed-out agent refusing the bootstrap', () => {
    it('shows the error with Sign in beside Reconnect', async () => {
        const { app, sends } = setup({ bootstraps: [SIGNED_OUT] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        const text = document.querySelector('#responseText').textContent;
        expect(text).toContain('floating.error.agent_unavailable');
        const actions = document.querySelector('#responseText .floating-error-actions');
        expect(actions.querySelector('.agent-sign-in-btn')).toBeTruthy();
        expect(
            actions.querySelector('.floating-error-reconnect:not(.agent-sign-in-btn)')
        ).toBeTruthy();
        expect(app.sessionBootstrapFailure).toEqual(SIGNED_OUT);
        expect(sends()).toEqual([]);
    });

    it('a clean exit restarts, bootstraps, then sends the waiting message once', async () => {
        const { app, starts, named, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        await signInCleanly(starts);

        expect(named('reconnect_acp')).toEqual([['reconnect_acp', { sessionId: null }]]);
        // The retry on send, then the one after the sign-in.
        expect(named('switch_acp_session')).toEqual([
            ['switch_acp_session', { sessionId: null }],
            ['switch_acp_session', { sessionId: null }],
        ]);
        const order = (cmd) => app.invoke.mock.calls.findIndex(([c]) => c === cmd);
        const lastBootstrap = app.invoke.mock.calls.findLastIndex(
            ([c]) => c === 'switch_acp_session'
        );
        expect(order('reconnect_acp')).toBeLessThan(lastBootstrap);
        expect(lastBootstrap).toBeLessThan(order('send_message_streaming'));
        expect(sends()).toEqual([
            {
                sessionId: 's-new',
                message: 'hello agent',
                attachments: null,
                turnId: expect.any(String),
            },
        ]);
        expect(app.floatingSessionId).toBe('s-new');
        expect(app.sessionBootstrapError).toBeNull();
        expect(app.sessionBootstrapFailure).toBeNull();
        expect(status()).toBe('shared.agent_sign_in.status.done');

        // Nothing sends it again: the host refuses, and a later Reconnect
        // has nothing waiting.
        expect(app._bootstrapWaitingSend).toBeNull();
        await flush(10);
        expect(sends().length).toBe(1);
    });

    it('a bootstrap that still fails shows the error again with Sign in, and sends nothing', async () => {
        const { app, starts, named, sends } = setup({ bootstraps: [SIGNED_OUT] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        await signInCleanly(starts);

        expect(named('reconnect_acp').length).toBe(1);
        expect(named('switch_acp_session').length).toBe(2);
        expect(sends()).toEqual([]);
        expect(document.querySelector('#responseText').textContent).toContain(
            'floating.error.agent_unavailable'
        );
        expect(signInBtn()).toBeTruthy();
        expect(status()).toBe('shared.agent_sign_in.status.restart_failed');

        // Signing in again, and now the bootstrap works: the waiting
        // message still goes, once.
        const orig = app.invoke.getMockImplementation();
        app.invoke.mockImplementation(async (cmd, args) =>
            cmd === 'switch_acp_session' ? 's-later' : orig(cmd, args)
        );
        await signInCleanly(starts);
        expect(sends().map((a) => a.message)).toEqual(['hello agent']);
        expect(sends()[0].sessionId).toBe('s-later');
    });

    it('sends nothing when the user typed something else meanwhile', async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        app.elements.input.value = 'something else';
        await signInCleanly(starts);
        expect(app.floatingSessionId).toBe('s-new');
        expect(sends()).toEqual([]);
        expect(status()).toBe('shared.agent_sign_in.status.ready');
    });

    it('sends nothing when the user sent something else meanwhile', async () => {
        const { app, starts, sends, named } = setup({ bootstraps: [SIGNED_OUT] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();
        // A second send while kiro-cli login runs: its retry fails too
        // (past the debounce window).
        app._lastBootstrapRetryAt = 0;
        app.elements.input.value = 'second';
        await app.sendChatMessage('second');
        await wait(250);
        await flush();
        expect(named('switch_acp_session').length).toBe(2);
        const orig = app.invoke.getMockImplementation();
        app.invoke.mockImplementation(async (cmd, args) =>
            cmd === 'switch_acp_session' ? 's-new' : orig(cmd, args)
        );
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush(20);
        expect(app.floatingSessionId).toBe('s-new');
        // Neither the first message nor the second one went out by itself.
        expect(sends()).toEqual([]);
        expect(status()).toBe('shared.agent_sign_in.status.ready');
    });

    it('with no send waiting, just bootstraps and shows ready', async () => {
        const { app, starts, sends } = setup({ bootstraps: ['s-new'] });
        app._setBootstrapFailure(SIGNED_OUT);
        app._showBootstrapError(null);
        await flush();
        await signInCleanly(starts);
        expect(app.floatingSessionId).toBe('s-new');
        expect(sends()).toEqual([]);
        expect(status()).toBe('shared.agent_sign_in.status.ready');
    });

    it('Reconnect under the error does the same: restart, bootstrap, one send', async () => {
        const { app, named, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        const btn = document.querySelector(
            '#responseText .floating-error-reconnect:not(.agent-sign-in-btn)'
        );
        btn.click();
        await flush(20);
        expect(named('reconnect_acp')).toEqual([['reconnect_acp', { sessionId: null }]]);
        expect(sends().map((a) => [a.sessionId, a.message])).toEqual([['s-new', 'hello agent']]);
        expect(sends()[0].turnId).toEqual(expect.any(String));
    });
});

describe('floating: other bootstrap failures', () => {
    it('a plain-string bootstrap error gets no Sign in', async () => {
        const plain = 'Failed to create session: Connection lost';
        const { app, named, sends } = setup({ bootstraps: [plain] });
        await failedBootstrapThenSend(app, plain);
        expect(document.querySelector('#responseText').textContent).toContain(
            'floating.error.agent_unavailable'
        );
        expect(signInBtn()).toBeNull();
        expect(document.querySelector('#responseText .floating-error-reconnect')).toBeNull();
        expect(named('agent_sign_in_context').length).toBe(0);
        expect(app.sessionBootstrapFailure).toBeNull();
        expect(sends()).toEqual([]);
    });

    it('an error object of another kind gets no Sign in', async () => {
        const lost = { kind: 'connection_lost', message: 'Could not connect to the agent' };
        const { app } = setup({ bootstraps: [lost] });
        await failedBootstrapThenSend(app, lost);
        expect(signInBtn()).toBeNull();
    });
});

/** A promise and the functions that settle it. */
function deferred() {
    let resolve;
    let reject;
    const promise = new Promise((res, rej) => {
        resolve = res;
        reject = rej;
    });
    return { promise, resolve, reject };
}

/**
 * From now on `switch_acp_session` answers with a deferred each (`pending`,
 * in call order), counting how many are open at once (`maxOpen()`), and
 * `reconnect_acp` with `reconnect()` (a value or a promise).
 */
function controlBootstraps(app, reconnect) {
    const orig = app.invoke.getMockImplementation();
    const pending = [];
    let open = 0;
    let max = 0;
    app.invoke.mockImplementation(async (cmd, args) => {
        if (cmd === 'switch_acp_session') {
            orig(cmd, args).catch(() => {}); // recorded in `calls`
            const d = deferred();
            pending.push(d);
            open += 1;
            max = Math.max(max, open);
            try {
                return await d.promise;
            } finally {
                open -= 1;
            }
        }
        if (cmd === 'reconnect_acp') {
            await orig(cmd, args);
            return reconnect();
        }
        return orig(cmd, args);
    });
    return { pending, maxOpen: () => max };
}

const RESTARTED = { reconnected: true, sessionId: null, sessionReloaded: false };

describe('floating: one bootstrap at a time after a failed one', () => {
    it('a send during the sign-in restart shares its bootstrap: one session, sent once', async () => {
        const { app, starts, named, sends } = setup({ bootstraps: [SIGNED_OUT] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        const restart = deferred();
        const ctl = controlBootstraps(app, () => restart.promise);

        signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush();
        expect(status()).toBe('shared.agent_sign_in.status.restarting');

        // Enter while the panel says "Restarting…": the send retries the
        // bootstrap, and the restart's bootstrap must join that one.
        app._lastBootstrapRetryAt = 0;
        app.elements.input.value = 'second';
        await app.sendChatMessage('second');
        expect(ctl.pending.length).toBe(1);
        restart.resolve(RESTARTED);
        await flush(20);
        expect(ctl.pending.length).toBe(1);

        ctl.pending[0].resolve('s-a');
        await wait(250);
        await flush(20);

        expect(named('switch_acp_session').length).toBe(2); // the failed retry, then one
        expect(ctl.maxOpen()).toBe(1);
        expect(app.floatingSessionId).toBe('s-a');
        expect(app.bootstrappingSession).toBe(false);
        // The newer send went, once, on the one session; the older one not.
        expect(sends().map((a) => [a.sessionId, a.message])).toEqual([['s-a', 'second']]);
        // That send's turn cleared the panel, which had ended ready, not failed.
        expect(status()).toBeUndefined();
        expect(document.querySelector('#responseText').textContent).not.toContain(
            'floating.error.agent_unavailable'
        );
    });

    it('Sign in and Reconnect together: a superseded failure changes nothing, sent once', async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        const ctl = controlBootstraps(app, () => RESTARTED);

        signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();
        document
            .querySelector('#responseText .floating-error-reconnect:not(.agent-sign-in-btn)')
            .click();
        await flush(20);
        expect(ctl.pending.length).toBe(1);

        // The sign-in ends while Reconnect's bootstrap is still out: its
        // restart's bootstrap waits on that one rather than starting another.
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush(20);
        expect(ctl.pending.length).toBe(1);

        const shown = vi.spyOn(app, '_showBootstrapError');
        // The second restart killed it; the sign-in's bootstrap goes next.
        ctl.pending[0].reject({ kind: 'connection_lost', message: 'Connection lost' });
        await flush(20);
        expect(shown).not.toHaveBeenCalled();
        expect(ctl.pending.length).toBe(2);
        ctl.pending[1].resolve('s-b');
        await flush(20);

        expect(ctl.maxOpen()).toBe(1);
        expect(shown).not.toHaveBeenCalled();
        expect(app.floatingSessionId).toBe('s-b');
        expect(app.sessionBootstrapError).toBeNull();
        expect(sends().map((a) => [a.sessionId, a.message])).toEqual([['s-b', 'hello agent']]);
        expect(sends()[0].turnId).toEqual(expect.any(String));
        expect(status()).toBe('shared.agent_sign_in.status.done');
    });

    it('a second Reconnect while one is under way starts no second restart', async () => {
        const { app, named } = setup({ bootstraps: [SIGNED_OUT] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        const restart = deferred();
        controlBootstraps(app, () => restart.promise);
        const waiting = app._bootstrapWaitingSend;
        const first = app._reconnectBootstrap(document.createElement('button'), waiting);
        await app._reconnectBootstrap(document.createElement('button'), waiting);
        expect(named('reconnect_acp').length).toBe(1);
        restart.resolve({ reconnected: false });
        await first;
    });
});
