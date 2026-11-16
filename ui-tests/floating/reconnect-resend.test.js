/**
 * Reconnect under a failed prompt in the floating window: it restarts the
 * agent (reconnect_acp) and then resends the failed turn, but only when the
 * backend's error says it is `resendable` for that turn's session, nothing
 * of it showed here, it wasn't a rate limit, no newer prompt went out since,
 * and the window still holds that session. The resend isn't a new message: no
 * floating_message_sent (the chat window already has the bubble) and the
 * input box is left alone.
 *
 * FloatingApp's constructor has heavy deps, so the methods run on a
 * prototype stub (same pattern as bootstrap-retry.test.js).
 */
import { describe, it, expect, vi, beforeAll, beforeEach, afterEach } from 'vitest';
import { FloatingApp } from '../../ui/js/floating/app.js';
import * as i18n from '../../ui/js/shared/i18n.js';
import { EVT } from '../../ui/js/shared/events.js';
import { initEnglish } from '../helpers/sync-i18n.js';

const INCIDENT =
    'Failed to send: ACP error: Internal error — Encountered an error in the response stream: An unknown error occurred: dispatch failure';
const IMAGE = { type: 'image', mimeType: 'image/png', data: 'AAAA' };

/**
 * The backend's object form for a prompt that failed: `resendable` is true
 * only when its turn progress shows nothing got through (no text, tool,
 * permission request, fs/terminal request or cancel).
 */
function promptFailure(sessionId = 's1', resendable = true) {
    return {
        kind: 'agent_unreachable',
        key: 'errors.agent.unreachable_after_restart_kiro',
        message: 'Kage restarted Kiro and tried again, but it still failed.',
        detail: INCIDENT,
        sessionId,
        resendable,
    };
}

beforeAll(async () => {
    await initEnglish(i18n);
});

let emit;
beforeEach(() => {
    emit = vi.fn();
    window.__TAURI__ = { event: { emit } };
});
afterEach(() => {
    document.body.innerHTML = '';
    delete window.__TAURI__;
});

function makeApp({ reconnect } = {}) {
    document.body.innerHTML = `
        <div id="contentArea"><div id="responseText"></div></div>
        <button id="expandBtn"></button>
        <button id="stopBtn"></button>
        <textarea id="input"></textarea>`;
    const app = Object.create(FloatingApp.prototype);
    const listeners = {};
    app.listen = vi.fn(async (evt, cb) => {
        listeners[evt] = cb;
        return () => {};
    });
    app.invoke = vi.fn(async (cmd, args) => {
        if (cmd === 'reconnect_acp') {
            if (reconnect) return reconnect();
            return { reconnected: true, sessionId: args.sessionId, sessionReloaded: true };
        }
        return null;
    });
    app.elements = {
        responseText: document.getElementById('responseText'),
        contentArea: document.getElementById('contentArea'),
        expandBtn: document.getElementById('expandBtn'),
        floatingStopBtn: document.getElementById('stopBtn'),
        input: document.getElementById('input'),
    };
    app.windowManager = {
        resizeWindow: vi.fn(async () => {}),
        resetHeightForNewMessage: vi.fn(async () => {}),
    };
    for (const m of [
        'startThinking',
        'stopThinking',
        'updateDatetimeVisibility',
        '_finishShellActivity',
        '_removeShellActivity',
    ]) {
        app[m] = vi.fn();
    }
    app._noBlurTools = new Set();
    app.extensionToolController = { reset: vi.fn() };
    app.automationPlanController = { reset: vi.fn() };
    app.banner = { dismiss: vi.fn() };
    app._promptGeneration = 0;
    app.floatingSessionId = 's1';
    app.currentResponse = '';
    app.toolUsages = [];
    app.bootstrappingSession = false;
    app.sessionBootstrapError = null;
    app.isWaitingForResponse = false;
    return { app, listeners };
}

async function flush() {
    for (let i = 0; i < 10; i++) await new Promise((r) => setTimeout(r, 0));
}

/** Send a turn the way handleInput does, then fail it with `payload`. */
async function sendAndFail(app, listeners, payload, { output = '' } = {}) {
    await app._startResponseTurn();
    await app._sendTrackedTurn({ sessionId: 's1', message: 'hello', attachments: [IMAGE] });
    app.currentResponse = output;
    // The backend's terminal events for the failed turn.
    listeners[EVT.SESSION_ACTIVITY]({ payload: { sessionId: 's1', kind: 'failed' } });
    await app._onMessageError({ payload }, true);
    await flush();
}

function sends(app) {
    return app.invoke.mock.calls.filter(([cmd]) => cmd === 'send_message_streaming');
}

function reconnectBtn() {
    return document.querySelector('.floating-error-reconnect');
}

describe('floating Reconnect after a failed prompt', () => {
    it('reconnects, then resends the same turn without a new message', async () => {
        const { app, listeners } = makeApp();
        await sendAndFail(app, listeners, promptFailure());
        app.elements.input.value = 'a draft';
        emit.mockClear();

        reconnectBtn().click();
        await flush();

        expect(app.invoke).toHaveBeenCalledWith('reconnect_acp', { sessionId: 's1' });
        expect(sends(app)).toHaveLength(2);
        expect(sends(app)[1][1]).toEqual({
            sessionId: 's1',
            message: 'hello',
            attachments: [IMAGE],
        });
        const order = app.invoke.mock.calls.map(([cmd]) => cmd);
        expect(order.lastIndexOf('reconnect_acp')).toBeLessThan(
            order.lastIndexOf('send_message_streaming')
        );
        expect(emit).not.toHaveBeenCalledWith('floating_message_sent', expect.anything());
        expect(app.elements.input.value).toBe('a draft');
        expect(app.isWaitingForResponse).toBe(true);
    });

    it('only reconnects when part of the turn already showed', async () => {
        const { app, listeners } = makeApp();
        await sendAndFail(app, listeners, promptFailure(), { output: 'Partial answer' });

        reconnectBtn().click();
        await flush();

        expect(app.invoke).toHaveBeenCalledWith('reconnect_acp', { sessionId: 's1' });
        expect(sends(app)).toHaveLength(1);
        expect(app.elements.responseText.textContent).toBe('');
        expect(app.elements.contentArea.classList.contains('visible')).toBe(false);
    });

    it('does not resend once a newer prompt went out', async () => {
        const { app, listeners } = makeApp();
        await sendAndFail(app, listeners, promptFailure());
        app._promptGeneration++;

        reconnectBtn().click();
        await flush();

        expect(app.invoke).toHaveBeenCalledWith('reconnect_acp', { sessionId: 's1' });
        expect(sends(app)).toHaveLength(1);
    });

    it('keeps the error and re-enables the button when reconnect fails', async () => {
        const { app, listeners } = makeApp({
            reconnect: () =>
                Promise.reject({
                    kind: 'auth_required',
                    key: 'errors.agent.auth_required_kiro',
                    message: 'Kiro needs you to sign in. Run kiro-cli login in a terminal, then click Reconnect.',
                }),
        });
        await sendAndFail(app, listeners, INCIDENT);

        reconnectBtn().click();
        await flush();

        const text = app.elements.responseText.textContent;
        expect(text).toContain('dispatch failure');
        expect(text).toContain('Reconnection failed: Kiro needs you to sign in');
        expect(text).toContain('kiro-cli login');
        expect(text).not.toContain('[object Object]');
        expect(reconnectBtn().disabled).toBe(false);
        expect(reconnectBtn().textContent).toBe('Reconnect');
        expect(sends(app)).toHaveLength(1);
    });

    it('renders a structured payload with its details line', async () => {
        const { app, listeners } = makeApp();
        await sendAndFail(app, listeners, {
            kind: 'agent_unreachable',
            key: 'errors.agent.unreachable_after_restart_kiro',
            message: 'Kage restarted Kiro and tried again, but it still failed.',
            detail: 'ACP error: Internal error — dispatch failure',
            sessionId: 's1',
        });

        const text = app.elements.responseText.textContent;
        expect(text).toContain('Kage restarted Kiro and tried again, but it still failed.');
        const detail = app.elements.responseText.querySelector('.error-detail');
        expect(detail.textContent).toBe('Details: ACP error: Internal error — dispatch failure');
        expect(detail.dir).toBe('ltr');
        expect(text).not.toContain('[object Object]');
        expect(text).not.toMatch(/^Error: /);
    });

    it('adopts a new session id from reconnect and resends to it', async () => {
        const { app, listeners } = makeApp({
            reconnect: async () => ({ reconnected: true, sessionId: 's2', sessionReloaded: false }),
        });
        await sendAndFail(app, listeners, promptFailure());

        reconnectBtn().click();
        await flush();

        expect(app.floatingSessionId).toBe('s2');
        expect(app.invoke).toHaveBeenCalledWith('set_window_session', {
            label: 'floating',
            sessionId: 's2',
        });
        expect(sends(app)).toHaveLength(2);
        expect(sends(app)[1][1].sessionId).toBe('s2');
    });

    it('never resends a rate-limited prompt', async () => {
        const { app, listeners } = makeApp();
        await sendAndFail(app, listeners, {
            ...promptFailure(),
            kind: 'rate_limited',
            message: 'Rate limit exceeded. Please wait a moment.',
        });

        reconnectBtn().click();
        await flush();

        expect(app.invoke).toHaveBeenCalledWith('reconnect_acp', { sessionId: 's1' });
        expect(sends(app)).toHaveLength(1);
    });

    // F3: the backend saw progress the window didn't (a permission request
    // the user approved, an fs/terminal request), so it says not resendable.
    it('does not resend when the backend says part of the turn got through', async () => {
        const { app, listeners } = makeApp();
        await sendAndFail(app, listeners, promptFailure('s1', false));

        reconnectBtn().click();
        await flush();

        expect(app.invoke).toHaveBeenCalledWith('reconnect_acp', { sessionId: 's1' });
        expect(sends(app)).toHaveLength(1);
    });

    it('never resends on a plain-string error (no session, no progress record)', async () => {
        const { app, listeners } = makeApp();
        await sendAndFail(app, listeners, INCIDENT);

        reconnectBtn().click();
        await flush();

        expect(app.invoke).toHaveBeenCalledWith('reconnect_acp', { sessionId: 's1' });
        expect(sends(app)).toHaveLength(1);
    });

    it('does not resend when the payload does not say it is resendable', async () => {
        const { app, listeners } = makeApp();
        const { resendable: _omitted, ...noFlag } = promptFailure();
        await sendAndFail(app, listeners, noFlag);

        reconnectBtn().click();
        await flush();

        expect(sends(app)).toHaveLength(1);
    });

    // F2: A fails on s1 after the window moved to s2 and sent B there.
    it("another session's error neither stops the shown turn nor becomes its retry", async () => {
        const { app } = makeApp();
        await app._startResponseTurn();
        await app._sendTrackedTurn({ sessionId: 's1', message: 'A', attachments: null });
        app.floatingSessionId = 's2';
        await app._startResponseTurn();
        await app._sendTrackedTurn({ sessionId: 's2', message: 'B', attachments: null });

        // A's error arrives before B has any output.
        await app._onMessageError({ payload: promptFailure('s1') }, true);
        await flush();

        expect(app.isWaitingForResponse).toBe(true);
        expect(reconnectBtn()).toBeNull();
        expect(sends(app).map(([, a]) => a.message)).toEqual(['A', 'B']);
    });

    it('matches an error naming the pre-migration id to the migrated turn', async () => {
        const { app, listeners } = makeApp();
        await app._startResponseTurn();
        await app._sendTrackedTurn({ sessionId: 's1', message: 'hello', attachments: null });
        // As the session_migrated listener does for this window's session.
        app._followTurnSession('s1', 's7');
        app.floatingSessionId = 's7';
        listeners[EVT.SESSION_ACTIVITY]({ payload: { sessionId: 's1', kind: 'failed' } });
        await app._onMessageError({ payload: promptFailure('s1') }, true);
        await flush();
        expect(app.isWaitingForResponse).toBe(false);

        reconnectBtn().click();
        await flush();

        expect(sends(app)).toHaveLength(2);
        expect(sends(app)[1][1].sessionId).toBe('s7');
    });

    // F6: Reconnect started on s1, the window moved to s2 before it finished.
    it('does not repin or resend when the window moved during Reconnect', async () => {
        let finish;
        const { app, listeners } = makeApp({
            reconnect: () =>
                new Promise((r) => {
                    finish = r;
                }),
        });
        await sendAndFail(app, listeners, promptFailure());
        reconnectBtn().click();
        await flush();
        app.floatingSessionId = 's2';
        finish({ reconnected: true, sessionId: 's1-new', sessionReloaded: false });
        await flush();

        expect(app.floatingSessionId).toBe('s2');
        const pins = app.invoke.mock.calls.filter(([cmd]) => cmd === 'set_window_session');
        expect(pins.every(([, a]) => a.sessionId === 's2')).toBe(true);
        expect(sends(app)).toHaveLength(1);
    });

    // F7 (floating): a refused Reconnect leaves the button, and the failed
    // turn, for the next try.
    it('a failed Reconnect keeps the failed turn for the next one, resent once', async () => {
        let calls = 0;
        const { app, listeners } = makeApp({
            reconnect: async () => {
                calls++;
                if (calls === 1) {
                    throw {
                        kind: 'connection_lost',
                        message: 'The agent is still working. Try again when it finishes.',
                    };
                }
                return { reconnected: true, sessionId: 's1', sessionReloaded: true };
            },
        });
        await sendAndFail(app, listeners, promptFailure());

        reconnectBtn().click();
        await flush();
        expect(sends(app)).toHaveLength(1);

        reconnectBtn().click();
        await flush();
        expect(sends(app)).toHaveLength(2);
        expect(reconnectBtn()).toBeNull();
    });

    it('a Reconnect that reports not reconnected is a failure, not a resend', async () => {
        let calls = 0;
        const { app, listeners } = makeApp({
            reconnect: async () => {
                calls++;
                return calls === 1
                    ? { reconnected: false, sessionId: null, sessionReloaded: false }
                    : { reconnected: true, sessionId: 's1', sessionReloaded: true };
            },
        });
        await sendAndFail(app, listeners, promptFailure());

        reconnectBtn().click();
        await flush();
        expect(sends(app)).toHaveLength(1);
        expect(reconnectBtn()).not.toBeNull();

        reconnectBtn().click();
        await flush();
        expect(sends(app)).toHaveLength(2);
    });
});
