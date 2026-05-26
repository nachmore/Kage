import { SettingsModule } from './base.js';
import * as agentConnectionsApi from '../shared/agent-connections.js';
import { formatBytes } from '../shared/tool-utils.js';
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
export class ConnectionSettingsModule extends SettingsModule {
    constructor() {
        // Section id stays `connection` so deep links (>logs-style
        // settings-subsection routes, the >version → Updates panel,
        // and any external tutorials) keep working. Display name +
        // sidebar label are user-facing and renamed to "Agents" —
        // since this page now also owns the Ollama wizard, "Agent
        // Connection" understated what's here.
        super('connection', 'Agents', '🤖');
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
        return agentConnectionsApi;
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
        } else if (this._view === 'ollama-edit') {
            this._renderOllamaEditView(root);
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
                    <button type="button" class="setting-btn-secondary" id="connAddBtn">+ Add agent</button>
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
        document.getElementById('connAddBtn')?.addEventListener('click', async () => {
            // Pick a type first, then route to the matching sub-flow.
            // Rationale: each agent type has a different ideal editor
            // (raw spawn for advanced users, wizard for Ollama, preset
            // form for ACP-compatible agents). Showing the right
            // editor up front beats one-form-fits-all.
            const kind = await this._api().pickAgentType();
            if (!kind) return;
            await this._handleAddAgentType(kind);
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
                if (!conn) return;
                // Ollama-shaped connections route to the wizard
                // sub-view, not the raw spawn-command form. The
                // wizard understands ollama_settings + builds the
                // right shell incantation on save.
                if (conn.preset_id === 'ollama') {
                    this._enterOllamaEdit(conn, { isNew: false });
                } else {
                    this._enterEdit(conn, { isNew: false });
                }
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

    /**
     * Route a type-picker result to the right add flow.
     *
     *   - 'detect'     — fall through to detect; if exactly one
     *     auto-detected agent is unsaved, open it pre-filled in the
     *     standard edit form. Otherwise nudge the user toward the
     *     existing "Auto-detected agents" group on the list.
     *   - 'ollama'     — open the Ollama wizard with a fresh draft.
     *   - 'acp_preset' — same standard edit form, with preset metadata
     *     surfaced. We let the user pick a preset inside the form.
     *     For now this resolves to the same blank-connection editor
     *     the old "+ New connection" button used; preset selection
     *     UI inside the form is a P2 polish item.
     *   - 'custom'     — same blank-connection editor.
     */
    async _handleAddAgentType(kind) {
        if (kind === 'ollama') {
            this._enterOllamaEdit(null, { isNew: true });
            return;
        }
        if (kind === 'detect') {
            // Make sure detect has run at least once. If we already
            // have results, surface the first un-saved one.
            this._detectLoading = true;
            this._renderRoot();
            await this._kickDetect();
            const unsaved = (this._detected || []).filter(
                (d) => !this._connections.some((c) => this._matchesDetected(c, d))
            );
            if (unsaved.length > 0) {
                const draft = this._api().connectionFromDetected(unsaved[0]);
                this._enterEdit(draft, { isNew: true });
            }
            // If nothing auto-detected, the list view shows the
            // "Searching…" / "no agents found" copy and the user
            // can fall back to + Add agent → Custom.
            return;
        }
        // 'acp_preset' and 'custom' both use the blank editor today.
        this._enterEdit(this._draftBlankConnection(), { isNew: true });
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

    // ---------------- OLLAMA WIZARD SUB-VIEW -------------------------
    //
    // Reached when the user clicks Edit on an Ollama-shaped connection
    // (preset_id === 'ollama') or when the type picker resolves with
    // 'ollama' on Add. Same wizard the standalone Settings → Ollama
    // page used to host: probe the daemon, list models, save back as
    // a Codex-via-Ollama connection.
    //
    // The wizard owns one connection draft (`this._editing`) the same
    // way the standard edit view does. On save we write both the
    // shell-wrapped spawn_command and the round-trippable
    // ollama_settings { base_url, model } together — a future Edit
    // can read settings without parsing the spawn string.

    _enterOllamaEdit(connection, { isNew }) {
        // Seed a draft. For "new" connections the type picker calls
        // through with a half-formed object; we fill in defaults here.
        const seeded = connection?.ollama_settings || {};
        this._editing = {
            id: connection?.id || this._api().uuidLite(),
            name: connection?.name || '',
            preset_id: 'ollama',
            mode: connection?.mode || { type: 'local', spawn_command: '' },
            sessions_directory: connection?.sessions_directory || null,
            ollama_settings: {
                base_url: seeded.base_url || 'http://localhost:11434',
                model: seeded.model || '',
                show_status_widget: !!seeded.show_status_widget,
            },
        };
        this._editingIsNew = isNew;
        this._view = 'ollama-edit';
        // Cached probe + model list — cleared each enter so a re-edit
        // gets fresh data.
        this._ollamaProbe = null;
        this._ollamaModels = [];
        this._renderRoot();
        // Kick a background probe + model fetch immediately so the
        // page lands populated for users who already have Ollama
        // running.
        this._kickOllamaProbe(false);
    }

    _renderOllamaEditView(root) {
        const api = this._api();
        const escape = api?.escapeHtml || ((s) => s);
        if (!api || !this._editing) {
            root.innerHTML = '';
            return;
        }
        const subtitle = this._editingIsNew ? 'New Ollama agent' : 'Editing Ollama agent';
        const settings = this._editing.ollama_settings || {};
        root.innerHTML = `
            <div class="conn-edit-header">
                <button type="button" class="setting-btn-secondary conn-back-btn" id="connBackBtn">← Back</button>
                <div class="conn-edit-subtitle">🦙 ${escape(subtitle)}</div>
            </div>
            <p class="setting-description" style="margin-bottom:12px;">
                Use a local model running on <a href="https://ollama.com/" target="_blank" rel="noreferrer noopener">Ollama</a> with Kage.
                Free, private, no API key. Wired through the Codex ACP adapter — Kage handles the env-var dance for you.
            </p>

            <div class="setting-row">
                <div class="setting-label">Friendly name</div>
                <div class="setting-description">Shown in the connections list. Defaults to "Ollama: &lt;model&gt;" on save if left empty.</div>
                <div class="setting-control">
                    <input type="text" class="setting-input" id="ollEditName"
                        value="${escape(this._editing.name || '')}"
                        placeholder="e.g. Ollama: llama3:8b">
                </div>
            </div>

            <div class="setting-row">
                <div class="setting-label">Ollama base URL</div>
                <div class="setting-description">Where the Ollama daemon is listening. Default is the local install.</div>
                <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                    <input type="text" class="setting-input" id="ollEditBaseUrl"
                        value="${escape(settings.base_url || 'http://localhost:11434')}"
                        placeholder="http://localhost:11434" style="flex:1;">
                    <button class="setting-button" type="button" id="ollEditTestBtn">Test connection</button>
                </div>
                <div id="ollEditProbeStatus" class="setting-description" style="margin-top:8px;min-height:1em;"></div>
            </div>

            <div class="setting-row">
                <div class="setting-label">Model</div>
                <div class="setting-description">Pulled models from this Ollama daemon. Click Refresh to re-scan.</div>
                <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                    <select class="setting-select" id="ollEditModel" style="flex:1;">
                        <option value="">—</option>
                    </select>
                    <button class="setting-button" type="button" id="ollEditRefreshBtn">Refresh</button>
                </div>
                <div id="ollEditModelHint" class="setting-description" style="margin-top:8px;min-height:1em;"></div>
            </div>

            <div class="setting-row">
                <div class="setting-label">Show status widget in floating window</div>
                <div class="setting-checkbox-row">
                    <label class="kage-checkbox">
                        <input type="checkbox" id="ollEditShowWidget"${settings.show_status_widget ? ' checked' : ''}>
                    </label>
                    <div class="setting-description">
                        Adds a small "🦙 ${escape(settings.model || 'model')} · ready" chip near the top of the floating window so you can see at a glance that the local model is up. Polls Ollama every ~30 seconds.
                    </div>
                </div>
            </div>

            <details style="margin-top:12px;">
                <summary style="cursor:pointer;font-size:12px;color:var(--kage-text-secondary);">Don't have Ollama yet?</summary>
                <div class="setting-description" style="margin-top:8px;line-height:1.6;">
                    1. Install Ollama from <a href="https://ollama.com/download" target="_blank" rel="noreferrer noopener">ollama.com/download</a>.<br>
                    2. Start the daemon (it runs in the background).<br>
                    3. Pull a model — for example <code>ollama pull llama3</code> or <code>ollama pull qwen2.5-coder</code>.<br>
                    4. Click Test connection above, pick the model, then save.
                </div>
            </details>

            <div class="conn-edit-actions">
                <button type="button" class="setting-btn-secondary" id="connEditCancelBtn">Cancel</button>
                <button type="button" class="setting-btn-primary" id="ollEditSaveBtn">${this._editingIsNew ? 'Add agent' : 'Save changes'}</button>
            </div>
            <div id="ollEditStatus" class="setting-description" style="margin-top:8px;min-height:1em;"></div>
        `;

        // Re-populate the model dropdown if we already have a list
        // cached (e.g. user came back via Cancel + re-Edit).
        this._populateOllamaModelDropdown(this._ollamaModels, settings.model || '');

        document
            .getElementById('connBackBtn')
            ?.addEventListener('click', () => this._exitEditWithoutSaving());
        document
            .getElementById('connEditCancelBtn')
            ?.addEventListener('click', () => this._exitEditWithoutSaving());
        document
            .getElementById('ollEditTestBtn')
            ?.addEventListener('click', () => this._kickOllamaProbe(true));
        document
            .getElementById('ollEditRefreshBtn')
            ?.addEventListener('click', () => this._kickOllamaModelRefresh(true));
        document
            .getElementById('ollEditSaveBtn')
            ?.addEventListener('click', () => this._saveOllamaEdit());
    }

    _currentOllamaBaseUrl() {
        const v = document.getElementById('ollEditBaseUrl')?.value?.trim();
        return v || 'http://localhost:11434';
    }

    async _kickOllamaProbe(verbose) {
        const baseUrl = this._currentOllamaBaseUrl();
        const status = document.getElementById('ollEditProbeStatus');
        if (verbose && status) {
            status.textContent = 'Probing…';
            status.style.color = '';
        }
        try {
            this._ollamaProbe = await window.__TAURI__.core.invoke('ollama_probe', {
                baseUrl,
            });
        } catch (e) {
            this._ollamaProbe = { status: 'Unreachable', reason: this._formatError(e) };
        }
        if (status) {
            if (this._ollamaProbe?.status === 'Reachable') {
                const v = this._ollamaProbe.version ? ` (Ollama ${this._ollamaProbe.version})` : '';
                status.textContent = `✓ Reachable${v}`;
                status.style.color = 'var(--kage-accent)';
            } else if (this._ollamaProbe) {
                status.textContent = `✕ ${this._ollamaProbe.reason || 'Unreachable'}`;
                status.style.color = '#c44';
            }
        }
        // After a successful probe, freshen the model list. On a
        // failed probe, leave whatever was last loaded — the user
        // may want to reuse a known model name and just fix the URL
        // before Save.
        if (this._ollamaProbe?.status === 'Reachable') {
            await this._kickOllamaModelRefresh(false);
        }
    }

    async _kickOllamaModelRefresh(verbose) {
        const baseUrl = this._currentOllamaBaseUrl();
        const hint = document.getElementById('ollEditModelHint');
        if (verbose && hint) {
            hint.textContent = 'Loading…';
            hint.style.color = '';
        }
        let models = [];
        try {
            models = await window.__TAURI__.core.invoke('ollama_list_models', { baseUrl });
        } catch (e) {
            this._ollamaModels = [];
            this._populateOllamaModelDropdown([], '');
            if (hint) {
                hint.textContent = `Couldn't list models: ${this._formatError(e)}`;
                hint.style.color = '#c44';
            }
            return;
        }
        this._ollamaModels = Array.isArray(models) ? models : [];
        const previous =
            document.getElementById('ollEditModel')?.value ||
            this._editing?.ollama_settings?.model ||
            '';
        this._populateOllamaModelDropdown(this._ollamaModels, previous);
        if (hint) {
            if (this._ollamaModels.length === 0) {
                hint.textContent =
                    'No models pulled yet. Try `ollama pull llama3` or `ollama pull qwen2.5-coder`.';
                hint.style.color = '';
            } else {
                hint.textContent = `${this._ollamaModels.length} model${this._ollamaModels.length === 1 ? '' : 's'} available.`;
                hint.style.color = 'var(--kage-text-secondary)';
            }
        }
    }

    _populateOllamaModelDropdown(models, selected) {
        const sel = document.getElementById('ollEditModel');
        if (!sel) return;
        const escapeAttr = (s) =>
            String(s).replace(
                /[&<>"']/g,
                (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]
            );
        const opts = ['<option value="">—</option>'];
        for (const m of models || []) {
            const sizeStr = formatBytes(m.size);
            const label = sizeStr ? `${m.name} — ${sizeStr}` : m.name;
            opts.push(`<option value="${escapeAttr(m.name)}">${escapeAttr(label)}</option>`);
        }
        sel.innerHTML = opts.join('');
        if (selected) sel.value = selected;
    }

    async _saveOllamaEdit() {
        const status = document.getElementById('ollEditStatus');
        const setStatus = (text, kind) => {
            if (!status) return;
            status.textContent = text || '';
            status.style.color =
                kind === 'error' ? '#c44' : kind === 'success' ? 'var(--kage-accent)' : '';
        };
        const baseUrl = this._currentOllamaBaseUrl();
        const model = document.getElementById('ollEditModel')?.value?.trim();
        const enteredName = document.getElementById('ollEditName')?.value?.trim() || '';

        if (!model) {
            setStatus('Pick a model before saving.', 'error');
            return;
        }

        // Build the spawn command server-side so the env-var quoting
        // is correct for the host platform (Windows wraps with `cmd
        // /c set ...`, macOS / Linux uses `env ...`).
        let spawnCommand;
        try {
            spawnCommand = await window.__TAURI__.core.invoke('ollama_codex_spawn_command', {
                baseUrl,
                model,
            });
        } catch (e) {
            setStatus('Could not build spawn command: ' + this._formatError(e), 'error');
            return;
        }

        const showWidget = !!document.getElementById('ollEditShowWidget')?.checked;
        const merged = {
            id: this._editing.id,
            // Default the friendly name to "Ollama: <model>" on save
            // when the user left it blank — matches the templating
            // convention agreed with the user.
            name: enteredName || `Ollama: ${model}`,
            preset_id: 'ollama',
            mode: { type: 'local', spawn_command: spawnCommand },
            sessions_directory: this._editing.sessions_directory ?? null,
            ollama_settings: {
                base_url: baseUrl,
                model,
                show_status_widget: showWidget,
            },
        };
        const idx = this._connections.findIndex((c) => c.id === merged.id);
        if (idx >= 0) {
            this._connections[idx] = merged;
        } else {
            this._connections.push(merged);
            // Same first-add affordance as _saveEdit: replace the
            // auto-created "Default" with the new connection if the
            // user is just bootstrapping.
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
