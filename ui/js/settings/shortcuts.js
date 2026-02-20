/**
 * Shortcuts Settings Module
 * Manages custom command shortcuts
 */
class ShortcutsSettingsModule extends SettingsModule {
    constructor() {
        super('shortcuts', 'Shortcuts');
        this.shortcuts = [];
        this.editingIndex = -1;
    }

    render() {
        return `
            <div class="settings-section-header">Shortcuts</div>
            
            <div class="setting-row">
                <div class="setting-label-container">
                    <div class="setting-label">Command Shortcuts</div>
                    <div class="setting-description">
                        Create shortcuts to run executables directly from the floating window without sending to the LLM.
                        Use {*} for all arguments after the shortcut, or {0}, {1}, etc. for specific arguments.
                    </div>
                </div>
            </div>

            <div class="shortcuts-list" id="shortcutsList"></div>

            <div class="shortcuts-actions">
                <button class="setting-button" onclick="shortcutsModule.showAddDialog()">Add Shortcut</button>
                <button class="setting-button" onclick="shortcutsModule.exportShortcuts()">Export to JSON</button>
                <button class="setting-button" onclick="shortcutsModule.importShortcuts()">Import from JSON</button>
            </div>

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
                            <label>Shortcut</label>
                            <input type="text" id="shortcutTrigger" class="setting-input" placeholder="e.g., code">
                        </div>
                        <div class="dialog-field">
                            <label>Action Type</label>
                            <select id="shortcutActionType" class="setting-select" onchange="shortcutsModule.onActionTypeChange()">
                                <option value="run_program">Run Program</option>
                                <option value="open_url">Open URL</option>
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
                                <label>URL</label>
                                <input type="text" id="shortcutUrl" class="setting-input" placeholder="e.g., https://example.com/?q={0}">
                                <div class="setting-description" style="margin-top: 4px;">
                                    Use {*} for all arguments, or {0}, {1}, etc. for specific arguments in the URL
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
                .shortcuts-list {
                    margin: 20px 0;
                    border: 1px solid #2b2b2b;
                    border-radius: 4px;
                    overflow: hidden;
                }

                .shortcut-item {
                    padding: 16px;
                    border-bottom: 1px solid #2b2b2b;
                    display: flex;
                    justify-content: space-between;
                    align-items: flex-start;
                    background: #2d2d30;
                }

                .shortcut-item:last-child {
                    border-bottom: none;
                }

                .shortcut-item:hover {
                    background: #323237;
                }

                .shortcut-info {
                    flex: 1;
                }

                .shortcut-name {
                    font-size: 14px;
                    font-weight: 500;
                    color: #ffffff;
                    margin-bottom: 4px;
                }

                .shortcut-trigger {
                    display: inline-block;
                    padding: 2px 8px;
                    background: #007acc;
                    color: #ffffff;
                    border-radius: 3px;
                    font-size: 12px;
                    font-family: 'Courier New', monospace;
                    margin-bottom: 8px;
                }

                .shortcut-details {
                    font-size: 12px;
                    color: #888888;
                    line-height: 1.6;
                }

                .shortcut-actions {
                    display: flex;
                    gap: 8px;
                }

                .shortcut-action-btn {
                    padding: 4px 12px;
                    background: transparent;
                    border: 1px solid #3c3c3c;
                    border-radius: 2px;
                    color: #cccccc;
                    font-size: 12px;
                    cursor: pointer;
                    transition: all 0.2s;
                }

                .shortcut-action-btn:hover {
                    background: #3c3c3c;
                    border-color: #007acc;
                }

                .shortcut-action-btn.delete:hover {
                    background: #c0392b;
                    border-color: #c0392b;
                    color: #ffffff;
                }

                .shortcuts-actions {
                    display: flex;
                    gap: 12px;
                    margin-top: 16px;
                }

                .shortcut-dialog {
                    position: fixed;
                    top: 0;
                    left: 0;
                    right: 0;
                    bottom: 0;
                    background: rgba(0, 0, 0, 0.7);
                    display: flex;
                    align-items: center;
                    justify-content: center;
                    z-index: 1000;
                }

                .shortcut-dialog-content {
                    background: #252526;
                    border: 1px solid #2b2b2b;
                    border-radius: 4px;
                    width: 600px;
                    max-width: 90%;
                    max-height: 90vh;
                    overflow: auto;
                }

                .shortcut-dialog-header {
                    padding: 16px 20px;
                    border-bottom: 1px solid #2b2b2b;
                    display: flex;
                    justify-content: space-between;
                    align-items: center;
                }

                .shortcut-dialog-header h3 {
                    font-size: 16px;
                    font-weight: 500;
                    color: #ffffff;
                    margin: 0;
                }

                .dialog-close-btn {
                    background: transparent;
                    border: none;
                    color: #cccccc;
                    font-size: 24px;
                    cursor: pointer;
                    padding: 0;
                    width: 30px;
                    height: 30px;
                    display: flex;
                    align-items: center;
                    justify-content: center;
                    border-radius: 2px;
                }

                .dialog-close-btn:hover {
                    background: #3e3e42;
                }

                .shortcut-dialog-body {
                    padding: 20px;
                }

                .dialog-field {
                    margin-bottom: 16px;
                }

                .dialog-field:last-child {
                    margin-bottom: 0;
                }

                .dialog-field label {
                    display: block;
                    font-size: 13px;
                    color: #cccccc;
                    margin-bottom: 6px;
                    font-weight: 500;
                }

                .shortcut-dialog-footer {
                    padding: 16px 20px;
                    border-top: 1px solid #2b2b2b;
                    display: flex;
                    justify-content: flex-end;
                    gap: 12px;
                }

                .shortcuts-empty {
                    padding: 40px;
                    text-align: center;
                    color: #888888;
                    font-size: 13px;
                }
            </style>
        `;
    }

    initialize() {
        this.renderShortcutsList();
        window.shortcutsModule = this;
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

        listEl.innerHTML = this.shortcuts.map((shortcut, index) => {
            const actionType = shortcut.action_type || 'run_program';
            const actionLabel = actionType === 'open_url' ? '🌐 Open URL' : '▶️ Run Program';
            
            let details = `<div><strong>Type:</strong> ${actionLabel}</div>`;
            
            if (actionType === 'open_url') {
                details += `<div><strong>URL:</strong> ${this.escapeHtml(shortcut.url || '')}</div>`;
            } else {
                details += `<div><strong>Path:</strong> ${this.escapeHtml(shortcut.path || '')}</div>`;
                if (shortcut.working_directory) {
                    details += `<div><strong>Working Dir:</strong> ${this.escapeHtml(shortcut.working_directory)}</div>`;
                }
                if (shortcut.arguments) {
                    details += `<div><strong>Arguments:</strong> ${this.escapeHtml(shortcut.arguments)}</div>`;
                }
            }
            
            return `
                <div class="shortcut-item">
                    <div class="shortcut-info">
                        <div class="shortcut-name">${this.escapeHtml(shortcut.name)}</div>
                        <div class="shortcut-trigger">${this.escapeHtml(shortcut.shortcut)}</div>
                        <div class="shortcut-details">
                            ${details}
                        </div>
                    </div>
                    <div class="shortcut-actions">
                        <button class="shortcut-action-btn" onclick="shortcutsModule.editShortcut(${index})">Edit</button>
                        <button class="shortcut-action-btn delete" onclick="shortcutsModule.deleteShortcut(${index})">Delete</button>
                    </div>
                </div>
            `;
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
        this.onActionTypeChange();
        document.getElementById('shortcutDialog').style.display = 'flex';
    }

    editShortcut(index) {
        this.editingIndex = index;
        const shortcut = this.shortcuts[index];
        const actionType = shortcut.action_type || 'run_program';
        
        document.getElementById('dialogTitle').textContent = 'Edit Shortcut';
        document.getElementById('shortcutName').value = shortcut.name;
        document.getElementById('shortcutTrigger').value = shortcut.shortcut;
        document.getElementById('shortcutActionType').value = actionType;
        document.getElementById('shortcutPath').value = shortcut.path || '';
        document.getElementById('shortcutWorkDir').value = shortcut.working_directory || '';
        document.getElementById('shortcutArgs').value = shortcut.arguments || '';
        document.getElementById('shortcutUrl').value = shortcut.url || '';
        this.onActionTypeChange();
        document.getElementById('shortcutDialog').style.display = 'flex';
    }

    onActionTypeChange() {
        const actionType = document.getElementById('shortcutActionType').value;
        const runProgramFields = document.getElementById('runProgramFields');
        const openUrlFields = document.getElementById('openUrlFields');
        
        if (actionType === 'open_url') {
            runProgramFields.style.display = 'none';
            openUrlFields.style.display = 'block';
        } else {
            runProgramFields.style.display = 'block';
            openUrlFields.style.display = 'none';
        }
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
            alert('Name and Shortcut are required fields.');
            return;
        }

        const shortcut = {
            name,
            shortcut: trigger,
            action_type: actionType
        };

        if (actionType === 'open_url') {
            const url = document.getElementById('shortcutUrl').value.trim();
            if (!url) {
                alert('URL is required for Open URL action type.');
                return;
            }
            shortcut.url = url;
        } else {
            const path = document.getElementById('shortcutPath').value.trim();
            if (!path) {
                alert('Executable Path is required for Run Program action type.');
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
        if (confirm('Are you sure you want to delete this shortcut?')) {
            this.shortcuts.splice(index, 1);
            this.renderShortcutsList();
        }
    }

    exportShortcuts() {
        const dataStr = JSON.stringify(this.shortcuts, null, 2);
        const dataBlob = new Blob([dataStr], { type: 'application/json' });
        const url = URL.createObjectURL(dataBlob);
        const link = document.createElement('a');
        link.href = url;
        link.download = 'kiro-shortcuts.json';
        link.click();
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
            reader.onload = (event) => {
                try {
                    const imported = JSON.parse(event.target.result);
                    if (!Array.isArray(imported)) {
                        alert('Invalid shortcuts file format.');
                        return;
                    }
                    this.shortcuts = imported;
                    this.renderShortcutsList();
                    alert('Shortcuts imported successfully!');
                } catch (error) {
                    alert('Failed to parse shortcuts file: ' + error.message);
                }
            };
            reader.readAsText(file);
        };
        input.click();
    }

    escapeHtml(text) {
        const div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
    }

    destroy() {
        delete window.shortcutsModule;
    }
}
