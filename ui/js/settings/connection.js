/**
 * Connection Settings Module
 *
 * Master-detail layout:
 *   - List view (default): workspace folder at top, then a single
 *     unified list. Active connection rendered first as a hero card,
 *     followed by other saved connections, then a "Detected on this
 *     machine" subgroup for agents Kage found that aren't yet saved.
 *     Detected entries use the same row chrome as saved ones with a
 *     subtle "detected" tint and an "Add" action instead of "Edit".
 *   - Edit view: full-width form for one connection with a Back
 *     button. Reached by clicking Edit on a saved connection or Add
 *     on a detected one. The form covers Name, Mode, Spawn / Host,
 *     and per-agent Sessions directory.
 *
 * The form layout, detect rendering, and validation all flow through
 * `js/shared/agent-connections.js` so the welcome wizard reuses the
 * same code.
 */
class ConnectionSettingsModule extends SettingsModule {
    constructor() {
        super('connection', 'Agent Connection', '🔌');
        this._connections = [];
        this._activeId = '';
        this._presets = [];
        this._detected = []; // raw `detect_agents` results
        this._detectLoading = true;
        // Cached validation results: id -> { ok, issues, resolved_path }
        this._issues = new Map();
        // Master-detail mode: 'list' or 'edit'.
        this._view = 'list';
        // The connection currently being edited. When entering edit
        // mode for a detected agent, we synthesize a draft connection
        // here (not yet in `_connections`); on save we add it.
        this._editing = null;
        // Snapshot for restart-detection (compared on save).
        this._initialActiveModeJson = '';
        this._initialWorkingDir = null;
        this._needsRestart = false;
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                <div id="connPageRoot"></div>
            </div>
        `;
    }

    load(config) {
        const acp = config.acp || {};
        const agentCfg = acp.agent || {};

        this._connections = Array.isArray(acp.connections) ? acp.connections.slice() : [];
        if (!this._connections.length) {
            this._connections = [
                {
                    id: 'default',
                    name: 'Default',
                    preset_id: null,
                    mode: {
                        type: 'remote',
                        host: '127.0.0.1',
                        port: 8765,
                        timeout_ms: 30000,
                    },
                    sessions_directory: null,
                },
            ];
        }
        this._activeId =
            acp.active_connection_id && this._hasId(acp.active_connection_id)
                ? acp.active_connection_id
                : this._connections[0].id;

        const active = this._activeConnection();
        this._initialActiveModeJson = JSON.stringify(active?.mode || null);
        this._initialActiveSessionsDir = active?.sessions_directory ?? null;
        this._initialWorkingDir = agentCfg.working_directory || null;
        this._needsRestart = false;

        // Stash the agent-config bits we own for save() to flush back.
        this._startSessionOnLaunch = agentCfg.start_session_on_launch !== false;
        this._workingDirectory = agentCfg.working_directory || '';

        this._view = 'list';
        this._editing = null;
        this._renderRoot();
        this._kickValidation();
        this._kickDetect();
        this._kickPresetLoad();
    }

    save(config) {
        if (!config.acp) config.acp = {};

        // Pull any in-progress edit into the working list before save.
        this._captureEdit();

        config.acp.connections = this._connections;
        config.acp.active_connection_id = this._activeId;

        const existingAgent = config.acp.agent || {};
        existingAgent.start_session_on_launch =
            document.getElementById('startSessionOnLaunch')?.checked ??
            this._startSessionOnLaunch ??
            true;
        existingAgent.working_directory =
            document.getElementById('workingDirectory')?.value?.trim() ||
            this._workingDirectory?.trim() ||
            null;
        // The legacy agent-level `sessions_directory` is gone (it's now
        // per-connection). Drop any stale value left over from older
        // configs so it doesn't keep round-tripping.
        delete existingAgent.sessions_directory;
        config.acp.agent = existingAgent;

        const active = this._activeConnection();
        const activeMode = JSON.stringify(active?.mode || null);
        const activeSessionsDir = active?.sessions_directory ?? null;
        const wd = existingAgent.working_directory || null;
        const modeChanged = activeMode !== this._initialActiveModeJson;
        const wdChanged = wd !== this._initialWorkingDir;
        const sessChanged = activeSessionsDir !== this._initialActiveSessionsDir;
        this._needsRestart = modeChanged || wdChanged || sessChanged;
    }

    validate() {
        this._captureEdit();
        const active = this._activeConnection();
        if (!active) {
            return { valid: false, error: 'No active connection selected.' };
        }
        if (active.mode?.type === 'local') {
            if (!(active.mode.spawn_command || '').trim()) {
                return {
                    valid: false,
                    error: 'The active connection is missing a spawn command.',
                };
            }
        }
        return { valid: true };
    }

    initialize() {
        // No-op: the dynamic list/edit views wire their own handlers
        // each time _renderRoot runs.
    }

    // -----------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------

    _api() {
        return window.kageAgentConnections;
    }

    _hasId(id) {
        return this._connections.some((c) => c.id === id);
    }

    _activeConnection() {
        return this._connections.find((c) => c.id === this._activeId) || this._connections[0];
    }

    /**
     * If the edit view is active, pull the current form values back
     * into `_connections` (or create the new entry for a draft).
     * Called before any operation that re-renders or saves.
     */
    _captureEdit() {
        if (this._view !== 'edit' || !this._editing) return;
        const api = this._api();
        if (!api) return;
        const form = api.readEditForm('connEdit', this._editing);
        if (!form) return;
        const idx = this._connections.findIndex((c) => c.id === this._editing.id);
        const merged = {
            ...this._editing,
            ...form,
            id: this._editing.id,
            // Preserve the preset_id from the saved entry; the form
            // doesn't expose it.
            preset_id: this._editing.preset_id || form.preset_id || null,
        };
        if (idx >= 0) {
            this._connections[idx] = merged;
        } else {
            this._connections.push(merged);
        }
        this._editing = merged;
    }

    _renderRoot() {
        const root = document.getElementById('connPageRoot');
        if (!root) return;
        if (this._view === 'edit') {
            this._renderEditView(root);
        } else {
            this._renderListView(root);
        }
    }

    // ---------------- LIST VIEW --------------------------------------

    _renderListView(root) {
        const api = this._api();
        const escape = api?.escapeHtml || ((s) => s);
        const platform = navigator.platform || '';
        const wdPlaceholder = platform.startsWith('Win')
            ? 'e.g., C:\\Projects\\my-app'
            : 'e.g., /home/you/projects/my-app';

        const active = this._activeConnection();
        const others = this._connections.filter((c) => c.id !== this._activeId);
        const detectedNotSaved = (this._detected || []).filter(
            (d) => !this._connections.some((c) => this._matchesDetected(c, d))
        );

        root.innerHTML = `
            <div class="setting-section-label">Session</div>
            <div class="setting-row">
                <div class="setting-label">Start agent backend on launch</div>
                <div class="setting-checkbox-row">
                    <label class="kage-checkbox">
                        <input type="checkbox" id="startSessionOnLaunch"${this._startSessionOnLaunch ? ' checked' : ''}>
                    </label>
                    <div class="setting-description">
                        Speed up initial responses by pre-launching the ACP backend when Kage starts.
                    </div>
                </div>
            </div>
            <div class="setting-row">
                <div class="setting-label">Agent workspace folder</div>
                <div class="setting-description">
                    The folder the agent works in. It can read and modify files under this path.
                    Leave empty to use the current directory.
                </div>
                <div class="setting-control">
                    <input type="text" class="setting-input" id="workingDirectory"
                        value="${escape(this._workingDirectory)}"
                        placeholder="${escape(wdPlaceholder)}">
                </div>
            </div>

            <div class="setting-section-label">Connections</div>
            <div class="setting-row">
                <div class="setting-description">
                    The active connection is what Kage uses to run sessions. Switching the active
                    connection requires a restart.
                </div>
                <div id="connUnifiedList" class="conn-saved-list">
                    ${active ? this._renderConnectionRow(active, { active: true }) : ''}
                    ${others.map((c) => this._renderConnectionRow(c, { active: false })).join('')}
                    ${
                        this._detectLoading
                            ? `<div class="conn-detected-subhead">Auto-detected agents</div>
                               <div class="agent-searching">🔍 Searching for agents…</div>`
                            : detectedNotSaved.length
                              ? `<div class="conn-detected-subhead">Auto-detected agents</div>
                                 ${detectedNotSaved.map((d) => this._renderDetectedRow(d)).join('')}`
                              : ''
                    }
                </div>
                <div class="conn-list-actions">
                    <button type="button" class="setting-btn-secondary" id="connAddManualBtn">+ New connection</button>
                    <button type="button" class="setting-btn-secondary" id="connRescanBtn">Rescan</button>
                </div>
            </div>
        `;

        this._wireListHandlers();
    }

    _renderConnectionRow(c, { active }) {
        const api = this._api();
        const escape = api?.escapeHtml || ((s) => s);
        const issues = this._issues.get(c.id);
        const detail =
            c.mode?.type === 'local'
                ? c.mode.spawn_command || '(no command)'
                : `${c.mode?.host || '?'}:${c.mode?.port || '?'}`;
        const issueBadge =
            issues && !issues.ok
                ? `<span class="conn-issue-badge" title="${escape(
                      issues.issues.map((i) => api?.describeIssue?.(i) || i).join(' ')
                  )}">⚠️ ${escape(issues.issues.length)} issue${issues.issues.length === 1 ? '' : 's'}</span>`
                : '';
        const presetBadge = c.preset_id
            ? `<span class="conn-preset-badge">${escape(c.preset_id)}</span>`
            : '';
        const canDelete = !active && this._connections.length > 1;
        return `
            <div class="conn-row${active ? ' conn-row-active' : ''}" data-id="${escape(c.id)}">
                <div class="conn-row-main">
                    <div class="conn-row-name">
                        ${active ? '<span class="conn-active-dot" title="Active"></span>' : ''}
                        <span class="conn-row-title">${escape(c.name)}</span>
                        ${presetBadge}
                        ${issueBadge}
                        ${active ? '<span class="conn-active-label">Active</span>' : ''}
                    </div>
                    <div class="conn-row-detail">${escape(detail)}</div>
                </div>
                <div class="conn-row-actions">
                    ${
                        active
                            ? ''
                            : `<button type="button" class="setting-btn-secondary conn-set-active-btn" data-id="${escape(c.id)}">Make active</button>`
                    }
                    <button type="button" class="setting-btn-secondary conn-edit-btn" data-id="${escape(c.id)}">Edit</button>
                    ${
                        canDelete
                            ? `<button type="button" class="setting-btn-secondary conn-delete-btn" data-id="${escape(c.id)}">Delete</button>`
                            : ''
                    }
                </div>
            </div>`;
    }

    _renderDetectedRow(d) {
        const api = this._api();
        const escape = api?.escapeHtml || ((s) => s);
        const detail = d.path || d.spawn_command || '';
        const versionBadge = d.version
            ? `<span class="conn-version-badge">${escape(d.version.split(/\s+/)[0] || d.version)}</span>`
            : '';
        return `
            <div class="conn-row conn-row-detected" data-detected-key="${escape(d.path)}">
                <div class="conn-row-main">
                    <div class="conn-row-name">
                        <span class="conn-row-title">${escape(d.name)}</span>
                        <span class="conn-preset-badge">${escape(d.preset_id || 'detected')}</span>
                        ${versionBadge}
                    </div>
                    <div class="conn-row-detail">${escape(detail)}</div>
                </div>
                <div class="conn-row-actions">
                    <button type="button" class="setting-btn-secondary conn-add-detected-btn"
                            data-detected-key="${escape(d.path)}">Add</button>
                </div>
            </div>`;
    }

    /**
     * Decide whether a saved connection corresponds to a detected
     * agent — used to suppress the "Detected" entry once the user has
     * adopted it.
     */
    _matchesDetected(connection, detected) {
        if (connection.mode?.type !== 'local') return false;
        const cmd = connection.mode.spawn_command || '';
        if (!cmd) return false;
        if (cmd === detected.spawn_command) return true;
        if (cmd.startsWith(detected.path)) return true;
        return false;
    }

    _wireListHandlers() {
        document.getElementById('connRescanBtn')?.addEventListener('click', () => {
            this._detectLoading = true;
            this._renderRoot();
            this._kickDetect();
        });
        document.getElementById('connAddManualBtn')?.addEventListener('click', () => {
            this._enterEdit(this._draftBlankConnection(), { isNew: true });
        });

        document.querySelectorAll('.conn-set-active-btn').forEach((btn) => {
            btn.addEventListener('click', () => {
                this._activeId = btn.getAttribute('data-id');
                this._renderRoot();
            });
        });
        document.querySelectorAll('.conn-edit-btn').forEach((btn) => {
            btn.addEventListener('click', () => {
                const id = btn.getAttribute('data-id');
                const conn = this._connections.find((c) => c.id === id);
                if (conn) this._enterEdit(conn, { isNew: false });
            });
        });
        document.querySelectorAll('.conn-delete-btn').forEach((btn) => {
            btn.addEventListener('click', () => {
                this._deleteConnection(btn.getAttribute('data-id'));
            });
        });
        document.querySelectorAll('.conn-add-detected-btn').forEach((btn) => {
            btn.addEventListener('click', () => {
                const key = btn.getAttribute('data-detected-key');
                const detected = (this._detected || []).find((d) => d.path === key);
                if (!detected) return;
                const draft = this._api().connectionFromDetected(detected);
                this._enterEdit(draft, { isNew: true });
            });
        });
    }

    _draftBlankConnection() {
        const api = this._api();
        return {
            id: api ? api.uuidLite() : `c-${Date.now()}`,
            name: 'New connection',
            preset_id: null,
            mode: { type: 'local', spawn_command: '' },
            sessions_directory: null,
        };
    }

    // ---------------- EDIT VIEW --------------------------------------

    _enterEdit(connection, { isNew }) {
        this._editing = { ...connection };
        this._editingIsNew = isNew;
        this._view = 'edit';
        this._renderRoot();
    }

    _exitEditWithoutSaving() {
        // For brand-new drafts that were never committed, just drop
        // them. For existing connections, restore from the saved list
        // (the in-memory `_connections` already reflects the canonical
        // state — captureEdit hasn't run on this path).
        this._editing = null;
        this._editingIsNew = false;
        this._view = 'list';
        this._renderRoot();
    }

    _saveEdit() {
        const api = this._api();
        if (!api || !this._editing) return;
        const form = api.readEditForm('connEdit', this._editing);
        if (!form) return;
        // Validation: local mode requires a non-empty spawn command.
        if (form.mode?.type === 'local' && !(form.mode.spawn_command || '').trim()) {
            alert('Spawn command is required for local connections.');
            return;
        }
        const merged = {
            ...this._editing,
            ...form,
            id: this._editing.id,
            preset_id: this._editing.preset_id || form.preset_id || null,
        };
        const idx = this._connections.findIndex((c) => c.id === merged.id);
        if (idx >= 0) {
            this._connections[idx] = merged;
        } else {
            this._connections.push(merged);
            // First-add affordance: if the user had no real
            // connections yet (only the auto-created Default), make
            // the new one active for them.
            if (
                this._editingIsNew &&
                this._connections.length === 2 &&
                this._activeId === 'default'
            ) {
                const def = this._connections.find((c) => c.id === 'default');
                if (def && def.name === 'Default' && def.preset_id === null) {
                    this._activeId = merged.id;
                    this._connections = this._connections.filter((c) => c.id !== 'default');
                }
            }
        }
        this._validateOne(merged.id);
        this._editing = null;
        this._editingIsNew = false;
        this._view = 'list';
        this._renderRoot();
    }

    _renderEditView(root) {
        const api = this._api();
        const escape = api?.escapeHtml || ((s) => s);
        if (!api || !this._editing) {
            root.innerHTML = '';
            return;
        }
        const preset = (this._presets || []).find((p) => p.id === this._editing.preset_id);
        const presetBlock = preset
            ? `
                <div class="conn-preset-info">
                    <div class="conn-preset-info-title">${escape(preset.display_name)}</div>
                    <div class="conn-preset-info-desc">${escape(preset.description)}</div>
                    ${
                        preset.requires_auth && preset.auth_hint
                            ? `<div class="conn-preset-info-auth">🔑 ${escape(preset.auth_hint)}</div>`
                            : ''
                    }
                    <a href="${escape(preset.install_url)}" target="_blank" class="conn-preset-info-link">Install / docs ↗</a>
                </div>`
            : '';

        const isActive = this._editing.id === this._activeId;
        const subtitle = this._editingIsNew
            ? 'New connection'
            : isActive
              ? 'Editing the active connection'
              : 'Editing connection';

        root.innerHTML = `
            <div class="conn-edit-header">
                <button type="button" class="setting-btn-secondary conn-back-btn" id="connBackBtn">← Back</button>
                <div class="conn-edit-subtitle">${escape(subtitle)}</div>
            </div>
            ${presetBlock}
            <div id="connEditFormContainer">
                ${api.renderEditForm(this._editing, { idPrefix: 'connEdit', style: 'settings' })}
            </div>
            <div class="conn-edit-actions">
                <button type="button" class="setting-btn-secondary" id="connEditCancelBtn">Cancel</button>
                <button type="button" class="setting-btn-primary" id="connEditSaveBtn">${this._editingIsNew ? 'Add connection' : 'Save changes'}</button>
            </div>
        `;
        api.bindEditForm('connEdit');

        document
            .getElementById('connBackBtn')
            ?.addEventListener('click', () => this._exitEditWithoutSaving());
        document
            .getElementById('connEditCancelBtn')
            ?.addEventListener('click', () => this._exitEditWithoutSaving());
        document
            .getElementById('connEditSaveBtn')
            ?.addEventListener('click', () => this._saveEdit());
    }

    // ---------------- async background work --------------------------

    _kickValidation() {
        const api = this._api();
        if (!api) return;
        this._connections.forEach((c) => this._validateOne(c.id));
    }

    async _validateOne(id) {
        const api = this._api();
        const conn = this._connections.find((c) => c.id === id);
        if (!conn || !api) return;
        const result = await api.validateMode(conn.mode);
        this._issues.set(id, result);
        if (this._view === 'list') this._renderRoot();
    }

    async _kickDetect() {
        const api = this._api();
        if (!api) return;
        this._detectLoading = true;
        try {
            const invoke = window.__TAURI__?.core?.invoke;
            this._detected = invoke ? (await invoke('detect_agents')) || [] : [];
        } catch (e) {
            console.warn('detect_agents failed:', e);
            this._detected = [];
        }
        this._detectLoading = false;
        if (this._view === 'list') this._renderRoot();
    }

    async _kickPresetLoad() {
        const api = this._api();
        if (!api) return;
        this._presets = await api.listPresets();
        if (this._view === 'edit') this._renderRoot();
    }

    _deleteConnection(id) {
        if (this._connections.length <= 1) return;
        if (id === this._activeId) {
            alert('Switch to a different connection before deleting this one.');
            return;
        }
        this._connections = this._connections.filter((c) => c.id !== id);
        this._issues.delete(id);
        this._renderRoot();
    }
}
