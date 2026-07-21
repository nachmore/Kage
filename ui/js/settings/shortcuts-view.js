import { t } from '../shared/i18n.js';
import { escapeHtml } from '../shared/tool-utils.js';

export function renderShortcutsSettings(module) {
    return `
            <div class="settings-section-header">${module.icon} ${t('settings.shortcuts.title')}</div>

            <div class="setting-row">
                <div class="setting-label-container">
                    <div class="setting-label">${t('settings.shortcuts.section.label')}</div>
                    <div class="setting-description">
                        ${t('settings.shortcuts.description_html')}
                    </div>
                </div>
            </div>

            <div class="shortcuts-actions">
                <button class="setting-button" data-action="shortcuts.showAddDialog">${t('settings.shortcuts.action.add')}</button>
                <button class="setting-button" data-action="shortcuts.openCommandsStore">${t('settings.shortcuts.action.store')}</button>
                <button class="setting-button" data-action="shortcuts.exportShortcuts">${t('settings.shortcuts.action.export')}</button>
                <button class="setting-button" data-action="shortcuts.importShortcuts">${t('settings.shortcuts.action.import')}</button>
            </div>
            <div class="setting-description" style="font-size:11px;margin-top:6px;">
                ${t('settings.shortcuts.export_note_html')}
            </div>

            <div class="shortcuts-list" id="shortcutsList"></div>

            <!-- Add/Edit Dialog -->
            <div id="shortcutDialog" class="shortcut-dialog" style="display: none;">
                <div class="shortcut-dialog-content">
                    <div class="shortcut-dialog-header">
                        <h3 id="dialogTitle">${t('settings.shortcuts.dialog.add_title')}</h3>
                        <button class="dialog-close-btn" data-action="shortcuts.closeDialog">×</button>
                    </div>
                    <div class="shortcut-dialog-body">
                        <div class="dialog-field">
                            <label>${t('settings.shortcuts.dialog.name.label')}</label>
                            <input type="text" id="shortcutName" class="setting-input" placeholder="${t('settings.shortcuts.dialog.name.placeholder')}">
                        </div>
                        <div class="dialog-field">
                            <label>${t('settings.shortcuts.dialog.trigger.label')}</label>
                            <input type="text" id="shortcutTrigger" class="setting-input" placeholder="${t('settings.shortcuts.dialog.trigger.placeholder')}">
                        </div>
                        <div class="dialog-field">
                            <label>${t('settings.shortcuts.dialog.icon.label')}</label>
                            <div style="display:flex;gap:8px;align-items:center;">
                                <div id="shortcutIconPreview" class="shortcut-icon-preview">⚡</div>
                                <input type="text" id="shortcutIconEmoji" class="setting-input" style="width:60px;text-align:center;" placeholder="${t('settings.shortcuts.dialog.icon.placeholder')}" maxlength="4">
                                <button class="setting-button" data-action="shortcuts.openIconFilePicker">${t('settings.shortcuts.dialog.icon.image_btn')}</button>
                                <button class="setting-button" id="shortcutFaviconBtn" style="display:none" data-action="shortcuts.fetchFavicon">${t('settings.shortcuts.dialog.icon.favicon_btn')}</button>
                                <button class="setting-button" id="shortcutIconClear" style="display:none" data-action="shortcuts.clearIcon">✕</button>
                                <input type="file" id="shortcutIconFile" accept="image/png,image/jpeg,image/x-icon,image/vnd.microsoft.icon,.ico,.png,.jpg,.jpeg" style="display:none">
                            </div>
                            <div style="font-size:11px;color:var(--kage-text-muted);margin-top:4px;">${t('settings.shortcuts.dialog.icon.help')}</div>
                        </div>
                        <div class="dialog-field">
                            <label>${t('settings.shortcuts.dialog.action_type.label')}</label>
                            <select id="shortcutActionType" class="setting-select" data-action-change="shortcuts.onActionTypeChange">
                                <option value="run_program">${t('settings.shortcuts.dialog.action_type.run_program')}</option>
                                <option value="open_url">${t('settings.shortcuts.dialog.action_type.open_url')}</option>
                                <option value="prompt">${t('settings.shortcuts.dialog.action_type.prompt')}</option>
                                <option value="script">${t('settings.shortcuts.dialog.action_type.script')}</option>
                            </select>
                        </div>

                        <!-- Run Program Fields -->
                        <div id="runProgramFields">
                            <div class="dialog-field">
                                <label>${t('settings.shortcuts.dialog.run.path.label')}</label>
                                <input type="text" id="shortcutPath" class="setting-input" placeholder="e.g., C:\\Program Files\\VSCode\\code.exe">
                            </div>
                            <div class="dialog-field">
                                <label>${t('settings.shortcuts.dialog.run.work_dir.label')}</label>
                                <input type="text" id="shortcutWorkDir" class="setting-input" placeholder="${t('settings.shortcuts.dialog.run.work_dir.placeholder')}">
                            </div>
                            <div class="dialog-field">
                                <label>${t('settings.shortcuts.dialog.run.args.label')}</label>
                                <input type="text" id="shortcutArgs" class="setting-input" placeholder="${t('settings.shortcuts.dialog.run.args.placeholder')}">
                                <div class="setting-description" style="margin-top: 4px;">
                                    ${t('settings.shortcuts.dialog.run.args.help')}
                                </div>
                            </div>
                        </div>

                        <!-- Open URL Fields -->
                        <div id="openUrlFields" style="display: none;">
                            <div class="dialog-field">
                                <label>${t('settings.shortcuts.dialog.url.label')}</label>
                                <input type="text" id="shortcutUrl" class="setting-input" placeholder="${t('settings.shortcuts.dialog.url.placeholder')}">
                                <div class="setting-description" style="margin-top: 4px;">
                                    ${t('settings.shortcuts.dialog.url.help')}
                                </div>
                            </div>
                        </div>

                        <!-- Prompt Fields -->
                        <div id="promptFields" style="display: none;">
                            <div class="dialog-field">
                                <label>${t('settings.shortcuts.dialog.prompt.label')}</label>
                                <textarea id="shortcutPrompt" class="setting-input" rows="3" placeholder="${t('settings.shortcuts.dialog.prompt.placeholder')}"></textarea>
                                <div class="setting-description" style="margin-top: 4px;">
                                    ${t('settings.shortcuts.dialog.prompt.help_html')}
                                </div>
                                <div id="shortcutPromptPlaceholders" class="prompt-placeholder-chips" style="margin-top:6px;display:none;"></div>
                            </div>
                        </div>

                        <!-- Script Fields -->
                        <div id="scriptFields" style="display: none;">
                            <div class="dialog-field">
                                <label>${t('settings.shortcuts.dialog.script.action.label')}</label>
                                <select id="shortcutScriptAction" class="setting-select">
                                    <option value="text">${t('settings.shortcuts.dialog.script.action.text')}</option>
                                    <option value="prompt">${t('settings.shortcuts.dialog.script.action.prompt')}</option>
                                    <option value="open_url">${t('settings.shortcuts.dialog.script.action.open_url')}</option>
                                    <option value="run_program">${t('settings.shortcuts.dialog.script.action.run_program')}</option>
                                </select>
                                <div class="setting-description" style="margin-top: 4px;">
                                    ${t('settings.shortcuts.dialog.script.action.help')}
                                </div>
                            </div>
                            <div class="dialog-field">
                                <label>${t('settings.shortcuts.dialog.script.ai.label')}</label>
                                <div class="ai-prompt-row">
                                    <textarea id="scriptAiPrompt" class="setting-input" rows="1" placeholder="${t('settings.shortcuts.dialog.script.ai.placeholder')}"></textarea>
                                    <button class="setting-button" id="scriptAiBtn" data-action="shortcuts.generateScript">${t('settings.shortcuts.dialog.script.ai.generate')}</button>
                                    <button class="setting-button" id="scriptAiUndo" data-action="shortcuts.undoGenerate" style="display:none">${t('settings.shortcuts.dialog.script.ai.undo')}</button>
                                </div>
                                <div id="scriptAiStatus" class="setting-description" style="margin-top: 4px;"></div>
                            </div>
                            <div class="dialog-field">
                                <label>${t('settings.shortcuts.dialog.script.body.label')}</label>
                                <div class="script-editor-container">
                                    <pre class="script-highlight" aria-hidden="true"><code class="language-javascript" id="shortcutScriptHighlight"></code></pre>
                                    <textarea id="shortcutScript" class="setting-input script-editor" rows="8" spellcheck="false" wrap="off"
                                        placeholder="${t('settings.shortcuts.dialog.script.body.placeholder')}"></textarea>
                                </div>
                                <div class="setting-description" style="margin-top: 4px;">
                                    ${t('settings.shortcuts.dialog.script.body.help_html')}
                                </div>
                            </div>
                        </div>
                    </div>
                    <div class="shortcut-test-section">
                        <div class="shortcut-test-header" data-action="shortcuts.toggleTestSection">
                            <span id="shortcutTestToggle">▶</span> ${t('settings.shortcuts.dialog.test.toggle')}
                        </div>
                        <div id="shortcutTestBody" style="display:none;">
                            <div class="dialog-field" style="margin-bottom:8px;">
                                <div style="display:flex;gap:8px;">
                                    <input type="text" id="shortcutTestArgs" class="setting-input" placeholder="${t('settings.shortcuts.dialog.test.placeholder')}" style="flex:1;">
                                    <button class="setting-button" data-action="shortcuts.runTest">${t('settings.shortcuts.dialog.test.run')}</button>
                                </div>
                            </div>
                            <pre id="shortcutTestOutput" class="shortcut-test-output" style="display:none;"></pre>
                        </div>
                    </div>
                    <div class="shortcut-dialog-footer">
                        <button class="setting-button" data-action="shortcuts.closeDialog">${t('settings.shortcuts.dialog.cancel')}</button>
                        <button class="setting-button" data-action="shortcuts.saveShortcut">${t('settings.shortcuts.dialog.save')}</button>
                    </div>
                </div>
            </div>

            <style>
                .shortcuts-list { margin: 20px 0; border: 1px solid var(--kage-border-subtle); border-radius: 4px; overflow: hidden; }
                .shortcut-item { padding: 16px; border-bottom: 1px solid var(--kage-border-subtle); display: flex; justify-content: space-between; align-items: flex-start; background: var(--kage-bg-input); }
                .shortcut-item:last-child { border-bottom: none; }
                .shortcut-item:hover { background: var(--kage-bg-elevated); }
                .shortcut-info { flex: 1; min-width: 0; }
                .shortcut-name { font-size: 14px; font-weight: 500; color: var(--kage-text-bright); margin-bottom: 4px; }
                .shortcut-trigger { display: inline-block; padding: 2px 8px; background: var(--kage-accent); color: #ffffff; border-radius: 3px; font-size: 12px; font-family: 'Courier New', monospace; margin-bottom: 8px; }
                .shortcut-details { font-size: 12px; color: var(--kage-text-muted); line-height: 1.6; overflow-wrap: break-word; word-break: break-all; }
                .shortcut-actions { display: flex; gap: 8px; flex-shrink: 0; }
                .shortcut-action-btn { padding: 4px 12px; background: transparent; border: 1px solid var(--kage-border); border-radius: 2px; color: var(--kage-text); font-size: 12px; cursor: pointer; transition: all 0.2s; }
                .shortcut-action-btn:hover { background: var(--kage-border); border-color: var(--kage-accent); }
                .shortcut-action-btn.delete:hover { background: #c0392b; border-color: #c0392b; color: #ffffff; }
                .shortcuts-actions { display: flex; gap: 12px; margin-bottom: 16px; }
                .shortcut-dialog { position: fixed; top: 0; left: 0; right: 0; bottom: 0; background: rgba(0,0,0,0.7); display: flex; align-items: center; justify-content: center; z-index: 1000; }
                .shortcut-dialog-content { background: var(--kage-bg-elevated); border: 1px solid var(--kage-border-subtle); border-radius: 4px; width: 600px; max-width: 90%; max-height: 90vh; overflow: auto; }
                .shortcut-dialog-header { padding: 16px 20px; border-bottom: 1px solid var(--kage-border-subtle); display: flex; justify-content: space-between; align-items: center; }
                .shortcut-dialog-header h3 { font-size: 16px; font-weight: 500; color: var(--kage-text-bright); margin: 0; }
                .dialog-close-btn { background: transparent; border: none; color: var(--kage-text); font-size: 24px; cursor: pointer; padding: 0; width: 30px; height: 30px; display: flex; align-items: center; justify-content: center; border-radius: 2px; }
                .dialog-close-btn:hover { background: var(--kage-border); }
                .shortcut-dialog-body { padding: 20px; }
                .dialog-field { margin-bottom: 16px; }
                .dialog-field:last-child { margin-bottom: 0; }
                .dialog-field label { display: block; font-size: 13px; color: var(--kage-text); margin-bottom: 6px; font-weight: 500; }
                .shortcut-dialog-footer { padding: 16px 20px; border-top: 1px solid var(--kage-border-subtle); display: flex; justify-content: flex-end; gap: 12px; }
                .shortcut-test-section { border-top: 1px solid var(--kage-border-subtle); }
                .shortcut-test-header { padding: 10px 20px; font-size: 12px; color: var(--kage-text-muted); cursor: pointer; user-select: none; }
                .shortcut-test-header:hover { color: var(--kage-text); background: var(--kage-bg-input); }
                #shortcutTestBody { padding: 0 20px 12px; }
                .shortcut-test-output { font-family: 'SF Mono', 'Consolas', monospace; font-size: 12px; line-height: 1.5; padding: 8px 12px; border-radius: 4px; background: var(--kage-bg-input); border: 1px solid var(--kage-border); color: var(--kage-text); white-space: pre-wrap; word-break: break-all; max-height: 120px; overflow-y: auto; margin: 0; }
                .shortcut-test-output.test-error { color: #e74c3c; border-color: #e74c3c40; }
                .shortcut-test-output.test-success { border-color: var(--kage-accent); }
                .shortcuts-empty { padding: 40px; text-align: center; color: var(--kage-text-muted); font-size: 13px; }
                .shortcut-icon-preview { width: 32px; height: 32px; border-radius: 6px; background: var(--kage-bg-input); border: 1px solid var(--kage-border); display: flex; align-items: center; justify-content: center; font-size: 18px; overflow: hidden; flex-shrink: 0; }
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
                    border: 1px solid var(--kage-border); border-radius: 4px;
                    overflow: auto; box-sizing: border-box;
                    min-height: 120px; tab-size: 2;
                }
                .script-editor-container .script-editor {
                    background: transparent; color: transparent;
                    caret-color: var(--kage-text-bright);
                    resize: vertical; width: 100%; z-index: 2;
                    outline: none;
                }
                .script-editor-container .script-editor:focus {
                    border-color: var(--kage-accent);
                }
                .script-editor-container .script-highlight {
                    background: var(--kage-bg-input); color: var(--kage-text);
                    pointer-events: none; z-index: 1;
                    border-color: transparent;
                    overflow: auto;
                }
                .script-highlight code {
                    font-family: inherit !important; font-size: 1em !important;
                    background: none !important; padding: 0 !important; margin: 0 !important;
                    color: inherit; word-wrap: break-word;
                }
                .ai-prompt-row { display: flex; gap: 8px; align-items: flex-start; }
                .ai-prompt-row .setting-input { flex: 1; }
                .ai-prompt-row textarea.setting-input { resize: none; overflow-y: hidden; line-height: 1.4; }
                .ai-prompt-row .setting-button { white-space: nowrap; }
                textarea.setting-input { resize: vertical; }
            </style>
        `;
}

export function renderShortcutsListView(module) {
    const listEl = document.getElementById('shortcutsList');
    if (!listEl) return;

    if (module.shortcuts.length === 0) {
        listEl.innerHTML = `<div class="shortcuts-empty">${t('settings.shortcuts.list.empty_full')}</div>`;
        return;
    }

    const actionLabels = {
        run_program: t('settings.shortcuts.action_label.run_program'),
        open_url: t('settings.shortcuts.action_label.open_url'),
        prompt: t('settings.shortcuts.action_label.prompt'),
        script: t('settings.shortcuts.action_label.script'),
    };
    const detailType = t('settings.shortcuts.detail.type');
    const detailUrl = t('settings.shortcuts.detail.url');
    const detailPrompt = t('settings.shortcuts.detail.prompt');
    const detailAction = t('settings.shortcuts.detail.action');
    const detailScript = t('settings.shortcuts.detail.script');
    const detailPath = t('settings.shortcuts.detail.path');
    const detailDir = t('settings.shortcuts.detail.dir');
    const detailArgs = t('settings.shortcuts.detail.args');

    listEl.innerHTML = module.shortcuts
        .map((s, index) => {
            const at = s.action_type || 'run_program';
            const label = actionLabels[at] || at;
            let details = `<div><strong>${detailType}</strong> ${label}</div>`;

            if (at === 'open_url') {
                details += `<div><strong>${detailUrl}</strong> ${escapeHtml(s.url || '')}</div>`;
            } else if (at === 'prompt') {
                details += `<div><strong>${detailPrompt}</strong> ${escapeHtml(s.prompt || '')}</div>`;
            } else if (at === 'script') {
                const saLabels = {
                    text: t('settings.shortcuts.script_label.text'),
                    prompt: t('settings.shortcuts.script_label.prompt'),
                    open_url: t('settings.shortcuts.script_label.open_url'),
                    run_program: t('settings.shortcuts.script_label.run_program'),
                };
                const fallback = t('settings.shortcuts.script_label.default');
                details += `<div><strong>${detailAction}</strong> ${saLabels[s.script_action] || fallback}</div>`;
                details += `<div><strong>${detailScript}</strong> <code>${escapeHtml((s.script || '').substring(0, 60))}${(s.script || '').length > 60 ? '...' : ''}</code></div>`;
            } else {
                details += `<div><strong>${detailPath}</strong> ${escapeHtml(s.path || '')}</div>`;
                if (s.working_directory)
                    details += `<div><strong>${detailDir}</strong> ${escapeHtml(s.working_directory)}</div>`;
                if (s.arguments)
                    details += `<div><strong>${detailArgs}</strong> ${escapeHtml(s.arguments)}</div>`;
            }

            let iconHtml;
            if (s.icon?.startsWith('data:')) {
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
                        <button class="shortcut-action-btn" data-action="shortcuts.editShortcut" data-arg="${index}">${t('settings.shortcuts.list.action.edit')}</button>
                        <button class="shortcut-action-btn delete" data-action="shortcuts.deleteShortcut" data-arg="${index}">${t('settings.shortcuts.list.action.delete')}</button>
                    </div>
                </div>`;
        })
        .join('');
}
