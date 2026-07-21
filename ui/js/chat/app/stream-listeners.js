export function createStreamListenersMixin(dependencies) {
    const { EVT, errLabel, t, submitSelection, processToolCallUpdate, drawContextRing } =
        dependencies;
    return class {
        setupStreamingListeners() {
            // Track focus for notification suppression
            this._windowFocused = false; // chat starts hidden
            this.appWindow.listen('tauri://focus', () => {
                this._windowFocused = true;
            });
            this.appWindow.listen('tauri://blur', () => {
                this._windowFocused = false;
            });

            this.listen(EVT.MESSAGE_CHUNK, (event) => {
                // Chunks are broadcast for EVERY session. Ours renders live
                // through the stream controller; other sessions (or all of
                // them, when this window isn't viewing any) just tick the
                // registry so the sidebar shows a live-activity badge.
                const sid = event?.payload?.sessionId || null;
                if (sid && sid !== this.activeSessionId) {
                    this.streamRegistry.noteChunk(sid);
                    return;
                }
                // First chunk after a mid-stream attach: the snapshot we
                // seeded from the backend accumulator runs AHEAD of the UI
                // chunk stream by up to one batcher flush (~16ms of text) —
                // accumulate happens before the flush emit. Trim the
                // duplicated prefix. Bounded search: the overlap can't
                // exceed one flush of text; 4KB is generous.
                if (this._trimNextChunkOverlap && event?.payload?.text) {
                    this._trimNextChunkOverlap = false;
                    const snap = this.currentStreamingContent || '';
                    const text = event.payload.text;
                    const maxK = Math.min(snap.length, text.length, 4096);
                    for (let k = maxK; k > 0; k--) {
                        if (snap.endsWith(text.slice(0, k))) {
                            event = {
                                ...event,
                                payload: { ...event.payload, text: text.slice(k) },
                            };
                            break;
                        }
                    }
                }
                this.handleMessageChunk(event);
            });
            this.listen(EVT.SESSION_ACTIVITY, (event) => {
                // A user turn started (or failed to start) somewhere — maybe
                // this window, maybe floating, maybe a peer. Track it so the
                // sidebar can badge sessions we're not viewing.
                const { sessionId, kind } = event?.payload || {};
                if (!sessionId) return;
                if (kind === 'failed') {
                    this.streamRegistry.fail(sessionId);
                } else {
                    this.streamRegistry.begin(sessionId);
                }
            });
            this.listen(EVT.MESSAGE_COMPLETE, (event) => {
                // Broadcast event — any chat window pinned to ANY session
                // hears it. Filter by sessionId (the active session, post
                // any in-flight recovery) OR oldSessionId (the session id
                // we issued the send against). Either match means this
                // complete belongs to us; otherwise it's a background
                // session finishing — flip its badge to unread.
                const newId = event?.payload?.sessionId;
                const oldId = event?.payload?.oldSessionId;
                const ours =
                    (newId &&
                        (newId === this.activeSessionId || newId === this.currentAcpSessionId)) ||
                    (oldId &&
                        (oldId === this.activeSessionId || oldId === this.currentAcpSessionId));
                if (!ours) {
                    for (const sid of [newId, oldId]) {
                        if (sid) this.streamRegistry.complete(sid, { viewing: false });
                    }
                    // Refresh the sidebar so the completed session's new
                    // title/timestamp (and unread badge) appear.
                    this.loadSessions();
                    return;
                }

                // Our turn completed — consume the registry entry (no badge).
                for (const sid of [newId, oldId]) {
                    if (sid) this.streamRegistry.complete(sid, { viewing: true });
                }

                // Recovery may have moved us to a fresh session id; pick
                // it up so subsequent sends/cancels target it.
                if (newId && newId !== this.activeSessionId) {
                    console.log('[chat] adopting recovery session id:', newId);
                    this.activeSessionId = newId;
                    this.currentAcpSessionId = newId;
                    this.invoke('set_window_session', {
                        label: this.windowLabel,
                        sessionId: newId,
                    }).catch(() => {});
                }
                this.handleMessageComplete();
            });
            this.listen(EVT.MESSAGE_ERROR, (event) => {
                // MESSAGE_ERROR is emitted only to the originating window and
                // carries no session id — it refers to the turn this window
                // last sent. Terminal for the active session's stream.
                if (this.activeSessionId) this.streamRegistry.fail(this.activeSessionId);
                this.handleMessageError(event);
            });
            this.listen(EVT.TOOL_CALL_UPDATE, (event) => {
                // Tool updates for background sessions feed the registry
                // (chips shown on switch-in); ours go through the controller.
                const sid = event?.payload?.params?.sessionId;
                if (sid && sid !== this.activeSessionId) {
                    this.streamRegistry.trackTool(sid, event, processToolCallUpdate);
                    return;
                }
                this.handleToolCallUpdate(event);
            });
            this.listen(EVT.AGENT_DISCONNECTED, () => {
                // The agent backend's stream closed (process died / connection
                // dropped) while we may have been idle. Reflect it in the header
                // immediately instead of showing "connected" until the next send.
                this.isConnected = false;
                this.updateConnectionStatus();
            });
            this.listen('session_migrated', (event) => {
                // Backend died mid-turn; recovery swapped us to a fresh session and
                // the recovered response streams under the new id shortly. Adopt it
                // without tearing down the waiting UI (unlike session_reset), and
                // drop the accumulated steering-reply text so the resend renders
                // clean. Match either pinned id, mirroring message_complete.
                const oldId = event?.payload?.oldSessionId;
                const newId = event?.payload?.newSessionId;
                // Move the stream's registry entry to the new id — the turn
                // continues there (chunks under newId auto-begin it anyway;
                // this just avoids a stale badge on the dead id).
                if (oldId) this.streamRegistry.fail(oldId);
                if (newId) this.streamRegistry.begin(newId);
                const ours =
                    oldId && (oldId === this.currentAcpSessionId || oldId === this.activeSessionId);
                if (!ours || !newId) return;
                console.log('[chat] session migrated mid-turn:', oldId, '→', newId);
                this.activeSessionId = newId;
                this.currentAcpSessionId = newId;
                // Drop the fresh session's steering-reply text so the resend
                // renders clean. Matches the stream controller's accumulator field.
                this.currentStreamingContent = '';
                this.invoke('set_window_session', {
                    label: this.windowLabel,
                    sessionId: newId,
                }).catch(() => {});
            });
            this.listen('session_reset', (event) => {
                // session_reset is broadcast to all windows; only adopt the
                // new id if our pinned session was the one that died.
                const oldId = event?.payload?.oldSessionId;
                const newId = event?.payload?.newSessionId;
                // The dead session's stream is over regardless of whose it was.
                if (oldId) this.streamRegistry.fail(oldId);
                const ours = oldId && oldId === this.currentAcpSessionId;
                if (!ours) return;
                if (newId) {
                    this.activeSessionId = newId;
                    this.currentAcpSessionId = newId;
                    this.invoke('set_window_session', {
                        label: this.windowLabel,
                        sessionId: newId,
                    }).catch(() => {});
                }
                this.handleSessionReset(event);
            });

            // Refresh session list when the backend detects directory changes
            this.listen('sessions_changed', () => this.loadSessions(true));

            // Cross-window session lifecycle. Backend emits this on rename
            // or delete; we refresh the sidebar always, and react to the
            // pinned session specifically.
            this.listen('session_changed', async (event) => {
                const { id, kind, title, source } = event.payload || {};
                if (!id) return;
                // Re-fetch the session list so renames/deletions show. We
                // pass the affected id so renderSessionList can flag the
                // sidebar entry for an animation when source is "ai".
                await this.loadSessions(true);
                if (source === 'ai') {
                    this._flashAiTitleInSidebar(id);
                }
                const isOurs = id === this.activeSessionId || id === this.currentAcpSessionId;
                if (!isOurs) return;
                if (kind === 'renamed' && title) {
                    animateTitleSwap(this.elements.chatHeaderTitle, title, source === 'ai');
                } else if (kind === 'deleted') {
                    this.activeSessionId = null;
                    this.currentAcpSessionId = null;
                    this.elements.messagesArea.innerHTML = `<div class="message-placeholder">${t('chat.placeholder.deleted')}</div>`;
                    this.elements.chatHeaderTitle.textContent = 'Kage';
                }
            });

            // When a message is sent from the floating window, mirror it in the chat
            this.listen('floating_message_sent', (event) => {
                const { message } = event.payload || {};
                if (!message) return;
                // Only mirror if we're actually viewing the floating/default
                // session. Require a real id — null === null must NOT match.
                if (!this.activeSessionId || !this.floatingSessionId) return;
                const isDefaultSession =
                    this.activeSessionId === this.floatingSessionId ||
                    this.activeSessionId === this.currentAcpSessionId;
                if (!isDefaultSession) return;
                this.addUserMessage(message);
                this.startStreaming();
            });

            // Real-time context usage from ACP metadata notifications.
            // `contextUsagePercentage` is already in percentage form (0..100)
            // — agents using either vendor namespace send it that way. The
            // values are floats with many decimals (e.g. 0.9581 means
            // 0.96%, not 96%); an earlier "scale up if ≤1" guess turned a
            // barely-touched session into 96% and triggered an
            // auto-compact loop.
            this.listen('context_metadata', (event) => {
                const raw = event.payload?.params?.contextUsagePercentage;
                if (raw == null || !Number.isFinite(raw) || raw < 0) return;
                const rounded = Math.round(raw);
                this.elements.contextPercent.textContent = rounded + '%';
                document.getElementById('contextIndicator').title = rounded + '% context used';
                this.drawContextRing(rounded);
                this.maybeAutoCompact(rounded);
            });

            // Compaction status from ACP notifications (works for both auto and manual /compact)
            this.listen(EVT.COMPACTION_STATUS, (event) => {
                const status = event.payload?.params?.status?.type;
                if (status === 'started') {
                    this.showCompactingNotice();
                } else if (status === 'completed') {
                    this.hideCompactingNotice(t('chat.compaction.completed'));
                    // Compaction is fully done — release the auto-compact
                    // gate so the next *changed* metric can trigger another
                    // round if needed. Set after hideCompactingNotice so the
                    // UI settles before we'd accept another trigger.
                    this._isCompacting = false;
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
            document.addEventListener('kage-show-response', (e) => {
                if (e.detail) {
                    this.addMessageFromHistory('assistant', e.detail);
                    this.scrollToBottom();
                }
            });

            // Prompt-dispatch slash commands (standard ACP, e.g. Claude): send the
            // slash text as a normal message so the agent interprets it and streams
            // the answer back through the usual pipeline.
            document.addEventListener('kage-send-prompt', (e) => {
                const text = e.detail?.text;
                if (!text) return;
                this.elements.chatInput.value = text;
                this.sendMessage();
            });

            document.addEventListener('kage-show-selection', (e) => {
                const { command, options } = e.detail;
                if (!options || options.length === 0) return;
                // Render an inline picker in the transcript. Uses the shared
                // `.slash-selection` component (see shared-components.css), NOT the
                // old red reconnect-error buttons. Submit goes through the shared
                // `submitSelection` so the arg-shape ({<cmd>Name: value}) matches
                // what the agent actually accepts — `{input: value}` silently
                // no-ops on the agent side.
                const placeholder =
                    this.elements.messagesArea.querySelector('.message-placeholder');
                if (placeholder) placeholder.remove();

                const container = document.createElement('div');
                container.className = 'slash-selection';

                const heading = document.createElement('div');
                heading.className = 'slash-selection-heading';
                heading.textContent = t('command.selection.heading', { command: '/' + command });
                container.appendChild(heading);

                options.forEach((opt) => {
                    const item = document.createElement('button');
                    item.className = 'slash-selection-item' + (opt.current ? ' current' : '');
                    item.innerHTML = `
                    <span class="slash-selection-marker">${opt.current ? '●' : '○'}</span>
                    <span class="slash-selection-label"></span>
                    ${opt.description ? '<span class="slash-selection-desc"></span>' : ''}
                `;
                    item.querySelector('.slash-selection-label').textContent = opt.label;
                    if (opt.description) {
                        item.querySelector('.slash-selection-desc').textContent = opt.description;
                    }
                    item.addEventListener('click', async () => {
                        try {
                            const msg = await submitSelection(
                                this.invoke,
                                this.activeSessionId,
                                command,
                                opt.value
                            );
                            container.remove();
                            this.addMessageFromHistory(
                                'assistant',
                                msg || t('chat.command.result_done')
                            );
                            this.scrollToBottom();
                        } catch (err) {
                            this.showError(errLabel(t('chat.error.command_failed'), err));
                        }
                    });
                    container.appendChild(item);
                });
                this.elements.messagesArea.appendChild(container);
                this.scrollToBottom();
            });
        }

        // --- Session Management ---
    };
}
