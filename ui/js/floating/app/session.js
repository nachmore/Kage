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
                try {
                    await this._runBootstrap();
                } catch (e) {
                    // Remembered (the error object: Sign in reads its
                    // `kind`) by _runBootstrap for a send waiting on it.
                    console.error('[floating] launch session/new failed:', e);
                }
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
                const id = await this._runBootstrap();
                console.log(`[floating] recovered with own session: ${id}`);
            } catch (e) {
                // Remembered by _runBootstrap, as above.
                console.error('[floating] recovery session/new also failed:', e);
            }
        } catch (e) {
            console.error('[floating] failed to adopt session:', e);
            // The error itself, as every bootstrap keeps it: its text is
            // shown, and a signed-out agent's gets Sign in and Reconnect.
            this._setBootstrapFailure(e);
            this._settleBootstrapping();
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
    _retryBootstrapAndSend(submission) {
        const now = Date.now();
        if (now - this._lastBootstrapRetryAt < BOOTSTRAP_RETRY_DEBOUNCE_MS) {
            // Too soon since the last attempt — a retry is likely still in
            // flight or only just failed. Surface the error rather than
            // kicking off another reconnect.
            this._showBootstrapError(this._waitingSendEntry(submission));
            return;
        }
        this._lastBootstrapRetryAt = now;
        this._pendingSend = submission;
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
            const id = await this._runBootstrap();
            console.log(`[floating] re-bootstrap succeeded: ${id}`);
        } catch (e) {
            console.error('[floating] re-bootstrap failed:', e);
        }
    },

    /**
     * The one bootstrap (`switch_acp_session`, no session) this window runs
     * at a time after a failed launch bootstrap: a send's retry and a
     * sign-in's or Reconnect's bootstrap after the restart all come through
     * here, and a call while one is under way joins it, so two never create
     * two sessions and pin the window to whichever answered last. Each
     * attempt has a token (`_bootstrapGen`); an attempt something else
     * superseded (a restart's reloaded session adopted) changes nothing,
     * success or failure. Resolves to the session id; rejects with the
     * error, remembered for the window when the attempt is still current.
     */
    _runBootstrap() {
        if (this._bootstrapInFlight) return this._bootstrapInFlight;
        const gen = (this._bootstrapGen || 0) + 1;
        this._bootstrapGen = gen;
        this.bootstrappingSession = true;
        // Asked for now (not a tick later); a throw becomes the attempt's rejection.
        let request;
        try {
            request = Promise.resolve(this.invoke('switch_acp_session', { sessionId: null }));
        } catch (e) {
            request = Promise.reject(e);
        }
        const attempt = (async () => {
            try {
                const id = await request;
                if (gen === this._bootstrapGen) {
                    this.floatingSessionId = id;
                    this._clearBootstrapFailure();
                }
                return id;
            } catch (e) {
                if (gen === this._bootstrapGen) this._setBootstrapFailure(e);
                throw e;
            } finally {
                if (this._bootstrapInFlight === attempt) this._bootstrapInFlight = null;
                this._settleBootstrapping();
            }
        })();
        this._bootstrapInFlight = attempt;
        return attempt;
    },

    /**
     * `bootstrappingSession` after a bootstrap or a restart's bootstrap
     * settled: still set while another one is under way, so a send queued
     * meanwhile waits for the last of them.
     */
    _settleBootstrapping() {
        this.bootstrappingSession = !!this._bootstrapInFlight || (this._restartBootstraps || 0) > 0;
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
     * from `_waitForBootstrapAndSend`. On success, replays what was
     * submitted (`_captureSubmission`, never the live composer) unless the
     * user edited the input since (_inputEditedSince: they are writing
     * something else); on failure, surfaces the error.
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
            this._showBootstrapError(this._waitingSendEntry(pending));
            return;
        }
        if (this._inputEditedSince(pending)) {
            console.log('[floating] the input was edited since the queued send; not sending it');
            return;
        }
        if (!this.floatingSessionId) {
            this.showError(t('floating.error.no_session'));
            return;
        }
        // Re-enter the send path; the bootstrap-guard at the top
        // will pass through now.
        this._sendSubmission(pending, true);
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

    /**
     * A send held up by a failed bootstrap, as `_showBootstrapError`
     * remembers it: what was submitted (`_captureSubmission`).
     */
    _waitingSendEntry(submission) {
        return { submission, generation: this._promptGeneration };
    },

    /**
     * Show the failed bootstrap's error, `waiting` being the send it held up
     * (or null). A signed-out agent's error also gets Sign in and Reconnect:
     * either restarts the agent, sets up a session and then sends the send
     * waiting when it ends once (`_sendWaitingAfterBootstrap`): `waiting`,
     * or a newer send that failed again meanwhile and redrew this error.
     * Any other error is shown as text.
     */
    _showBootstrapError(waiting = null) {
        this._bootstrapWaitingSend = waiting;
        const text = t('floating.error.agent_unavailable', { reason: this.sessionBootstrapError });
        const failure = this.sessionBootstrapFailure;
        if (!canOfferSignIn(failure)) {
            this.showError(text);
        } else {
            this.showError(text, {
                reconnect: true,
                onReconnect: (btn) => this._reconnectBootstrap(btn),
                signIn: failure,
                signInHost: this._bootstrapSignInHost(),
            });
        }
        // Its first line: still on screen while nothing newer replaced it.
        this._bootstrapErrorShown = this.elements.responseText?.firstChild ?? null;
    },

    /**
     * A session is up after a failed bootstrap: take its error off the
     * screen, as Reconnect does once the agent is back, unless something
     * newer (a send's turn, another error) replaced it meanwhile.
     */
    _clearShownBootstrapError() {
        const shown = this._bootstrapErrorShown;
        this._bootstrapErrorShown = null;
        if (!shown?.isConnected) return;
        this.currentResponse = '';
        this.elements.responseText.textContent = '';
        this.elements.contentArea.classList.remove('visible');
        this.elements.expandBtn.classList.remove('visible');
        this.windowManager.resizeWindow();
    },

    /**
     * After the agent restarted under a failed bootstrap: set up the
     * window's session on it, the one the restart reloaded or else a new
     * one, and resolve to its id. Never a second bootstrap beside one under
     * way (a send's retry, the other of Sign in and Reconnect): that one
     * settles first, and when it left the window a session (and no
     * failure), that is the one. A failure is remembered and shown again
     * (with Sign in, if it still applies) for the send that is waiting now,
     * unless another restart's bootstrap is still to come and will show
     * its own, and thrown.
     */
    async _bootstrapAfterRestart(outcome) {
        this._restartBootstraps = (this._restartBootstraps || 0) + 1;
        this.bootstrappingSession = true;
        try {
            if (this._bootstrapInFlight) {
                await this._bootstrapInFlight.catch(() => {});
            }
            if (outcome?.sessionId && outcome.sessionReloaded !== false) {
                // Supersedes any bootstrap that might still answer.
                this._bootstrapGen = (this._bootstrapGen || 0) + 1;
                this._adoptReconnectSession(outcome.sessionId);
                this._clearBootstrapFailure();
                this._clearShownBootstrapError();
                return outcome.sessionId;
            }
            if (this.floatingSessionId && !this.sessionBootstrapError) {
                console.log(
                    `[floating] after the restart: keeping the session already set up: ${this.floatingSessionId}`
                );
                this._clearShownBootstrapError();
                return this.floatingSessionId;
            }
            try {
                const id = await this._runBootstrap();
                console.log(`[floating] bootstrapped after the restart: ${id}`);
                // The session is ready: its error goes (a waiting send, if
                // any, then draws its own turn).
                this._clearShownBootstrapError();
                return id;
            } catch (e) {
                console.error('[floating] bootstrap after the restart failed:', e);
                if (this._restartBootstraps <= 1) {
                    this._showBootstrapError(this._bootstrapWaitingSend);
                }
                throw e;
            }
        } finally {
            this._restartBootstraps -= 1;
            this._settleBootstrapping();
        }
    },

    /**
     * Whether `waiting` (the send a failed bootstrap held up) may go out now
     * on `sid`: it is still the one waiting (no send since), no prompt went
     * out since, the user hasn't edited the input since it was submitted
     * (any edit, even back to the same text or to nothing: they are
     * writing something else), and the window can send.
     */
    _canSendWaitingAfterBootstrap(waiting, sid) {
        if (!waiting || this._bootstrapWaitingSend !== waiting) return false;
        if (waiting.generation !== this._promptGeneration) return false;
        if (this._inputEditedSince(waiting.submission)) return false;
        return !!sid && sid === this.floatingSessionId && this.canSendAgentPrompt(sid);
    },

    /**
     * Send `waiting` once, as it was submitted, through the normal send path
     * (a turn id of its own). Files attached since stay in the composer.
     */
    async _sendWaitingAfterBootstrap(waiting) {
        if (this._bootstrapWaitingSend !== waiting) return false;
        this._bootstrapWaitingSend = null;
        await this._sendSubmission(waiting.submission, true);
        return true;
    },

    /**
     * The sign-in under a failed bootstrap's error: as a prompt's
     * (`_agentSignInHost`), except that after the restart it sets up the
     * window's session (`establishSession`) and then sends the message
     * waiting then (`_bootstrapWaitingSend`), at most once.
     */
    _bootstrapSignInHost() {
        const base = this._agentSignInHost(this.sessionBootstrapFailure, null);
        let resent = false;
        // The send waiting when the sign-in ends, not when its button was
        // drawn: a send made during the login that failed again is waiting
        // now (_showBootstrapError redrew the error for it), and it is the
        // one the user wants sent.
        let waiting = null;
        return {
            ...base,
            establishSession: (outcome) => this._bootstrapAfterRestart(outcome),
            canResend: (sid) => {
                waiting = this._bootstrapWaitingSend;
                return !resent && this._canSendWaitingAfterBootstrap(waiting, sid);
            },
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
     * a session, then send the message waiting then (as the sign-in does:
     * a send made meanwhile that failed again is the one waiting). A failed
     * attempt leaves the button for the next one.
     */
    async _reconnectBootstrap(btn) {
        // One Reconnect at a time (a redrawn error has a new button).
        if (this._bootstrapReconnecting) return;
        this._bootstrapReconnecting = true;
        btn.disabled = true;
        btn.textContent = t('chat.error.btn.reconnecting');
        let sid;
        try {
            let outcome;
            try {
                outcome = await this.invoke('reconnect_acp', { sessionId: null });
            } catch (e) {
                console.log('Reconnect failed:', e);
                this._showReconnectFailure(
                    btn,
                    errLabel(t('chat.error.reconnect_failed_label'), e)
                );
                return;
            }
            if (!outcome?.reconnected) {
                this._showReconnectFailure(btn, t('chat.error.reconnect_failed'));
                return;
            }
            signalMascotRecovered();
            try {
                sid = await this._bootstrapAfterRestart(outcome);
            } catch {
                return; // shown again by _bootstrapAfterRestart
            }
        } finally {
            this._bootstrapReconnecting = false;
        }
        // The bootstrap took its error off the screen (_clearShownBootstrapError).
        const waiting = this._bootstrapWaitingSend;
        if (this._canSendWaitingAfterBootstrap(waiting, sid)) {
            await this._sendWaitingAfterBootstrap(waiting);
        }
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
