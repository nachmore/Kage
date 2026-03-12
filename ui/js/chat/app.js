// Expanded chat application logic
import { renderMarkdown, initMarkdown, createTaskPlanElement, setAppIconInvoke } from '../shared/markdown.js';
import { AttachmentManager, handlePasteEvent, setupDragDrop, renderAttachmentPreviews, attachmentPreviewHtml, sessionImageToDataUrl } from '../shared/attachments.js';
import { matchCommands, matchSlashCommands, loadSlashCommands, executeCommand } from '../shared/commands.js';
import { escapeHtml } from '../shared/tool-utils.js';
import { processToolCallUpdate, renderToolChipsHtml, renderSourceChipsHtml, getSessionResetMessage, detectAutomationPlan, detectAutomationPlanIncremental, automationPlanToTasks, detectExtensionToolCall, detectExtensionToolCallIncremental, extractSuggestedActions } from '../shared/streaming-utils.js';
import { sendAppNotification } from '../shared/notify.js';
import { SpeechController } from '../shared/speech.js';
import { ExtensionManager } from '../shared/extension-manager.js';
import { unifiedSearch, loadFrecency, setExtensionManager, recordSelection, getExtensionManager } from '../shared/search-engine.js';
import { matchShortcut, buildShortcutCommand } from '../shared/shortcuts.js';
import { executeResult as executeResultShared, executeShortcutCommand, handleEnterAction } from '../shared/result-executor.js';
import { getActionsForText, renderQuickActionChips } from '../shared/quick-actions.js';
import { setupRtlDetection } from '../shared/rtl.js';

/** Prefix used to identify steering messages that should be hidden in the UI */
const STEERING_MSG_PREFIX = '[KIRO_STEERING_IGNORE]';

/** Map MCP/agent tool names to user-friendly descriptions for the spinner. */
function _chatToolFriendlyName(title) {
    const map = {
        list_windows: 'Checking visible windows', list_all_windows: 'Enumerating all open windows',
        get_ui_tree: 'Reading application UI', find_elements: 'Searching for UI elements',
        get_element_text: 'Reading text content', launch_app: 'Launching application',
        launch_and_get_tree: 'Launching application', click_and_get_tree: 'Clicking and reading UI',
        click_and_read_result: 'Clicking and reading result', type_and_get_tree: 'Typing text',
        screenshot: 'Taking screenshot', read: 'Reading file', write: 'Writing file',
        execute: 'Running command', search: 'Searching',
    };
    if (map[title]) return map[title];
    for (const [key, friendly] of Object.entries(map)) {
        if (title.includes(key)) return friendly;
    }
    return title.replace(/^Running:\s*@\S+\s*/, '').replace(/_/g, ' ') || 'Working on it';
}

/** Build metadata object for a message from session turn data. */
function _buildMsgMeta(messageId, timestamps, durations, role) {
    if (!messageId) return null;
    const endTs = timestamps[messageId];
    if (!endTs) return null;
    const dur = durations[messageId] || 0;
    if (role === 'user' && dur > 0) {
        const endDate = new Date(endTs);
        return { timestamp: new Date(endDate.getTime() - dur * 1000).toISOString() };
    }
    return { timestamp: endTs, durationSecs: role === 'assistant' ? dur : null };
}

export class ChatApp {
    constructor(invoke, appWindow, listen) {
        this.invoke = invoke;
        this.appWindow = appWindow;
        this.listen = listen;

        this.messages = [];
        this.currentStreamingMessage = null;
        this.currentStreamingContent = '';
        this.isWaitingForResponse = false;
        this.isConnected = false;
        this.sessions = [];
        this._sessionsFullyLoaded = false;
        this._loadingMore = false;
        this.activeSessionId = null;
        this.floatingSessionId = null;
        this.currentAcpSessionId = null;
        this.toolSources = [];
        this.toolUsages = [];
        this._toolCallIds = new Set();
        this.userInfo = null;
        this.attachmentManager = new AttachmentManager();
        this.currentSuggestions = [];
        this.suggestionIndex = -1;
        this.availableModels = [];
        this.currentModelId = null;
        this._showSpeakBtn = false;
        this._showTranslateBtn = false;

        this.elements = {};
    }

    async init() {
        initMarkdown();
        setAppIconInvoke(this.invoke);
        this.cacheElements();
        this.setupSpeech();
        this.setupEventListeners();
        this.setupStreamingListeners();
        await this.loadFloatingSessionId();
        await this.loadCurrentSessionId();
        await this.loadUserInfo();
        await this.loadActionButtonConfig();
        await loadSlashCommands(this.invoke);
        await this.loadShortcuts();

        // Initialize extension manager for search
        this.extensionManager = new ExtensionManager(this.invoke);
        await this.extensionManager.initialize();
        setExtensionManager(this.extensionManager);
        await loadFrecency(this.invoke);

        // Send extension tool definitions to the agent as steering
        this._sendExtensionToolSteering();

        // Load sessions in background — don't block init
        this.loadSessions();

        await this.checkConnection();

        // Load toolbar data in background
        this.loadModels();
        this.refreshContextUsage();

        console.log('[CHAT] Init - currentAcpSessionId:', this.currentAcpSessionId);
        console.log('[CHAT] Init - floatingSessionId:', this.floatingSessionId);
        console.log('[CHAT] Init - sessions count:', this.sessions.length);
        console.log('[CHAT] Init - session IDs:', this.sessions.map(s => s.session_id));

        // Auto-select the current ACP session if one exists
        if (this.currentAcpSessionId) {
            // Ensure sessions are loaded before trying to find the current one
            if (this.sessions.length === 0) {
                await this.loadSessions();
            }
            let exists = this.sessions.find(s => s.session_id === this.currentAcpSessionId);
            if (!exists) {
                // Session not on disk yet — add a synthetic entry so it appears in the list
                console.log('[CHAT] Current session not on disk, adding synthetic entry:', this.currentAcpSessionId);
                const synthetic = {
                    session_id: this.currentAcpSessionId,
                    title: 'Current Session',
                    created_at: new Date().toISOString(),
                    updated_at: new Date().toISOString()
                };
                this.sessions.unshift(synthetic);
                this.renderSessionList();
            }
            // Select it — load from disk if available, otherwise just mark it active
            this.activeSessionId = this.currentAcpSessionId;
            this.renderSessionList();
            try {
                const sessionData = await this.invoke('load_session', { sessionId: this.currentAcpSessionId });
                this.displaySession(sessionData);
            } catch (e) {
                console.log('[CHAT] Could not load session from disk (may be new):', e);
                // Session is new / not on disk — just show empty chat
                this.elements.messagesArea.innerHTML = '<div class="message-placeholder">Continue your conversation...</div>';
            }
            this.elements.chatHeaderTitle.textContent = exists?.title || 'Current Session';
        }

        this.elements.chatInput.focus();

        // RTL detection — flip input and message layout when first char is RTL
        const chatInputWrapper = this.elements.chatInput?.closest('.chat-input-wrapper');
        setupRtlDetection(this.elements.chatInput, chatInputWrapper, this.elements.messagesArea);

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
            settingsBtn: document.getElementById('settingsBtn'),
            connectionStatus: document.getElementById('connectionStatus'),
            chatHeaderTitle: document.getElementById('chatHeaderTitle'),
            chatHeaderTitleInput: document.getElementById('chatHeaderTitleInput'),
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
            chatSpeechWave: document.getElementById('chatSpeechWave')
        };
    }

    setupEventListeners() {
        this.elements.chatInput.addEventListener('input', () => {
            this.elements.chatInput.style.height = 'auto';
            this.elements.chatInput.style.height = Math.min(this.elements.chatInput.scrollHeight, 120) + 'px';
            this._tabCycleActive = false;
            this.updateSuggestions();
        });

        this.elements.chatInput.addEventListener('keydown', async (e) => {
            if (e.key === 'Tab') {
                e.preventDefault();
                if (this.currentSuggestions.length > 0) {
                    // Cycle through suggestions on repeated Tab presses
                    if (this._tabCycleActive) {
                        this._tabCycleIndex = (this._tabCycleIndex + 1) % this.currentSuggestions.length;
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
                this.suggestionIndex = (this.suggestionIndex + 1) % this.currentSuggestions.length;
                this.renderSuggestions();
            } else if (e.key === 'ArrowUp' && this.currentSuggestions.length > 0) {
                e.preventDefault();
                this.suggestionIndex = this.suggestionIndex <= 0 ? this.currentSuggestions.length - 1 : this.suggestionIndex - 1;
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
        this.elements.chatInput.addEventListener('paste', (e) => handlePasteEvent(e, this.attachmentManager));

        // Double-click header title to rename session
        this.elements.chatHeaderTitle.addEventListener('dblclick', () => this.startTitleEdit());
        this.elements.chatHeaderTitleInput.addEventListener('blur', () => this.finishTitleEdit());
        this.elements.chatHeaderTitleInput.addEventListener('keydown', (e) => {
            if (e.key === 'Enter') { e.preventDefault(); this.finishTitleEdit(); }
            if (e.key === 'Escape') { this.cancelTitleEdit(); }
        });

        // Drag-and-drop for files on the main chat area
        setupDragDrop(this.elements.chatMain, this.elements.chatMain, this.attachmentManager);

        // Re-render previews when attachments change
        this.attachmentManager.onChange((attachments) => {
            renderAttachmentPreviews(this.elements.attachmentPreviews, attachments, this.attachmentManager);
        });

        // Toolbar: attach file
        this.elements.attachFileBtn.addEventListener('click', () => this.elements.fileInput.click());
        this.elements.fileInput.addEventListener('change', (e) => this.handleFileAttach(e));

        // Toolbar: attach image
        this.elements.attachImageBtn.addEventListener('click', () => this.elements.imageInput.click());
        this.elements.imageInput.addEventListener('change', (e) => this.handleImageAttach(e));

        // Toolbar: model selector
        this.elements.modelSelector.addEventListener('click', () => this.toggleModelDropdown());
        document.addEventListener('click', (e) => {
            if (!this.elements.modelSelector.contains(e.target) && !this.elements.modelDropdown.contains(e.target)) {
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
            // Ctrl+N — new session
            if (e.ctrlKey && e.key === 'n') {
                e.preventDefault();
                this.createNewSession();
                return;
            }
            // Ctrl+, — open settings
            if (e.ctrlKey && e.key === ',') {
                e.preventDefault();
                this.invoke('open_settings_window');
                return;
            }
            // Ctrl+W — hide window
            if (e.ctrlKey && e.key === 'w') {
                e.preventDefault();
                this.appWindow.hide();
                return;
            }
            // Ctrl+Shift+C — copy last response
            if (e.ctrlKey && e.shiftKey && e.key === 'C') {
                e.preventDefault();
                if (this.currentStreamingContent) {
                    navigator.clipboard.writeText(this.currentStreamingContent).catch(() => {});
                }
                return;
            }
        });
    }

    setupStreamingListeners() {
        this.listen('message_chunk', (event) => this.handleMessageChunk(event));
        this.listen('message_complete', () => this.handleMessageComplete());
        this.listen('message_error', (event) => this.handleMessageError(event));
        this.listen('tool_call_update', (event) => this.handleToolCallUpdate(event));
        this.listen('session_reset', (event) => this.handleSessionReset(event));

        // Real-time context usage from ACP metadata notifications
        this.listen('context_metadata', (event) => {
            const pct = event.payload?.params?.contextUsagePercentage;
            if (pct != null) {
                const rounded = Math.round(pct);
                this.elements.contextPercent.textContent = rounded + '%';
                document.getElementById('contextIndicator').title = rounded + '% context used';
                this.drawContextRing(rounded);
                this.maybeAutoCompact(rounded);
            }
        });

        // Compaction status from ACP notifications (works for both auto and manual /compact)
        this.listen('compaction_status', (event) => {
            const status = event.payload?.params?.status?.type;
            if (status === 'started') {
                this.showCompactingNotice();
            } else if (status === 'completed') {
                this.hideCompactingNotice('Context compacted successfully');
            }
        });

        this.listen('initial_message', (event) => {
            const message = event.payload;
            if (message) {
                this.addUserMessage(message);
                this.startStreaming();
            }
        });

        // Handle slash command results (dispatched by floating-commands.js execute functions)
        document.addEventListener('kiro-show-response', (e) => {
            if (e.detail) {
                this.addMessageFromHistory('assistant', e.detail);
                this.scrollToBottom();
            }
        });

        document.addEventListener('kiro-show-selection', (e) => {
            const { command, options } = e.detail;
            if (!options || options.length === 0) return;
            // Show selection as a system message with clickable options
            const placeholder = this.elements.messagesArea.querySelector('.message-placeholder');
            if (placeholder) placeholder.remove();

            const container = document.createElement('div');
            container.className = 'session-reset-notice';
            options.forEach(opt => {
                const btn = document.createElement('button');
                btn.className = 'chat-error-btn reconnect';
                btn.style.margin = '4px';
                btn.textContent = opt.label + (opt.current ? ' ●' : '');
                btn.addEventListener('click', async () => {
                    try {
                        const result = await this.invoke('execute_slash_command', {
                            command,
                            args: { input: opt.value }
                        });
                        container.remove();
                        const msg = result?.message || 'Done';
                        this.addMessageFromHistory('assistant', msg);
                        this.scrollToBottom();
                    } catch (err) {
                        this.showError('Command failed: ' + err);
                    }
                });
                container.appendChild(btn);
            });
            this.elements.messagesArea.appendChild(container);
            this.scrollToBottom();
        });
    }

    // --- Session Management ---

    async loadFloatingSessionId() {
        try {
            this.floatingSessionId = await this.invoke('get_floating_session_id');
        } catch (e) {
            console.error('Failed to get floating session ID:', e);
            this.floatingSessionId = null;
        }
    }

    async loadCurrentSessionId() {
        try {
            this.currentAcpSessionId = await this.invoke('get_current_session_id');
        } catch (e) {
            console.error('Failed to get current session ID:', e);
            this.currentAcpSessionId = null;
        }
    }

    async loadUserInfo() {
        try {
            this.userInfo = await this.invoke('get_user_info');
        } catch (e) {
            console.error('[USER] Failed to get user info:', e);
            this.userInfo = null;
        }
    }

    async loadActionButtonConfig() {
        try {
            const config = await this.invoke('get_config');
            this._showSpeakBtn = config.ui?.show_speech_button === true || config.pocket_tts?.enabled === true;
            this._showTranslateBtn = !!(config.quick_actions?.translate_language);
            this._translateLang = config.quick_actions?.translate_language || 'English';
        } catch (e) {
            console.warn('[CHAT] Failed to load action button config:', e);
        }
    }

    // ── Suggestion Chips ──

    async showSuggestionChips() {
        this.hideSuggestionChips();
        const area = this.elements.messagesArea;
        if (!area || this.messages.length === 0) return;

        // Get the last assistant message content for context-aware actions
        const lastMsg = [...this.messages].reverse().find(m => m.role === 'assistant');
        const responseText = lastMsg?.content || '';

        try {
            const config = await this.invoke('get_config');
            if (!config.ui?.show_response_actions) return;
            const qaConfig = config.quick_actions || { enabled: true, custom_actions: [] };
            const actions = getActionsForText(responseText || 'general text', qaConfig);
            if (actions.length === 0) return;

            const chips = document.createElement('div');
            chips.id = 'chatSuggestionChips';
            chips.className = 'chat-suggestion-chips';

            for (const action of actions) {
                const chip = document.createElement('button');
                chip.className = 'chat-chip';
                chip.textContent = `${action.icon || '⚡'} ${action.label}`;
                chip.onclick = () => {
                    const prompt = action.prompt.replace(/\{text\}/g, responseText);
                    this.elements.chatInput.value = prompt;
                    this.sendMessage();
                };
                chips.appendChild(chip);
            }

            area.appendChild(chips);
            this.scrollToBottom();
        } catch (e) { console.warn('Suggestion chips error:', e); }
    }

    hideSuggestionChips() {
        const existing = document.getElementById('chatSuggestionChips');
        if (existing) existing.remove();
    }

    async loadSessions(loadAll = false) {
        try {
            const params = loadAll ? { force: true } : { limit: 50, offset: 0 };
            const sessions = await this.invoke('list_sessions', params);
            if (loadAll || !this._sessionsFullyLoaded) {
                this.sessions = sessions;
                this._sessionsFullyLoaded = loadAll || sessions.length < 50;
            }
            this.renderSessionList();
        } catch (error) {
            console.error('Failed to load sessions:', error);
            this.sessions = [];
            this.renderSessionList();
        }
    }

    async loadMoreSessions() {
        if (this._sessionsFullyLoaded || this._loadingMore) return;
        this._loadingMore = true;
        try {
            const more = await this.invoke('list_sessions', { limit: 50, offset: this.sessions.length });
            if (more.length > 0) {
                this.sessions = this.sessions.concat(more);
                this.renderSessionList();
            }
            if (more.length < 50) this._sessionsFullyLoaded = true;
        } catch (e) {
            console.error('Failed to load more sessions:', e);
        } finally {
            this._loadingMore = false;
        }
    }

    renderSessionList() {
            const list = this.elements.sessionList;
            const searchQuery = (this.elements.sessionSearch?.value || '').toLowerCase().trim();

            if (this.sessions.length === 0) {
                list.innerHTML = '<div class="session-list-empty">No sessions yet</div>';
                return;
            }

            // Sort: default session first, then by updated_at descending
            const defaultId = this.currentAcpSessionId || this.floatingSessionId;
            const sorted = [...this.sessions].sort((a, b) => {
                const aIsDefault = a.session_id === defaultId;
                const bIsDefault = b.session_id === defaultId;
                if (aIsDefault && !bIsDefault) return -1;
                if (!aIsDefault && bIsDefault) return 1;
                return (b.updated_at || '').localeCompare(a.updated_at || '');
            });

            // Filter by search query
            const filtered = searchQuery
                ? sorted.filter(s => (s.title || 'New Chat').toLowerCase().includes(searchQuery))
                : sorted;

            if (filtered.length === 0) {
                list.innerHTML = '<div class="session-list-empty">No matching sessions</div>';
                return;
            }

            // Build map of existing DOM items by session_id for diffing
            const existingById = new Map();
            list.querySelectorAll('.session-item[data-session-id]').forEach(el => {
                existingById.set(el.dataset.sessionId, el);
            });

            // Build the desired ordered list of session_ids + separator positions
            const desiredIds = [];
            for (const session of filtered) {
                desiredIds.push(session.session_id);
                const isDefault = session.session_id === this.currentAcpSessionId || session.session_id === this.floatingSessionId;
                if (isDefault && !searchQuery) {
                    desiredIds.push('__separator__');
                }
            }

            // Remove items no longer in the filtered list
            const desiredSet = new Set(filtered.map(s => s.session_id));
            for (const [id, el] of existingById) {
                if (!desiredSet.has(id)) el.remove();
            }
            // Remove stale empty-state messages and separators (will re-add separator if needed)
            list.querySelectorAll('.session-list-empty, .session-list-separator').forEach(el => el.remove());

            // Create or update each item, then ensure correct DOM order
            let insertionIndex = 0;
            for (const key of desiredIds) {
                if (key === '__separator__') {
                    // Insert separator if not already at this position
                    const current = list.children[insertionIndex];
                    if (!current || !current.classList.contains('session-list-separator')) {
                        const sep = document.createElement('div');
                        sep.className = 'session-list-separator';
                        if (current) list.insertBefore(sep, current);
                        else list.appendChild(sep);
                    }
                    insertionIndex++;
                    continue;
                }

                const session = filtered.find(s => s.session_id === key);
                const isFloating = session.session_id === this.floatingSessionId;
                const isCurrent = session.session_id === this.currentAcpSessionId;
                const isActive = session.session_id === this.activeSessionId;
                const title = session.title || 'New Chat';
                const date = new Date(session.updated_at || session.created_at);
                const dateStr = this.formatDate(date);

                let item = existingById.get(key);
                if (item) {
                    // Reuse existing DOM node — update only what changed
                    item.classList.toggle('active', isActive);

                    const titleEl = item.querySelector('.session-item-title');
                    const badges = (isCurrent || isFloating) ? '<span class="session-current-badge">●</span>' : '';
                    const newTitleHtml = `${escapeHtml(title)}${badges}`;
                    if (titleEl && titleEl.innerHTML !== newTitleHtml) titleEl.innerHTML = newTitleHtml;

                    const dateEl = item.querySelector('.session-item-date');
                    const dateSuffix = (isCurrent || isFloating) ? ' · <span class="session-default-label">default session</span>' : '';
                    const newDateHtml = `${dateStr}${dateSuffix}`;
                    if (dateEl && dateEl.innerHTML !== newDateHtml) dateEl.innerHTML = newDateHtml;
                } else {
                    // Create new item
                    item = this._createSessionItem(session, { isFloating, isCurrent, isActive, title, dateStr });
                    existingById.set(key, item);
                }

                // Ensure correct position in DOM
                if (list.children[insertionIndex] !== item) {
                    if (insertionIndex < list.children.length) {
                        list.insertBefore(item, list.children[insertionIndex]);
                    } else {
                        list.appendChild(item);
                    }
                }
                insertionIndex++;
            }

            // Remove any trailing stale children
            while (list.children.length > insertionIndex) {
                list.lastChild.remove();
            }
        }

    /** Create a new session-item DOM element with event listeners. */
    _createSessionItem(session, { isFloating, isCurrent, isActive, title, dateStr }) {
            const item = document.createElement('div');
            item.className = 'session-item' + (isActive ? ' active' : '');
            item.dataset.sessionId = session.session_id;

            const badges = (isCurrent || isFloating) ? '<span class="session-current-badge">●</span>' : '';
            const dateSuffix = (isCurrent || isFloating) ? ' · <span class="session-default-label">default session</span>' : '';

            item.innerHTML = `
                <div class="session-item-content">
                    <div class="session-item-title">${escapeHtml(title)}${badges}</div>
                    <div class="session-item-date">${dateStr}${dateSuffix}</div>
                </div>
                <div class="session-item-actions">
                    <button class="session-action-btn session-action-edit" title="Rename">
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.85 2.85 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/><path d="m15 5 4 4"/></svg>
                    </button>
                    <button class="session-action-btn session-action-reveal" title="Show file">
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/></svg>
                    </button>
                    <button class="session-action-btn session-action-delete" title="Delete">
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/></svg>
                    </button>
                </div>
            `;

            item.querySelector('.session-action-edit').addEventListener('click', (e) => {
                e.stopPropagation();
                this.startInlineRename(session.session_id, item);
            });
            item.querySelector('.session-action-reveal').addEventListener('click', (e) => {
                e.stopPropagation();
                this.revealSessionFile(session.session_id);
            });
            item.querySelector('.session-action-delete').addEventListener('click', (e) => {
                e.stopPropagation();
                this.deleteSession(session.session_id, title);
            });

            item.addEventListener('click', () => this.selectSession(session.session_id));
            return item;
        }

    formatDate(date) {
        const now = new Date();
        const diff = now - date;
        const days = Math.floor(diff / (1000 * 60 * 60 * 24));

        if (days === 0) {
            return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
        } else if (days === 1) {
            return 'Yesterday';
        } else if (days < 7) {
            return date.toLocaleDateString([], { weekday: 'short' });
        } else {
            return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
        }
    }

    async selectSession(sessionId) {
            if (sessionId === this.activeSessionId) return;

            this.activeSessionId = sessionId;
            this.renderSessionList();

            // Clear any previous error
            this.elements.errorContainer.innerHTML = '';

            // Hide/show permission modal based on which session is active
            if (window.ChatPermissions) {
                window.ChatPermissions.onSessionSwitch(sessionId);
            }

            // Load and display session messages from files immediately
            try {
                const sessionData = await this.invoke('load_session', { sessionId });
                this.displaySession(sessionData);
            } catch (error) {
                console.error('Failed to load session files:', error);
                this.showError('Failed to load session: ' + this.formatError(error));
            }

            // Show connecting state in the input
            this.elements.chatInput.disabled = true;
            this.elements.chatInput.placeholder = 'Connecting to session...';
            this.elements.sendBtn.disabled = true;

            // Switch ACP session in parallel
            try {
                await this.invoke('switch_acp_session', { sessionId });
                console.log('ACP session switched to:', sessionId);
                this.isConnected = true;
                this.updateConnectionStatus();
                this.elements.chatInput.disabled = false;
                this.elements.chatInput.placeholder = 'Type your message...';
                this.elements.sendBtn.disabled = false;
                this.elements.chatInput.focus();
            } catch (error) {
                console.error('Failed to switch ACP session:', error);
                const msg = this.formatError(error);
                this.showError(msg);
                this.isConnected = false;
                this.updateConnectionStatus();
                // Keep input disabled on session error
                this.elements.chatInput.disabled = true;
                this.elements.chatInput.placeholder = 'Session unavailable';
                this.elements.sendBtn.disabled = true;
            }
        }

    displaySession(sessionData) {
        this.messages = [];
        this.elements.messagesArea.innerHTML = '';
        this.toolSources = [];
        this.toolUsages = [];
        this._toolCallIds = new Set();
        const timestamps = sessionData.message_timestamps || {};
        const durations = sessionData.message_durations || {};

        if (!sessionData.messages || sessionData.messages.length === 0) {
            this.elements.messagesArea.innerHTML = '<div class="message-placeholder">Empty session</div>';
            return;
        }

        // Phase 1: parse messages into lightweight render instructions (no DOM work)
        const renderQueue = this._buildRenderQueue(sessionData.messages, timestamps, durations);

        if (renderQueue.length === 0) {
            this.elements.messagesArea.innerHTML = '<div class="message-placeholder">Empty session</div>';
            return;
        }

        // Phase 2: render in batches to avoid blocking the main thread
        const BATCH_SIZE = 10;
        let idx = 0;
        // Cancel any previous in-flight batch render
        if (this._displaySessionRafId) {
            cancelAnimationFrame(this._displaySessionRafId);
            this._displaySessionRafId = null;
        }

        const renderBatch = () => {
            const end = Math.min(idx + BATCH_SIZE, renderQueue.length);
            for (; idx < end; idx++) {
                this._renderQueueItem(renderQueue[idx]);
            }
            if (idx < renderQueue.length) {
                this._displaySessionRafId = requestAnimationFrame(renderBatch);
            } else {
                this._displaySessionRafId = null;
                // All messages rendered — finalize
                const session = this.sessions.find(s => s.session_id === this.activeSessionId);
                if (session) {
                    this.elements.chatHeaderTitle.textContent = session.title || 'Chat';
                }
                this.scrollToBottom();
                if (this.messages.length > 0) {
                    this.showSuggestionChips();
                }
            }
        };

        // Update header title immediately (don't wait for batches)
        const session = this.sessions.find(s => s.session_id === this.activeSessionId);
        if (session) {
            this.elements.chatHeaderTitle.textContent = session.title || 'Chat';
        }

        renderBatch();
    }

    /**
     * Parse session messages into a lightweight render queue.
     * No DOM work — just data extraction.
     */
    _buildRenderQueue(messages, timestamps, durations) {
        const queue = [];
        let skipNextAssistant = false;

        for (const msg of messages) {
            if (msg.kind === 'Prompt') {
                let textParts = [];
                let imageDataUrls = [];
                let isSteering = false;
                for (const item of msg.content) {
                    if (item.kind === 'text' && typeof item.data === 'string') {
                        if (item.data.startsWith(STEERING_MSG_PREFIX)) {
                            isSteering = true;
                            textParts.push(item.data.substring(STEERING_MSG_PREFIX.length).trim());
                            continue;
                        }
                        textParts.push(item.data);
                    } else if (item.kind === 'image') {
                        const dataUrl = sessionImageToDataUrl(item);
                        if (dataUrl) imageDataUrls.push(dataUrl);
                    }
                }

                if (isSteering) {
                    skipNextAssistant = true;
                    queue.push({ type: 'steering', text: textParts.join('\n\n') });
                    continue;
                }

                if (textParts.length > 0 || imageDataUrls.length > 0) {
                    const text = textParts.join('\n');
                    const snapshots = imageDataUrls.length > 0 ? imageDataUrls.map(url => ({ type: 'image', previewUrl: url })) : null;
                    queue.push({
                        type: 'user',
                        text,
                        snapshots,
                        meta: _buildMsgMeta(msg.message_id, timestamps, durations, 'user'),
                    });
                }
            } else if (msg.kind === 'AssistantMessage') {
                if (skipNextAssistant) {
                    skipNextAssistant = false;
                    const ackText = [];
                    for (const item of msg.content) {
                        if (item.kind === 'text' && typeof item.data === 'string' && item.data.trim()) {
                            ackText.push(item.data.trim());
                        }
                    }
                    if (ackText.length > 0) {
                        queue.push({ type: 'steering_ack', text: ackText.join(' ') });
                    }
                    continue;
                }
                const textParts = [];
                for (const item of msg.content) {
                    if (item.kind === 'text' && typeof item.data === 'string' && item.data.trim()) {
                        textParts.push(item.data);
                    }
                }
                if (textParts.length > 0) {
                    queue.push({
                        type: 'assistant',
                        text: textParts.join('\n\n'),
                        meta: _buildMsgMeta(msg.message_id, timestamps, durations, 'assistant'),
                    });
                }
            }
        }
        return queue;
    }

    /**
     * Render a single item from the render queue into the DOM.
     */
    _renderQueueItem(item) {
        switch (item.type) {
            case 'steering': {
                const steeringEl = document.createElement('div');
                steeringEl.className = 'steering-message collapsed';
                steeringEl.innerHTML = `
                    <div class="steering-header" onclick="this.parentElement.classList.toggle('collapsed')">
                        <span class="steering-icon">🛞</span>
                        <span class="steering-label">Steering context sent</span>
                        <span class="steering-toggle">▶</span>
                    </div>
                    <div class="steering-body">
                        <div class="steering-content"></div>
                    </div>
                `;
                const contentEl = steeringEl.querySelector('.steering-content');
                renderMarkdown(item.text, contentEl);
                this.elements.messagesArea.appendChild(steeringEl);
                break;
            }
            case 'steering_ack': {
                const lastSteering = this.elements.messagesArea.querySelector('.steering-message:last-of-type');
                if (lastSteering) {
                    const ackEl = document.createElement('div');
                    ackEl.className = 'steering-ack';
                    ackEl.textContent = '↩ ' + item.text;
                    lastSteering.querySelector('.steering-body').appendChild(ackEl);
                }
                break;
            }
            case 'user':
                this.addMessageFromHistory('user', item.text, item.snapshots, item.meta);
                break;
            case 'assistant':
                this.addMessageFromHistory('assistant', item.text, null, item.meta);
                break;
        }
    }

    addMessageFromHistory(role, text, imageSnapshots, meta) {
        const msgEl = this.createMessageElement(role, '');
        const contentDiv = msgEl.querySelector('.message-content');
        if (role === 'assistant') {
            renderMarkdown(text, contentDiv);
        } else {
            if (text) contentDiv.textContent = text;
        }
        if (imageSnapshots && imageSnapshots.length > 0) {
            contentDiv.insertAdjacentHTML('beforeend', attachmentPreviewHtml(imageSnapshots));
        }
        // Set timestamp and duration from session metadata
        if (meta) {
            const ts = msgEl.querySelector('.msg-timestamp');
            if (ts && meta.timestamp) {
                const date = new Date(meta.timestamp);
                if (!isNaN(date)) {
                    let label = date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
                    if (role === 'assistant' && meta.durationSecs) {
                        label += ` (${this._formatDuration(meta.durationSecs)})`;
                    }
                    ts.textContent = label;
                }
            }
        }
        this.elements.messagesArea.appendChild(msgEl);
        this.messages.push({ role, content: text });
    }

    _formatDuration(totalSecs) {
        const secs = Math.round(totalSecs);
        if (secs < 60) return `${secs}s`;
        const mins = Math.floor(secs / 60);
        const rem = secs % 60;
        return rem > 0 ? `${mins}m${rem}s` : `${mins}m`;
    }

    async createNewSession() {
        this.messages = [];
        this.toolSources = [];
        this.toolUsages = [];
        this._toolCallIds = new Set();
        this.elements.messagesArea.innerHTML = '<div class="message-placeholder">Start a conversation with Kiro...</div>';
        this.elements.chatHeaderTitle.textContent = 'New Chat';
        this.elements.chatInput.focus();

        try {
            const newId = await this.invoke('switch_acp_session', { sessionId: null });
            this.activeSessionId = newId;
            // Add the new session to the list so it appears immediately
            if (!this.sessions.find(s => s.session_id === newId)) {
                this.sessions.push({
                    session_id: newId,
                    title: 'New Chat',
                    created_at: new Date().toISOString(),
                    updated_at: new Date().toISOString()
                });
            }
            this.renderSessionList();
            console.log('Created new ACP session:', newId);

            // Send steering for the new session (fire and forget)
            try {
                await this.invoke('send_steering_message');
            } catch (e) {
                console.log('Steering message not sent (may be disabled):', e);
            }
        } catch (error) {
            console.error('Failed to create new session:', error);
            this.renderSessionList();
        }
    }

    async deleteSession(sessionId, title) {
        const isActive = sessionId === this.activeSessionId;
        const isCurrent = sessionId === this.currentAcpSessionId || sessionId === this.floatingSessionId;

        if (isCurrent) {
            this.showError('Cannot delete the active session. Switch to a different session first.');
            return;
        }

        let dir = '';
        try { dir = await this.invoke('get_sessions_directory'); } catch { /* ignore */ }

        const msg = `Delete session "${title} from ${dir || 'sessions directory'}?\n\n• ${sessionId}.json\n• ${sessionId}.jsonl\n• ${sessionId}.lock\n\nThis cannot be undone.`;
        if (!confirm(msg)) return;

        try {
            await this.invoke('delete_session', { sessionId });
            this.sessions = this.sessions.filter(s => s.session_id !== sessionId);

            if (isActive) {
                // Clear the display
                this.activeSessionId = null;
                this.elements.messagesArea.innerHTML = '<div class="message-placeholder">Select a session to continue...</div>';
                this.elements.chatHeaderTitle.textContent = 'Kiro Assistant';
            }

            this.renderSessionList();
        } catch (e) {
            this.showError('Failed to delete session: ' + e);
        }
    }

    // --- Speech ---

    setupSpeech() {
        this.speech = new SpeechController({
            invoke: this.invoke,
            elements: {
                input: this.elements.chatInput,
                speechBtn: this.elements.chatSpeechBtn,
                speechWave: this.elements.chatSpeechWave
            },
            onSend: (text) => {
                this.elements.chatInput.value = text;
                this.sendMessage();
            },
            onVisibilityUpdate: () => {},
            barContainer: document.querySelector('.chat-input-container') || document.querySelector('.chat-input'),
        });
        this.speech.setup();
    }


    // --- Messaging ---

    async sendMessage() {
        // Stop any ongoing TTS and speech recognition
        if (this.speech) {
            this.speech.cancelSpeech();
            if (this.speech.isListening) this.speech.stop();
        }

        let message = this.elements.chatInput.value.trim();
        const hasAttachments = this.attachmentManager.hasAttachments();
        const hasPendingFiles = this._pendingFiles && this._pendingFiles.length > 0;
        if ((!message && !hasAttachments && !hasPendingFiles) || this.isWaitingForResponse) return;

        // If a plan is pending review, send the message as a revision request
        if (this._pendingPlanRevision && message) {
            this._pendingPlanRevision = null;
            this._automationPlanStarted = false;
            this.elements.chatInput.value = '';
            this.elements.chatInput.style.height = 'auto';
            this.clearSuggestions();

            // Add user message to chat
            const userMsg = this.createMessageElement('user', message);
            this.elements.messagesArea.appendChild(userMsg);
            this.messages.push({ role: 'user', content: message });

            // Set up for new streaming response
            this.currentStreamingContent = '';
            this.toolSources = [];
            this.toolUsages = [];
            this._toolCallIds = new Set();
            this.isWaitingForResponse = true;
            this._extensionToolCallHandled = false;
            this._extensionToolExecuting = false;
            this._streamStartTime = Date.now();
            this.updateInputState();
            this.showTypingIndicator();
            this.currentStreamingMessage = this.createMessageElement('assistant', '');
            this.elements.messagesArea.appendChild(this.currentStreamingMessage);
            this.scrollToBottom();

            try {
                await this.invoke('send_message_streaming', { message, attachments: null });
            } catch (e) {
                this.handleMessageError({ payload: 'Error: ' + e });
            }
            return;
        }

        // Clear suggestions
        this.clearSuggestions();
        this.hideSuggestionChips();

        // Read pending file contents and prepend to message
        if (hasPendingFiles) {
            const fileParts = [];
            for (const file of this._pendingFiles) {
                try {
                    const text = await file.text();
                    const truncated = text.length > 100000 ? text.substring(0, 100000) + '\n\n[...truncated at 100k chars]' : text;
                    fileParts.push(`Contents of \`${file.name}\`:\n\`\`\`\n${truncated}\n\`\`\``);
                } catch (e) {
                    fileParts.push(`Could not read \`${file.name}\`: ${e.message}`);
                }
            }
            this._pendingFiles = [];
            const fileBlock = fileParts.join('\n\n');
            message = message ? fileBlock + '\n\n' + message : fileBlock;
        }

        const attachments = this.attachmentManager.toContentBlocks();
        const attachmentSnapshots = hasAttachments ? [...this.attachmentManager.attachments] : null;
        this.attachmentManager.clear();

        this.elements.chatInput.value = '';
        this.elements.chatInput.style.height = 'auto';

        // Handle > local commands
        if (!hasAttachments && message.startsWith('>')) {
            const cmdText = message.substring(1).trim();
            if (cmdText && await executeCommand(cmdText, this.invoke, this.appWindow)) {
                return;
            }
        }

        // Handle / slash commands (only if no attachments)
        if (!hasAttachments && message.startsWith('/')) {
            try {
                const parts = message.split(' ');
                const cmdName = parts[0].substring(1); // strip leading /
                const cmdArgs = parts.length > 1 ? { input: parts.slice(1).join(' ') } : {};
                const result = await this.invoke('execute_slash_command', {
                    command: cmdName,
                    args: cmdArgs
                });
                // Show the command and result in the chat (suppress compact — handled by compaction_status)
                this.addUserMessage(message);
                if (cmdName !== 'compact') {
                    const resultText = result?.message || JSON.stringify(result, null, 2);
                    this.addMessageFromHistory('assistant', resultText);
                }
                this.scrollToBottom();
                return;
            } catch (e) {
                console.error('Slash command failed:', e);
                this.addUserMessage(message);
                this.addMessageFromHistory('assistant', 'Command failed: ' + e);
                this.scrollToBottom();
                return;
            }
        }

        this.addUserMessage(message, attachmentSnapshots);
        this.startStreaming();

        try {
            await this.invoke('send_message_streaming', { message, attachments });
            this.isConnected = true;
            this.updateConnectionStatus();
        } catch (error) {
            this.hideTypingIndicator();
            if (this.currentStreamingMessage) {
                this.currentStreamingMessage.remove();
                this.currentStreamingMessage = null;
            }
            this.showError('Error: ' + error);
            this.isConnected = false;
            this.updateConnectionStatus();
            this.isWaitingForResponse = false;
            this.updateInputState();
        }
    }

    addUserMessage(text, attachmentSnapshots) {
        const placeholder = this.elements.messagesArea.querySelector('.message-placeholder');
        if (placeholder) placeholder.remove();

        this.messages.push({ role: 'user', content: text });
        const msgEl = this.createMessageElement('user', text);

        // Set timestamp
        const ts = msgEl.querySelector('.msg-timestamp');
        if (ts) ts.textContent = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

        // Append attachment previews to the message bubble
        if (attachmentSnapshots && attachmentSnapshots.length > 0) {
            const contentDiv = msgEl.querySelector('.message-content');
            if (contentDiv) {
                contentDiv.insertAdjacentHTML('beforeend', attachmentPreviewHtml(attachmentSnapshots));
            }
        }

        this.elements.messagesArea.appendChild(msgEl);
        this.scrollToBottom();
    }

    startStreaming() {
            this.currentStreamingContent = '';
            this.toolSources = [];
            this.toolUsages = [];
            this._toolCallIds = new Set();
            this.isWaitingForResponse = true;
            this._extensionToolCallHandled = false;
            this._extensionToolExecuting = false;
            this._streamStartTime = Date.now();
            this.updateInputState();
            this.showTypingIndicator();

            this.currentStreamingMessage = this.createMessageElement('assistant', '');
            this.elements.messagesArea.appendChild(this.currentStreamingMessage);
            this.scrollToBottom();
        }

    stopGenerating() {
        if (!this.isWaitingForResponse) return;

        // If an automation plan is running, stop it gracefully
        if (this._automationPlanStarted && this._automationStatuses) {
            for (const [step, status] of Object.entries(this._automationStatuses)) {
                if (status === 'running') {
                    this._automationStatuses[step] = 'stopped';
                }
            }
            this._renderAutomationPlanChat();
            if (this._automationCleanup) this._automationCleanup();
            this._automationPlanStarted = false;
        }

        this.isWaitingForResponse = false;
        this.hideTypingIndicator();

        if (this.currentStreamingMessage && !this._automationPlan) {
            const contentDiv = this.currentStreamingMessage.querySelector('.message-content');
            const indicator = contentDiv?.querySelector('.streaming-indicator');
            if (indicator) indicator.remove();
            if (this.currentStreamingContent) {
                renderMarkdown(this.currentStreamingContent, contentDiv);
            }
            if (this.toolSources.length > 0 || this.toolUsages.length > 0) {
                this.renderSourcesInMessage(contentDiv);
            }
            this.currentStreamingMessage = null;
        }

        this.updateInputState();
        this.elements.chatInput.focus();
        this.scrollToBottom();
        this.invoke('cancel_generation').catch(e => console.log('Cancel:', e));
    }


    createMessageElement(role, content) {
        const msg = document.createElement('div');
        msg.className = `message ${role}`;

        const avatar = document.createElement('div');
        avatar.className = 'message-avatar';
        if (role === 'assistant') {
            avatar.innerHTML = `<svg width="18" height="18" viewBox="0 0 65 47" fill="none" xmlns="http://www.w3.org/2000/svg">
                <path d="M5.71599 33.2597C21.3537 50.3579 43.692 49.7224 56.8482 37.6892C64.8725 30.3497 68.8862 13.8647 55.4115 3.72686C41.9368 -6.41103 32.4042 11.2128 17.2667 8.73447C14.1417 8.22797 9.94157 9.04188 12.6668 12.7323C13.1844 13.4347 13.8741 13.9921 14.4889 14.4572C10.198 14.6069 8.69922 14.3808 6.07118 14.3457C3.69479 14.2406 2.01125 14.368 1.05082 15.569C-0.207458 17.5201 3.17874 20.5431 6.24957 23.1473C8.02071 24.8452 9.81893 27.134 10.9737 29.0437C9.58639 28.7602 9.25032 28.6837 7.17973 28.4703C3.87477 28.131 1.42511 28.5658 5.71759 33.2597H5.71599Z" fill="white"/>
                <path d="M48.5012 21.9388C46.3685 22.1093 45.8461 19.5864 45.7234 18.0669C45.6135 16.6955 45.7712 15.5853 46.1821 14.8574C46.5437 14.2172 47.1044 13.862 47.8482 13.8015C48.5936 13.7409 49.2578 14.0037 49.7579 14.5851C50.3281 15.2477 50.6865 16.3101 50.7948 17.6591C50.9986 20.2075 50.1417 21.8066 48.5012 21.9372V21.9388Z" fill="black"/>
                <path d="M57.2707 21.2344C55.138 21.4048 54.614 18.8819 54.493 17.3624C54.3831 15.991 54.5407 14.8825 54.9517 14.153C55.3116 13.5127 55.8739 13.1575 56.6177 13.097C57.3631 13.0381 58.0273 13.2993 58.5274 13.8807C59.0976 14.5433 59.456 15.6056 59.5643 16.9547C59.7682 19.5031 58.9113 21.1022 57.2707 21.2328V21.2344Z" fill="black"/>
            </svg>`;
        } else {
            if (this.userInfo?.avatar_base64) {
                const img = document.createElement('img');
                img.src = this.userInfo.avatar_base64;
                img.style.cssText = 'width:100%;height:100%;border-radius:50%;object-fit:cover';
                img.onerror = () => { avatar.textContent = this.userInfo?.initials || '?'; img.remove(); };
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
                <button class="msg-action-btn" data-action="copy" title="Copy">
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
                </button>
                <button class="msg-action-btn" data-action="speak" title="Read aloud" style="display:${this._showSpeakBtn ? '' : 'none'}">
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><path d="M15.54 8.46a5 5 0 0 1 0 7.07"/></svg>
                </button>
                <button class="msg-action-btn" data-action="translate" title="Translate" style="display:${this._showTranslateBtn ? '' : 'none'}">
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m5 8 6 6"/><path d="m4 14 6-6 2-3"/><path d="M2 5h12"/><path d="M7 2h1"/><path d="m22 22-5-10-5 10"/><path d="M14 18h6"/></svg>
                </button>
                <span class="msg-timestamp"></span>
            `;
            // Wire up action buttons
            actions.querySelector('[data-action="copy"]').onclick = () => {
                const text = contentDiv.textContent || '';
                navigator.clipboard.writeText(text).then(() => {
                    const btn = actions.querySelector('[data-action="copy"]');
                    btn.innerHTML = '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>';
                    setTimeout(() => { btn.innerHTML = '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>'; }, 1500);
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
                    const config = await this.invoke('get_config');
                    const lang = config.quick_actions?.translate_language || 'English';
                    this.elements.chatInput.value = `Translate the following to ${lang}:\n\n${text.substring(0, 500)}`;
                    this.elements.chatInput.focus();
                } catch (e) { console.warn('Translate failed:', e); }
            };
            bubble.appendChild(actions);
        }

        msg.appendChild(avatar);
        msg.appendChild(bubble);

        return msg;
    }

    // --- Streaming Handlers ---

    handleMessageChunk(event) {
        if (!this.isWaitingForResponse || !this.currentStreamingMessage) return;

        this.currentStreamingContent = event.payload;
        this.hideTypingIndicator();

        // Detect complete automation plan during streaming and execute immediately
        if (!this._automationPlanStarted) {
            const completePlan = detectAutomationPlan(this.currentStreamingContent);
            if (completePlan) {
                this._automationPlanStarted = true;
                const contentDiv = this.currentStreamingMessage.querySelector('.message-content');
                this._showPlanForReviewChat(completePlan, contentDiv);
                return;
            }

            // Show tasks incrementally as they stream in
            const partialPlan = detectAutomationPlanIncremental(this.currentStreamingContent);
            if (partialPlan) {
                this._automationPlan = partialPlan;
                this._automationStatuses = {};
                this._automationResults = {};
                for (const s of partialPlan) this._automationStatuses[s.step] = 'pending';
                const contentDiv = this.currentStreamingMessage.querySelector('.message-content');
                const tasks = automationPlanToTasks(partialPlan, this._automationStatuses, {});
                const wrapper = createTaskPlanElement(tasks);
                contentDiv.innerHTML = '';
                contentDiv.appendChild(wrapper);
                this.scrollToBottom();
                return;
            }
        }

        // If automation plan is running, don't overwrite the plan UI
        if (this._automationPlanStarted) return;

        // Detect extension tool calls in streaming text
        if (!this._extensionToolCallHandled) {
            const toolCall = detectExtensionToolCall(this.currentStreamingContent);
            if (toolCall) {
                this._extensionToolCallHandled = true;
                const contentDiv = this.currentStreamingMessage.querySelector('.message-content');
                this._handleExtensionToolCall(toolCall, contentDiv);
                return;
            }

            const partial = detectExtensionToolCallIncremental(this.currentStreamingContent);
            if (partial) {
                const contentDiv = this.currentStreamingMessage.querySelector('.message-content');
                this._renderExtensionToolIndicator(partial, contentDiv);
                this.scrollToBottom();
                return;
            }
        } else if (!this._extensionToolExecuting) {
            if (!this.currentStreamingContent.includes('```extension_tool_call')) {
                this._extensionToolCallHandled = false;
            }
        }

        // If extension tool is executing, don't overwrite the indicator
        if (this._extensionToolExecuting) return;

        const contentDiv = this.currentStreamingMessage.querySelector('.message-content');
        renderMarkdown(this.currentStreamingContent, contentDiv, true);

        // Remove tool-running spinner now that text is flowing
        const toolSpinner = contentDiv.querySelector('.tool-running-indicator');
        if (toolSpinner) toolSpinner.remove();

        // Feed streaming text to TTS for sentence-chunked playback
        if (this.speech) this.speech.feedStreamingText(this.currentStreamingContent);

        let indicator = contentDiv.querySelector('.streaming-indicator');
        if (!indicator) {
            indicator = document.createElement('span');
            indicator.className = 'streaming-indicator';
            indicator.textContent = '...';
            contentDiv.appendChild(indicator);
        }

        this.scrollToBottom();
    }

    async handleMessageComplete() {
            if (!this.isWaitingForResponse) return;

            // If automation plan is running, don't overwrite the plan UI
            if (this._automationPlanStarted) return;

            // If extension tool is executing, the response will come as a follow-up
            if (this._extensionToolExecuting || this._extensionToolCallHandled) return;

            // Check for extension tool call in the completed response (fallback if not caught during streaming)
            if (!this._extensionToolCallHandled) {
                const toolCall = detectExtensionToolCall(this.currentStreamingContent);
                if (toolCall) {
                    this._extensionToolCallHandled = true;
                    const contentDiv = this.currentStreamingMessage?.querySelector('.message-content');
                    this._handleExtensionToolCall(toolCall, contentDiv);
                    return;
                }
            }

            this.hideTypingIndicator();

            if (this.currentStreamingMessage) {
                const contentDiv = this.currentStreamingMessage.querySelector('.message-content');
                const indicator = contentDiv.querySelector('.streaming-indicator');
                if (indicator) indicator.remove();

                // Check for automation plan in the response (fallback if not caught during streaming)
                const plan = detectAutomationPlan(this.currentStreamingContent);
                if (plan && !this._automationPlanStarted) {
                    this._automationPlanStarted = true;
                    this._showPlanForReviewChat(plan, contentDiv);
                    return;
                }

                renderMarkdown(this.currentStreamingContent, contentDiv);

                if (this.toolSources.length > 0 || this.toolUsages.length > 0) {
                    this.renderSourcesInMessage(contentDiv);
                }

                this.messages.push({ role: 'assistant', content: this.currentStreamingContent });
                this.currentStreamingMessage = null;
            }

            const finalContent = this.messages[this.messages.length - 1]?.content || '';
            this.currentStreamingContent = '';
            this.isWaitingForResponse = false;
            this.updateInputState();
            this.elements.chatInput.focus();
            this.scrollToBottom();

            // Read back response if speech was used
            if (this.speech && finalContent) {
                this.speech.finishStreamingText(finalContent);
                this.speech.speakResponse(finalContent);
            }

            this.loadSessions();
            this.loadFloatingSessionId();

            // Set timestamp on the message actions bar
            if (this.elements.messagesArea.lastElementChild) {
                const msgEl = this.elements.messagesArea.querySelector('.message.assistant:last-of-type');
                const ts = msgEl?.querySelector('.msg-timestamp');
                if (ts) {
                    let label = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
                    if (this._streamStartTime) {
                        const durSecs = (Date.now() - this._streamStartTime) / 1000;
                        label += ` (${this._formatDuration(durSecs)})`;
                    }
                    ts.textContent = label;
                }
            }

            // Show suggestion chips
            this.showSuggestionChips();

            // Notify if window is hidden
            try {
                const isVisible = await this.appWindow.isVisible();
                if (!isVisible) {
                    const preview = (this.messages[this.messages.length - 1]?.content || '').substring(0, 100).replace(/[#*`\n]/g, ' ').trim();
                    await sendAppNotification(this.invoke, 'Kiro Assistant', preview || 'Response ready', 'main');
                }
            } catch { /* ignore */ }
        }

    handleMessageError(event) {
        this.hideTypingIndicator();

        if (this.currentStreamingMessage) {
            this.currentStreamingMessage.remove();
            this.currentStreamingMessage = null;
        }

        this.showError('Error: ' + event.payload);
        this.isConnected = false;
        this.updateConnectionStatus();
        this.isWaitingForResponse = false;
        this.updateInputState();
        this.elements.chatInput.focus();
    }

    handleSessionReset(event) {
            this.hideTypingIndicator();

            if (this.currentStreamingMessage) {
                this.currentStreamingMessage.remove();
                this.currentStreamingMessage = null;
            }

            const data = event.payload;
            const msg = getSessionResetMessage(data);
            if (data?.reason === 'image_unsupported' && data.reconnected) {
                this.isConnected = true;
                this.updateConnectionStatus();
                this.showSessionResetMessage(msg);
            } else {
                if (data?.reason === 'image_unsupported') {
                    this.isConnected = false;
                    this.updateConnectionStatus();
                }
                this.showError(msg);
            }

            this.isWaitingForResponse = false;
            this.updateInputState();
            this.elements.chatInput.focus();
            this.loadSessions();
        }

    handleToolCallUpdate(event) {
            const { updated, update } = processToolCallUpdate(event, this);

            // Flush any pending throttled markdown render so the user sees the
            // full streamed text before a permission dialog appears
            if (this.currentStreamingMessage && this.currentStreamingContent) {
                const contentDiv = this.currentStreamingMessage.querySelector('.message-content');
                if (contentDiv) {
                    renderMarkdown(this.currentStreamingContent, contentDiv);
                }
            }

            // Show a spinner while a tool is executing
            if (update?.title && this.currentStreamingMessage) {
                const contentDiv = this.currentStreamingMessage.querySelector('.message-content');
                if (contentDiv) {
                    const friendly = _chatToolFriendlyName(update.title);
                    let spinner = contentDiv.querySelector('.tool-running-indicator');
                    if (!spinner) {
                        spinner = document.createElement('div');
                        spinner.className = 'folder-plan-spinner-row tool-running-indicator';
                        contentDiv.appendChild(spinner);
                    }
                    spinner.innerHTML = `<span class="folder-plan-spinner"></span> ${friendly}...`;
                }
            }

            // Chat app doesn't render sources inline during streaming —
            // they're rendered when the message completes in renderSourcesInMessage()
        }




    renderSourcesInMessage(contentDiv) {
            let sourcesEl = contentDiv.querySelector('.tool-sources');
            if (!sourcesEl) {
                sourcesEl = document.createElement('div');
                sourcesEl.className = 'tool-sources';
                contentDiv.appendChild(sourcesEl);
            }
            sourcesEl.innerHTML = renderToolChipsHtml(this.toolUsages) + renderSourceChipsHtml(this.toolSources);
        }

    /**
     * Get the icon for an extension by ID.
     */
    _getExtensionIcon(extensionId) {
        if (!extensionId || !this.extensionManager) return '🧩';
        const defs = this.extensionManager.getToolDefinitions();
        const def = defs.find(d => d.extensionId === extensionId);
        return def?.extensionIcon || '🧩';
    }

    /**
     * Send extension tool definitions to the agent as a steering message.
     */
    async _sendExtensionToolSteering() {
        const block = this.extensionManager.buildToolSteeringBlock();
        if (!block) return;
        try {
            await this.invoke('send_extension_tool_steering', { toolSteering: block });
        } catch (e) {
            console.warn('Failed to send extension tool steering:', e);
        }
    }

    /**
     * Render a loading indicator for an extension tool call.
     */
    _renderExtensionToolIndicator(info, contentDiv) {
        if (info.extension && info.tool) {
            const toolTitle = `ext:${info.extension}/${info.tool}`;
            const toolCallId = `ext-${info.extension}-${info.tool}`;
            if (!this._toolCallIds) this._toolCallIds = new Set();
            if (!this._toolCallIds.has(toolCallId)) {
                this._toolCallIds.add(toolCallId);
                this.toolUsages.push({ toolCallId, title: toolTitle, kind: 'extension' });
            }
            this.renderSourcesInMessage(contentDiv);
        }

        // Show any text before the fence, but hide the fence itself
        if (contentDiv) {
            const beforeFence = (this.currentStreamingContent || '').split('```extension_tool_call')[0].trim();
            if (beforeFence) {
                renderMarkdown(beforeFence, contentDiv, true);
            } else {
                const friendlyName = this._getToolFriendlyName(info.extension, info.tool);
                contentDiv.innerHTML = `<div class="folder-plan-spinner-row"><span class="folder-plan-spinner"></span> ${friendlyName}...</div>`;
            }
        }
    }

    /** Look up a friendly display name for an extension tool. */
    _getToolFriendlyName(extensionId, toolName) {
        if (extensionId && toolName && this.extensionManager) {
            const defs = this.extensionManager.getToolDefinitions();
            const extDef = defs.find(d => d.extensionId === extensionId);
            if (extDef?.tools) {
                const tool = extDef.tools.find(t => t.name === toolName);
                if (tool?.friendlyName) return tool.friendlyName;
            }
        }
        return 'Working on it';
    }

    /**
     * Handle a detected extension tool call in the chat window.
     */
    async _handleExtensionToolCall(toolCall, contentDiv) {
        const { extension, tool, params } = toolCall;
        const icon = this._getExtensionIcon(extension);
        const toolTitle = `ext:${extension}/${tool}`;

        console.log(`Extension tool call: ${extension}/${tool}`, params);

        // Track as a standard tool usage
        const extToolCallId = `ext-${extension}-${tool}`;
        if (!this._toolCallIds) this._toolCallIds = new Set();
        if (!this._toolCallIds.has(extToolCallId)) {
            this._toolCallIds.add(extToolCallId);
            this.toolUsages.push({ toolCallId: extToolCallId, title: toolTitle, kind: 'extension' });
        }
        this.renderSourcesInMessage(contentDiv);

        // Check permission policy — skip if the tool has built-in confirmation UI
        const toolDefs = this.extensionManager.getToolDefinitions();
        const extDef = toolDefs.find(d => d.extensionId === extension);
        const toolDef = extDef?.tools?.find(t => t.name === tool);
        const hasBuiltInConfirmation = toolDef?.hasBuiltInConfirmation === true;

        let policy = hasBuiltInConfirmation ? 'allow' : undefined;
        if (!policy) {
            try {
                policy = await this.invoke('check_extension_tool_permission', {
                    extensionId: extension,
                    toolName: tool,
                });
            } catch (e) {
                console.error('Failed to check extension tool permission:', e);
                policy = 'ask';
            }
        }

        if (policy === 'deny') {
            this._extensionToolExecuting = false;
            this._extensionToolCallHandled = false;
            try {
                await this.invoke('extension_tool_response', {
                    extensionId: extension,
                    toolName: tool,
                    resultJson: JSON.stringify('Permission denied by user policy'),
                    success: false,
                });
            } catch (e) {
                console.error('Failed to send denial:', e);
            }
            return;
        }

        if (policy === 'ask') {
            const allowed = await window.ChatPermissions.showForExtensionTool(extension, tool, icon);
            if (!allowed) {
                this._extensionToolExecuting = false;
                this._extensionToolCallHandled = false;
                try {
                    await this.invoke('extension_tool_response', {
                        extensionId: extension,
                        toolName: tool,
                        resultJson: JSON.stringify('Permission denied by user'),
                        success: false,
                    });
                } catch (e) {
                    console.error('Failed to send denial:', e);
                }
                return;
            }
        }

        this._extensionToolExecuting = true;

        const result = await this.extensionManager.executeExtensionTool(extension, tool, params);
        const success = !result.error;
        const resultJson = JSON.stringify(success ? result.result : result.error);

        try {
            await this.invoke('extension_tool_response', {
                extensionId: extension,
                toolName: tool,
                resultJson,
                success,
            });
        } catch (e) {
            console.error('Failed to send extension tool response:', e);
        }

        this._extensionToolExecuting = false;
        // Reset state for the follow-up response. Clear currentResponse so the old
        // extension_tool_call fence doesn't get re-detected when chunks arrive.
        this.currentResponse = '';
        this._extensionToolCallHandled = false;
        this.isWaitingForResponse = true;
        // Show typing indicator while waiting for the agent's follow-up response
        this.showTypingIndicator();
    }

    /**
     * Show an automation plan for user review in the chat window.
     */
    _showPlanForReviewChat(plan, contentDiv) {
        this._automationPlan = plan;
        this._automationStatuses = {};
        this._automationResults = {};
        this._automationContentDiv = contentDiv;
        for (const s of plan) this._automationStatuses[s.step] = 'pending';
        this._renderAutomationPlanChat();

        // Add review action bar
        const actionsBar = document.createElement('div');
        actionsBar.className = 'taskplan-review-actions';
        actionsBar.innerHTML = `
            <button class="taskplan-review-btn taskplan-run-btn" id="planRunBtn">▶ Run</button>
            <span class="taskplan-review-hint">or type to revise the plan</span>
        `;
        contentDiv.appendChild(actionsBar);

        // Stop waiting state — plan is ready for review
        this.hideTypingIndicator();
        this.isWaitingForResponse = false;
        this.updateInputState();
        this.elements.chatInput.focus();
        this.scrollToBottom();

        // Run button handler
        document.getElementById('planRunBtn')?.addEventListener('click', () => {
            actionsBar.remove();
            this.isWaitingForResponse = true;
            this.updateInputState();
            this._executeAutomationPlan(plan, contentDiv);
        });

        // Flag for revision handling
        this._pendingPlanRevision = plan;
        this._pendingPlanContentDiv = contentDiv;
    }

    async _executeAutomationPlan(plan, contentDiv) {
        // Render the plan as a task list in the message
        this._automationPlan = plan;
        this._automationStatuses = {};
        this._automationResults = {};
        this._automationContentDiv = contentDiv;
        for (const s of plan) this._automationStatuses[s.step] = 'pending';
        this._renderAutomationPlanChat();
        this.scrollToBottom();

        this._automationCleanup = null;

        // Listen for step progress events
        const stepStartUnlisten = await this.listen('automation_step_start', (event) => {
            const { step } = event.payload;
            this._automationStatuses[step] = 'running';
            this._renderAutomationPlanChat();
            this.scrollToBottom();
        });

        const stepCompleteUnlisten = await this.listen('automation_step_complete', (event) => {
            const { step, success, result } = event.payload;
            this._automationStatuses[step] = success ? 'done' : 'failed';
            if (result) this._automationResults[step] = result.substring(0, 200);
            this._renderAutomationPlanChat();
            this.scrollToBottom();
        });

        const cleanup = () => {
            stepStartUnlisten();
            stepCompleteUnlisten();
            planCompleteUnlisten();
            this._automationCleanup = null;
        };
        this._automationCleanup = cleanup;

        const planCompleteUnlisten = await this.listen('automation_plan_complete', async () => {
            cleanup();
            this._automationPlanStarted = false;
            this.messages.push({ role: 'assistant', content: '[Automation plan completed]' });
            this.currentStreamingMessage = null;
            this.currentStreamingContent = '';
            this.isWaitingForResponse = false;
            this.updateInputState();
            this.elements.chatInput.focus();
            this.scrollToBottom();
            this.loadSessions();
        });

        // Execute the plan
        try {
            await this.invoke('execute_automation_plan', {
                planJson: JSON.stringify(plan)
            });
        } catch (e) {
            console.error('Automation plan execution failed:', e);
            cleanup();
            this._automationPlanStarted = false;
            this.isWaitingForResponse = false;
            this.updateInputState();
        }
    }

    _renderAutomationPlanChat() {
        if (!this._automationPlan || !this._automationContentDiv) return;
        const tasks = automationPlanToTasks(
            this._automationPlan,
            this._automationStatuses || {},
            this._automationResults || {}
        );
        const wrapper = createTaskPlanElement(tasks);
        this._automationContentDiv.innerHTML = '';
        this._automationContentDiv.appendChild(wrapper);
    }

    // --- UI Helpers ---

    showTypingIndicator() {
        this.hideTypingIndicator();
        const indicator = document.createElement('div');
        indicator.className = 'typing-indicator';
        indicator.id = 'typingIndicator';
        indicator.innerHTML = '<div class="loading-dot"></div><div class="loading-dot"></div><div class="loading-dot"></div>';
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
            btn.setAttribute('aria-label', 'Stop generating');
            btn.title = 'Stop generating (Esc)';
            if (sendIcon) sendIcon.style.display = 'none';
            if (stopIcon) stopIcon.style.display = '';
        } else {
            btn.classList.remove('stop-mode');
            btn.disabled = false;
            btn.setAttribute('aria-label', 'Send message');
            btn.title = 'Send message';
            if (sendIcon) sendIcon.style.display = '';
            if (stopIcon) stopIcon.style.display = 'none';
        }
        this.elements.chatInput.disabled = this.isWaitingForResponse;
    }

    async checkConnection() {
        try {
            this.isConnected = await this.invoke('check_connection');
        } catch (e) {
            this.isConnected = false;
        }
        this.updateConnectionStatus();
    }

    updateConnectionStatus() {
        const el = this.elements.connectionStatus;
        if (this.isConnected) {
            el.textContent = 'Connected';
            el.className = 'chat-header-status connected';
        } else {
            el.textContent = 'Disconnected';
            el.className = 'chat-header-status disconnected';
        }
    }

    showError(message) {
        this.elements.errorContainer.innerHTML = `
            <div class="chat-error">
                <span>${escapeHtml(message)}</span>
                <div class="chat-error-actions">
                    <button class="chat-error-btn reconnect" id="errorReconnectBtn">Reconnect</button>
                    <button class="chat-error-btn dismiss" id="errorDismissBtn">Dismiss</button>
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
                    this.showError('Reconnection failed.');
                }
            } catch (e) {
                this.showError('Reconnection failed: ' + e);
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
        if (!error) return 'Unknown error';
        const str = typeof error === 'string' ? error : String(error);
        // Try to extract structured error fields (e.g., "Internal error: Failed to start session: ...")
        // The Rust backend often formats as "message: data" or just the data string
        return str;
    }


    startTitleEdit() {
        if (!this.activeSessionId) return;
        const titleEl = this.elements.chatHeaderTitle;
        const inputEl = this.elements.chatHeaderTitleInput;
        inputEl.value = titleEl.textContent;
        titleEl.style.display = 'none';
        inputEl.style.display = 'inline-block';
        inputEl.focus();
        inputEl.select();
    }

    cancelTitleEdit() {
        this.elements.chatHeaderTitleInput.style.display = 'none';
        this.elements.chatHeaderTitle.style.display = '';
    }

    async finishTitleEdit() {
        const inputEl = this.elements.chatHeaderTitleInput;
        const titleEl = this.elements.chatHeaderTitle;
        const newTitle = inputEl.value.trim();

        inputEl.style.display = 'none';
        titleEl.style.display = '';

        if (!newTitle || !this.activeSessionId || newTitle === titleEl.textContent) return;

        try {
            await this.invoke('rename_session', {
                sessionId: this.activeSessionId,
                title: newTitle
            });
            titleEl.textContent = newTitle;
            // Update in the sessions list too
            const session = this.sessions.find(s => s.session_id === this.activeSessionId);
            if (session) session.title = newTitle;
            this.renderSessionList();
        } catch (e) {
            console.error('Failed to rename session:', e);
        }
    }

    async revealSessionFile(sessionId) {
        const id = sessionId || this.activeSessionId;
        if (!id) return;
        try {
            await this.invoke('reveal_session_file', { sessionId: id });
        } catch (e) {
            console.error('Failed to reveal session file:', e);
        }
    }

    startInlineRename(sessionId, itemEl) {
        const titleEl = itemEl.querySelector('.session-item-title');
        if (!titleEl) return;
        const currentTitle = titleEl.textContent.replace('●', '').trim();
        const input = document.createElement('input');
        input.className = 'session-rename-input';
        input.value = currentTitle;
        input.maxLength = 80;

        const contentEl = itemEl.querySelector('.session-item-content');
        contentEl.style.display = 'none';
        itemEl.querySelector('.session-item-actions').style.display = 'none';
        itemEl.insertBefore(input, itemEl.firstChild);
        input.focus();
        input.select();

        const finish = async () => {
            const newTitle = input.value.trim();
            input.remove();
            contentEl.style.display = '';
            itemEl.querySelector('.session-item-actions').style.display = '';

            if (newTitle && newTitle !== currentTitle) {
                try {
                    await this.invoke('rename_session', { sessionId, title: newTitle });
                    const session = this.sessions.find(s => s.session_id === sessionId);
                    if (session) session.title = newTitle;
                    if (sessionId === this.activeSessionId) {
                        this.elements.chatHeaderTitle.textContent = newTitle;
                    }
                    this.renderSessionList();
                } catch (e) {
                    console.error('Failed to rename:', e);
                }
            }
        };

        input.addEventListener('blur', finish);
        input.addEventListener('keydown', (e) => {
            if (e.key === 'Enter') { e.preventDefault(); input.blur(); }
            if (e.key === 'Escape') { input.value = currentTitle; input.blur(); }
        });
    }


    async loadShortcuts() {
        try {
            const config = await this.invoke('get_config');
            this.shortcuts = config.shortcuts || [];
        } catch {
            this.shortcuts = [];
        }
    }

    async updateSuggestions() {
        const input = this.elements.chatInput.value;
        const trimmed = input.trim();

        if (!trimmed) {
            this.clearSuggestions();
            return;
        }

        this._searchGeneration = (this._searchGeneration || 0) + 1;
        const gen = this._searchGeneration;
        const results = await unifiedSearch(trimmed, this.invoke, this.shortcuts);
        if (gen !== this._searchGeneration) return;
        if (results.length > 0) {
            this.currentSuggestions = results;
            this.suggestionIndex = 0;
            this.renderSuggestions();
        } else {
            this.clearSuggestions();
        }
    }

    renderSuggestions() {
        const container = this.elements.chatSuggestions;
        container.innerHTML = '';

        if (this.currentSuggestions.length === 0) {
            container.classList.remove('visible');
            return;
        }

        const extMgr = getExtensionManager();

        this.currentSuggestions.forEach((cmd, index) => {
            const item = document.createElement('div');
            item.className = 'chat-suggestion-item' + (index === this.suggestionIndex ? ' selected' : '');

            // Let extensions render their own results
            if (cmd._extensionId && extMgr) {
                const customEl = document.createElement('div');
                customEl.style.cssText = 'display:flex;align-items:center;gap:8px;flex:1;';
                if (extMgr.renderResult(cmd, customEl)) {
                    item.appendChild(customEl);
                    item.addEventListener('click', () => this.executeSuggestion(cmd));
                    container.appendChild(item);
                    return;
                }
            }

            // Default rendering for non-extension results
            let iconHtml;
            if (cmd.type === 'app' && cmd.data?.icon_base64) {
                const src = cmd.data.icon_base64.startsWith('data:') ? cmd.data.icon_base64 : 'data:image/png;base64,' + cmd.data.icon_base64;
                iconHtml = `<img src="${src}" style="width:20px;height:20px;border-radius:4px;" onerror="this.replaceWith(document.createTextNode('${cmd.icon || cmd.label.charAt(0)}'))">`;
            } else {
                iconHtml = `<span class="chat-suggestion-icon">${cmd.icon || cmd.label?.charAt(0) || '?'}</span>`;
            }

            item.innerHTML = `
                ${iconHtml}
                <div class="chat-suggestion-info">
                    <div class="chat-suggestion-name">${escapeHtml(cmd.label || cmd.name || '')}</div>
                    ${cmd.description ? `<div class="chat-suggestion-desc">${escapeHtml(cmd.description)}</div>` : ''}
                </div>
            `;
            item.addEventListener('click', () => this.executeSuggestion(cmd));
            container.appendChild(item);
        });

        container.classList.add('visible');
    }

    clearSuggestions() {
        this.currentSuggestions = [];
        this.suggestionIndex = -1;
        this.elements.chatSuggestions.innerHTML = '';
        this.elements.chatSuggestions.classList.remove('visible');
    }

    /** Build execution context for the shared result executor. */
    _getExecCtx() {
        return {
            invoke: this.invoke,
            appWindow: this.appWindow,
            extensionManager: getExtensionManager(),
            onPrompt: (text) => {
                this.elements.chatInput.value = text;
                this.sendMessage();
            },
            onDisplay: (text) => {
                this.addMessageFromHistory('assistant', text);
                this.scrollToBottom();
            },
            onCopy: async (text) => { try { await navigator.clipboard.writeText(text); } catch {} },
        };
    }

    async executeSuggestion(cmd) {
        const query = this.elements.chatInput.value.trim();
        this.elements.chatInput.value = '';
        this.elements.chatInput.style.height = 'auto';
        this.clearSuggestions();

        const result = await executeResultShared(cmd, query, this._getExecCtx());
        if (result.handled) return;

        // Fallback for unhandled types
        if (cmd.execute) {
            await cmd.execute(this.invoke, this.appWindow);
        }
    }

    async handleEnterKey() {
        const message = this.elements.chatInput.value.trim();
        const hasAttachments = this.attachmentManager.hasAttachments();
        const hasSelection = this.currentSuggestions.length > 0 && this.suggestionIndex >= 0;

        if (!message && !hasAttachments && !hasSelection) return;

        if (this.isWaitingForResponse) {
            this.stopGenerating();
        }

        const result = await handleEnterAction({
            message,
            suggestions: this.currentSuggestions,
            selectedIndex: this.suggestionIndex,
            shortcuts: this.shortcuts,
            ctx: this._getExecCtx(),
            onSend: (msg) => {
                this.elements.chatInput.value = msg;
                this.sendMessage();
            },
        });

        if (result.handled) {
            this.elements.chatInput.value = '';
            this.elements.chatInput.style.height = 'auto';
            this.clearSuggestions();
        }
    }

    scrollToBottom() {
        const area = this.elements.messagesArea;
        requestAnimationFrame(() => {
            area.scrollTo({ top: area.scrollHeight, behavior: 'smooth' });
        });
    }


    convertFileSrc(path) {
        // Tauri 2 uses asset protocol for local files
        if (window.__TAURI__?.core?.convertFileSrc) {
            return window.__TAURI__.core.convertFileSrc(path);
        }
        // Fallback: use file:// protocol
        return 'file://' + path.replace(/\\/g, '/');
    }

    // --- Toolbar: File & Image Attach ---

    async handleFileAttach(event) {
        const files = event.target.files;
        if (!files || files.length === 0) return;
        for (const file of files) {
            this.attachmentManager.addFile(file.name, file.name, file.type || 'text/plain');
        }
        // Store the actual File objects so we can read them at send time
        if (!this._pendingFiles) this._pendingFiles = [];
        for (const file of files) {
            this._pendingFiles.push(file);
        }
        event.target.value = '';
    }

    async handleImageAttach(event) {
        const files = event.target.files;
        if (!files || files.length === 0) return;
        for (const file of files) {
            if (!file.type.startsWith('image/')) continue;
            try {
                const base64 = await this._fileToBase64(file);
                this.attachmentManager.addImage(base64, file.type);
            } catch (e) {
                console.error('Failed to read image:', file.name, e);
            }
        }
        event.target.value = '';
    }

    _fileToBase64(file) {
        return new Promise((resolve, reject) => {
            const reader = new FileReader();
            reader.onload = () => {
                const result = reader.result;
                const base64 = result.split(',')[1];
                resolve(base64);
            };
            reader.onerror = reject;
            reader.readAsDataURL(file);
        });
    }

    // --- Toolbar: Context Indicator ---

    async refreshContextUsage() {
        try {
            const result = await this.invoke('execute_slash_command', {
                command: 'context',
                args: {}
            });
            const msg = result?.message || JSON.stringify(result);
            const match = msg.match(/(\d+)%/);
            if (match) {
                const pct = parseInt(match[1], 10);
                this.elements.contextPercent.textContent = pct + '%';
                document.getElementById('contextIndicator').title = pct + '% context used';
                this.drawContextRing(pct);
            }
        } catch (e) {
            console.log('[CONTEXT] Failed to fetch context usage:', e);
        }
    }

    drawContextRing(percent) {
        const canvas = document.getElementById('contextRing');
        if (!canvas) return;
        const ctx = canvas.getContext('2d');
        const size = 16;
        const cx = size / 2, cy = size / 2, r = 6;
        const lineWidth = 2;
        ctx.clearRect(0, 0, size, size);

        // Background ring (gray track)
        const isDark = document.body.classList.contains('dark-theme');
        ctx.beginPath();
        ctx.arc(cx, cy, r, 0, Math.PI * 2);
        ctx.strokeStyle = isDark ? 'rgba(255,255,255,0.15)' : 'rgba(0,0,0,0.1)';
        ctx.lineWidth = lineWidth;
        ctx.stroke();

        // Filled arc
        if (percent > 0) {
            let color = '#22c55e'; // green
            if (percent >= 90) color = '#ef4444'; // red
            else if (percent >= 75) color = '#eab308'; // yellow
            const startAngle = -Math.PI / 2;
            const endAngle = startAngle + (Math.PI * 2 * Math.min(percent, 100) / 100);
            ctx.beginPath();
            ctx.arc(cx, cy, r, startAngle, endAngle);
            ctx.strokeStyle = color;
            ctx.lineWidth = lineWidth;
            ctx.lineCap = 'round';
            ctx.stroke();
        }
    }

    // --- Auto-Compact ---

    async maybeAutoCompact(percent) {
        if (this._isCompacting) return;
        try {
            const config = await this.invoke('get_config');
            const threshold = config.acp?.assistant?.auto_compact_threshold ?? 90;
            if (threshold === 0 || percent < threshold) return;
        } catch { return; }

        this._isCompacting = true;
        try {
            await this.invoke('execute_slash_command', {
                command: 'compact',
                args: {}
            });
        } catch (e) {
            console.error('[COMPACT] Auto-compact failed:', e);
            this.hideCompactingNotice('Auto-compact failed');
        }
        this._isCompacting = false;
    }

    showCompactingNotice() {
        let notice = document.getElementById('compactingNotice');
        if (!notice) {
            notice = document.createElement('div');
            notice.id = 'compactingNotice';
            notice.className = 'compacting-notice';
            this.elements.messagesArea.appendChild(notice);
        }
        notice.classList.remove('compacting-done');
        notice.innerHTML = '<span class="compacting-spinner"></span> Compacting context...';
        notice.style.display = '';
        this.scrollToBottom();
    }

    hideCompactingNotice(message) {
        const notice = document.getElementById('compactingNotice');
        if (notice) {
            notice.innerHTML = '📦 ' + message;
            notice.classList.add('compacting-done');
            notice.removeAttribute('id'); // Make it static so next compaction creates a new one
        }
    }

    // --- Toolbar: Model Selector ---

    async loadModels() {
        try {
            const models = await this.invoke('get_available_models');
            this.availableModels = models || [];
            if (this.availableModels.length > 0) {
                // Try to find the current model name from the first model or a marked current one
                const current = this.availableModels[0];
                this.elements.modelName.textContent = current.name || current.modelId || 'Unknown';
                this.currentModelId = current.modelId;
            } else {
                this.elements.modelName.textContent = 'No models';
            }
        } catch (e) {
            console.log('[MODELS] Failed to load models:', e);
            this.elements.modelName.textContent = 'Unavailable';
        }
    }

    toggleModelDropdown() {
        const dd = this.elements.modelDropdown;
        if (dd.style.display !== 'none') {
            dd.style.display = 'none';
            return;
        }
        dd.innerHTML = '';
        if (!this.availableModels || this.availableModels.length === 0) {
            dd.innerHTML = '<div class="chat-model-dropdown-item"><span class="chat-model-dropdown-item-name">No models available</span></div>';
            dd.style.display = '';
            return;
        }
        for (const model of this.availableModels) {
            const item = document.createElement('div');
            item.className = 'chat-model-dropdown-item' + (model.modelId === this.currentModelId ? ' active' : '');
            item.innerHTML = `
                <span class="chat-model-dropdown-item-name">${escapeHtml(model.name || model.modelId)}</span>
                <span class="chat-model-dropdown-item-desc">${escapeHtml(model.description || '')}</span>
            `;
            item.addEventListener('click', () => this.selectModel(model));
            dd.appendChild(item);
        }
        dd.style.display = '';
    }

    async selectModel(model) {
        this.elements.modelDropdown.style.display = 'none';
        this.elements.modelName.textContent = model.name || model.modelId;
        this.currentModelId = model.modelId;
        try {
            await this.invoke('execute_slash_command', {
                command: 'model',
                args: { modelName: model.modelId }
            });
        } catch (e) {
            console.error('[MODELS] Failed to switch model:', e);
            this.showError('Failed to switch model: ' + e);
        }
    }

    /**
     * Render extension-contributed toolbar buttons into the chat toolbar.
     */
    renderExtensionToolbarButtons() {
        if (!this.extensionManager) return;
        const buttons = this.extensionManager.getToolbarButtons();
        if (buttons.length === 0) return;

        const toolbarLeft = document.querySelector('.chat-toolbar-left');
        if (!toolbarLeft) return;

        // Remove any previously rendered extension buttons
        toolbarLeft.querySelectorAll('.ext-toolbar-btn').forEach(el => el.remove());

        for (const btn of buttons) {
            const el = document.createElement('button');
            el.className = 'chat-toolbar-btn ext-toolbar-btn';
            el.title = btn.tooltip || btn.id;
            el.innerHTML = typeof btn.icon === 'string' && btn.icon.startsWith('<')
                ? btn.icon  // SVG string
                : `<span style="font-size:16px;">${btn.icon || '🔧'}</span>`;
            el.addEventListener('click', () => {
                try {
                    btn.onClick?.({
                        invoke: this.invoke,
                        getInput: () => this.elements.chatInput?.value || '',
                        setInput: (v) => { if (this.elements.chatInput) this.elements.chatInput.value = v; },
                        getMessages: () => this.messages || [],
                    });
                } catch (e) {
                    console.warn(`Extension toolbar button error (${btn.extensionId}):`, e);
                }
            });
            toolbarLeft.appendChild(el);
        }
    }
}
