import {
    attachSourceClickHandler,
    escapeHtml,
    getActionsForText,
    getConfig,
    renderMarkdown,
    renderSourceBubblesHtml,
    renderSourceChipsHtml,
    renderToolChipsHtml,
    t,
} from './dependencies.js';

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
                    // Stop any existing speech before starting new one
                    this.speech.cancelSpeech();
                    this.speech.usedSpeechForLastMessage = true;
                    this.speech.speakResponse(this.currentResponse);
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
                        const prompt = action.prompt.replace(/\{text\}/g, responseText);
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
        sourcesEl.innerHTML =
            renderToolChipsHtml(this.toolUsages) + renderSourceChipsHtml(this.toolSources);
        attachSourceClickHandler(sourcesEl, this.invoke);
        this.windowManager.resizeWindow();
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

        if (opts.reconnect) {
            const btn = document.createElement('button');
            btn.className = 'floating-error-reconnect';
            btn.textContent = t('chat.error.btn.reconnect');
            btn.addEventListener('click', async () => {
                btn.disabled = true;
                try {
                    const success = await this.invoke('reconnect_acp');
                    if (success) {
                        this.elements.responseText.textContent = '';
                        this.elements.contentArea.classList.remove('visible');
                        this.elements.expandBtn.classList.remove('visible');
                        this.windowManager.resizeWindow();
                        return;
                    }
                } catch (e) {
                    console.log('Reconnect failed:', e);
                }
                btn.disabled = false;
            });
            this.elements.responseText.appendChild(document.createElement('br'));
            this.elements.responseText.appendChild(btn);
        }

        this.windowManager.resizeWindow();
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
            await this.invoke('open_chat_window');
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
