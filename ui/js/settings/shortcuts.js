import { SettingsModule } from './base.js';
import { summarizeNamedPlaceholders } from '../shared/shortcuts.js';
import { renderShortcutsListView, renderShortcutsSettings } from './shortcuts-view.js';
import { t } from '../shared/i18n.js';
import { registerSettingsActions } from './module-registry.js';
import { errLabel } from '../shared/error-message.js';
/**
 * Commands & Prompts settings module.
 *
 * Originally just "Quick Commands" — extended with named-placeholder
 * support so the same machinery powers persistent prompt templates
 * (e.g. `tr {lang}: {*}` for translate-to-X). Section title and
 * sidebar label updated to reflect the broader purpose; the underlying
 * config field is still `shortcuts` and the section id still
 * `shortcuts` so existing config files / deep links keep working.
 */
export class ShortcutsSettingsModule extends SettingsModule {
    constructor() {
        super('shortcuts', t('settings.shortcuts.title'), '⚡');
        this.shortcuts = [];
        this.editingIndex = -1;
    }

    render() {
        return renderShortcutsSettings(this);
    }

    initialize() {
        this.renderShortcutsList();
        this._registerActions();
        this._wireDialogListeners();
        this._escHandler = (e) => {
            if (
                e.key === 'Escape' &&
                document.getElementById('shortcutDialog')?.style.display === 'flex'
            ) {
                e.stopPropagation();
                this.closeDialog();
            }
        };
        document.addEventListener('keydown', this._escHandler, true);

        // Paste image into icon field (works on both the emoji input and the preview)
        const emojiInput = document.getElementById('shortcutIconEmoji');
        const iconPreview = document.getElementById('shortcutIconPreview');
        const handleIconPaste = (e) => {
            const items = e.clipboardData?.items;
            if (!items) return;
            for (const item of items) {
                if (item.type.startsWith('image/')) {
                    e.preventDefault();
                    const file = item.getAsFile();
                    if (!file || file.size > 65536) return;
                    const reader = new FileReader();
                    reader.onload = (ev) => this._setIconPreview(ev.target.result);
                    reader.readAsDataURL(file);
                    return;
                }
            }
        };
        if (emojiInput) emojiInput.addEventListener('paste', handleIconPaste);
        if (iconPreview) {
            iconPreview.setAttribute('tabindex', '0');
            iconPreview.addEventListener('paste', handleIconPaste);
        }
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
        return renderShortcutsListView(this);
    }

    showAddDialog() {
        this.editingIndex = -1;
        document.getElementById('dialogTitle').textContent = t(
            'settings.shortcuts.dialog.add_title'
        );
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
        const aiPromptEl = document.getElementById('scriptAiPrompt');
        aiPromptEl.value = '';
        // Reset auto-grown height back to a single row.
        aiPromptEl.style.height = '';
        aiPromptEl.style.overflowY = 'hidden';
        document.getElementById('scriptAiStatus').textContent = '';
        document.getElementById('scriptAiUndo').style.display = 'none';
        this._previousScript = null;
        this._setIconPreview('');
        this.onActionTypeChange();
        this._resetTestSection();
        this._wirePlaceholderPreview();
        this._renderPlaceholderChips();
        document.getElementById('shortcutDialog').style.display = 'flex';
    }

    editShortcut(index) {
        this.editingIndex = index;
        const s = this.shortcuts[index];
        document.getElementById('dialogTitle').textContent = t(
            'settings.shortcuts.dialog.edit_title'
        );
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
        const aiPromptEl = document.getElementById('scriptAiPrompt');
        aiPromptEl.value = '';
        // Reset auto-grown height back to a single row.
        aiPromptEl.style.height = '';
        aiPromptEl.style.overflowY = 'hidden';
        document.getElementById('scriptAiStatus').textContent = '';
        document.getElementById('scriptAiUndo').style.display = 'none';
        this._previousScript = null;
        this._setIconPreview(s.icon || '');
        this.onActionTypeChange();
        this._resetTestSection();
        this._wirePlaceholderPreview();
        this._renderPlaceholderChips();
        document.getElementById('shortcutDialog').style.display = 'flex';
        if ((s.action_type || 'run_program') === 'script') this.updateHighlight();
    }

    /**
     * Wire the prompt textarea so the placeholder chips below it
     * refresh as the user types. Idempotent — only attaches once per
     * dialog instance via the `_placeholderListenerAttached` flag.
     */
    _wirePlaceholderPreview() {
        if (this._placeholderListenerAttached) return;
        const textarea = document.getElementById('shortcutPrompt');
        if (!textarea) return;
        textarea.addEventListener('input', () => this._renderPlaceholderChips());
        this._placeholderListenerAttached = true;
    }

    _renderPlaceholderChips() {
        const container = document.getElementById('shortcutPromptPlaceholders');
        const textarea = document.getElementById('shortcutPrompt');
        if (!container || !textarea) return;
        const named = summarizeNamedPlaceholders(textarea.value);
        if (named.length === 0) {
            container.style.display = 'none';
            container.innerHTML = '';
            return;
        }
        container.style.display = '';
        const label =
            '<span class="prompt-placeholder-label">Will ask for:</span>' +
            named
                .map(
                    (p) =>
                        `<span class="prompt-placeholder-chip${p.optional ? ' optional' : ''}">${escapeHtml(p.name)}${p.optional ? '?' : ''}</span>`
                )
                .join('');
        container.innerHTML = label;
    }

    onActionTypeChange() {
        const at = document.getElementById('shortcutActionType').value;
        document.getElementById('runProgramFields').style.display =
            at === 'run_program' ? 'block' : 'none';
        document.getElementById('openUrlFields').style.display =
            at === 'open_url' ? 'block' : 'none';
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

        if (icon?.startsWith('data:')) {
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
            alert(t('settings.shortcuts.alert.icon_too_large'));
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
        if (!url) {
            alert(t('settings.shortcuts.alert.url_first'));
            return;
        }

        const btn = document.getElementById('shortcutFaviconBtn');
        const origText = btn?.textContent;
        if (btn) btn.textContent = t('settings.shortcuts.favicon_fetching');

        try {
            const invoke = window.__TAURI__.core.invoke;
            const dataUri = await invoke('fetch_favicon', { url });
            this._setIconPreview(dataUri);
        } catch (e) {
            console.warn('Favicon fetch failed:', e);
            alert(t('settings.shortcuts.alert.no_favicon'));
        } finally {
            if (btn) btn.textContent = origText;
        }
    }

    /** Clear the custom icon */
    clearIcon() {
        this._setIconPreview('');
    }

    toggleTestSection() {
        const body = document.getElementById('shortcutTestBody');
        const toggle = document.getElementById('shortcutTestToggle');
        if (!body) return;
        const show = body.style.display === 'none';
        body.style.display = show ? '' : 'none';
        if (toggle) toggle.textContent = show ? '▼' : '▶';
    }

    _resetTestSection() {
        const args = document.getElementById('shortcutTestArgs');
        const output = document.getElementById('shortcutTestOutput');
        const body = document.getElementById('shortcutTestBody');
        const toggle = document.getElementById('shortcutTestToggle');
        if (args) args.value = '';
        if (output) {
            output.style.display = 'none';
            output.textContent = '';
            output.className = 'shortcut-test-output';
        }
        if (body) body.style.display = 'none';
        if (toggle) toggle.textContent = '▶';
    }

    /** Build a temporary shortcut from the current dialog fields and dry-run it */
    runTest() {
        const output = document.getElementById('shortcutTestOutput');
        if (!output) return;

        const actionType = document.getElementById('shortcutActionType').value;
        const testInput = document.getElementById('shortcutTestArgs')?.value || '';
        const args = testInput.trim() ? testInput.trim().split(/\s+/) : [];

        // Build a temporary shortcut object from dialog fields
        const sc = {
            name: document.getElementById('shortcutName').value || 'test',
            shortcut: document.getElementById('shortcutTrigger').value || 'test',
            action_type: actionType,
            path: document.getElementById('shortcutPath').value || '',
            url: document.getElementById('shortcutUrl').value || '',
            prompt: document.getElementById('shortcutPrompt').value || '',
            script: document.getElementById('shortcutScript').value || '',
            script_action: document.getElementById('shortcutScriptAction').value || 'text',
            arguments: document.getElementById('shortcutArgs').value || '',
            working_directory: document.getElementById('shortcutWorkDir').value || '',
        };

        try {
            // Import buildShortcutCommand dynamically to avoid circular deps
            import('../shared/shortcuts.js').then(({ buildShortcutCommand }) => {
                const cmd = buildShortcutCommand(sc, args, '');
                output.style.display = 'block';
                output.className = 'shortcut-test-output';

                if (cmd.type === 'error') {
                    output.textContent = '✗ ' + cmd.message;
                    output.classList.add('test-error');
                    return;
                }

                output.classList.add('test-success');
                let text = '';
                if (cmd.type === 'open_url') {
                    text = '🌐 URL: ' + cmd.url;
                } else if (cmd.type === 'prompt') {
                    text = '💬 Prompt: ' + cmd.message;
                } else if (cmd.type === 'text') {
                    text = '📝 Output: ' + cmd.message;
                } else if (cmd.type === 'run_program') {
                    text = '▶️ Run: ' + cmd.path;
                    if (cmd.args?.length) text += '\n   Args: ' + cmd.args.join(' ');
                    if (cmd.workDir) text += '\n   Dir: ' + cmd.workDir;
                } else if (cmd.type === 'noop') {
                    text = '(no output — script returned null/undefined)';
                } else {
                    text = JSON.stringify(cmd, null, 2);
                }
                output.textContent = text;
            });
        } catch (e) {
            output.style.display = 'block';
            output.className = 'shortcut-test-output test-error';
            output.textContent = '✗ ' + e.message;
        }
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
        if (!userPrompt) {
            statusEl.textContent = t('settings.shortcuts.script_ai.empty_prompt');
            return;
        }

        const scriptAction = document.getElementById('shortcutScriptAction')?.value || 'text';
        const currentScript = document.getElementById('shortcutScript')?.value.trim() || '';

        // Build action-specific return format hints
        let returnSpec;
        if (scriptAction === 'run_program') {
            returnSpec =
                'Return an array: [command, workingDirectory, ...args]. workingDirectory can be an empty string for the default directory. Example: return ["git", "C:\\\\projects", "status"];';
        } else if (scriptAction === 'open_url') {
            returnSpec =
                'Return a string containing a valid URL. Example: return "https://example.com/search?q=" + encodeURIComponent(args[0]);';
        } else if (scriptAction === 'prompt') {
            returnSpec =
                'Return a string that will be sent to an AI agent as a prompt. Example: return "Explain this error: " + args.join(" ");';
        } else {
            returnSpec =
                'Return a string that will be displayed to the user. Example: return "Result: " + args.join(", ");';
        }

        const parts = [
            '<role>You are a JavaScript code generator for Kage shortcut scripts.</role>',
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
        btn.textContent = t('settings.shortcuts.script_ai.btn.generating');
        statusEl.textContent = t('settings.shortcuts.script_ai.status.sending');

        // Store current script for undo
        this._previousScript = document.getElementById('shortcutScript')?.value || '';
        document.getElementById('scriptAiUndo').style.display = 'none';

        try {
            const invoke = window.__TAURI__.core.invoke;

            // Generation runs on a throwaway backend session
            // (generate_script → ephemeral_session), so it works even when
            // Settings is open with no chat window and never pollutes the
            // user's real conversation. Blocks until the agent finishes
            // and returns the full reply.
            statusEl.textContent = t('settings.shortcuts.script_ai.status.receiving');
            const response = await invoke('generate_script', { prompt: fullPrompt });

            // Extract code — strip markdown fences if present
            let code = (response || '').trim();
            const fenceMatch = code.match(/```(?:javascript|js)?\s*\n([\s\S]*?)```/);
            if (fenceMatch) code = fenceMatch[1].trim();
            // Also strip bare ``` at start/end
            code = code
                .replace(/^```\w*\n?/, '')
                .replace(/\n?```$/, '')
                .trim();

            const textarea = document.getElementById('shortcutScript');
            if (textarea) {
                textarea.value = code;
                this.updateHighlight();
            }
            statusEl.textContent = t('settings.shortcuts.script_ai.status.generated');
            document.getElementById('scriptAiUndo').style.display = '';
        } catch (e) {
            statusEl.textContent = errLabel(t('settings.shortcuts.script_ai.error.label'), e);
        } finally {
            btn.disabled = false;
            btn.textContent = t('settings.shortcuts.script_ai.btn.generate');
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
        document.getElementById('scriptAiStatus').textContent = t(
            'settings.shortcuts.script_ai.status.reverted'
        );
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

        if (!name || !trigger) {
            alert(t('settings.shortcuts.alert.name_required'));
            return;
        }

        const shortcut = { name, shortcut: trigger, action_type: actionType };
        if (this._currentIcon) shortcut.icon = this._currentIcon;

        if (actionType === 'open_url') {
            const url = document.getElementById('shortcutUrl').value.trim();
            if (!url) {
                alert(t('settings.shortcuts.alert.url_required'));
                return;
            }
            shortcut.url = url;
        } else if (actionType === 'prompt') {
            const prompt = document.getElementById('shortcutPrompt').value.trim();
            if (!prompt) {
                alert(t('settings.shortcuts.alert.prompt_required'));
                return;
            }
            shortcut.prompt = prompt;
        } else if (actionType === 'script') {
            const script = document.getElementById('shortcutScript').value.trim();
            if (!script) {
                alert(t('settings.shortcuts.alert.script_required'));
                return;
            }
            shortcut.script = script;
            shortcut.script_action = document.getElementById('shortcutScriptAction').value;
            // Validate script syntax
            try {
                new Function('...args', script);
            } catch (e) {
                alert(t('settings.shortcuts.alert.script_syntax_error', { reason: e.message }));
                return;
            }
        } else {
            const path = document.getElementById('shortcutPath').value.trim();
            if (!path) {
                alert(t('settings.shortcuts.alert.path_required'));
                return;
            }
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
        if (confirm(t('settings.shortcuts.delete_confirm'))) {
            this.shortcuts.splice(index, 1);
            this.renderShortcutsList();
        }
    }

    exportShortcuts() {
        const blob = new Blob([JSON.stringify(this.shortcuts, null, 2)], {
            type: 'application/json',
        });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = 'kage-shortcuts.json';
        a.click();
        URL.revokeObjectURL(url);
    }

    importShortcuts() {
        const input = document.createElement('input');
        input.type = 'file';
        input.accept = 'application/json';
        input.onchange = (e) => {
            const file = e.target.files[0];
            if (!file) return;
            const reader = new FileReader();
            reader.onload = (ev) => {
                try {
                    const imported = JSON.parse(ev.target.result);
                    if (!Array.isArray(imported)) {
                        alert(t('settings.shortcuts.import.invalid_format'));
                        return;
                    }
                    this.shortcuts = imported;
                    this.renderShortcutsList();
                } catch (err) {
                    alert(t('settings.shortcuts.import.parse_failed', { message: err.message }));
                }
            };
            reader.readAsText(file);
        };
        input.click();
    }

    destroy() {
        if (this._escHandler) document.removeEventListener('keydown', this._escHandler, true);
    }

    // Wire input/change/keydown listeners that previously lived as inline
    // attributes (oninput=, onkeydown=). These are non-trivial — they
    // either need to read event.key or call a method on this — so they
    // can't go through the data-action dispatcher cleanly. addEventListener
    // is the appropriate tool here.
    _wireDialogListeners() {
        const emoji = document.getElementById('shortcutIconEmoji');
        if (emoji) emoji.addEventListener('input', () => this.onIconInput());

        const iconFile = document.getElementById('shortcutIconFile');
        if (iconFile) iconFile.addEventListener('change', (e) => this.onIconFileSelected(e));

        const script = document.getElementById('shortcutScript');
        if (script) script.addEventListener('input', () => this.updateHighlight());

        const aiPrompt = document.getElementById('scriptAiPrompt');
        if (aiPrompt) {
            // Auto-grow up to ~5 lines, then scroll internally. Reset to
            // auto first so the field shrinks on delete, and account for
            // the border (box-sizing:border-box) so we don't leave a
            // phantom scrollbar.
            const MAX_AI_PROMPT_PX = 96;
            const autoGrow = () => {
                aiPrompt.style.height = 'auto';
                const cs = getComputedStyle(aiPrompt);
                const border =
                    parseFloat(cs.borderTopWidth || '0') + parseFloat(cs.borderBottomWidth || '0');
                const content = aiPrompt.scrollHeight + border;
                aiPrompt.style.height = Math.min(content, MAX_AI_PROMPT_PX) + 'px';
                aiPrompt.style.overflowY = content > MAX_AI_PROMPT_PX ? 'auto' : 'hidden';
            };
            aiPrompt.addEventListener('input', autoGrow);
            aiPrompt.addEventListener('keydown', (e) => {
                // Enter submits; Shift+Enter inserts a newline.
                if (e.key === 'Enter' && !e.shiftKey) {
                    e.preventDefault();
                    this.generateScript();
                }
            });
        }

        const testArgs = document.getElementById('shortcutTestArgs');
        if (testArgs)
            testArgs.addEventListener('keydown', (e) => {
                if (e.key === 'Enter') {
                    e.preventDefault();
                    this.runTest();
                }
            });
    }

    // Register every data-action="shortcuts.*" handler with the dispatcher.
    // This replaces the previous pattern of inline onclick="shortcutsModule.X()"
    // attributes — that pattern coupled rendered HTML to a window global and
    // forced eval-style attribute strings (which CSP and content-sniffing
    // checkers flag).
    _registerActions() {
        registerSettingsActions({
            'shortcuts.showAddDialog': () => this.showAddDialog(),
            'shortcuts.openCommandsStore': () => {
                if (window.__TAURI__?.core) {
                    window.__TAURI__.core.invoke('open_store_window', { tab: 'commands' });
                }
            },
            'shortcuts.exportShortcuts': () => this.exportShortcuts(),
            'shortcuts.importShortcuts': () => this.importShortcuts(),
            'shortcuts.openFullBackup': (_arg, _el, ev) => {
                // Same routing the >logs command uses: switch the
                // settings sidebar to About, then dispatch a
                // settings-subsection event so the backup section
                // expands and scrolls into view.
                if (ev?.preventDefault) ev.preventDefault();
                window.dispatchSettingsAction?.('switchSection', 'about');
                document.dispatchEvent(
                    new CustomEvent('settings-subsection', { detail: 'backup' })
                );
            },
            'shortcuts.closeDialog': () => this.closeDialog(),
            'shortcuts.openIconFilePicker': () =>
                document.getElementById('shortcutIconFile')?.click(),
            'shortcuts.fetchFavicon': () => this.fetchFavicon(),
            'shortcuts.clearIcon': () => this.clearIcon(),
            'shortcuts.onActionTypeChange': () => this.onActionTypeChange(),
            'shortcuts.generateScript': () => this.generateScript(),
            'shortcuts.undoGenerate': () => this.undoGenerate(),
            'shortcuts.toggleTestSection': () => this.toggleTestSection(),
            'shortcuts.runTest': () => this.runTest(),
            'shortcuts.saveShortcut': () => this.saveShortcut(),
            'shortcuts.editShortcut': (arg) => this.editShortcut(parseInt(arg, 10)),
            'shortcuts.deleteShortcut': (arg) => this.deleteShortcut(parseInt(arg, 10)),
        });
    }
}
