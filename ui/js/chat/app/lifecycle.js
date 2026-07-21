export function createLifecycleMixin(dependencies) {
    const {
        initMarkdown,
        setAppIconInvoke,
        handlePasteEvent,
        setupDragDrop,
        renderAttachmentPreviews,
        loadSlashCommands,
        stripKageTags,
        t,
        ExtensionManager,
        unifiedSearch,
        loadFrecency,
        setExtensionManager,
        cmdOrCtrlPressed,
        setupRtlDetection,
    } = dependencies;
    return class {
        async init() {
            initMarkdown();
            setAppIconInvoke(this.invoke);
            this.cacheElements();
            this.setupSpeech();
            this.setupEventListeners();
            this.setupStreamingListeners();
            // Load user info early — before any of the awaits below — so that
            // historical messages rendered by displaySession (triggered either
            // from this init or from the tauri://focus listener in main.js)
            // see populated `this.userInfo` and produce the avatar instead of
            // a `?` fallback. Symptom pre-fix: chat opened, focus listener
            // raced ahead of loadUserInfo, displaySession rendered all user
            // messages with `?` in the avatar slot, and there was no
            // re-render once userInfo arrived a few ms later.
            await this.loadUserInfo();
            await this.loadFloatingSessionId();
            await this.loadCurrentSessionId();
            // Peer chat windows (`chat-<uuid>`) have no pinned session
            // until they bootstrap one — load from URL or create fresh.
            await this._bootstrapChatPeerSession();
            await this.loadActionButtonConfig();
            await loadSlashCommands(this.invoke);
            await this.loadShortcuts();

            // Initialize extension manager for search
            this.extensionManager = new ExtensionManager(this.invoke);
            await this.extensionManager.initialize();
            setExtensionManager(this.extensionManager);
            await loadFrecency(this.invoke);

            // Send extension tool definitions to the agent as steering
            this.extensionToolController.sendSteering();

            // Load sessions in background — don't block init
            this.loadSessions();

            await this.checkConnection();
            this.setupNetworkMonitor();

            // Load toolbar data in background
            this.loadModels();
            this.refreshContextUsage();

            console.log('[CHAT] Init - currentAcpSessionId:', this.currentAcpSessionId);
            console.log('[CHAT] Init - floatingSessionId:', this.floatingSessionId);
            console.log('[CHAT] Init - sessions count:', this.sessions.length);
            console.log(
                '[CHAT] Init - session IDs:',
                this.sessions.map((s) => s.session_id)
            );

            // Auto-select the current ACP session if one exists
            if (this.currentAcpSessionId) {
                // Ensure sessions are loaded before trying to find the current one
                if (this.sessions.length === 0) {
                    await this.loadSessions();
                }
                const exists = this.sessions.find((s) => s.session_id === this.currentAcpSessionId);
                if (!exists) {
                    // Session not on disk yet — add a synthetic entry so it appears in the list
                    console.log(
                        '[CHAT] Current session not on disk, adding synthetic entry:',
                        this.currentAcpSessionId
                    );
                    const synthetic = {
                        session_id: this.currentAcpSessionId,
                        title: t('chat.session.current_title'),
                        created_at: new Date().toISOString(),
                        updated_at: new Date().toISOString(),
                    };
                    this.sessions.unshift(synthetic);
                    this.renderSessionList();
                }
                // Select it — load from disk if available, otherwise just mark it active
                this.activeSessionId = this.currentAcpSessionId;
                this.renderSessionList();
                try {
                    const sessionData = await this.invoke('load_session', {
                        sessionId: this.currentAcpSessionId,
                    });
                    this.displaySession(sessionData);
                } catch (e) {
                    console.log('[CHAT] Could not load session from disk (may be new):', e);
                    // Session is new / not on disk — just show empty chat
                    this.elements.messagesArea.innerHTML = `<div class="message-placeholder">${t('chat.placeholder.continue')}</div>`;
                }
                this.elements.chatHeaderTitle.textContent =
                    stripKageTags(exists?.title) || t('chat.session.current_title');
            }

            this.elements.chatInput.focus();

            // RTL detection — flip input and message layout when first char is RTL
            const chatInputWrapper = this.elements.chatInput?.closest('.chat-input-wrapper');
            setupRtlDetection(
                this.elements.chatInput,
                chatInputWrapper,
                this.elements.messagesArea
            );

            console.log('Chat app initialized');
        }

        cacheElements() {
            this.elements = {
                chatInput: document.getElementById('chatInput'),
                sendBtn: document.getElementById('sendBtn'),
                messagesArea: document.getElementById('messagesArea'),
                sessionList: document.getElementById('sessionList'),
                sessionSearch: document.getElementById('sessionSearch'),
                newSessionBtn: document.getElementById('newSessionBtn'),
                newWindowBtn: document.getElementById('newWindowBtn'),
                settingsBtn: document.getElementById('settingsBtn'),
                connectionStatus: document.getElementById('connectionStatus'),
                chatHeaderTitle: document.getElementById('chatHeaderTitle'),
                chatHeaderTitleInput: document.getElementById('chatHeaderTitleInput'),
                chatExportBtn: document.getElementById('chatExportBtn'),
                errorContainer: document.getElementById('errorContainer'),
                chatSuggestions: document.getElementById('chatSuggestions'),
                attachmentPreviews: document.getElementById('attachmentPreviews'),
                chatMain: document.querySelector('.chat-main'),
                attachFileBtn: document.getElementById('attachFileBtn'),
                attachImageBtn: document.getElementById('attachImageBtn'),
                fileInput: document.getElementById('fileInput'),
                imageInput: document.getElementById('imageInput'),
                contextPercent: document.getElementById('contextPercent'),
                modelSelector: document.getElementById('modelSelector'),
                modelName: document.getElementById('modelName'),
                modelDropdown: document.getElementById('modelDropdown'),
                chatSpeechBtn: document.getElementById('chatSpeechBtn'),
                chatSpeechWave: document.getElementById('chatSpeechWave'),
            };
        }

        setupEventListeners() {
            this.elements.chatInput.addEventListener('input', () => {
                this.elements.chatInput.style.height = 'auto';
                this.elements.chatInput.style.height =
                    Math.min(this.elements.chatInput.scrollHeight, 120) + 'px';
                this._tabCycleActive = false;
                // Debounce: every keystroke would otherwise trigger a fresh
                // unifiedSearch (3+ IPC roundtrips: shortcuts, frecency, file
                // search). Coalesce into one query 100ms after the user
                // stops typing — matches the floating window's debounce
                // policy. Empty input clears immediately so the suggestions
                // panel doesn't linger after the field is cleared.
                if (this._suggestionsDebounce) {
                    clearTimeout(this._suggestionsDebounce);
                    this._suggestionsDebounce = null;
                }
                if (!this.elements.chatInput.value.trim()) {
                    this.updateSuggestions(); // sync clear
                } else {
                    this._suggestionsDebounce = setTimeout(() => {
                        this._suggestionsDebounce = null;
                        this.updateSuggestions();
                    }, 100);
                }
            });

            this.elements.chatInput.addEventListener('keydown', async (e) => {
                if (e.key === 'Tab') {
                    e.preventDefault();
                    if (this.currentSuggestions.length > 0) {
                        // Cycle through suggestions on repeated Tab presses
                        if (this._tabCycleActive) {
                            this._tabCycleIndex =
                                (this._tabCycleIndex + 1) % this.currentSuggestions.length;
                        } else {
                            this._tabCycleIndex = 0;
                            this._tabCycleActive = true;
                        }
                        const pick = this.currentSuggestions[this._tabCycleIndex];
                        if (pick.type === 'command') {
                            this.elements.chatInput.value = '>' + pick.name + ' ';
                        } else if (pick.type === 'slash') {
                            this.elements.chatInput.value = pick.name + ' ';
                        }
                        this.suggestionIndex = this._tabCycleIndex;
                        this.renderSuggestions();
                    }
                } else if (e.key === 'ArrowDown' && this.currentSuggestions.length > 0) {
                    e.preventDefault();
                    this.suggestionIndex =
                        (this.suggestionIndex + 1) % this.currentSuggestions.length;
                    this.renderSuggestions();
                } else if (e.key === 'ArrowUp' && this.currentSuggestions.length > 0) {
                    e.preventDefault();
                    this.suggestionIndex =
                        this.suggestionIndex <= 0
                            ? this.currentSuggestions.length - 1
                            : this.suggestionIndex - 1;
                    this.renderSuggestions();
                } else if (e.key === 'Escape' && this.currentSuggestions.length > 0) {
                    e.preventDefault();
                    this.clearSuggestions();
                } else if (e.key === 'Enter' && !e.shiftKey) {
                    e.preventDefault();
                    await this.handleEnterKey();
                }
            });

            this.elements.sendBtn.addEventListener('click', () => {
                if (this.isWaitingForResponse) {
                    this.stopGenerating();
                } else {
                    this.sendMessage();
                }
            });
            this.elements.newSessionBtn.addEventListener('click', () => this.createNewSession());
            if (this.elements.newWindowBtn) {
                this.elements.newWindowBtn.addEventListener('click', () =>
                    this.openNewChatWindow()
                );
            }
            // Ctrl/Cmd+Shift+N — spawn a new chat window pinned to a fresh session
            document.addEventListener('keydown', (e) => {
                if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === 'n') {
                    e.preventDefault();
                    this.openNewChatWindow();
                }
            });

            // Session search — load all sessions when user starts searching
            this.elements.sessionSearch.addEventListener('input', () => {
                const query = (this.elements.sessionSearch?.value || '').trim();
                if (query && !this._sessionsFullyLoaded) {
                    this.loadSessions(true);
                } else {
                    this.renderSessionList();
                }
            });

            // Lazy-load more sessions on scroll
            this.elements.sessionList?.addEventListener('scroll', () => {
                const el = this.elements.sessionList;
                if (el.scrollTop + el.clientHeight >= el.scrollHeight - 100) {
                    this.loadMoreSessions();
                }
            });

            // Reload slash commands when input is focused (may not have been available at init)
            this.elements.chatInput.addEventListener('focus', () => {
                loadSlashCommands(this.invoke);
            });

            this.elements.settingsBtn.addEventListener('click', async () => {
                await this.invoke('open_settings_window');
            });

            // Paste handler for images
            this.elements.chatInput.addEventListener('paste', (e) =>
                handlePasteEvent(e, this.attachmentManager)
            );

            // Export the current chat to a Markdown file the user picks.
            this.elements.chatExportBtn?.addEventListener('click', () =>
                this.exportChatAsMarkdown()
            );

            // Double-click header title to rename session
            this.elements.chatHeaderTitle.addEventListener('dblclick', () => this.startTitleEdit());
            this.elements.chatHeaderTitleInput.addEventListener('blur', () =>
                this.finishTitleEdit()
            );
            this.elements.chatHeaderTitleInput.addEventListener('keydown', (e) => {
                if (e.key === 'Enter') {
                    e.preventDefault();
                    this.finishTitleEdit();
                }
                if (e.key === 'Escape') {
                    this.cancelTitleEdit();
                }
            });

            // Drag-and-drop for files on the main chat area
            setupDragDrop(this.elements.chatMain, this.elements.chatMain, this.attachmentManager);

            // Re-render previews when attachments change
            this.attachmentManager.onChange((attachments) => {
                renderAttachmentPreviews(
                    this.elements.attachmentPreviews,
                    attachments,
                    this.attachmentManager
                );
            });

            // Toolbar: attach file
            this.elements.attachFileBtn.addEventListener('click', () =>
                this.elements.fileInput.click()
            );
            this.elements.fileInput.addEventListener('change', (e) => this.handleFileAttach(e));

            // Toolbar: attach image
            this.elements.attachImageBtn.addEventListener('click', () =>
                this.elements.imageInput.click()
            );
            this.elements.imageInput.addEventListener('change', (e) => this.handleImageAttach(e));

            // Toolbar: model selector
            this.elements.modelSelector.addEventListener('click', () => this.toggleModelDropdown());
            document.addEventListener('click', (e) => {
                if (
                    !this.elements.modelSelector.contains(e.target) &&
                    !this.elements.modelDropdown.contains(e.target)
                ) {
                    this.elements.modelDropdown.style.display = 'none';
                }
            });

            // Image lightbox — click any message image to zoom
            const lightbox = document.getElementById('imageLightbox');
            const lightboxImg = document.getElementById('lightboxImg');

            this.elements.messagesArea.addEventListener('click', (e) => {
                if (e.target.classList.contains('message-attachment-img')) {
                    lightboxImg.src = e.target.src;
                    lightbox.style.display = 'flex';
                }
            });

            lightbox.addEventListener('click', () => {
                lightbox.style.display = 'none';
                lightboxImg.src = '';
            });

            document.addEventListener('keydown', (e) => {
                // Escape — stop speech/TTS, then stop generating, or close lightbox
                if (e.key === 'Escape') {
                    if (lightbox.style.display !== 'none') {
                        lightbox.style.display = 'none';
                        lightboxImg.src = '';
                        return;
                    }
                    if (this.speech?.isActive) {
                        e.preventDefault();
                        this.speech.stop();
                        this.speech.cancelSpeech();
                        return;
                    }
                    if (this.isWaitingForResponse) {
                        e.preventDefault();
                        this.stopGenerating();
                        return;
                    }
                }
                // Ctrl/⌘+N — new session
                if (cmdOrCtrlPressed(e) && e.key === 'n') {
                    e.preventDefault();
                    this.createNewSession();
                    return;
                }
                // Ctrl/⌘+, — open settings
                if (cmdOrCtrlPressed(e) && e.key === ',') {
                    e.preventDefault();
                    this.invoke('open_settings_window');
                    return;
                }
                // Ctrl/⌘+W — hide window
                if (cmdOrCtrlPressed(e) && e.key === 'w') {
                    e.preventDefault();
                    this.appWindow.close();
                    return;
                }
                // Ctrl/⌘+Shift+C — copy last response
                if (cmdOrCtrlPressed(e) && e.shiftKey && e.key === 'C') {
                    e.preventDefault();
                    if (this.currentStreamingContent) {
                        navigator.clipboard.writeText(this.currentStreamingContent).catch(() => {});
                    }
                    return;
                }
            });
        }
    };
}
