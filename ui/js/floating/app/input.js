import {
    cmdOrCtrlPressed,
    errLabel,
    getConfig,
    handleEnterAction,
    messageLengthBucket,
    submitSelection,
    t,
    trackEvent,
    updateSelection,
} from './dependencies.js';

export const InputMethods = {
    async executeCommandAction(cmd) {
        this._clearInput();
        await cmd.execute(this.invoke, this.appWindow);
    },

    async executeSelection(command, value) {
        this.clearSuggestions();
        try {
            // Shared submit — sends { <command>Name: value }, the arg-shape the
            // agent actually accepts (verified via scripts/probe_slash.py).
            const msg = await submitSelection(this.invoke, this.floatingSessionId, command, value);
            document.dispatchEvent(
                new CustomEvent('kage-show-response', {
                    detail: msg || t('floating.suggestions.selection_fallback', { value }),
                })
            );
        } catch (e) {
            document.dispatchEvent(
                new CustomEvent('kage-show-response', {
                    detail: errLabel(t('floating.error.error_label'), e),
                })
            );
        }
    },

    async handleKeyDown(event) {
        if (event.key === 'Tab') {
            event.preventDefault();
            // Cycle through suggestions on repeated Tab presses
            if (this.currentMatches.length > 0) {
                if (this._tabCycleActive) {
                    this._tabCycleIndex = (this._tabCycleIndex + 1) % this.currentMatches.length;
                } else {
                    this._tabCycleIndex = 0;
                    this._tabCycleActive = true;
                }
                const pick = this.currentMatches[this._tabCycleIndex];
                if (pick.type === 'command') {
                    this.elements.input.value = '>' + pick.name + ' ';
                } else if (pick.type === 'slash') {
                    this.elements.input.value = pick.name + ' ';
                } else if (pick.name) {
                    this.elements.input.value = pick.name;
                }
                this.selectedIndex = this._tabCycleIndex;
                updateSelection(this.elements.appSuggestions, this.selectedIndex);
            }
        } else if (event.key === 'ArrowDown') {
            // History navigation: if browsing history, go forward
            if (this._historyIndex >= 0 && this.currentMatches.length === 0) {
                event.preventDefault();
                this._historyIndex--;
                if (this._historyIndex < 0) {
                    // Back to the original input
                    this.elements.input.value = this._historySaved;
                    this._historySaved = '';
                } else {
                    this.elements.input.value = this._messageHistory[this._historyIndex];
                }
                return;
            }
            const itemCount =
                this.elements.appSuggestions.querySelectorAll('.app-suggestion-item').length;
            if (itemCount > 0) {
                // Only navigate suggestions if cursor is on the last line of the textarea
                const ta = this.elements.input;
                const _textBeforeCursor = ta.value.substring(0, ta.selectionStart);
                const textAfterCursor = ta.value.substring(ta.selectionEnd);
                const isLastLine = !textAfterCursor.includes('\n');
                if (isLastLine) {
                    event.preventDefault();
                    this.selectedIndex = (this.selectedIndex + 1) % itemCount;
                    updateSelection(this.elements.appSuggestions, this.selectedIndex);
                }
            }
            // When no suggestions or not on last line, let default behavior handle cursor movement
        } else if (event.key === 'ArrowUp') {
            // History navigation: if input is empty (or already browsing) and no suggestions
            if (this._messageHistory.length > 0 && this.currentMatches.length === 0) {
                const inputVal = this.elements.input.value;
                const isEmpty = inputVal.trim() === '' || this._historyIndex >= 0;
                if (isEmpty && this._historyIndex < this._messageHistory.length - 1) {
                    event.preventDefault();
                    if (this._historyIndex < 0) {
                        this._historySaved = inputVal; // stash whatever was typed
                    }
                    this._historyIndex++;
                    this.elements.input.value = this._messageHistory[this._historyIndex];
                    return;
                }
            }
            const itemCount =
                this.elements.appSuggestions.querySelectorAll('.app-suggestion-item').length;
            if (itemCount > 0) {
                // Only navigate suggestions if cursor is on the first line of the textarea
                const ta = this.elements.input;
                const textBeforeCursor = ta.value.substring(0, ta.selectionStart);
                const isFirstLine = !textBeforeCursor.includes('\n');
                if (isFirstLine) {
                    event.preventDefault();
                    this.selectedIndex =
                        this.selectedIndex <= 0 ? itemCount - 1 : this.selectedIndex - 1;
                    updateSelection(this.elements.appSuggestions, this.selectedIndex);
                }
            }
            // When no suggestions or not on first line, let default behavior handle cursor movement
        } else if (event.key === 'Backspace') {
            // Empty-input backspace dismisses the App Mode chip (same
            // effect as clicking it). Lets users back out of a matched
            // mode without reaching for the mouse. Only fires when the
            // input is genuinely empty so normal text editing isn't
            // affected; the click handler in `_refreshAppModeChip` is
            // still the canonical clear path.
            if (this.elements.input.value === '' && this._appModeMatch && !event.repeat) {
                const chip = document.getElementById('appModeChip');
                if (chip && chip.style.display !== 'none') {
                    event.preventDefault();
                    this._appModeMatch = null;
                    chip.style.display = 'none';
                    return;
                }
            }
        } else if (event.key === 'Escape') {
            if (this._clipboardMode) {
                event.preventDefault();
                this._restoreOverlaysAfterClipboard();
                this._clipboardMode = false;
                this._clipboardEntries = null;
                this._clearInput();
                return;
            }
            if (this.isWaitingForResponse) {
                event.preventDefault();
                this.stopGenerating();
            } else if (this._justStoppedGenerating) {
                event.preventDefault();
            } else {
                await this.appWindow.hide();
            }
        } else if (event.key === 'Enter' && cmdOrCtrlPressed(event) && event.shiftKey) {
            // Ctrl/⌘+Shift+Enter: execute as elevated (admin) if it's a system command
            event.preventDefault();
            if (this.currentMatches.length > 0 && this.selectedIndex >= 0) {
                const selected = this.currentMatches[this.selectedIndex];
                if (selected.type === 'system') {
                    await this._executeSystemCommand(selected.cmdId, selected.needsConfirm, true);
                    return;
                }
            }
        } else if (event.key === 'Enter' && cmdOrCtrlPressed(event)) {
            // Ctrl/⌘+Enter: send directly to agent, bypassing suggestions and input classification
            event.preventDefault();
            const message = this.elements.input.value.trim();
            if (message) {
                await this.clearSuggestions();
                await this.sendChatMessage(message, { forceChat: true });
            }
        } else if (event.key === 'Enter' && !event.shiftKey && !cmdOrCtrlPressed(event)) {
            event.preventDefault();
            await this.handleEnterKey();
        }
    },

    async handleEnterKey() {
        // Cancel any pending debounced search so we don't use stale suggestions.
        // When typing fast, the last input event's debounce may not have fired yet,
        // meaning currentMatches reflects an older, partial query.
        if (this.searchTimeout) {
            clearTimeout(this.searchTimeout);
            this.searchTimeout = null;
            // Stale suggestions — clear them so handleEnterAction falls through
            // to direct shortcut/command matching on the actual input value.
            this.currentMatches = [];
            this.selectedIndex = -1;
        }

        const message = this.elements.input.value.trim();
        const hasAttachments = this.attachmentManager.hasAttachments();
        const hasSelection = this.currentMatches.length > 0 && this.selectedIndex >= 0;

        // Clipboard history mode — paste selected item into the previously focused app
        if (this._clipboardMode && hasSelection) {
            const selected = this.currentMatches[this.selectedIndex];
            if (selected.type === 'clipboard' && selected.data?.text) {
                this._clearInput();
                await this.appWindow.hide();
                // Small delay to let the previous window regain focus
                await new Promise((r) => setTimeout(r, 150));
                try {
                    await this.invoke('paste_clipboard_item', { text: selected.data.text });
                    console.log(
                        '[Clipboard] Pasted to active app:',
                        selected.data.text.slice(0, 50)
                    );
                } catch (e) {
                    console.warn('[Clipboard] Failed to paste:', e);
                }
                return;
            }
        }

        if (!message && !hasAttachments && !hasSelection) return;

        const result = await handleEnterAction({
            message,
            suggestions: this.currentMatches,
            selectedIndex: this.selectedIndex,
            shortcuts: this.shortcuts,
            ctx: this._getExecCtx(),
            onSend: (msg) => this.sendChatMessage(msg),
            onSystemCommand: (cmdId, needsConfirm, elevated) =>
                this._executeSystemCommand(cmdId, needsConfirm, elevated),
            onSelection: (command, value) => this.executeSelection(command, value),
        });

        await this._applyEnterActionResult(result);
    },

    /**
     * Apply the {handled, action} verdict from handleEnterAction. Shared by
     * the Enter key (handleEnterKey) and click-to-execute (_onResultClick) so
     * both paths treat hide / replace_input / keep_suggestions identically.
     */
    async _applyEnterActionResult(result) {
        if (!result?.handled) return;
        if (result.action === 'replace_input') {
            /* input already replaced by onReplaceInput callback */
        } else if (result.action === 'hide') {
            this.resetUI();
            await this.appWindow.hide();
        } else if (result.action === 'keep_suggestions') {
            // A selection picker was just rendered into the suggestions
            // dropdown (e.g. /agent, /model). Clear the input text but
            // DON'T touch the suggestions — clearSuggestions() would wipe
            // the picker we just painted, which was the silent-failure bug.
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
        } else {
            this._clearInput();
        }
    },

    /**
     * Click-to-execute for a unified-search result row. Mirrors pressing
     * Enter on that row: runs it through the same handleEnterAction machinery
     * (so slash commands, system commands, shortcuts, apps, URLs all behave
     * identically whether clicked or keyed) and applies the same verdict.
     */
    async _onResultClick(result) {
        if (!result) return;
        // Point the selection at the clicked row so handleEnterAction executes
        // exactly it, regardless of what was keyboard-highlighted.
        const idx = this.currentMatches.indexOf(result);
        if (idx < 0) return;
        this.selectedIndex = idx;

        // Clipboard mode is paste-on-select — handle it the same way Enter does.
        if (this._clipboardMode && result.type === 'clipboard' && result.data?.text) {
            this._clearInput();
            await this.appWindow.hide();
            await new Promise((r) => setTimeout(r, 150));
            try {
                await this.invoke('paste_clipboard_item', { text: result.data.text });
            } catch (e) {
                console.warn('[Clipboard] Failed to paste:', e);
            }
            return;
        }

        const actionResult = await handleEnterAction({
            message: this.elements.input.value.trim(),
            suggestions: this.currentMatches,
            selectedIndex: this.selectedIndex,
            shortcuts: this.shortcuts,
            ctx: this._getExecCtx(),
            onSend: (msg) => this.sendChatMessage(msg),
            onSystemCommand: (cmdId, needsConfirm, elevated) =>
                this._executeSystemCommand(cmdId, needsConfirm, elevated),
            onSelection: (command, value) => this.executeSelection(command, value),
        });
        await this._applyEnterActionResult(actionResult);
    },

    _clearInput() {
        this.elements.input.value = '';
        this.elements.input.style.height = 'auto';
        this.clearSuggestions();
    },

    async sendChatMessage(message, options = {}) {
        // If bootstrap is still running, show a "Spinning up agent…"
        // placeholder and queue the send. The bootstrap path will
        // call _flushPendingSend() once the session arrives.
        if (this.bootstrappingSession) {
            this._showBootstrapSpinner();
            this._pendingSend = { message, options };
            this._waitForBootstrapAndSend();
            return;
        }
        // Bootstrap previously failed. Rather than latching that error until
        // the app restarts (the old behaviour — a transient backend outage at
        // launch left floating permanently dead), retry the bootstrap. It's
        // debounced so rapid sends can't cascade into repeated reconnects.
        if (this.sessionBootstrapError) {
            this._retryBootstrapAndSend(message, options);
            return;
        }

        // Track message in shell-style history (skip duplicates of the last entry)
        if (
            message.trim() &&
            (this._messageHistory.length === 0 || this._messageHistory[0] !== message.trim())
        ) {
            this._messageHistory.unshift(message.trim());
            if (this._messageHistory.length > 50) this._messageHistory.pop();
        }
        this._historyIndex = -1;
        this._historySaved = '';

        // Stop any ongoing TTS; in voice mode, don't kill the mic — it will restart after response
        if (this.speech) {
            this.speech.cancelSpeech();
            if (this.speech.isListening && !this.speech.voiceMode) {
                this.speech.stop();
            }
        }

        // If a plan is pending review, send the message as a revision request
        if (this._pendingPlanRevision) {
            this.automationPlanController.reset();
            this.extensionToolController.reset();
            // Reset UI for the new response
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
            this.currentResponse = '';
            this.elements.responseText.textContent = '';
            this.elements.contentArea.classList.add('visible');
            this.isWaitingForResponse = true;
            this._promptGeneration++;
            this.startThinking();
            this.updateDatetimeVisibility();
            await this.windowManager.resizeWindow();
            try {
                // Notify the chat window so it can show the user bubble
                window.__TAURI__.event.emit('floating_message_sent', { message });
                trackEvent('message_sent', {
                    source: 'floating',
                    length: messageLengthBucket(message),
                });
                await this.invoke('send_message_streaming', {
                    sessionId: this.floatingSessionId,
                    message,
                    attachments: null,
                });
            } catch (e) {
                this.showError(errLabel(t('floating.error.error_label'), e));
            }
            return;
        }

        const attachments = this.attachmentManager.toContentBlocks();
        this.attachmentManager.clear();

        // Include selected text as context if checkbox is checked
        const useSelection =
            !options.skipSelection && document.getElementById('useSelectionCheckbox')?.checked;
        if (useSelection && this.lastSelection?.trim()) {
            message = `The following text is currently selected in my active window:\n\`\`\`\n${this.lastSelection.trim()}\n\`\`\`\n\n${message}`;
        }
        // Hide selection indicator after use
        const indicator = document.getElementById('selectionIndicator');
        if (indicator) indicator.style.display = 'none';
        this._setSelectionPreview(null);
        const quickActionsContainer = document.getElementById('quickActionsContainer');
        if (quickActionsContainer) quickActionsContainer.style.display = 'none';
        const responseActionsContainer = document.getElementById('responseActionsContainer');
        if (responseActionsContainer) responseActionsContainer.style.display = 'none';
        this.lastSelection = null;

        this.elements.input.value = '';
        this.elements.input.style.height = 'auto';
        this.elements.appSuggestions.classList.remove('visible');
        this.currentMatches = [];
        this.selectedIndex = -1;

        // Dismiss any pending permission request blocking OUR session so
        // it isn't stalled waiting for a response. Scoped by session id —
        // other windows' pending permissions are theirs to answer.
        try {
            await this.invoke('dismiss_pending_permission', {
                sessionId: this.floatingSessionId ?? null,
            });
        } catch (e) {
            console.log('No pending permission to dismiss:', e);
        }

        try {
            // If forceChat, attachments present, or we already know there's no match, skip classification
            let rustResults = [];
            if (options.forceChat || attachments) {
                rustResults = [];
            } else if (this._noMatchSinceLen > 0 && message.length >= this._noMatchSinceLen) {
                rustResults = [];
            } else {
                try {
                    const json = await this.invoke('handle_floating_input', { input: message });
                    rustResults = JSON.parse(json);
                } catch {
                    rustResults = [];
                }
            }
            this._noMatchSinceLen = 0;

            // Check if the top result is a URL, path, or app launch
            const top = rustResults[0];
            if (top?.type === 'url') {
                await this.openUrl(top.value);
            } else if (top?.type === 'path') {
                await this.openPath(top.value);
            } else if (top?.type === 'app') {
                await this.launchApp(top.name);
            } else {
                // No actionable match — send to agent. Reset UI now.
                // If a response is in progress, cancel it first
                if (this.isWaitingForResponse) {
                    this.invoke('cancel_generation', {
                        sessionId: this.floatingSessionId,
                    }).catch((e) => console.log('Cancel:', e));
                    this.isWaitingForResponse = false;
                    this.stopThinking();
                    this.elements.floatingStopBtn.style.display = 'none';
                    const indicator =
                        this.elements.responseText.querySelector('.streaming-indicator');
                    if (indicator) indicator.remove();
                }

                this.elements.contentArea.classList.remove('visible');
                this.toolSources = [];
                this.toolUsages = [];
                this._toolCallIds = new Set();
                this._sourceDomains = new Set();
                const sourcesEl2 = document.getElementById('toolSources');
                if (sourcesEl2) sourcesEl2.remove();
                const compactEl2 = document.getElementById('toolSourcesCompact');
                if (compactEl2) compactEl2.remove();
                await this.windowManager.resetHeightForNewMessage();
                this.startThinking();
                this.updateDatetimeVisibility();
                this.elements.expandBtn.classList.remove('visible');

                // No actionable match — send to agent
                this.currentResponse = '';
                this.elements.responseText.textContent = this.currentResponse;
                this.elements.contentArea.classList.add('visible');
                this.elements.expandBtn.classList.add('visible');
                this.isWaitingForResponse = true;
                this.extensionToolController.reset();
                this.automationPlanController.reset();
                this._promptGeneration++;
                const _gen = this._promptGeneration;
                await this.windowManager.resizeWindow();
                this.banner.dismiss();

                // Prepend screen context (source window info) and any
                // App Mode steering. Both ride at the head of the
                // outgoing prompt so the agent sees them before the
                // actual user message. App-mode steering travels with
                // every prompt where it applies — consciously kept
                // light (per-rule cap of 500 chars) so token cost
                // stays small even on long conversations.
                try {
                    const config = await getConfig(this.invoke);
                    if (config?.system?.screen_context) {
                        const sw = await this.invoke('get_source_window');
                        if (sw) {
                            message = `<_kage_ctx app="${sw.processName}" title="${sw.title}"/>\n${message}`;
                        }
                    }
                } catch (e) {
                    console.log('Screen context unavailable:', e);
                }

                // App Mode steering — _appModeMatch was set by
                // _refreshAppModeChip when the user summoned. Click-
                // dismiss clears it without touching config; we just
                // skip the splice in that case.
                if (this._appModeMatch?.steering_payload) {
                    message = `${this._appModeMatch.steering_payload}\n${message}`;
                }

                // Notify the chat window so it can show the user bubble
                window.__TAURI__.event.emit('floating_message_sent', { message });
                trackEvent('message_sent', {
                    source: 'floating',
                    length: messageLengthBucket(message),
                    attachments: attachments?.length || 0,
                });
                await this.invoke('send_message_streaming', {
                    sessionId: this.floatingSessionId,
                    message,
                    attachments,
                });
            }
        } catch (error) {
            console.error('Error handling input:', error);
            this.showError(errLabel(t('floating.error.error_label'), error));
        }
    },

    /** Force the streaming renderer to paint the full accumulated text now.
     *  Called by the permission modal handler before showing the dialog so
     *  the user sees the complete streamed text behind it. */
};
