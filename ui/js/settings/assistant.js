/**
 * Assistant Settings Module
 * Manages Kage-specific settings: session launch, steering documents
 */
class AssistantSettingsModule extends SettingsModule {
    constructor() {
        super('personalization', 'Personalization', '✨');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                <div class="setting-row">
                    <div class="setting-label">Learn my preferences</div>
                    <div class="setting-checkbox-row">
                        <label class="kage-checkbox">
                            <input type="checkbox" id="autoSteeringEnabled">
                        </label>
                        <div class="setting-description">Let Kage learn your preferences from conversations and remember them across sessions. You can view and edit what it learns at any time. This data stays on your machine and is only shared with your chosen agent.</div>
                    </div>
                    <div class="setting-control" style="margin-top: 8px;">
                        <button class="setting-button" id="openAutoSteeringBtn">View Learned Preferences</button>
                    </div>
                </div>

                ${this.createControlWithActionRow(
                    'Custom steering document',
                    'Point Kage to your own instructions file. This is always included alongside learned preferences and is never modified by Kage.',
                    '<input type="text" class="setting-input" id="userSteeringPath" placeholder="">',
                    '<button class="setting-button" id="openUserSteeringBtn">Open</button>'
                )}

                <div class="setting-section-label">Quick Actions</div>

                ${this.createCheckboxRow(
                    'Show quick actions on responses',
                    'Show context-aware action chips after agent responses.',
                    'showResponseActions',
                    true
                )}

                ${this.createCheckboxRow(
                    'Show quick actions on selected text',
                    'When you summon Kage with text selected, show smart action chips (Summarize, Translate, Explain code, etc.) based on the content type.',
                    'quickActionsEnabled',
                    true
                )}

                <div class="setting-row">
                    <div class="setting-label">Translate language</div>
                    <div class="setting-description" id="translateLanguageDesc">Default target language for the Translate action. Leave empty to use the system default.</div>
                    <div class="setting-control">
                        <input type="text" class="setting-input" id="translateLanguage" placeholder="e.g., English, Spanish, Japanese" style="max-width: 250px;">
                    </div>
                </div>

                <div class="setting-row">
                    <div class="setting-label">Custom actions</div>
                    <div class="setting-description">Add your own quick actions. Use <code>{text}</code> in the prompt as a placeholder for the selected text.</div>
                    <div id="customActionsContainer" style="margin-top: 8px;"></div>
                    <button class="setting-button" id="addCustomActionBtn" style="margin-top: 8px;">+ Add Action</button>
                </div>
            </div>
        `;
    }

    load(config) {
        const agentCfg = config.acp?.agent || {};
        const autoSteering = document.getElementById('autoSteeringEnabled');
        const userPath = document.getElementById('userSteeringPath');

        if (autoSteering) autoSteering.checked = agentCfg.auto_steering_enabled || false;
        if (userPath) userPath.value = agentCfg.user_steering_path || '';

        // Quick actions
        const qaEnabled = document.getElementById('quickActionsEnabled');
        const qa = config.quick_actions || { enabled: true, custom_actions: [] };
        if (qaEnabled) qaEnabled.checked = qa.enabled !== false;
        const showResponseActions = document.getElementById('showResponseActions');
        if (showResponseActions) showResponseActions.checked = config.ui?.show_response_actions !== false;
        const translateLang = document.getElementById('translateLanguage');
        if (translateLang) translateLang.value = qa.translate_language || '';
        this._renderCustomActions(qa.custom_actions || []);
    }

    save(config) {
        if (!config.acp) config.acp = {};
        if (!config.acp.agent) config.acp.agent = {};
        config.acp.agent.auto_steering_enabled = document.getElementById('autoSteeringEnabled').checked;
        config.acp.agent.user_steering_path = document.getElementById('userSteeringPath').value.trim() || null;

        // Quick actions
        config.quick_actions = config.quick_actions || {};
        config.quick_actions.enabled = document.getElementById('quickActionsEnabled')?.checked ?? true;
        config.quick_actions.translate_language = document.getElementById('translateLanguage')?.value?.trim() || null;
        config.quick_actions.custom_actions = this._collectCustomActions();
        // Response actions (stored in ui config)
        config.ui = config.ui || {};
        config.ui.show_response_actions = document.getElementById('showResponseActions')?.checked ?? true;
    }

    initialize() {
        // Set OS-appropriate placeholder for user steering path
        const pathInput = document.getElementById('userSteeringPath');
        if (pathInput) {
            const platform = navigator.platform || '';
            if (platform.startsWith('Win')) {
                pathInput.placeholder = 'e.g., C:\\Users\\you\\kage-steering.md';
            } else if (platform.startsWith('Mac')) {
                pathInput.placeholder = 'e.g., /Users/you/kage-steering.md';
            } else {
                pathInput.placeholder = 'e.g., /home/you/kage-steering.md';
            }
        }

        const openBtn = document.getElementById('openAutoSteeringBtn');
        if (openBtn) {
            openBtn.addEventListener('click', async () => {
                try {
                    await window.__TAURI__.core.invoke('open_auto_steering_file');
                } catch (error) {
                    console.error('Failed to open auto steering file:', error);
                    alert('Failed to open steering file: ' + error);
                }
            });
        }

        const openUserBtn = document.getElementById('openUserSteeringBtn');
        if (openUserBtn) {
            openUserBtn.addEventListener('click', async () => {
                const pathInput = document.getElementById('userSteeringPath');
                const path = pathInput?.value?.trim();
                if (!path) {
                    alert('Please enter a file path first.');
                    return;
                }
                try {
                    await window.__TAURI__.core.invoke('open_path', { path });
                } catch (error) {
                    console.error('Failed to open user steering file:', error);
                    alert('Failed to open file: ' + error);
                }
            });
        }

        // Add custom action button
        const addBtn = document.getElementById('addCustomActionBtn');
        if (addBtn) {
            addBtn.addEventListener('click', () => this._addCustomActionRow());
        }

        // Show system default language in translate description
        const translateDesc = document.getElementById('translateLanguageDesc');
        if (translateDesc) {
            try {
                const locale = navigator.language || 'en';
                let langName = 'English';
                if (typeof Intl !== 'undefined' && Intl.DisplayNames) {
                    const display = new Intl.DisplayNames(['en'], { type: 'language' });
                    const name = display.of(locale);
                    if (name) langName = name.charAt(0).toUpperCase() + name.slice(1);
                }
                translateDesc.textContent = `Default target language for the Translate action. Leave empty to use the system default (${langName}).`;
            } catch {}
        }
    }

    _renderCustomActions(actions) {
        const container = document.getElementById('customActionsContainer');
        if (!container) return;
        container.innerHTML = '';
        for (const action of actions) {
            this._addCustomActionRow(action);
        }
    }

    _addCustomActionRow(action = null) {
        const container = document.getElementById('customActionsContainer');
        if (!container) return;

        const row = document.createElement('div');
        row.className = 'custom-action-row';
        row.style.cssText = 'display:flex;gap:8px;align-items:center;margin-bottom:6px;';
        row.innerHTML = `
            <input type="text" class="setting-input ca-icon" placeholder="📝" value="${action?.icon || ''}" style="width:40px;text-align:center;">
            <input type="text" class="setting-input ca-label" placeholder="Label" value="${action?.label || ''}" style="width:100px;">
            <input type="text" class="setting-input ca-prompt" placeholder="Prompt ({text} = selection)" value="${(action?.prompt || '').replace(/"/g, '&quot;')}" style="flex:1;">
            <button class="setting-button ca-remove" style="padding:4px 8px;">✕</button>
        `;
        row.querySelector('.ca-remove').addEventListener('click', () => row.remove());
        container.appendChild(row);
    }

    _collectCustomActions() {
        const container = document.getElementById('customActionsContainer');
        if (!container) return [];
        const actions = [];
        for (const row of container.querySelectorAll('.custom-action-row')) {
            const label = row.querySelector('.ca-label')?.value?.trim();
            const prompt = row.querySelector('.ca-prompt')?.value?.trim();
            if (label && prompt) {
                actions.push({
                    label,
                    icon: row.querySelector('.ca-icon')?.value?.trim() || '⚡',
                    prompt,
                    content_types: [],
                });
            }
        }
        return actions;
    }
}
