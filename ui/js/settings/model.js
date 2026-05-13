/**
 * Model Settings Module
 * Allows selecting a default model for new sessions.
 */
class ModelSettingsModule extends SettingsModule {
    constructor() {
        super('model', 'Model', '🧠');
        this.models = [];
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                <div class="setting-row">
                    <div class="setting-label">Default Model</div>
                    <div class="setting-description">Select the default model used conversations — you can change your model on the fly in the chat sessions (>sessions) experience.</div>
                    <div class="setting-control">
                        <select id="defaultModelSelect" class="setting-select">
                            <option value="">Loading models...</option>
                        </select>
                    </div>
                </div>
                <div class="setting-row">
                    <div class="setting-label">Auto-Compact Threshold</div>
                    <div class="setting-description">Automatically compact the conversation when context usage reaches this percentage. Set to 0 to disable.</div>
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
                label.textContent = v === 0 ? 'Off' : v + '%';
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
            select.innerHTML = '<option value="">No models available yet</option>';
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
        if (label) label.textContent = threshold === 0 ? 'Off' : threshold + '%';
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
