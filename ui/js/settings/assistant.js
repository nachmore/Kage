import { createLineEditor } from '../shared/line-editor.js';
import { registerSettingsActions } from './module-registry.js';
import { SettingsModule } from './base.js';

/**
 * Personalization settings module.
 *
 * Has two sub-views inside the same section:
 *   - main: toggles + buttons that launch the editors
 *   - editor: line-by-line editor for either steering doc, with a
 *     back button (top), Save (bottom), and (for the user doc only)
 *     an Import button that seeds the editor from any markdown file
 *     the user picks via the file dialog.
 *
 * One shared editor slot is reused for both kinds — re-populating
 * the same DOM each time is simpler than maintaining two parallel
 * trees, and the user only ever edits one doc at a time anyway.
 *
 * Quick Actions remain in this section under the steering blocks.
 * Their UI is unchanged from the previous module so the prior IA
 * (everything personal-to-me lives in Personalization) is preserved.
 */
export class AssistantSettingsModule extends SettingsModule {
    constructor() {
        super('personalization', 'Personalization', '✨');
        // Active LineEditor controller, or null when on the main view.
        this._editor = null;
        // Steering kind currently being edited ('auto' | 'user').
        this._editingKind = null;
        // Resolved on-disk path of the file being edited — surfaced in the UI.
        this._editingPath = '';
        // Set of action handlers we register on first initialize().
        // Idempotent so a re-render doesn't double-register.
        this._actionsRegistered = false;
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <div id="personalization-main">
                    <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                    <div class="setting-row">
                        <div class="setting-label">Learn my preferences</div>
                        <div class="setting-checkbox-row">
                            <label class="kage-checkbox">
                                <input type="checkbox" id="autoSteeringEnabled">
                            </label>
                            <div class="setting-description">Let Kage learn your preferences from conversations and remember them across sessions. You can view and edit what it learns at any time. This data stays on your machine and is only shared with your chosen agent.</div>
                        </div>
                        <div class="setting-control" style="margin-top: 8px;">
                            <button class="setting-button" data-action="openSteeringEditor" data-arg="auto">View Learned Preferences</button>
                        </div>
                    </div>

                    <div class="setting-row">
                        <div class="setting-label">Custom steering document</div>
                        <div class="setting-description">Add your own persistent instructions. Edited line by line — reorder, add, or delete bullets. Always included alongside learned preferences and never modified by Kage.</div>
                        <div class="setting-control" style="margin-top: 8px; display:flex; gap:8px;">
                            <button class="setting-button" data-action="openSteeringEditor" data-arg="user">Edit custom steering</button>
                        </div>
                    </div>

                    <div class="setting-section-label">Quick Actions</div>

                    ${this.createCheckboxRow(
                        'Show quick actions on responses',
                        'Show context-aware action chips after agent responses.',
                        'showResponseActions',
                        true
                    )}

                    ${this.createCheckboxRow(
                        'Show quick actions on selected text',
                        'When you summon Kage with text selected, show smart action chips (Summarize, Translate, Explain code, etc.) based on the content type.',
                        'quickActionsEnabled',
                        true
                    )}

                    <div class="setting-row">
                        <div class="setting-label">Translate language</div>
                        <div class="setting-description" id="translateLanguageDesc">Default target language for the Translate action. Leave empty to use the system default.</div>
                        <div class="setting-control">
                            <input type="text" class="setting-input" id="translateLanguage" placeholder="e.g., English, Spanish, Japanese" style="max-width: 250px;">
                        </div>
                    </div>

                    <div class="setting-row">
                        <div class="setting-label">Custom actions</div>
                        <div class="setting-description">Add your own quick actions. Use <code>{text}</code> in the prompt as a placeholder for the selected text.</div>
                        <div id="customActionsContainer" style="margin-top: 8px;"></div>
                        <button class="setting-button" id="addCustomActionBtn" style="margin-top: 8px;">+ Add Action</button>
                    </div>
                </div>

                <div id="personalization-editor" hidden>
                    <div style="display:flex;align-items:center;gap:8px;margin-bottom:8px;">
                        <button class="setting-button" data-action="closeSteeringEditor" style="background:transparent;color:var(--kage-text-secondary);border:1px solid var(--kage-border);">‹ Back</button>
                        <h2 class="settings-section-header" id="personalization-editor-title" style="margin:0;flex:1;"></h2>
                    </div>
                    <div class="setting-description" id="personalization-editor-subtitle" style="margin-bottom:6px;"></div>
                    <div class="setting-description" id="personalization-editor-path" style="font-size:11px;opacity:0.75;margin-bottom:8px;word-break:break-all;"></div>
                    <div id="personalization-editor-actions" style="display:flex;gap:8px;margin-bottom:10px;">
                        <button class="setting-button" data-action="importSteering" id="personalization-editor-importBtn" style="display:none;">Import…</button>
                        <button class="setting-button" data-action="revealSteering" id="personalization-editor-revealBtn" style="background:transparent;color:var(--kage-text-secondary);border:1px solid var(--kage-border);">Reveal in file explorer</button>
                    </div>
                    <div id="personalization-editor-mount"></div>
                    <div style="display:flex;gap:8px;margin-top:12px;">
                        <button class="setting-button" data-action="saveSteering">Save</button>
                        <button class="setting-button" data-action="closeSteeringEditor" style="background:transparent;color:var(--kage-text-secondary);border:1px solid var(--kage-border);">Cancel</button>
                    </div>
                    <div id="personalization-editor-status" class="setting-description" style="margin-top:8px;min-height:1em;"></div>
                </div>
            </div>
        `;
    }

    load(config) {
        const agentCfg = config.acp?.agent || {};
        const autoSteering = document.getElementById('autoSteeringEnabled');
        if (autoSteering) autoSteering.checked = agentCfg.auto_steering_enabled || false;

        // Quick actions
        const qaEnabled = document.getElementById('quickActionsEnabled');
        const qa = config.quick_actions || { enabled: true, custom_actions: [] };
        if (qaEnabled) qaEnabled.checked = qa.enabled !== false;
        const showResponseActions = document.getElementById('showResponseActions');
        if (showResponseActions)
            showResponseActions.checked = config.ui?.show_response_actions !== false;
        const translateLang = document.getElementById('translateLanguage');
        if (translateLang) translateLang.value = qa.translate_language || '';
        this._renderCustomActions(qa.custom_actions || []);

        // Switching to this section while the editor is open is unusual but
        // possible if the user clicked the sidebar — drop back to main view.
        this._showMainView();
    }

    save(config) {
        if (!config.acp) config.acp = {};
        if (!config.acp.agent) config.acp.agent = {};
        config.acp.agent.auto_steering_enabled =
            document.getElementById('autoSteeringEnabled').checked;
        // user_steering_path is owned by write_steering_lines now — first
        // non-empty save pins the resolved default into config. We never
        // overwrite it from the UI here, so a hand-edited config.json
        // pointing at a custom path keeps working.

        // Quick actions
        config.quick_actions = config.quick_actions || {};
        config.quick_actions.enabled =
            document.getElementById('quickActionsEnabled')?.checked ?? true;
        config.quick_actions.translate_language =
            document.getElementById('translateLanguage')?.value?.trim() || null;
        config.quick_actions.custom_actions = this._collectCustomActions();
        // Response actions (stored in ui config)
        config.ui = config.ui || {};
        config.ui.show_response_actions =
            document.getElementById('showResponseActions')?.checked ?? true;
    }

    initialize() {
        // Add custom action button
        const addBtn = document.getElementById('addCustomActionBtn');
        if (addBtn) {
            addBtn.addEventListener('click', () => this._addCustomActionRow());
        }

        // Show system default language in translate description
        const translateDesc = document.getElementById('translateLanguageDesc');
        if (translateDesc) {
            try {
                const locale = navigator.language || 'en';
                let langName = 'English';
                if (typeof Intl !== 'undefined' && Intl.DisplayNames) {
                    const display = new Intl.DisplayNames(['en'], { type: 'language' });
                    const name = display.of(locale);
                    if (name) langName = name.charAt(0).toUpperCase() + name.slice(1);
                }
                translateDesc.textContent = `Default target language for the Translate action. Leave empty to use the system default (${langName}).`;
            } catch {}
        }

        if (this._actionsRegistered) return;
        this._actionsRegistered = true;
        // The action dispatcher is global and shared across all settings
        // modules — handlers are keyed by name so registering twice from
        // a re-render would just overwrite-with-same-thing, but we cheap-
        // skip to avoid surprise on future refactors.
        registerSettingsActions({
            openSteeringEditor: (kind) => {
                this._openEditor(kind === 'auto' ? 'auto' : 'user');
            },
            closeSteeringEditor: () => this._showMainView(),
            saveSteering: () => this._saveCurrentEditor(),
            importSteering: () => this._importIntoEditor(),
            revealSteering: () => this._revealCurrentFile(),
        });
    }

    // --- editor view machinery ------------------------------------------

    _showMainView() {
        const main = document.getElementById('personalization-main');
        const editor = document.getElementById('personalization-editor');
        if (main) main.hidden = false;
        if (editor) editor.hidden = true;
        if (this._editor) {
            try {
                this._editor.destroy();
            } catch {}
            this._editor = null;
        }
        this._editingKind = null;
        this._editingPath = '';
        this._setStatus('');
    }

    async _openEditor(kind) {
        const main = document.getElementById('personalization-main');
        const editor = document.getElementById('personalization-editor');
        const titleEl = document.getElementById('personalization-editor-title');
        const subtitle = document.getElementById('personalization-editor-subtitle');
        const pathEl = document.getElementById('personalization-editor-path');
        const importBtn = document.getElementById('personalization-editor-importBtn');
        const mount = document.getElementById('personalization-editor-mount');
        if (!editor || !mount) return;

        this._editingKind = kind;

        // Title + copy + import-button visibility differ by kind. Auto is
        // a "what Kage learned about me" view; user is a "my custom rules"
        // view. The Import button is user-only — importing into an
        // auto-generated doc would just be overwritten on the next
        // generation cycle.
        if (titleEl) {
            titleEl.textContent =
                kind === 'auto' ? 'Learned Preferences' : 'Custom steering document';
        }
        if (subtitle) {
            subtitle.textContent =
                kind === 'auto'
                    ? 'Edit, reorder, or remove what Kage has learned about you. Saved changes apply to new chats.'
                    : 'Your persistent instructions to Kage. Edited line by line.';
        }
        if (importBtn) {
            importBtn.style.display = kind === 'user' ? '' : 'none';
        }

        this._setStatus('Loading…');
        let result;
        try {
            result = await window.__TAURI__.core.invoke('read_steering_lines', { kind });
        } catch (error) {
            this._setStatus('Failed to read: ' + this._formatError(error), 'error');
            // Still flip to the editor view so the user can hit Back.
            if (main) main.hidden = true;
            editor.hidden = false;
            return;
        }
        this._editingPath = result?.path || '';
        if (pathEl) pathEl.textContent = this._editingPath ? 'Saved at: ' + this._editingPath : '';
        this._setStatus(
            result?.exists === false ? 'No file yet — start typing to create one.' : ''
        );

        if (this._editor) {
            try {
                this._editor.destroy();
            } catch {}
        }
        this._editor = createLineEditor(mount, {
            lines: Array.isArray(result?.lines) ? result.lines : [],
            emptyHint:
                kind === 'auto'
                    ? 'Nothing learned yet — chat with Kage and your preferences will start to appear here.'
                    : 'Empty — start with a line, then Save.',
            rowPlaceholder: kind === 'auto' ? 'Preference or fact' : 'Instruction',
        });

        if (main) main.hidden = true;
        editor.hidden = false;
    }

    async _saveCurrentEditor() {
        if (!this._editor || !this._editingKind) return;
        const lines = this._editor
            .getLines()
            // Drop trailing blank rows the user added but didn't fill in.
            // Interior blanks are preserved (paragraph breaks survive).
            .reduce((acc, line) => {
                if (line === '') {
                    acc._pendingBlanks = (acc._pendingBlanks || 0) + 1;
                } else {
                    while (acc._pendingBlanks > 0) {
                        acc.push('');
                        acc._pendingBlanks -= 1;
                    }
                    acc.push(line);
                }
                return acc;
            }, []);

        this._setStatus('Saving…');
        try {
            await window.__TAURI__.core.invoke('write_steering_lines', {
                kind: this._editingKind,
                lines,
            });
            this._setStatus('Saved.', 'success');
        } catch (error) {
            this._setStatus('Failed to save: ' + this._formatError(error), 'error');
        }
    }

    async _importIntoEditor() {
        if (!this._editor || this._editingKind !== 'user') return;
        let chosen;
        try {
            chosen = await window.__TAURI__.dialog.open({
                multiple: false,
                directory: false,
                filters: [
                    { name: 'Markdown', extensions: ['md', 'markdown', 'txt'] },
                    { name: 'All files', extensions: ['*'] },
                ],
            });
        } catch (error) {
            this._setStatus('Import cancelled: ' + this._formatError(error), 'error');
            return;
        }
        if (!chosen || typeof chosen !== 'string') return;

        this._setStatus('Importing…');
        try {
            const lines = await window.__TAURI__.core.invoke('import_steering_lines', {
                path: chosen,
            });
            this._editor.setLines(Array.isArray(lines) ? lines : []);
            this._setStatus('Imported. Click Save to keep these lines.', 'success');
        } catch (error) {
            this._setStatus('Import failed: ' + this._formatError(error), 'error');
        }
    }

    async _revealCurrentFile() {
        if (!this._editingPath) return;
        try {
            await window.__TAURI__.core.invoke('open_path', { path: this._editingPath });
        } catch (error) {
            this._setStatus('Could not open file location: ' + this._formatError(error), 'error');
        }
    }

    _setStatus(text, kind) {
        const el = document.getElementById('personalization-editor-status');
        if (!el) return;
        el.textContent = text || '';
        el.style.color = kind === 'error' ? '#c44' : kind === 'success' ? 'var(--kage-accent)' : '';
    }

    _formatError(e) {
        if (!e) return 'Unknown error';
        if (typeof e === 'string') return e;
        if (e instanceof Error) return e.message || String(e);
        if (typeof e === 'object') {
            if (typeof e.message === 'string' && e.message) return e.message;
            try {
                return JSON.stringify(e);
            } catch {
                return String(e);
            }
        }
        return String(e);
    }

    // --- quick actions (unchanged) --------------------------------------

    _renderCustomActions(actions) {
        const container = document.getElementById('customActionsContainer');
        if (!container) return;
        container.innerHTML = '';
        for (const action of actions) {
            this._addCustomActionRow(action);
        }
    }

    _addCustomActionRow(action = null) {
        const container = document.getElementById('customActionsContainer');
        if (!container) return;

        const row = document.createElement('div');
        row.className = 'custom-action-row';
        row.style.cssText = 'display:flex;gap:8px;align-items:center;margin-bottom:6px;';
        row.innerHTML = `
            <input type="text" class="setting-input ca-icon" placeholder="📝" value="${action?.icon || ''}" style="width:40px;text-align:center;">
            <input type="text" class="setting-input ca-label" placeholder="Label" value="${action?.label || ''}" style="width:100px;">
            <input type="text" class="setting-input ca-prompt" placeholder="Prompt ({text} = selection)" value="${(action?.prompt || '').replace(/"/g, '&quot;')}" style="flex:1;">
            <button class="setting-button ca-remove" style="padding:4px 8px;">✕</button>
        `;
        row.querySelector('.ca-remove').addEventListener('click', () => row.remove());
        container.appendChild(row);
    }

    _collectCustomActions() {
        const container = document.getElementById('customActionsContainer');
        if (!container) return [];
        const actions = [];
        for (const row of container.querySelectorAll('.custom-action-row')) {
            const label = row.querySelector('.ca-label')?.value?.trim();
            const prompt = row.querySelector('.ca-prompt')?.value?.trim();
            if (label && prompt) {
                actions.push({
                    label,
                    icon: row.querySelector('.ca-icon')?.value?.trim() || '⚡',
                    prompt,
                    content_types: [],
                });
            }
        }
        return actions;
    }
}
