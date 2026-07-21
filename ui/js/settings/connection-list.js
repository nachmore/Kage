import { t, tHtml } from '../shared/i18n.js';

// Lucide-style inline SVGs for the icon-only row actions. Matches the
// chat session list (`kd-action-btn`) so users see one consistent
// button shape across the app, instead of a mix of unicode glyphs (the
// monograph ⎘ is unreadable in many fonts) and text labels.
const ICON_EDIT = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z"/></svg>`;
const ICON_DUPLICATE = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>`;
const ICON_DELETE = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>`;

class ConnectionListMethods {
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
}

export function installConnectionListMethods(ConnectionSettingsModule) {
    const descriptors = Object.getOwnPropertyDescriptors(ConnectionListMethods.prototype);
    delete descriptors.constructor;
    Object.defineProperties(ConnectionSettingsModule.prototype, descriptors);
}
