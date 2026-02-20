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
                    <div class="setting-description">Select the model to use when starting a new session. This applies on app launch and when creating new chats.</div>
                    <div class="setting-control">
                        <select id="defaultModelSelect" class="setting-select">
                            <option value="">Loading models...</option>
                        </select>
                    </div>
                </div>
            </div>
        `;
    }

    async initialize() {
        await this.loadModels();
    }

    async loadModels() {
        try {
            this.models = await this.getInvoke()('get_available_models') || [];
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
        select.innerHTML = this.models.map(m => {
            const selected = m.modelId === defaultModel ? ' selected' : '';
            return `<option value="${m.modelId}"${selected}>${m.name}</option>`;
        }).join('');
    }

    getSelectedModelId() {
        const select = document.getElementById('defaultModelSelect');
        return select ? select.value : (this._loadedDefault || '');
    }

    load(config) {
        this._loadedDefault = config.acp?.assistant?.default_model || '';
        // Refresh models from backend every time the tab is shown
        this.loadModels();
    }

    save(config) {
        if (!config.acp) config.acp = {};
        if (!config.acp.assistant) config.acp.assistant = {};
        const selected = this.getSelectedModelId();
        config.acp.assistant.default_model = selected || null;
    }

    validate() {
        return { valid: true };
    }

    destroy() {}
}
