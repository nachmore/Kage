// Expanded chat application logic
import { renderMarkdown, initMarkdown } from './floating-markdown.js';
import { AttachmentManager, handlePasteEvent, setupDragDrop, renderAttachmentPreviews, attachmentPreviewHtml, sessionImageToDataUrl } from './attachments.js';

/** Prefix used to identify steering messages that should be hidden in the UI */
const STEERING_MSG_PREFIX = '[KIRO_STEERING_IGNORE]';

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
        this.activeSessionId = null;
        this.floatingSessionId = null;
        this.currentAcpSessionId = null;
        this.toolSources = [];
        this.toolUsages = [];
        this.userInfo = null;
        this.attachmentManager = new AttachmentManager();

        this.elements = {};
    }

    async init() {
        initMarkdown();
        this.cacheElements();
        this.setupEventListeners();
        this.setupStreamingListeners();
        await this.loadFloatingSessionId();
        await this.loadCurrentSessionId();
        await this.loadUserInfo();

        // Load sessions in background — don't block init
        this.loadSessions();

        await this.checkConnection();

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
        console.log('Chat app initialized');
    }

    cacheElements() {
        this.elements = {
            chatInput: document.getElementById('chatInput'),
            sendBtn: document.getElementById('sendBtn'),
            messagesArea: document.getElementById('messagesArea'),
            sessionList: document.getElementById('sessionList'),
            newSessionBtn: document.getElementById('newSessionBtn'),
            settingsBtn: document.getElementById('settingsBtn'),
            floatingBtn: document.getElementById('floatingBtn'),
            connectionStatus: document.getElementById('connectionStatus'),
            chatHeaderTitle: document.getElementById('chatHeaderTitle'),
            errorContainer: document.getElementById('errorContainer'),
            attachmentPreviews: document.getElementById('attachmentPreviews'),
            chatMain: document.querySelector('.chat-main')
        };
    }

    setupEventListeners() {
        this.elements.chatInput.addEventListener('input', () => {
            this.elements.chatInput.style.height = 'auto';
            this.elements.chatInput.style.height = Math.min(this.elements.chatInput.scrollHeight, 120) + 'px';
        });

        this.elements.chatInput.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                this.sendMessage();
            }
        });

        this.elements.sendBtn.addEventListener('click', () => this.sendMessage());
        this.elements.newSessionBtn.addEventListener('click', () => this.createNewSession());

        this.elements.settingsBtn.addEventListener('click', async () => {
            await this.invoke('open_settings_window');
        });

        this.elements.floatingBtn.addEventListener('click', async () => {
            await this.invoke('test_floating_window');
        });

        // Paste handler for images
        this.elements.chatInput.addEventListener('paste', (e) => handlePasteEvent(e, this.attachmentManager));

        // Drag-and-drop for files on the main chat area
        setupDragDrop(this.elements.chatMain, this.elements.chatMain, this.attachmentManager);

        // Re-render previews when attachments change
        this.attachmentManager.onChange((attachments) => {
            renderAttachmentPreviews(this.elements.attachmentPreviews, attachments, this.attachmentManager);
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
            if (e.key === 'Escape' && lightbox.style.display !== 'none') {
                lightbox.style.display = 'none';
                lightboxImg.src = '';
            }
        });
    }

    setupStreamingListeners() {
        this.listen('message_chunk', (event) => this.handleMessageChunk(event));
        this.listen('message_complete', () => this.handleMessageComplete());
        this.listen('message_error', (event) => this.handleMessageError(event));
        this.listen('tool_call_update', (event) => this.handleToolCallUpdate(event));
        this.listen('session_reset', (event) => this.handleSessionReset(event));

        this.listen('initial_message', (event) => {
            const message = event.payload;
            if (message) {
                this.addUserMessage(message);
                this.startStreaming();
            }
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
            console.log('[USER] User info loaded:', JSON.stringify(this.userInfo));
        } catch (e) {
            console.error('[USER] Failed to get user info:', e);
            this.userInfo = null;
        }
    }

    async loadSessions() {
        try {
            const sessions = await this.invoke('list_sessions');
            this.sessions = sessions;
            this.renderSessionList();
        } catch (error) {
            console.error('Failed to load sessions:', error);
            this.sessions = [];
            this.renderSessionList();
        }
    }

    renderSessionList() {
        const list = this.elements.sessionList;

        if (this.sessions.length === 0) {
            list.innerHTML = '<div class="session-list-empty">No sessions yet</div>';
            return;
        }

        // Sort: default session (current/floating) first, then by updated_at descending
        const defaultId = this.currentAcpSessionId || this.floatingSessionId;
        const sorted = [...this.sessions].sort((a, b) => {
            const aIsDefault = a.session_id === defaultId;
            const bIsDefault = b.session_id === defaultId;
            if (aIsDefault && !bIsDefault) return -1;
            if (!aIsDefault && bIsDefault) return 1;
            return (b.updated_at || '').localeCompare(a.updated_at || '');
        });

        list.innerHTML = '';
        for (const session of sorted) {
            const item = document.createElement('div');
            item.className = 'session-item' + (session.session_id === this.activeSessionId ? ' active' : '');
            item.dataset.sessionId = session.session_id;

            const isFloating = session.session_id === this.floatingSessionId;
            const isCurrent = session.session_id === this.currentAcpSessionId;
            const title = session.title || 'New Chat';
            const date = new Date(session.updated_at || session.created_at);
            const dateStr = this.formatDate(date);

            let badges = '';
            if (isCurrent || isFloating) badges += '<span class="session-current-badge">●</span>';

            let dateSuffix = '';
            if (isCurrent || isFloating) dateSuffix = ' · <span class="session-default-label">default session</span>';

            item.innerHTML = `
                <div class="session-item-title">${this.escapeHtml(title)}${badges}</div>
                <div class="session-item-date">${dateStr}${dateSuffix}</div>
            `;

            item.addEventListener('click', () => this.selectSession(session.session_id));
            list.appendChild(item);

            // Add separator after the default session
            if ((isCurrent || isFloating) && list.querySelectorAll('.session-list-separator').length === 0) {
                const sep = document.createElement('div');
                sep.className = 'session-list-separator';
                list.appendChild(sep);
            }
        }
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
            this.showError('Failed to load session: ' + error);
        }

        // Show connecting state in the input
        this.elements.chatInput.disabled = true;
        this.elements.chatInput.placeholder = 'Connecting to session...';
        this.elements.sendBtn.disabled = true;

        // Switch ACP session in parallel
        try {
            await this.invoke('switch_acp_session', { sessionId });
            console.log('ACP session switched to:', sessionId);
        } catch (error) {
            console.error('Failed to switch ACP session:', error);
            this.showError('Failed to connect to session: ' + error);
        } finally {
            this.elements.chatInput.disabled = false;
            this.elements.chatInput.placeholder = 'Type your message...';
            this.elements.sendBtn.disabled = false;
            this.elements.chatInput.focus();
        }
    }

    displaySession(sessionData) {
        this.messages = [];
        this.elements.messagesArea.innerHTML = '';
        this.toolSources = [];
        this.toolUsages = [];

        if (!sessionData.messages || sessionData.messages.length === 0) {
            this.elements.messagesArea.innerHTML = '<div class="message-placeholder">Empty session</div>';
            return;
        }

        // Walk through the JSONL messages in order
        let isFirstMessage = true;
        let skipNextAssistant = false;
        for (const msg of sessionData.messages) {
            if (msg.kind === 'Prompt') {
                // Collect text and images from content blocks
                let textParts = [];
                let imageDataUrls = [];
                for (const item of msg.content) {
                    if (item.kind === 'text' && typeof item.data === 'string') {
                        // Hide the steering message if it is the very first message
                        if (isFirstMessage && item.data.startsWith(STEERING_MSG_PREFIX)) {
                            isFirstMessage = false;
                            skipNextAssistant = true;
                            continue;
                        }
                        isFirstMessage = false;
                        textParts.push(item.data);
                    } else if (item.kind === 'image') {
                        isFirstMessage = false;
                        const dataUrl = sessionImageToDataUrl(item);
                        if (dataUrl) imageDataUrls.push(dataUrl);
                    }
                }

                // Render user message with text and images
                if (textParts.length > 0 || imageDataUrls.length > 0) {
                    const text = textParts.join('\n');
                    const snapshots = imageDataUrls.map(url => ({
                        type: 'image',
                        previewUrl: url
                    }));
                    this.addMessageFromHistory('user', text, snapshots.length > 0 ? snapshots : null);
                }
            } else if (msg.kind === 'AssistantMessage') {
                isFirstMessage = false;
                // Skip the assistant response to the steering message
                if (skipNextAssistant) {
                    skipNextAssistant = false;
                    continue;
                }
                // Assistant message — extract text content, skip tool use entries
                const textParts = [];
                for (const item of msg.content) {
                    if (item.kind === 'text' && typeof item.data === 'string' && item.data.trim()) {
                        textParts.push(item.data);
                    }
                }
                if (textParts.length > 0) {
                    this.addMessageFromHistory('assistant', textParts.join('\n\n'));
                }
            }
            // ToolResults are skipped in the display — they're intermediate
        }

        // Update header title
        const session = this.sessions.find(s => s.session_id === this.activeSessionId);
        if (session) {
            this.elements.chatHeaderTitle.textContent = session.title || 'Chat';
        }

        this.scrollToBottom();
    }

    addMessageFromHistory(role, text, imageSnapshots) {
        const msgEl = this.createMessageElement(role, '');
        const contentDiv = msgEl.querySelector('.message-content');
        if (role === 'assistant') {
            renderMarkdown(text, contentDiv);
        } else {
            if (text) contentDiv.textContent = text;
        }
        // Append image previews if present
        if (imageSnapshots && imageSnapshots.length > 0) {
            contentDiv.insertAdjacentHTML('beforeend', attachmentPreviewHtml(imageSnapshots));
        }
        this.elements.messagesArea.appendChild(msgEl);
        this.messages.push({ role, content: text });
    }

    async createNewSession() {
        this.messages = [];
        this.toolSources = [];
        this.toolUsages = [];
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

    // --- Messaging ---

    async sendMessage() {
        const message = this.elements.chatInput.value.trim();
        const hasAttachments = this.attachmentManager.hasAttachments();
        if ((!message && !hasAttachments) || this.isWaitingForResponse) return;

        const attachments = this.attachmentManager.toContentBlocks();
        const attachmentSnapshots = hasAttachments ? [...this.attachmentManager.attachments] : null;
        this.attachmentManager.clear();

        this.elements.chatInput.value = '';
        this.elements.chatInput.style.height = 'auto';

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
                // Show the command and result in the chat
                this.addUserMessage(message);
                const resultText = result?.message || JSON.stringify(result, null, 2);
                this.addMessageFromHistory('assistant', resultText);
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
        this.isWaitingForResponse = true;
        this.updateInputState();
        this.showTypingIndicator();

        this.currentStreamingMessage = this.createMessageElement('assistant', '');
        this.elements.messagesArea.appendChild(this.currentStreamingMessage);
        this.scrollToBottom();
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
            // User avatar: profile picture > initials > fallback
            if (this.userInfo?.avatar_base64) {
                console.log('[USER] Using base64 avatar, length:', this.userInfo.avatar_base64.length);
                const img = document.createElement('img');
                img.src = this.userInfo.avatar_base64;
                img.style.cssText = 'width:100%;height:100%;border-radius:50%;object-fit:cover';
                img.onload = () => console.log('[USER] Avatar image loaded successfully');
                img.onerror = (e) => {
                    console.error('[USER] Avatar image failed to load:', e);
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

        const header = document.createElement('div');
        header.className = 'message-header';
        header.textContent = role === 'user' ? (this.userInfo?.display_name || 'You') : 'Kiro';

        const contentDiv = document.createElement('div');
        contentDiv.className = 'message-content';
        if (content) contentDiv.textContent = content;

        bubble.appendChild(header);
        bubble.appendChild(contentDiv);
        msg.appendChild(avatar);
        msg.appendChild(bubble);

        return msg;
    }

    // --- Streaming Handlers ---

    handleMessageChunk(event) {
        if (!this.isWaitingForResponse || !this.currentStreamingMessage) return;

        this.currentStreamingContent = event.payload;
        this.hideTypingIndicator();

        const contentDiv = this.currentStreamingMessage.querySelector('.message-content');
        renderMarkdown(this.currentStreamingContent, contentDiv);

        let indicator = contentDiv.querySelector('.streaming-indicator');
        if (!indicator) {
            indicator = document.createElement('span');
            indicator.className = 'streaming-indicator';
            indicator.textContent = '...';
            contentDiv.appendChild(indicator);
        }

        this.scrollToBottom();
    }

    handleMessageComplete() {
        if (!this.isWaitingForResponse) return;

        this.hideTypingIndicator();

        if (this.currentStreamingMessage) {
            const contentDiv = this.currentStreamingMessage.querySelector('.message-content');
            const indicator = contentDiv.querySelector('.streaming-indicator');
            if (indicator) indicator.remove();

            renderMarkdown(this.currentStreamingContent, contentDiv);

            if (this.toolSources.length > 0 || this.toolUsages.length > 0) {
                this.renderSourcesInMessage(contentDiv);
            }

            this.messages.push({ role: 'assistant', content: this.currentStreamingContent });
            this.currentStreamingMessage = null;
        }

        this.currentStreamingContent = '';
        this.isWaitingForResponse = false;
        this.updateInputState();
        this.elements.chatInput.focus();
        this.scrollToBottom();

        // Reload sessions to pick up new/updated session
        this.loadSessions();
        this.loadFloatingSessionId();
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
        if (data?.reason === 'image_unsupported') {
            const reconnected = data.reconnected;
            if (reconnected) {
                this.isConnected = true;
                this.updateConnectionStatus();
                this.showSessionResetMessage(
                    '🖼️ The current model doesn\'t support images. A new session has been started automatically — try switching to a vision-capable model.'
                );
            } else {
                this.isConnected = false;
                this.updateConnectionStatus();
                this.showError(
                    '🖼️ The current model doesn\'t support images and the connection could not be restored. Please reconnect manually.'
                );
            }
        } else {
            this.showError('Session was reset due to an error.');
        }

        this.isWaitingForResponse = false;
        this.updateInputState();
        this.elements.chatInput.focus();

        // Reload sessions since a new one was created
        this.loadSessions();
    }

    handleToolCallUpdate(event) {
        const notification = event.payload;
        const update = notification?.params?.update;
        if (!update) return;

        // Track tool usage for display
        if (update.title && update.toolCallId) {
            if (!this.toolUsages.find(t => t.toolCallId === update.toolCallId)) {
                this.toolUsages.push({
                    toolCallId: update.toolCallId,
                    title: update.title,
                    kind: update.kind
                });
            }
        }

        const rawOutput = update.rawOutput;
        if (rawOutput && (update.kind === 'search' || update.title?.toLowerCase().includes('search'))) {
            this.extractSources(rawOutput);
        }

        if (update.content && Array.isArray(update.content)) {
            for (const item of update.content) {
                if (item.type === 'content' && item.content?.text) {
                    this.extractSourcesFromText(item.content.text);
                }
            }
        }
    }

    // --- Tool Sources ---

    extractSources(rawOutput) {
        const tryExtract = (results) => {
            if (Array.isArray(results)) {
                for (const r of results) { if (r.url) this.addSource(r.url, r.title, r.domain); }
            }
        };

        if (rawOutput?.items && Array.isArray(rawOutput.items)) {
            for (const item of rawOutput.items) {
                tryExtract(item?.Json?.results || item?.results);
            }
        } else if (Array.isArray(rawOutput)) {
            tryExtract(rawOutput);
        } else if (typeof rawOutput === 'object') {
            tryExtract(rawOutput.results || rawOutput.searchResults);
        }
    }

    extractSourcesFromText(text) {
        const linkRegex = /\[([^\]]*)\]\((https?:\/\/[^\s)]+)\)/g;
        let match;
        while ((match = linkRegex.exec(text)) !== null) {
            this.addSource(match[2], match[1]);
        }
    }

    addSource(url, title, domainHint) {
        try {
            const parsed = new URL(url);
            const domain = domainHint || parsed.hostname.replace(/^www\./, '');
            if (!this.toolSources.find(s => s.domain === domain)) {
                const initials = domain.split('.')[0].substring(0, 2).toUpperCase();
                let hash = 0;
                for (let i = 0; i < domain.length; i++) {
                    hash = domain.charCodeAt(i) + ((hash << 5) - hash);
                }
                const hue = Math.abs(hash) % 360;
                this.toolSources.push({
                    url, domain,
                    title: title || domain,
                    initials,
                    color: `hsl(${hue}, 55%, 45%)`,
                    favicon: `https://www.google.com/s2/favicons?domain=${domain}&sz=32`
                });
            }
        } catch (e) { /* skip */ }
    }

    renderSourcesInMessage(contentDiv) {
        let sourcesEl = contentDiv.querySelector('.tool-sources');
        if (!sourcesEl) {
            sourcesEl = document.createElement('div');
            sourcesEl.className = 'tool-sources';
            contentDiv.appendChild(sourcesEl);
        }

        const getToolIcon = (kind) => {
            const k = (kind || '').toLowerCase();
            if (k === 'search' || k === 'web_search') return '🔍';
            if (k === 'edit' || k === 'write') return '✏️';
            if (k === 'read') return '📖';
            if (k === 'shell' || k === 'terminal') return '💻';
            if (k === 'fetch' || k === 'web') return '🌐';
            return '🔧';
        };

        const toolChips = this.toolUsages.map(t => `
            <span class="source-chip tool-chip" title="Tool: ${this.escapeHtml(t.title)}">
                <span class="tool-chip-icon">${getToolIcon(t.kind)}</span>
                <span class="source-domain">Tool: ${this.escapeHtml(t.title)}</span>
            </span>
        `).join('');

        const sourceChips = this.toolSources.map(s => `
            <a class="source-chip" href="#" onclick="event.preventDefault(); window.__TAURI__.core.invoke('open_url', { url: '${s.url.replace(/'/g, "\\'")}' })" title="${this.escapeHtml(s.title)}">
                <span class="source-icon-wrapper">
                    <span class="source-initials" style="background:${s.color}">${s.initials}</span>
                    <img class="source-favicon" src="${s.favicon}" alt="" onload="this.previousElementSibling.style.display='none'" onerror="this.style.display='none'">
                </span>
                <span class="source-domain">${this.escapeHtml(s.domain)}</span>
            </a>
        `).join('');

        sourcesEl.innerHTML = toolChips + sourceChips;
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
        this.elements.sendBtn.disabled = this.isWaitingForResponse;
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
                <span>${this.escapeHtml(message)}</span>
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
        msgEl.innerHTML = `<span>${this.escapeHtml(message)}</span>`;
        this.elements.messagesArea.appendChild(msgEl);
        this.scrollToBottom();
    }

    scrollToBottom() {
        const area = this.elements.messagesArea;
        requestAnimationFrame(() => {
            area.scrollTo({ top: area.scrollHeight, behavior: 'smooth' });
        });
    }

    escapeHtml(str) {
        const div = document.createElement('div');
        div.textContent = str;
        return div.innerHTML;
    }

    convertFileSrc(path) {
        // Tauri 2 uses asset protocol for local files
        if (window.__TAURI__?.core?.convertFileSrc) {
            return window.__TAURI__.core.convertFileSrc(path);
        }
        // Fallback: use file:// protocol
        return 'file://' + path.replace(/\\/g, '/');
    }
}
