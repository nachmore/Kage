export function createComposerMixin(dependencies) {
    const {
        clearFinishedHostCommandCards,
        renderMarkdown,
        attachmentPreviewHtml,
        executeCommand,
        getSlashCommandMeta,
        getSlashCommandDispatch,
        errLabel,
        t,
        loadSelection,
        SpeechController,
        trackEvent,
        messageLengthBucket,
        newTurnId,
        errorSessionId,
        errorTurnId,
    } = dependencies;
    return class {
        setupSpeech() {
            this.speech = new SpeechController({
                invoke: this.invoke,
                elements: {
                    input: this.elements.chatInput,
                    speechBtn: this.elements.chatSpeechBtn,
                    speechWave: this.elements.chatSpeechWave,
                },
                onSend: (text) => {
                    this.elements.chatInput.value = text;
                    this.sendMessage();
                },
                onVisibilityUpdate: () => {},
                barContainer:
                    document.querySelector('.chat-input-container') ||
                    document.querySelector('.chat-input'),
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
            if ((!message && !hasAttachments && !hasPendingFiles) || this.isWaitingForResponse)
                return;
            // A view change is still under way (a selection, New Chat, the
            // current session's paint): its transcript would wipe this turn.
            // The text stays in the input.
            if (this._viewChangePending()) return;

            // If a plan is pending review, send the message as a revision request
            if (this._pendingPlanRevision && message) {
                this.automationPlanController.reset();
                this.extensionToolController.reset();
                this.elements.chatInput.value = '';
                this.elements.chatInput.style.height = 'auto';
                this.clearSuggestions();

                // A turn starting in the window (see _startTurnView).
                this._startTurnView();
                // Add user message to chat
                const userMsg = this.createMessageElement('user', message);
                this.elements.messagesArea.appendChild(userMsg);
                this.messages.push({ role: 'user', content: message });

                // Set up for new streaming response
                clearFinishedHostCommandCards?.();
                this.currentStreamingContent = '';
                this.toolSources = [];
                this.toolUsages = [];
                this._toolCallIds = new Set();
                this._sourceDomains = new Set();
                this.isWaitingForResponse = true;
                this._turnOriginatedHere = true; // plan revision is a chat-originated turn
                this._streamStartTime = Date.now();
                this.updateInputState();
                this.showTypingIndicator();
                this.currentStreamingMessage = this.createMessageElement('assistant', '');
                this.elements.messagesArea.appendChild(this.currentStreamingMessage);
                this._autoScrollEnabled = true;
                this.scrollToBottom(true);

                try {
                    trackEvent('message_sent', {
                        source: 'chat',
                        length: messageLengthBucket(message),
                    });
                    const turnId = this._recordTurn(this.activeSessionId, message, null);
                    await this.invoke('send_message_streaming', {
                        sessionId: this.activeSessionId,
                        message,
                        attachments: null,
                        turnId,
                    });
                } catch (e) {
                    this.handleMessageError({ payload: errLabel(t('chat.error.error_label'), e) });
                }
                return;
            }

            // Clear suggestions
            this.clearSuggestions();
            this.hideSuggestionChips();

            // Read pending file contents and prepend to message. The reads
            // are async: if the view changes meanwhile (the user picks
            // another session), nothing is sent there and the draft and
            // files stay for the user to send again.
            if (hasPendingFiles) {
                const readGen = this._viewGen;
                const files = this._pendingFiles;
                const fileParts = [];
                for (const file of files) {
                    try {
                        const text = await file.text();
                        const truncated =
                            text.length > 100000
                                ? text.substring(0, 100000) + '\n\n[...truncated at 100k chars]'
                                : text;
                        fileParts.push(
                            `Contents of \`${file.name}\`:\n\`\`\`\n${truncated}\n\`\`\``
                        );
                    } catch (e) {
                        fileParts.push(`Could not read \`${file.name}\`: ${e.message}`);
                    }
                }
                if (!this._viewIs(readGen) || this._viewChangePending()) {
                    console.log('[chat] the view changed while files were read; not sending');
                    return;
                }
                this._pendingFiles = [];
                const fileBlock = fileParts.join('\n\n');
                message = message ? fileBlock + '\n\n' + message : fileBlock;
            }

            const attachments = this.attachmentManager.toContentBlocks();
            const attachmentSnapshots = hasAttachments
                ? [...this.attachmentManager.attachments]
                : null;
            this.attachmentManager.clear();

            this.elements.chatInput.value = '';
            this.elements.chatInput.style.height = 'auto';

            // Handle > local commands
            if (!hasAttachments && message.startsWith('>')) {
                const cmdText = message.substring(1).trim();
                if (cmdText && (await executeCommand(cmdText, this.invoke, this.appWindow))) {
                    return;
                }
            }

            // Handle / slash commands (only if no attachments). Prompt-dispatch
            // commands (standard ACP, e.g. Claude) are NOT intercepted — the agent
            // interprets the slash text itself, so they fall through to the normal
            // streaming send below.
            if (
                !hasAttachments &&
                message.startsWith('/') &&
                getSlashCommandDispatch(message.split(' ')[0]) !== 'prompt'
            ) {
                try {
                    const parts = message.split(' ');
                    const cmdName = parts[0].substring(1); // strip leading /
                    const rest = parts.slice(1).join(' ').trim();

                    // Bare selection-type command (e.g. just "/agent"): render the
                    // inline picker instead of dumping the agent's pre-formatted
                    // list text. The kage-show-selection handler paints the picker
                    // and routes the submit through the correct arg-shape.
                    if (!rest && getSlashCommandMeta(cmdName)?.inputType === 'selection') {
                        this.addUserMessage(message);
                        const res = await loadSelection(this.invoke, this.activeSessionId, cmdName);
                        if (res.kind === 'options') {
                            document.dispatchEvent(
                                new CustomEvent('kage-show-selection', {
                                    detail: { command: cmdName, options: res.options },
                                })
                            );
                        } else {
                            this.addMessageFromHistory(
                                'assistant',
                                res.text || t('command.slash.no_options')
                            );
                        }
                        this.scrollToBottom();
                        return;
                    }

                    // Everything else: execute with any trailing args as `input`
                    // (panel commands and selection subcommands like
                    // "/agent swap foo" accept this) and show the reply.
                    const cmdArgs = rest ? { input: rest } : {};
                    const result = await this.invoke('execute_slash_command', {
                        sessionId: this.activeSessionId,
                        command: cmdName,
                        args: cmdArgs,
                    });
                    // Show the command and result in the chat (suppress compact — handled by compaction_status)
                    this.addUserMessage(message);
                    if (cmdName !== 'compact') {
                        // Prefer agent-prettified markdown (displayMessage) from
                        // the Rust slash_format layer; fall back to plain message.
                        const resultText =
                            result?.displayMessage ||
                            result?.message ||
                            JSON.stringify(result, null, 2);
                        this.addMessageFromHistory('assistant', resultText);
                    }
                    this.scrollToBottom();
                    return;
                } catch (e) {
                    console.error('Slash command failed:', e);
                    this.addUserMessage(message);
                    this.addMessageFromHistory(
                        'assistant',
                        errLabel(t('chat.error.command_failed'), e)
                    );
                    this.scrollToBottom();
                    return;
                }
            }

            this.addUserMessage(message, attachmentSnapshots);
            this.startStreaming();
            // A finished sign-in's status goes with the new turn.
            this._clearAgentSignInSlot?.();

            try {
                trackEvent('message_sent', {
                    source: 'chat',
                    length: messageLengthBucket(message),
                    attachments: attachments?.length || 0,
                });
                const turnId = this._recordTurn(this.activeSessionId, message, attachments);
                await this.invoke('send_message_streaming', {
                    sessionId: this.activeSessionId,
                    message,
                    attachments,
                    turnId,
                });
                this.isConnected = true;
                this.updateConnectionStatus();
            } catch (error) {
                this._abortStreamingSend(error);
            }
        }

        /**
         * Remember the prompt going out, for Reconnect to resend if it fails
         * before any output. `seq` ties it to this send: a newer one
         * replaces it. Returns the turn's id, which goes with the send
         * (`turnId`) and comes back on its MESSAGE_ERROR: an error is this
         * turn's only when it names this id. `turnId` given: a turn the
         * backend started and named (open_chat_with_message); `null`: one
         * it named nothing, whose errors are then never this turn's.
         */
        _recordTurn(sessionId, message, attachments, turnId = newTurnId()) {
            this._sendSeq = (this._sendSeq || 0) + 1;
            this._lastTurn = {
                sessionId,
                // Every id the turn's session had (a migration adds one):
                // its error may name any of them.
                sessionIds: sessionId ? [sessionId] : [],
                message,
                attachments,
                seq: this._sendSeq,
                turnId,
            };
            // The latest turn this window sent on each session: after the
            // user switches back to a session, the stream shown there is
            // that turn's (see _errorEndsShownStream).
            if (sessionId) {
                if (!this._turnOnSession) this._turnOnSession = new Map();
                this._turnOnSession.set(sessionId, turnId);
            }
            return turnId;
        }

        /**
         * The window adopted `sessionId` for a turn it recorded with no
         * session (open_chat_with_message created the session because the
         * window had none): that turn is now on it, so its error can end
         * the stream shown there (see _errorEndsShownStream).
         */
        _adoptTurnSession(sessionId) {
            const turn = this._lastTurn;
            if (!sessionId || !turn || turn.sessionId) return;
            this._lastTurn = {
                ...turn,
                sessionId,
                sessionIds: [...(turn.sessionIds || []), sessionId],
            };
            if (turn.turnId) {
                if (!this._turnOnSession) this._turnOnSession = new Map();
                if (!this._turnOnSession.has(sessionId))
                    this._turnOnSession.set(sessionId, turn.turnId);
            }
        }

        /**
         * Recovery moved the session `oldId` to `newId`: so did the last
         * turn, if it was on it.
         */
        _followTurnSession(oldId, newId) {
            if (oldId && newId && this._turnOnSession?.has(oldId)) {
                if (!this._turnOnSession.has(newId)) {
                    this._turnOnSession.set(newId, this._turnOnSession.get(oldId));
                }
            }
            const turn = this._lastTurn;
            if (oldId && newId && turn?.sessionId === oldId) {
                this._lastTurn = {
                    ...turn,
                    sessionId: newId,
                    sessionIds: [...(turn.sessionIds || []), newId],
                };
            }
        }

        /** Whether `turn` ran on `sessionId` (under any id it had). */
        _turnWasOn(turn, sessionId) {
            return !!sessionId && !!turn?.sessionIds?.includes(sessionId);
        }

        /**
         * Whether `id` (a message_complete or session_migrated id) names the
         * session this window shows: it is `activeSessionId`, or the turn
         * still running that the window last sent is now on the session
         * shown and earlier ran under `id` (a migration). Never by the pin
         * (`currentAcpSessionId`), which can name a session the window has
         * left, and never by a turn that has moved on from the session
         * shown or has ended: after a migration the user can select the
         * old id again, and a later turn on the new one is a background one.
         */
        _isShownSessionId(id) {
            const shown = this.activeSessionId;
            if (!id || !shown) return false;
            if (id === shown) return true;
            const turn = this._lastTurn;
            return (
                !!this.isWaitingForResponse &&
                turn?.sessionId === shown &&
                this._turnWasOn(turn, id)
            );
        }

        /**
         * Whether a `message_error` naming `sessionId` is about the turn this
         * window shows: the session shown or pinned, or an earlier id of the
         * shown session's last turn. Otherwise it's a turn on a session the
         * user left, and must not end (or be resent as) the one on screen.
         */
        _errorIsForShownSession(sessionId) {
            if (sessionId === this.activeSessionId || sessionId === this.currentAcpSessionId) {
                return true;
            }
            const turn = this._lastTurn;
            return (
                !!this.activeSessionId &&
                turn?.sessionId === this.activeSessionId &&
                this._turnWasOn(turn, sessionId)
            );
        }

        /**
         * Whether a `message_error` naming a turn other than the window's
         * last one is the error of the stream on screen, so it ends that
         * stream (never with a resend: the turn isn't the last one). That
         * is so when the user sent the last turn on another session and
         * then switched back here, reattaching to a turn still running:
         * the error names the shown session and the latest turn this
         * window sent on it. An earlier turn's late error on that session
         * (stopped, then another prompt went out there) names an older
         * id and leaves the stream alone.
         */
        _errorEndsShownStream(payload) {
            const turnId = errorTurnId(payload);
            const sessionId = errorSessionId(payload);
            if (!turnId || !sessionId || !this.activeSessionId) return false;
            if (sessionId !== this.activeSessionId && sessionId !== this.currentAcpSessionId) {
                return false;
            }
            // The last turn runs on the shown session: the stream there is its.
            if (this._turnWasOn(this._lastTurn, this.activeSessionId)) return false;
            return this._turnOnSession?.get(sessionId) === turnId;
        }

        /**
         * Resend a turn that failed before any output, after Reconnect. The
         * user's bubble is already in the transcript, so none is added, and
         * the composer (draft, attachments) is left alone. A failed send is
         * shown as an error; `throwOnError` (the agent sign-in) also throws.
         */
        async _resendTurn(turn, { throwOnError = false } = {}) {
            this.startStreaming();
            // A turn of its own: the failed one's late errors aren't its.
            const turnId = this._recordTurn(this.activeSessionId, turn.message, turn.attachments);
            try {
                await this.invoke('send_message_streaming', {
                    sessionId: this.activeSessionId,
                    message: turn.message,
                    attachments: turn.attachments,
                    turnId,
                });
                this.isConnected = true;
                this.updateConnectionStatus();
            } catch (error) {
                this._abortStreamingSend(error);
                if (throwOnError) throw error;
            }
        }

        /** A send the backend refused: undo startStreaming and say why. */
        _abortStreamingSend(error) {
            this.hideTypingIndicator();
            if (this.currentStreamingMessage) {
                this.currentStreamingMessage.remove();
                this.currentStreamingMessage = null;
            }
            this.showError(errLabel(t('chat.error.error_label'), error));
            this.isConnected = false;
            this.updateConnectionStatus();
            this.isWaitingForResponse = false;
            this.updateInputState();
        }

        /**
         * Whether a prompt can go to `sessionId` now: it's the session this
         * window shows, its view change (selection, paint) is done, and no
         * turn is in flight on it (here, from another window, or a plan
         * waiting for review).
         */
        canSendAgentPrompt(sessionId) {
            return (
                !!sessionId &&
                sessionId === this.activeSessionId &&
                !this._viewChangePending() &&
                !this.isWaitingForResponse &&
                !this._pendingPlanRevision &&
                !this.streamRegistry?.isStreaming(sessionId)
            );
        }

        /**
         * Send `message` as the user's next prompt, shown in the chat like
         * one they typed, without touching the composer (its draft and
         * attachments stay) or the slash/`>` command handling. Hands the
         * result of a command the user ran outside the agent's sandbox back
         * to the agent (shared/shell-run-view.js). Throws when
         * `canSendAgentPrompt` says no (the caller queues) or the send fails.
         */
        async sendAgentPrompt(sessionId, message) {
            if (!this.canSendAgentPrompt(sessionId)) throw new Error('The agent is busy');
            this.addUserMessage(message);
            this.startStreaming();
            try {
                trackEvent('message_sent', {
                    source: 'shell_run',
                    length: messageLengthBucket(message),
                });
                const turnId = this._recordTurn(sessionId, message, null);
                await this.invoke('send_message_streaming', {
                    sessionId,
                    message,
                    attachments: null,
                    turnId,
                });
                this.isConnected = true;
                this.updateConnectionStatus();
            } catch (error) {
                this._abortStreamingSend(error);
                throw error;
            }
        }

        addUserMessage(text, attachmentSnapshots) {
            // A history render still in flight goes in first, so the prompt
            // and its reply follow the whole transcript.
            this._finishDisplaySessionRender?.();
            const placeholder = this.elements.messagesArea.querySelector('.message-placeholder');
            if (placeholder) placeholder.remove();

            // Internal Kage tags are agent metadata; injected steering and
            // selection get their own section ahead of the bubble.
            const displayText = this.appendPromptContext(text);

            this.messages.push({ role: 'user', content: text });
            const msgEl = this.createMessageElement('user', displayText);

            // Set timestamp
            const ts = msgEl.querySelector('.msg-timestamp');
            if (ts)
                ts.textContent = new Date().toLocaleTimeString([], {
                    hour: '2-digit',
                    minute: '2-digit',
                });

            // Append attachment previews to the message bubble
            if (attachmentSnapshots && attachmentSnapshots.length > 0) {
                const contentDiv = msgEl.querySelector('.message-content');
                if (contentDiv) {
                    contentDiv.insertAdjacentHTML(
                        'beforeend',
                        attachmentPreviewHtml(attachmentSnapshots)
                    );
                }
            }

            this.elements.messagesArea.appendChild(msgEl);
            // A new user turn re-sticks the viewport to the bottom: the user
            // is asking a question and wants to watch the answer stream in.
            this._autoScrollEnabled = true;
            this.scrollToBottom(true);
        }

        // `originatedHere` distinguishes a turn this window actually sent from
        // one it's only passively mirroring (the floating window's send, echoed
        // via the `floating_message_sent` event). Only originated turns fire a
        // completion notification — otherwise the floating window AND every
        // chat window viewing the same session each toast the same completion.
        startStreaming(originatedHere = true) {
            this._startTurnView();
            this._turnOriginatedHere = originatedHere;
            // The user's next prompt from here: cards for commands outside
            // the sandbox that are over go (one still waiting stays).
            if (originatedHere) clearFinishedHostCommandCards?.();
            // An earlier failure's error box goes with a new prompt: its
            // Reconnect would restart the agent under this one.
            if (
                originatedHere &&
                this.elements.errorContainer?.querySelector('#errorReconnectBtn')
            ) {
                this.elements.errorContainer.innerHTML = '';
            }
            this.currentStreamingContent = '';
            this.toolSources = [];
            this.toolUsages = [];
            this._toolCallIds = new Set();
            this._sourceDomains = new Set();
            this.isWaitingForResponse = true;
            this.extensionToolController.reset();
            this.automationPlanController.reset();
            this._streamStartTime = Date.now();
            this.updateInputState();
            this.showTypingIndicator();

            this.currentStreamingMessage = this.createMessageElement('assistant', '');
            this.elements.messagesArea.appendChild(this.currentStreamingMessage);
            this.scrollToBottom();
        }

        /**
         * A turn starts in the window (a local send, a resend, open in
         * chat's prompt, a floating send mirrored here): a view change.
         * A history render still in flight goes in first, whole; anything
         * else still in flight for the view before (a pin read, a session
         * list) is superseded.
         */
        _startTurnView() {
            this._finishDisplaySessionRender?.();
            this._bumpViewGen();
        }

        stopGenerating() {
            if (!this.isWaitingForResponse) return;

            // If an automation plan is running, stop it gracefully
            if (this._automationPlanStarted) {
                this.automationPlanController.stopGracefully();
            }

            this.isWaitingForResponse = false;
            this.hideTypingIndicator();
            this._finishShellActivity();

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
            // Cancellation is terminal for this session's stream — clear the
            // registry entry (cancel_generation emits no completion event).
            if (this.activeSessionId) this.streamRegistry.fail(this.activeSessionId);
            this.invoke('cancel_generation', { sessionId: this.activeSessionId }).catch((e) =>
                console.log('Cancel:', e)
            );
        }
    };
}
