import {
    clearAgentSignInSlot,
    _resetDiagramFailures,
    attachSourceClickHandler,
    createShellActivitySlot,
    errLabel,
    errMessage,
    escapeHtml,
    finishShellActivity,
    getActionsForText,
    getConfig,
    handOffShellActivity,
    isResendablePayload,
    mountSignInAction,
    offlineMessage,
    pruneShellActivity,
    renderMarkdown,
    renderSourceBubblesHtml,
    renderToolSources,
    sendAppNotification,
    SHELL_LAYOUT_EVENT,
    showShellActivity,
    signalMascot,
    signalMascotRecovered,
    syncToolSources,
    t,
    WINDOW,
} from './dependencies.js';
import { presentOverlayWindow } from '../../shared/overlay-window.js';

export const ResponseUiMethods = {
    flushStreamingRender() {
        this.messageStreamController.flushStreamingRender();
    },

    handleMessageChunk(event) {
        return this.messageStreamController.handleChunk(event);
    },

    async handleMessageComplete() {
        return this.messageStreamController.handleComplete();
    },

    /**
     * Render a loading indicator while an extension tool call is being streamed.
     * Window-specific DOM — wired into the ExtensionToolController via host adapter.
     * Tool-usage tracking is handled by the controller before this fires.
     */
    _renderExtensionToolIndicator(info) {
        const beforeFence = this.currentResponse.split('```extension_tool_call')[0].trim();
        if (beforeFence) {
            renderMarkdown(beforeFence, this.elements.responseText, true);
        } else {
            const friendlyName = this.extensionToolController.getExtensionToolFriendlyName(
                info.extension,
                info.tool
            );
            this.elements.responseText.innerHTML = `<div class="folder-plan-spinner-row"><span class="folder-plan-spinner"></span> ${escapeHtml(friendlyName)}...</div>`;
        }
    },

    _showFloatingResponseActions() {
        const bar = document.getElementById('floatingResponseActions');
        if (!bar) return;
        bar.style.display = 'flex';

        const copyBtn = document.getElementById('floatingCopyBtn');
        const speakBtn = document.getElementById('floatingSpeakBtn');

        // Show speak button if TTS is available
        if (speakBtn) {
            const hasTts = this.speech?.pocketTtsEnabled || this.speech?.readBack;
            speakBtn.style.display = hasTts ? '' : 'none';
        }

        // Wire copy
        if (copyBtn) {
            copyBtn.onclick = () => {
                const text = this.currentResponse || '';
                navigator.clipboard.writeText(text).then(() => {
                    signalMascot('copy');
                    copyBtn.innerHTML =
                        '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>';
                    setTimeout(() => {
                        copyBtn.innerHTML =
                            '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>';
                    }, 1500);
                });
            };
        }

        // Wire speak
        if (speakBtn) {
            speakBtn.onclick = () => {
                if (this.speech && this.currentResponse) {
                    // Stop any existing speech before starting new one.
                    // speakText ignores the read-back gate (explicit request).
                    this.speech.cancelSpeech();
                    this.speech.speakText(this.currentResponse);
                }
            };
        }
    },

    async _showResponseActions(responseText) {
        console.log('[QA] _showResponseActions called, text length:', responseText?.length);
        if (!responseText?.trim()) return;
        try {
            const config = await getConfig(this.invoke);
            if (!config.ui?.show_response_actions) return;
            const qaConfig = config.quick_actions || { enabled: true, custom_actions: [] };
            const actions = await getActionsForText(responseText, qaConfig);
            console.log('[QA] Actions found:', actions.length);
            if (actions.length === 0) return;
            const container = document.getElementById('responseActionsContainer');
            if (container) {
                container.innerHTML = '';
                for (const action of actions) {
                    const chip = document.createElement('button');
                    chip.className = 'quick-action-chip';
                    chip.title = action.label;
                    const iconSpan = document.createElement('span');
                    iconSpan.className = 'quick-action-icon';
                    iconSpan.textContent = action.icon || '⚡';
                    const labelSpan = document.createElement('span');
                    labelSpan.className = 'quick-action-label';
                    labelSpan.textContent = action.label;
                    chip.appendChild(iconSpan);
                    chip.appendChild(labelSpan);
                    chip.addEventListener('click', () => {
                        // Replacer fn so `$&`, `$$`, `$'` in the response stay literal.
                        const prompt = action.prompt.replace(/\{text\}/g, () => responseText);
                        container.style.display = 'none';
                        this.sendChatMessage(prompt, { skipSelection: true });
                    });
                    container.appendChild(chip);
                }
                container.style.display = 'flex';
                await this.windowManager.resizeWindow();
            }
        } catch (e) {
            console.warn('[QA] Response actions error:', e);
        }
    },

    /**
     * Render agent-suggested action chips below the response.
     * These come from a hidden ```suggested_actions``` fence in the agent's response.
     */
    _renderSuggestedActions(actions) {
        const container = document.getElementById('responseActionsContainer');
        if (!container) return;
        container.innerHTML = '';
        container.style.display = 'flex';
        for (const action of actions) {
            const chip = document.createElement('button');
            chip.className = 'quick-action-chip';
            chip.title = action.prompt;
            // action.label comes from an agent-authored JSON fence — untrusted.
            const labelSpan = document.createElement('span');
            labelSpan.className = 'quick-action-label';
            labelSpan.textContent = action.label;
            chip.appendChild(labelSpan);
            chip.addEventListener('click', () => {
                container.style.display = 'none';
                this.sendChatMessage(action.prompt, { skipSelection: true });
            });
            container.appendChild(chip);
        }
        this.windowManager.resizeWindow();
    },

    _showCompactionIndicator() {
        // Show a subtle notice that context is being compacted
        let notice = document.getElementById('compactionNotice');
        if (!notice) {
            notice = document.createElement('div');
            notice.id = 'compactionNotice';
            notice.className = 'compaction-notice';
            notice.innerHTML = `<span class="folder-plan-spinner"></span> ${t('floating.compaction.in_progress')}`;
            this.elements.responseText?.appendChild(notice);
        }
    },

    _hideCompactionIndicator() {
        const notice = document.getElementById('compactionNotice');
        if (notice) notice.remove();
    },

    async handleMessageError(event) {
        return this.messageStreamController.handleError(event);
    },

    handleSessionReset(event) {
        return this.messageStreamController.handleSessionReset(event);
    },

    handleToolCallUpdate(event) {
        return this.messageStreamController.handleToolCallUpdate(event);
    },

    renderSources() {
        const compactEl = document.getElementById('toolSourcesCompact');
        if (compactEl) compactEl.remove();

        let sourcesEl = document.getElementById('toolSources');
        if (!sourcesEl) {
            sourcesEl = document.createElement('div');
            sourcesEl.id = 'toolSources';
            sourcesEl.className = 'tool-sources';
            if (this.elements.contentArea) this.elements.contentArea.appendChild(sourcesEl);
        }

        if (this.toolSources.length === 0 && this.toolUsages.length === 0) {
            sourcesEl.style.display = 'none';
            return;
        }

        sourcesEl.style.display = 'flex';
        this.elements.contentArea.classList.add('visible');
        // Updates the chip row in place: an open tool list (and any call
        // expanded in it) survives the re-render on every new tool call.
        renderToolSources(sourcesEl, this.toolUsages, this.toolSources);
        attachSourceClickHandler(sourcesEl, this.invoke);
        this._wireShellLayout();
        this.windowManager.resizeWindow();
    },

    /**
     * The slot right under the response text where a shell command's block
     * shows while it runs (in place of the generic tool spinner line). Lives
     * outside `responseText` so streaming markdown renders never touch it.
     */
    _shellActivitySlot(create = false) {
        let slot = document.getElementById('shellActivity');
        if (!slot && create && this.elements.responseText) {
            slot = createShellActivitySlot();
            slot.id = 'shellActivity';
            this.elements.responseText.after(slot);
            this._wireShellLayout();
        }
        return slot;
    },

    /** Re-measure the window when a tool block or a tool list opens/closes. */
    _wireShellLayout() {
        const area = this.elements.contentArea;
        if (!area || area._kageShellLayoutWired) return;
        area._kageShellLayoutWired = true;
        area.addEventListener(SHELL_LAYOUT_EVENT, () => this.windowManager.resizeWindow());
    },

    _showShellCommand(record) {
        // The command block takes over from any generic spinner line.
        const spinner = this.elements.responseText?.querySelector('.tool-running-indicator');
        if (spinner) spinner.remove();
        showShellActivity(this._shellActivitySlot(true), record);
        const sourcesEl = document.getElementById('toolSources');
        if (sourcesEl) syncToolSources(sourcesEl, this.toolUsages);
        this.elements.contentArea.classList.add('visible');
        this.windowManager.resizeWindow();
    },

    /**
     * Another tool's call changed (status, arguments, output): refresh its
     * block under its chip, if the chip row is showing. New calls and
     * calls that changed chip come through renderSources instead.
     */
    _refreshToolCall() {
        const sourcesEl = document.getElementById('toolSources');
        if (!sourcesEl || sourcesEl.style.display === 'none') return;
        syncToolSources(sourcesEl, this.toolUsages);
    },

    _pruneShellActivity() {
        const slot = this._shellActivitySlot();
        if (slot) pruneShellActivity(slot);
    },

    /**
     * Turn over (complete / error / stop): settle running commands, redraw
     * the whole chip row (a call can have become a shell call after its
     * plain chip was drawn) and move blocks the user still has open from
     * the activity slot into the command list, so each command shows once.
     */
    _finishShellActivity() {
        const slot = this._shellActivitySlot();
        finishShellActivity(slot, this.toolUsages);
        const sourcesEl = document.getElementById('toolSources');
        if (!sourcesEl) return;
        renderToolSources(sourcesEl, this.toolUsages, this.toolSources);
        attachSourceClickHandler(sourcesEl, this.invoke);
        handOffShellActivity(slot, sourcesEl);
    },

    _removeShellActivity() {
        document.getElementById('shellActivity')?.remove();
    },

    renderSourcesCompact() {
        this.elements.loadingDots.classList.remove('visible');
        this.elements.mascotContainer.classList.remove('thinking');

        let compactEl = document.getElementById('toolSourcesCompact');
        if (!compactEl) {
            compactEl = document.createElement('div');
            compactEl.id = 'toolSourcesCompact';
            compactEl.className = 'tool-sources-compact';
            const speechBubble = document.querySelector('.speech-bubble');
            if (speechBubble) speechBubble.insertBefore(compactEl, this.elements.contentArea);
        }

        compactEl.style.display = 'flex';
        compactEl.innerHTML = renderSourceBubblesHtml(this.toolUsages, this.toolSources);
        attachSourceClickHandler(compactEl, this.invoke);
        this.windowManager.resizeWindow();
    },

    /**
     * The turn on screen as a MESSAGE_ERROR arrives (see
     * MessageStreamController.handleError): `_promptGeneration` moves on
     * with every response turn, and the session with a new chat.
     */
    _errorContext() {
        return {
            generation: this._promptGeneration,
            sessionId: this.floatingSessionId ?? null,
        };
    },

    /**
     * The turn failed (MESSAGE_ERROR). The payload is a string, or for a
     * sign-in demand / "still failing after a restart" an AppError-shaped
     * `{ kind, message, detail }`. `context` is `_errorContext()` from
     * when the error arrived.
     */
    async _onMessageError(event, online, context) {
        // A newer turn started while the online probe ran (a follow-up held
        // until this turn settled), or the window moved to another session:
        // the error is about a turn no longer on screen. It must not end the
        // new one, nor make it the turn Reconnect resends.
        if (
            context &&
            (context.generation !== this._promptGeneration ||
                context.sessionId !== (this.floatingSessionId ?? null))
        ) {
            console.log('[floating] message_error for a turn no longer shown; ignoring it');
            return;
        }
        if (!this.isWaitingForResponse) return;
        const payload = event.payload;
        // An earlier turn's error (it names another turn id, e.g. one the
        // user stopped before sending this one on the same session): not
        // the turn on screen, which goes on.
        if (this._errorIsForOtherTurn(payload)) {
            console.log('[floating] message_error for an earlier turn; ignoring it');
            return;
        }
        // A turn on a session this window has since left (and maybe sent a
        // new prompt from): not the turn on screen, which goes on.
        if (this._errorIsForOtherSession(payload)) {
            console.log('[floating] message_error for a session not shown:', payload.sessionId);
            return;
        }
        this.isWaitingForResponse = false;
        this._finishShellActivity();
        this.computerControlActive = false;
        this._noBlurTools.clear();
        this.elements.floatingStopBtn.style.display = 'none';
        this.updateDatetimeVisibility();
        _resetDiagramFailures();
        // Read before showError, which overwrites currentResponse. An extra
        // veto on top of the backend's `resendable` (it sees progress this
        // window doesn't, e.g. a permission request that was approved).
        const hadOutput = !!this.currentResponse?.trim() || this.toolUsages?.length > 0;
        if (!online) {
            this.showError(offlineMessage());
            return;
        }
        // Reconnect may resend this turn only when the error names this very
        // turn (its `turnId`), the backend says nothing of it got through,
        // and the error is about its session (any id it had across a
        // migration). A plain string, or an error naming no turn, is never
        // resent.
        const turn = this._lastTurn;
        const retryTurn =
            !hadOutput &&
            isResendablePayload(payload, turn?.turnId) &&
            this._turnWasOn(turn, payload.sessionId)
                ? { ...turn }
                : null;
        const text =
            typeof payload === 'string'
                ? t('floating.error.error_with_payload', { payload })
                : errMessage(payload);
        // signIn: a signed-out agent's error object also gets "Sign in"
        // (shared/agent-sign-in.js), which resends retryTurn after the
        // sign-in under the same rules; a plain string doesn't.
        this.showError(text, {
            reconnect: true,
            retryTurn,
            detail: payload?.detail,
            signIn: payload,
        });
    },

    showError(message, opts = {}) {
        this.stopThinking();
        this.currentResponse = message;
        this.elements.contentArea.classList.add('visible');
        this.elements.expandBtn.classList.add('visible');

        // For connection errors, offer a Reconnect affordance (parity with the
        // chat window). Other errors (shortcut failed, no session, etc.) stay
        // plain text. Build via DOM API so `message` never touches innerHTML.
        this.elements.responseText.textContent = '';
        const msgSpan = document.createElement('span');
        msgSpan.textContent = message;
        this.elements.responseText.appendChild(msgSpan);

        // The agent's own text, for troubleshooting (not translated).
        if (opts.detail) {
            const detail = document.createElement('div');
            detail.className = 'error-detail';
            detail.dir = 'ltr';
            detail.textContent = t('shared.streaming.error_details', { detail: opts.detail });
            this.elements.responseText.appendChild(detail);
        }

        // Reconnect, and "Sign in" for a signed-out agent (opts.signIn: the
        // MESSAGE_ERROR payload; a plain string draws nothing).
        const actions = document.createElement('span');
        actions.className = 'floating-error-actions';
        if (opts.reconnect) {
            const btn = document.createElement('button');
            btn.className = 'floating-error-reconnect';
            btn.textContent = t('chat.error.btn.reconnect');
            btn.addEventListener('click', () => this._reconnectFromError(btn, opts.retryTurn));
            actions.appendChild(btn);
        }
        if (opts.reconnect || opts.signIn) {
            this.elements.responseText.appendChild(document.createElement('br'));
            this.elements.responseText.appendChild(actions);
        }
        if (opts.signIn) {
            mountSignInAction(
                actions,
                opts.signIn,
                this._agentSignInHost(opts.signIn, opts.retryTurn ?? null)
            ).catch((e) => console.warn('[floating] sign-in action unavailable:', e));
        }

        this.windowManager.resizeWindow();
    },

    /**
     * Reconnect clicked under an error: restart the agent and reload this
     * window's session, then resend the failed turn when the backend said it
     * may be (`retryTurn`, see _onMessageError), nothing was sent since, and
     * the window still holds the session the Reconnect started on. A failed
     * attempt leaves the button, and the turn, for the next one.
     */
    async _reconnectFromError(btn, retryTurn) {
        btn.disabled = true;
        btn.textContent = t('chat.error.btn.reconnecting');
        const startedOn = this.floatingSessionId ?? null;
        let outcome;
        try {
            outcome = await this.invoke('reconnect_acp', { sessionId: startedOn });
        } catch (e) {
            console.log('Reconnect failed:', e);
            this._showReconnectFailure(btn, errLabel(t('chat.error.reconnect_failed_label'), e));
            return;
        }
        if (!outcome?.reconnected) {
            console.log('Reconnect failed: the agent did not come back');
            this._showReconnectFailure(btn, t('chat.error.reconnect_failed'));
            return;
        }
        // The agent is back — drop the error/limit pose.
        signalMascotRecovered();
        const moved = (this.floatingSessionId ?? null) !== startedOn;
        const sid = outcome.sessionId;
        if (moved) {
            // reconnect_acp pinned the window to the session it reloaded;
            // the window holds another one now. Keep that one, resend
            // nothing.
            console.log(
                '[floating] reconnect finished after the window moved to',
                this.floatingSessionId
            );
            this._repinFloatingSession();
        } else {
            // The reload made a fresh session: the resend below goes to it.
            this._adoptReconnectSession(sid);
        }
        if (!moved && this._canResendTurn(retryTurn, startedOn)) {
            await this._resendTurn(retryTurn);
            return;
        }
        // Only this error goes: a turn started since keeps its response.
        if (!btn.isConnected) return;
        this.elements.responseText.textContent = '';
        this.elements.contentArea.classList.remove('visible');
        this.elements.expandBtn.classList.remove('visible');
        this.windowManager.resizeWindow();
    },

    /** A Reconnect that failed: say why above the button, ready again. */
    _showReconnectFailure(btn, text) {
        btn.textContent = t('chat.error.btn.reconnect');
        btn.disabled = false;
        let failure = this.elements.responseText.querySelector('.error-reconnect-failed');
        if (!failure) {
            failure = document.createElement('div');
            failure.className = 'error-reconnect-failed';
            (btn.closest('.floating-error-actions') || btn).before(failure);
        }
        failure.textContent = text;
        this.windowManager.resizeWindow();
    },

    /** Pin the window back to the session it holds (reconnect_acp pinned it to another). */
    _repinFloatingSession() {
        if (!this.floatingSessionId) return;
        this.invoke('set_window_session', {
            label: WINDOW.FLOATING,
            sessionId: this.floatingSessionId,
        }).catch(() => {});
    },

    /** Take the session id `reconnect_acp` answered with, if it moved. */
    _adoptReconnectSession(sid) {
        if (!sid || sid === this.floatingSessionId) return;
        console.log('[floating] adopting reconnect session id:', sid);
        this.floatingSessionId = sid;
        this.invoke('set_window_session', {
            label: WINDOW.FLOATING,
            sessionId: sid,
        }).catch(() => {});
    },

    /**
     * Whether the failed turn `retryTurn` (onError's: null when any of it got
     * through, it was a rate limit, or the error named another turn) may go
     * out again now: it went to `shownOn` (the session the window held
     * before adopting a reconnect's id), no prompt was sent since and the
     * window can send.
     */
    _canResendTurn(retryTurn, shownOn) {
        return (
            !!retryTurn &&
            retryTurn.sessionId === shownOn &&
            retryTurn.generation === this._promptGeneration &&
            this.canSendAgentPrompt(this.floatingSessionId)
        );
    },

    /**
     * The floating window's side of the agent sign-in (shared/agent-sign-in.js)
     * for one error. The run lives in #agentSignInSlot, outside the response,
     * so a new turn or reset keeps it; hiding the window never stops it.
     * `payload` is that error and `retryTurn` its failed turn (see
     * _onMessageError): the sign-in resends only that turn, by Reconnect's
     * rules — the error names it (`turnId`) and says it may go again
     * (isResendablePayload), the window still holds its session and sent
     * nothing since (_canResendTurn) — and at most once. The resend mints
     * a turn id of its own (_sendTrackedTurn).
     */
    _agentSignInHost(payload, retryTurn) {
        // The session the window held when the restart answered, before
        // adopting a reloaded id; undefined until a restart answers.
        let shownOn;
        let resent = false;
        return {
            slot: () => document.getElementById('agentSignInSlot'),
            rows: 10,
            compact: true,
            buttonClass: 'floating-error-reconnect',
            invoke: (cmd, args) => this.invoke(cmd, args),
            layoutChanged: () => this.windowManager.resizeWindow(),
            onActiveChange: (active) => {
                // Focus moving into the terminal must not read as a blur.
                if (active) this._armSignInBlurSuppression();
            },
            onEnded: ({ state, outcome, agent }) => this._onAgentSignInEnded(state, outcome, agent),
            // The restart pinned the window to `sid` (the failed session
            // `from`, reloaded): follow it only while the window still
            // holds `from`; else keep the session it holds now.
            adoptSession: (sid, from) => {
                shownOn = this.floatingSessionId ?? null;
                if (from && from !== shownOn) {
                    console.log('[floating] sign-in finished after the window moved to', shownOn);
                    this._repinFloatingSession();
                    return;
                }
                this._adoptReconnectSession(sid);
            },
            canResend: (sid) => {
                const before = shownOn === undefined ? (this.floatingSessionId ?? null) : shownOn;
                shownOn = undefined;
                return (
                    !resent &&
                    !!retryTurn &&
                    sid === this.floatingSessionId &&
                    isResendablePayload(payload, retryTurn.turnId) &&
                    this._canResendTurn(retryTurn, before)
                );
            },
            resend: async () => {
                if (resent) return 'none';
                resent = true;
                return (await this._resendTurn(retryTurn, { throwOnError: true }))
                    ? 'sent'
                    : 'none';
            },
            // The resend's new turn already replaced the error.
            clearError: () => {},
        };
    },

    _armSignInBlurSuppression() {
        this._suppressBlurHideUntil = Math.max(this._suppressBlurHideUntil || 0, Date.now() + 1500);
    },

    /**
     * A sign-in run ended. The user was most likely in the browser, and the
     * launcher hid on blur: bring it back to show the result (and the resend
     * streaming in). A run stopped by the time limit doesn't pull the window
     * up (the user has likely moved on); a notification says so instead.
     */
    async _onAgentSignInEnded(state, outcome, agent) {
        if (state === 'success') signalMascotRecovered();
        if (outcome === 'timed_out') {
            try {
                await sendAppNotification(
                    this.invoke,
                    t('shared.notify.kage_title'),
                    t('shared.agent_sign_in.notify.timed_out', { agent }),
                    WINDOW.FLOATING
                );
            } catch {
                /* ignore */
            }
            return;
        }
        if (outcome !== 'exited' || !window._kageFloatingHidden) return;
        this._armSignInBlurSuppression();
        try {
            await presentOverlayWindow(this.appWindow);
        } catch (e) {
            console.warn('[floating] could not re-show after sign-in:', e);
        }
        this.windowManager.resizeWindow();
    },

    /** Empty #agentSignInSlot, unless a sign-in still holds it. */
    _clearAgentSignInSlot() {
        clearAgentSignInSlot(document.getElementById('agentSignInSlot'));
    },

    async openUrl(url) {
        try {
            await this.invoke('open_url', { url });
            await this.clearSuggestions();
            this.elements.input.value = '';
        } catch (error) {
            console.error('Error opening URL:', error);
        }
    },

    async openPath(path) {
        try {
            await this.invoke('open_path', { path });
            await this.clearSuggestions();
            this.elements.input.value = '';
        } catch (error) {
            console.error('Error opening path:', error);
        }
    },

    async launchApp(appName) {
        try {
            await this.invoke('launch_app_by_name', { appName });
            await this.clearSuggestions();
            this.elements.input.value = '';
        } catch (error) {
            console.error('Error launching app:', error);
        }
    },

    async handleExpandClick() {
        try {
            await this.invoke('open_chat_window', { showDefaultSession: true });
            await this.appWindow.hide();
        } catch (error) {
            console.error('Error opening chat window:', error);
        }
    },

    async handleOutsideClick(event) {
        // Don't hide if the permission modal is open
        const permissionModal = document.getElementById('permissionModal');
        if (permissionModal && permissionModal.style.display !== 'none') {
            return;
        }
        // Don't hide if an extension tool is being processed
        if (this._extensionToolExecuting || this._extensionToolCallHandled) {
            return;
        }

        // Don't hide if we just finished resizing or dragging — the mouseup
        // outside the window boundary fires a click event we should ignore.
        if (this.windowManager.isResizing || this.windowManager.isDragging) return;
        if (
            this.windowManager._resizeEndedAt &&
            Date.now() - this.windowManager._resizeEndedAt < 300
        )
            return;

        const container = document.querySelector('.floating-container');
        if (container && !container.contains(event.target)) {
            // Don't hide if a sandbox iframe is running (Try button)
            if (window._kageSandboxActive) return;
            await this.appWindow.hide();
        }
    },
};
