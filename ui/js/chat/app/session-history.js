export function createSessionHistoryMixin(dependencies) {
    const {
        renderMarkdown,
        attachmentPreviewHtml,
        sessionImageToDataUrl,
        stripKageTags,
        errLabel,
        t,
        buildRenderQueue,
        formatDuration,
    } = dependencies;
    return class {
        async selectSession(sessionId) {
            if (sessionId === this.activeSessionId) return;

            // Switching AWAY from a mid-stream session: detach the viewport
            // without touching the turn. The agent keeps working; the
            // registry keeps its 'streaming' badge; the backend accumulator
            // keeps the text for switch-back. Only the window-local render
            // state is dropped.
            if (this.isWaitingForResponse) {
                this.hideTypingIndicator();
                this.currentStreamingMessage = null;
                this.currentStreamingContent = '';
                this.isWaitingForResponse = false;
            }

            // Mark as seen (removes the "new" indicator)
            this._seenSessionIds.add(sessionId);

            this.activeSessionId = sessionId;
            // Entering the session consumes its unread badge (if any).
            this.streamRegistry.markRead(sessionId);
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
                this.showError(errLabel(t('chat.error.failed_load_session'), error));
            }

            // Mid-stream switch-in: the session is already live in the agent
            // process — do NOT `switch_acp_session` (its session/load would
            // both be redundant and drop live chunks while the replay guard
            // is up). Pin the window, re-attach to the stream, and render
            // the text streamed so far from the backend accumulator.
            if (this.streamRegistry.isStreaming(sessionId)) {
                this.currentAcpSessionId = sessionId;
                this.invoke('set_window_session', {
                    label: this.windowLabel,
                    sessionId,
                }).catch(() => {});
                await this._attachToLiveStream(sessionId);
                this.isConnected = true;
                this.updateConnectionStatus();
                this.elements.chatInput.disabled = false;
                this.elements.chatInput.placeholder = t('chat.placeholder.type_message');
                this.elements.sendBtn.disabled = false;
                return;
            }

            // Show connecting state in the input
            this.elements.chatInput.disabled = true;
            this.elements.chatInput.placeholder = t('chat.placeholder.connecting');
            this.elements.sendBtn.disabled = true;

            // Switch ACP session in parallel
            try {
                const adoptedId = await this.invoke('switch_acp_session', { sessionId });
                this.currentAcpSessionId = adoptedId;
                console.log('ACP session switched to:', adoptedId);
                this.isConnected = true;
                this.updateConnectionStatus();
                this.elements.chatInput.disabled = false;
                this.elements.chatInput.placeholder = t('chat.placeholder.type_message');
                this.elements.sendBtn.disabled = false;
                this.elements.chatInput.focus();
            } catch (error) {
                console.error('Failed to switch ACP session:', error);
                const msg = this.formatError(error);
                const isLocked =
                    msg.includes('active in another process') || msg.includes('Session is active');
                if (isLocked) {
                    const pidMatch = msg.match(/PID\s+(\d+)/);
                    this.showSessionLocked(sessionId, pidMatch ? pidMatch[1] : null);
                } else {
                    this.showError(msg);
                }
                this.isConnected = false;
                this.updateConnectionStatus();
                // Keep input disabled on session error
                this.elements.chatInput.disabled = true;
                this.elements.chatInput.placeholder = isLocked
                    ? t('chat.placeholder.session_readonly')
                    : t('chat.placeholder.session_unavailable');
                this.elements.sendBtn.disabled = true;
            }
        }

        /**
         * Re-attach the viewport to a turn that's already in flight on
         * `sessionId`. Seeds the transcript with the user prompt that started
         * the turn and the streaming message with the text streamed so far
         * (both peeked from the backend — disk only has completed turns) plus
         * any tool chips the registry tracked while the session was
         * backgrounded, then lets the normal chunk stream continue rendering
         * from that point.
         */
        async _attachToLiveStream(sessionId) {
            // Restore tool chips tracked while backgrounded.
            const entry = this.streamRegistry.get(sessionId);
            if (entry) {
                this.toolUsages = entry.toolUsages;
                this.toolSources = entry.toolSources;
                this._toolCallIds = entry._toolCallIds;
                this._sourceDomains = entry._sourceDomains;
            }

            let snapshot = '';
            let prompt = null;
            try {
                const peek = await this.invoke('get_session_stream_snapshot', { sessionId });
                snapshot = peek?.text || '';
                prompt = peek?.prompt || null;
            } catch (e) {
                console.warn('[chat] stream snapshot failed, attaching empty:', e);
            }

            // The stream may have completed while we awaited the snapshot —
            // the accumulator is evicted on MESSAGE_COMPLETE, and our
            // complete listener consumed the registry entry. displaySession
            // already painted the final text from disk in that case.
            if (!this.streamRegistry.isStreaming(sessionId)) return;

            // The user's own message isn't on disk until the turn completes —
            // paint it from the in-flight record so switching away and back
            // mid-turn doesn't make the prompt vanish.
            const promptText = prompt ? stripKageTags(prompt) : '';
            if (promptText) {
                this.elements.messagesArea.appendChild(
                    this.createMessageElement('user', promptText)
                );
            }

            // The backend accumulator runs AHEAD of the emitted chunk stream
            // (accumulate happens before the batcher flush), so the first
            // chunk after attach can duplicate the snapshot's tail. The
            // chunk listener trims that overlap once.
            this._trimNextChunkOverlap = true;
            this.currentStreamingContent = snapshot;
            this.isWaitingForResponse = true;
            this._streamStartTime = entry?.startedAt || Date.now();
            this.updateInputState();
            this.currentStreamingMessage = this.createMessageElement('assistant', '');
            this.elements.messagesArea.appendChild(this.currentStreamingMessage);
            if (snapshot) {
                const contentDiv = this.currentStreamingMessage.querySelector('.message-content');
                renderMarkdown(snapshot, contentDiv, true);
            } else {
                this.showTypingIndicator();
            }
            this.scrollToBottom();
        }

        displaySession(sessionData) {
            this.messages = [];
            this.elements.messagesArea.innerHTML = '';
            this.toolSources = [];
            this.toolUsages = [];
            this._toolCallIds = new Set();
            this._sourceDomains = new Set();
            const timestamps = sessionData.message_timestamps || {};
            const durations = sessionData.message_durations || {};

            if (!sessionData.messages || sessionData.messages.length === 0) {
                this.elements.messagesArea.innerHTML = `<div class="message-placeholder">${t('chat.placeholder.empty_session')}</div>`;
                return;
            }

            // Phase 1: parse messages into lightweight render instructions (no DOM work)
            const fullQueue = this._buildRenderQueue(sessionData.messages, timestamps, durations);

            if (fullQueue.length === 0) {
                this.elements.messagesArea.innerHTML = `<div class="message-placeholder">${t('chat.placeholder.empty_session')}</div>`;
                return;
            }

            // Cap the initial render: a long session (hundreds–thousands of
            // turns) would otherwise materialize the whole transcript in the
            // DOM — unbounded memory/layout cost, each assistant message
            // paying markdown/Prism/mermaid parsing. Older items render on
            // demand via the "load earlier" affordance.
            const INITIAL_RENDER_CAP = 100;
            let renderQueue = fullQueue;
            if (fullQueue.length > INITIAL_RENDER_CAP) {
                const hiddenItems = fullQueue.slice(0, fullQueue.length - INITIAL_RENDER_CAP);
                renderQueue = fullQueue.slice(fullQueue.length - INITIAL_RENDER_CAP);
                this._appendLoadEarlierButton(hiddenItems);
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
                    const session = this.sessions.find(
                        (s) => s.session_id === this.activeSessionId
                    );
                    if (session) {
                        this.elements.chatHeaderTitle.textContent =
                            stripKageTags(session.title) || t('chat.session.fallback_title');
                    }
                    this.scrollToBottom();
                    if (this.messages.length > 0) {
                        this.showSuggestionChips();
                    }
                }
            };

            // Update header title immediately (don't wait for batches)
            const session = this.sessions.find((s) => s.session_id === this.activeSessionId);
            if (session) {
                this.elements.chatHeaderTitle.textContent =
                    stripKageTags(session.title) || t('chat.session.fallback_title');
            }

            renderBatch();
        }

        /**
         * Prepend a "load earlier messages" button holding the un-rendered
         * head of the transcript. Clicking renders those items above the
         * existing ones, preserving the scroll position.
         */
        _appendLoadEarlierButton(hiddenItems) {
            const area = this.elements.messagesArea;
            const btn = document.createElement('button');
            btn.className = 'load-earlier-btn';
            btn.textContent = t('chat.transcript.load_earlier', { count: hiddenItems.length });
            btn.addEventListener('click', () => {
                btn.remove();
                // Render into a detached holder (the render helpers append to
                // this.elements.messagesArea, so point it at the holder for
                // the duration), then prepend the results in order.
                const holder = document.createElement('div');
                const prevMessages = this.messages;
                this.messages = [];
                this.elements.messagesArea = holder;
                try {
                    for (const item of hiddenItems) {
                        this._renderQueueItem(item);
                    }
                } finally {
                    this.elements.messagesArea = area;
                }
                // Keep this.messages in transcript order.
                this.messages = [...this.messages, ...prevMessages];
                const prevScrollHeight = area.scrollHeight;
                area.prepend(...holder.childNodes);
                // Hold the viewport on the message the user was looking at.
                area.scrollTop += area.scrollHeight - prevScrollHeight;
            });
            area.appendChild(btn);
        }

        /**
         * Parse session messages into a lightweight render queue.
         * No DOM work — just data extraction.
         */
        _buildRenderQueue(messages, timestamps, durations) {
            return buildRenderQueue(messages, timestamps, durations, sessionImageToDataUrl);
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
                        <span class="steering-icon"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="2"/><line x1="12" y1="14" x2="12" y2="22"/><line x1="10" y1="12" x2="2.5" y2="10.7"/><line x1="14" y1="12" x2="21.5" y2="10.7"/></svg></span>
                        <span class="steering-label">${t('chat.message.steering_label')}</span>
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
                    const lastSteering = this.elements.messagesArea.querySelector(
                        '.steering-message:last-of-type'
                    );
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
                // Strip internal Kage tags from display
                const displayText = text ? stripKageTags(text) : text;
                if (displayText) contentDiv.textContent = displayText;
            }
            if (imageSnapshots && imageSnapshots.length > 0) {
                contentDiv.insertAdjacentHTML('beforeend', attachmentPreviewHtml(imageSnapshots));
            }
            // Set timestamp and duration from session metadata
            if (meta) {
                const ts = msgEl.querySelector('.msg-timestamp');
                if (ts && meta.timestamp) {
                    const date = new Date(meta.timestamp);
                    if (!Number.isNaN(date)) {
                        let label = date.toLocaleTimeString([], {
                            hour: '2-digit',
                            minute: '2-digit',
                        });
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
            return formatDuration(totalSecs);
        }

        /**
         * Spawn a new peer chat window pinned to a fresh session. Different
         * from `createNewSession`, which swaps the *current* window's
         * session in place.
         */
    };
}
