import { t } from '../shared/i18n.js';

export function installAssistantAppModeMethods(AssistantSettingsModule) {
    Object.assign(AssistantSettingsModule.prototype, {
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
                        ? t('settings.assistant.app_modes.suggest.skipped_suffix', {
                              count: skipped,
                          })
                        : '';
                this._setAppModesStatus(
                    t('settings.assistant.app_modes.suggest.added', {
                        count: added,
                        skipped: skipNote,
                    }),
                    'success'
                );
            }
        },

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
        },

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
                    t('settings.assistant.app_modes.save.failed', {
                        message: this._formatError(e),
                    }),
                    'error'
                );
            }
        },

        _setAppModesStatus(text, kind) {
            const el = document.getElementById('appModesStatus');
            if (!el) return;
            el.textContent = text || '';
            el.style.color =
                kind === 'error'
                    ? 'var(--kage-error)'
                    : kind === 'success'
                      ? 'var(--kage-accent)'
                      : '';
        },
    });
}
