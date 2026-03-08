/**
 * Shortcuts Settings Module
 * Manages custom command shortcuts
 */
class ShortcutsSettingsModule extends SettingsModule {
    constructor() {
        super('shortcuts', 'Quick Commands', '⚡');
        this.shortcuts = [];
        this.editingIndex = -1;
    }

    render() {
        return `
            <div class="settings-section-header">${this.icon} Quick Commands</div>
            
            <div class="setting-row">
                <div class="setting-label-container">
                    <div class="setting-label">Quick Commands</div>
                    <div class="setting-description">
                        Create quick commands to run programs, open URLs, send prompts to the agent, or run custom scripts.
                        Use {*} for all arguments after the shortcut, or {0}, {1}, etc. for specific arguments.
                    </div>
                </div>
            </div>

            <div class="shortcuts-actions">
                <button class="setting-button" onclick="shortcutsModule.showAddDialog()">+ Add</button>
                <button class="setting-button" onclick="if(window.__TAURI__?.core)window.__TAURI__.core.invoke('open_store_window',{tab:'commands'})">🛍️ Store</button>
                <button class="setting-button" onclick="shortcutsModule.exportShortcuts()">Export</button>
                <button class="setting-button" onclick="shortcutsModule.importShortcuts()">Import</button>
            </div>

            <div class="shortcuts-list" id="shortcutsList"></div>

            <!-- Add/Edit Dialog -->
            <div id="shortcutDialog" class="shortcut-dialog" style="display: none;">
                <div class="shortcut-dialog-content">
                    <div class="shortcut-dialog-header">
                        <h3 id="dialogTitle">Add Shortcut</h3>
                        <button class="dialog-close-btn" onclick="shortcutsModule.closeDialog()">×</button>
                    </div>
                    <div class="shortcut-dialog-body">
                        <div class="dialog-field">
                            <label>Name / Description</label>
                            <input type="text" id="shortcutName" class="setting-input" placeholder="e.g., Open VSCode">
                        </div>
                        <div class="dialog-field">
                            <label>Trigger Word</label>
                            <input type="text" id="shortcutTrigger" class="setting-input" placeholder="e.g., code">
                        </div>
                        <div class="dialog-field">
                            <label>Icon (optional)</label>
                            <div style="display:flex;gap:8px;align-items:center;">
                                <div id="shortcutIconPreview" class="shortcut-icon-preview">⚡</div>
                                <input type="text" id="shortcutIconEmoji" class="setting-input" style="width:60px;text-align:center;" placeholder="⚡" maxlength="4" oninput="shortcutsModule.onIconInput()">
                                <button class="setting-button" onclick="document.getElementById('shortcutIconFile').click()">📁 Image</button>
                                <button class="setting-button" id="shortcutFaviconBtn" style="display:none" onclick="shortcutsModule.fetchFavicon()">🌐 Use Favicon</button>
                                <button class="setting-button" id="shortcutIconClear" style="display:none" onclick="shortcutsModule.clearIcon()">✕</button>
                                <input type="file" id="shortcutIconFile" accept="image/png,image/jpeg,image/x-icon,image/vnd.microsoft.icon,.ico,.png,.jpg,.jpeg" style="display:none" onchange="shortcutsModule.onIconFileSelected(event)">
                            </div>
                        </div>
                        <div class="dialog-field">
                            <label>Action Type</label>
                            <select id="shortcutActionType" class="setting-select" onchange="shortcutsModule.onActionTypeChange()">
                                <option value="run_program">▶️ Run Program</option>
                                <option value="open_url">🌐 Open URL</option>
                                <option value="prompt">💬 Send Prompt to Agent</option>
                                <option value="script">📜 Run Script</option>
                            </select>
                        </div>
                        
                        <!-- Run Program Fields -->
                        <div id="runProgramFields">
                            <div class="dialog-field">
                                <label>Executable Path</label>
                                <input type="text" id="shortcutPath" class="setting-input" placeholder="e.g., C:\\Program Files\\VSCode\\code.exe">
                            </div>
                            <div class="dialog-field">
                                <label>Working Directory (optional)</label>
                                <input type="text" id="shortcutWorkDir" class="setting-input" placeholder="e.g., C:\\Projects">
                            </div>
                            <div class="dialog-field">
                                <label>Arguments (optional)</label>
                                <input type="text" id="shortcutArgs" class="setting-input" placeholder="e.g., --send {1} --to {0} or {*}">
                                <div class="setting-description" style="margin-top: 4px;">
                                    Use {*} for all arguments, or {0}, {1}, etc. for specific arguments
                                </div>
                            </div>
                        </div>
                        
                        <!-- Open URL Fields -->
                        <div id="openUrlFields" style="display: none;">
                            <div class="dialog-field">
                                <label>URL Template</label>
                                <input type="text" id="shortcutUrl" class="setting-input" placeholder="e.g., https://google.com/search?q={*}">
                                <div class="setting-description" style="margin-top: 4px;">
                                    Use {*} for all arguments, or {0}, {1}, etc. for specific arguments in the URL
                                </div>
                            </div>
                        </div>

                        <!-- Prompt Fields -->
                        <div id="promptFields" style="display: none;">
                            <div class="dialog-field">
                                <label>Prompt Template</label>
                                <textarea id="shortcutPrompt" class="setting-input" rows="3" placeholder="e.g., Explain this error: {*}"></textarea>
                                <div class="setting-description" style="margin-top: 4px;">
                                    Use {*} for all arguments, or {0}, {1}, etc. The result is sent to the agent as a message.
                                </div>
                            </div>
                        </div>

                        <!-- Script Fields -->
                        <div id="scriptFields" style="display: none;">
                            <div class="dialog-field">
                                <label>Script Action</label>
                                <select id="shortcutScriptAction" class="setting-select">
                                    <option value="text">📝 Display Result</option>
                                    <option value="prompt">💬 Send to Agent</option>
                                    <option value="open_url">🌐 Open as URL</option>
                                    <option value="run_program">▶️ Run as Command</option>
                                </select>
                                <div class="setting-description" style="margin-top: 4px;">
                                    What to do with the string returned by your script
                                </div>
                            </div>
                            <div class="dialog-field">
                                <label>✨ Ask Kiro to write or update the script</label>
                                <div class="ai-prompt-row">
                                    <input type="text" id="scriptAiPrompt" class="setting-input" placeholder="e.g., Parse a Jira ticket URL and return the ticket ID" onkeydown="if(event.key==='Enter'){event.preventDefault();shortcutsModule.generateScript()}">
                                    <button class="setting-button" id="scriptAiBtn" onclick="shortcutsModule.generateScript()">Generate</button>
                                    <button class="setting-button" id="scriptAiUndo" onclick="shortcutsModule.undoGenerate()" style="display:none">Undo</button>
                                </div>
                                <div id="scriptAiStatus" class="setting-description" style="margin-top: 4px;"></div>
                            </div>
                            <div class="dialog-field">
                                <label>Script Body</label>
                                <div class="script-editor-container">
                                    <pre class="script-highlight" aria-hidden="true"><code class="language-javascript" id="shortcutScriptHighlight"></code></pre>
                                    <textarea id="shortcutScript" class="setting-input script-editor" rows="8" spellcheck="false" wrap="off"
                                        placeholder="// Arguments are passed as ...args&#10;// Return a string (or array for Run as Command)&#10;const query = args.join(' ');&#10;return 'Processed: ' + query;"
                                        oninput="shortcutsModule.updateHighlight()"></textarea>
                                </div>
                                <div class="setting-description" style="margin-top: 4px;">
                                    JavaScript function body. Receives arguments as <code>...args</code>.
                                    Return a string for Display/Agent/URL actions, or an array <code>[cmd, workDir, ...args]</code> for Run as Command.
                                </div>
                            </div>
                        </div>
                    </div>
                    <div class="shortcut-dialog-footer">
                        <button class="setting-button" onclick="shortcutsModule.closeDialog()">Cancel</button>
                        <button class="setting-button" onclick="shortcutsModule.saveShortcut()">Save</button>
                    </div>
                </div>
            </div>

            <style>
                .shortcuts-list { margin: 20px 0; border: 1px solid var(--kiro-border-subtle); border-radius: 4px; overflow: hidden; }
                .shortcut-item { padding: 16px; border-bottom: 1px solid var(--kiro-border-subtle); display: flex; justify-content: space-between; align-items: flex-start; background: var(--kiro-bg-input); }
                .shortcut-item:last-child { border-bottom: none; }
                .shortcut-item:hover { background: var(--kiro-bg-elevated); }
                .shortcut-info { flex: 1; }
                .shortcut-name { font-size: 14px; font-weight: 500; color: var(--kiro-text-bright); margin-bottom: 4px; }
                .shortcut-trigger { display: inline-block; padding: 2px 8px; background: var(--kiro-accent); color: #ffffff; border-radius: 3px; font-size: 12px; font-family: 'Courier New', monospace; margin-bottom: 8px; }
                .shortcut-details { font-size: 12px; color: var(--kiro-text-muted); line-height: 1.6; }
                .shortcut-actions { display: flex; gap: 8px; }
                .shortcut-action-btn { padding: 4px 12px; background: transparent; border: 1px solid var(--kiro-border); border-radius: 2px; color: var(--kiro-text); font-size: 12px; cursor: pointer; transition: all 0.2s; }
                .shortcut-action-btn:hover { background: var(--kiro-border); border-color: var(--kiro-accent); }
                .shortcut-action-btn.delete:hover { background: #c0392b; border-color: #c0392b; color: #ffffff; }
                .shortcuts-actions { display: flex; gap: 12px; margin-bottom: 16px; }
                .shortcut-dialog { position: fixed; top: 0; left: 0; right: 0; bottom: 0; background: rgba(0,0,0,0.7); display: flex; align-items: center; justify-content: center; z-index: 1000; }
                .shortcut-dialog-content { background: var(--kiro-bg-elevated); border: 1px solid var(--kiro-border-subtle); border-radius: 4px; width: 600px; max-width: 90%; max-height: 90vh; overflow: auto; }
                .shortcut-dialog-header { padding: 16px 20px; border-bottom: 1px solid var(--kiro-border-subtle); display: flex; justify-content: space-between; align-items: center; }
                .shortcut-dialog-header h3 { font-size: 16px; font-weight: 500; color: var(--kiro-text-bright); margin: 0; }
                .dialog-close-btn { background: transparent; border: none; color: var(--kiro-text); font-size: 24px; cursor: pointer; padding: 0; width: 30px; height: 30px; display: flex; align-items: center; justify-content: center; border-radius: 2px; }
                .dialog-close-btn:hover { background: var(--kiro-border); }
                .shortcut-dialog-body { padding: 20px; }
                .dialog-field { margin-bottom: 16px; }
                .dialog-field:last-child { margin-bottom: 0; }
                .dialog-field label { display: block; font-size: 13px; color: var(--kiro-text); margin-bottom: 6px; font-weight: 500; }
                .shortcut-dialog-footer { padding: 16px 20px; border-top: 1px solid var(--kiro-border-subtle); display: flex; justify-content: flex-end; gap: 12px; }
                .shortcuts-empty { padding: 40px; text-align: center; color: var(--kiro-text-muted); font-size: 13px; }
                .shortcut-icon-preview { width: 32px; height: 32px; border-radius: 6px; background: var(--kiro-bg-input); border: 1px solid var(--kiro-border); display: flex; align-items: center; justify-content: center; font-size: 18px; overflow: hidden; flex-shrink: 0; }
                .shortcut-icon-preview img { width: 100%; height: 100%; object-fit: cover; }
                .script-editor { font-family: 'SF Mono', 'Consolas', 'Monaco', monospace; font-size: 12px; line-height: 1.5; resize: vertical; min-height: 120px; }
                .script-editor-container {
                    display: grid; grid-template-columns: 1fr; grid-template-rows: 1fr; gap: 0;
                }
                .script-editor-container .script-editor,
                .script-editor-container .script-highlight {
                    grid-area: 1 / 1 / 2 / 2;
                    font-family: 'SF Mono', 'Consolas', 'Monaco', monospace;
                    font-size: 12px; line-height: 1.5;
                    padding: 6px 10px; margin: 0;
                    white-space: pre; word-break: normal;
                    border: 1px solid var(--kiro-border); border-radius: 4px;
                    overflow: auto; box-sizing: border-box;
                    min-height: 120px; tab-size: 2;
                }
                .script-editor-container .script-editor {
                    background: transparent; color: transparent;
                    caret-color: var(--kiro-text-bright);
                    resize: vertical; width: 100%; z-index: 2;
                    outline: none;
                }
                .script-editor-container .script-editor:focus {
                    border-color: var(--kiro-accent);
                }
                .script-editor-container .script-highlight {
                    background: var(--kiro-bg-input); color: var(--kiro-text);
                    pointer-events: none; z-index: 1;
                    border-color: transparent;
                    overflow: auto;
                }
                .script-highlight code {
                    font-family: inherit !important; font-size: 1em !important;
                    background: none !important; padding: 0 !important; margin: 0 !important;
                    color: inherit; word-wrap: break-word;
                }
                .ai-prompt-row { display: flex; gap: 8px; }
                .ai-prompt-row .setting-input { flex: 1; }
                .ai-prompt-row .setting-button { white-space: nowrap; }
                textarea.setting-input { resize: vertical; }
            </style>
        `;
    }

    initialize() {
        this.renderShortcutsList();
        window.shortcutsModule = this;
        this._escHandler = (e) => {
            if (e.key === 'Escape' && document.getElementById('shortcutDialog')?.style.display === 'flex') {
                e.stopPropagation();
                this.closeDialog();
            }
        };
        document.addEventListener('keydown', this._escHandler, true);
    }

    load(config) {
        this.shortcuts = config.shortcuts || [];
        this.renderShortcutsList();
    }

    save(config) {
        config.shortcuts = this.shortcuts;
    }

    validate() {
        return { valid: true };
    }

    renderShortcutsList() {
        const listEl = document.getElementById('shortcutsList');
        if (!listEl) return;

        if (this.shortcuts.length === 0) {
            listEl.innerHTML = '<div class="shortcuts-empty">No shortcuts configured. Click "Add Shortcut" to create one.</div>';
            return;
        }

        const actionLabels = {
            run_program: '▶️ Run Program',
            open_url: '🌐 Open URL',
            prompt: '💬 Prompt',
            script: '📜 Script'
        };

        listEl.innerHTML = this.shortcuts.map((s, index) => {
            const at = s.action_type || 'run_program';
            const label = actionLabels[at] || at;
            let details = `<div><strong>Type:</strong> ${label}</div>`;

            if (at === 'open_url') {
                details += `<div><strong>URL:</strong> ${escapeHtml(s.url || '')}</div>`;
            } else if (at === 'prompt') {
                details += `<div><strong>Prompt:</strong> ${escapeHtml(s.prompt || '')}</div>`;
            } else if (at === 'script') {
                const saLabels = { text: '📝 Display', prompt: '💬 Agent', open_url: '🌐 URL', run_program: '▶️ Run' };
                details += `<div><strong>Action:</strong> ${saLabels[s.script_action] || 'Display'}</div>`;
                details += `<div><strong>Script:</strong> <code>${escapeHtml((s.script || '').substring(0, 60))}${(s.script || '').length > 60 ? '...' : ''}</code></div>`;
            } else {
                details += `<div><strong>Path:</strong> ${escapeHtml(s.path || '')}</div>`;
                if (s.working_directory) details += `<div><strong>Dir:</strong> ${escapeHtml(s.working_directory)}</div>`;
                if (s.arguments) details += `<div><strong>Args:</strong> ${escapeHtml(s.arguments)}</div>`;
            }

            let iconHtml;
            if (s.icon && s.icon.startsWith('data:')) {
                iconHtml = `<img src="${s.icon}" style="width:24px;height:24px;border-radius:4px;object-fit:cover;margin-right:8px;">`;
            } else if (s.icon) {
                iconHtml = `<span style="font-size:18px;margin-right:8px;">${s.icon}</span>`;
            } else {
                iconHtml = `<span style="font-size:18px;margin-right:8px;">⚡</span>`;
            }

            return `
                <div class="shortcut-item">
                    <div class="shortcut-info" style="display:flex;align-items:flex-start;">
                        <div style="padding-top:2px;">${iconHtml}</div>
                        <div>
                            <div class="shortcut-name">${escapeHtml(s.name)}</div>
                            <div class="shortcut-trigger">${escapeHtml(s.shortcut)}</div>
                            <div class="shortcut-details">${details}</div>
                        </div>
                    </div>
                    <div class="shortcut-actions">
                        <button class="shortcut-action-btn" onclick="shortcutsModule.editShortcut(${index})">Edit</button>
                        <button class="shortcut-action-btn delete" onclick="shortcutsModule.deleteShortcut(${index})">Delete</button>
                    </div>
                </div>`;
        }).join('');
    }

    showAddDialog() {
        this.editingIndex = -1;
        document.getElementById('dialogTitle').textContent = 'Add Shortcut';
        document.getElementById('shortcutName').value = '';
        document.getElementById('shortcutTrigger').value = '';
        document.getElementById('shortcutActionType').value = 'run_program';
        document.getElementById('shortcutPath').value = '';
        document.getElementById('shortcutWorkDir').value = '';
        document.getElementById('shortcutArgs').value = '';
        document.getElementById('shortcutUrl').value = '';
        document.getElementById('shortcutPrompt').value = '';
        document.getElementById('shortcutScript').value = '';
        document.getElementById('shortcutScriptAction').value = 'text';
        document.getElementById('scriptAiPrompt').value = '';
        document.getElementById('scriptAiStatus').textContent = '';
        document.getElementById('scriptAiUndo').style.display = 'none';
        this._previousScript = null;
        this._setIconPreview('');
        this.onActionTypeChange();
        document.getElementById('shortcutDialog').style.display = 'flex';
    }

    editShortcut(index) {
        this.editingIndex = index;
        const s = this.shortcuts[index];
        document.getElementById('dialogTitle').textContent = 'Edit Shortcut';
        document.getElementById('shortcutName').value = s.name;
        document.getElementById('shortcutTrigger').value = s.shortcut;
        document.getElementById('shortcutActionType').value = s.action_type || 'run_program';
        document.getElementById('shortcutPath').value = s.path || '';
        document.getElementById('shortcutWorkDir').value = s.working_directory || '';
        document.getElementById('shortcutArgs').value = s.arguments || '';
        document.getElementById('shortcutUrl').value = s.url || '';
        document.getElementById('shortcutPrompt').value = s.prompt || '';
        document.getElementById('shortcutScript').value = s.script || '';
        document.getElementById('shortcutScriptAction').value = s.script_action || 'text';
        document.getElementById('scriptAiPrompt').value = '';
        document.getElementById('scriptAiStatus').textContent = '';
        document.getElementById('scriptAiUndo').style.display = 'none';
        this._previousScript = null;
        this._setIconPreview(s.icon || '');
        this.onActionTypeChange();
        document.getElementById('shortcutDialog').style.display = 'flex';
        if ((s.action_type || 'run_program') === 'script') this.updateHighlight();
    }

    onActionTypeChange() {
        const at = document.getElementById('shortcutActionType').value;
        document.getElementById('runProgramFields').style.display = at === 'run_program' ? 'block' : 'none';
        document.getElementById('openUrlFields').style.display = at === 'open_url' ? 'block' : 'none';
        document.getElementById('promptFields').style.display = at === 'prompt' ? 'block' : 'none';
        document.getElementById('scriptFields').style.display = at === 'script' ? 'block' : 'none';
        // Show "Use Favicon" button only for URL shortcuts
        const faviconBtn = document.getElementById('shortcutFaviconBtn');
        if (faviconBtn) faviconBtn.style.display = at === 'open_url' ? '' : 'none';
        if (at === 'script') this.updateHighlight();
    }

    /** Set the icon preview and hidden state */
    _setIconPreview(icon) {
        this._currentIcon = icon || '';
        const preview = document.getElementById('shortcutIconPreview');
        const emojiInput = document.getElementById('shortcutIconEmoji');
        const clearBtn = document.getElementById('shortcutIconClear');
        if (!preview) return;

        if (icon && icon.startsWith('data:')) {
            // Base64 image
            preview.innerHTML = `<img src="${icon}">`;
            if (emojiInput) emojiInput.value = '';
            if (clearBtn) clearBtn.style.display = '';
        } else if (icon) {
            // Emoji
            preview.textContent = icon;
            if (emojiInput) emojiInput.value = icon;
            if (clearBtn) clearBtn.style.display = '';
        } else {
            preview.textContent = '⚡';
            if (emojiInput) emojiInput.value = '';
            if (clearBtn) clearBtn.style.display = 'none';
        }
    }

    /** Handle emoji input */
    onIconInput() {
        const val = document.getElementById('shortcutIconEmoji')?.value || '';
        this._setIconPreview(val);
    }

    /** Handle image file selection */
    onIconFileSelected(event) {
        const file = event.target.files?.[0];
        if (!file) return;
        // Limit to 64KB
        if (file.size > 65536) {
            alert('Icon image must be under 64KB');
            return;
        }
        const reader = new FileReader();
        reader.onload = (e) => {
            this._setIconPreview(e.target.result);
        };
        reader.readAsDataURL(file);
        // Reset file input so the same file can be re-selected
        event.target.value = '';
    }

    /** Fetch favicon from the URL field via backend (avoids CORS) */
    async fetchFavicon() {
        const urlInput = document.getElementById('shortcutUrl');
        const url = urlInput?.value?.trim();
        if (!url) { alert('Enter a URL first'); return; }

        const btn = document.getElementById('shortcutFaviconBtn');
        const origText = btn?.textContent;
        if (btn) btn.textContent = '⏳ Fetching...';

        try {
            const invoke = window.__TAURI__.core.invoke;
            const dataUri = await invoke('fetch_favicon', { url });
            this._setIconPreview(dataUri);
        } catch (e) {
            console.warn('Favicon fetch failed:', e);
            alert('Could not fetch favicon for this URL');
        } finally {
            if (btn) btn.textContent = origText;
        }
    }

    /** Clear the custom icon */
    clearIcon() {
        this._setIconPreview('');
    }

    updateHighlight() {
        const textarea = document.getElementById('shortcutScript');
        const highlight = document.getElementById('shortcutScriptHighlight');
        if (!textarea || !highlight) return;
        highlight.textContent = textarea.value + '\n';
        if (window.Prism) Prism.highlightElement(highlight);

        // Attach scroll sync once
        if (!textarea._scrollSynced) {
            textarea._scrollSynced = true;
            const pre = highlight.parentElement;
            textarea.addEventListener('scroll', () => {
                pre.scrollTop = textarea.scrollTop;
                pre.scrollLeft = textarea.scrollLeft;
            });
        }
    }

    async generateScript() {
        const promptInput = document.getElementById('scriptAiPrompt');
        const statusEl = document.getElementById('scriptAiStatus');
        const btn = document.getElementById('scriptAiBtn');
        const userPrompt = promptInput?.value.trim();
        if (!userPrompt) { statusEl.textContent = 'Please enter a description.'; return; }

        const scriptAction = document.getElementById('shortcutScriptAction')?.value || 'text';
        const currentScript = document.getElementById('shortcutScript')?.value.trim() || '';

        // Build action-specific return format hints
        let returnSpec;
        if (scriptAction === 'run_program') {
            returnSpec = 'Return an array: [command, workingDirectory, ...args]. workingDirectory can be an empty string for the default directory. Example: return ["git", "C:\\\\projects", "status"];';
        } else if (scriptAction === 'open_url') {
            returnSpec = 'Return a string containing a valid URL. Example: return "https://example.com/search?q=" + encodeURIComponent(args[0]);';
        } else if (scriptAction === 'prompt') {
            returnSpec = 'Return a string that will be sent to an AI agent as a prompt. Example: return "Explain this error: " + args.join(" ");';
        } else {
            returnSpec = 'Return a string that will be displayed to the user. Example: return "Result: " + args.join(", ");';
        }

        const parts = [
            '<role>You are a JavaScript code generator for Kiro Assistant shortcut scripts.</role>',
            '',
            '<instructions>',
            'Write a JavaScript function body that will be used inside `new Function("...args", <your code>)`.',
            'The function receives user arguments via the `args` rest parameter (an array of strings).',
            'Return null to explicitly do nothing.',
            returnSpec,
            '',
            'Respond with only the raw code. No explanation, no markdown fences, no surrounding comments.',
            '</instructions>',
        ];

        if (currentScript) {
            parts.push('', '<current_script>', currentScript, '</current_script>');
        }

        parts.push('', '<task>' + userPrompt + '</task>');

        const fullPrompt = parts.join('\n');

        btn.disabled = true;
        btn.textContent = 'Generating...';
        statusEl.textContent = 'Sending to agent...';

        // Store current script for undo
        this._previousScript = document.getElementById('shortcutScript')?.value || '';
        document.getElementById('scriptAiUndo').style.display = 'none';

        try {
            const invoke = window.__TAURI__.core.invoke;
            const listen = window.__TAURI__.event.listen;

            // Collect streamed response
            let response = '';
            const unlisten = await listen('message_chunk', (event) => {
                response = event.payload;
                statusEl.textContent = 'Receiving...';
            });

            const completionPromise = new Promise((resolve) => {
                const unlistenComplete = listen('message_complete', () => {
                    unlistenComplete.then(fn => fn());
                    resolve();
                });
            });

            await invoke('send_message_streaming', { message: fullPrompt, attachments: null });
            await completionPromise;
            unlisten();

            // Extract code — strip markdown fences if present
            let code = response.trim();
            const fenceMatch = code.match(/```(?:javascript|js)?\s*\n([\s\S]*?)```/);
            if (fenceMatch) code = fenceMatch[1].trim();
            // Also strip bare ``` at start/end
            code = code.replace(/^```\w*\n?/, '').replace(/\n?```$/, '').trim();

            const textarea = document.getElementById('shortcutScript');
            if (textarea) {
                textarea.value = code;
                this.updateHighlight();
            }
            statusEl.textContent = 'Script generated. Review and save.';
            document.getElementById('scriptAiUndo').style.display = '';
        } catch (e) {
            statusEl.textContent = 'Error: ' + e;
        } finally {
            btn.disabled = false;
            btn.textContent = 'Generate';
        }
    }

    undoGenerate() {
        if (this._previousScript == null) return;
        const textarea = document.getElementById('shortcutScript');
        if (textarea) {
            textarea.value = this._previousScript;
            this.updateHighlight();
        }
        document.getElementById('scriptAiUndo').style.display = 'none';
        document.getElementById('scriptAiStatus').textContent = 'Reverted to previous script.';
        this._previousScript = null;
    }

    closeDialog() {
        document.getElementById('shortcutDialog').style.display = 'none';
        this.editingIndex = -1;
    }

    saveShortcut() {
        const name = document.getElementById('shortcutName').value.trim();
        const trigger = document.getElementById('shortcutTrigger').value.trim();
        const actionType = document.getElementById('shortcutActionType').value;

        if (!name || !trigger) { alert('Name and Trigger Word are required.'); return; }

        const shortcut = { name, shortcut: trigger, action_type: actionType };
        if (this._currentIcon) shortcut.icon = this._currentIcon;

        if (actionType === 'open_url') {
            const url = document.getElementById('shortcutUrl').value.trim();
            if (!url) { alert('URL is required.'); return; }
            shortcut.url = url;
        } else if (actionType === 'prompt') {
            const prompt = document.getElementById('shortcutPrompt').value.trim();
            if (!prompt) { alert('Prompt template is required.'); return; }
            shortcut.prompt = prompt;
        } else if (actionType === 'script') {
            const script = document.getElementById('shortcutScript').value.trim();
            if (!script) { alert('Script body is required.'); return; }
            shortcut.script = script;
            shortcut.script_action = document.getElementById('shortcutScriptAction').value;
            // Validate script syntax
            try { new Function('...args', script); }
            catch (e) { alert('Script syntax error: ' + e.message); return; }
        } else {
            const path = document.getElementById('shortcutPath').value.trim();
            if (!path) { alert('Executable Path is required.'); return; }
            shortcut.path = path;
            const workDir = document.getElementById('shortcutWorkDir').value.trim();
            const args = document.getElementById('shortcutArgs').value.trim();
            if (workDir) shortcut.working_directory = workDir;
            if (args) shortcut.arguments = args;
        }

        if (this.editingIndex >= 0) {
            this.shortcuts[this.editingIndex] = shortcut;
        } else {
            this.shortcuts.push(shortcut);
        }
        this.renderShortcutsList();
        this.closeDialog();
    }

    deleteShortcut(index) {
        if (confirm('Delete this shortcut?')) {
            this.shortcuts.splice(index, 1);
            this.renderShortcutsList();
        }
    }

    exportShortcuts() {
        const blob = new Blob([JSON.stringify(this.shortcuts, null, 2)], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url; a.download = 'kiro-shortcuts.json'; a.click();
        URL.revokeObjectURL(url);
    }

    importShortcuts() {
        const input = document.createElement('input');
        input.type = 'file'; input.accept = 'application/json';
        input.onchange = (e) => {
            const file = e.target.files[0];
            if (!file) return;
            const reader = new FileReader();
            reader.onload = (ev) => {
                try {
                    const imported = JSON.parse(ev.target.result);
                    if (!Array.isArray(imported)) { alert('Invalid format.'); return; }
                    this.shortcuts = imported;
                    this.renderShortcutsList();
                } catch (err) { alert('Failed to parse: ' + err.message); }
            };
            reader.readAsText(file);
        };
        input.click();
    }

    destroy() {
        if (this._escHandler) document.removeEventListener('keydown', this._escHandler, true);
        delete window.shortcutsModule;
    }
}
