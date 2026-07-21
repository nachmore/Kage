import {
    cmdOrCtrlPressed,
    EVT,
    getActionsForText,
    getConfig,
    handlePasteEvent,
    renderAttachmentPreviews,
    renderMarkdown,
    renderQuickActionChips,
    setExtensionManager,
    updateSelection,
    WINDOW,
} from './dependencies.js';

export const LifecycleEventsMethods = {
    _initializeExtensionsInBackground(_ts) {
        this.extensionManager
            .initialize()
            .then(() => {
                _ts?.('Extensions initialized (background)');
                setExtensionManager(this.extensionManager);
                this.extensionToolController.sendSteering();
                if (this._onExtensionsReady) this._onExtensionsReady();
                _ts?.('Extension steering sent (background)');
            })
            .catch((e) => {
                console.warn('Background extension init failed:', e);
            });
    },

    cacheElements() {
        this.elements = {
            input: document.getElementById('promptInput'),
            appSuggestions: document.getElementById('appSuggestions'),
            contentArea: document.getElementById('contentArea'),
            responseText: document.getElementById('responseText'),
            loadingDots: document.getElementById('loadingDots'),
            loadingElapsed: document.getElementById('loadingElapsed'),
            expandBtn: document.getElementById('expandBtn'),
            floatingStopBtn: document.getElementById('floatingStopBtn'),
            mascotContainer: document.querySelector('.mascot-container'),
            attachmentPreviews: document.getElementById('attachmentPreviews'),
            datetimeDisplay: document.getElementById('datetimeDisplay'),
            speechBtn: document.getElementById('speechBtn'),
            speechWave: document.getElementById('speechWave'),
            floatingToolbar: document.getElementById('floatingToolbar'),
            floatingAttachFileBtn: document.getElementById('floatingAttachFileBtn'),
            floatingAttachImageBtn: document.getElementById('floatingAttachImageBtn'),
            floatingFileInput: document.getElementById('floatingFileInput'),
            floatingImageInput: document.getElementById('floatingImageInput'),
            floatingToolbarExt: document.getElementById('floatingToolbarExt'),
            floatingContextIndicator: document.getElementById('floatingContextIndicator'),
            floatingContextRing: document.getElementById('floatingContextRing'),
            floatingContextPercent: document.getElementById('floatingContextPercent'),
            floatingModelSelector: document.getElementById('floatingModelSelector'),
            floatingModelName: document.getElementById('floatingModelName'),
        };
    },

    /**
     * Single source of truth for datetime visibility.
     * Call this instead of directly manipulating the datetime element.
     */
    updateDatetimeVisibility() {
        const dt = this.elements.datetimeDisplay;
        if (!dt) return;
        // Don't show if datetime is disabled in config (managed by applyDateTime in theme.js)
        const configEnabled = dt.dataset.enabled === '1';
        // Hide if: not configured, streaming, stop button visible, input has text, quick actions visible, or speech listening
        const stopVisible = this.elements.floatingStopBtn.style.display !== 'none';
        const hasInput = this.elements.input.value.length > 0;
        const qaVisible =
            document.getElementById('quickActionsContainer')?.style.display === 'flex' ||
            document.getElementById('responseActionsContainer')?.style.display === 'flex';
        const dtHidden =
            !configEnabled ||
            this.isWaitingForResponse ||
            stopVisible ||
            hasInput ||
            qaVisible ||
            this.speech?.isListening;
        if (dtHidden) {
            dt.style.display = 'none';
        } else {
            dt.style.display = '';
            dt.style.opacity = '1';
        }
        // Position speech button: to the left of datetime when visible, or at right edge
        // Hide speech button when stop button is showing (generating response)
        if (this.elements.speechBtn) {
            if (stopVisible) {
                this.elements.speechBtn.style.display = 'none';
            } else {
                // Re-show if config says so (updateVisibility sets the base display)
                // Only restore if it was hidden by us, not by config
                if (this.elements.speechBtn.dataset.configVisible === 'true') {
                    this.elements.speechBtn.style.display = '';
                }
                if (this.elements.speechBtn.style.display !== 'none') {
                    if (!dtHidden && dt.style.display !== 'none') {
                        const dtWidth = dt.offsetWidth || 60;
                        this.elements.speechBtn.style.right = dtWidth + 18 + 'px';
                    } else {
                        this.elements.speechBtn.style.right = '10px';
                    }
                }
            }
        }
    },

    /**
     * Set (or clear) the native hover tooltip previewing the captured
     * selection. Uses the browser's built-in `title` so multi-line text
     * doesn't reflow the input layout — the OS renders the tooltip in its
     * own layer. The value is plain text, never HTML.
     */
    _setSelectionPreview(text) {
        const el = document.getElementById('selectionCheckboxLabel');
        if (!el) return;
        const trimmed = (text || '').trim();
        if (trimmed) {
            el.title = trimmed;
        } else {
            el.removeAttribute('title');
        }
    },

    setupEventListeners() {
        this.elements.input.addEventListener('input', (e) => this.handleInputChange(e));
        this.elements.input.addEventListener('keydown', (e) => this.handleKeyDown(e));
        this.elements.expandBtn.addEventListener('click', () => this.handleExpandClick());
        this.elements.floatingStopBtn.addEventListener('click', () => this.stopGenerating());
        document.addEventListener('click', (e) => this.handleOutsideClick(e));

        // Global keyboard shortcuts
        document.addEventListener('keydown', (e) => {
            // Escape — stop speech/TTS first, then stop generating, then hide
            if (e.key === 'Escape') {
                // Stop speech recognition or TTS first
                if (this.speech?.isActive || this.speech?.isListening) {
                    e.preventDefault();
                    this.speech.stopVoiceMode();
                    this.speech.cancelSpeech();
                    return;
                }
                if (this.isWaitingForResponse) {
                    e.preventDefault();
                    this.stopGenerating();
                    return;
                }
                if (this._justStoppedGenerating) {
                    e.preventDefault();
                    return;
                }
                this.appWindow.hide();
                return;
            }
            // Ctrl/⌘+, — open settings
            if (cmdOrCtrlPressed(e) && e.key === ',') {
                e.preventDefault();
                this.invoke('open_settings_window');
                return;
            }
            // Ctrl/⌘+E — expand to full chat
            if (cmdOrCtrlPressed(e) && e.key === 'e') {
                e.preventDefault();
                this.handleExpandClick();
                return;
            }
            // Ctrl/⌘+L — clear/reset
            if (cmdOrCtrlPressed(e) && e.key === 'l') {
                e.preventDefault();
                this.resetUI();
                this.windowManager.userSetHeight = null;
                this.windowManager.resizeWindow();
                return;
            }
            // Ctrl/⌘+Shift+C — copy last response
            if (cmdOrCtrlPressed(e) && e.shiftKey && e.key === 'C') {
                e.preventDefault();
                if (this.currentResponse) {
                    navigator.clipboard.writeText(this.currentResponse).catch(() => {});
                }
                return;
            }
            // Ctrl/⌘+W — hide window
            if (cmdOrCtrlPressed(e) && e.key === 'w') {
                e.preventDefault();
                this.appWindow.hide();
                return;
            }
        });

        // Paste handler for images
        this.elements.input.addEventListener('paste', (e) =>
            handlePasteEvent(e, this.attachmentManager)
        );

        // Re-render previews when attachments change and resize window
        this.attachmentManager.onChange((attachments) => {
            renderAttachmentPreviews(
                this.elements.attachmentPreviews,
                attachments,
                this.attachmentManager
            );
            this.windowManager.resizeWindow();
        });

        // Floating toolbar: attach file/image buttons
        this.elements.floatingAttachFileBtn?.addEventListener('click', () => {
            this._filePickerOpen = true;
            this.elements.floatingFileInput?.click();
        });
        this.elements.floatingAttachImageBtn?.addEventListener('click', () => {
            this._filePickerOpen = true;
            this.elements.floatingImageInput?.click();
        });
        this.elements.floatingFileInput?.addEventListener('change', (e) => {
            this._filePickerOpen = false;
            for (const file of e.target.files) {
                const path = file.path || file.name;
                this.attachmentManager.addFile(path, file.name, file.type || 'text/plain');
            }
            e.target.value = '';
            this.appWindow.show();
            this.appWindow.setFocus();
        });
        this.elements.floatingImageInput?.addEventListener('change', async (e) => {
            this._filePickerOpen = false;
            for (const file of e.target.files) {
                if (!file.type.startsWith('image/')) continue;
                try {
                    const base64 = await new Promise((resolve, reject) => {
                        const reader = new FileReader();
                        reader.onload = () => resolve(reader.result.split(',')[1]);
                        reader.onerror = reject;
                        reader.readAsDataURL(file);
                    });
                    this.attachmentManager.addImage(base64, file.type);
                } catch (err) {
                    console.error('Failed to read image:', file.name, err);
                }
            }
            e.target.value = '';
            this.appWindow.show();
            this.appWindow.setFocus();
        });
        // Handle file picker cancel (no change event fires)
        window.addEventListener('focus', () => {
            if (this._filePickerOpen) {
                this._filePickerOpen = false;
                this.appWindow.show();
                this.appWindow.setFocus();
            }
        });

        // Show/hide toolbar based on config
        this._updateToolbarVisibility();

        // Model selector in toolbar — opens model settings
        this.elements.floatingModelSelector?.addEventListener('click', () => {
            this.invoke('open_settings_window', { section: 'model' });
        });
    },

    setupStreamingListeners() {
        this.listen(EVT.MESSAGE_CHUNK, (event) => this.handleMessageChunk(event));
        this.listen(EVT.MESSAGE_COMPLETE, (event) => {
            // Broadcast event — every streaming-audience window hears
            // every session's completes. Only treat it as ours when the
            // active session (post any in-flight recovery) or the
            // pre-recovery session matches our pin; a payload with no
            // session id at all (automation-plan completion) also counts.
            // Without this filter, a turn finishing in a chat window
            // re-pinned floating to that window's session.
            const newId = event?.payload?.sessionId;
            const oldId = event?.payload?.oldSessionId;
            const ours =
                (!newId && !oldId) ||
                newId === this.floatingSessionId ||
                (oldId && oldId === this.floatingSessionId);
            if (!ours) return;

            // Recovery may have moved us to a fresh session; pick up
            // the new id so subsequent sends/cancels target it.
            if (newId && newId !== this.floatingSessionId) {
                console.log('[floating] adopting recovery session id:', newId);
                this.floatingSessionId = newId;
                this.invoke('set_window_session', {
                    label: WINDOW.FLOATING,
                    sessionId: newId,
                }).catch(() => {});
            }
            this.handleMessageComplete();
        });
        this.listen(EVT.MESSAGE_ERROR, (event) => this.handleMessageError(event));
        this.listen(EVT.TOOL_CALL_UPDATE, (event) => this.handleToolCallUpdate(event));
        this.listen('session_migrated', (event) => {
            // The backend died mid-turn and recovery swapped us to a fresh
            // session; the recovered response is about to stream under the
            // new id. Adopt it *without* tearing down the waiting UI (unlike
            // session_reset) and drop the accumulated steering-reply text so
            // the resend renders clean rather than after a stray greeting.
            const oldId = event?.payload?.oldSessionId;
            const newId = event?.payload?.newSessionId;
            const ours = oldId && oldId === this.floatingSessionId;
            if (!ours || !newId) return;
            console.log('[floating] session migrated mid-turn:', oldId, '→', newId);
            this.floatingSessionId = newId;
            this.currentResponse = '';
            this.invoke('set_window_session', {
                label: WINDOW.FLOATING,
                sessionId: newId,
            }).catch(() => {});
        });
        this.listen('session_reset', (event) => {
            // session_reset is broadcast to all windows; only adopt the
            // new id if our pinned session was the one that died.
            const oldId = event?.payload?.oldSessionId;
            const newId = event?.payload?.newSessionId;
            const ours = oldId && oldId === this.floatingSessionId;
            if (!ours) return;
            if (newId) {
                this.floatingSessionId = newId;
                this.invoke('set_window_session', {
                    label: WINDOW.FLOATING,
                    sessionId: newId,
                }).catch(() => {});
            }
            this.handleSessionReset(event);
        });
        this.toolSources = [];

        // Track compaction state — queue outgoing messages while compacting
        this.listen(EVT.COMPACTION_STATUS, (event) => {
            const status = event.payload?.params?.status?.type;
            if (status === 'started') {
                this._compacting = true;
                this._showCompactionIndicator();
            } else if (status === 'completed') {
                this._compacting = false;
                this._hideCompactionIndicator();
                // Ensure stop button is hidden after compaction — it may have been
                // left visible if handleMessageComplete was skipped during tool execution.
                this.elements.floatingStopBtn.style.display = 'none';
                this.updateDatetimeVisibility();
            }
        });

        // Listen for selection captured from previous window
        this.listen('selection_captured', async (event) => {
            const hasSelection = event.payload;
            const indicator = document.getElementById('selectionIndicator');
            const checkbox = document.getElementById('useSelectionCheckbox');
            const quickActionsContainer = document.getElementById('quickActionsContainer');
            if (hasSelection) {
                try {
                    const raw = await this.invoke('get_last_selection');
                    this.lastSelection = raw?.trim() || null;
                } catch {
                    this.lastSelection = null;
                }
                if (this.lastSelection) {
                    if (indicator) indicator.style.display = '';
                    if (checkbox) checkbox.checked = true;
                    this._setSelectionPreview(this.lastSelection);
                    // Hide datetime to make room for quick actions
                    this.updateDatetimeVisibility();

                    // Show quick action chips based on text content
                    if (quickActionsContainer) {
                        try {
                            const config = await getConfig(this.invoke);
                            const qaConfig = config.quick_actions || {
                                enabled: true,
                                custom_actions: [],
                            };
                            const actions = await getActionsForText(this.lastSelection, qaConfig);
                            renderQuickActionChips(
                                actions,
                                quickActionsContainer,
                                (promptTemplate) => {
                                    const prompt = promptTemplate.replace(
                                        /\{text\}/g,
                                        this.lastSelection
                                    );
                                    this.sendChatMessage(prompt, { skipSelection: true });
                                }
                            );
                        } catch (e) {
                            console.error('Quick actions error:', e);
                            quickActionsContainer.style.display = 'none';
                        }
                    }

                    this.windowManager.resizeWindow();
                    return;
                }
            }
            this.lastSelection = null;
            if (indicator) indicator.style.display = 'none';
            this._setSelectionPreview(null);
            if (quickActionsContainer) quickActionsContainer.style.display = 'none';
            // Restore datetime and resize back to normal
            this.updateDatetimeVisibility();
            this.windowManager.resizeWindow();
        });

        document.addEventListener('kage-clear', () => {
            this.resetUI();
            this.windowManager.userSetHeight = null;
            this.windowManager.resizeWindow();
        });

        document.addEventListener('kage-resize-request', () => {
            this.windowManager.resizeWindow();
        });

        document.addEventListener('kage-show-response', (e) => {
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
            this.elements.appSuggestions.classList.remove('visible');
            this.currentMatches = [];
            this.selectedIndex = -1;
            this.currentResponse = e.detail;
            renderMarkdown(e.detail, this.elements.responseText);
            this.elements.contentArea.classList.add('visible');
            this.windowManager.resizeWindow();
        });

        // Prompt-dispatch slash commands (standard ACP, e.g. Claude): send the
        // slash text as a normal message so the agent interprets it and streams
        // the answer back through the usual pipeline.
        document.addEventListener('kage-send-prompt', (e) => {
            const text = e.detail?.text;
            if (!text) return;
            this.clearSuggestions();
            this.sendChatMessage(text, { forceChat: true });
        });

        document.addEventListener('kage-show-selection', (e) => {
            const { command, options } = e.detail;
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
            this.elements.contentArea.classList.remove('visible');

            // Show options as selectable items in the suggestions dropdown
            this.currentMatches = options.map((opt) => ({
                type: 'selection',
                name: opt.label,
                value: opt.value,
                current: opt.current,
                command: command,
            }));
            this.selectedIndex = options.findIndex((o) => o.current);
            if (this.selectedIndex < 0) this.selectedIndex = 0;

            const container = this.elements.appSuggestions;
            container.innerHTML = '';
            container.scrollTop = 0;

            options.forEach((opt, index) => {
                const item = document.createElement('div');
                item.className =
                    'app-suggestion-item' + (index === this.selectedIndex ? ' selected' : '');
                // Prefer the human description (e.g. an agent's blurb); fall
                // back to the raw value when the agent gave no description.
                // label/description/value come from the agent's commands/execute
                // response — untrusted content in a privileged webview, so build
                // the structure statically and set the text via textContent
                // (mirrors the chat window's slash-selection renderer).
                const subtitle = opt.description || opt.value || '';
                item.innerHTML = `
                <div class="app-icon">${opt.current ? '✓' : '○'}</div>
                <div class="app-info">
                    <div class="app-name"><span class="app-name-label"></span>${opt.current ? '<span class="selection-current">●</span>' : ''}</div>
                    <div class="app-description"></div>
                </div>
            `;
                item.querySelector('.app-name-label').textContent = opt.label || '';
                item.querySelector('.app-description').textContent = subtitle;
                item.addEventListener('click', () => this.executeSelection(command, opt.value));
                container.appendChild(item);
            });

            container.classList.add('visible');
            // Defer scroll-to-selected until after layout is complete
            this.windowManager.resizeWindow();
            setTimeout(() => updateSelection(container, this.selectedIndex), 20);
        });
    },
};
