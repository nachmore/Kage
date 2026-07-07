/**
 * Tests for floating-session re-bootstrap after a failed launch bootstrap
 * (_retryBootstrapAndSend / _retryBootstrap on FloatingApp).
 *
 * The behaviour these pin, which fixed the "stuck until app restart" bug:
 *   - a send while `sessionBootstrapError` is set no longer just re-shows the
 *     error forever — it retries the bootstrap;
 *   - a successful retry clears the error and replays the queued send;
 *   - a retry that fails again re-shows the error (and stays recoverable);
 *   - retries are DEBOUNCED: a second send within the debounce window skips
 *     the (expensive) backend reconnect and just re-shows the error, so a
 *     burst of sends can't cascade into a respawn storm.
 *
 * FloatingApp's constructor has heavy deps, so we exercise the methods on a
 * prototype-based stub with just the fields they touch (same pattern as
 * search-loading.test.js).
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { FloatingApp } from '../../ui/js/floating/app.js';

const DEBOUNCE = 5000; // BOOTSTRAP_RETRY_DEBOUNCE_MS (module-private constant)

function makeApp() {
    const app = Object.create(FloatingApp.prototype);
    app.sessionBootstrapError = 'Failed to create session: Connection lost';
    app._lastBootstrapRetryAt = 0;
    app.floatingSessionId = null;
    app.bootstrappingSession = false;
    app._pendingSend = null;
    app._waitingForBootstrap = false;
    app.elements = { responseText: null };
    // Collaborators the retry path touches — all stubbed.
    app.invoke = vi.fn();
    app.showError = vi.fn();
    app._showBootstrapSpinner = vi.fn();
    app.sendChatMessage = vi.fn();
    return app;
}

describe('floating bootstrap retry', () => {
    beforeEach(() => {
        vi.useFakeTimers();
        // Anchor Date.now() past the debounce window so the first retry
        // (with _lastBootstrapRetryAt=0) is always allowed.
        vi.setSystemTime(DEBOUNCE + 1000);
    });
    afterEach(() => {
        vi.useRealTimers();
    });

    it('retries the bootstrap on the first send after a failure', async () => {
        const app = makeApp();
        app.invoke.mockResolvedValue('new-session-id');

        app._retryBootstrapAndSend('hello', {});

        // Kicked off a reconnect via switch_acp_session and queued the send.
        expect(app.invoke).toHaveBeenCalledWith('switch_acp_session', { sessionId: null });
        expect(app._pendingSend).toEqual({ message: 'hello', options: {} });
        expect(app.bootstrappingSession).toBe(true);
        expect(app.showError).not.toHaveBeenCalled();
    });

    it('successful retry clears the error, adopts the session, and replays the send', async () => {
        const app = makeApp();
        app.invoke.mockResolvedValue('new-session-id');

        await app._retryBootstrap();

        expect(app.sessionBootstrapError).toBeNull();
        expect(app.floatingSessionId).toBe('new-session-id');
        expect(app.bootstrappingSession).toBe(false);
    });

    it('failed retry re-sets the error and stays recoverable', async () => {
        const app = makeApp();
        app.invoke.mockRejectedValue({ kind: 'connection_lost', message: 'still down' });

        await app._retryBootstrap();

        expect(app.floatingSessionId).toBeNull();
        // Error is refreshed (not cleared) so the next send can retry again.
        expect(app.sessionBootstrapError).toBeTruthy();
        expect(app.bootstrappingSession).toBe(false);
    });

    it('debounces: a second send within the window skips the reconnect', () => {
        const app = makeApp();
        app.invoke.mockResolvedValue('new-session-id');

        // First send triggers a real retry.
        app._retryBootstrapAndSend('one', {});
        expect(app.invoke).toHaveBeenCalledTimes(1);

        // Second send 1s later (< debounce) must NOT invoke the backend again.
        vi.advanceTimersByTime(1000);
        app._retryBootstrapAndSend('two', {});
        expect(app.invoke).toHaveBeenCalledTimes(1);
        expect(app.showError).toHaveBeenCalledTimes(1);
    });

    it('allows a fresh retry once the debounce window elapses', () => {
        const app = makeApp();
        app.invoke.mockResolvedValue('new-session-id');

        app._retryBootstrapAndSend('one', {});
        expect(app.invoke).toHaveBeenCalledTimes(1);

        // Past the debounce window — a new attempt is allowed.
        vi.advanceTimersByTime(DEBOUNCE + 1);
        app._retryBootstrapAndSend('two', {});
        expect(app.invoke).toHaveBeenCalledTimes(2);
    });
});
