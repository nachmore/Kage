export function createComposerMixin(dependencies) {
    const {
        renderMarkdown,
        attachmentPreviewHtml,
        executeCommand,
        getSlashCommandMeta,
        getSlashCommandDispatch,
        stripKageTags,
        errLabel,
        t,
        loadSelection,
        SpeechController,
        trackEvent,
        messageLengthBucket,
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

            // If a plan is pending review, send the message as a revision request
            if (this._pendingPlanRevision && message) {
                this.automationPlanController.reset();
                this.extensionToolController.reset();
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
                this._sourceDomains = new Set();
                this.isWaitingForResponse = true;
                this._streamStartTime = Date.now();
                this.updateInputState();
                this.showTypingIndicator();
                this.currentStreamingMessage = this.createMessageElement('assistant', '');
                this.elements.messagesArea.appendChild(this.currentStreamingMessage);
                this.scrollToBottom();

                try {
                    trackEvent('message_sent', {
                        source: 'chat',
                        length: messageLengthBucket(message),
                    });
                    await this.invoke('send_message_streaming', {
                        sessionId: this.activeSessionId,
                        message,
                        attachments: null,
                    });
                } catch (e) {
                    this.handleMessageError({ payload: errLabel(t('chat.error.error_label'), e) });
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

            try {
                trackEvent('message_sent', {
                    source: 'chat',
                    length: messageLengthBucket(message),
                    attachments: attachments?.length || 0,
                });
                await this.invoke('send_message_streaming', {
                    sessionId: this.activeSessionId,
                    message,
                    attachments,
                });
                this.isConnected = true;
                this.updateConnectionStatus();
            } catch (error) {
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
        }

        addUserMessage(text, attachmentSnapshots) {
            const placeholder = this.elements.messagesArea.querySelector('.message-placeholder');
            if (placeholder) placeholder.remove();

            // Strip internal Kage tags from display (it's metadata for the agent, not for the user)
            const displayText = stripKageTags(text);

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
            this.scrollToBottom();
        }

        startStreaming() {
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

        stopGenerating() {
            if (!this.isWaitingForResponse) return;

            // If an automation plan is running, stop it gracefully
            if (this._automationPlanStarted) {
                this.automationPlanController.stopGracefully();
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
            // Cancellation is terminal for this session's stream — clear the
            // registry entry (cancel_generation emits no completion event).
            if (this.activeSessionId) this.streamRegistry.fail(this.activeSessionId);
            this.invoke('cancel_generation', { sessionId: this.activeSessionId }).catch((e) =>
                console.log('Cancel:', e)
            );
        }
    };
}
