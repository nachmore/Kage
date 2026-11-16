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
            // The answer is the window's only when nothing changed the view
            // or the pin while it was in flight: not while a view change
            // (a selection, New Chat, the bootstrap, a paint) is under way
            // or happened since (it owns `currentAcpSessionId`), and not
            // when a switch or re-pin overlapped the read (the pin may
            // still be the old session, and adopting it would let that
            // session's turns claim the view). Nor while the window shows
            // a session whose switch was refused (_onSessionSwitchFailed):
            // it has no session of its own then, and the pin (the session
            // it had) is not the one on screen, so a turn finishing there
            // must stay a background one.
            const gen = this._viewGen;
            const pinGen = this._pinGen;
            const clear = this._pinsSettled() && !this._viewChangePending();
            const raced = () =>
                !clear ||
                !this._viewIs(gen) ||
                pinGen !== this._pinGen ||
                (!!this.activeSessionId && this.activeSessionId === this._setupFailedSessionId);
            try {
                const pinned = await this.invoke('get_window_session', {
                    label: this.windowLabel,
                });
                if (raced()) return;
                this.currentAcpSessionId = pinned;
            } catch (e) {
                console.error('Failed to get current session ID:', e);
                if (raced()) return;
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
        async _bootstrapChatPeerSession({ throwOnError = false } = {}) {
            if (this.currentAcpSessionId) return null; // already bootstrapped
            // A selection or New Chat under way (clicked during init) will
            // adopt its own session; bootstrapping here would race it.
            if (this._viewChangePending()) return null;
            // What the window shows now: if this changes while the switch
            // below is in flight, the user moved and their choice wins.
            const startedOn = this.currentAcpSessionId ?? this.activeSessionId ?? null;
            // A view change of its own: whatever changes the view after it
            // (a selection, New Chat, open in chat) supersedes it.
            const gen = this._beginViewChange();
            try {
                return await this._runBootstrap(gen, startedOn, throwOnError);
            } finally {
                this._endViewChange(gen);
            }
        }

        /** _bootstrapChatPeerSession's switch, under the view change `gen`. */
        async _runBootstrap(gen, startedOn, throwOnError) {
            // A signed-out agent refused the session: say so, with Sign in,
            // which bootstraps again after it and then draws the session as
            // init does (sidebar entry, transcript, header; init's own
            // auto-select ran long ago, with no session). Any other failure
            // stays in the console, as before. A failure after the window
            // moved on is not its to show.
            const failed = (e) => {
                if (!this._viewIs(gen)) {
                    console.log('[CHAT] bootstrap failed after the window moved on');
                    return;
                }
                this._showSessionSetupError?.(this.formatError?.(e) ?? String(e), e, {
                    sessionId: this.activeSessionId ?? null,
                    retry: async () => {
                        const adopted = await this._bootstrapChatPeerSession({
                            throwOnError: true,
                        });
                        // Draw only the session this bootstrap adopted: if the
                        // user picked one meanwhile, that one stays shown.
                        if (adopted) {
                            await this._showCurrentSession();
                            await this.checkConnection();
                        }
                        return this.currentAcpSessionId ?? null;
                    },
                    onlyIfSignIn: true,
                });
                if (throwOnError) throw e;
            };

            const isPeer = isChatLabel(this.windowLabel);
            if (isPeer) {
                const params = new URLSearchParams(window.location.search);
                const resumeId = params.get('resumeSessionId');
                try {
                    const adoptedId = await this._switchAcpSession(resumeId || null);
                    if (this._movedDuringBootstrap(startedOn, gen, adoptedId)) return null;
                    this.currentAcpSessionId = adoptedId;
                    this.activeSessionId = adoptedId;
                    console.log(`[CHAT] Bootstrapped ${this.windowLabel} -> ${adoptedId}`);
                    return adoptedId;
                } catch (e) {
                    console.error('[CHAT] Failed to bootstrap peer:', e);
                    failed(e);
                }
                return null;
            }

            // main — adopt floating's session so the user sees their
            // ongoing conversation. switch_acp_session sends session/load
            // and pins to this window's label (`main`).
            const floatingId = await getWindowSessionOrNull(this.invoke, WINDOW.FLOATING);
            if (!this._viewIs(gen)) return null;
            if (!floatingId) {
                console.log('[CHAT] main bootstrap: no floating session yet, leaving empty');
                return null;
            }
            try {
                const adoptedId = await this._switchAcpSession(floatingId);
                if (this._movedDuringBootstrap(startedOn, gen, adoptedId)) return null;
                this.currentAcpSessionId = adoptedId;
                this.activeSessionId = adoptedId;
                console.log(`[CHAT] main adopted floating's session: ${adoptedId}`);
                return adoptedId;
            } catch (e) {
                console.error('[CHAT] Failed to adopt floating session:', e);
                failed(e);
            }
            return null;
        }

        /**
         * The view changed (a selection, New Chat, open in chat) while a
         * bootstrap's switch_acp_session was in flight (the view change
         * `gen`): keep what the window shows, and pin it back to that, since
         * the bootstrap's switch (to `landedId`) may have landed on the
         * backend after theirs.
         */
        _movedDuringBootstrap(startedOn, gen, landedId) {
            const now = this.currentAcpSessionId ?? this.activeSessionId ?? null;
            if (this._viewIs(gen) && now === startedOn) return false;
            console.log('[CHAT] bootstrap finished after the window moved to', now);
            this._supersededPin(landedId);
            return true;
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
                        // Replacer function: a string replacement would expand
                        // `$&`, `$$`, etc. inside the agent's response text.
                        const prompt = action.prompt.replace(/\{text\}/g, () => responseText);
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
            // A list asked for before a view change (a selection, New Chat,
            // a turn starting) is not applied over it: the window may have
            // listed its new session since. Ask again instead.
            const gen = this._viewGen;
            try {
                const params = loadAll ? { force: true } : { limit: 50, offset: 0 };
                const sessions = await this.invoke('list_sessions', params);
                if (!this._viewIs(gen)) return this.loadSessions(loadAll);
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
                if (!this._viewIs(gen)) return;
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
