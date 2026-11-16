export function createSessionActionsMixin(dependencies) {
    const { errLabel, t } = dependencies;
    return class {
        async openNewChatWindow() {
            try {
                await this.invoke('open_new_chat_window', { resumeSessionId: null });
            } catch (e) {
                console.error('Failed to open new chat window:', e);
            }
        }

        /**
         * New Chat. Resolves to the new session's id (null when it failed
         * or a newer selection took over); `throwOnError` (the sign-in's
         * retry) rejects with the failure instead.
         */
        async createNewSession({ throwOnError = false } = {}) {
            // A view change: supersedes whatever is in flight for the old
            // view (a selection, an earlier New Chat, a paint, a pin read)
            // so its late IPC can't paint or adopt over this one, and is
            // superseded in turn by the next one (open in chat included).
            const gen = this._beginViewChange();
            this._cancelDisplaySessionRender();

            // Detach from an in-flight turn exactly like selectSession does:
            // the old turn keeps streaming in the background, but its
            // chunks/complete stop matching this view, so nothing would ever
            // clear the waiting state (input disabled, Stop button stuck).
            this.hideTypingIndicator();
            this.currentStreamingMessage = null;
            this.currentStreamingContent = '';
            this.isWaitingForResponse = false;
            this.updateInputState();

            // Drop the old id while the switch is in flight so the old
            // session's chunks route to the registry instead of this view,
            // and the pin: it is the old session's until the switch lands.
            const prevSessionId = this.activeSessionId;
            const prevAcpSessionId = this.currentAcpSessionId;
            this.activeSessionId = null;
            this.currentAcpSessionId = null;
            if (window.ChatPermissions) {
                window.ChatPermissions.onSessionSwitch(null);
            }
            // No session to send to until the switch lands.
            this.elements.chatInput.disabled = true;
            this.elements.chatInput.placeholder = t('chat.placeholder.connecting');
            this.elements.sendBtn.disabled = true;
            const enableInput = () => {
                this.elements.chatInput.disabled = false;
                this.elements.chatInput.placeholder = t('chat.placeholder.type_message');
                this.elements.sendBtn.disabled = false;
                this.elements.chatInput.focus();
            };

            this.messages = [];
            this.toolSources = [];
            this.toolUsages = [];
            this._toolCallIds = new Set();
            this._sourceDomains = new Set();
            this.elements.messagesArea.innerHTML = `<div class="message-placeholder">${t('chat.placeholder.start_conversation')}</div>`;
            this._transcriptOf = null;
            this.elements.chatHeaderTitle.textContent = t('chat.header.default_title');

            // The backend pin still names the old session until the switch
            // lands. While the view change is pending the focus refresh
            // neither re-reads that pin nor auto-selects from the null
            // `activeSessionId` (either would undo this New Chat), and a
            // pin read that resolves after it is discarded (_pinChange).
            try {
                const newId = await this._switchAcpSession(null);
                if (!this._viewIs(gen)) {
                    // A newer view change owns the window; undo our pin.
                    this._supersededPin(newId);
                    return null;
                }
                this._endViewChange(gen);
                enableInput();
                this.activeSessionId = newId;
                this.currentAcpSessionId = newId;
                this._seenSessionIds.add(newId);
                // Add the new session to the list so it appears immediately
                if (!this.sessions.find((s) => s.session_id === newId)) {
                    this.sessions.push({
                        session_id: newId,
                        title: 'New Chat',
                        created_at: new Date().toISOString(),
                        updated_at: new Date().toISOString(),
                    });
                }
                this.renderSessionList();
                console.log('Created new ACP session:', newId);

                // Send steering for the new session (fire and forget)
                try {
                    await this.invoke('send_steering_message', { sessionId: newId });
                } catch (e) {
                    console.log('Steering message not sent (may be disabled):', e);
                }
                return newId;
            } catch (error) {
                console.error('Failed to create new session:', error);
                if (!this._viewIs(gen)) return null;
                this._endViewChange(gen);
                // The backend is still on the previous session; keep sends
                // targeting it as before.
                this.activeSessionId = prevSessionId;
                this.currentAcpSessionId = prevAcpSessionId;
                if (window.ChatPermissions) {
                    window.ChatPermissions.onSessionSwitch(prevSessionId);
                }
                enableInput();
                this.renderSessionList();
                // A signed-out agent: say so, with Sign in, which makes the
                // new chat after it. Any other failure stays quiet as before.
                this._showSessionSetupError?.(this.formatError(error), error, {
                    sessionId: prevSessionId ?? null,
                    retry: () => this.createNewSession({ throwOnError: true }),
                    onlyIfSignIn: true,
                });
                if (throwOnError) throw error;
                return null;
            } finally {
                this._endViewChange(gen);
            }
        }

        async deleteSession(sessionId, title) {
            const isActive = sessionId === this.activeSessionId;
            const isCurrent =
                sessionId === this.currentAcpSessionId || sessionId === this.floatingSessionId;

            if (isCurrent) {
                this.showError(t('chat.delete.cannot_active'));
                return;
            }

            let dir = '';
            try {
                dir = await this.invoke('get_sessions_directory');
            } catch {
                /* ignore */
            }

            const msg = t('chat.delete.confirm', {
                title,
                dir: dir || t('chat.delete.dir_fallback'),
                sessionId,
            });
            if (!confirm(msg)) return;

            const gen = this._viewGen;
            try {
                await this.invoke('delete_session', { sessionId });
                this.sessions = this.sessions.filter((s) => s.session_id !== sessionId);

                // Clear the display, unless the window moved on meanwhile.
                if (isActive && this._viewIs(gen) && this.activeSessionId === sessionId) {
                    this._bumpViewGen();
                    this.activeSessionId = null;
                    this.elements.messagesArea.innerHTML = `<div class="message-placeholder">${t('chat.placeholder.select_session')}</div>`;
                    this.elements.chatHeaderTitle.textContent = 'Kage';
                }

                this.renderSessionList();
            } catch (e) {
                this.showError(errLabel(t('chat.error.failed_delete_session'), e));
            }
        }

        // --- Speech ---
    };
}
