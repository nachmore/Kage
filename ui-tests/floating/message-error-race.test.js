/**
 * A prompt's MESSAGE_ERROR is handled after an online probe that can take
 * seconds. A newer turn the floating window started meanwhile (a held
 * follow-up released when the failed turn settled) must not be torn down
 * by the old error, nor become its retry: Reconnect would send it again
 * after it had succeeded.
 *
 * FloatingApp's constructor has heavy deps, so the methods run on a
 * prototype stub (same pattern as reconnect-resend.test.js).
 */
import { describe, it, expect, vi, beforeAll, beforeEach, afterEach } from 'vitest';

let probe;
vi.mock('../../ui/js/shared/network.js', async (importOriginal) => ({
    ...(await importOriginal()),
    checkOnError: () => probe.promise,
}));

import { FloatingApp } from '../../ui/js/floating/app.js';
import { MessageStreamController } from '../../ui/js/shared/message-stream-controller.js';
import * as i18n from '../../ui/js/shared/i18n.js';
import { EVT } from '../../ui/js/shared/events.js';
import { initEnglish } from '../helpers/sync-i18n.js';

function deferred() {
    let resolve;
    const promise = new Promise((r) => {
        resolve = r;
    });
    return { promise, resolve };
}

function promptFailure(sessionId = 's1', turnId = undefined) {
    return {
        kind: 'agent_unreachable',
        key: 'errors.agent.unreachable_after_restart_kiro',
        message: 'Kage restarted Kiro and tried again, but it still failed.',
        detail: 'dispatch failure',
        sessionId,
        resendable: true,
        turnId,
    };
}

/** The turn id the latest send_message_streaming carried. */
function lastTurnId(app) {
    return app.invoke.mock.calls.filter(([cmd]) => cmd === 'send_message_streaming').at(-1)?.[1]
        ?.turnId;
}

beforeAll(async () => {
    await initEnglish(i18n);
});
beforeEach(() => {
    probe = deferred();
    window.__TAURI__ = { event: { emit: vi.fn() } };
});
afterEach(() => {
    document.body.innerHTML = '';
    delete window.__TAURI__;
});

function makeApp() {
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
    // The window's controller, as FloatingApp's constructor wires it.
    app.messageStreamController = new MessageStreamController({
        errorContext: () => app._errorContext(),
        onError: (event, online, context) => app._onMessageError(event, online, context),
    });
    return { app, listeners };
}

async function flush() {
    for (let i = 0; i < 10; i++) await new Promise((r) => setTimeout(r, 0));
}

function sentMessages(app) {
    return app.invoke.mock.calls
        .filter(([cmd]) => cmd === 'send_message_streaming')
        .map(([, args]) => args.message);
}

/** handleInput's agent path: hold for prior turns, then start and send. */
async function userSends(app, message) {
    if (!(await app._awaitPriorTurnsSettled())) return;
    await app._startResponseTurn();
    await app._sendTrackedTurn({ sessionId: 's1', message, attachments: null });
}

describe('floating MESSAGE_ERROR handled after a newer turn started', () => {
    it('neither tears down nor resends the newer turn', async () => {
        const { app, listeners } = makeApp();
        await userSends(app, 'A');
        const turnA = lastTurnId(app);
        // A is recovering; the user sends C, which is held behind it (and
        // handleInput drops A's waiting state as it cancels A).
        app.isWaitingForResponse = false;
        const c = userSends(app, 'C');
        await flush();
        expect(sentMessages(app)).toEqual(['A']);

        // A's terminal events: MESSAGE_ERROR (handled after the probe),
        // then SESSION_ACTIVITY failed, which releases C.
        const handled = app.handleMessageError({ payload: promptFailure('s1', turnA) });
        listeners[EVT.SESSION_ACTIVITY]({ payload: { sessionId: 's1', kind: 'failed' } });
        await c;
        await flush();
        expect(sentMessages(app)).toEqual(['A', 'C']);

        probe.resolve(true);
        await handled;
        await flush();

        // C is still the turn on screen, waiting for its reply.
        expect(app.isWaitingForResponse).toBe(true);
        expect(document.querySelector('.floating-error-reconnect')).toBeNull();

        // C completes; a stray Reconnect (if any) must not send C again.
        listeners[EVT.MESSAGE_COMPLETE]({ payload: { sessionId: 's1' } });
        document.querySelector('.floating-error-reconnect')?.click();
        await flush();
        expect(sentMessages(app)).toEqual(['A', 'C']);
    });

    it('still handles the error of the turn on screen after the probe', async () => {
        const { app, listeners } = makeApp();
        await userSends(app, 'A');

        const handled = app.handleMessageError({ payload: promptFailure('s1', lastTurnId(app)) });
        listeners[EVT.SESSION_ACTIVITY]({ payload: { sessionId: 's1', kind: 'failed' } });
        probe.resolve(true);
        await handled;
        await flush();

        expect(app.isWaitingForResponse).toBe(false);
        document.querySelector('.floating-error-reconnect').click();
        await flush();
        expect(sentMessages(app)).toEqual(['A', 'A']);
    });
});
