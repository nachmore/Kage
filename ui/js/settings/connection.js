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

                <div class="setting-section-label">Session</div>
                
                ${this.createCheckboxRow(
                    'Start agent backend on launch',
                    'Speed up initial responses by pre-launching the ACP backend when Kage starts.',
                    'startSessionOnLaunch',
                    true
                )}

                <div class="setting-section-label">Connection</div>

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

                <div class="setting-section-label">Workspace</div>

                ${this.createControlRow(
                    'Agent workspace folder',
                    'The folder the agent works in. It can read and modify files under this path. Leave empty to use the current directory.',
                    `<input type="text" class="setting-input" id="workingDirectory" placeholder="">`
                )}

                ${this.createControlRow(
                    'Sessions directory',
                    'Where session files are stored. Leave empty to auto-detect based on the ACP server.',
                    `<input type="text" class="setting-input" id="sessionsDirectory" placeholder="Auto-detect">`
                )}
            </div>
        `;
    }

    load(config) {
        // Snapshot connection config for change detection
        this._initialConfig = JSON.stringify({
            mode: config.acp?.mode,
            working_directory: config.acp?.agent?.working_directory || null,
        });
        this._needsRestart = false;

        // Load start-on-launch setting
        const agentCfg = config.acp?.agent || {};
        const startSession = document.getElementById('startSessionOnLaunch');
        if (startSession) startSession.checked = agentCfg.start_session_on_launch !== false;

        const workDir = document.getElementById('workingDirectory');
        if (workDir) workDir.value = agentCfg.working_directory || '';

        const sessDir = document.getElementById('sessionsDirectory');
        if (sessDir) sessDir.value = agentCfg.sessions_directory || '';

        if (config.acp?.mode) {
            const mode = config.acp.mode;
            const modeSelect = document.getElementById('acpMode');
            if (!modeSelect) return;

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

        // Preserve existing agent settings
        const existingAgent = config.acp.agent || {};
        existingAgent.start_session_on_launch =
            document.getElementById('startSessionOnLaunch').checked;
        existingAgent.working_directory =
            document.getElementById('workingDirectory').value.trim() || null;
        existingAgent.sessions_directory =
            document.getElementById('sessionsDirectory').value.trim() || null;

        const mode = document.getElementById('acpMode').value;

        if (mode === 'local') {
            const spawnCommand = document.getElementById('spawnCommand').value.trim();
            config.acp.mode = {
                type: 'local',
                spawn_command: spawnCommand,
            };
        } else {
            config.acp.mode = {
                type: 'remote',
                host: document.getElementById('acpHost').value,
                port: parseInt(document.getElementById('acpPort').value, 10),
                timeout_ms: parseInt(document.getElementById('acpTimeout').value, 10),
            };
        }

        config.acp.agent = existingAgent;

        // Check if connection settings changed — prompt restart
        const currentConfig = JSON.stringify({
            mode: config.acp.mode,
            working_directory: config.acp.agent?.working_directory || null,
        });
        if (this._initialConfig && currentConfig !== this._initialConfig) {
            this._needsRestart = true;
        } else {
            this._needsRestart = false;
        }
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

        // Set OS-appropriate placeholder for working directory
        const workDir = document.getElementById('workingDirectory');
        if (workDir) {
            const platform = navigator.platform || '';
            if (platform.startsWith('Win')) {
                workDir.placeholder = 'e.g., C:\\Projects\\my-app';
            } else {
                workDir.placeholder = 'e.g., /home/you/projects/my-app';
            }
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
