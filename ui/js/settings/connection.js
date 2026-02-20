/**
 * Connection Settings Module
 */
class ConnectionSettingsModule extends SettingsModule {
    constructor() {
        super('connection', 'Agent Connection', '🔌');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                
                ${this.createCheckboxRow(
                    'Start Kiro backend on launch',
                    'Speed up initial responses by pre-launching the Kiro backend on Assistant launch.',
                    'startSessionOnLaunch',
                    true
                )}

                ${this.createControlRow(
                    'Connection Mode',
                    'Choose how to connect to the ACP server.',
                    `
                    <select class="setting-select" id="acpMode">
                        <option value="local">Local (Spawn Process)</option>
                        <option value="remote">Remote (TCP Connection)</option>
                    </select>
                    `
                )}
                
                <div id="localModeSettings" style="display: none;">
                    ${this.createControlRow(
                        'Spawn Command',
                        'Full command to spawn the ACP server (including binary path and arguments).',
                        `<input type="text" class="setting-input" id="spawnCommand" placeholder="e.g., C:\\path\\to\\chat_cli.exe acp">`
                    )}
                </div>
                
                <div id="remoteModeSettings" style="display: none;">
                    ${this.createControlRow(
                        'Host',
                        '',
                        `<input type="text" class="setting-input" id="acpHost" value="127.0.0.1">`
                    )}
                    
                    ${this.createControlRow(
                        'Port',
                        '',
                        `<input type="number" class="setting-input" id="acpPort" value="8765">`
                    )}
                    
                    ${this.createControlRow(
                        'Timeout (ms)',
                        'Connection timeout in milliseconds.',
                        `<input type="number" class="setting-input" id="acpTimeout" value="30000">`
                    )}
                </div>
            </div>
        `;
    }

    load(config) {
        // Load start-on-launch setting
        const assistant = config.acp?.assistant || {};
        const startSession = document.getElementById('startSessionOnLaunch');
        if (startSession) startSession.checked = assistant.start_session_on_launch !== false;

        if (config.acp && config.acp.mode) {
            const mode = config.acp.mode;
            const modeSelect = document.getElementById('acpMode');
            
            if (mode.type === 'local') {
                modeSelect.value = 'local';
                const spawnCmd = document.getElementById('spawnCommand');
                if (spawnCmd) spawnCmd.value = mode.spawn_command || '';
            } else if (mode.type === 'remote') {
                modeSelect.value = 'remote';
                const host = document.getElementById('acpHost');
                const port = document.getElementById('acpPort');
                const timeout = document.getElementById('acpTimeout');
                
                if (host) host.value = mode.host;
                if (port) port.value = mode.port;
                if (timeout) timeout.value = mode.timeout_ms;
            }
            
            this.toggleMode();
        }
    }

    save(config) {
        if (!config.acp) config.acp = {};

        // Preserve existing assistant settings
        const existingAssistant = config.acp.assistant || {};
        existingAssistant.start_session_on_launch = document.getElementById('startSessionOnLaunch').checked;
        
        const mode = document.getElementById('acpMode').value;
        
        if (mode === 'local') {
            const spawnCommand = document.getElementById('spawnCommand').value.trim();
            config.acp.mode = {
                type: 'local',
                spawn_command: spawnCommand
            };
        } else {
            config.acp.mode = {
                type: 'remote',
                host: document.getElementById('acpHost').value,
                port: parseInt(document.getElementById('acpPort').value),
                timeout_ms: parseInt(document.getElementById('acpTimeout').value)
            };
        }

        config.acp.assistant = existingAssistant;
    }

    validate() {
        const mode = document.getElementById('acpMode').value;
        
        if (mode === 'local') {
            const spawnCommand = document.getElementById('spawnCommand').value.trim();
            if (!spawnCommand) {
                return { valid: false, error: 'Spawn command is required for local mode' };
            }
        }
        
        return { valid: true };
    }

    initialize() {
        const modeSelect = document.getElementById('acpMode');
        if (modeSelect) {
            modeSelect.addEventListener('change', () => this.toggleMode());
        }
    }

    toggleMode() {
        const mode = document.getElementById('acpMode').value;
        const localSettings = document.getElementById('localModeSettings');
        const remoteSettings = document.getElementById('remoteModeSettings');
        
        if (mode === 'local') {
            localSettings.style.display = 'block';
            remoteSettings.style.display = 'none';
        } else {
            localSettings.style.display = 'none';
            remoteSettings.style.display = 'block';
        }
    }
}
