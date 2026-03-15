/**
 * Updates Settings Module
 * Auto-update configuration and changelog display
 */
class UpdatesSettingsModule extends SettingsModule {
    constructor() {
        super('updates', 'Updates', '🔄');
        this._knownUpdate = null; // cached available version
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                <div class="update-status-card" id="updateStatusCard">
                    <div class="update-status-icon" id="updateStatusIcon">
                        <div class="update-spinner" id="updateSpinner"></div>
                    </div>
                    <div class="update-status-body">
                        <div class="update-status-title" id="updateStatusTitle">Checking for updates</div>
                        <div class="update-status-detail" id="updateStatusDetail">Version ${escapeHtml(window._kiroVersion || '...')}</div>
                    </div>
                    <div class="update-status-action" id="updateStatusAction"></div>
                </div>

                ${this.createCheckboxRow(
                    'Automatically Check for Updates',
                    'Periodically check for updates.',
                    'updateAutoCheck',
                    false
                )}

                ${this.createCheckboxRow(
                    'Silent Updates',
                    'Download and install updates automatically when idle (no floating window activity for 5 minutes).',
                    'updateSilentUpdate',
                    false
                )}

                <div class="setting-row">
                    <div class="setting-label">Changelog</div>
                    <div class="setting-description">Release notes and version history.</div>
                    <div id="changelogContainer" class="changelog-box">
                        <em>Loading changelog...</em>
                    </div>
                </div>
            </div>
        `;
    }

    async initialize() {
        // Cache current version for display
        try {
            const info = await window.__TAURI__.core.invoke('get_app_info');
            window._kiroVersion = info.version;
        } catch (e) { console.warn('[Updates] Failed to get app info:', e); }

        this.loadChangelog();
    }

    /** Called each time the Updates tab is shown */
    onShow() {
        this.autoCheck();
    }

    async autoCheck() {
        // If we already know about an available update, show it immediately
        if (this._knownUpdate) {
            this.showUpdateAvailable(this._knownUpdate);
            return;
        }
        this.showChecking();
        try {
            const result = await window.__TAURI__.core.invoke('check_for_update');
            if (result.available_version) {
                this._knownUpdate = result.available_version;
                this.showUpdateAvailable(result.available_version);
            } else {
                this.showUpToDate(result.current_version);
            }
        } catch (e) {
            this.showCheckFailed(String(e));
        }
    }

    showChecking() {
        const icon = document.getElementById('updateStatusIcon');
        const title = document.getElementById('updateStatusTitle');
        const detail = document.getElementById('updateStatusDetail');
        const action = document.getElementById('updateStatusAction');
        if (icon) icon.innerHTML = '<div class="update-spinner"></div>';
        if (title) title.textContent = 'Checking for updates...';
        if (detail) detail.textContent = 'Version ' + (window._kiroVersion || '...');
        if (action) action.innerHTML = '';
    }

    showUpToDate(version) {
        const icon = document.getElementById('updateStatusIcon');
        const title = document.getElementById('updateStatusTitle');
        const detail = document.getElementById('updateStatusDetail');
        const action = document.getElementById('updateStatusAction');
        if (icon) icon.innerHTML = '<span class="update-check-icon">✓</span>';
        if (title) title.textContent = 'Kiro Assistant is up to date';
        if (detail) detail.textContent = 'Version ' + escapeHtml(version);
        if (action) action.innerHTML = '<button class="setting-button" id="recheckBtn">Check again</button>';
        document.getElementById('recheckBtn')?.addEventListener('click', () => {
            this._knownUpdate = null;
            this.autoCheck();
        });
    }

    showUpdateAvailable(version) {
        const icon = document.getElementById('updateStatusIcon');
        const title = document.getElementById('updateStatusTitle');
        const detail = document.getElementById('updateStatusDetail');
        const action = document.getElementById('updateStatusAction');
        if (icon) icon.innerHTML = '<span class="update-available-icon">⬆</span>';
        if (title) title.textContent = 'Update available — v' + escapeHtml(version);
        if (detail) detail.textContent = 'Current version: ' + (window._kiroVersion || '...');
        if (action) action.innerHTML = '<button class="setting-button update-install-btn" id="installNowBtn">Install Now</button>';
        document.getElementById('installNowBtn')?.addEventListener('click', () => this.installUpdate());
    }

    showCheckFailed(error) {
        const icon = document.getElementById('updateStatusIcon');
        const title = document.getElementById('updateStatusTitle');
        const detail = document.getElementById('updateStatusDetail');
        const action = document.getElementById('updateStatusAction');
        if (icon) icon.innerHTML = '<span class="update-error-icon">✕</span>';
        if (title) title.textContent = 'Update check failed';
        if (detail) detail.textContent = error;
        if (action) action.innerHTML = '<button class="setting-button" id="retryBtn">Retry</button>';
        document.getElementById('retryBtn')?.addEventListener('click', () => this.autoCheck());
    }

    async installUpdate() {
        const icon = document.getElementById('updateStatusIcon');
        const title = document.getElementById('updateStatusTitle');
        const action = document.getElementById('updateStatusAction');
        if (icon) icon.innerHTML = '<div class="update-spinner"></div>';
        if (title) title.textContent = 'Downloading and installing...';
        if (action) action.innerHTML = '';
        try {
            await window.__TAURI__.core.invoke('download_and_install_update');
        } catch (e) {
            this.showCheckFailed('Install failed: ' + String(e));
        }
    }

    async loadChangelog() {
        const container = document.getElementById('changelogContainer');
        if (!container) return;
        try {
            let markdown = await window.__TAURI__.core.invoke('fetch_changelog');
            markdown = markdown.replace(/</g, '&lt;').replace(/>/g, '&gt;');
            if (window.marked) {
                marked.setOptions({ breaks: true, gfm: true });
                container.innerHTML = marked.parse(markdown);
            } else {
                container.textContent = markdown;
            }
        } catch (e) {
            container.innerHTML = '<em>Failed to load changelog.</em>';
        }
    }

    load(config) {
        const u = config.updates || {};
        const autoCheck = document.getElementById('updateAutoCheck');
        const silentUpdate = document.getElementById('updateSilentUpdate');
        if (autoCheck) autoCheck.checked = u.auto_check || false;
        if (silentUpdate) silentUpdate.checked = u.silent_update || false;

        // Only auto-check when the Updates tab is actually visible
        const section = document.querySelector('[data-section-content="updates"]');
        if (section && !section.classList.contains('hidden')) {
            this.onShow();
        }
    }

    save(config) {
        if (!config.updates) config.updates = {};
        config.updates.auto_check = document.getElementById('updateAutoCheck')?.checked || false;
        config.updates.silent_update = document.getElementById('updateSilentUpdate')?.checked || false;
    }

    validate() { return { valid: true }; }
    destroy() {}
}
