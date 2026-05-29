import { SettingsModule } from './base.js';
import { t } from '../shared/i18n.js';
/**
 * Model Settings Module
 * Allows selecting a default model for new sessions.
 */
export class ModelSettingsModule extends SettingsModule {
    constructor() {
        super('model', t('settings.model.title'), '🧠');
        this.models = [];
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                <div class="setting-row">
                    <div class="setting-label">${t('settings.model.default.label')}</div>
                    <div class="setting-description">${t('settings.model.default.description')}</div>
                    <div class="setting-control">
                        <select id="defaultModelSelect" class="setting-select">
                            <option value="">${t('settings.model.default.loading')}</option>
                        </select>
                    </div>
                </div>
                <div class="setting-row">
                    <div class="setting-label">${t('settings.model.auto_compact.label')}</div>
                    <div class="setting-description">${t('settings.model.auto_compact.description')}</div>
                    <div class="setting-control" style="display:flex;align-items:center;gap:8px;">
                        <input type="range" id="autoCompactThreshold" min="0" max="100" step="5" class="setting-range" style="flex:1">
                        <span id="autoCompactThresholdValue" style="min-width:36px;text-align:right;font-size:13px;color:#9ca3af;">90%</span>
                    </div>
                </div>
            </div>
        `;
    }

    async initialize() {
        await this.loadModels();
        const slider = document.getElementById('autoCompactThreshold');
        const label = document.getElementById('autoCompactThresholdValue');
        if (slider && label) {
            slider.addEventListener('input', () => {
                const v = parseInt(slider.value, 10);
                label.textContent = v === 0 ? t('settings.model.auto_compact.off') : v + '%';
            });
        }
    }

    async loadModels() {
        try {
            this.models = (await this.getInvoke()('get_available_models')) || [];
        } catch (e) {
            console.log('Could not load models:', e);
            this.models = [];
        }
        this.renderModelList();
    }

    getInvoke() {
        return window.__TAURI__.core.invoke;
    }

    renderModelList() {
        const select = document.getElementById('defaultModelSelect');
        if (!select) return;

        if (this.models.length === 0) {
            select.innerHTML = `<option value="">${t('settings.model.default.empty')}</option>`;
            return;
        }

        const defaultModel = this._loadedDefault || '';
        select.innerHTML = this.models
            .map((m) => {
                const selected = m.modelId === defaultModel ? ' selected' : '';
                return `<option value="${m.modelId}"${selected}>${m.name}</option>`;
            })
            .join('');
    }

    getSelectedModelId() {
        const select = document.getElementById('defaultModelSelect');
        return select ? select.value : this._loadedDefault || '';
    }

    load(config) {
        this._loadedDefault = config.acp?.agent?.default_model || '';
        // Refresh models from backend every time the tab is shown
        this.loadModels();

        const threshold = config.acp?.agent?.auto_compact_threshold ?? 90;
        const slider = document.getElementById('autoCompactThreshold');
        const label = document.getElementById('autoCompactThresholdValue');
        if (slider) slider.value = threshold;
        if (label)
            label.textContent =
                threshold === 0 ? t('settings.model.auto_compact.off') : threshold + '%';
    }

    save(config) {
        if (!config.acp) config.acp = {};
        if (!config.acp.agent) config.acp.agent = {};
        const selected = this.getSelectedModelId();
        config.acp.agent.default_model = selected || null;
        const slider = document.getElementById('autoCompactThreshold');
        config.acp.agent.auto_compact_threshold = slider ? parseInt(slider.value, 10) : 90;
    }

    validate() {
        return { valid: true };
    }

    destroy() {}
}
