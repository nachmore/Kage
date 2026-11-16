import {
    canOfferSignIn,
    errLabel,
    errMessage,
    getConfig,
    getWindowSessionOrNull,
    signalMascotRecovered,
    t,
    WINDOW,
} from './dependencies.js';
import { BOOTSTRAP_RETRY_DEBOUNCE_MS } from './helpers.js';

export const SessionMethods = {
    async _adoptFloatingSession() {
        this.bootstrappingSession = true;
        // Register both listeners first so a fast event can't fire
        // between our synchronous check and the await below.
        let resolveAdopted;
        let resolveFailed;
        const adopted = new Promise((resolve) => {
            resolveAdopted = resolve;
        });
        const failed = new Promise((resolve) => {
            resolveFailed = resolve;
        });
        const unlistenPinned = await this.listen('session_pinned', (event) => {
            const { label, sessionId } = event?.payload || {};
            if (label === WINDOW.FLOATING && sessionId) {
                resolveAdopted(sessionId);
            }
        });
        const unlistenFailed = await this.listen('session_pin_failed', (event) => {
            const { label, reason } = event?.payload || {};
            if (label === WINDOW.FLOATING) {
                resolveFailed(reason || 'unknown');
            }
        });

        try {
            // Setup may have already pinned us before our init ran.
            const existing = await getWindowSessionOrNull(this.invoke, WINDOW.FLOATING);
            if (existing) {
                this.floatingSessionId = existing;
                this.bootstrappingSession = false;
                console.log(`[floating] adopted pre-pinned session: ${existing}`);
                return;
            }

            // Read the config so we know whether to wait or create now.
            // Default to true on read failure — the wait + failure path
            // below recovers gracefully if setup never emits anything.
            let willPreload = true;
            try {
                const config = await getConfig(this.invoke);
                willPreload = config?.acp?.agent?.start_session_on_launch !== false;
            } catch (e) {
                console.warn('[floating] config read failed, assuming preload:', e);
            }

            if (!willPreload) {
                console.log('[floating] start_session_on_launch=false, creating session now');
                const id = await this.invoke('switch_acp_session', { sessionId: null });
                this.floatingSessionId = id;
                this.bootstrappingSession = false;
                return;
            }

            // Race the two outcomes. No timeout — failure events
            // bound the wait, kiro-cli cold-start can be slow.
            const winner = await Promise.race([
                adopted.then((sid) => ({ kind: 'pinned', sid })),
                failed.then((reason) => ({ kind: 'failed', reason })),
            ]);

            if (winner.kind === 'pinned') {
                this.floatingSessionId = winner.sid;
                this.bootstrappingSession = false;
                console.log(`[floating] adopted pinned session via event: ${winner.sid}`);
                return;
            }

            // Setup told us it failed. Try to create our own as a
            // last resort; if THAT fails the user gets an explicit
            // error rather than a silent hang.
            console.warn(`[floating] setup reported pin failure: ${winner.reason}`);
            try {
                const id = await this.invoke('switch_acp_session', { sessionId: null });
                this.floatingSessionId = id;
                console.log(`[floating] recovered with own session: ${id}`);
            } catch (e) {
                console.error('[floating] recovery session/new also failed:', e);
                this._setBootstrapFailure(e);
            } finally {
                this.bootstrappingSession = false;
            }
        } catch (e) {
            console.error('[floating] failed to adopt session:', e);
            this.floatingSessionId = null;
            this.sessionBootstrapError = String(e);
            this.sessionBootstrapFailure = null;
            this.bootstrappingSession = false;
        } finally {
            if (typeof unlistenPinned === 'function') unlistenPinned();
            if (typeof unlistenFailed === 'function') unlistenFailed();
        }
    },

    /**
     * Re-attempt session bootstrap after a prior failure, then flush the
     * queued send. Triggered from `sendChatMessage` when the user sends
     * while `sessionBootstrapError` is set — a transient backend outage at
     * launch shouldn't strand the floating window until the app restarts.
     *
     * Debounced via `_lastBootstrapRetryAt`: retries closer together than
     * `BOOTSTRAP_RETRY_DEBOUNCE_MS` skip the (expensive) backend reconnect
     * and just re-show the existing error, so a burst of sends can't cascade
     * into a respawn storm against the agent backend. The backend
     * `restart_connection` has its own coalesce+retry guard too; this is the
     * front line of the same defence.
     *
     * Reuses the existing queue/poll/flush machinery: we set
     * `bootstrappingSession` so `_waitForBootstrapAndSend` waits for the
     * retry to settle, then `_flushPendingSend` either replays the send (on
     * success) or re-shows the error (on repeat failure).
     */
    _retryBootstrapAndSend(message, options) {
        const now = Date.now();
        if (now - this._lastBootstrapRetryAt < BOOTSTRAP_RETRY_DEBOUNCE_MS) {
            // Too soon since the last attempt — a retry is likely still in
            // flight or only just failed. Surface the error rather than
            // kicking off another reconnect.
            this._showBootstrapError(this._waitingSendEntry(message, options));
            return;
        }
        this._lastBootstrapRetryAt = now;
        this._pendingSend = { message, options };
        this._showBootstrapSpinner();
        // Gate BEFORE starting the async retry so the poller waits on it.
        this.bootstrappingSession = true;
        this._retryBootstrap();
        this._waitForBootstrapAndSend();
    },

    /**
     * Single re-bootstrap attempt: ask the backend for a session (which
     * lazily reconnects/respawns the agent if the connection died), and
     * clear or refresh `sessionBootstrapError` based on the outcome. Always
     * clears `bootstrappingSession` on the way out so the poller unblocks.
     */
    async _retryBootstrap() {
        console.log('[floating] retrying session bootstrap after prior failure');
        try {
            const id = await this.invoke('switch_acp_session', { sessionId: null });
            this.floatingSessionId = id;
            this._clearBootstrapFailure();
            console.log(`[floating] re-bootstrap succeeded: ${id}`);
        } catch (e) {
            console.error('[floating] re-bootstrap failed:', e);
            this._setBootstrapFailure(e);
        } finally {
            this.bootstrappingSession = false;
        }
    },

    /**
     * Show a "Spinning up agent…" placeholder in floating's response
     * area while we're waiting for the launch session to be pinned.
     * Removed once `_flushPendingSend()` runs OR the bootstrap fails
     * (showError replaces it).
     */
    _showBootstrapSpinner() {
        if (!this.elements.responseText) return;
        this.elements.contentArea.classList.add('visible');
        this.elements.responseText.innerHTML = `
        <div class="bootstrap-spinner">
            ${t('floating.bootstrap.spinner')}
            <span class="bootstrap-dot">.</span><span class="bootstrap-dot">.</span><span class="bootstrap-dot">.</span>
        </div>`;
        this.windowManager.resizeWindow();
    },

    /**
     * After bootstrap completes (or fails), flush a queued send. Called
     * from `_waitForBootstrapAndSend`. On success, replays the original
     * `sendChatMessage`; on failure, surfaces the error.
     */
    _flushPendingSend() {
        const pending = this._pendingSend;
        this._pendingSend = null;
        if (!pending) return;
        // Clear the spinner — sendChatMessage's normal path will set
        // up its own thinking indicator.
        if (this.elements.responseText) {
            this.elements.responseText.innerHTML = '';
        }
        if (this.sessionBootstrapError) {
            this._showBootstrapError(this._waitingSendEntry(pending.message, pending.options));
            return;
        }
        if (!this.floatingSessionId) {
            this.showError(t('floating.error.no_session'));
            return;
        }
        // Re-enter sendChatMessage; the bootstrap-guard at the top
        // will pass through now.
        this.sendChatMessage(pending.message, pending.options);
    },

    /** Remember a failed bootstrap: its text, and the error itself (Sign in reads its `kind`). */
    _setBootstrapFailure(e) {
        this.floatingSessionId = null;
        this.sessionBootstrapError = errMessage(e);
        this.sessionBootstrapFailure = e && typeof e === 'object' ? e : null;
    },

    _clearBootstrapFailure() {
        this.sessionBootstrapError = null;
        this.sessionBootstrapFailure = null;
    },

    /** A send held up by a failed bootstrap, as `_showBootstrapError` remembers it. */
    _waitingSendEntry(message, options) {
        return { message, options, generation: this._promptGeneration };
    },

    /**
     * Show the failed bootstrap's error, `waiting` being the send it held up
     * (or null). A signed-out agent's error also gets Sign in and Reconnect:
     * either restarts the agent, sets up a session and then sends `waiting`
     * once (`_sendWaitingAfterBootstrap`). Any other error is shown as text.
     */
    _showBootstrapError(waiting = null) {
        this._bootstrapWaitingSend = waiting;
        const text = t('floating.error.agent_unavailable', { reason: this.sessionBootstrapError });
        const failure = this.sessionBootstrapFailure;
        if (!canOfferSignIn(failure)) {
            this.showError(text);
            return;
        }
        this.showError(text, {
            reconnect: true,
            onReconnect: (btn) => this._reconnectBootstrap(btn, waiting),
            signIn: failure,
            signInHost: this._bootstrapSignInHost(waiting),
        });
    },

    /**
     * After the agent restarted under a failed bootstrap: set up the
     * window's session on it, the one the restart reloaded or else a new
     * one (`switch_acp_session`), and resolve to its id. A failure is
     * remembered and shown again (with Sign in, if it still applies) for
     * the send that is waiting now, and thrown.
     */
    async _bootstrapAfterRestart(outcome) {
        if (outcome?.sessionId && outcome.sessionReloaded !== false) {
            this._adoptReconnectSession(outcome.sessionId);
            this._clearBootstrapFailure();
            return outcome.sessionId;
        }
        this.bootstrappingSession = true;
        try {
            const id = await this.invoke('switch_acp_session', { sessionId: null });
            this.floatingSessionId = id;
            this._clearBootstrapFailure();
            console.log(`[floating] bootstrapped after the restart: ${id}`);
            return id;
        } catch (e) {
            console.error('[floating] bootstrap after the restart failed:', e);
            this._setBootstrapFailure(e);
            this._showBootstrapError(this._bootstrapWaitingSend);
            throw e;
        } finally {
            this.bootstrappingSession = false;
        }
    },

    /**
     * Whether `waiting` (the send a failed bootstrap held up) may go out now
     * on `sid`: it is still the one waiting (no send since), no prompt went
     * out since, the input holds nothing else, and the window can send.
     */
    _canSendWaitingAfterBootstrap(waiting, sid) {
        if (!waiting || this._bootstrapWaitingSend !== waiting) return false;
        if (waiting.generation !== this._promptGeneration) return false;
        const typed = this.elements.input?.value?.trim() ?? '';
        if (typed && typed !== String(waiting.message ?? '').trim()) return false;
        return !!sid && sid === this.floatingSessionId && this.canSendAgentPrompt(sid);
    },

    /** Send `waiting` through the normal send path (a turn id of its own), once. */
    async _sendWaitingAfterBootstrap(waiting) {
        if (this._bootstrapWaitingSend !== waiting) return false;
        this._bootstrapWaitingSend = null;
        await this.sendChatMessage(waiting.message, waiting.options);
        return true;
    },

    /**
     * The sign-in under a failed bootstrap's error: as a prompt's
     * (`_agentSignInHost`), except that after the restart it sets up the
     * window's session (`establishSession`) and then sends the message that
     * was waiting, at most once.
     */
    _bootstrapSignInHost(waiting) {
        const base = this._agentSignInHost(this.sessionBootstrapFailure, null);
        let resent = false;
        return {
            ...base,
            establishSession: (outcome) => this._bootstrapAfterRestart(outcome),
            canResend: (sid) => !resent && this._canSendWaitingAfterBootstrap(waiting, sid),
            resend: async () => {
                if (resent) return 'none';
                resent = true;
                return (await this._sendWaitingAfterBootstrap(waiting)) ? 'sent' : 'none';
            },
            // The send's own turn replaced the error.
            clearError: () => {},
        };
    },

    /**
     * Reconnect under a failed bootstrap's error: restart the agent, set up
     * a session, then send the message that was waiting (as the sign-in
     * does). A failed attempt leaves the button for the next one.
     */
    async _reconnectBootstrap(btn, waiting) {
        btn.disabled = true;
        btn.textContent = t('chat.error.btn.reconnecting');
        let outcome;
        try {
            outcome = await this.invoke('reconnect_acp', { sessionId: null });
        } catch (e) {
            console.log('Reconnect failed:', e);
            this._showReconnectFailure(btn, errLabel(t('chat.error.reconnect_failed_label'), e));
            return;
        }
        if (!outcome?.reconnected) {
            this._showReconnectFailure(btn, t('chat.error.reconnect_failed'));
            return;
        }
        signalMascotRecovered();
        let sid;
        try {
            sid = await this._bootstrapAfterRestart(outcome);
        } catch {
            return; // shown again by _bootstrapAfterRestart
        }
        if (this._canSendWaitingAfterBootstrap(waiting, sid)) {
            await this._sendWaitingAfterBootstrap(waiting);
            return;
        }
        if (!btn.isConnected) return;
        this.elements.responseText.textContent = '';
        this.elements.contentArea.classList.remove('visible');
        this.elements.expandBtn.classList.remove('visible');
        this.windowManager.resizeWindow();
    },

    /**
     * Poll once-per-100ms for bootstrap completion (success or failure)
     * and flush. Cheap because we're only running while the user has a
     * pending send queued — usually <1s on hot launches, ~10s on cold.
     */
    async _waitForBootstrapAndSend() {
        // If multiple calls race, only one polling loop should run.
        if (this._waitingForBootstrap) return;
        this._waitingForBootstrap = true;
        try {
            while (this.bootstrappingSession) {
                await new Promise((r) => setTimeout(r, 100));
            }
        } finally {
            this._waitingForBootstrap = false;
        }
        this._flushPendingSend();
    },
};
