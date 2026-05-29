import { SettingsModule } from './base.js';
import { escapeHtml } from '../shared/tool-utils.js';
import { t } from '../shared/i18n.js';
import { registerSettingsActions } from './module-registry.js';

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
 * `ui-tests/shared/updates-format-err.test.js`).
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
        super('updates', t('settings.updates.title'), '🔄');
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
                        <div class="update-status-title" id="updateStatusTitle">${t('settings.updates.checking')}</div>
                        <div class="update-status-detail" id="updateStatusDetail">${t('settings.updates.version_label', { version: escapeHtml(window._kageVersion || '...') })}</div>
                    </div>
                    <div class="update-status-action" id="updateStatusAction"></div>
                </div>

                <div class="setting-section-label">${t('settings.updates.preferences.label')}</div>

                ${this.createControlRow(
                    t('settings.updates.channel.label'),
                    t('settings.updates.channel.description_html'),
                    `<select class="setting-input" id="updateChannel">
                        <option value="stable">${t('settings.updates.channel.option.stable')}</option>
                        <option value="beta">${t('settings.updates.channel.option.beta')}</option>
                        <option value="dev">${t('settings.updates.channel.option.dev')}</option>
                    </select>`
                )}

                ${this.createCheckboxRow(
                    t('settings.updates.auto_check.label'),
                    t('settings.updates.auto_check.description'),
                    'updateAutoCheck',
                    false
                )}

                ${this.createCheckboxRow(
                    t('settings.updates.silent.label'),
                    t('settings.updates.silent.description'),
                    'updateSilentUpdate',
                    false
                )}

                <div class="setting-section-label">${t('settings.updates.changelog.label')}</div>

                <div class="setting-row">
                    <div class="setting-description">
                        ${t('settings.updates.changelog.description_html')}
                    </div>
                    <div id="changelogContainer" class="changelog-box">
                        <em>${t('settings.updates.changelog.loading')}</em>
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
            // Build a /releases URL once. Falls back to "" when no
            // repo is configured (third-party fork without a
            // [package.metadata.links] block); the click handler
            // below treats that as a noop so the link still renders
            // but doesn't navigate users to a 404.
            const repo = info?.links?.repository || '';
            this._releasesUrl = repo ? repo.replace(/\/+$/, '') + '/releases' : '';
            this._renderChannelOptions();
        } catch (e) {
            console.warn('[Updates] Failed to get app info:', e);
            this._validChannels = ['stable', 'beta', 'dev'];
            this._releasesUrl = '';
        }

        // Open the releases page in the user's default browser. We
        // route through the shell plugin's open_url command so the
        // link works regardless of webview navigation policy. See
        // src/commands/system.rs::open_url.
        registerSettingsActions({
            'updates.openReleases': () => {
                if (!this._releasesUrl) return;
                window.__TAURI__?.core?.invoke('open_url', { url: this._releasesUrl });
            },
        });

        this.loadChangelog();

        // Channel changes invalidate any cached "update available"
        // state because a different channel may have a different
        // latest version. Re-check after the settings save completes.
        const channelEl = document.getElementById('updateChannel');
        if (channelEl) {
            channelEl.addEventListener('change', async () => {
                this._knownUpdate = null;
                // Persist the new channel immediately so the next
                // check_for_update reads the correct endpoint.
                try {
                    const config = await window.__TAURI__.core.invoke('get_config');
                    if (!config.updates) config.updates = {};
                    config.updates.channel = channelEl.value || 'stable';
                    await window.__TAURI__.core.invoke('save_config', { config });
                } catch (e) {
                    console.warn('[Updates] Failed to persist channel change:', e);
                }
                // Re-check against the new channel.
                this.autoCheck();
            });
        }

        // Cross-section navigation: when something dispatches
        // settings-subsection with detail === 'changelog', scroll
        // the changelog block into view. Used by the post-update
        // banner click ("View changelog →") so the user lands at
        // the relevant content, not at the top of Updates.
        document.addEventListener('settings-subsection', (e) => {
            if (e.detail !== 'changelog') return;
            const el = document.getElementById('changelogContainer');
            if (el) el.scrollIntoView({ behavior: 'smooth', block: 'start' });
        });
    }

    /** Replace the static stable/beta/dev <option> tags with whatever
     *  the backend reports. Idempotent — safe to call before load().
     *
     *  Internal IDs (`stable`, `beta`, `dev`) are unchanged — they
     *  feed into Cargo.toml's [package.metadata.update] endpoint
     *  routing and existing saved config values. The user-facing
     *  labels are friendlier ("Preview" beats "Beta" for non-
     *  developers; "Nightly" sets the right "expect breakage"
     *  expectation that "Dev" left vague). */
    _renderChannelOptions() {
        const channelEl = document.getElementById('updateChannel');
        if (!channelEl || !Array.isArray(this._validChannels)) return;
        const labels = {
            stable: t('settings.updates.channel.option.stable'),
            beta: t('settings.updates.channel.option.beta'),
            dev: t('settings.updates.channel.option.dev'),
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
        if (title) title.textContent = t('settings.updates.checking_progress');
        if (detail)
            detail.textContent = t('settings.updates.version_label', {
                version: window._kageVersion || '...',
            });
        if (action) action.innerHTML = '';
    }

    showUpToDate(version) {
        const icon = document.getElementById('updateStatusIcon');
        const title = document.getElementById('updateStatusTitle');
        const detail = document.getElementById('updateStatusDetail');
        const action = document.getElementById('updateStatusAction');
        if (icon) icon.innerHTML = '<span class="update-check-icon">✓</span>';
        if (title) title.textContent = t('settings.updates.up_to_date');
        if (detail) detail.textContent = t('settings.updates.version_label', { version });
        if (action)
            action.innerHTML = `<button class="setting-button" id="recheckBtn">${t('settings.updates.action.recheck')}</button>`;
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
        if (title) title.textContent = t('settings.updates.update_available_title', { version });
        if (detail)
            detail.textContent = t('settings.updates.current_version_label', {
                version: window._kageVersion || '...',
            });
        if (action)
            action.innerHTML = `<button class="setting-button update-install-btn" id="installNowBtn">${t('settings.updates.action.install_now')}</button>`;
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
        if (title) title.textContent = t('settings.updates.check_failed');
        if (detail) detail.textContent = error;
        if (action)
            action.innerHTML = `<button class="setting-button" id="retryBtn">${t('settings.updates.action.retry')}</button>`;
        document.getElementById('retryBtn')?.addEventListener('click', () => this.autoCheck());
    }

    async installUpdate() {
        const icon = document.getElementById('updateStatusIcon');
        const title = document.getElementById('updateStatusTitle');
        const action = document.getElementById('updateStatusAction');
        if (icon) icon.innerHTML = '<div class="update-spinner"></div>';
        if (title) title.textContent = t('settings.updates.installing');
        if (action) action.innerHTML = '';
        try {
            await window.__TAURI__.core.invoke('download_and_install_update');
        } catch (e) {
            // Backend already returns a user-readable, classified
            // message (signature / network / disk full / permission /
            // …). No prefix here — it would double-stack ("Install
            // failed: Install failed: …") with the historical wrap.
            this.showCheckFailed(formatErr(e));
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
            container.innerHTML = `<em>${t('settings.updates.changelog.load_failed')}</em>`;
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
