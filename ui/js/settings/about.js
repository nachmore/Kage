import { errMessage } from '../shared/error-message.js';
import { formatBytes } from '../shared/tool-utils.js';
import { SettingsModule } from './base.js';
/**
 * About Settings Module
 * Shows version, author, copyright info, links to welcome screen, and logging section
 */
export class AboutSettingsModule extends SettingsModule {
    constructor() {
        super('about', 'About Kage', 'ℹ️');
        this._logExpanded = false;
        this._logEntries = [];
        this._filterLevel = 'all';
        this._filterSource = 'all';
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                <div class="about-card">
                    <div class="about-logo-row">
                        <div class="about-logo" id="aboutMascot"></div>
                        <div>
                            <div class="about-app-name">Kage</div>
                            <div class="about-version" id="aboutVersion">loading...</div>
                            <div class="about-homepage" id="aboutHomepage"></div>
                        </div>
                    </div>
                    <div class="about-description" id="aboutDescription"></div>
                    <div class="about-info" id="aboutInfo">
                        <div class="about-row"><span class="about-label">Loading...</span></div>
                    </div>
                    <div class="about-actions">
                        <button class="setting-button" id="showWelcomeBtn">Show Welcome Screen</button>
                        <button class="setting-button" id="openConfigFolderBtn" style="margin-left:8px;">Open Config Folder</button>
                        <button class="setting-button" id="sendFeedbackBtn" style="margin-left:8px;">Send Feedback</button>
                    </div>
                </div>

                <!-- Backup & Restore Section -->
                <div class="about-logging-section">
                    <div class="about-logging-header" id="backupToggle">
                        <span class="about-logging-arrow" id="backupArrow">▶</span>
                        <span>💾 Backup &amp; restore</span>
                    </div>
                    <div class="about-logging-body" id="backupBody" style="display:none;">
                        <div class="setting-description" style="margin-bottom:12px;">
                            Move your Kage setup between machines. The backup includes config,
                            saved prompts &amp; shortcuts, custom and learned steering, and extension
                            data. Encryption with a passphrase is optional but recommended if the
                            file will leave your machine.
                        </div>

                        <div class="setting-row">
                            <div class="setting-label">Export</div>
                            <div class="setting-checkbox-row">
                                <label class="kage-checkbox">
                                    <input type="checkbox" id="backupEncryptToggle">
                                </label>
                                <div class="setting-description">Encrypt with a passphrase (AES-256-GCM with Argon2id key derivation).</div>
                            </div>
                            <div id="backupPassphraseRow" style="display:none;margin-top:8px;">
                                <input type="password" id="backupPassphrase" class="setting-input" placeholder="Passphrase" autocomplete="new-password" style="margin-bottom:6px;">
                                <input type="password" id="backupPassphraseConfirm" class="setting-input" placeholder="Confirm passphrase" autocomplete="new-password">
                                <div class="setting-description" style="margin-top:4px;font-size:11px;">Pick something memorable. There's no recovery — lose the passphrase and the file is unreadable.</div>
                            </div>
                            <div class="setting-control" style="margin-top:8px;">
                                <button class="setting-button" id="backupExportBtn">Export…</button>
                            </div>
                        </div>

                        <div class="setting-row">
                            <div class="setting-label">Import</div>
                            <div class="setting-description">Pick a previously exported <code>.kage</code> or <code>.kage.enc</code> file. Imports replace your current config — your machine's window positions, telemetry install ID, and OS startup setting are kept local.</div>
                            <div class="setting-control" style="margin-top:8px;">
                                <button class="setting-button" id="backupImportBtn">Import…</button>
                            </div>
                        </div>

                        <div id="backupStatus" class="setting-description" style="min-height:1em;margin-top:8px;"></div>
                    </div>
                </div>

                <!-- Logging Section -->
                <div class="about-logging-section">
                    <div class="about-logging-header" id="loggingToggle">
                        <span class="about-logging-arrow" id="loggingArrow">▶</span>
                        <span>📋 Logging</span>
                    </div>
                    <div class="about-logging-body" id="loggingBody" style="display:none;">
                        <div class="setting-row">
                            <div class="setting-label">Log Buffer Size</div>
                            <div class="setting-description">Maximum number of log entries to keep (requires save)</div>
                            <div class="setting-control">
                                <input type="number" id="logBufferSize" class="setting-input" min="100" max="50000" step="100" style="width:120px;">
                            </div>
                        </div>
                        <div class="setting-row">
                            <div class="setting-label">Log all messages</div>
                            <div class="setting-checkbox-row">
                                <label class="kage-checkbox">
                                    <input type="checkbox" id="verboseFrontendLogging">
                                </label>
                                <div class="setting-description">Include informational messages, not just warnings and errors. Note: this can make Kage run slower if you have extensions that log a lot of messages.</div>
                            </div>
                        </div>
                        <div class="about-log-toolbar">
                            <select id="logFilterLevel" class="setting-input" style="width:auto;min-width:90px;">
                                <option value="all">All levels</option>
                                <option value="debug">Debug</option>
                                <option value="info">Info</option>
                                <option value="warn">Warn</option>
                                <option value="error">Error</option>
                            </select>
                            <select id="logFilterSource" class="setting-input" style="width:auto;min-width:120px;">
                                <option value="all">All sources</option>
                            </select>
                            <div style="flex:1;"></div>
                            <button class="setting-button" id="logRefreshBtn">Refresh</button>
                            <button class="setting-button" id="logClearBtn">Clear Logs</button>
                            <button class="setting-button" id="logOpenFolderBtn">Open Logs Folder</button>
                        </div>
                        <div class="about-log-viewer" id="logViewer">
                            <div class="about-log-empty">Expand to load log entries...</div>
                        </div>
                    </div>
                </div>
            </div>

            <style>
                .about-logging-section { margin-top: 20px; border: 1px solid var(--kage-border-subtle); border-radius: 4px; overflow: hidden; }
                .about-logging-header { padding: 12px 16px; cursor: pointer; user-select: none; display: flex; align-items: center; gap: 8px; font-size: 14px; font-weight: 500; color: var(--kage-text-bright); background: var(--kage-bg-input); }
                .about-logging-header:hover { background: var(--kage-bg-elevated); }
                .about-logging-arrow { font-size: 10px; transition: transform 0.2s; display: inline-block; }
                .about-logging-arrow.expanded { transform: rotate(90deg); }
                .about-logging-body { padding: 16px; }
                .about-log-toolbar { display: flex; align-items: center; gap: 8px; margin: 12px 0 8px; flex-wrap: wrap; }
                .about-log-viewer { max-height: 400px; overflow-y: auto; border: 1px solid var(--kage-border-subtle); border-radius: 4px; background: var(--kage-bg-input); font-family: 'Courier New', monospace; font-size: 11px; line-height: 1.5; }
                .about-log-empty { padding: 20px; text-align: center; color: var(--kage-text-muted); font-family: inherit; font-size: 12px; }
                .about-log-line { padding: 2px 8px; border-bottom: 1px solid var(--kage-border-subtle, rgba(255,255,255,0.05)); white-space: pre-wrap; word-break: break-all; }
                .about-log-line:last-child { border-bottom: none; }
                .about-log-line .log-ts { color: var(--kage-text-muted); }
                .about-log-line .log-level { font-weight: 600; min-width: 44px; display: inline-block; }
                .about-log-line .log-level.debug { color: #888; }
                .about-log-line .log-level.info { color: #4fc3f7; }
                .about-log-line .log-level.warn { color: #ffb74d; }
                .about-log-line .log-level.error { color: #ef5350; }
                .about-log-line .log-source { color: var(--kage-accent); }
            </style>
        `;
    }

    async initialize() {
        // Render mascot
        const mascotEl = document.getElementById('aboutMascot');
        if (mascotEl) {
            const { createMascot, getMascotThemeSettings } = await import('../shared/mascot.js');
            const { outlineColor, invert } = getMascotThemeSettings();
            const owl = await createMascot({
                size: 72,
                invert,
                outline: { color: outlineColor, radius: 1.5 },
            });
            mascotEl.appendChild(owl);
        }

        const btn = document.getElementById('showWelcomeBtn');
        if (btn) {
            btn.addEventListener('click', async () => {
                try {
                    await window.__TAURI__.core.invoke('open_welcome_window');
                } catch (e) {
                    console.error('Failed to open welcome window:', e);
                }
            });
        }

        const configBtn = document.getElementById('openConfigFolderBtn');
        if (configBtn) {
            configBtn.addEventListener('click', async () => {
                try {
                    const folders = await window.__TAURI__.core.invoke('get_common_folders');
                    const configBase = folders?.config;
                    if (configBase) {
                        const sep = configBase.includes('\\') ? '\\' : '/';
                        await window.__TAURI__.core.invoke('open_path', {
                            path: configBase + sep + 'kage',
                        });
                    }
                } catch (e) {
                    console.error('Failed to open config folder:', e);
                }
            });
        }

        // Load app info
        try {
            const info = await window.__TAURI__.core.invoke('get_app_info');
            document.getElementById('aboutVersion').textContent = 'v' + info.version;
            const hpEl = document.getElementById('aboutHomepage');
            if (hpEl && info.homepage) {
                hpEl.innerHTML =
                    '<a href="' + info.homepage + '" target="_blank">' + info.homepage + '</a>';
            }
            const descEl = document.getElementById('aboutDescription');
            if (descEl && info.description) descEl.textContent = info.description;
            const infoEl = document.getElementById('aboutInfo');
            if (infoEl) {
                const rows = [];
                if (info.authors) rows.push(this.infoRow('Author', info.authors));
                if (info.repository && info.repository !== 'TBD')
                    rows.push(
                        this.infoRow(
                            'Repository',
                            '<a href="' +
                                info.repository +
                                '" target="_blank">' +
                                info.repository.replace('https://', '') +
                                '</a>'
                        )
                    );
                if (info.license) rows.push(this.infoRow('License', info.license));
                rows.push(this.infoRow('Copyright', '© 2025 ' + (info.authors || 'Kage Team')));
                infoEl.innerHTML = rows.join('');
            }
            // Wire Send Feedback once we know the version + issues URL.
            // Done here (rather than in parallel with the other button
            // wiring above) so the URL builder always has the info
            // payload — no race where a fast click would open a
            // bare github.com/issues/new without the env block.
            this._wireFeedbackButton(info);
        } catch (_e) {
            // Don't use console.log here to avoid recursive logging in settings
        }

        // Logging section toggle
        const toggle = document.getElementById('loggingToggle');
        if (toggle) {
            toggle.addEventListener('click', () => this._toggleLogging());
        }

        // Backup & restore wiring
        this._wireBackupSection();

        // Filter listeners
        const levelFilter = document.getElementById('logFilterLevel');
        if (levelFilter)
            levelFilter.addEventListener('change', () => {
                this._filterLevel = levelFilter.value;
                this._renderLogEntries();
            });
        const sourceFilter = document.getElementById('logFilterSource');
        if (sourceFilter)
            sourceFilter.addEventListener('change', () => {
                this._filterSource = sourceFilter.value;
                this._renderLogEntries();
            });

        // Refresh button
        const refreshBtn = document.getElementById('logRefreshBtn');
        if (refreshBtn) {
            refreshBtn.addEventListener('click', () => this._loadLogs());
        }

        // Clear button
        const clearBtn = document.getElementById('logClearBtn');
        if (clearBtn) {
            clearBtn.addEventListener('click', async () => {
                try {
                    await window.__TAURI__.core.invoke('app_log_clear');
                    this._logEntries = [];
                    this._renderLogEntries();
                } catch (_e) {
                    // silent
                }
            });
        }

        // Open logs folder button
        const openBtn = document.getElementById('logOpenFolderBtn');
        if (openBtn) {
            openBtn.addEventListener('click', async () => {
                try {
                    const dir = await window.__TAURI__.core.invoke('app_log_get_dir');
                    if (dir) await window.__TAURI__.core.invoke('open_path', { path: dir });
                } catch (_e) {
                    // silent
                }
            });
        }

        // Listen for subsection navigation (e.g. >logs command, or
        // a deep link from Commands & Prompts → "use full export").
        document.addEventListener('settings-subsection', (e) => {
            if (e.detail === 'logging' && !this._logExpanded) {
                this._toggleLogging();
            } else if (e.detail === 'backup') {
                const body = document.getElementById('backupBody');
                if (body && body.style.display === 'none') this._toggleBackup();
                // Scroll the section into view so it's obvious where the
                // user landed.
                document.getElementById('backupToggle')?.scrollIntoView({
                    behavior: 'smooth',
                    block: 'start',
                });
            }
        });
    }

    /** Build a GitHub issue URL prefilled with environment info using
     *  the `feedback.yml` issue template at
     *  `.github/ISSUE_TEMPLATE/feedback.yml`. The template handles
     *  title, labels, and the structured form; we just supply the
     *  values for the auto-filled fields via query params.
     *
     *  We use the template route (rather than freeform `body=...`)
     *  because it gives us the structured form (dropdowns, sections)
     *  and keeps the labels in one place — the template file. If the
     *  template ever evolves, no code change here is needed. */
    _wireFeedbackButton(info) {
        const btn = document.getElementById('sendFeedbackBtn');
        if (!btn) return;
        const issuesUrl = info?.links?.issues || '';
        if (!issuesUrl) {
            // Repo isn't configured to expose an issues URL; hide the
            // button rather than open something broken.
            btn.style.display = 'none';
            return;
        }
        btn.addEventListener('click', async () => {
            try {
                const url = this._buildFeedbackUrl(info);
                await window.__TAURI__.core.invoke('open_url', { url });
            } catch (e) {
                console.error('Failed to open feedback URL:', e);
            }
        });
    }

    /** Compose the github.com/.../issues/new URL using the feedback
     *  template. Autofills the Environment textarea with version + OS
     *  info so triage knows exactly what the user is running.
     *
     *  GitHub's issue-template form maps query params to fields by
     *  the `id` attribute on each form element. See
     *  `.github/ISSUE_TEMPLATE/feedback.yml` for the canonical IDs. */
    _buildFeedbackUrl(info) {
        const issuesUrl = info?.links?.issues || '';
        const base = issuesUrl.replace(/\/+$/, '');
        const newUrl = base.endsWith('/new') ? base : base + '/new';

        const ua = (typeof navigator !== 'undefined' && navigator.userAgent) || 'unknown';
        // navigator.platform is deprecated but still useful for a
        // human-readable OS hint; userAgent has the long form anyway.
        const platform = (typeof navigator !== 'undefined' && navigator.platform) || 'unknown';
        const version = info?.version || 'unknown';

        const environment = [
            '- Kage version: `' + version + '`',
            '- Platform: `' + platform + '`',
            '- User agent: `' + ua + '`',
            '- How you opened this issue: in-app button',
        ].join('\n');

        // `template=feedback.yml` selects the form; the remaining
        // params target individual fields by their `id`. Fields the
        // template marks as required (e.g. summary) stay empty so the
        // user has to fill them in — that's the whole point.
        const params = new URLSearchParams({
            template: 'feedback.yml',
            environment,
        });
        return newUrl + '?' + params.toString();
    }

    _toggleLogging() {
        this._logExpanded = !this._logExpanded;
        const body = document.getElementById('loggingBody');
        const arrow = document.getElementById('loggingArrow');
        if (body) body.style.display = this._logExpanded ? '' : 'none';
        if (arrow) arrow.classList.toggle('expanded', this._logExpanded);
        if (this._logExpanded) this._loadLogs();
    }

    async _loadLogs() {
        const viewer = document.getElementById('logViewer');
        if (!viewer) return;
        viewer.innerHTML = '<div class="about-log-empty">Loading...</div>';
        try {
            this._logEntries = await window.__TAURI__.core.invoke('app_log_get_entries');
            this._populateSourceFilter();
            this._renderLogEntries();
        } catch (_e) {
            viewer.innerHTML = '<div class="about-log-empty">Failed to load logs</div>';
        }
    }

    _populateSourceFilter() {
        const select = document.getElementById('logFilterSource');
        if (!select) return;
        const sources = [...new Set(this._logEntries.map((e) => e.source))].sort();
        const current = select.value;
        select.innerHTML =
            '<option value="all">All sources</option>' +
            sources
                .map(
                    (s) =>
                        '<option value="' +
                        s +
                        '"' +
                        (s === current ? ' selected' : '') +
                        '>' +
                        s +
                        '</option>'
                )
                .join('');
    }

    _renderLogEntries() {
        const viewer = document.getElementById('logViewer');
        if (!viewer) return;
        let entries = this._logEntries;
        if (this._filterLevel !== 'all')
            entries = entries.filter((e) => e.level === this._filterLevel);
        if (this._filterSource !== 'all')
            entries = entries.filter((e) => e.source === this._filterSource);
        if (entries.length === 0) {
            viewer.innerHTML =
                '<div class="about-log-empty">No log entries' +
                (this._filterLevel !== 'all' || this._filterSource !== 'all'
                    ? ' matching filters'
                    : '') +
                '</div>';
            return;
        }
        viewer.innerHTML = entries
            .map((e) => {
                const ts = e.ts ? e.ts.replace('T', ' ').replace('Z', '').substring(0, 23) : '';
                const esc = (s) => {
                    const d = document.createElement('div');
                    d.textContent = s;
                    return d.innerHTML;
                };
                return (
                    '<div class="about-log-line">' +
                    '<span class="log-ts">' +
                    ts +
                    '</span> ' +
                    '<span class="log-level ' +
                    e.level +
                    '">' +
                    e.level.toUpperCase() +
                    '</span> ' +
                    '<span class="log-source">[' +
                    esc(e.source) +
                    ']</span> ' +
                    esc(e.msg) +
                    '</div>'
                );
            })
            .join('');
        // Scroll to bottom
        viewer.scrollTop = viewer.scrollHeight;
    }

    infoRow(label, value) {
        return (
            '<div class="about-row"><span class="about-label">' +
            label +
            '</span><span>' +
            value +
            '</span></div>'
        );
    }

    load(config) {
        const input = document.getElementById('logBufferSize');
        if (input) input.value = config?.system?.log_buffer_size || 1000;
        const verbose = document.getElementById('verboseFrontendLogging');
        if (verbose) verbose.checked = !!config?.system?.verbose_frontend_logging;
    }

    save(config) {
        const input = document.getElementById('logBufferSize');
        if (!config.system) config.system = {};
        if (input) {
            const val = parseInt(input.value, 10);
            if (!Number.isNaN(val) && val >= 100) {
                config.system.log_buffer_size = val;
            }
        }
        const verbose = document.getElementById('verboseFrontendLogging');
        if (verbose) {
            config.system.verbose_frontend_logging = !!verbose.checked;
        }
    }

    validate() {
        const input = document.getElementById('logBufferSize');
        if (input) {
            const val = parseInt(input.value, 10);
            if (Number.isNaN(val) || val < 100 || val > 50000) {
                return { valid: false, error: 'Log buffer size must be between 100 and 50,000' };
            }
        }
        return { valid: true };
    }

    destroy() {}

    // --- Backup & restore -------------------------------------------------

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
    }

    _toggleBackup() {
        const body = document.getElementById('backupBody');
        const arrow = document.getElementById('backupArrow');
        if (!body || !arrow) return;
        const visible = body.style.display !== 'none';
        body.style.display = visible ? 'none' : '';
        arrow.classList.toggle('expanded', !visible);
    }

    _setBackupStatus(text, kind) {
        const el = document.getElementById('backupStatus');
        if (!el) return;
        el.textContent = text || '';
        el.style.color = kind === 'error' ? '#c44' : kind === 'success' ? 'var(--kage-accent)' : '';
    }

    async _runBackupExport() {
        const encryptEl = document.getElementById('backupEncryptToggle');
        const encrypt = !!encryptEl?.checked;
        let passphrase = null;
        if (encrypt) {
            const pw = document.getElementById('backupPassphrase')?.value || '';
            const pw2 = document.getElementById('backupPassphraseConfirm')?.value || '';
            if (!pw) {
                this._setBackupStatus('Enter a passphrase first.', 'error');
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
            defaultName = await invoke('export_config_default_filename', { encrypted: encrypt });
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
            this._setBackupStatus('Save dialog cancelled: ' + this._formatError(e), 'error');
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
    }

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
            this._setBackupStatus('Open dialog cancelled: ' + this._formatError(e), 'error');
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
                const ok = await ask(
                    'Import will replace your current Kage config (window positions, install ID, and OS startup setting are kept local).\n\nContinue?',
                    { title: 'Import backup', kind: 'warning' }
                );
                if (!ok) return;
            }
        } catch {}

        this._setBackupStatus('Importing…');
        let summary;
        try {
            summary = await invoke('import_config_bundle', {
                path: chosen,
                passphrase,
            });
        } catch (e) {
            this._setBackupStatus('Import failed: ' + this._formatError(e), 'error');
            return;
        }

        const parts = [];
        parts.push(`Restored ${summary.shortcuts} shortcut${summary.shortcuts === 1 ? '' : 's'}`);
        parts.push(
            `${summary.extensions} extension data file${summary.extensions === 1 ? '' : 's'}`
        );
        if (summary.steering_bytes > 0) {
            parts.push(`${formatBytes(summary.steering_bytes)} of steering`);
        }
        const exportedAt = summary.exported_at ? ` (exported ${summary.exported_at})` : '';
        this._setBackupStatus(`✓ ${parts.join(', ')}${exportedAt}.`, 'success');

        try {
            const { ask } = window.__TAURI__.dialog || {};
            if (typeof ask === 'function') {
                const restart = await ask(
                    'Some changes only take effect on restart (agent connection, hotkeys, theme). Restart Kage now?',
                    { title: 'Restart required', kind: 'info' }
                );
                if (restart) await invoke('restart_app');
            }
        } catch {
            // Non-fatal — user can restart manually.
        }
    }

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
            overlay.querySelector('.backup-passphrase-cancel').addEventListener('click', cancel);
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
    }

    _formatError(e) {
        // Delegate to the shared helper so AppError-shaped errors render
        // consistently across windows. Wrapping it in a method here is
        // legacy; future call sites should import errMessage directly.
        return errMessage(e);
    }
}
