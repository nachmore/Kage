export function createModelsToolbarMixin(dependencies) {
    const {
        escapeHtml,
        errLabel,
        t,
        sanitizeExtensionHtmlStatic,
        renderToolbarButtons,
        runToolbarHostEffect,
        getConfig,
        parseContextPercent,
        drawContextRing,
    } = dependencies;
    return class {
        async refreshContextUsage() {
            if (!this.activeSessionId) return;
            try {
                const result = await this.invoke('execute_slash_command', {
                    sessionId: this.activeSessionId,
                    command: 'context',
                    args: {},
                });
                const pct = parseContextPercent(result);
                if (pct !== null) {
                    this.elements.contextPercent.textContent = pct + '%';
                    document.getElementById('contextIndicator').title = pct + '% context used';
                    this.drawContextRing(pct);
                }
            } catch (e) {
                console.log('[CONTEXT] Failed to fetch context usage:', e);
            }
        }

        drawContextRing(percent) {
            drawContextRing(document.getElementById('contextRing'), percent);
        }

        // --- Auto-Compact ---

        async maybeAutoCompact(percent) {
            // Don't kick off a second compaction while one is in flight.
            // `_isCompacting` is now cleared by the `compaction_status`
            // "completed" listener — not immediately after the slash-command
            // RPC returns — so the gate covers the agent's actual work.
            if (this._isCompacting) return;

            try {
                const config = await getConfig(this.invoke);
                const threshold = config.acp?.agent?.auto_compact_threshold ?? 90;
                if (threshold === 0 || percent < threshold) return;
            } catch {
                return;
            }

            // Don't auto-compact on the same value twice. Some agents (kiro)
            // report the same context-usage value before and after a
            // compaction round when the metric they expose is cumulative
            // (e.g. lifetime tokens) rather than live in-flight tokens. In
            // that case retrying immediately just loops forever — request a
            // change before the next attempt. Slack of 1pp guards float
            // jitter; the threshold itself remains the gate for *whether* to
            // compact at all.
            if (
                this._lastAutoCompactedAt != null &&
                Math.abs(percent - this._lastAutoCompactedAt) < 1
            ) {
                return;
            }

            this._isCompacting = true;
            this._lastAutoCompactedAt = percent;
            try {
                await this.invoke('execute_slash_command', {
                    sessionId: this.activeSessionId,
                    command: 'compact',
                    args: {},
                });
            } catch (e) {
                console.error('[COMPACT] Auto-compact failed:', e);
                this.hideCompactingNotice('Auto-compact failed');
                // Slash-command path failed — release the gate now since we
                // won't get a `compaction_status` "completed" event.
                this._isCompacting = false;
            }
            // On success, `_isCompacting` stays true until the
            // `compaction_status` "completed" notification clears it.
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
            notice.innerHTML = `<span class="compacting-spinner"></span> ${t('chat.compacting.in_progress')}`;
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
                    this.elements.modelName.textContent =
                        current.name || current.modelId || t('chat.model.unknown');
                    this.currentModelId = current.modelId;
                } else {
                    this.elements.modelName.textContent = t('chat.model.no_models');
                }
            } catch (e) {
                console.log('[MODELS] Failed to load models:', e);
                this.elements.modelName.textContent = t('chat.model.unavailable');
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
                dd.innerHTML = `<div class="chat-model-dropdown-item"><span class="chat-model-dropdown-item-name">${t('chat.model.no_models_available')}</span></div>`;
                dd.style.display = '';
                return;
            }
            for (const model of this.availableModels) {
                const item = document.createElement('div');
                item.className =
                    'chat-model-dropdown-item' +
                    (model.modelId === this.currentModelId ? ' active' : '');
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
                    sessionId: this.activeSessionId,
                    command: 'model',
                    args: { modelName: model.modelId },
                });
            } catch (e) {
                console.error('[MODELS] Failed to switch model:', e);
                this.showError(errLabel(t('chat.error.failed_switch_model'), e));
            }
        }

        /**
         * Render extension-contributed toolbar buttons into the chat toolbar.
         *
         * Sandboxed contract: each button's onClick is a host-side function
         * that round-trips to the sandbox with the current chat state and
         * may return a host effect describing what the host should do
         * (replace the input text, send a message, or show a notice).
         */
        renderExtensionToolbarButtons() {
            if (!this.extensionManager) return;
            const buttons = this.extensionManager.getToolbarButtons();

            const toolbarLeft = document.querySelector('.chat-toolbar-left');
            if (!toolbarLeft) return;

            renderToolbarButtons({
                container: toolbarLeft,
                buttons,
                buttonClass: 'chat-toolbar-btn',
                sanitizeIcon: (iconStr) => sanitizeExtensionHtmlStatic(iconStr, 'icon'),
                buildContext: () => ({
                    input: this.elements.chatInput?.value || '',
                    messages: (this.messages || []).map((m) => ({
                        role: m?.role || '',
                        content: typeof m?.content === 'string' ? m.content : '',
                    })),
                }),
                onHostEffect: (host, btn) => {
                    // Stamp the origin so the host effect handler can scope
                    // ephemeral bubbles / side effects to the right extension.
                    host.extensionId = btn.extensionId;
                    this._runToolbarHostEffect(host);
                },
            });
        }

        /**
         * Apply a host effect returned from a toolbar-button RPC.
         * Shared applier; the chat window supplies its input element and
         * an ephemeral-bubble renderer (extensions use those for
         * summaries/status that don't need to live in session history).
         */
        _runToolbarHostEffect(host) {
            runToolbarHostEffect(host, {
                input: this.elements.chatInput,
                onEphemeralMessage: (h) => this._renderEphemeralMessage(h),
                logTag: 'Chat',
            });
        }

        /**
         * Render an ephemeral (non-persisted) message bubble from an
         * extension. The HTML is sanitized with the rich policy; the bubble
         * is tagged so subsequent ephemeral messages from the same extension
         * replace the previous one rather than piling up.
         */
        _renderEphemeralMessage(host) {
            const messagesArea =
                document.querySelector('.messages-area') ||
                document.querySelector('.chat-messages');
            if (!messagesArea) return;

            const tag = String(host.tag || 'default');
            const extensionId = String(host.extensionId || 'unknown');
            const selector = `.ext-ephemeral-bubble[data-ext-bubble="${extensionId}:${tag}"]`;
            messagesArea.querySelectorAll(selector).forEach((el) => el.remove());

            const bubble = document.createElement('div');
            bubble.className = 'ext-ephemeral-bubble';
            bubble.setAttribute('data-ext-bubble', `${extensionId}:${tag}`);

            const title = host.title ? String(host.title) : '';
            if (title) {
                const header = document.createElement('div');
                header.className = 'ext-ephemeral-header';
                const titleSpan = document.createElement('span');
                titleSpan.textContent = title;
                header.appendChild(titleSpan);
                const close = document.createElement('button');
                close.className = 'ext-ephemeral-close';
                close.textContent = '✕';
                close.title = t('chat.dismiss_title');
                close.addEventListener('click', () => bubble.remove());
                header.appendChild(close);
                bubble.appendChild(header);
            }

            const body = document.createElement('div');
            body.className = 'ext-ephemeral-body';
            const frag = sanitizeExtensionHtmlStatic(String(host.html || ''), 'rich');
            body.appendChild(frag);
            bubble.appendChild(body);

            messagesArea.appendChild(bubble);
            messagesArea.scrollTop = messagesArea.scrollHeight;
        }
    };
}
