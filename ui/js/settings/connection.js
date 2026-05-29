import { SettingsModule } from './base.js';
import * as agentConnectionsApi from '../shared/agent-connections.js';
import { escapeAttr, formatBytes } from '../shared/tool-utils.js';
import { t, tHtml } from '../shared/i18n.js';

// Lucide-style inline SVGs for the icon-only row actions. Matches the
// chat session list (`kd-action-btn`) so users see one consistent
// button shape across the app, instead of a mix of unicode glyphs (the
// monograph ⎘ is unreadable in many fonts) and text labels.
const ICON_EDIT = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z"/></svg>`;
const ICON_DUPLICATE = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>`;
const ICON_DELETE = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>`;

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
        super('connection', t('settings.sidebar.agents'), '🤖');
        this._connections = [];
        this._activeId = '';
        this._presets = [];
        this._detected = []; // raw `detect_agents` results
        this._detectLoading = true;
        // Cached validation results: id -> { ok, issues, resolved_path }
        this._issues = new Map();
        // Cached version probes: id -> string ("0.0.0-dev", "2.5.0", …).
        // Only populated for connections whose binary actually replied.
        this._versions = new Map();
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
        this._kickVersionProbe();
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
            return { valid: false, error: t('settings.connection.validate.no_active') };
        }
        if (active.mode?.type === 'local') {
            if (!(active.mode.spawn_command || '').trim()) {
                return {
                    valid: false,
                    error: t('settings.connection.validate.missing_spawn'),
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
            ? t('settings.connection.workspace.placeholder.windows')
            : t('settings.connection.workspace.placeholder.unix');

        const active = this._activeConnection();
        const others = this._connections.filter((c) => c.id !== this._activeId);
        const detectedNotSaved = (this._detected || []).filter(
            (d) => !this._connections.some((c) => this._matchesDetected(c, d))
        );

        root.innerHTML = `
            <div class="setting-section-label">${t('settings.connection.session.section')}</div>
            <div class="setting-row">
                <div class="setting-label">${t('settings.connection.session.start_on_launch.label')}</div>
                <div class="setting-checkbox-row">
                    <label class="kage-checkbox">
                        <input type="checkbox" id="startSessionOnLaunch"${this._startSessionOnLaunch ? ' checked' : ''}>
                    </label>
                    <div class="setting-description">
                        ${t('settings.connection.session.start_on_launch.description')}
                    </div>
                </div>
            </div>
            <div class="setting-row">
                <div class="setting-label">${t('settings.connection.workspace.label')}</div>
                <div class="setting-description">
                    ${t('settings.connection.workspace.description')}
                </div>
                <div class="setting-control">
                    <input type="text" class="setting-input" id="workingDirectory"
                        value="${escape(this._workingDirectory)}"
                        placeholder="${escape(wdPlaceholder)}">
                </div>
            </div>

            <div class="setting-section-label">${t('settings.connection.list.section')}</div>
            <div class="setting-row">
                <div class="setting-description">
                    ${t('settings.connection.list.description')}
                </div>
                <div id="connUnifiedList" class="conn-saved-list">
                    ${active ? this._renderConnectionRow(active, { active: true }) : ''}
                    ${others.map((c) => this._renderConnectionRow(c, { active: false })).join('')}
                    ${
                        this._detectLoading
                            ? `<div class="conn-detected-subhead">${t('settings.connection.list.detected_subhead')}</div>
                               <div class="agent-searching">${t('settings.connection.list.searching')}</div>`
                            : detectedNotSaved.length
                              ? `<div class="conn-detected-subhead">${t('settings.connection.list.detected_subhead')}</div>
                                 ${detectedNotSaved.map((d) => this._renderDetectedRow(d)).join('')}`
                              : ''
                    }
                </div>
                <div class="conn-list-actions">
                    <button type="button" class="setting-btn-secondary" id="connAddBtn">${t('settings.connection.list.add_btn')}</button>
                    <button type="button" class="setting-btn-secondary" id="connRescanBtn">${t('settings.connection.list.rescan_btn')}</button>
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
                ? c.mode.spawn_command || t('settings.connection.row.no_command')
                : `${c.mode?.host || '?'}:${c.mode?.port || '?'}`;
        const issueBadge =
            issues && !issues.ok
                ? `<span class="conn-issue-badge" title="${escape(
                      issues.issues.map((i) => api?.describeIssue?.(i) || i).join(' ')
                  )}">${t('settings.connection.row.issue_count', { count: issues.issues.length })}</span>`
                : '';
        const presetBadge = c.preset_id
            ? `<span class="conn-preset-badge">${escape(c.preset_id)}</span>`
            : '';
        const version = this._versions.get(c.id);
        const versionBadge = version
            ? `<span class="conn-version-badge">${escape(version)}</span>`
            : '';
        const canDelete = !active && this._connections.length > 1;
        return `
            <div class="conn-row${active ? ' conn-row-active' : ''}" data-id="${escape(c.id)}">
                <div class="conn-row-main">
                    <div class="conn-row-name">
                        ${active ? `<span class="conn-active-dot" title="${t('settings.connection.row.active.title')}"></span>` : ''}
                        <span class="conn-row-title">${escape(c.name)}</span>
                        ${presetBadge}
                        ${versionBadge}
                        ${issueBadge}
                        ${active ? `<span class="conn-active-label">${t('settings.connection.row.active.title')}</span>` : ''}
                    </div>
                    <div class="conn-row-detail">${escape(detail)}</div>
                </div>
                <div class="conn-row-actions">
                    ${
                        active
                            ? ''
                            : `<button type="button" class="setting-btn-secondary conn-set-active-btn" data-id="${escape(c.id)}">${t('settings.connection.row.make_active_btn')}</button>`
                    }
                    <button type="button" class="conn-icon-btn conn-edit-btn" data-id="${escape(c.id)}"
                            title="${t('settings.connection.row.edit_title')}" aria-label="${t('settings.connection.row.edit_title')}">
                        ${ICON_EDIT}
                    </button>
                    <button type="button" class="conn-icon-btn conn-duplicate-btn" data-id="${escape(c.id)}"
                            title="${t('settings.connection.row.duplicate_title')}" aria-label="${t('settings.connection.row.duplicate_title')}">
                        ${ICON_DUPLICATE}
                    </button>
                    ${
                        canDelete
                            ? `<button type="button" class="conn-icon-btn conn-delete-btn" data-id="${escape(c.id)}"
                                    title="${t('settings.connection.row.delete_title')}" aria-label="${t('settings.connection.row.delete_title')}">
                                ${ICON_DELETE}
                            </button>`
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
            ? `<span class="conn-version-badge">${escape(d.version)}</span>`
            : '';
        // Wrapper-needed entries (bare `claude`) get a different action
        // — they can't be added as a saved connection until the wrapper
        // is installed via npm.
        if (d.needs_wrapper_npm_package) {
            return `
                <div class="conn-row conn-row-detected conn-row-wrapper-needed" data-detected-key="${escape(d.path)}">
                    <div class="conn-row-main">
                        <div class="conn-row-name">
                            <span class="conn-row-title">${escape(d.name)}</span>
                            <span class="conn-preset-badge">${t('settings.connection.detected.needs_wrapper')}</span>
                        </div>
                        <div class="conn-row-detail">${escape(detail)}</div>
                        <div class="conn-row-detail conn-wrapper-hint">
                            ${tHtml('settings.connection.detected.wrapper_hint_html', { package: d.needs_wrapper_npm_package })}
                        </div>
                        <div class="agent-install-status" data-detected-key="${escape(d.path)}" aria-live="polite"></div>
                    </div>
                    <div class="conn-row-actions">
                        <button type="button" class="setting-btn-secondary conn-install-wrapper-btn"
                                data-detected-key="${escape(d.path)}">${t('settings.connection.detected.install_wrapper_btn')}</button>
                    </div>
                </div>`;
        }
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
                            data-detected-key="${escape(d.path)}">${t('settings.connection.detected.add_btn')}</button>
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
        document.querySelectorAll('.conn-duplicate-btn').forEach((btn) => {
            btn.addEventListener('click', () => {
                this._duplicateConnection(btn.getAttribute('data-id'));
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
        document.querySelectorAll('.conn-install-wrapper-btn').forEach((btn) => {
            btn.addEventListener('click', async () => {
                const key = btn.getAttribute('data-detected-key');
                const detected = (this._detected || []).find((d) => d.path === key);
                if (!detected) return;
                await this._handleInstallWrapper(detected, btn);
            });
        });
    }

    async _handleInstallWrapper(detected, btn) {
        const root = document.getElementById('connPageRoot');
        const status = root?.querySelector(
            `.agent-install-status[data-detected-key="${CSS.escape(detected.path)}"]`
        );
        const setStatus = (html) => {
            if (status) status.innerHTML = html;
        };
        const escape = this._api()?.escapeHtml || ((s) => s);

        const originalLabel = btn.textContent;
        btn.disabled = true;
        btn.textContent = t('settings.connection.install.checking_npm');
        setStatus('');

        const invoke = window.__TAURI__?.core?.invoke;
        if (!invoke) {
            btn.disabled = false;
            btn.textContent = originalLabel;
            return;
        }

        let npm;
        try {
            npm = await invoke('check_npm_available');
        } catch (e) {
            console.warn('check_npm_available failed:', e);
            npm = { available: false };
        }

        const cmd = `npm install -g ${detected.needs_wrapper_npm_package}`;
        if (!npm?.available) {
            btn.disabled = false;
            btn.textContent = originalLabel;
            setStatus(tHtml('settings.connection.install.no_npm_html', { cmd }));
            return;
        }

        btn.textContent = t('settings.connection.install.installing');
        try {
            await invoke('install_acp_wrapper', {
                package: detected.needs_wrapper_npm_package,
            });
        } catch (e) {
            console.warn('install_acp_wrapper failed:', e);
            btn.disabled = false;
            btn.textContent = originalLabel;
            const msg =
                e?.message ||
                (typeof e === 'string' ? e : t('settings.connection.install.failed_default'));
            setStatus(
                tHtml('settings.connection.install.failed_html', {
                    message: msg,
                    cmd,
                })
            );
            return;
        }

        setStatus(tHtml('settings.connection.install.success_html'));
        // Re-detect — the wrapper now shows up as a ready-to-use entry,
        // and the backend filter hides the wrapper-needed entry.
        this._detectLoading = true;
        this._renderRoot();
        await this._kickDetect();
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
            name: t('settings.connection.draft.default_name'),
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
            alert(t('settings.connection.edit.spawn_required'));
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
        this._probeVersionOne(merged.id);
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
                    <a href="${escape(preset.install_url)}" target="_blank" class="conn-preset-info-link">${t('settings.connection.edit.preset_install_link')}</a>
                </div>`
            : '';

        const isActive = this._editing.id === this._activeId;
        const subtitle = this._editingIsNew
            ? t('settings.connection.edit.subtitle.new')
            : isActive
              ? t('settings.connection.edit.subtitle.active')
              : t('settings.connection.edit.subtitle.editing');

        root.innerHTML = `
            <div class="conn-edit-header">
                <button type="button" class="setting-btn-secondary conn-back-btn" id="connBackBtn">${t('settings.connection.edit.back_btn')}</button>
                <div class="conn-edit-subtitle">${escape(subtitle)}</div>
            </div>
            ${presetBlock}
            <div id="connEditFormContainer">
                ${api.renderEditForm(this._editing, { idPrefix: 'connEdit', style: 'settings' })}
            </div>
            <div class="conn-edit-actions">
                <button type="button" class="setting-btn-secondary" id="connEditCancelBtn">${t('settings.connection.edit.cancel_btn')}</button>
                <button type="button" class="setting-btn-primary" id="connEditSaveBtn">${escape(this._editingIsNew ? t('settings.connection.edit.add_btn') : t('settings.connection.edit.save_changes_btn'))}</button>
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
        const subtitle = this._editingIsNew
            ? t('settings.connection.ollama.subtitle.new')
            : t('settings.connection.ollama.subtitle.editing');
        const settings = this._editing.ollama_settings || {};
        const widgetModel =
            settings.model || t('settings.connection.ollama.show_widget.fallback_model');
        root.innerHTML = `
            <div class="conn-edit-header">
                <button type="button" class="setting-btn-secondary conn-back-btn" id="connBackBtn">${t('settings.connection.edit.back_btn')}</button>
                <div class="conn-edit-subtitle">🦙 ${escape(subtitle)}</div>
            </div>
            <p class="setting-description" style="margin-bottom:12px;">
                ${t('settings.connection.ollama.intro_html')}
            </p>

            <div class="setting-row">
                <div class="setting-label">${t('settings.connection.ollama.name.label')}</div>
                <div class="setting-description">${t('settings.connection.ollama.name.description')}</div>
                <div class="setting-control">
                    <input type="text" class="setting-input" id="ollEditName"
                        value="${escape(this._editing.name || '')}"
                        placeholder="${t('settings.connection.ollama.name.placeholder')}">
                </div>
            </div>

            <div class="setting-row">
                <div class="setting-label">${t('settings.connection.ollama.base_url.label')}</div>
                <div class="setting-description">${t('settings.connection.ollama.base_url.description')}</div>
                <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                    <input type="text" class="setting-input" id="ollEditBaseUrl"
                        value="${escape(settings.base_url || 'http://localhost:11434')}"
                        placeholder="http://localhost:11434" style="flex:1;">
                    <button class="setting-button" type="button" id="ollEditTestBtn">${t('settings.connection.ollama.test_btn')}</button>
                </div>
                <div id="ollEditProbeStatus" class="setting-description" style="margin-top:8px;min-height:1em;"></div>
            </div>

            <div class="setting-row">
                <div class="setting-label">${t('settings.connection.ollama.model.label')}</div>
                <div class="setting-description">${t('settings.connection.ollama.model.description')}</div>
                <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                    <select class="setting-select" id="ollEditModel" style="flex:1;">
                        <option value="">${t('settings.connection.ollama.dropdown.empty')}</option>
                    </select>
                    <button class="setting-button" type="button" id="ollEditRefreshBtn">${t('settings.connection.ollama.refresh_btn')}</button>
                </div>
                <div id="ollEditModelHint" class="setting-description" style="margin-top:8px;min-height:1em;"></div>
            </div>

            <div class="setting-row">
                <div class="setting-label">${t('settings.connection.ollama.show_widget.label')}</div>
                <div class="setting-checkbox-row">
                    <label class="kage-checkbox">
                        <input type="checkbox" id="ollEditShowWidget"${settings.show_status_widget ? ' checked' : ''}>
                    </label>
                    <div class="setting-description">
                        ${tHtml('settings.connection.ollama.show_widget.description_html', { model: widgetModel })}
                    </div>
                </div>
            </div>

            <details style="margin-top:12px;">
                <summary style="cursor:pointer;font-size:12px;color:var(--kage-text-secondary);">${t('settings.connection.ollama.help.summary')}</summary>
                <div class="setting-description" style="margin-top:8px;line-height:1.6;">
                    ${t('settings.connection.ollama.help.body_html')}
                </div>
            </details>

            <div class="conn-edit-actions">
                <button type="button" class="setting-btn-secondary" id="connEditCancelBtn">${t('settings.connection.edit.cancel_btn')}</button>
                <button type="button" class="setting-btn-primary" id="ollEditSaveBtn">${escape(this._editingIsNew ? t('settings.connection.ollama.add_btn') : t('settings.connection.edit.save_changes_btn'))}</button>
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
            status.textContent = t('settings.connection.ollama.probe.probing');
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
                const versionSuffix = this._ollamaProbe.version
                    ? t('settings.connection.ollama.probe.version_suffix', {
                          version: this._ollamaProbe.version,
                      })
                    : '';
                status.textContent = t('settings.connection.ollama.probe.reachable', {
                    version: versionSuffix,
                });
                status.style.color = 'var(--kage-accent)';
            } else if (this._ollamaProbe) {
                status.textContent = t('settings.connection.ollama.probe.unreachable', {
                    reason:
                        this._ollamaProbe.reason ||
                        t('settings.connection.ollama.probe.unreachable_default'),
                });
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
            hint.textContent = t('settings.connection.ollama.models.loading');
            hint.style.color = '';
        }
        let models = [];
        try {
            models = await window.__TAURI__.core.invoke('ollama_list_models', { baseUrl });
        } catch (e) {
            this._ollamaModels = [];
            this._populateOllamaModelDropdown([], '');
            if (hint) {
                hint.textContent = t('settings.connection.ollama.models.list_failed', {
                    message: this._formatError(e),
                });
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
                hint.textContent = t('settings.connection.ollama.models.none');
                hint.style.color = '';
            } else {
                hint.textContent = t('settings.connection.ollama.models.count', {
                    count: this._ollamaModels.length,
                });
                hint.style.color = 'var(--kage-text-secondary)';
            }
        }
    }

    _populateOllamaModelDropdown(models, selected) {
        const sel = document.getElementById('ollEditModel');
        if (!sel) return;
        const opts = [
            `<option value="">${t('settings.connection.ollama.dropdown.empty')}</option>`,
        ];
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
            setStatus(t('settings.connection.ollama.save.pick_model'), 'error');
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
            setStatus(
                t('settings.connection.ollama.save.spawn_failed', {
                    message: this._formatError(e),
                }),
                'error'
            );
            return;
        }

        const showWidget = !!document.getElementById('ollEditShowWidget')?.checked;
        const merged = {
            id: this._editing.id,
            // Default the friendly name to "Ollama: <model>" on save
            // when the user left it blank — matches the templating
            // convention agreed with the user.
            name: enteredName || t('settings.connection.ollama.save.default_name', { model }),
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
        this._probeVersionOne(merged.id);
        this._editing = null;
        this._editingIsNew = false;
        this._view = 'list';
        this._renderRoot();
    }

    _formatError(e) {
        if (!e) return t('settings.connection.error.unknown');
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

    /** Probe a version string for every local saved connection. Remote
     * connections are skipped — there's no local binary to query. */
    _kickVersionProbe() {
        this._connections
            .filter((c) => c.mode?.type === 'local')
            .forEach((c) => this._probeVersionOne(c.id));
    }

    async _probeVersionOne(id) {
        const conn = this._connections.find((c) => c.id === id);
        if (!conn || conn.mode?.type !== 'local') return;
        const cmd = (conn.mode.spawn_command || '').trim();
        if (!cmd) return;
        const invoke = window.__TAURI__?.core?.invoke;
        if (!invoke) return;
        let version = null;
        try {
            version = await invoke('probe_connection_version', {
                spawnCommand: cmd,
                presetId: conn.preset_id || null,
            });
        } catch (e) {
            console.warn('probe_connection_version failed:', e);
        }
        if (version) {
            this._versions.set(id, version);
            if (this._view === 'list') this._renderRoot();
        }
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
            alert(t('settings.connection.edit.cannot_delete_active'));
            return;
        }
        this._connections = this._connections.filter((c) => c.id !== id);
        this._issues.delete(id);
        this._renderRoot();
    }

    /**
     * Open the editor with a deep-copied draft of `id`. The draft gets
     * a fresh uuid + "Copy of …" name; nothing is added to the saved
     * list until the user hits Save in the editor (the standard new-
     * draft contract). Routes Ollama-shaped connections to the wizard
     * sub-view since the raw spawn-command form can't read the
     * `ollama_settings` block back out.
     */
    _duplicateConnection(id) {
        const original = this._connections.find((c) => c.id === id);
        if (!original) return;
        const api = this._api();
        // Structured-clone the whole record so nested `mode` /
        // `ollama_settings` objects don't share references with the
        // saved entry — edits to the draft must not mutate the
        // original until Save commits them.
        const draft = JSON.parse(JSON.stringify(original));
        draft.id = api?.uuidLite ? api.uuidLite() : `c-${Date.now()}`;
        draft.name = t('settings.connection.duplicate.copy_of', {
            name: original.name || t('settings.connection.duplicate.fallback_name'),
        });
        if (original.preset_id === 'ollama') {
            this._enterOllamaEdit(draft, { isNew: true });
        } else {
            this._enterEdit(draft, { isNew: true });
        }
    }
}
