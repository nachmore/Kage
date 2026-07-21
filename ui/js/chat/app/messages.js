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
        renderToolChipsHtml,
        renderSourceChipsHtml,
        attachSourceClickHandler,
        getConfig,
        ExtensionToolController,
        formatErrorShared,
    } = dependencies;
    return class {
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

            // Timestamp for user messages
            if (role === 'user') {
                const tsEl = document.createElement('div');
                tsEl.className = 'message-actions user-actions';
                tsEl.innerHTML = '<span class="msg-timestamp"></span>';
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
                    const text = contentDiv.textContent || '';
                    navigator.clipboard.writeText(text).then(() => {
                        const btn = actions.querySelector('[data-action="copy"]');
                        btn.innerHTML =
                            '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>';
                        setTimeout(() => {
                            btn.innerHTML =
                                '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>';
                        }, 1500);
                    });
                };
                actions.querySelector('[data-action="speak"]').onclick = () => {
                    if (this.speech) {
                        // Stop any existing speech before starting new one
                        this.speech.cancelSpeech();
                        const text = contentDiv.textContent || '';
                        this.speech.usedSpeechForLastMessage = true;
                        this.speech.speakResponse(text);
                    }
                };
                actions.querySelector('[data-action="translate"]').onclick = async () => {
                    const text = contentDiv.textContent || '';
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
            sourcesEl.innerHTML =
                renderToolChipsHtml(this.toolUsages) + renderSourceChipsHtml(this.toolSources);
            attachSourceClickHandler(sourcesEl, this.invoke);
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

        showError(message) {
            this.elements.errorContainer.innerHTML = `
            <div class="chat-error">
                <span>${escapeHtml(message)}</span>
                <div class="chat-error-actions">
                    <button class="chat-error-btn reconnect" id="errorReconnectBtn">${t('chat.error.btn.reconnect')}</button>
                    <button class="chat-error-btn dismiss" id="errorDismissBtn">${t('chat.error.btn.dismiss')}</button>
                </div>
            </div>
        `;

            document.getElementById('errorDismissBtn')?.addEventListener('click', () => {
                this.elements.errorContainer.innerHTML = '';
            });

            document.getElementById('errorReconnectBtn')?.addEventListener('click', async () => {
                try {
                    const success = await this.invoke('reconnect_acp');
                    if (success) {
                        this.isConnected = true;
                        this.updateConnectionStatus();
                        this.elements.errorContainer.innerHTML = '';
                    } else {
                        this.showError(t('chat.error.reconnect_failed'));
                    }
                } catch (e) {
                    this.showError(errLabel(t('chat.error.reconnect_failed_label'), e));
                }
            });
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
