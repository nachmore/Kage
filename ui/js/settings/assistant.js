/**
 * Assistant Settings Module
 * Manages Kiro Assistant-specific settings: session launch, steering documents
 */
class AssistantSettingsModule extends SettingsModule {
    constructor() {
        super('assistant', 'Assistant', '🤖');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                ${this.createCheckboxRow(
                    'Start default session on launch',
                    'Connect to the ACP backend and create a session immediately when Kiro starts, for faster first interaction.',
                    'startSessionOnLaunch',
                    true
                )}

                <div class="setting-row">
                    <div class="setting-label">Auto-generate steering document</div>
                    <div class="setting-checkbox-row">
                        <label class="kiro-checkbox">
                            <input type="checkbox" id="autoSteeringEnabled">
                        </label>
                        <div class="setting-description">Automatically summarize your conversations to build a personalized steering document that guides the assistant across all sessions.</div>
                    </div>
                    <div class="setting-control" style="margin-top: 8px;">
                        <button class="setting-button" id="openAutoSteeringBtn">Open File</button>
                    </div>
                </div>

                ${this.createControlWithActionRow(
                    'User steering document',
                    'Path to your own steering document. This takes precedence over the auto-generated one and will never be overwritten.',
                    '<input type="text" class="setting-input" id="userSteeringPath" placeholder="">',
                    '<button class="setting-button" id="openUserSteeringBtn">Open</button>'
                )}
            </div>
        `;
    }

    load(config) {
        const assistant = config.acp?.assistant || {};
        const startSession = document.getElementById('startSessionOnLaunch');
        const autoSteering = document.getElementById('autoSteeringEnabled');
        const userPath = document.getElementById('userSteeringPath');

        if (startSession) startSession.checked = assistant.start_session_on_launch !== false;
        if (autoSteering) autoSteering.checked = assistant.auto_steering_enabled || false;
        if (userPath) userPath.value = assistant.user_steering_path || '';
    }

    save(config) {
        if (!config.acp) config.acp = {};
        config.acp.assistant = {
            start_session_on_launch: document.getElementById('startSessionOnLaunch').checked,
            auto_steering_enabled: document.getElementById('autoSteeringEnabled').checked,
            user_steering_path: document.getElementById('userSteeringPath').value.trim() || null
        };
    }

    initialize() {
        // Set OS-appropriate placeholder for user steering path
        const pathInput = document.getElementById('userSteeringPath');
        if (pathInput) {
            const platform = navigator.platform || '';
            if (platform.startsWith('Win')) {
                pathInput.placeholder = 'e.g., C:\\Users\\you\\kiro-steering.md';
            } else if (platform.startsWith('Mac')) {
                pathInput.placeholder = 'e.g., /Users/you/kiro-steering.md';
            } else {
                pathInput.placeholder = 'e.g., /home/you/kiro-steering.md';
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
    }
}
