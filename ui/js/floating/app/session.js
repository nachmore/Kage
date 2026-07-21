import { errMessage, getConfig, getWindowSessionOrNull, t, WINDOW } from './dependencies.js';
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
                this.floatingSessionId = null;
                this.sessionBootstrapError = errMessage(e);
            } finally {
                this.bootstrappingSession = false;
            }
        } catch (e) {
            console.error('[floating] failed to adopt session:', e);
            this.floatingSessionId = null;
            this.sessionBootstrapError = String(e);
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
            this.showError(
                t('floating.error.agent_unavailable', { reason: this.sessionBootstrapError })
            );
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
            this.sessionBootstrapError = null;
            console.log(`[floating] re-bootstrap succeeded: ${id}`);
        } catch (e) {
            console.error('[floating] re-bootstrap failed:', e);
            this.floatingSessionId = null;
            this.sessionBootstrapError = errMessage(e);
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
            this.showError(
                t('floating.error.agent_unavailable', { reason: this.sessionBootstrapError })
            );
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
