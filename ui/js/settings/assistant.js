import { createLineEditor } from '../shared/line-editor.js';
import { t } from '../shared/i18n.js';
import { escapeAttr } from '../shared/tool-utils.js';
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
        super('personalization', t('settings.sidebar.personalization'), '✨');
        // Active LineEditor controller, or null when on the main view.
        this._editor = null;
        // Steering kind currently being edited ('auto' | 'user').
        this._editingKind = null;
        // Resolved on-disk path of the file being edited — surfaced in the UI.
        this._editingPath = '';
        // Set of action handlers we register on first initialize().
        // Idempotent so a re-render doesn't double-register.
        this._actionsRegistered = false;
        // Mechanical fields handled by the bind DSL. Custom logic
        // (context-rules snapshot, custom-actions list, post-load
        // view reset) stays in load()/save().
        this.bindFields([
            {
                id: 'autoSteeringEnabled',
                path: 'acp.agent.auto_steering_enabled',
                kind: 'checkbox',
                default: false,
            },
            {
                id: 'quickActionsEnabled',
                path: 'quick_actions.enabled',
                kind: 'checkbox',
                default: true,
            },
            {
                id: 'showResponseActions',
                path: 'ui.show_response_actions',
                kind: 'checkbox',
                default: true,
            },
            {
                id: 'translateLanguage',
                path: 'quick_actions.translate_language',
                kind: 'value',
                default: '',
            },
        ]);
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <div id="personalization-main">
                    <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                    <div class="setting-row">
                        <div class="setting-label">${t('settings.assistant.learn.label')}</div>
                        <div class="setting-checkbox-row">
                            <label class="kage-checkbox">
                                <input type="checkbox" id="autoSteeringEnabled">
                            </label>
                            <div class="setting-description">${t('settings.assistant.learn.description')}</div>
                        </div>
                        <div class="setting-control" style="margin-top: 8px;">
                            <button class="setting-button" data-action="openSteeringEditor" data-arg="auto">${t('settings.assistant.learn.view_btn')}</button>
                        </div>
                    </div>

                    <div class="setting-row">
                        <div class="setting-label">${t('settings.assistant.custom_steering.label')}</div>
                        <div class="setting-description">${t('settings.assistant.custom_steering.description')}</div>
                        <div class="setting-control" style="margin-top: 8px; display:flex; gap:8px;">
                            <button class="setting-button" data-action="openSteeringEditor" data-arg="user">${t('settings.assistant.custom_steering.edit_btn')}</button>
                        </div>
                    </div>

                    <div class="setting-row">
                        <div class="setting-label">${t('settings.assistant.app_modes.label')}</div>
                        <div class="setting-description">${t('settings.assistant.app_modes.description')}</div>
                        <div class="setting-control" style="margin-top: 8px; display:flex; gap:8px; align-items: center;">
                            <button class="setting-button" data-action="openAppModesEditor">${t('settings.assistant.app_modes.manage_btn')}</button>
                            <span id="appModesSummary" class="setting-description" style="margin: 0;"></span>
                        </div>
                    </div>

                    <div class="setting-section-label">${t('settings.assistant.quick_actions.section')}</div>

                    ${this.createCheckboxRow(
                        t('settings.assistant.quick_actions.show_responses.label'),
                        t('settings.assistant.quick_actions.show_responses.description'),
                        'showResponseActions',
                        true
                    )}

                    ${this.createCheckboxRow(
                        t('settings.assistant.quick_actions.show_selection.label'),
                        t('settings.assistant.quick_actions.show_selection.description'),
                        'quickActionsEnabled',
                        true
                    )}

                    <div class="setting-row">
                        <div class="setting-label">${t('settings.assistant.quick_actions.translate_lang.label')}</div>
                        <div class="setting-description" id="translateLanguageDesc">${t('settings.assistant.quick_actions.translate_lang.fallback_description')}</div>
                        <div class="setting-control">
                            <input type="text" class="setting-input" id="translateLanguage" placeholder="${t('settings.assistant.quick_actions.translate_lang.placeholder')}" style="max-width: 250px;">
                        </div>
                    </div>

                    <div class="setting-row">
                        <div class="setting-label">${t('settings.assistant.quick_actions.custom.label')}</div>
                        <div class="setting-description">${t('settings.assistant.quick_actions.custom.description_html')}</div>
                        <div id="customActionsContainer" style="margin-top: 8px;"></div>
                        <button class="setting-button" id="addCustomActionBtn" style="margin-top: 8px;">${t('settings.assistant.quick_actions.custom.add_btn')}</button>
                    </div>
                </div>

                <div id="personalization-app-modes" hidden>
                    <div style="display:flex;align-items:center;gap:8px;margin-bottom:8px;">
                        <button class="setting-button" data-action="closeAppModesEditor" style="background:transparent;color:var(--kage-text-secondary);border:1px solid var(--kage-border);">${t('settings.assistant.app_modes.editor.back_btn')}</button>
                        <h2 class="settings-section-header" style="margin:0;flex:1;">${t('settings.assistant.app_modes.editor.title')}</h2>
                    </div>
                    <div class="setting-description" style="margin-bottom:8px;">
                        ${t('settings.assistant.app_modes.editor.intro_html')}
                    </div>
                    <div class="setting-description" style="margin-bottom:12px;font-size:11px;opacity:0.75;">
                        ${t('settings.assistant.app_modes.editor.tip_html', { max: AssistantSettingsModule.APP_MODE_STEERING_MAX })}
                    </div>
                    <div id="appModesContainer" style="display:flex;flex-direction:column;gap:10px;"></div>
                    <div style="display:flex;gap:8px;margin-top:10px;flex-wrap:wrap;">
                        <button class="setting-button" data-action="addAppMode">${t('settings.assistant.app_modes.editor.add_btn')}</button>
                        <button class="setting-button" data-action="addSuggestedAppModes" style="background:transparent;color:var(--kage-text-secondary);border:1px solid var(--kage-border);" title="${t('settings.assistant.app_modes.add_suggested.title')}">${t('settings.assistant.app_modes.editor.add_suggested_btn')}</button>
                    </div>
                    <div style="display:flex;gap:8px;margin-top:14px;">
                        <button class="setting-button" data-action="saveAppModes">${t('settings.assistant.app_modes.editor.save_btn')}</button>
                        <button class="setting-button" data-action="closeAppModesEditor" style="background:transparent;color:var(--kage-text-secondary);border:1px solid var(--kage-border);">${t('settings.assistant.app_modes.editor.cancel_btn')}</button>
                    </div>
                    <div id="appModesStatus" class="setting-description" style="margin-top:8px;min-height:1em;"></div>
                </div>

                <div id="personalization-editor" hidden>
                    <div style="display:flex;align-items:center;gap:8px;margin-bottom:8px;">
                        <button class="setting-button" data-action="closeSteeringEditor" style="background:transparent;color:var(--kage-text-secondary);border:1px solid var(--kage-border);">${t('settings.assistant.app_modes.editor.back_btn')}</button>
                        <h2 class="settings-section-header" id="personalization-editor-title" style="margin:0;flex:1;"></h2>
                    </div>
                    <div class="setting-description" id="personalization-editor-subtitle" style="margin-bottom:6px;"></div>
                    <div class="setting-description" id="personalization-editor-path" style="font-size:11px;opacity:0.75;margin-bottom:8px;word-break:break-all;"></div>
                    <div id="personalization-editor-actions" style="display:flex;gap:8px;margin-bottom:10px;">
                        <button class="setting-button" data-action="importSteering" id="personalization-editor-importBtn" style="display:none;">${t('settings.assistant.editor.import_btn')}</button>
                        <button class="setting-button" data-action="revealSteering" id="personalization-editor-revealBtn" style="background:transparent;color:var(--kage-text-secondary);border:1px solid var(--kage-border);">${t('settings.assistant.editor.reveal_btn')}</button>
                    </div>
                    <div id="personalization-editor-mount"></div>
                    <div style="display:flex;gap:8px;margin-top:12px;">
                        <button class="setting-button" data-action="saveSteering">${t('settings.assistant.editor.save_btn')}</button>
                        <button class="setting-button" data-action="closeSteeringEditor" style="background:transparent;color:var(--kage-text-secondary);border:1px solid var(--kage-border);">${t('settings.assistant.editor.cancel_btn')}</button>
                    </div>
                    <div id="personalization-editor-status" class="setting-description" style="margin-top:8px;min-height:1em;"></div>
                </div>
            </div>
        `;
    }

    load(config) {
        this.loadFields(config);

        // App Modes — keep a snapshot for the editor + populate the
        // summary chip on the main view ("3 active rules" / "None").
        // Storing on `this` so the editor can read without an extra
        // get_config invoke.
        this._appModesSnapshot = Array.isArray(config.context_rules)
            ? config.context_rules.slice()
            : [];
        this._renderAppModesSummary();

        // Custom actions — list shape, can't go through bindFields.
        const qa = config.quick_actions || {};
        this._renderCustomActions(qa.custom_actions || []);

        // Switching to this section while either editor sub-view is open
        // is unusual but possible if the user clicked the sidebar — drop
        // back to main view.
        this._showMainView();
    }

    save(config) {
        this.saveFields(config);
        // user_steering_path is owned by write_steering_lines now — first
        // non-empty save pins the resolved default into config. We never
        // overwrite it from the UI here, so a hand-edited config.json
        // pointing at a custom path keeps working.

        // The bind DSL writes translateLanguage as the trimmed value;
        // the previous code coerced empty strings to `null` so the
        // backend's "no translation language set" sentinel still works.
        // Reapply that one transform.
        config.quick_actions = config.quick_actions || {};
        const lang = config.quick_actions.translate_language;
        config.quick_actions.translate_language =
            typeof lang === 'string' && lang.trim() ? lang.trim() : null;

        // Custom actions — list shape, populated from a dynamic UI.
        config.quick_actions.custom_actions = this._collectCustomActions();
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
                translateDesc.textContent = t('settings.assistant.translate.description', {
                    language: langName,
                });
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
            // App Modes
            openAppModesEditor: () => this._openAppModesEditor(),
            closeAppModesEditor: () => this._showMainView(),
            addAppMode: () => this._addAppModeRow(),
            addSuggestedAppModes: () => this._addSuggestedAppModes(),
            saveAppModes: () => this._saveAppModes(),
        });
    }

    // --- editor view machinery ------------------------------------------

    _showMainView() {
        const main = document.getElementById('personalization-main');
        const editor = document.getElementById('personalization-editor');
        const appModes = document.getElementById('personalization-app-modes');
        if (main) main.hidden = false;
        if (editor) editor.hidden = true;
        if (appModes) appModes.hidden = true;
        if (this._editor) {
            try {
                this._editor.destroy();
            } catch {}
            this._editor = null;
        }
        this._editingKind = null;
        this._editingPath = '';
        this._setStatus('');
        this._setAppModesStatus('');
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
            titleEl.textContent = t(
                kind === 'auto'
                    ? 'settings.assistant.editor.title.auto'
                    : 'settings.assistant.editor.title.user'
            );
        }
        if (subtitle) {
            subtitle.textContent = t(
                kind === 'auto'
                    ? 'settings.assistant.editor.subtitle.auto'
                    : 'settings.assistant.editor.subtitle.user'
            );
        }
        if (importBtn) {
            importBtn.style.display = kind === 'user' ? '' : 'none';
        }

        this._setStatus(t('settings.assistant.editor.status.loading'));
        let result;
        try {
            result = await window.__TAURI__.core.invoke('read_steering_lines', { kind });
        } catch (error) {
            this._setStatus(
                t('settings.assistant.editor.status.read_failed', {
                    message: this._formatError(error),
                }),
                'error'
            );
            // Still flip to the editor view so the user can hit Back.
            if (main) main.hidden = true;
            editor.hidden = false;
            return;
        }
        this._editingPath = result?.path || '';
        if (pathEl) {
            pathEl.textContent = this._editingPath
                ? t('settings.assistant.editor.path_label', { path: this._editingPath })
                : '';
        }
        this._setStatus(result?.exists === false ? t('settings.assistant.editor.no_file_yet') : '');

        if (this._editor) {
            try {
                this._editor.destroy();
            } catch {}
        }
        this._editor = createLineEditor(mount, {
            lines: Array.isArray(result?.lines) ? result.lines : [],
            emptyHint: t(
                kind === 'auto'
                    ? 'settings.assistant.editor.empty_hint.auto'
                    : 'settings.assistant.editor.empty_hint.user'
            ),
            rowPlaceholder: t(
                kind === 'auto'
                    ? 'settings.assistant.editor.row_placeholder.auto'
                    : 'settings.assistant.editor.row_placeholder.user'
            ),
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

        this._setStatus(t('settings.assistant.editor.status.saving'));
        try {
            await window.__TAURI__.core.invoke('write_steering_lines', {
                kind: this._editingKind,
                lines,
            });
            this._setStatus(t('settings.assistant.editor.status.saved'), 'success');
        } catch (error) {
            this._setStatus(
                t('settings.assistant.editor.status.save_failed', {
                    message: this._formatError(error),
                }),
                'error'
            );
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
            this._setStatus(
                t('settings.assistant.editor.status.import_cancelled', {
                    message: this._formatError(error),
                }),
                'error'
            );
            return;
        }
        if (!chosen || typeof chosen !== 'string') return;

        this._setStatus(t('settings.assistant.editor.status.importing'));
        try {
            const lines = await window.__TAURI__.core.invoke('import_steering_lines', {
                path: chosen,
            });
            this._editor.setLines(Array.isArray(lines) ? lines : []);
            this._setStatus(t('settings.assistant.editor.status.imported'), 'success');
        } catch (error) {
            this._setStatus(
                t('settings.assistant.editor.status.import_failed', {
                    message: this._formatError(error),
                }),
                'error'
            );
        }
    }

    async _revealCurrentFile() {
        if (!this._editingPath) return;
        try {
            await window.__TAURI__.core.invoke('open_path', { path: this._editingPath });
        } catch (error) {
            this._setStatus(
                t('settings.assistant.editor.status.reveal_failed', {
                    message: this._formatError(error),
                }),
                'error'
            );
        }
    }

    _setStatus(text, kind) {
        const el = document.getElementById('personalization-editor-status');
        if (!el) return;
        el.textContent = text || '';
        el.style.color = kind === 'error' ? 'var(--kage-error)' : kind === 'success' ? 'var(--kage-accent)' : '';
    }

    _formatError(e) {
        if (!e) return t('settings.assistant.editor.unknown_error');
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

    // --- app modes ------------------------------------------------------

    /**
     * Per-rule cap. Mirrors `MAX_STEERING_LEN` in `src/context_rules.rs`.
     * Kept as a JS-side const so the live counter doesn't need an IPC
     * round trip on every keypress; if we change one we change the
     * other.
     */
    static APP_MODE_STEERING_MAX = 500;

    _renderAppModesSummary() {
        const summary = document.getElementById('appModesSummary');
        if (!summary) return;
        const list = this._appModesSnapshot || [];
        const enabledCount = list.filter((r) => r.enabled !== false).length;
        if (list.length === 0) {
            summary.textContent = t('settings.assistant.app_modes.summary.none');
        } else if (list.length === enabledCount) {
            summary.textContent = t('settings.assistant.app_modes.summary.active_only', {
                active: enabledCount,
            });
        } else {
            summary.textContent = t('settings.assistant.app_modes.summary.active_disabled', {
                active: enabledCount,
                disabled: list.length - enabledCount,
            });
        }
    }

    _openAppModesEditor() {
        const main = document.getElementById('personalization-main');
        const view = document.getElementById('personalization-app-modes');
        const container = document.getElementById('appModesContainer');
        if (!view || !container) return;
        // Re-render rows from the in-memory snapshot every open. The
        // user may have hit Cancel last time and we want fresh state.
        container.innerHTML = '';
        const rules = this._appModesSnapshot || [];
        if (rules.length === 0) {
            this._addAppModeRow(); // start with one empty row
        } else {
            for (const rule of rules) this._addAppModeRow(rule);
        }
        if (main) main.hidden = true;
        view.hidden = false;
        this._setAppModesStatus('');
    }

    _addAppModeRow(rule = null) {
        const container = document.getElementById('appModesContainer');
        if (!container) return;
        const row = document.createElement('div');
        row.className = 'app-mode-row';
        row.style.cssText =
            'border:1px solid var(--kage-border);border-radius:6px;padding:10px;display:flex;flex-direction:column;gap:6px;';
        const nameVal = (rule?.friendly_name || '').replace(/"/g, '&quot;');
        const exeVal = (rule?.executable || '').replace(/"/g, '&quot;');
        const steeringVal = rule?.steering || '';
        const enabledChecked = rule?.enabled === false ? '' : ' checked';
        row.innerHTML = `
            <div style="display:flex;gap:8px;align-items:center;">
                <input type="text" class="setting-input am-name" placeholder="${t('settings.assistant.app_modes.row.name_placeholder')}" value="${nameVal}" style="flex:1;">
                <input type="text" class="setting-input am-exe" placeholder="${t('settings.assistant.app_modes.row.exe_placeholder')}" value="${exeVal}" style="flex:1;">
                <label class="kage-checkbox" title="${t('settings.assistant.app_modes.enable_rule.title')}" style="margin-left:4px;">
                    <input type="checkbox" class="am-enabled"${enabledChecked}>
                    <span style="font-size:11px;">${t('settings.assistant.app_modes.row.enabled_label')}</span>
                </label>
                <button class="setting-button am-remove icon-button-danger" type="button" title="${t('settings.assistant.app_modes.remove.title')}" aria-label="${t('settings.assistant.app_modes.remove.title')}">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="14" height="14"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
                </button>
            </div>
            <textarea class="setting-input am-steering" rows="3" placeholder="${escapeAttr(t('settings.assistant.app_modes.row.steering_placeholder'))}"></textarea>
            <div class="setting-description am-counter" style="font-size:11px;text-align:right;margin:0;"></div>
        `;
        const ta = row.querySelector('.am-steering');
        ta.value = steeringVal;
        const counter = row.querySelector('.am-counter');
        const updateCounter = () => {
            const len = ta.value.length;
            const max = AssistantSettingsModule.APP_MODE_STEERING_MAX;
            counter.textContent = `${len} / ${max}`;
            counter.style.color = len > max ? 'var(--kage-error)' : '';
        };
        ta.addEventListener('input', updateCounter);
        updateCounter();
        row.querySelector('.am-remove').addEventListener('click', () => {
            row.remove();
            // Empty container is fine — user can re-add or leave blank.
        });
        container.appendChild(row);
    }

    /**
     * Curated starter rules for common apps. Names cover Windows, macOS,
     * and Linux variants for the same app where they differ — the
     * matcher is whole-token / case-insensitive / .exe-stripping
     * (see src/context_rules.rs::matches), so a single token usually
     * matches across platforms (e.g. "code" hits Code.exe, Visual
     * Studio Code, and code on Linux). When that doesn't work we list
     * an OS-specific token; the user can tighten or split as needed.
     *
     * Steering is short and imperative per the docs in builtin_steering.md.
     */
    static SUGGESTED_APP_MODES = [
        {
            friendly_name: 'Code editor',
            executable: 'code',
            steering:
                'You are pair-programming. Be terse. Show diffs or full functions, not narration. Prefer the language already in the file. No "Sure, here\'s…"',
        },
        {
            friendly_name: 'Terminal',
            executable: 'terminal',
            steering:
                'Reply with shell commands first, prose second. Detect the OS from context. One-liners over scripts when possible. Mark destructive commands with a brief warning.',
        },
        {
            friendly_name: 'Browser',
            executable: 'chrome',
            steering:
                'Assume the user is reading a web page. Summarise concisely, surface the key claim, and flag anything that looks paywalled or AI-generated. Cite the page when quoting.',
        },
        {
            friendly_name: 'Email',
            executable: 'outlook',
            steering:
                'Match the tone of the thread. Default to short replies. If drafting from scratch, give two options: a 1-liner and a 3-sentence version. No filler ("hope this helps").',
        },
        {
            friendly_name: 'Slack',
            executable: 'slack',
            steering:
                'Casual, lowercase-okay, emoji sparingly. Reply in 1–3 sentences. If the user pastes a thread, summarise + suggest one next message.',
        },
        {
            friendly_name: 'Notes / writing',
            executable: 'notion',
            steering:
                'Help the user think on paper. Ask clarifying questions when the goal is ambiguous. Prefer structured bullets and short headings over walls of prose.',
        },
        {
            friendly_name: 'Spreadsheet',
            executable: 'excel',
            steering:
                'Default to formulas (Excel/Google Sheets dialect). When ambiguous, ask whether the data is a range or a table. Flag locale-sensitive things (decimals, dates) explicitly.',
        },
        {
            friendly_name: 'Design tool',
            executable: 'figma',
            steering:
                'Think about visual hierarchy, contrast, and spacing first. Suggest concrete CSS values or design tokens, not vague directions like "make it pop".',
        },
        {
            friendly_name: 'Video call',
            executable: 'zoom',
            steering:
                'Optimise for speaking aloud: short sentences, no markdown, no code blocks unless asked. Be ready to repeat or rephrase the previous answer in fewer words.',
        },
        {
            friendly_name: 'PDF reader',
            executable: 'acrobat',
            steering:
                'Assume the user is reading a long document. Summarise sections on request, extract action items, and quote with page references when possible.',
        },
    ];

    /**
     * Drop in any suggested rule whose token isn't already represented
     * by an existing row's executable field. "Already represented"
     * means the suggestion's exe token appears in any existing exe
     * field after normalisation (mirrors the Rust matcher's behaviour).
     */
    _addSuggestedAppModes() {
        const container = document.getElementById('appModesContainer');
        if (!container) return;

        const norm = (s) =>
            String(s || '')
                .trim()
                .toLowerCase()
                .replace(/\.exe$/, '');

        // Build a set of exe tokens already present in the editor
        // (across all rows, not just the saved snapshot — the user
        // might have just typed something they don't want clobbered).
        const present = new Set();
        for (const row of container.querySelectorAll('.am-exe')) {
            const v = norm(row.value);
            if (v) present.add(v);
        }
        // First open lands a single empty row; if the user clicks
        // "Add suggested" without touching it, drop it so we don't
        // leave a dangling blank.
        const blanks = Array.from(container.querySelectorAll('.app-mode-row')).filter((row) => {
            const n = row.querySelector('.am-name')?.value?.trim();
            const e = row.querySelector('.am-exe')?.value?.trim();
            const s = row.querySelector('.am-steering')?.value?.trim();
            return !n && !e && !s;
        });
        for (const b of blanks) b.remove();

        let added = 0;
        let skipped = 0;
        for (const sug of AssistantSettingsModule.SUGGESTED_APP_MODES) {
            if (present.has(norm(sug.executable))) {
                skipped += 1;
                continue;
            }
            this._addAppModeRow({ ...sug, enabled: true });
            present.add(norm(sug.executable));
            added += 1;
        }

        if (added === 0) {
            this._setAppModesStatus(
                t('settings.assistant.app_modes.suggest.none_added'),
                'success'
            );
        } else {
            const skipNote =
                skipped > 0
                    ? t('settings.assistant.app_modes.suggest.skipped_suffix', { count: skipped })
                    : '';
            this._setAppModesStatus(
                t('settings.assistant.app_modes.suggest.added', {
                    count: added,
                    skipped: skipNote,
                }),
                'success'
            );
        }
    }

    _collectAppModes() {
        const container = document.getElementById('appModesContainer');
        if (!container) return [];
        const rules = [];
        for (const row of container.querySelectorAll('.app-mode-row')) {
            const friendly = row.querySelector('.am-name')?.value?.trim() || '';
            const exe = row.querySelector('.am-exe')?.value?.trim() || '';
            const steering = row.querySelector('.am-steering')?.value?.trim() || '';
            const enabled = !!row.querySelector('.am-enabled')?.checked;
            // Drop completely empty rows silently — they're abandoned
            // additions, not data the user wants saved.
            if (!friendly && !exe && !steering) continue;
            rules.push({
                friendly_name: friendly,
                executable: exe,
                steering,
                enabled,
            });
        }
        return rules;
    }

    async _saveAppModes() {
        const rules = this._collectAppModes();
        // Validate: every populated row needs at minimum a friendly
        // name and executable. Steering may be empty (the rule will
        // simply not contribute anything).
        const max = AssistantSettingsModule.APP_MODE_STEERING_MAX;
        for (const r of rules) {
            if (!r.friendly_name) {
                this._setAppModesStatus(
                    t('settings.assistant.app_modes.validate.name_required'),
                    'error'
                );
                return;
            }
            if (!r.executable) {
                this._setAppModesStatus(
                    t('settings.assistant.app_modes.validate.exe_required', {
                        name: r.friendly_name,
                    }),
                    'error'
                );
                return;
            }
            if (r.steering.length > max) {
                this._setAppModesStatus(
                    t('settings.assistant.app_modes.validate.steering_too_long', {
                        name: r.friendly_name,
                        length: r.steering.length,
                        max,
                    }),
                    'error'
                );
                return;
            }
        }

        // Read full config, splice, save. We don't go through the
        // SettingsManager save path because that would also overwrite
        // every other field on the page from current DOM state — fine
        // for normal saves but surprising for a sub-view that only
        // owns context_rules.
        try {
            const invoke = window.__TAURI__.core.invoke;
            const config = await invoke('get_config');
            config.context_rules = rules;
            await invoke('save_config', { config });
            this._appModesSnapshot = rules.slice();
            this._renderAppModesSummary();
            this._setAppModesStatus(t('settings.assistant.app_modes.save.saved'), 'success');
            // Brief delay so the user sees the success toast, then
            // back to the main view.
            setTimeout(() => this._showMainView(), 450);
        } catch (e) {
            this._setAppModesStatus(
                t('settings.assistant.app_modes.save.failed', { message: this._formatError(e) }),
                'error'
            );
        }
    }

    _setAppModesStatus(text, kind) {
        const el = document.getElementById('appModesStatus');
        if (!el) return;
        el.textContent = text || '';
        el.style.color = kind === 'error' ? 'var(--kage-error)' : kind === 'success' ? 'var(--kage-accent)' : '';
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
