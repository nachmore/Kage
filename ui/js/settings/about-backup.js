import { t } from '../shared/i18n.js';
import { formatBytes } from '../shared/tool-utils.js';

export function installAboutBackupMethods(AboutSettingsModule) {
    Object.assign(AboutSettingsModule.prototype, {
        _wireBackupSection() {
            const toggle = document.getElementById('backupToggle');
            if (toggle) {
                toggle.addEventListener('click', () => this._toggleBackup());
            }
            const encrypt = document.getElementById('backupEncryptToggle');
            if (encrypt) {
                encrypt.addEventListener('change', () => {
                    const row = document.getElementById('backupPassphraseRow');
                    if (row) row.style.display = encrypt.checked ? '' : 'none';
                    if (!encrypt.checked) {
                        // Wipe the field so a half-typed passphrase doesn't
                        // linger in DOM if the user toggles off.
                        const pw = document.getElementById('backupPassphrase');
                        const pw2 = document.getElementById('backupPassphraseConfirm');
                        if (pw) pw.value = '';
                        if (pw2) pw2.value = '';
                    }
                });
            }
            const exportBtn = document.getElementById('backupExportBtn');
            if (exportBtn) exportBtn.addEventListener('click', () => this._runBackupExport());
            const importBtn = document.getElementById('backupImportBtn');
            if (importBtn) importBtn.addEventListener('click', () => this._runBackupImport());
        },

        _toggleBackup() {
            const body = document.getElementById('backupBody');
            const arrow = document.getElementById('backupArrow');
            if (!body || !arrow) return;
            const visible = body.style.display !== 'none';
            body.style.display = visible ? 'none' : '';
            arrow.classList.toggle('expanded', !visible);
        },

        _setBackupStatus(text, kind) {
            const el = document.getElementById('backupStatus');
            if (!el) return;
            el.textContent = text || '';
            el.style.color =
                kind === 'error'
                    ? 'var(--kage-error)'
                    : kind === 'success'
                      ? 'var(--kage-accent)'
                      : '';
        },

        async _runBackupExport() {
            const encryptEl = document.getElementById('backupEncryptToggle');
            const encrypt = !!encryptEl?.checked;
            let passphrase = null;
            if (encrypt) {
                const pw = document.getElementById('backupPassphrase')?.value || '';
                const pw2 = document.getElementById('backupPassphraseConfirm')?.value || '';
                if (!pw) {
                    this._setBackupStatus(t('settings.about.backup.passphrase_required'), 'error');
                    return;
                }
                if (pw !== pw2) {
                    this._setBackupStatus("Passphrases don't match.", 'error');
                    return;
                }
                passphrase = pw;
            }

            const invoke = window.__TAURI__.core.invoke;
            const dialog = window.__TAURI__.dialog;
            let defaultName = 'kage-backup.kage';
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
                            name: encrypt ? 'Kage encrypted backup' : 'Kage backup',
                            extensions: encrypt ? ['enc', 'kage'] : ['kage'],
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

            this._setBackupStatus('Exporting…');
            try {
                const bytes = await invoke('export_config_bundle', {
                    path: target,
                    passphrase,
                });
                this._setBackupStatus(`✓ Saved ${formatBytes(bytes)} to ${target}`, 'success');
                // Clear passphrase fields so the value doesn't persist
                // visibly — Argon2id derived a key once and we don't need
                // it again.
                const pw = document.getElementById('backupPassphrase');
                const pw2 = document.getElementById('backupPassphraseConfirm');
                if (pw) pw.value = '';
                if (pw2) pw2.value = '';
            } catch (e) {
                this._setBackupStatus('Export failed: ' + this._formatError(e), 'error');
            }
        },

        async _runBackupImport() {
            const invoke = window.__TAURI__.core.invoke;
            const dialog = window.__TAURI__.dialog;

            let chosen;
            try {
                chosen = await dialog.open({
                    multiple: false,
                    directory: false,
                    filters: [
                        { name: 'Kage backup', extensions: ['kage', 'enc'] },
                        { name: 'All files', extensions: ['*'] },
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

            // Detect encryption by extension. `.kage.enc` and `.enc` both
            // route through the encrypted unwrap; `.kage` is plain. The
            // backend also has a runtime check on the magic prefix, so a
            // user who renames their file is still safe.
            const looksEncrypted = /\.enc$/i.test(chosen);
            let passphrase = null;
            if (looksEncrypted) {
                passphrase = await this._promptForPassphrase();
                if (passphrase === null) {
                    // user cancelled the passphrase prompt
                    return;
                }
            }

            // Confirm before clobbering the local config — import
            // *replaces* (after sanitising the device-local fields).
            try {
                const { ask } = window.__TAURI__.dialog || {};
                if (typeof ask === 'function') {
                    const ok = await ask(t('settings.about.dialog.import.message'), {
                        title: t('settings.about.dialog.import.title'),
                        kind: 'warning',
                    });
                    if (!ok) return;
                }
            } catch {}

            this._setBackupStatus(t('settings.about.import.importing'));
            let summary;
            try {
                summary = await invoke('import_config_bundle', {
                    path: chosen,
                    passphrase,
                });
            } catch (e) {
                this._setBackupStatus(
                    t('settings.about.import.failed', { message: this._formatError(e) }),
                    'error'
                );
                return;
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
            const exportedAt = summary.exported_at
                ? t('settings.about.import.summary.exported_at', { date: summary.exported_at })
                : '';
            this._setBackupStatus(
                t('settings.about.import.success', {
                    parts: parts.join(', '),
                    exported_at: exportedAt,
                }),
                'success'
            );

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
         * Prompt for a passphrase via a tiny inline overlay. Returns the
         * string on Enter, or `null` on cancel/Escape. Lives inline (not as
         * its own class) because every other passphrase prompt in the
         * codebase is going through the same UX — keeping it focused here
         * avoids the abstraction and stays auditable.
         */
        _promptForPassphrase() {
            return new Promise((resolve) => {
                const overlay = document.createElement('div');
                overlay.className = 'backup-passphrase-overlay';
                overlay.innerHTML = `
                <div class="backup-passphrase-box">
                    <div class="backup-passphrase-title">Enter passphrase</div>
                    <div class="backup-passphrase-desc">This file is encrypted. Enter the passphrase you used when exporting.</div>
                    <input type="password" class="setting-input" autocomplete="off" spellcheck="false">
                    <div class="backup-passphrase-actions">
                        <button class="setting-button backup-passphrase-cancel" type="button">Cancel</button>
                        <button class="setting-button backup-passphrase-ok" type="button">Unlock</button>
                    </div>
                </div>
            `;
                document.body.appendChild(overlay);
                const input = overlay.querySelector('input');
                const cancel = () => {
                    overlay.remove();
                    resolve(null);
                };
                const ok = () => {
                    const v = input.value;
                    overlay.remove();
                    resolve(v);
                };
                overlay
                    .querySelector('.backup-passphrase-cancel')
                    .addEventListener('click', cancel);
                overlay.querySelector('.backup-passphrase-ok').addEventListener('click', ok);
                input.addEventListener('keydown', (e) => {
                    if (e.key === 'Enter') {
                        e.preventDefault();
                        ok();
                    } else if (e.key === 'Escape') {
                        e.preventDefault();
                        cancel();
                    }
                });
                setTimeout(() => input.focus(), 0);
            });
        },
    });
}
