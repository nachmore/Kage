import { errMessage } from '../shared/error-message.js';
import { t } from '../shared/i18n.js';
import { SettingsModule } from './base.js';
import { installAboutBackupMethods } from './about-backup.js';
/**
 * About Settings Module
 * Shows version, author, copyright info, links to welcome screen, and logging section
 */
export class AboutSettingsModule extends SettingsModule {
    constructor() {
        super('about', t('settings.about.title'), 'ℹ️');
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
                            <div class="about-version" id="aboutVersion">${t('settings.about.version_loading')}</div>
                            <div class="about-homepage" id="aboutHomepage"></div>
                        </div>
                    </div>
                    <div class="about-description" id="aboutDescription"></div>
                    <div class="about-info" id="aboutInfo">
                        <div class="about-row"><span class="about-label">${t('settings.about.info.loading')}</span></div>
                    </div>
                    <div class="about-actions">
                        <button class="setting-button" id="showWelcomeBtn">${t('settings.about.action.show_welcome')}</button>
                        <button class="setting-button" id="sendFeedbackBtn" style="margin-left:8px;">${t('settings.about.action.send_feedback')}</button>
                    </div>
                </div>

                <!-- Backup & Restore Section -->
                <div class="about-logging-section">
                    <div class="about-logging-header" id="backupToggle">
                        <span class="about-logging-arrow" id="backupArrow">▶</span>
                        <span>${t('settings.about.backup.title')}</span>
                    </div>
                    <div class="about-logging-body" id="backupBody" style="display:none;">
                        <div class="setting-description" style="margin-bottom:12px;">
                            ${t('settings.about.backup.description')}
                        </div>

                        <div class="setting-row">
                            <div class="setting-label">${t('settings.about.backup.export.label')}</div>
                            <div class="setting-checkbox-row">
                                <label class="kage-checkbox">
                                    <input type="checkbox" id="backupEncryptToggle">
                                </label>
                                <div class="setting-description">${t('settings.about.backup.encrypt_toggle')}</div>
                            </div>
                            <div id="backupPassphraseRow" style="display:none;margin-top:8px;">
                                <input type="password" id="backupPassphrase" class="setting-input" placeholder="${t('settings.about.backup.passphrase.placeholder')}" autocomplete="new-password" style="margin-bottom:6px;">
                                <input type="password" id="backupPassphraseConfirm" class="setting-input" placeholder="${t('settings.about.backup.passphrase.confirm_placeholder')}" autocomplete="new-password">
                                <div class="setting-description" style="margin-top:4px;font-size:11px;">${t('settings.about.backup.passphrase.note')}</div>
                            </div>
                            <div class="setting-control" style="margin-top:8px;">
                                <button class="setting-button" id="backupExportBtn">${t('settings.about.backup.export_btn')}</button>
                            </div>
                        </div>

                        <div class="setting-row">
                            <div class="setting-label">${t('settings.about.backup.import.label')}</div>
                            <div class="setting-description">${t('settings.about.backup.import.description')}</div>
                            <div class="setting-control" style="margin-top:8px;">
                                <button class="setting-button" id="backupImportBtn">${t('settings.about.backup.import_btn')}</button>
                            </div>
                        </div>

                        <div class="setting-row">
                            <div class="setting-label">${t('settings.about.backup.config_folder.label')}</div>
                            <div class="setting-description">${t('settings.about.backup.config_folder.description')}</div>
                            <div class="setting-control" style="margin-top:8px;">
                                <button class="setting-button" id="openConfigFolderBtn">${t('settings.about.action.open_config')}</button>
                            </div>
                        </div>

                        <div id="backupStatus" class="setting-description" style="min-height:1em;margin-top:8px;"></div>
                    </div>
                </div>

                <!-- Logging Section -->
                <div class="about-logging-section">
                    <div class="about-logging-header" id="loggingToggle">
                        <span class="about-logging-arrow" id="loggingArrow">▶</span>
                        <span>${t('settings.about.logging.title')}</span>
                    </div>
                    <div class="about-logging-body" id="loggingBody" style="display:none;">
                        <div class="setting-row">
                            <div class="setting-label">${t('settings.about.logging.buffer_size.label')}</div>
                            <div class="setting-description">${t('settings.about.logging.buffer_size.description')}</div>
                            <div class="setting-control">
                                <input type="number" id="logBufferSize" class="setting-input" min="100" max="50000" step="100" style="width:120px;">
                            </div>
                        </div>
                        <div class="setting-row">
                            <div class="setting-label">${t('settings.about.logging.verbose.label')}</div>
                            <div class="setting-checkbox-row">
                                <label class="kage-checkbox">
                                    <input type="checkbox" id="verboseFrontendLogging">
                                </label>
                                <div class="setting-description">${t('settings.about.logging.verbose.description')}</div>
                            </div>
                        </div>
                        <div class="setting-row">
                            <div class="setting-label">${t('settings.about.logging.message_content.label')}</div>
                            <div class="setting-checkbox-row">
                                <label class="kage-checkbox">
                                    <input type="checkbox" id="logMessageContent">
                                </label>
                                <div class="setting-description">${t('settings.about.logging.message_content.description')}</div>
                            </div>
                        </div>
                        <div class="about-log-toolbar">
                            <select id="logFilterLevel" class="setting-input" style="width:auto;min-width:90px;">
                                <option value="all">${t('settings.about.logging.filter.all_levels')}</option>
                                <option value="debug">${t('settings.about.logging.filter.debug')}</option>
                                <option value="info">${t('settings.about.logging.filter.info')}</option>
                                <option value="warn">${t('settings.about.logging.filter.warn')}</option>
                                <option value="error">${t('settings.about.logging.filter.error')}</option>
                            </select>
                            <select id="logFilterSource" class="setting-input" style="width:auto;min-width:120px;">
                                <option value="all">${t('settings.about.logging.filter.all_sources')}</option>
                            </select>
                            <div style="flex:1;"></div>
                            <button class="setting-button" id="logRefreshBtn">${t('settings.about.logging.refresh')}</button>
                            <button class="setting-button" id="logClearBtn">${t('settings.about.logging.clear')}</button>
                            <button class="setting-button" id="logOpenFolderBtn">${t('settings.about.logging.open_folder')}</button>
                        </div>
                        <div class="about-log-viewer" id="logViewer">
                            <div class="about-log-empty">${t('settings.about.logging.empty')}</div>
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
            document.getElementById('aboutVersion').textContent = t(
                'settings.about.version_format',
                { version: info.version }
            );
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
                if (info.authors)
                    rows.push(this.infoRow(t('settings.about.info.author_label'), info.authors));
                if (info.repository && info.repository !== 'TBD')
                    rows.push(
                        this.infoRow(
                            t('settings.about.info.repository_label'),
                            '<a href="' +
                                info.repository +
                                '" target="_blank">' +
                                info.repository.replace('https://', '') +
                                '</a>'
                        )
                    );
                if (info.license)
                    rows.push(this.infoRow(t('settings.about.info.license_label'), info.license));
                rows.push(
                    this.infoRow(
                        t('settings.about.info.copyright_label'),
                        t('settings.about.info.copyright_value', {
                            authors:
                                info.authors || t('settings.about.info.copyright_default_authors'),
                        })
                    )
                );
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
        viewer.innerHTML = `<div class="about-log-empty">${t('settings.about.logging.loading')}</div>`;
        try {
            this._logEntries = await window.__TAURI__.core.invoke('app_log_get_entries');
            this._populateSourceFilter();
            this._renderLogEntries();
        } catch (_e) {
            viewer.innerHTML = `<div class="about-log-empty">${t('settings.about.logging.load_failed')}</div>`;
        }
    }

    _populateSourceFilter() {
        const select = document.getElementById('logFilterSource');
        if (!select) return;
        const sources = [...new Set(this._logEntries.map((e) => e.source))].sort();
        const current = select.value;
        const allSourcesLabel = t('settings.about.logging.filter.all_sources');
        select.innerHTML =
            `<option value="all">${allSourcesLabel}</option>` +
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
            const filtered = this._filterLevel !== 'all' || this._filterSource !== 'all';
            const emptyText = filtered
                ? t('settings.about.logging.no_entries_filtered')
                : t('settings.about.logging.no_entries');
            viewer.innerHTML = `<div class="about-log-empty">${emptyText}</div>`;
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
        const msgContent = document.getElementById('logMessageContent');
        if (msgContent) msgContent.checked = !!config?.system?.log_message_content;
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
        const msgContent = document.getElementById('logMessageContent');
        if (msgContent) {
            config.system.log_message_content = !!msgContent.checked;
        }
    }

    validate() {
        const input = document.getElementById('logBufferSize');
        if (input) {
            const val = parseInt(input.value, 10);
            if (Number.isNaN(val) || val < 100 || val > 50000) {
                return { valid: false, error: t('settings.about.log_buffer.range_error') };
            }
        }
        return { valid: true };
    }

    destroy() {}

    // --- Backup & restore -------------------------------------------------

    _formatError(e) {
        // Delegate to the shared helper so AppError-shaped errors render
        // consistently across windows. Wrapping it in a method here is
        // legacy; future call sites should import errMessage directly.
        return errMessage(e);
    }
}

installAboutBackupMethods(AboutSettingsModule);
