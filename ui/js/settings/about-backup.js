import { activeLanguage, t } from '../shared/i18n.js';
import { formatBytes } from '../shared/tool-utils.js';
import { getSettingsManager } from './module-registry.js';

/** AppError key: the chosen backup is encrypted and no passphrase was given. */
export const IMPORT_PASSPHRASE_REQUIRED_KEY = 'errors.config.import_passphrase_required';

/** AppError key: the passphrase didn't decrypt the backup (or it's damaged). */
export const IMPORT_WRONG_PASSPHRASE_KEY = 'errors.config.import_wrong_passphrase';

/** The two kinds of backup (`ExportMode` in src/config_export.rs). */
export const MODE_ENCRYPTED = 'encrypted';
export const MODE_SETTINGS_ONLY = 'settings_only';

/**
 * Settings → About → Backup & restore markup. The section's behaviour lives
 * in the methods {@link installAboutBackupMethods} adds to the About module.
 *
 * Export offers exactly two kinds of backup: everything, encrypted with a
 * passphrase (the default), or an unencrypted file of settings only — an
 * allowlist the backend enforces, so no secret can be in it. Before a
 * settings-only file is saved, the section lists in plain words what it
 * leaves out.
 */
export function renderBackupSection() {
    return `
                <div class="about-logging-section">
                    <div class="about-logging-header" id="backupToggle" role="button" tabindex="0" aria-expanded="false" aria-controls="backupBody">
                        <span class="about-logging-arrow" id="backupArrow" aria-hidden="true">▶</span>
                        <span>${t('settings.about.backup.title')}</span>
                    </div>
                    <div class="about-logging-body" id="backupBody" style="display:none;">
                        <div class="setting-description" style="margin-bottom:12px;">
                            ${t('settings.about.backup.description')}
                        </div>
                        <!-- Shown once the build offers sync (settings/sync.js). -->
                        <p class="setting-description" id="backupSyncPointer" data-sync-available-only hidden>
                            ${t('settings.about.backup.sync_pointer')}
                            <button type="button" class="setting-button" data-action="switchSection" data-arg="sync">${t('settings.about.backup.sync_pointer_btn')}</button>
                        </p>

                        <div class="setting-row">
                            <div class="setting-label">${t('settings.about.backup.export.label')}</div>
                            <fieldset id="backupMode" class="backup-mode">
                                <legend class="backup-mode-title">${t('settings.about.backup.mode.title')}</legend>
                                <div class="backup-mode-option">
                                    <input type="radio" name="backupMode" id="backupModeEncrypted" value="${MODE_ENCRYPTED}" checked aria-describedby="backupModeEncryptedDesc">
                                    <div class="backup-mode-option-text">
                                        <label for="backupModeEncrypted">${t('settings.about.backup.mode.encrypted.label')}</label>
                                        <div class="setting-description" id="backupModeEncryptedDesc">${t('settings.about.backup.mode.encrypted.description')}</div>
                                        <div id="backupPassphraseRow" class="backup-passphrase-row">
                                            <input type="password" id="backupPassphrase" class="setting-input" placeholder="${t('settings.about.backup.passphrase.placeholder')}" aria-label="${t('settings.about.backup.passphrase.placeholder')}" aria-describedby="backupPassphraseNote" autocomplete="new-password" style="margin-bottom:6px;">
                                            <input type="password" id="backupPassphraseConfirm" class="setting-input" placeholder="${t('settings.about.backup.passphrase.confirm_placeholder')}" aria-label="${t('settings.about.backup.passphrase.confirm_placeholder')}" autocomplete="new-password">
                                            <div class="setting-description" id="backupPassphraseNote" style="margin-top:4px;font-size:11px;">${t('settings.about.backup.passphrase.note')}</div>
                                        </div>
                                    </div>
                                </div>
                                <div class="backup-mode-option">
                                    <input type="radio" name="backupMode" id="backupModeSettings" value="${MODE_SETTINGS_ONLY}" aria-describedby="backupModeSettingsDesc backupLeftOutGeneric backupLeftOut">
                                    <div class="backup-mode-option-text">
                                        <label for="backupModeSettings">${t('settings.about.backup.mode.settings_only.label')}</label>
                                        <div class="setting-description" id="backupModeSettingsDesc">${t('settings.about.backup.mode.settings_only.description')}</div>
                                        <p class="setting-description backup-left-out-generic" id="backupLeftOutGeneric">${t('settings.about.backup.settings_only.left_out_generic')}</p>
                                        <div class="backup-left-out" id="backupLeftOut" hidden>
                                            <p class="setting-description" id="backupLeftOutIntro">${t('settings.about.backup.settings_only.left_out_intro')}</p>
                                            <ul class="backup-left-out-list" id="backupLeftOutList"></ul>
                                            <p class="setting-description" id="backupFlaggedIntro" hidden></p>
                                            <ul class="backup-left-out-list" id="backupFlaggedList" hidden></ul>
                                            <p class="setting-description" id="backupLeftOutOutro">${t('settings.about.backup.settings_only.left_out_outro')}</p>
                                        </div>
                                    </div>
                                </div>
                            </fieldset>
                            <div class="setting-control backup-export-actions" style="margin-top:8px;">
                                <button class="setting-button" id="backupExportBtn" type="button">${t('settings.about.backup.export_btn')}</button>
                            </div>
                        </div>

                        <div class="setting-row">
                            <div class="setting-label">${t('settings.about.backup.import.label')}</div>
                            <div class="setting-description">${t('settings.about.backup.import.description')}</div>
                            <div class="setting-control" style="margin-top:8px;">
                                <button class="setting-button" id="backupImportBtn" type="button">${t('settings.about.backup.import_btn')}</button>
                            </div>
                        </div>

                        <div class="setting-row">
                            <div class="setting-label">${t('settings.about.backup.config_folder.label')}</div>
                            <div class="setting-description">${t('settings.about.backup.config_folder.description')}</div>
                            <div class="setting-control" style="margin-top:8px;">
                                <button class="setting-button" id="openConfigFolderBtn" type="button">${t('settings.about.action.open_config')}</button>
                            </div>
                        </div>

                        <div id="backupStatus" class="setting-description" role="status" aria-live="polite" style="min-height:1em;margin-top:8px;"></div>
                    </div>
                </div>`;
}

/**
 * Bidi-isolate a value placed inside a localised sentence (FSI … PDI), so a
 * path or a Latin name in an RTL message keeps its punctuation in place.
 */
function isolate(value) {
    return `\u2068${value}\u2069`;
}

/** Join names as a list in the UI language ("A, B and C"). */
function formatList(items) {
    items = items.map(isolate);
    try {
        return new Intl.ListFormat(activeLanguage() || undefined, {
            style: 'long',
            type: 'conjunction',
        }).format(items);
    } catch {
        return items.join(', ');
    }
}

const $ = (id) => document.getElementById(id);

/**
 * How each kind of thing a settings-only backup leaves out is worded (the
 * backend sends `{ kind, count }`; ICU plurals are rendered here).
 */
const LEFT_OUT_LABELS = Object.freeze({
    steering: (count) => t('settings.about.backup.settings_only.left_out.steering', { count }),
    shortcuts: (count) => t('settings.about.backup.settings_only.left_out.shortcuts', { count }),
    automations: (count) =>
        t('settings.about.backup.settings_only.left_out.automations', { count }),
    quick_actions: (count) =>
        t('settings.about.backup.settings_only.left_out.quick_actions', { count }),
    app_modes: (count) => t('settings.about.backup.settings_only.left_out.app_modes', { count }),
    agents: (count) => t('settings.about.backup.settings_only.left_out.agents', { count }),
    internal_sites: (count) =>
        t('settings.about.backup.settings_only.left_out.internal_sites', { count }),
    tool_permissions: (count) =>
        t('settings.about.backup.settings_only.left_out.tool_permissions', { count }),
    stores: (count) => t('settings.about.backup.settings_only.left_out.stores', { count }),
    extension_data: (count) =>
        t('settings.about.backup.settings_only.left_out.extension_data', { count }),
    extension_settings: (count) =>
        t('settings.about.backup.settings_only.left_out.extension_settings', { count }),
});

/** The words for one left-out item; an unknown kind (newer backend) by name. */
export function leftOutLabel(item) {
    if (!Object.hasOwn(LEFT_OUT_LABELS, item?.kind)) return String(item?.kind ?? '');
    return LEFT_OUT_LABELS[item.kind](Number(item.count) || 0);
}

/** Fill a list with text items (labels come from the backend: never markup). */
function fillList(list, labels) {
    if (!list) return;
    list.replaceChildren(
        ...labels.map((label) => {
            const li = document.createElement('li');
            li.dir = 'auto';
            li.textContent = label;
            return li;
        })
    );
}

export function installAboutBackupMethods(AboutSettingsModule) {
    Object.assign(AboutSettingsModule.prototype, {
        _wireBackupSection() {
            const toggle = $('backupToggle');
            if (toggle) {
                toggle.addEventListener('click', () => this._toggleBackup());
                toggle.addEventListener('keydown', (e) => {
                    if (e.key === 'Enter' || e.key === ' ') {
                        e.preventDefault();
                        this._toggleBackup();
                    }
                });
            }
            for (const id of ['backupModeEncrypted', 'backupModeSettings']) {
                $(id)?.addEventListener('change', () => this._onBackupModeChange());
            }
            this._onBackupModeChange();
            const exportBtn = $('backupExportBtn');
            if (exportBtn) exportBtn.addEventListener('click', () => this._runBackupExport());
            const importBtn = $('backupImportBtn');
            if (importBtn) importBtn.addEventListener('click', () => this._runBackupImport());
        },

        _toggleBackup() {
            const body = $('backupBody');
            const arrow = $('backupArrow');
            if (!body || !arrow) return;
            const visible = body.style.display !== 'none';
            body.style.display = visible ? 'none' : '';
            arrow.classList.toggle('expanded', !visible);
            $('backupToggle')?.setAttribute('aria-expanded', String(!visible));
        },

        _setBackupStatus(text, kind) {
            const el = $('backupStatus');
            if (!el) return;
            el.textContent = text || '';
            el.style.color =
                kind === 'error'
                    ? 'var(--kage-error)'
                    : kind === 'success'
                      ? 'var(--kage-accent)'
                      : '';
        },

        _clearPassphrases() {
            const pw = $('backupPassphrase');
            const pw2 = $('backupPassphraseConfirm');
            if (pw) pw.value = '';
            if (pw2) pw2.value = '';
        },

        /** 'encrypted' | 'settings_only' — the export choice. */
        _selectedBackupMode() {
            return $('backupModeSettings')?.checked ? MODE_SETTINGS_ONLY : MODE_ENCRYPTED;
        },

        /**
         * Show the passphrase fields only for an encrypted backup (wiping
         * them when it's not chosen, so a half-typed passphrase doesn't
         * linger in the DOM), and list what a settings-only one leaves out
         * when that's chosen.
         */
        _onBackupModeChange() {
            const settingsOnly = this._selectedBackupMode() === MODE_SETTINGS_ONLY;
            const row = $('backupPassphraseRow');
            if (row) row.hidden = settingsOnly;
            if (settingsOnly) {
                this._clearPassphrases();
                this._leftOutShown = this._showLeftOut();
            } else {
                const box = $('backupLeftOut');
                if (box) box.hidden = true;
                this._leftOutShown = null;
            }
        },

        /**
         * Load and show what a settings-only backup of this machine leaves
         * out. Resolves once it's on screen (or the failure is). Never
         * carries a value: the backend returns labels and counts only.
         */
        async _showLeftOut() {
            const box = $('backupLeftOut');
            const list = $('backupLeftOutList');
            const flaggedIntro = $('backupFlaggedIntro');
            const flaggedList = $('backupFlaggedList');
            let preview;
            try {
                preview = await window.__TAURI__.core.invoke('preview_config_export');
            } catch (e) {
                fillList(list, [
                    t('settings.about.backup.settings_only.list_unavailable', {
                        message: this._formatError(e),
                    }),
                ]);
                if (flaggedIntro) flaggedIntro.hidden = true;
                if (flaggedList) flaggedList.hidden = true;
                if (box) box.hidden = false;
                return;
            }
            // The answer may arrive after the user switched back.
            if (this._selectedBackupMode() !== MODE_SETTINGS_ONLY) return;
            const leftOut = Array.isArray(preview?.left_out) ? preview.left_out : [];
            const flagged = Array.isArray(preview?.flagged) ? preview.flagged : [];
            fillList(list, leftOut.map(leftOutLabel));
            const intro = $('backupLeftOutIntro');
            if (intro) intro.hidden = leftOut.length === 0;
            if (list) list.hidden = leftOut.length === 0;
            if (flaggedIntro) {
                flaggedIntro.hidden = flagged.length === 0;
                flaggedIntro.textContent = t('settings.about.backup.settings_only.flagged_intro', {
                    count: flagged.length,
                });
            }
            if (flaggedList) {
                flaggedList.hidden = flagged.length === 0;
                fillList(flaggedList, flagged);
            }
            if (box) box.hidden = leftOut.length === 0 && flagged.length === 0;
        },

        /**
         * Export flow. An encrypted backup needs a passphrase, typed twice.
         * A settings-only one is saved only once the list of what it leaves
         * out is on screen. There is no unencrypted full backup.
         */
        async _runBackupExport() {
            // One export at a time: a double-click would otherwise open two
            // save dialogs (and run Argon2id twice).
            if (this._backupExportBusy) return;
            this._backupExportBusy = true;
            const exportBtn = $('backupExportBtn');
            exportBtn?.setAttribute('aria-busy', 'true');
            try {
                await this._runBackupExportOnce();
            } finally {
                this._backupExportBusy = false;
                exportBtn?.removeAttribute('aria-busy');
            }
        },

        async _runBackupExportOnce() {
            const mode = this._selectedBackupMode();
            const encrypt = mode === MODE_ENCRYPTED;
            let passphrase = null;
            if (encrypt) {
                const pw = $('backupPassphrase')?.value || '';
                const pw2 = $('backupPassphraseConfirm')?.value || '';
                if (!pw) {
                    this._setBackupStatus(t('settings.about.backup.passphrase_required'), 'error');
                    $('backupPassphrase')?.focus();
                    return;
                }
                // Refuse an all-whitespace passphrase, but send any other
                // one exactly as typed: spaces around it are part of it.
                if (!pw.trim()) {
                    this._setBackupStatus(t('settings.about.backup.passphrase_blank'), 'error');
                    $('backupPassphrase')?.focus();
                    return;
                }
                if (pw !== pw2) {
                    this._setBackupStatus(t('settings.about.backup.passphrase_mismatch'), 'error');
                    $('backupPassphraseConfirm')?.focus();
                    return;
                }
                passphrase = pw;
            } else {
                // What it leaves out is listed before anything is saved.
                await (this._leftOutShown || (this._leftOutShown = this._showLeftOut()));
            }

            const invoke = window.__TAURI__.core.invoke;
            const dialog = window.__TAURI__.dialog;
            let defaultName = encrypt ? 'kage-backup.kage.enc' : 'kage-settings.kage';
            try {
                defaultName = await invoke('export_config_default_filename', {
                    encrypted: encrypt,
                });
            } catch {}

            let target;
            try {
                target = await dialog.save({
                    defaultPath: defaultName,
                    filters: [
                        {
                            name: encrypt
                                ? t('settings.about.backup.filter.encrypted')
                                : t('settings.about.backup.filter.plain'),
                            // Encrypted backups end in .enc, so import
                            // knows to ask for the passphrase up front.
                            extensions: encrypt ? ['enc'] : ['kage'],
                        },
                    ],
                });
            } catch (e) {
                this._setBackupStatus(
                    t('settings.about.backup.save_dialog_cancelled', {
                        message: this._formatError(e),
                    }),
                    'error'
                );
                return;
            }
            if (!target) return; // user cancelled

            this._setBackupStatus(t('settings.about.backup.exporting'));
            try {
                const bytes = await invoke('export_config_bundle', {
                    path: target,
                    mode,
                    passphrase,
                });
                const vars = { size: formatBytes(bytes), path: isolate(target) };
                this._setBackupStatus(
                    encrypt
                        ? t('settings.about.backup.saved', vars)
                        : t('settings.about.backup.saved_settings_only', vars),
                    'success'
                );
                // Clear passphrase fields so the value doesn't persist
                // visibly — Argon2id derived a key once and we don't need
                // it again.
                this._clearPassphrases();
            } catch (e) {
                this._setBackupStatus(
                    t('settings.about.backup.export_failed', { message: this._formatError(e) }),
                    'error'
                );
            }
        },

        /**
         * The import summary line, including what needs signing in again,
         * what kept this machine's settings, and which extensions are off
         * until the user approves them.
         */
        _importSummaryText(summary) {
            const exportedAt = summary.exported_at
                ? t('settings.about.import.summary.exported_at', { date: summary.exported_at })
                : '';
            const list = (v) => (Array.isArray(v) ? v : []);
            const approve = list(summary.needs_approval);
            const keptWhole = list(summary.kept_local);
            // Either kind of import: extensions signed in here that kept
            // this machine's settings whole, then those awaiting approval.
            const withKeptAndApprovals = (sentences) => {
                if (keptWhole.length) {
                    sentences.push(
                        t('settings.about.import.summary.kept_local_settings', {
                            count: keptWhole.length,
                            names: formatList(keptWhole),
                        })
                    );
                }
                if (approve.length) {
                    sentences.push(
                        t('settings.about.import.summary.needs_approval', {
                            count: approve.length,
                            names: formatList(approve),
                        })
                    );
                }
                return sentences.join(' ');
            };
            if (summary.kind === MODE_SETTINGS_ONLY) {
                return withKeptAndApprovals([
                    t('settings.about.import.summary.settings_only', {
                        count: summary.settings || 0,
                        exported_at: exportedAt,
                    }),
                ]);
            }
            const parts = [];
            parts.push(t('settings.about.import.summary.shortcuts', { count: summary.shortcuts }));
            parts.push(
                t('settings.about.import.summary.extensions', { count: summary.extensions })
            );
            if (summary.steering_bytes > 0) {
                parts.push(
                    t('settings.about.import.summary.steering', {
                        size: formatBytes(summary.steering_bytes),
                    })
                );
            }
            const sentences = [
                t('settings.about.import.success', {
                    parts: parts.join(', '),
                    exported_at: exportedAt,
                }),
            ];
            const signIn = Array.isArray(summary.sign_in_again) ? summary.sign_in_again : [];
            const reenter = Array.isArray(summary.reenter) ? summary.reenter : [];
            if (signIn.length) {
                sentences.push(
                    t('settings.about.import.summary.sign_in_again', { names: formatList(signIn) })
                );
            }
            if (reenter.length) {
                sentences.push(
                    t('settings.about.import.summary.reenter', { items: formatList(reenter) })
                );
            }
            if (summary.left_out > 0 && !signIn.length && !reenter.length) {
                sentences.push(
                    t('settings.about.import.summary.kept_local', { count: summary.left_out })
                );
            }
            return withKeptAndApprovals(sentences);
        },

        async _runBackupImport() {
            // One import at a time: a double-click would otherwise open two
            // file dialogs and two overwrite prompts.
            if (this._backupImportBusy) return;
            this._backupImportBusy = true;
            const importBtn = $('backupImportBtn');
            importBtn?.setAttribute('aria-busy', 'true');
            try {
                await this._runBackupImportOnce();
            } finally {
                this._backupImportBusy = false;
                importBtn?.removeAttribute('aria-busy');
            }
        },

        /**
         * Import flow: pick a file, inspect it, show a confirm card with
         * Advanced per-category toggles, then import. A `.enc`-named file
         * asks for the passphrase up front; a file without that
         * extension is inspected without one first, and the backend
         * recognises an encrypted envelope by its magic bytes and
         * surfaces as `kind: "encrypted"` so the UI prompts. A wrong
         * passphrase asks again until it works, another error occurs,
         * or the user cancels.
         */
        async _runBackupImportOnce() {
            const invoke = window.__TAURI__.core.invoke;
            const dialog = window.__TAURI__.dialog;

            let chosen;
            try {
                chosen = await dialog.open({
                    multiple: false,
                    directory: false,
                    filters: [
                        {
                            name: t('settings.about.backup.filter.plain'),
                            extensions: ['kage', 'enc'],
                        },
                        { name: t('settings.about.backup.filter.all'), extensions: ['*'] },
                    ],
                });
            } catch (e) {
                this._setBackupStatus(
                    t('settings.about.backup.open_dialog_cancelled', {
                        message: this._formatError(e),
                    }),
                    'error'
                );
                return;
            }
            if (!chosen || typeof chosen !== 'string') return;

            // A `.enc` name is a strong hint: ask before the first try.
            let passphrase = null;
            if (/\.enc$/i.test(chosen)) {
                passphrase = await this._promptForPassphrase();
                if (passphrase === null) return; // cancelled
            }

            // Peek inside the backup: what kind, and which categories
            // carry content. If encrypted and no passphrase yet, ask
            // once and retry.
            let inspection;
            for (;;) {
                try {
                    inspection = await invoke('inspect_backup', {
                        path: chosen,
                        passphrase,
                    });
                } catch (e) {
                    const wrong = e?.key === IMPORT_WRONG_PASSPHRASE_KEY;
                    if (wrong || e?.key === IMPORT_PASSPHRASE_REQUIRED_KEY) {
                        passphrase = await this._promptForPassphrase({ retry: wrong });
                        if (passphrase === null) return;
                        continue;
                    }
                    this._setBackupStatus(
                        t('settings.about.import.failed', {
                            message: this._formatError(e),
                        }),
                        'error'
                    );
                    return;
                }
                if (
                    inspection?.kind === 'encrypted' &&
                    (!Array.isArray(inspection.categoriesPresent) ||
                        inspection.categoriesPresent.length === 0) &&
                    !passphrase
                ) {
                    passphrase = await this._promptForPassphrase();
                    if (passphrase === null) return;
                    continue;
                }
                break;
            }

            // Show the confirm card. The user may untick categories (all
            // on by default); on cancel, nothing is imported.
            const decision = await this._showImportConfirmCard(inspection);
            if (!decision) return;
            const { skipCategories } = decision;

            this._setBackupStatus(t('settings.about.import.importing'));
            let summary;
            for (;;) {
                try {
                    summary = await invoke('import_config_bundle', {
                        path: chosen,
                        passphrase,
                        skipCategories,
                    });
                    break;
                } catch (e) {
                    const wrong = e?.key === IMPORT_WRONG_PASSPHRASE_KEY;
                    if (!wrong && e?.key !== IMPORT_PASSPHRASE_REQUIRED_KEY) {
                        this._setBackupStatus(
                            t('settings.about.import.failed', {
                                message: this._formatError(e),
                            }),
                            'error'
                        );
                        return;
                    }
                    passphrase = await this._promptForPassphrase({ retry: wrong });
                    if (passphrase === null) {
                        // Cancelled: nothing was imported, nothing to report.
                        this._setBackupStatus('');
                        return;
                    }
                }
            }

            this._setBackupStatus(this._importSummaryText(summary), 'success');

            // The bundle replaced config.json, but every module's form still
            // holds pre-import values; a later Save would write them back
            // over the import. Reload before the restart prompt so this
            // holds even when the user declines the restart.
            try {
                await getSettingsManager()?.load();
            } catch (e) {
                console.warn('[Settings] Reload after import failed:', e);
            }

            try {
                const { ask } = window.__TAURI__.dialog || {};
                if (typeof ask === 'function') {
                    const restart = await ask(t('settings.about.dialog.restart.message'), {
                        title: t('settings.about.dialog.restart.title'),
                        kind: 'info',
                    });
                    if (restart) await invoke('restart_app');
                }
            } catch {
                // Non-fatal — user can restart manually.
            }
        },

        /**
         * Show the Import confirm card: a modal with per-category
         * checkboxes behind a collapsed `<details>` "Advanced" section
         * (everything ticked by default). A category whose id is not in
         * `inspection.categoriesPresent` is rendered disabled with a
         * "not in this backup" note. Resolves `{ skipCategories: [ids] }`
         * on Import, or `null` on Cancel.
         */
        async _showImportConfirmCard(inspection) {
            const invoke = window.__TAURI__.core.invoke;
            let categories = [];
            try {
                categories = (await invoke('get_import_categories')) || [];
            } catch (e) {
                console.warn('[Settings] get_import_categories failed:', e);
            }
            const present = new Set(
                Array.isArray(inspection?.categoriesPresent) ? inspection.categoriesPresent : []
            );
            return new Promise((resolve) => {
                const returnFocus = document.activeElement;
                const overlay = document.createElement('div');
                overlay.className = 'backup-passphrase-overlay';
                overlay.dataset.testid = 'importConfirmCard';
                overlay.innerHTML = `
                <div class="backup-passphrase-box" role="dialog" aria-modal="true" aria-labelledby="importConfirmTitle" aria-describedby="importConfirmDesc" style="min-width:360px;max-width:520px;">
                    <div class="backup-passphrase-title" id="importConfirmTitle"></div>
                    <div class="backup-passphrase-desc" id="importConfirmDesc"></div>
                    <details class="backup-import-advanced" id="importAdvanced">
                        <summary id="importAdvancedSummary"></summary>
                        <p class="setting-description" id="importAdvancedDesc"></p>
                        <ul class="backup-import-category-list" id="importCategoryList"></ul>
                    </details>
                    <div class="backup-passphrase-actions">
                        <button class="setting-button backup-passphrase-cancel" type="button"></button>
                        <button class="setting-button backup-passphrase-ok" type="button"></button>
                    </div>
                </div>
            `;
                overlay.querySelector('#importConfirmTitle').textContent = t(
                    'settings.about.import.confirm.title'
                );
                overlay.querySelector('#importConfirmDesc').textContent = t(
                    'settings.about.dialog.import.message'
                );
                overlay.querySelector('#importAdvancedSummary').textContent = t(
                    'settings.about.import.advanced.title'
                );
                overlay.querySelector('#importAdvancedDesc').textContent = t(
                    'settings.about.import.advanced.description'
                );
                const list = overlay.querySelector('#importCategoryList');
                for (const cat of categories) {
                    const li = document.createElement('li');
                    li.className = 'backup-import-category';
                    li.dataset.categoryId = cat.id;
                    const label = document.createElement('label');
                    const cb = document.createElement('input');
                    cb.type = 'checkbox';
                    cb.checked = true;
                    cb.dataset.categoryId = cat.id;
                    const name = document.createElement('span');
                    name.textContent =
                        (cat.label_key && t(cat.label_key)) || cat.en_label || cat.id;
                    label.appendChild(cb);
                    label.appendChild(document.createTextNode(' '));
                    label.appendChild(name);
                    li.appendChild(label);
                    if (!present.has(cat.id)) {
                        cb.disabled = true;
                        cb.checked = false;
                        li.dataset.disabled = 'true';
                        const note = document.createElement('span');
                        note.className = 'setting-description';
                        note.textContent = ` — ${t('settings.about.import.advanced.not_in_backup')}`;
                        li.appendChild(note);
                    }
                    list.appendChild(li);
                }
                const cancelBtn = overlay.querySelector('.backup-passphrase-cancel');
                const okBtn = overlay.querySelector('.backup-passphrase-ok');
                cancelBtn.textContent = t('settings.about.import.confirm.cancel');
                okBtn.textContent = t('settings.about.import.confirm.ok');
                document.body.appendChild(overlay);
                const onKeydown = (e) => {
                    if (e.key === 'Escape') {
                        e.preventDefault();
                        close(null);
                    }
                };
                const close = (value) => {
                    document.removeEventListener('keydown', onKeydown, true);
                    overlay.remove();
                    if (returnFocus && typeof returnFocus.focus === 'function') returnFocus.focus();
                    resolve(value);
                };
                document.addEventListener('keydown', onKeydown, true);
                cancelBtn.addEventListener('click', () => close(null));
                okBtn.addEventListener('click', () => {
                    const skipCategories = Array.from(
                        list.querySelectorAll('input[type="checkbox"]')
                    )
                        .filter((cb) => !cb.disabled && !cb.checked)
                        .map((cb) => cb.dataset.categoryId);
                    close({ skipCategories });
                });
                setTimeout(() => okBtn.focus(), 0);
            });
        },

        /**
         * Prompt for a passphrase in a small modal dialog. Resolves to the
         * string on Unlock/Enter, or `null` on Cancel/Escape. Focus moves
         * into the dialog, Tab stays inside it, and focus returns to where
         * it was when it closes. With `retry`, the body says the last
         * passphrase didn't work.
         */
        _promptForPassphrase({ retry = false } = {}) {
            return new Promise((resolve) => {
                const returnFocus = document.activeElement;
                const overlay = document.createElement('div');
                overlay.className = 'backup-passphrase-overlay';
                overlay.innerHTML = `
                <div class="backup-passphrase-box" role="dialog" aria-modal="true" aria-labelledby="backupUnlockTitle" aria-describedby="backupUnlockDesc">
                    <div class="backup-passphrase-title" id="backupUnlockTitle"></div>
                    <div class="backup-passphrase-desc" id="backupUnlockDesc"></div>
                    <input type="password" class="setting-input" autocomplete="off" spellcheck="false" aria-labelledby="backupUnlockTitle">
                    <div class="backup-passphrase-actions">
                        <button class="setting-button backup-passphrase-cancel" type="button"></button>
                        <button class="setting-button backup-passphrase-ok" type="button"></button>
                    </div>
                </div>
            `;
                overlay.querySelector('#backupUnlockTitle').textContent = t(
                    'settings.about.backup.unlock.title'
                );
                overlay.querySelector('#backupUnlockDesc').textContent = retry
                    ? t('settings.about.backup.unlock.wrong')
                    : t('settings.about.backup.unlock.description');
                const cancelBtn = overlay.querySelector('.backup-passphrase-cancel');
                const okBtn = overlay.querySelector('.backup-passphrase-ok');
                cancelBtn.textContent = t('settings.about.backup.unlock.cancel');
                okBtn.textContent = t('settings.about.backup.unlock.ok');
                document.body.appendChild(overlay);
                const input = overlay.querySelector('input');
                const order = [input, cancelBtn, okBtn];
                // On the document, not the overlay: a click on the backdrop
                // moves focus to <body>, and Escape / Tab must still work.
                const onKeydown = (e) => {
                    if (e.key === 'Escape') {
                        e.preventDefault();
                        close(null);
                    } else if (e.key === 'Enter' && e.target === input) {
                        e.preventDefault();
                        close(input.value);
                    } else if (e.key === 'Tab') {
                        // Keep focus inside the dialog.
                        const i = order.indexOf(document.activeElement);
                        const next = e.shiftKey
                            ? order[(i <= 0 ? order.length : i) - 1]
                            : order[(i + 1) % order.length];
                        e.preventDefault();
                        next.focus();
                    }
                };
                const close = (value) => {
                    document.removeEventListener('keydown', onKeydown, true);
                    overlay.remove();
                    if (returnFocus && typeof returnFocus.focus === 'function') returnFocus.focus();
                    resolve(value);
                };
                document.addEventListener('keydown', onKeydown, true);
                overlay.addEventListener('mousedown', (e) => {
                    if (e.target === overlay) {
                        e.preventDefault();
                        input.focus();
                    }
                });
                cancelBtn.addEventListener('click', () => close(null));
                okBtn.addEventListener('click', () => close(input.value));
                setTimeout(() => input.focus(), 0);
            });
        },
    });
}
