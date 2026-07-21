export function createSessionStateMixin(dependencies) {
    const { WINDOW, isChatLabel, getWindowSessionOrNull, t, getActionsForText, getConfig } =
        dependencies;
    return class {
        async loadFloatingSessionId() {
            try {
                this.floatingSessionId = await this.invoke('get_window_session', {
                    label: WINDOW.FLOATING,
                });
            } catch (e) {
                console.error('Failed to get floating session ID:', e);
                this.floatingSessionId = null;
            }
        }

        async loadCurrentSessionId() {
            // Read this window's own pinned session id. For `main` that's
            // bootstrapped at app launch. For `chat-<uuid>` peers it's
            // bootstrapped on first load via _bootstrapChatPeerSession.
            try {
                this.currentAcpSessionId = await this.invoke('get_window_session', {
                    label: this.windowLabel,
                });
            } catch (e) {
                console.error('Failed to get current session ID:', e);
                this.currentAcpSessionId = null;
            }
        }

        /**
         * Bootstrap this chat window's session on first load.
         *
         * - For `chat-<uuid>` peers (Ctrl+Shift+N "new chat window"
         *   intent): read `?resumeSessionId=<id>` from the URL — if
         *   present, load that session via `switch_acp_session`; otherwise
         *   create a fresh one.
         *
         * - For `main`: never eagerly create. Default to floating's
         *   pinned session so the user immediately sees the conversation
         *   they were having. If floating doesn't have one yet (e.g.
         *   `start_session_on_launch=false`), leave the chat empty —
         *   the user picks a session from the sidebar or hits "New Chat"
         *   to create one explicitly. This avoids the "spawn 3 sessions
         *   on launch" race the eager-create version produced.
         */
        async _bootstrapChatPeerSession() {
            if (this.currentAcpSessionId) return; // already bootstrapped

            const isPeer = isChatLabel(this.windowLabel);
            if (isPeer) {
                const params = new URLSearchParams(window.location.search);
                const resumeId = params.get('resumeSessionId');
                try {
                    const adoptedId = await this.invoke('switch_acp_session', {
                        sessionId: resumeId || null,
                    });
                    this.currentAcpSessionId = adoptedId;
                    this.activeSessionId = adoptedId;
                    console.log(`[CHAT] Bootstrapped ${this.windowLabel} -> ${adoptedId}`);
                } catch (e) {
                    console.error('[CHAT] Failed to bootstrap peer:', e);
                }
                return;
            }

            // main — adopt floating's session so the user sees their
            // ongoing conversation. switch_acp_session sends session/load
            // and pins to this window's label (`main`).
            const floatingId = await getWindowSessionOrNull(this.invoke, WINDOW.FLOATING);
            if (!floatingId) {
                console.log('[CHAT] main bootstrap: no floating session yet, leaving empty');
                return;
            }
            try {
                const adoptedId = await this.invoke('switch_acp_session', {
                    sessionId: floatingId,
                });
                this.currentAcpSessionId = adoptedId;
                this.activeSessionId = adoptedId;
                console.log(`[CHAT] main adopted floating's session: ${adoptedId}`);
            } catch (e) {
                console.error('[CHAT] Failed to adopt floating session:', e);
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
                const config = await getConfig(this.invoke);
                this._showSpeakBtn =
                    config.ui?.show_speech_button === true || config.pocket_tts?.enabled === true;
                this._showTranslateBtn = !!config.quick_actions?.translate_language;
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
            const lastMsg = [...this.messages].reverse().find((m) => m.role === 'assistant');
            const responseText = lastMsg?.content || '';

            try {
                const config = await getConfig(this.invoke);
                if (!config.ui?.show_response_actions) return;
                const qaConfig = config.quick_actions || { enabled: true, custom_actions: [] };
                const actions = await getActionsForText(responseText || 'general text', qaConfig);
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
            } catch (e) {
                console.warn('Suggestion chips error:', e);
            }
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
                // On initial load, mark all sessions as seen.
                // On subsequent refreshes, new IDs stay unseen until clicked.
                if (this._seenSessionIds.size === 0) {
                    for (const s of this.sessions) this._seenSessionIds.add(s.session_id);
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

            // Show loading dots at the bottom of the list
            const list = this.elements.sessionList;
            let loader = list.querySelector('.session-list-loader');
            if (!loader) {
                loader = document.createElement('div');
                loader.className = 'session-list-loader';
                loader.innerHTML =
                    '<div class="loading-dot"></div><div class="loading-dot"></div><div class="loading-dot"></div>';
                list.appendChild(loader);
            }

            try {
                const more = await this.invoke('list_sessions', {
                    limit: 50,
                    offset: this.sessions.length,
                });
                if (more.length > 0) {
                    // Mark loaded sessions as seen (they're not new — just paginated in)
                    for (const s of more) this._seenSessionIds.add(s.session_id);
                    this.sessions = this.sessions.concat(more);
                    this.renderSessionList();
                }
                if (more.length < 50) this._sessionsFullyLoaded = true;
            } catch (e) {
                console.error('Failed to load more sessions:', e);
            } finally {
                this._loadingMore = false;
                list.querySelector('.session-list-loader')?.remove();
            }
        }

        /**
         * Briefly flash the sidebar entry for the session whose AI title
         * just arrived. Called from the session_changed listener after
         * loadSessions has rebuilt the list. The CSS class triggers a
         * background-tinted fade matching the chat header animation.
         */
        _flashAiTitleInSidebar(sessionId) {
            if (!this.elements.sessionList) return;
            const item = this.elements.sessionList.querySelector(
                `.session-item[data-session-id="${CSS.escape(sessionId)}"] .session-item-title`
            );
            if (!item) return;
            item.classList.add('kd-title-flash');
            setTimeout(() => item.classList.remove('kd-title-flash'), 700);
        }
    };
}
