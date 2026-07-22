export function createChatActionsMixin(dependencies) {
    const {
        buildChatMarkdown,
        buildExecCtx,
        defaultExportFilename,
        escapeHtml,
        stripKageTags,
        t,
        unifiedSearch,
        getExtensionManager,
        executeResultShared,
        handleEnterAction,
        getConfig,
    } = dependencies;
    return class {
        async exportChatAsMarkdown() {
            if (!Array.isArray(this.messages) || this.messages.length === 0) {
                return;
            }
            const dialog = window.__TAURI__?.dialog;
            if (!dialog?.save) return;

            const title = stripKageTags(this.elements.chatHeaderTitle?.textContent || '') || '';
            const md = buildChatMarkdown({
                messages: this.messages,
                title,
                model: this.elements.modelName?.textContent?.trim() || '',
                sessionId: this.currentAcpSessionId || this.activeSessionId || '',
            });

            let target;
            try {
                target = await dialog.save({
                    defaultPath: defaultExportFilename(title),
                    filters: [{ name: 'Markdown', extensions: ['md'] }],
                });
            } catch {
                return;
            }
            if (!target) return; // user cancelled

            try {
                await this.invoke('write_text_file', { path: target, contents: md });
            } catch (e) {
                console.error('Failed to export chat:', e);
                alert(t('chat.export.failed', { message: e?.message || String(e) }));
            }
        }

        startTitleEdit() {
            if (!this.activeSessionId) return;
            const titleEl = this.elements.chatHeaderTitle;
            const inputEl = this.elements.chatHeaderTitleInput;
            inputEl.value = titleEl.textContent;
            titleEl.style.display = 'none';
            inputEl.style.display = 'inline-block';
            inputEl.focus();
            inputEl.select();
        }

        cancelTitleEdit() {
            this.elements.chatHeaderTitleInput.style.display = 'none';
            this.elements.chatHeaderTitle.style.display = '';
        }

        async finishTitleEdit() {
            const inputEl = this.elements.chatHeaderTitleInput;
            const titleEl = this.elements.chatHeaderTitle;
            const newTitle = inputEl.value.trim();

            inputEl.style.display = 'none';
            titleEl.style.display = '';

            if (!newTitle || !this.activeSessionId || newTitle === titleEl.textContent) return;

            try {
                await this.invoke('rename_session', {
                    sessionId: this.activeSessionId,
                    title: newTitle,
                });
                titleEl.textContent = newTitle;
                // Update in the sessions list too
                const session = this.sessions.find((s) => s.session_id === this.activeSessionId);
                if (session) session.title = newTitle;
                this.renderSessionList();
            } catch (e) {
                console.error('Failed to rename session:', e);
            }
        }

        async revealSessionFile(sessionId) {
            const id = sessionId || this.activeSessionId;
            if (!id) return;
            try {
                await this.invoke('reveal_session_file', { sessionId: id });
            } catch (e) {
                console.error('Failed to reveal session file:', e);
            }
        }

        startInlineRename(sessionId, itemEl) {
            const titleEl = itemEl.querySelector('.session-item-title');
            if (!titleEl) return;
            const currentTitle = titleEl.textContent.replace('●', '').trim();
            const input = document.createElement('input');
            input.className = 'session-rename-input';
            input.value = currentTitle;
            input.maxLength = 80;

            const contentEl = itemEl.querySelector('.session-item-content');
            contentEl.style.display = 'none';
            itemEl.querySelector('.session-item-actions').style.display = 'none';
            itemEl.insertBefore(input, itemEl.firstChild);
            input.focus();
            input.select();

            const finish = async () => {
                const newTitle = input.value.trim();
                input.remove();
                contentEl.style.display = '';
                itemEl.querySelector('.session-item-actions').style.display = '';

                if (newTitle && newTitle !== currentTitle) {
                    try {
                        await this.invoke('rename_session', { sessionId, title: newTitle });
                        const session = this.sessions.find((s) => s.session_id === sessionId);
                        if (session) session.title = newTitle;
                        if (sessionId === this.activeSessionId) {
                            this.elements.chatHeaderTitle.textContent = newTitle;
                        }
                        this.renderSessionList();
                    } catch (e) {
                        console.error('Failed to rename:', e);
                    }
                }
            };

            input.addEventListener('blur', finish);
            input.addEventListener('keydown', (e) => {
                if (e.key === 'Enter') {
                    e.preventDefault();
                    input.blur();
                }
                if (e.key === 'Escape') {
                    input.value = currentTitle;
                    input.blur();
                }
            });
        }

        async loadShortcuts() {
            try {
                const config = await getConfig(this.invoke);
                this.shortcuts = config.shortcuts || [];
            } catch {
                this.shortcuts = [];
            }
        }

        async updateSuggestions() {
            const input = this.elements.chatInput.value;
            const trimmed = input.trim();

            if (!trimmed) {
                this.clearSuggestions();
                return;
            }

            this._searchGeneration = (this._searchGeneration || 0) + 1;
            const gen = this._searchGeneration;
            const results = await unifiedSearch(
                trimmed,
                this.invoke,
                this.shortcuts,
                (partial, { done, pending }) => {
                    if (gen !== this._searchGeneration) return;
                    if (partial.length > 0) {
                        this.currentSuggestions = partial;
                        this.suggestionIndex = 0;
                        this.renderSuggestions();
                    }
                    // Show/hide loading indicator with provider names
                    const container = this.elements.chatSuggestions;
                    const existing = container.querySelector('.suggestions-loading');
                    if (done) {
                        if (existing) existing.remove();
                    } else if (container.classList.contains('visible')) {
                        let label = t('chat.suggestions.loading_more');
                        if (pending && pending.length > 0) {
                            const shown = pending.slice(0, 2).join(', ');
                            label += ' (' + shown + (pending.length > 2 ? ', \u2026' : '') + ')';
                        }
                        label += '\u2026';
                        if (existing) {
                            existing.textContent = label;
                        } else {
                            const hint = document.createElement('div');
                            hint.className = 'suggestions-hint suggestions-loading';
                            hint.textContent = label;
                            container.appendChild(hint);
                        }
                    }
                }
            );
            if (gen !== this._searchGeneration) return;
            // Remove loading indicator — all providers have resolved
            const loadingEl = this.elements.chatSuggestions.querySelector('.suggestions-loading');
            if (loadingEl) loadingEl.remove();
            this._searchCompletedGen = gen;
            if (results.length > 0) {
                this.currentSuggestions = results;
                this.suggestionIndex = 0;
                this.renderSuggestions();
            } else {
                this.clearSuggestions();
            }
        }

        async renderSuggestions() {
            const container = this.elements.chatSuggestions;
            container.innerHTML = '';

            if (this.currentSuggestions.length === 0) {
                container.classList.remove('visible');
                return;
            }

            const extMgr = getExtensionManager();
            // Prime the custom-render cache so the synchronous renderResult()
            // calls below can resolve from cache.
            if (extMgr?.prefetchCustomRender) {
                try {
                    await extMgr.prefetchCustomRender(this.currentSuggestions);
                } catch {}
            }

            this.currentSuggestions.forEach((cmd, index) => {
                const item = document.createElement('div');
                item.className =
                    'chat-suggestion-item' + (index === this.suggestionIndex ? ' selected' : '');

                // Let extensions render their own results
                if (cmd._extensionId && extMgr) {
                    const customEl = document.createElement('div');
                    customEl.style.cssText = 'display:flex;align-items:center;gap:8px;flex:1;';
                    if (extMgr.renderResult(cmd, customEl)) {
                        item.appendChild(customEl);
                        item.addEventListener('click', () => this.executeSuggestion(cmd));
                        container.appendChild(item);
                        return;
                    }
                }

                // Default rendering for non-extension results. Build the icon as
                // a real element so the fallback glyph goes through textContent —
                // the old inline onerror interpolated cmd.label into a JS string
                // inside an HTML attribute, so a label starting with `'` broke out.
                const fallbackGlyph = cmd.icon || cmd.label?.charAt(0) || '?';
                let iconEl;
                if (cmd.type === 'app' && cmd.data?.icon_base64) {
                    const src = cmd.data.icon_base64.startsWith('data:')
                        ? cmd.data.icon_base64
                        : 'data:image/png;base64,' + cmd.data.icon_base64;
                    iconEl = document.createElement('img');
                    iconEl.src = src;
                    iconEl.style.cssText = 'width:20px;height:20px;border-radius:4px;';
                    iconEl.onerror = () =>
                        iconEl.replaceWith(document.createTextNode(fallbackGlyph));
                } else {
                    iconEl = document.createElement('span');
                    iconEl.className = 'chat-suggestion-icon';
                    iconEl.textContent = fallbackGlyph;
                }

                item.innerHTML = `
                <div class="chat-suggestion-info">
                    <div class="chat-suggestion-name">${escapeHtml(cmd.label || cmd.name || '')}</div>
                    ${cmd.description ? `<div class="chat-suggestion-desc">${escapeHtml(cmd.description)}</div>` : ''}
                </div>
            `;
                item.insertBefore(iconEl, item.firstChild);
                item.addEventListener('click', () => this.executeSuggestion(cmd));
                container.appendChild(item);
            });

            container.classList.add('visible');
        }

        clearSuggestions() {
            this.currentSuggestions = [];
            this.suggestionIndex = -1;
            this._searchGeneration = (this._searchGeneration || 0) + 1; // discard in-flight searches
            this.elements.chatSuggestions.innerHTML = '';
            this.elements.chatSuggestions.classList.remove('visible');
        }

        /** Build execution context for the shared result executor. */
        _getExecCtx() {
            return buildExecCtx({
                invoke: this.invoke,
                appWindow: this.appWindow,
                extensionManager: getExtensionManager(),
                input: this.elements.chatInput,
                extra: {
                    onPrompt: (text) => {
                        this.elements.chatInput.value = text;
                        this.sendMessage();
                    },
                    // Prompt-type Quick Commands with unfilled named placeholders
                    // surface the form in the floating window, not here. In the
                    // chat window we don't have the same focused launcher UI;
                    // typing the trigger with positional args (`tr spanish hi`)
                    // works exactly the same as before. If a user runs a form-
                    // requiring command from the chat sidebar, we surface a
                    // helpful note rather than a silent no-op.
                    onPromptForm: (formCmd) => {
                        const slot = formCmd.missing
                            .map((p) => (p.optional ? `${p.name}?` : p.name))
                            .join(', ');
                        this.addMessageFromHistory(
                            'assistant',
                            `This Quick Command needs values for: \`${slot}\`. Try \`${formCmd.shortcut.shortcut} <${slot}>\` or run it from the floating window.`
                        );
                        this.scrollToBottom();
                    },
                    onDisplay: (text) => {
                        this.addMessageFromHistory('assistant', text);
                        this.scrollToBottom();
                    },
                },
            });
        }

        async executeSuggestion(cmd) {
            const query = this.elements.chatInput.value.trim();
            this.elements.chatInput.value = '';
            this.elements.chatInput.style.height = 'auto';
            this.clearSuggestions();

            const result = await executeResultShared(cmd, query, this._getExecCtx());
            if (result.handled) return;

            // Fallback for unhandled types
            if (cmd.execute) {
                await cmd.execute(this.invoke, this.appWindow);
            }
        }

        async handleEnterKey() {
            // If an async search is still in-flight (started but not yet resolved),
            // discard it and clear stale suggestions so we fall through to direct
            // shortcut/command matching on the actual input value.
            if ((this._searchGeneration || 0) !== (this._searchCompletedGen || 0)) {
                this._searchGeneration = (this._searchGeneration || 0) + 1;
                this.currentSuggestions = [];
                this.suggestionIndex = -1;
            }

            const message = this.elements.chatInput.value.trim();
            const hasAttachments = this.attachmentManager.hasAttachments();
            const hasSelection = this.currentSuggestions.length > 0 && this.suggestionIndex >= 0;

            if (!message && !hasAttachments && !hasSelection) return;

            if (this.isWaitingForResponse) {
                this.stopGenerating();
            }

            const result = await handleEnterAction({
                message,
                suggestions: this.currentSuggestions,
                selectedIndex: this.suggestionIndex,
                shortcuts: this.shortcuts,
                ctx: this._getExecCtx(),
                onSend: (msg) => {
                    this.elements.chatInput.value = msg;
                    this.sendMessage();
                },
            });

            if (result.handled) {
                this.elements.chatInput.value = '';
                this.elements.chatInput.style.height = 'auto';
                this.clearSuggestions();
            }
        }

        scrollToBottom() {
            const area = this.elements.messagesArea;
            // 'auto' during streaming: renderStreaming re-issues this every
            // ~150ms, and overlapping smooth animations toward a growing
            // scrollHeight fight each other — visible jank on long responses.
            // Smooth is reserved for discrete jumps (send, final render).
            const behavior = this.isWaitingForResponse ? 'auto' : 'smooth';
            requestAnimationFrame(() => {
                area.scrollTo({ top: area.scrollHeight, behavior });
            });
        }

        convertFileSrc(path) {
            // Tauri 2 uses asset protocol for local files
            if (window.__TAURI__?.core?.convertFileSrc) {
                return window.__TAURI__.core.convertFileSrc(path);
            }
            // Fallback: use file:// protocol
            return 'file://' + path.replace(/\\/g, '/');
        }

        // --- Toolbar: File & Image Attach ---

        async handleFileAttach(event) {
            const files = event.target.files;
            if (!files || files.length === 0) return;
            for (const file of files) {
                this.attachmentManager.addFile(file.name, file.name, file.type || 'text/plain');
            }
            // Store the actual File objects so we can read them at send time
            if (!this._pendingFiles) this._pendingFiles = [];
            for (const file of files) {
                this._pendingFiles.push(file);
            }
            event.target.value = '';
        }

        async handleImageAttach(event) {
            const files = event.target.files;
            if (!files || files.length === 0) return;
            for (const file of files) {
                if (!file.type.startsWith('image/')) continue;
                try {
                    const base64 = await this._fileToBase64(file);
                    this.attachmentManager.addImage(base64, file.type);
                } catch (e) {
                    console.error('Failed to read image:', file.name, e);
                }
            }
            event.target.value = '';
        }

        _fileToBase64(file) {
            return new Promise((resolve, reject) => {
                const reader = new FileReader();
                reader.onload = () => {
                    const result = reader.result;
                    const base64 = result.split(',')[1];
                    resolve(base64);
                };
                reader.onerror = reject;
                reader.readAsDataURL(file);
            });
        }

        // --- Toolbar: Context Indicator ---
    };
}
