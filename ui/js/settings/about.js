/**
 * About Settings Module
 * Shows version, author, copyright info, links to welcome screen, and logging section
 */
class AboutSettingsModule extends SettingsModule {
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
                        await window.__TAURI__.core.invoke('open_path', { path: configBase + sep + 'kage' });
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
                hpEl.innerHTML = '<a href="' + info.homepage + '" target="_blank">' + info.homepage + '</a>';
            }
            const descEl = document.getElementById('aboutDescription');
            if (descEl && info.description) descEl.textContent = info.description;
            const infoEl = document.getElementById('aboutInfo');
            if (infoEl) {
                const rows = [];
                if (info.authors) rows.push(this.infoRow('Author', info.authors));
                if (info.repository && info.repository !== 'TBD') rows.push(this.infoRow('Repository', '<a href="' + info.repository + '" target="_blank">' + info.repository.replace('https://', '') + '</a>'));
                if (info.license) rows.push(this.infoRow('License', info.license));
                rows.push(this.infoRow('Copyright', '© 2025 ' + (info.authors || 'Kage Team')));
                infoEl.innerHTML = rows.join('');
            }
        } catch (e) {
            // Don't use console.log here to avoid recursive logging in settings
        }

        // Logging section toggle
        const toggle = document.getElementById('loggingToggle');
        if (toggle) {
            toggle.addEventListener('click', () => this._toggleLogging());
        }

        // Filter listeners
        const levelFilter = document.getElementById('logFilterLevel');
        if (levelFilter) levelFilter.addEventListener('change', () => { this._filterLevel = levelFilter.value; this._renderLogEntries(); });
        const sourceFilter = document.getElementById('logFilterSource');
        if (sourceFilter) sourceFilter.addEventListener('change', () => { this._filterSource = sourceFilter.value; this._renderLogEntries(); });

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
                } catch (e) {
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
                } catch (e) {
                    // silent
                }
            });
        }

        // Listen for subsection navigation (e.g. >logs command)
        document.addEventListener('settings-subsection', (e) => {
            if (e.detail === 'logging' && !this._logExpanded) {
                this._toggleLogging();
            }
        });
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
        } catch (e) {
            viewer.innerHTML = '<div class="about-log-empty">Failed to load logs</div>';
        }
    }

    _populateSourceFilter() {
        const select = document.getElementById('logFilterSource');
        if (!select) return;
        const sources = [...new Set(this._logEntries.map(e => e.source))].sort();
        const current = select.value;
        select.innerHTML = '<option value="all">All sources</option>' +
            sources.map(s => '<option value="' + s + '"' + (s === current ? ' selected' : '') + '>' + s + '</option>').join('');
    }

    _renderLogEntries() {
        const viewer = document.getElementById('logViewer');
        if (!viewer) return;
        let entries = this._logEntries;
        if (this._filterLevel !== 'all') entries = entries.filter(e => e.level === this._filterLevel);
        if (this._filterSource !== 'all') entries = entries.filter(e => e.source === this._filterSource);
        if (entries.length === 0) {
            viewer.innerHTML = '<div class="about-log-empty">No log entries' + (this._filterLevel !== 'all' || this._filterSource !== 'all' ? ' matching filters' : '') + '</div>';
            return;
        }
        viewer.innerHTML = entries.map(e => {
            const ts = e.ts ? e.ts.replace('T', ' ').replace('Z', '').substring(0, 23) : '';
            const esc = (s) => { const d = document.createElement('div'); d.textContent = s; return d.innerHTML; };
            return '<div class="about-log-line">' +
                '<span class="log-ts">' + ts + '</span> ' +
                '<span class="log-level ' + e.level + '">' + e.level.toUpperCase() + '</span> ' +
                '<span class="log-source">[' + esc(e.source) + ']</span> ' +
                esc(e.msg) +
                '</div>';
        }).join('');
        // Scroll to bottom
        viewer.scrollTop = viewer.scrollHeight;
    }

    infoRow(label, value) {
        return '<div class="about-row"><span class="about-label">' + label + '</span><span>' + value + '</span></div>';
    }

    load(config) {
        const input = document.getElementById('logBufferSize');
        if (input) input.value = config?.system?.log_buffer_size || 1000;
    }

    save(config) {
        const input = document.getElementById('logBufferSize');
        if (input) {
            const val = parseInt(input.value, 10);
            if (!isNaN(val) && val >= 100) {
                if (!config.system) config.system = {};
                config.system.log_buffer_size = val;
            }
        }
    }

    validate() {
        const input = document.getElementById('logBufferSize');
        if (input) {
            const val = parseInt(input.value, 10);
            if (isNaN(val) || val < 100 || val > 50000) {
                return { valid: false, error: 'Log buffer size must be between 100 and 50,000' };
            }
        }
        return { valid: true };
    }

    destroy() {}
}
