/**
 * Sign in under a failed floating bootstrap: after an update kiro-cli is
 * signed out and refuses `session/new` with `-32000`, which the backend
 * reports as `{ kind: 'auth_required' }`. The bootstrap's error then gets
 * Sign in (and Reconnect); a clean exit restarts the agent
 * (`reconnect_acp`, no session), bootstraps a session
 * (`switch_acp_session`) and only then sends the message that was waiting
 * on the bootstrap, exactly once, with a turn id of its own. A bootstrap
 * that still fails shows the error again, with Sign in, and sends nothing;
 * any edit the user made to the input since (an `input` event, or a write
 * of theirs that fires none: dictation, Ctrl+L) means nothing is sent, on
 * every replay path (a send queued on a bootstrap too), while the window's
 * own clears (after a send, voice mode listening again, resetUI) are no
 * edit; a send made meanwhile
 * that failed again is the one waiting, and the one sent; a plain-string
 * bootstrap error gets no Sign in.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { marked } from 'marked';
import { _resetAgentSignInForTests } from '../../ui/js/shared/agent-sign-in.js';
import { _setTerminalFactoryForTests } from '../../ui/js/shared/terminal-view.js';
import { AttachmentManager } from '../../ui/js/shared/attachments.js';
import { invalidateConfig } from '../../ui/js/shared/config-cache.js';

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
    // What setupEventListeners wires on the input: its edits are counted.
    app._watchInputEdits?.();
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

/** The user edits the input: its value changes and it fires `input`. */
function typeInto(app, value) {
    app.elements.input.value = value;
    app.elements.input.dispatchEvent(new Event('input', { bubbles: true }));
}

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
        typeInto(app, 'something else');
        await signInCleanly(starts);
        expect(app.floatingSessionId).toBe('s-new');
        expect(sends()).toEqual([]);
        expect(status()).toBe('shared.agent_sign_in.status.ready');
    });

    it('a send made meanwhile is the one waiting: it goes once, the first one not', async () => {
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
        // The second send failed again and is the one waiting now: it goes
        // out once; the first one, which it replaced, does not.
        expect(sends().map((a) => [a.sessionId, a.message])).toEqual([['s-new', 'second']]);
        expect(status()).toBe('shared.agent_sign_in.status.done');
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

    it('a sign-in that sends nothing clears the old error: the session is ready', async () => {
        const { app, starts, sends } = setup({ bootstraps: ['s-new'] });
        app._setBootstrapFailure(SIGNED_OUT);
        app._showBootstrapError(null);
        await flush();
        await signInCleanly(starts);
        expect(app.sessionBootstrapError).toBeNull();
        expect(sends()).toEqual([]);
        const response = document.querySelector('#responseText');
        expect(response.textContent).not.toContain('floating.error.agent_unavailable');
        expect(response.querySelector('.floating-error-reconnect')).toBeNull();
        expect(app.elements.contentArea.classList.contains('visible')).toBe(false);
        // The panel still says how the sign-in went.
        expect(status()).toBe('shared.agent_sign_in.status.ready');
    });

    it('a sign-in after the user typed something else clears the old error too', async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        typeInto(app, 'something else');
        await signInCleanly(starts);
        expect(sends()).toEqual([]);
        expect(document.querySelector('#responseText').textContent).not.toContain(
            'floating.error.agent_unavailable'
        );
        // The draft is the user's.
        expect(app.elements.input.value).toBe('something else');
    });

    it('a sign-in leaves alone what replaced the error meanwhile', async () => {
        const { app, starts } = setup({ bootstraps: ['s-new'] });
        app._setBootstrapFailure(SIGNED_OUT);
        app._showBootstrapError(null);
        await flush();
        signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();
        app.showError('a newer error');
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush(20);
        expect(app.floatingSessionId).toBe('s-new');
        expect(document.querySelector('#responseText').textContent).toContain('a newer error');
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

    it("a send queued on the sign-in's bootstrap, then a draft typed and erased: nothing is sent", async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        const ctl = controlBootstraps(app, () => RESTARTED);
        await signInCleanly(starts);
        // The sign-in's bootstrap is under way: a newer send is queued on it.
        expect(ctl.pending.length).toBe(1);
        expect(app.bootstrappingSession).toBe(true);
        typeInto(app, 'second');
        await app.sendChatMessage('second');
        // Then the user starts a replacement and erases it.
        typeInto(app, 'something else');
        typeInto(app, '');

        ctl.pending[0].resolve('s-a');
        await wait(250);
        await flush(20);
        expect(app.floatingSessionId).toBe('s-a');
        expect(app.bootstrappingSession).toBe(false);
        expect(sends()).toEqual([]);
    });

    it("a send queued on the sign-in's bootstrap with no edit since goes out once", async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        const ctl = controlBootstraps(app, () => RESTARTED);
        await signInCleanly(starts);
        typeInto(app, 'second');
        await app.sendChatMessage('second');
        ctl.pending[0].resolve('s-a');
        await wait(250);
        await flush(20);
        expect(sends().map((a) => [a.sessionId, a.message])).toEqual([['s-a', 'second']]);
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

    it('a Reconnect sends the send that redrew its error meanwhile, once', async () => {
        const { app, sends } = setup({ bootstraps: [SIGNED_OUT] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        const restart = deferred();
        const ctl = controlBootstraps(app, () => restart.promise);
        document
            .querySelector('#responseText .floating-error-reconnect:not(.agent-sign-in-btn)')
            .click();
        await flush();
        // Enter again within the debounce window: the same error, redrawn
        // (a new Reconnect button), for the newer send.
        app.elements.input.value = 'second';
        await app.sendChatMessage('second');
        await flush();
        expect(document.querySelector('#responseText').textContent).toContain(
            'floating.error.agent_unavailable'
        );
        restart.resolve(RESTARTED);
        await flush(20);
        ctl.pending[0].resolve('s-a');
        await flush(20);
        expect(app.floatingSessionId).toBe('s-a');
        // The newer send is the one waiting: it goes, once; the first not.
        expect(sends().map((a) => [a.sessionId, a.message])).toEqual([['s-a', 'second']]);
        expect(document.querySelector('#responseText').textContent).not.toContain(
            'floating.error.agent_unavailable'
        );
    });

    it('a second Reconnect while one is under way starts no second restart', async () => {
        const { app, named } = setup({ bootstraps: [SIGNED_OUT] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        const restart = deferred();
        controlBootstraps(app, () => restart.promise);
        const first = app._reconnectBootstrap(document.createElement('button'));
        await app._reconnectBootstrap(document.createElement('button'));
        expect(named('reconnect_acp').length).toBe(1);
        restart.resolve({ reconnected: false });
        await first;
    });
});

describe('floating: the waiting send is what was submitted', () => {
    const withRealAttachments = (app) => {
        app.attachmentManager = new AttachmentManager();
        return app.attachmentManager;
    };

    it('a file attached during the sign-in is not sent with the waiting message; it stays in the composer', async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        const attachments = withRealAttachments(app);
        await failedBootstrapThenSend(app, SIGNED_OUT);
        signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();
        // kiro-cli login runs; the user attaches a file for their next send.
        attachments.addFile('/tmp/later.txt', 'later.txt', 'text/plain');
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush(20);

        expect(sends()).toEqual([
            {
                sessionId: 's-new',
                message: 'hello agent',
                attachments: null,
                turnId: expect.any(String),
            },
        ]);
        expect(attachments.attachments.map((a) => a.name)).toEqual(['later.txt']);
        expect(status()).toBe('shared.agent_sign_in.status.done');
    });

    it('sends the attachments that were submitted, not ones added since; those stay', async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        const attachments = withRealAttachments(app);
        attachments.addFile('/tmp/first.txt', 'first.txt', 'text/plain');
        await failedBootstrapThenSend(app, SIGNED_OUT);
        signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();
        attachments.addFile('/tmp/later.txt', 'later.txt', 'text/plain');
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush(20);

        expect(sends().map((a) => [a.message, a.attachments?.map((b) => b.name)])).toEqual([
            ['hello agent', ['first.txt']],
        ]);
        expect(attachments.attachments.map((a) => a.name)).toEqual(['later.txt']);
    });

    it('Reconnect sends the submitted snapshot too', async () => {
        const { app, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        const attachments = withRealAttachments(app);
        await failedBootstrapThenSend(app, SIGNED_OUT);
        attachments.addFile('/tmp/later.txt', 'later.txt', 'text/plain');
        document
            .querySelector('#responseText .floating-error-reconnect:not(.agent-sign-in-btn)')
            .click();
        await flush(20);
        expect(sends().map((a) => [a.message, a.attachments])).toEqual([['hello agent', null]]);
        expect(attachments.attachments.map((a) => a.name)).toEqual(['later.txt']);
    });

    it('a send queued while the bootstrap runs goes out as submitted', async () => {
        const { app, sends } = setup({ bootstraps: ['s-new'] });
        const attachments = withRealAttachments(app);
        app.bootstrappingSession = true;
        app.elements.input.value = 'queued';
        await app.sendChatMessage('queued');
        attachments.addFile('/tmp/later.txt', 'later.txt', 'text/plain');
        app.floatingSessionId = 's-new';
        app.bootstrappingSession = false;
        await wait(250);
        await flush(20);
        expect(sends().map((a) => [a.sessionId, a.message, a.attachments])).toEqual([
            ['s-new', 'queued', null],
        ]);
        expect(attachments.attachments.map((a) => a.name)).toEqual(['later.txt']);
    });
});

describe('floating: an edit to the input cancels the waiting send', () => {
    it('a draft typed and then erased: nothing is sent, the sign-in ends ready', async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();
        typeInto(app, 'a different question');
        typeInto(app, '');
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush(20);
        expect(app.floatingSessionId).toBe('s-new');
        expect(sends()).toEqual([]);
        expect(status()).toBe('shared.agent_sign_in.status.ready');
    });

    it('an edit back to the very same text: nothing is sent', async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        typeInto(app, 'hello agen');
        typeInto(app, 'hello agent');
        await signInCleanly(starts);
        expect(sends()).toEqual([]);
        expect(status()).toBe('shared.agent_sign_in.status.ready');
        expect(app.elements.input.value).toBe('hello agent');
    });

    it('a paste: nothing is sent', async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        // A text paste, as the browser reports it: the value grows and
        // the input fires `input` (insertFromPaste).
        app.elements.input.value += ' and more';
        app.elements.input.dispatchEvent(
            new InputEvent('input', { bubbles: true, inputType: 'insertFromPaste' })
        );
        await signInCleanly(starts);
        expect(sends()).toEqual([]);
    });

    it('Reconnect after an edit sends nothing either', async () => {
        const { app, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        typeInto(app, '');
        document
            .querySelector('#responseText .floating-error-reconnect:not(.agent-sign-in-btn)')
            .click();
        await flush(20);
        expect(app.floatingSessionId).toBe('s-new');
        expect(sends()).toEqual([]);
    });

    it('no edit: the waiting message still goes, once, and the input empties', async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        await signInCleanly(starts);
        expect(sends().map((a) => a.message)).toEqual(['hello agent']);
        expect(app.elements.input.value).toBe('');
    });
});

describe('floating: the launch bootstrap keeps its typed error', () => {
    it('start_session_on_launch=false: a send queued on the pending bootstrap gets Sign in and Reconnect', async () => {
        invalidateConfig();
        const { app, starts, named, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        const orig = app.invoke.getMockImplementation();
        let refuse;
        let first = true;
        app.invoke.mockImplementation(async (cmd, args) => {
            if (cmd === 'get_config') return { acp: { agent: { start_session_on_launch: false } } };
            if (cmd === 'switch_acp_session' && first) {
                first = false;
                orig(cmd, args).catch(() => {}); // recorded in `calls`
                return new Promise((_, reject) => (refuse = reject));
            }
            return orig(cmd, args);
        });
        const adopting = app._adoptFloatingSession();
        await flush();
        expect(app.bootstrappingSession).toBe(true);
        app.elements.input.value = 'hello agent';
        await app.sendChatMessage('hello agent');
        refuse(SIGNED_OUT);
        await adopting;
        await wait(250);
        await flush();

        const text = document.querySelector('#responseText').textContent;
        expect(text).toContain('floating.error.agent_unavailable');
        expect(text).not.toContain('[object Object]');
        expect(app.sessionBootstrapFailure).toEqual(SIGNED_OUT);
        expect(signInBtn()).toBeTruthy();
        expect(
            document.querySelector(
                '#responseText .floating-error-reconnect:not(.agent-sign-in-btn)'
            )
        ).toBeTruthy();
        expect(sends()).toEqual([]);

        // And the sign-in sends the waiting message once.
        await signInCleanly(starts);
        expect(named('reconnect_acp').length).toBe(1);
        expect(sends().map((a) => [a.sessionId, a.message])).toEqual([['s-new', 'hello agent']]);
        invalidateConfig();
    });

    it('the launch pin failing, then its own session/new refused: Sign in too', async () => {
        invalidateConfig();
        const { app, sends } = setup({ bootstraps: [SIGNED_OUT] });
        const handlers = {};
        app.listen = vi.fn(async (evt, cb) => {
            handlers[evt] = cb;
            return () => {};
        });
        const adopting = app._adoptFloatingSession();
        await flush();
        app.elements.input.value = 'hello agent';
        await app.sendChatMessage('hello agent');
        handlers.session_pin_failed({ payload: { label: 'floating', reason: 'auth' } });
        await adopting;
        await wait(250);
        await flush();
        expect(app.sessionBootstrapFailure).toEqual(SIGNED_OUT);
        expect(signInBtn()).toBeTruthy();
        expect(app.bootstrappingSession).toBe(false);
        expect(sends()).toEqual([]);
        invalidateConfig();
    });
});

describe("floating: the user's writes that fire no `input` event are edits, the window's own are not", () => {
    /**
     * A fake Web Speech recognizer: `SpeechController.start()` builds one,
     * and `say(text, isFinal)` delivers a result the way the browser does.
     */
    function fakeRecognition() {
        const made = [];
        class FakeRecognition {
            constructor() {
                made.push(this);
            }
            start() {}
            stop() {
                this.onend?.();
            }
        }
        window.SpeechRecognition = FakeRecognition;
        return {
            say(text, isFinal = true) {
                const rec = made[made.length - 1];
                const result = Object.assign([{ transcript: text }], { isFinal });
                rec.onresult({ resultIndex: 0, results: [result] });
            },
            /** The recognizer ends by itself, as the browser's does. */
            end() {
                made[made.length - 1].onend?.();
            },
            started: () => made.length,
        };
    }

    afterEach(() => {
        delete window.SpeechRecognition;
        delete window.speechSynthesis;
    });

    it('voice dictation during the sign-in: nothing is sent, and the dictated draft stays', async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        app.setupSpeech();
        // No auto-submit on silence: the transcript stays in the input.
        app.speech.silenceTimeout = 0;
        const mic = fakeRecognition();
        await failedBootstrapThenSend(app, SIGNED_OUT);
        signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();

        // While kiro-cli login runs: the user dictates a replacement, then
        // stops the mic before it is sent.
        app.speech.start();
        mic.say('a dictated question');
        app.speech.stop();
        expect(app.elements.input.value).toBe('a dictated question');

        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush(20);
        expect(app.floatingSessionId).toBe('s-new');
        expect(sends()).toEqual([]);
        expect(app.elements.input.value).toBe('a dictated question');
        expect(status()).toBe('shared.agent_sign_in.status.ready');
    });

    it('Ctrl+L during the sign-in: the old message is not sent', async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();
        const key = new KeyboardEvent('keydown', { key: 'l', ctrlKey: true, cancelable: true });
        app.handleGlobalKeyDown(key);
        expect(key.defaultPrevented).toBe(true);
        expect(app.elements.input.value).toBe('');
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush(20);
        expect(app.floatingSessionId).toBe('s-new');
        expect(sends()).toEqual([]);
    });

    it("the window's own reset during the sign-in (resetUI, as on hide) is no edit: still sent", async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();
        app.resetUI();
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush(20);
        expect(sends().map((a) => [a.sessionId, a.message])).toEqual([['s-new', 'hello agent']]);
    });

    it("voice mode: the mic's restart clear after a dictated send is no edit: it still goes after the sign-in", async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        // jsdom has no text-to-speech; the send cancels any playback.
        window.speechSynthesis = { cancel: vi.fn() };
        app.setupSpeech();
        app.speech.silenceTimeout = 0;
        const mic = fakeRecognition();
        app._setBootstrapFailure(SIGNED_OUT);
        // Voice mode: the user dictates, the recognizer ends, and the
        // transcript is sent; the bootstrap's retry is refused.
        app.speech.voiceMode = true;
        app.speech.start();
        mic.say('hello agent');
        mic.end();
        await wait(250);
        await flush();
        expect(signInBtn()).toBeTruthy();
        // Voice mode then restarts the mic for the next utterance, and
        // empties the input itself.
        await wait(150);
        expect(mic.started()).toBe(2);
        expect(app.elements.input.value).toBe('');

        await signInCleanly(starts);
        expect(sends().map((a) => [a.sessionId, a.message])).toEqual([['s-new', 'hello agent']]);
        expect(status()).toBe('shared.agent_sign_in.status.done');
        app.speech.stopVoiceMode();
    });

    it('Enter empties the input after the send it left waiting: still sent after the sign-in', async () => {
        const { app, starts, sends } = setup({ bootstraps: [SIGNED_OUT, 's-new'] });
        app.attachmentManager = new AttachmentManager();
        app.shortcuts = [];
        app._getExecCtx = () => ({});
        app.clearSuggestions = vi.fn(async () => {});
        app._setBootstrapFailure(SIGNED_OUT);
        typeInto(app, 'hello agent');
        await app.handleEnterKey();
        await wait(250);
        await flush();
        expect(signInBtn()).toBeTruthy();
        // The Enter path emptied the input itself, as it does after a send.
        expect(app.elements.input.value).toBe('');
        await signInCleanly(starts);
        expect(sends().map((a) => [a.sessionId, a.message])).toEqual([['s-new', 'hello agent']]);
    });
});

describe('floating: the send waiting when the sign-in ends is the one sent', () => {
    it('Enter again during the login (its retry refused too): that send goes once after it', async () => {
        const { app, starts, sends, named } = setup({ bootstraps: [SIGNED_OUT] });
        await failedBootstrapThenSend(app, SIGNED_OUT);
        signInBtn().dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        await flush();
        // Past the debounce: the send retries the bootstrap, which is
        // refused again and redraws the error for it.
        app._lastBootstrapRetryAt = 0;
        await app.sendChatMessage('hello agent');
        await wait(250);
        await flush();
        expect(named('switch_acp_session').length).toBe(2);
        expect(signInBtn()).toBeTruthy();
        const orig = app.invoke.getMockImplementation();
        app.invoke.mockImplementation(async (cmd, args) =>
            cmd === 'switch_acp_session' ? 's-new' : orig(cmd, args)
        );
        starts[0].resolve({ outcome: 'exited', exitCode: 0 });
        await flush(20);
        expect(app.floatingSessionId).toBe('s-new');
        expect(sends().map((a) => [a.sessionId, a.message])).toEqual([['s-new', 'hello agent']]);
        expect(status()).toBe('shared.agent_sign_in.status.done');
        // Its own Sign in / Reconnect have nothing left to send.
        expect(app._bootstrapWaitingSend).toBeNull();
    });
});
