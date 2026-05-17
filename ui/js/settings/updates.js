import { SettingsModule } from './base.js';
import { escapeHtml } from '../shared/tool-utils.js';

/**
 * Coerce whatever Tauri threw into a readable string.
 *
 * Tauri serializes Rust errors via the type's Serialize impl. Our
 * AppError serializes as `{ kind, message }` — calling `String(e)` on
 * an object renders `[object Object]`, which is what users were
 * seeing in the "Update check failed" line. Read the message field
 * when present, fall through to a JSON dump otherwise.
 *
 * Exported so unit tests can lock the behavior in (see
 * `ui/tests/shared/updates-format-err.test.js`).
 */
export function formatErr(e) {
    if (e == null) return 'Unknown error';
    if (typeof e === 'string') return e;
    if (e instanceof Error) return e.message || String(e);
    if (typeof e === 'object') {
        if (typeof e.message === 'string' && e.message) return e.message;
        try {
            return JSON.stringify(e);
        } catch {
            return String(e);
        }
    }
    return String(e);
}

/**
 * Updates Settings Module
 * Auto-update configuration and changelog display
 */
export class UpdatesSettingsModule extends SettingsModule {
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
                        <div class="update-status-detail" id="updateStatusDetail">Version ${escapeHtml(window._kageVersion || '...')}</div>
                    </div>
                    <div class="update-status-action" id="updateStatusAction"></div>
                </div>

                <div class="setting-section-label">Preferences</div>

                ${this.createControlRow(
                    'Update Channel',
                    "Which release stream this install follows. <strong>Stable</strong> is the recommended default (curated v-tagged releases). <strong>Beta</strong> is for previewing what's coming next. <strong>Dev</strong> tracks every commit — expect rough edges.",
                    `<select class="setting-input" id="updateChannel">
                        <option value="stable">Stable (recommended)</option>
                        <option value="beta">Beta</option>
                        <option value="dev">Dev (bleeding edge)</option>
                    </select>`
                )}

                ${this.createCheckboxRow(
                    'Automatically Check for Updates',
                    'Periodically check for updates.',
                    'updateAutoCheck',
                    false
                )}

                ${this.createCheckboxRow(
                    'Silent Updates',
                    'Download and install updates automatically when idle (no Launcher activity for 5 minutes).',
                    'updateSilentUpdate',
                    false
                )}

                <div class="setting-section-label">Changelog</div>

                <div class="setting-row">
                    <div class="setting-description">Release notes and version history.</div>
                    <div id="changelogContainer" class="changelog-box">
                        <em>Loading changelog...</em>
                    </div>
                </div>
            </div>
        `;
    }

    async initialize() {
        // Cache current version + valid channel list for display.
        // The channel list comes from Rust (updater::VALID_CHANNELS)
        // so the dropdown stays in sync with the backend allow-list
        // automatically — adding a new channel only requires Rust
        // changes plus the Cargo.toml endpoint, no JS edit.
        try {
            const info = await window.__TAURI__.core.invoke('get_app_info');
            window._kageVersion = info.version;
            this._validChannels =
                Array.isArray(info.update_channels) && info.update_channels.length > 0
                    ? info.update_channels
                    : ['stable', 'beta', 'dev'];
            this._renderChannelOptions();
        } catch (e) {
            console.warn('[Updates] Failed to get app info:', e);
            this._validChannels = ['stable', 'beta', 'dev'];
        }

        this.loadChangelog();

        // Channel changes invalidate any cached "update available"
        // state because a different channel may have a different
        // latest version. Re-check after the settings save completes.
        const channelEl = document.getElementById('updateChannel');
        if (channelEl) {
            channelEl.addEventListener('change', () => {
                this._knownUpdate = null;
                // The outer save-settings flow will persist the new
                // channel; we re-check on the next onShow which fires
                // right after. A manual re-check here would race the
                // save — let the normal lifecycle handle it.
            });
        }
    }

    /** Replace the static stable/beta/dev <option> tags with whatever
     *  the backend reports. Idempotent — safe to call before load(). */
    _renderChannelOptions() {
        const channelEl = document.getElementById('updateChannel');
        if (!channelEl || !Array.isArray(this._validChannels)) return;
        const labels = {
            stable: 'Stable (recommended)',
            beta: 'Beta',
            dev: 'Dev (bleeding edge)',
        };
        const previous = channelEl.value;
        channelEl.innerHTML = this._validChannels
            .map((c) => `<option value="${escapeHtml(c)}">${escapeHtml(labels[c] || c)}</option>`)
            .join('');
        if (previous && this._validChannels.includes(previous)) {
            channelEl.value = previous;
        }
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
            this.showCheckFailed(formatErr(e));
        }
    }

    showChecking() {
        const icon = document.getElementById('updateStatusIcon');
        const title = document.getElementById('updateStatusTitle');
        const detail = document.getElementById('updateStatusDetail');
        const action = document.getElementById('updateStatusAction');
        if (icon) icon.innerHTML = '<div class="update-spinner"></div>';
        if (title) title.textContent = 'Checking for updates...';
        if (detail) detail.textContent = 'Version ' + (window._kageVersion || '...');
        if (action) action.innerHTML = '';
    }

    showUpToDate(version) {
        const icon = document.getElementById('updateStatusIcon');
        const title = document.getElementById('updateStatusTitle');
        const detail = document.getElementById('updateStatusDetail');
        const action = document.getElementById('updateStatusAction');
        if (icon) icon.innerHTML = '<span class="update-check-icon">✓</span>';
        if (title) title.textContent = 'Kage is up to date';
        if (detail) detail.textContent = 'Version ' + escapeHtml(version);
        if (action)
            action.innerHTML =
                '<button class="setting-button" id="recheckBtn">Check again</button>';
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
        if (detail) detail.textContent = 'Current version: ' + (window._kageVersion || '...');
        if (action)
            action.innerHTML =
                '<button class="setting-button update-install-btn" id="installNowBtn">Install Now</button>';
        document
            .getElementById('installNowBtn')
            ?.addEventListener('click', () => this.installUpdate());
    }

    showCheckFailed(error) {
        const icon = document.getElementById('updateStatusIcon');
        const title = document.getElementById('updateStatusTitle');
        const detail = document.getElementById('updateStatusDetail');
        const action = document.getElementById('updateStatusAction');
        if (icon) icon.innerHTML = '<span class="update-error-icon">✕</span>';
        if (title) title.textContent = 'Update check failed';
        if (detail) detail.textContent = error;
        if (action)
            action.innerHTML = '<button class="setting-button" id="retryBtn">Retry</button>';
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
            this.showCheckFailed('Install failed: ' + formatErr(e));
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
        } catch (_e) {
            container.innerHTML = '<em>Failed to load changelog.</em>';
        }
    }

    load(config) {
        const u = config.updates || {};
        const autoCheck = document.getElementById('updateAutoCheck');
        const silentUpdate = document.getElementById('updateSilentUpdate');
        const channel = document.getElementById('updateChannel');
        if (autoCheck) autoCheck.checked = u.auto_check || false;
        if (silentUpdate) silentUpdate.checked = u.silent_update || false;
        // Allow-list sourced from get_app_info (mirrors
        // src/updater.rs::VALID_CHANNELS). Unknown values collapse
        // to stable so a stale config can't orphan the user on a
        // dead channel — the Rust save_config also normalises on the
        // way in for defense in depth.
        const known = this._validChannels || ['stable', 'beta', 'dev'];
        if (channel) channel.value = known.includes(u.channel) ? u.channel : 'stable';

        // Only auto-check when the Updates tab is actually visible
        const section = document.querySelector('[data-section-content="updates"]');
        if (section && !section.classList.contains('hidden')) {
            this.onShow();
        }
    }

    save(config) {
        if (!config.updates) config.updates = {};
        config.updates.auto_check = document.getElementById('updateAutoCheck')?.checked || false;
        config.updates.silent_update =
            document.getElementById('updateSilentUpdate')?.checked || false;
        const channelEl = document.getElementById('updateChannel');
        if (channelEl) config.updates.channel = channelEl.value || 'stable';
    }

    validate() {
        return { valid: true };
    }
    destroy() {}
}
