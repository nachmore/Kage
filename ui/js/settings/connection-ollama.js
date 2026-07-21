import { escapeAttr, formatBytes } from '../shared/tool-utils.js';
import { t, tHtml } from '../shared/i18n.js';

class ConnectionOllamaMethods {
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
                status.style.color = 'var(--kage-error)';
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
                hint.style.color = 'var(--kage-error)';
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
                kind === 'error'
                    ? 'var(--kage-error)'
                    : kind === 'success'
                      ? 'var(--kage-accent)'
                      : '';
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
}

export function installConnectionOllamaMethods(ConnectionSettingsModule) {
    const descriptors = Object.getOwnPropertyDescriptors(ConnectionOllamaMethods.prototype);
    delete descriptors.constructor;
    Object.defineProperties(ConnectionSettingsModule.prototype, descriptors);
}
