/**
 * The text of a reply for Copy / Speak / Translate: what the agent wrote,
 * without the tool/source chip row rendered into the same element (its
 * shell command list holds every command and any output the user opened).
 */
function replyText(contentDiv) {
    const clone = contentDiv.cloneNode(true);
    for (const node of clone.querySelectorAll('.tool-sources')) node.remove();
    return clone.textContent || '';
}

/** What the user typed in their bubble, without its attachment previews. */
export function userText(contentDiv) {
    const clone = contentDiv.cloneNode(true);
    for (const node of clone.querySelectorAll('.message-attachments')) node.remove();
    return clone.textContent || '';
}

const COPY_ICON =
    '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>';
const COPIED_ICON =
    '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>';

/** Copy `text`, and show a tick on `btn` for a moment. */
function copyWithTick(btn, text) {
    return navigator.clipboard.writeText(text).then(() => {
        btn.innerHTML = COPIED_ICON;
        setTimeout(() => {
            btn.innerHTML = COPY_ICON;
        }, 1500);
    });
}

export function createMessagesMixin(dependencies) {
    const {
        renderMarkdown,
        escapeHtml,
        errLabel,
        t,
        mascotHTML,
        isOnline,
        checkOnline,
        onNetworkChange,
        offlineMessage,
        attachSourceClickHandler,
        createShellActivitySlot,
        finishShellActivity,
        handOffShellActivity,
        pruneShellActivity,
        renderToolSources,
        showShellActivity,
        getConfig,
        ExtensionToolController,
        formatErrorShared,
        splitPromptContext,
        mountSignInAction,
        canOfferSignIn,
        clearAgentSignInSlot,
        isResendablePayload,
        presentOverlayWindow,
        sendAppNotification,
        WINDOW,
    } = dependencies;
    return class {
        /**
         * Build a collapsed steering section (header + toggle). `fillBody`
         * receives the `.steering-content` element to populate.
         */
        createSteeringSection(fillBody) {
            const steeringEl = document.createElement('div');
            steeringEl.className = 'steering-message collapsed';
            steeringEl.innerHTML = `
                    <div class="steering-header" onclick="this.parentElement.classList.toggle('collapsed')">
                        <span class="steering-icon"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="2"/><line x1="12" y1="14" x2="12" y2="22"/><line x1="10" y1="12" x2="2.5" y2="10.7"/><line x1="14" y1="12" x2="21.5" y2="10.7"/></svg></span>
                        <span class="steering-label">${t('chat.message.steering_label')}</span>
                        <span class="steering-toggle">▶</span>
                    </div>
                    <div class="steering-body">
                        <div class="steering-content"></div>
                    </div>
                `;
            fillBody(steeringEl.querySelector('.steering-content'));
            return steeringEl;
        }

        /**
         * Split the context the floating window injects ahead of a prompt
         * (App Mode steering, selected text) into its own collapsed section,
         * appended to the transcript just before the user's bubble. Returns
         * the text the bubble itself should show.
         */
        appendPromptContext(text) {
            const { prompt, steering, selection } = splitPromptContext(text);
            if (steering.length === 0 && selection === null) return prompt;
            const section = this.createSteeringSection((contentEl) => {
                for (const { app, body } of steering) {
                    const appEl = document.createElement('div');
                    appEl.className = 'prompt-context-app';
                    appEl.textContent = app;
                    const bodyEl = document.createElement('div');
                    bodyEl.className = 'prompt-context-body';
                    bodyEl.textContent = body;
                    contentEl.append(appEl, bodyEl);
                }
                if (selection !== null) {
                    const pre = document.createElement('pre');
                    pre.className = 'prompt-context-selection';
                    pre.textContent = selection;
                    contentEl.appendChild(pre);
                }
            });
            this.elements.messagesArea.appendChild(section);
            return prompt;
        }

        createMessageElement(role, content) {
            const msg = document.createElement('div');
            msg.className = `message ${role}`;

            const avatar = document.createElement('div');
            avatar.className = 'message-avatar';
            if (role === 'assistant') {
                avatar.innerHTML = mascotHTML({ size: 18 });
            } else {
                if (this.userInfo?.avatar_base64) {
                    const img = document.createElement('img');
                    img.src = this.userInfo.avatar_base64;
                    img.style.cssText = 'width:100%;height:100%;border-radius:50%;object-fit:cover';
                    img.onerror = () => {
                        avatar.textContent = this.userInfo?.initials || '?';
                        img.remove();
                    };
                    avatar.appendChild(img);
                } else {
                    avatar.textContent = this.userInfo?.initials || '?';
                    avatar.style.fontSize = '13px';
                    avatar.style.fontWeight = '600';
                }
            }

            const bubble = document.createElement('div');
            bubble.className = 'message-bubble';

            const contentDiv = document.createElement('div');
            contentDiv.className = 'message-content';
            contentDiv.dir = 'auto';
            if (content) contentDiv.textContent = content;

            bubble.appendChild(contentDiv);

            // Timestamp for user messages, and a Copy button that shows on
            // hover beside it.
            if (role === 'user') {
                const tsEl = document.createElement('div');
                tsEl.className = 'message-actions user-actions';
                tsEl.innerHTML = `<button class="msg-action-btn" data-action="copy" title="${t('chat.message.action.copy')}" aria-label="${t('chat.message.action.copy')}">${COPY_ICON}</button><span class="msg-timestamp"></span>`;
                const copyBtn = tsEl.querySelector('[data-action="copy"]');
                copyBtn.onclick = () => {
                    copyWithTick(copyBtn, userText(contentDiv)).catch((e) =>
                        console.warn('[CHAT] Copy failed:', e)
                    );
                };
                bubble.appendChild(tsEl);
            }

            // Action bar for assistant messages
            if (role === 'assistant') {
                const actions = document.createElement('div');
                actions.className = 'message-actions';
                actions.innerHTML = `
                <button class="msg-action-btn" data-action="copy" title="${t('chat.message.action.copy')}">
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
                </button>
                <button class="msg-action-btn" data-action="speak" title="${t('chat.message.action.speak')}" style="display:${this._showSpeakBtn ? '' : 'none'}">
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><path d="M15.54 8.46a5 5 0 0 1 0 7.07"/></svg>
                </button>
                <button class="msg-action-btn" data-action="translate" title="${t('chat.message.action.translate')}" style="display:${this._showTranslateBtn ? '' : 'none'}">
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m5 8 6 6"/><path d="m4 14 6-6 2-3"/><path d="M2 5h12"/><path d="M7 2h1"/><path d="m22 22-5-10-5 10"/><path d="M14 18h6"/></svg>
                </button>
                <span class="msg-timestamp"></span>
            `;
                // Wire up action buttons
                actions.querySelector('[data-action="copy"]').onclick = () => {
                    const btn = actions.querySelector('[data-action="copy"]');
                    copyWithTick(btn, replyText(contentDiv)).catch((e) =>
                        console.warn('[CHAT] Copy failed:', e)
                    );
                };
                actions.querySelector('[data-action="speak"]').onclick = () => {
                    if (this.speech) {
                        // Stop any existing speech before starting new one
                        this.speech.cancelSpeech();
                        const text = replyText(contentDiv);
                        // speakText ignores the read-back gate (explicit request).
                        this.speech.speakText(text);
                    }
                };
                actions.querySelector('[data-action="translate"]').onclick = async () => {
                    const text = replyText(contentDiv);
                    if (!text.trim()) return;
                    try {
                        const config = await getConfig(this.invoke);
                        const lang = config.quick_actions?.translate_language || 'English';
                        this.elements.chatInput.value = `Translate the following to ${lang}:\n\n${text.substring(0, 500)}`;
                        this.elements.chatInput.focus();
                    } catch (e) {
                        console.warn('Translate failed:', e);
                    }
                };
                bubble.appendChild(actions);
            }

            msg.appendChild(avatar);
            msg.appendChild(bubble);

            return msg;
        }

        // --- Streaming Handlers ---

        /** Force the streaming renderer to paint the full accumulated text now.
         *  Called by the permission modal handler before showing the dialog so
         *  the user sees the complete streamed text behind it. */
        flushStreamingRender() {
            this.messageStreamController.flushStreamingRender();
        }

        handleMessageChunk(event) {
            return this.messageStreamController.handleChunk(event);
        }

        async handleMessageComplete() {
            return this.messageStreamController.handleComplete();
        }

        async handleMessageError(event) {
            return this.messageStreamController.handleError(event);
        }

        handleSessionReset(event) {
            return this.messageStreamController.handleSessionReset(event);
        }

        handleToolCallUpdate(event) {
            return this.messageStreamController.handleToolCallUpdate(event);
        }

        renderSourcesInMessage(contentDiv) {
            let sourcesEl = contentDiv.querySelector('.tool-sources');
            if (!sourcesEl) {
                sourcesEl = document.createElement('div');
                sourcesEl.className = 'tool-sources';
                contentDiv.appendChild(sourcesEl);
            }
            // In place: an open "shell" command list survives re-renders.
            renderToolSources(sourcesEl, this.toolUsages, this.toolSources);
            attachSourceClickHandler(sourcesEl, this.invoke);
            // Commands the user still has open in the live slot (outside
            // the bubble) move into the list, so each is listed once.
            const slot = contentDiv.parentElement?.querySelector(':scope > .shell-activity');
            handOffShellActivity(slot, sourcesEl);
        }

        /**
         * The streaming message's slot for running shell command blocks
         * (in place of the generic tool spinner line). A sibling right
         * after `.message-content`, which markdown re-renders on every
         * chunk — blocks in here keep their expanded state.
         */
        _shellActivitySlot(create = false) {
            const msg = this.currentStreamingMessage;
            const contentDiv = msg?.querySelector('.message-content');
            if (!contentDiv) return null;
            let slot = contentDiv.parentElement.querySelector(':scope > .shell-activity');
            if (!slot && create) {
                slot = createShellActivitySlot();
                contentDiv.after(slot);
            }
            return slot;
        }

        /**
         * Put a host-command card (an agent's command outside its sandbox,
         * shared/host-command-view.js) in the answer being written for
         * the session in view, right under its text. Returns false — the
         * card goes to the tray — for any other session.
         */
        placeHostCommandCard(card, prompt) {
            if (!prompt?.sessionId || prompt.sessionId !== this.activeSessionId) return false;
            const contentDiv = this.currentStreamingMessage?.querySelector('.message-content');
            if (!contentDiv) return false;
            let slot = contentDiv.parentElement.querySelector(':scope > .host-cmd-slot');
            if (!slot) {
                slot = document.createElement('div');
                slot.className = 'host-cmd-slot';
                slot.dir = 'auto';
                contentDiv.after(slot);
            }
            slot.append(card);
            this.scrollToBottom();
            return true;
        }

        _showShellCommand(record) {
            const slot = this._shellActivitySlot(true);
            if (!slot) return;
            // The command block takes over from any generic spinner line.
            const contentDiv = this.currentStreamingMessage.querySelector('.message-content');
            contentDiv?.querySelector('.tool-running-indicator')?.remove();
            showShellActivity(slot, record);
            this.scrollToBottom();
        }

        _pruneShellActivity() {
            const slot = this._shellActivitySlot();
            if (slot) pruneShellActivity(slot);
        }

        /** Turn over (complete / stop): settle running commands. */
        _finishShellActivity() {
            finishShellActivity(this._shellActivitySlot(), this.toolUsages);
        }

        /**
         * Render a loading indicator for an extension tool call into the per-message div.
         * Window-specific DOM — wired into the ExtensionToolController via host adapter.
         * Tool-usage tracking is handled by the controller before this fires.
         */
        _renderExtensionToolIndicator(info, contentDiv) {
            if (!contentDiv) return;
            const beforeFence = (this.currentStreamingContent || '')
                .split('```extension_tool_call')[0]
                .trim();
            if (beforeFence) {
                renderMarkdown(beforeFence, contentDiv, true);
            } else {
                const friendlyName = this.extensionToolController.getExtensionToolFriendlyName(
                    info.extension,
                    info.tool
                );
                contentDiv.innerHTML = `<div class="folder-plan-spinner-row"><span class="folder-plan-spinner"></span> ${escapeHtml(friendlyName)}...</div>`;
            }
        }

        // --- UI Helpers ---

        showTypingIndicator() {
            this.hideTypingIndicator();
            const indicator = document.createElement('div');
            indicator.className = 'typing-indicator';
            indicator.id = 'typingIndicator';
            indicator.innerHTML =
                '<div class="loading-dot"></div><div class="loading-dot"></div><div class="loading-dot"></div>';
            this.elements.messagesArea.appendChild(indicator);
            this.scrollToBottom();
        }

        hideTypingIndicator() {
            const el = document.getElementById('typingIndicator');
            if (el) el.remove();
        }

        updateInputState() {
            const btn = this.elements.sendBtn;
            const sendIcon = btn.querySelector('.send-icon');
            const stopIcon = btn.querySelector('.stop-icon');
            if (this.isWaitingForResponse) {
                btn.classList.add('stop-mode');
                btn.disabled = false;
                btn.setAttribute('aria-label', t('chat.send_btn.aria.stop'));
                btn.title = t('chat.send_btn.stop_title');
                if (sendIcon) sendIcon.style.display = 'none';
                if (stopIcon) stopIcon.style.display = '';
            } else {
                btn.classList.remove('stop-mode');
                btn.disabled = false;
                btn.setAttribute('aria-label', t('chat.send_btn.aria.send'));
                btn.title = t('chat.send_btn.send_title');
                if (sendIcon) sendIcon.style.display = '';
                if (stopIcon) stopIcon.style.display = 'none';
            }
            this.elements.chatInput.disabled = this.isWaitingForResponse;
        }

        async checkConnection() {
            try {
                this.isConnected = await this.invoke('check_connection');
            } catch (_e) {
                this.isConnected = false;
            }
            this.updateConnectionStatus();
        }

        setupNetworkMonitor() {
            onNetworkChange((online) => {
                this.updateConnectionStatus();
                if (!online) {
                    this.showError(offlineMessage());
                } else {
                    const container = this.elements.errorContainer;
                    if (container?.textContent?.includes(offlineMessage())) {
                        container.innerHTML = '';
                    }
                }
            });
            // Do a real connectivity check on startup
            checkOnline().then((online) => {
                if (!online) {
                    this.isConnected = false;
                    this.updateConnectionStatus();
                }
            });
        }

        updateConnectionStatus() {
            const el = this.elements.connectionStatus;
            if (!isOnline()) {
                el.textContent = t('chat.connection.offline');
                el.className = 'chat-header-status disconnected';
            } else if (this.isConnected) {
                el.textContent = t('chat.connection.connected');
                el.className = 'chat-header-status connected';
            } else {
                el.textContent = t('chat.connection.disconnected');
                el.className = 'chat-header-status disconnected';
            }
        }

        /**
         * Show an error with Reconnect and Dismiss. `opts.detail` adds the
         * agent's raw text as a second line; `opts.retryTurn` (onError) is
         * the failed turn, resent after a Reconnect when it still applies:
         * the window still shows the session the Reconnect started on, the
         * turn ran there, and nothing was sent since. `opts.signIn`: the
         * MESSAGE_ERROR payload — a signed-out agent's error object also
         * gets "Sign in" (shared/agent-sign-in.js), which resends
         * `opts.retryTurn` under the same rules; a plain string doesn't.
         */
        showError(message, opts = {}) {
            const detail = opts.detail
                ? `<div class="error-detail" dir="ltr">${escapeHtml(
                      t('shared.streaming.error_details', { detail: opts.detail })
                  )}</div>`
                : '';
            this.elements.errorContainer.innerHTML = `
            <div class="chat-error">
                <div class="chat-error-text"><span>${escapeHtml(message)}</span>${detail}</div>
                <div class="chat-error-actions">
                    <button class="chat-error-btn reconnect" id="errorReconnectBtn">${t('chat.error.btn.reconnect')}</button>
                    <button class="chat-error-btn dismiss" id="errorDismissBtn">${t('chat.error.btn.dismiss')}</button>
                </div>
            </div>
        `;

            document.getElementById('errorDismissBtn')?.addEventListener('click', () => {
                this.elements.errorContainer.innerHTML = '';
            });

            const reconnectBtn = document.getElementById('errorReconnectBtn');
            reconnectBtn?.addEventListener('click', async () => {
                reconnectBtn.disabled = true;
                reconnectBtn.textContent = t('chat.error.btn.reconnecting');
                // The session this Reconnect is for. The user may select
                // another while the agent restarts: then the outcome is
                // not this window's to act on.
                const startedOn = this.activeSessionId ?? null;
                const moved = () => this.activeSessionId !== startedOn;
                // A failed attempt's error keeps the failed turn, so the
                // next Reconnect can still resend it.
                // (and, for a signed-out agent, its Sign in).
                const keep = {
                    retryTurn: opts.retryTurn,
                    signIn: opts.signIn,
                    establishSession: opts.establishSession,
                };
                let outcome;
                try {
                    outcome = await this.invoke('reconnect_acp', { sessionId: startedOn });
                } catch (e) {
                    if (moved()) {
                        console.warn('[chat] reconnect failed for a session no longer shown');
                        return;
                    }
                    this.showError(errLabel(t('chat.error.reconnect_failed_label'), e), keep);
                    return;
                }
                if (!outcome?.reconnected) {
                    if (!moved()) this.showError(t('chat.error.reconnect_failed'), keep);
                    return;
                }
                this.isConnected = true;
                this.updateConnectionStatus();
                if (moved()) {
                    // reconnect_acp pinned the window to the session it
                    // reloaded; the window shows another one now. Keep that
                    // one, and resend nothing.
                    console.log(
                        '[chat] reconnect finished after the window moved to',
                        this.activeSessionId
                    );
                    this._repinWindowSession();
                    return;
                }
                // Only this error's box: a newer one (or a new prompt that
                // cleared it) stays.
                if (reconnectBtn.isConnected) this.elements.errorContainer.innerHTML = '';
                if (opts.establishSession) {
                    // The window's session never got set up: set it up on
                    // the restarted agent (a failure shows its own error).
                    await opts.establishSession(outcome).catch(() => {});
                    return;
                }
                const shown = startedOn;
                this._adoptReconnectSession(outcome.sessionId);
                const turn = opts.retryTurn;
                if (this._canResendTurn(turn, shown)) {
                    await this._resendTurn(turn);
                }
            });

            if (opts.signIn) {
                const actions = this.elements.errorContainer.querySelector('.chat-error-actions');
                const host = opts.establishSession
                    ? this._sessionSetupSignInHost(opts.signIn, opts.establishSession, actions)
                    : this._agentSignInHost(opts.signIn, opts.retryTurn ?? null, actions);
                mountSignInAction?.(actions, opts.signIn, host)?.catch((e) =>
                    console.warn('[chat] sign-in action unavailable:', e)
                );
            }
        }

        /**
         * Show `error`, a failure to set up this window's session
         * (`switch_acp_session` for a selection, a New Chat or the window's
         * first session), as `text`. A signed-out agent's error also gets
         * Sign in (shared/agent-sign-in.js), and both it and Reconnect then
         * set the session up again on the restarted agent with
         * `retry(outcome)` (`outcome`: reconnect_acp's answer), which
         * resolves to the session id or rejects having shown its own error.
         * `sessionId` is the session the window shows meanwhile (the one
         * the restart reloads). With `onlyIfSignIn`, any other error shows
         * nothing (the caller's old behaviour). Returns whether it showed.
         */
        _showSessionSetupError(text, error, { sessionId = null, retry, onlyIfSignIn = false }) {
            if (!canOfferSignIn?.(error)) {
                if (onlyIfSignIn) return false;
                this.showError(text);
                return true;
            }
            this.showError(text, {
                signIn: { ...error, sessionId },
                establishSession: retry,
            });
            return true;
        }

        /**
         * The sign-in under a session-setup error (`_showSessionSetupError`):
         * as a prompt's (`_agentSignInHost`) but nothing was waiting to be
         * sent. After the restart it sets up the session the window wanted
         * (`retry`), unless the window has moved on to another session
         * since; then it keeps that one.
         */
        _sessionSetupSignInHost(payload, retry, ownActions = null) {
            const base = this._agentSignInHost(payload, null, ownActions);
            const wanted = payload?.sessionId ?? null;
            return {
                ...base,
                establishSession: async (outcome) => {
                    if ((this.activeSessionId ?? null) !== wanted) {
                        console.log(
                            '[chat] sign-in finished after the window moved to',
                            this.activeSessionId
                        );
                        this._repinWindowSession();
                        return null;
                    }
                    // This error's box goes; a failure shows a new one.
                    if (!ownActions || ownActions.isConnected) {
                        this.elements.errorContainer.innerHTML = '';
                    }
                    return retry(outcome);
                },
                canResend: () => false,
                resend: async () => 'none',
            };
        }

        /** Take the session id `reconnect_acp` answered with, if it moved. */
        _adoptReconnectSession(sid) {
            if (!sid || sid === this.activeSessionId) return;
            console.log('[chat] adopting reconnect session id:', sid);
            this.activeSessionId = sid;
            this.currentAcpSessionId = sid;
            this.invoke('set_window_session', {
                label: this.windowLabel,
                sessionId: sid,
            }).catch(() => {});
        }

        /**
         * Whether the failed turn `turn` (onError's retryTurn: null when any
         * of it got through or it was a rate limit) may go out again now:
         * nothing was sent since, it went to `shown` (the session the window
         * showed before adopting a reconnect's id) and the window can send.
         */
        _canResendTurn(turn, shown) {
            return (
                !!turn &&
                turn.seq === this._sendSeq &&
                turn.sessionId === shown &&
                this.canSendAgentPrompt(this.activeSessionId)
            );
        }

        /**
         * The chat window's side of the agent sign-in for one error: the run
         * lives in #agentSignInSlot, outside #errorContainer. `payload` is
         * that error and `retryTurn` its failed turn (onError): the sign-in
         * resends only that turn, by Reconnect's rules — the error names it
         * (`turnId`) and says it may go again (isResendablePayload), the
         * window still shows its session and sent nothing since
         * (_canResendTurn) — and at most once. The resend records a turn
         * of its own, with a fresh id (_resendTurn).
         */
        _agentSignInHost(payload, retryTurn, ownActions = null) {
            // The session the window showed when the restart answered,
            // before adopting a reloaded id; undefined until one answers.
            let shown;
            let resent = false;
            return {
                slot: () => document.getElementById('agentSignInSlot'),
                rows: 16,
                compact: false,
                buttonClass: 'chat-error-btn reconnect',
                invoke: (cmd, args) => this.invoke(cmd, args),
                layoutChanged: () => {},
                onActiveChange: () => {},
                onEnded: async ({ outcome, agent }) => {
                    // The user was most likely in the browser: bring the
                    // chat back to show the result (and the resend). Not at
                    // the time limit; a notification says so instead.
                    try {
                        if (outcome === 'timed_out') {
                            await sendAppNotification?.(
                                this.invoke,
                                t('shared.notify.kage_title'),
                                t('shared.agent_sign_in.notify.timed_out', { agent }),
                                this.windowLabel || WINDOW?.MAIN
                            );
                        } else if (outcome === 'exited' && this.appWindow) {
                            await presentOverlayWindow?.(this.appWindow);
                        }
                    } catch (e) {
                        console.warn('[chat] after sign-in:', e);
                    }
                },
                // The restart pinned the window to `sid` (the failed
                // session `from`, reloaded): follow it only while the window
                // still shows `from`; else keep the session shown now.
                adoptSession: (sid, from) => {
                    shown = this.activeSessionId ?? null;
                    if (from && from !== shown) {
                        console.log('[chat] sign-in finished after the window moved to', shown);
                        this._repinWindowSession();
                        return;
                    }
                    this._adoptReconnectSession(sid);
                },
                canResend: (sid) => {
                    const before = shown === undefined ? (this.activeSessionId ?? null) : shown;
                    shown = undefined;
                    return (
                        !resent &&
                        !!retryTurn &&
                        sid === this.activeSessionId &&
                        isResendablePayload(payload, retryTurn.turnId) &&
                        this._canResendTurn(retryTurn, before)
                    );
                },
                resend: async () => {
                    if (resent) return 'none';
                    resent = true;
                    // Clear this sign-in's error before the resend, and only
                    // while it is still the one shown (as Reconnect does):
                    // the resent turn's own error, if it fails fast, must
                    // stay on screen.
                    if (!ownActions || ownActions.isConnected) {
                        this.elements.errorContainer.innerHTML = '';
                    }
                    await this._resendTurn(retryTurn, { throwOnError: true });
                    return 'sent';
                },
                // Cleared before the resend (above); after it, the box may
                // hold the resent turn's own error.
                clearError: () => {},
            };
        }

        /** Empty #agentSignInSlot, unless a sign-in still holds it. */
        _clearAgentSignInSlot() {
            clearAgentSignInSlot?.(document.getElementById('agentSignInSlot'));
        }

        async showSessionLocked(sessionId, pid) {
            let processInfo = '';
            if (pid) {
                try {
                    const name = await this.invoke('get_process_name', { pid: parseInt(pid, 10) });
                    processInfo = name ? ` (${name}, PID ${pid})` : ` (PID ${pid})`;
                } catch {
                    processInfo = ` (PID ${pid})`;
                }
            }
            this.elements.errorContainer.innerHTML = `
                <div class="chat-error chat-warning">
                    <span>${t('chat.error.session_locked', { processInfo: escapeHtml(processInfo) })}</span>
                    <div class="chat-error-actions">
                        <button class="chat-error-btn retry" id="errorRetryBtn">${t('chat.error.btn.retry')}</button>
                    </div>
                </div>
            `;

            document.getElementById('errorRetryBtn')?.addEventListener('click', async () => {
                this.elements.errorContainer.innerHTML = '';
                try {
                    await this.invoke('switch_acp_session', { sessionId });
                    this.isConnected = true;
                    this.updateConnectionStatus();
                    this.elements.chatInput.disabled = false;
                    this.elements.chatInput.placeholder = t('chat.placeholder.type_message');
                    this.elements.sendBtn.disabled = false;
                    this.elements.chatInput.focus();
                } catch (error) {
                    const msg = this.formatError(error);
                    const isLocked =
                        msg.includes('active in another process') ||
                        msg.includes('Session is active');
                    if (isLocked) {
                        const retryPidMatch = msg.match(/PID\s+(\d+)/);
                        this.showSessionLocked(sessionId, retryPidMatch ? retryPidMatch[1] : null);
                    } else {
                        this.showError(msg);
                    }
                }
            });
        }

        showSessionResetMessage(message) {
            // Show as an inline system message in the chat area
            const placeholder = this.elements.messagesArea.querySelector('.message-placeholder');
            if (placeholder) placeholder.remove();

            const msgEl = document.createElement('div');
            msgEl.className = 'session-reset-notice';
            msgEl.innerHTML = `<span>${escapeHtml(message)}</span>`;
            this.elements.messagesArea.appendChild(msgEl);
            this.scrollToBottom();
        }

        /**
         * Format an error for display — extracts message and data from structured errors
         */
        formatError(error) {
            return formatErrorShared(error);
        }

        /**
         * Export the active chat to a Markdown file.
         *
         * Uses the same `messages` array we render from, so what the user
         * sees on screen is what they get in the export — minus tool
         * call internals and the streaming-state UI chrome. Title +
         * model + session id come from the existing on-screen state.
         */
    };
}
