import { SettingsModule } from './base.js';
import { errMessage } from '../shared/error-message.js';
import * as agentConnectionsApi from '../shared/agent-connections.js';
import { t } from '../shared/i18n.js';
import { installConnectionListMethods } from './connection-list.js';
import { installConnectionOllamaMethods } from './connection-ollama.js';

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

    _formatError(e) {
        return e ? errMessage(e) : t('settings.connection.error.unknown');
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

installConnectionListMethods(ConnectionSettingsModule);
installConnectionOllamaMethods(ConnectionSettingsModule);
