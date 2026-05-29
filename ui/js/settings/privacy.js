import { SettingsModule } from './base.js';
import { t } from '../shared/i18n.js';
/**
 * Privacy & Analytics settings module.
 *
 * Surfaces the user's telemetry choice, lets them flip it, and exposes
 * a reset button for the anonymous install ID. Matches the welcome-screen
 * disclosure word-for-word on the "what we collect" / "what we never
 * collect" lists — discrepancies between the two is what trips up trust
 * audits. The shared `welcome.privacy.collect.*` / `welcome.privacy.never.*`
 * keys are reused here so the two surfaces can never drift.
 */
export class PrivacySettingsModule extends SettingsModule {
    constructor() {
        super('privacy', t('settings.privacy.title'), '🛡️');
        this._info = null;
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                <div id="telemetryStaleBanner" class="setting-row" style="display:none;">
                    <div style="background:rgba(200,150,0,0.1);border:1px solid rgba(200,150,0,0.35);border-radius:8px;padding:12px;font-size:12px;color:#e0b040;">
                        ${t('settings.privacy.stale_consent_html')}
                    </div>
                </div>

                <div id="telemetryTransportWarning" class="setting-row" style="display:none;">
                    <div style="background:rgba(200,120,0,0.08);border:1px solid rgba(200,120,0,0.3);border-radius:8px;padding:12px;font-size:12px;color:#c89000;">
                        ${t('settings.privacy.no_transport_warning')}
                    </div>
                </div>

                ${this.createCheckboxRow(
                    t('settings.privacy.toggle.label'),
                    t('settings.privacy.toggle.description'),
                    'telemetryEnabled',
                    true
                )}

                <div class="setting-row">
                    <div class="setting-label">${t('welcome.privacy.collect.label')}</div>
                    <ul style="font-size:12px;color:var(--kage-text-muted);line-height:1.8;padding-left:20px;margin:0;">
                        <li>${t('welcome.privacy.collect.install_id')}</li>
                        <li>${t('welcome.privacy.collect.app_meta')}</li>
                        <li>${t('welcome.privacy.collect.feature_usage')}</li>
                    </ul>
                </div>

                <div class="setting-row">
                    <div class="setting-label">${t('welcome.privacy.never.label')}</div>
                    <ul style="font-size:12px;color:var(--kage-text-muted);line-height:1.8;padding-left:20px;margin:0;">
                        <li>${t('welcome.privacy.never.content')}</li>
                        <li>${t('welcome.privacy.never.paths')}</li>
                        <li>${t('welcome.privacy.never.identity')}</li>
                    </ul>
                </div>

                <div class="setting-row">
                    <div class="setting-label">${t('settings.privacy.install_id.label')}</div>
                    <div class="setting-description">${t('settings.privacy.install_id.description')}</div>
                    <div class="setting-control-with-action">
                        <input type="text" class="setting-input" id="telemetryInstallId" readonly style="font-family:monospace;font-size:12px;">
                        <button class="setting-button" id="resetTelemetryIdBtn" style="min-width:80px;">${t('settings.privacy.install_id.reset')}</button>
                    </div>
                </div>

                <div class="setting-row">
                    <div class="setting-label">${t('settings.privacy.policy.label')}</div>
                    <div class="setting-description">${t('settings.privacy.policy.description_html')}</div>
                </div>
            </div>
        `;
    }

    async load(_config) {
        try {
            this._info = await window.__TAURI__.core.invoke('get_telemetry_info');
        } catch (e) {
            console.warn('Failed to load telemetry info:', e);
            this._info = {
                enabled: false,
                install_id: null,
                transport_available: false,
                consent_version: 0,
                current_policy_version: 0,
            };
        }

        const enabled = document.getElementById('telemetryEnabled');
        const idField = document.getElementById('telemetryInstallId');
        const warning = document.getElementById('telemetryTransportWarning');
        const staleBanner = document.getElementById('telemetryStaleBanner');

        if (enabled) enabled.checked = !!this._info.enabled;
        if (idField)
            idField.value = this._info.install_id || t('settings.privacy.install_id.placeholder');
        if (warning) warning.style.display = this._info.transport_available ? 'none' : '';

        // Stale-consent detection: when the bundled privacy policy
        // version exceeds the last version the user agreed to, show a
        // banner. The user keeps control — this doesn't force the
        // toggle off on their behalf, but it does flag that the
        // disclosure has changed since they last looked.
        //
        // current_policy_version = 0 means transport isn't configured
        // in this build; skip the banner in that case because there's
        // nothing to consent to.
        const stale =
            this._info.current_policy_version > 0 &&
            this._info.consent_version > 0 &&
            this._info.consent_version < this._info.current_policy_version;
        if (staleBanner) staleBanner.style.display = stale ? '' : 'none';
    }

    initialize() {
        const enabled = document.getElementById('telemetryEnabled');
        if (enabled) {
            enabled.addEventListener('change', async () => {
                try {
                    await window.__TAURI__.core.invoke('set_telemetry_enabled', {
                        enabled: enabled.checked,
                    });
                    // Refresh the displayed ID — enabling from the off state
                    // generates one on the backend.
                    await this.load({});
                } catch (e) {
                    console.error('Failed to toggle telemetry:', e);
                }
            });
        }

        const resetBtn = document.getElementById('resetTelemetryIdBtn');
        if (resetBtn) {
            resetBtn.addEventListener('click', async () => {
                const ok = confirm(t('settings.privacy.install_id.reset_confirm'));
                if (!ok) return;
                try {
                    await window.__TAURI__.core.invoke('reset_telemetry_install_id');
                    await this.load({});
                } catch (e) {
                    console.error('Failed to reset install ID:', e);
                }
            });
        }

        const policyLink = document.getElementById('openPrivacyPolicy');
        if (policyLink) {
            policyLink.addEventListener('click', async (e) => {
                e.preventDefault();
                try {
                    // Link URL sourced from [package.metadata.links] via
                    // get_app_info. Silent no-op if unconfigured, which
                    // is the right behaviour for forks without a policy.
                    const info = await window.__TAURI__.core.invoke('get_app_info');
                    const url = info?.links?.privacy;
                    if (!url) return;
                    const { open } = window.__TAURI__.shell || {};
                    if (open) await open(url);
                } catch {}
            });
        }
    }

    // Privacy is managed through dedicated commands, not the generic
    // save_config flow. No-op here to satisfy the module contract.
    save(_config) {}
    validate() {
        return { valid: true };
    }
}
