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

        async createNewSession() {
            // Supersede any in-flight selectSession / earlier New Chat so its
            // late IPC can't paint or adopt over this one.
            const gen = this._nextSelectGen();
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
            // session's chunks route to the registry instead of this view.
            // Null the pin too: MESSAGE_COMPLETE treats a `currentAcpSessionId`
            // match as ours and would re-adopt the old id mid-switch.
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
            this.elements.chatHeaderTitle.textContent = t('chat.header.default_title');

            try {
                const newId = await this.invoke('switch_acp_session', { sessionId: null });
                if (gen !== this._selectGen) {
                    // A newer selection/New Chat owns the view; undo our pin.
                    this._repinWindowSession();
                    return;
                }
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
            } catch (error) {
                console.error('Failed to create new session:', error);
                if (gen !== this._selectGen) return;
                // The backend is still on the previous session; keep sends
                // targeting it as before.
                this.activeSessionId = prevSessionId;
                this.currentAcpSessionId = prevAcpSessionId;
                if (window.ChatPermissions) {
                    window.ChatPermissions.onSessionSwitch(prevSessionId);
                }
                enableInput();
                this.renderSessionList();
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

            try {
                await this.invoke('delete_session', { sessionId });
                this.sessions = this.sessions.filter((s) => s.session_id !== sessionId);

                if (isActive) {
                    // Clear the display
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
