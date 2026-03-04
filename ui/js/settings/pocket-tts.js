/**
 * Pocket TTS Settings Module
 * Manages kyutai-labs/pocket-tts as a local neural TTS engine.
 * Guides the user through installation, voice selection, and server management.
 */
class PocketTtsSettingsModule extends SettingsModule {
    constructor() {
        super('pocket-tts', 'Pocket TTS', '🔊');
        this._status = null;
        this._pollTimer = null;
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                <p style="font-size:12px;color:var(--kiro-text-muted);margin:0 0 16px;line-height:1.5;">
                    High-quality local text-to-speech powered by
                    <a href="https://github.com/kyutai-labs/pocket-tts" target="_blank">Pocket TTS</a>
                    from Kyutai Labs. Runs on CPU, ~100M parameters, low latency.
                </p>

                <!-- Status banner -->
                <div id="pocketTtsStatusBanner" class="setting-row" style="padding:10px 12px;border-radius:8px;background:var(--kiro-bg-secondary);margin-bottom:12px;">
                    <span id="pocketTtsStatusText" style="font-size:12px;">Checking status...</span>
                </div>

                <!-- Setup wizard steps -->
                <div id="pocketTtsSetup">
                    <!-- Step 1: Python check -->
                    <div id="pocketTtsStep1" class="setting-row" style="display:none;">
                        <div class="setting-label">Step 1: Python</div>
                        <div class="setting-description">
                            Pocket TTS requires Python 3.10 or later.
                        </div>
                        <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                            <span id="pocketTtsPythonStatus" style="font-size:12px;">Checking...</span>
                        </div>
                    </div>

                    <!-- Step 2: Install pocket-tts -->
                    <div id="pocketTtsStep2" class="setting-row" style="display:none;">
                        <div class="setting-label">Step 2: Install pocket-tts</div>
                        <div class="setting-description">
                            Install the pocket-tts Python package via pip. This downloads the model (~400MB) on first use.
                        </div>
                        <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                            <button class="setting-button" id="pocketTtsInstallBtn" onclick="pocketTtsInstall()">
                                Install pocket-tts
                            </button>
                            <span id="pocketTtsInstallStatus" style="font-size:12px;"></span>
                        </div>
                        <pre id="pocketTtsInstallLog" style="display:none;font-size:11px;max-height:150px;overflow-y:auto;background:var(--kiro-bg-tertiary);padding:8px;border-radius:6px;margin-top:8px;white-space:pre-wrap;word-break:break-all;"></pre>
                    </div>

                    <!-- Step 3: Server control -->
                    <div id="pocketTtsStep3" class="setting-row" style="display:none;">
                        <div class="setting-label">Step 3: Start Server</div>
                        <div class="setting-description">
                            The TTS server runs locally and keeps the model in memory for fast generation.
                            First start takes ~10-30s to load the model.
                        </div>
                        <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                            <button class="setting-button" id="pocketTtsStartBtn" onclick="pocketTtsToggleServer()">
                                Start Server
                            </button>
                            <span id="pocketTtsServerStatus" style="font-size:12px;"></span>
                        </div>
                    </div>
                </div>

                <!-- Configuration (shown when installed) -->
                <div id="pocketTtsConfig" style="display:none;">
                    ${this.createCheckboxRow(
                        'Enable Pocket TTS',
                        'Use Pocket TTS instead of the browser\'s built-in speech synthesis for reading back responses.',
                        'pocketTtsEnabled',
                        false
                    )}

                    ${this.createCheckboxRow(
                        'Auto-Start Server',
                        'Automatically start the Pocket TTS server when Kiro launches.',
                        'pocketTtsAutoStart',
                        false
                    )}

                    ${this.createControlRow(
                        'Voice',
                        'Select the voice for speech generation. Built-in voices are English. You can add custom voices by placing .wav files in the voices directory.',
                        '<select class="setting-select" id="pocketTtsVoice"></select>'
                    )}

                    ${this.createControlRow(
                        'Server Port',
                        'Local port for the TTS server (change only if 9877 conflicts with another service).',
                        '<input type="number" class="setting-input" id="pocketTtsPort" min="1024" max="65535" value="9877" style="width:100px;">'
                    )}

                    ${this.createControlRow(
                        'Temperature',
                        'Controls voice expressiveness. Lower = more consistent and robotic, higher = more varied and expressive.',
                        '<div class="range-container"><input type="range" class="range-slider" id="pocketTtsTemp" min="0.3" max="1.0" step="0.1" value="0.7"><span class="range-value" id="pocketTtsTempValue">0.7</span></div>'
                    )}

                    ${this.createControlRow(
                        'EOS Threshold',
                        'End-of-sequence detection sensitivity. Lower values make the model less likely to stop speaking early. Default: -4.0.',
                        '<div class="range-container"><input type="range" class="range-slider" id="pocketTtsEos" min="-8.0" max="-1.0" step="0.5" value="-4.0"><span class="range-value" id="pocketTtsEosValue">-4.0</span></div>'
                    )}

                    <!-- Test -->
                    <div class="setting-row">
                        <div class="setting-label">Test</div>
                        <div class="setting-description">Generate a short sample to verify everything works.</div>
                        <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                            <button class="setting-button" id="pocketTtsTestBtn" onclick="pocketTtsTest()">
                                🔊 Test Voice
                            </button>
                            <span id="pocketTtsTestSpinner" style="display:none;font-size:12px;">⏳ Generating...</span>
                            <span id="pocketTtsTestStatus" style="font-size:12px;"></span>
                        </div>
                    </div>

                    <!-- HF Voice URLs -->
                    <div class="setting-row">
                        <div class="setting-label">Add Voice from URL</div>
                        <div class="setting-description">
                            Paste a HuggingFace voice URL (e.g. <code>hf://kyutai/tts-voices/alba-mackenna/casual.wav</code>) or any HTTP URL to a .wav file.
                            The voice will be downloaded, cached as safetensors for fast loading, and added to the voice list.
                            <a href="https://huggingface.co/kyutai/tts-voices" target="_blank">Browse voices</a>
                        </div>
                        <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                            <input type="text" class="setting-input" id="pocketTtsVoiceUrl" placeholder="hf://kyutai/tts-voices/..." style="flex:1;">
                            <input type="text" class="setting-input" id="pocketTtsVoiceName" placeholder="Name" style="width:100px;">
                            <button class="setting-button" id="pocketTtsAddVoiceBtn" onclick="pocketTtsAddVoice()">Add</button>
                        </div>
                        <div id="pocketTtsAddVoiceStatus" style="font-size:12px;margin-top:4px;"></div>
                    </div>

                    <!-- Voice management -->
                    <div class="setting-row">
                        <div class="setting-label">Custom Voices</div>
                        <div class="setting-description">
                            Place <code>.wav</code> files in the voices directory for voice cloning.
                            Short, clean audio samples (5-20s) work best.
                            <a href="https://huggingface.co/kyutai/tts-voices" target="_blank">Browse community voices</a>
                        </div>
                        <div class="setting-control">
                            <button class="setting-button" onclick="pocketTtsOpenVoicesDir()">Open Voices Folder</button>
                        </div>
                    </div>
                </div>
            </div>
        `;
    }

    load(config) {
        const ptts = config.pocket_tts || {};
        const enabled = document.getElementById('pocketTtsEnabled');
        const autoStart = document.getElementById('pocketTtsAutoStart');
        const voice = document.getElementById('pocketTtsVoice');
        const port = document.getElementById('pocketTtsPort');
        const temp = document.getElementById('pocketTtsTemp');
        const tempValue = document.getElementById('pocketTtsTempValue');
        const eos = document.getElementById('pocketTtsEos');
        const eosValue = document.getElementById('pocketTtsEosValue');

        if (enabled) enabled.checked = ptts.enabled === true;
        if (autoStart) autoStart.checked = ptts.auto_start === true;
        if (port) port.value = ptts.port || 9877;
        if (temp) {
            temp.value = ptts.temp ?? 0.7;
            if (tempValue) tempValue.textContent = (ptts.temp ?? 0.7).toFixed(1);
        }
        if (eos) {
            eos.value = ptts.eos_threshold ?? -4.0;
            if (eosValue) eosValue.textContent = (ptts.eos_threshold ?? -4.0).toFixed(1);
        }
        this._savedVoice = ptts.voice || 'alba';

        // Refresh status and populate voices
        this.refreshStatus();
    }

    save(config) {
        config.pocket_tts = config.pocket_tts || {};
        config.pocket_tts.enabled = document.getElementById('pocketTtsEnabled')?.checked ?? false;
        config.pocket_tts.auto_start = document.getElementById('pocketTtsAutoStart')?.checked ?? false;
        config.pocket_tts.voice = document.getElementById('pocketTtsVoice')?.value || 'alba';
        config.pocket_tts.port = parseInt(document.getElementById('pocketTtsPort')?.value || '9877', 10);
        config.pocket_tts.temp = parseFloat(document.getElementById('pocketTtsTemp')?.value || '0.7');
        config.pocket_tts.eos_threshold = parseFloat(document.getElementById('pocketTtsEos')?.value || '-4.0');
        // Preserve python_path and installed from existing config
        if (this._status) {
            config.pocket_tts.python_path = this._status.python_path || null;
            config.pocket_tts.installed = this._status.installed || false;
        }
    }

    validate() {
        const port = parseInt(document.getElementById('pocketTtsPort')?.value || '9877', 10);
        if (port < 1024 || port > 65535) {
            return { valid: false, error: 'Pocket TTS port must be between 1024 and 65535.' };
        }
        return { valid: true };
    }

    initialize() {
        this.refreshStatus();
        // Wire up range slider labels
        const temp = document.getElementById('pocketTtsTemp');
        const tempValue = document.getElementById('pocketTtsTempValue');
        if (temp && tempValue) {
            temp.addEventListener('input', () => { tempValue.textContent = parseFloat(temp.value).toFixed(1); });
        }
        const eos = document.getElementById('pocketTtsEos');
        const eosValue = document.getElementById('pocketTtsEosValue');
        if (eos && eosValue) {
            eos.addEventListener('input', () => { eosValue.textContent = parseFloat(eos.value).toFixed(1); });
        }
    }

    destroy() {
        if (this._pollTimer) {
            clearInterval(this._pollTimer);
            this._pollTimer = null;
        }
    }

    async refreshStatus() {
        const invoke = window.__TAURI__.core.invoke;
        try {
            this._status = await invoke('pocket_tts_status');
            this.updateUI();
            this.populateVoices();
        } catch (e) {
            console.warn('[PocketTTS] Status check failed:', e);
            this.setStatusBanner('⚠️ Could not check Pocket TTS status', 'warning');
        }
    }

    updateUI() {
        const s = this._status;
        if (!s) return;

        const step1 = document.getElementById('pocketTtsStep1');
        const step2 = document.getElementById('pocketTtsStep2');
        const step3 = document.getElementById('pocketTtsStep3');
        const configSection = document.getElementById('pocketTtsConfig');
        const pythonStatus = document.getElementById('pocketTtsPythonStatus');
        const installStatus = document.getElementById('pocketTtsInstallStatus');
        const serverStatus = document.getElementById('pocketTtsServerStatus');
        const startBtn = document.getElementById('pocketTtsStartBtn');

        // Always show step 1
        if (step1) step1.style.display = '';

        if (!s.python_found) {
            // Python not found
            if (pythonStatus) {
                pythonStatus.innerHTML = '❌ Python 3 not found. <a href="https://www.python.org/downloads/" target="_blank">Download Python</a>';
            }
            this.setStatusBanner('❌ Python 3.10+ required — install Python first', 'error');
            if (step2) step2.style.display = 'none';
            if (step3) step3.style.display = 'none';
            if (configSection) configSection.style.display = 'none';
            return;
        }

        if (pythonStatus) {
            pythonStatus.textContent = `✅ ${s.python_path || 'python3'} found`;
        }

        // Show step 2
        if (step2) step2.style.display = '';

        if (!s.installed) {
            if (installStatus) installStatus.textContent = 'Not installed';
            this.setStatusBanner('📦 pocket-tts not installed — click Install below', 'info');
            if (step3) step3.style.display = 'none';
            if (configSection) configSection.style.display = 'none';
            return;
        }

        if (installStatus) installStatus.textContent = '✅ Installed';
        const installBtn = document.getElementById('pocketTtsInstallBtn');
        if (installBtn) {
            installBtn.textContent = 'Reinstall';
            installBtn.style.opacity = '0.7';
        }

        // Show step 3 and config
        if (step3) step3.style.display = '';
        if (configSection) configSection.style.display = '';

        if (s.server_running) {
            if (serverStatus) serverStatus.textContent = '✅ Running on port ' + s.port;
            if (startBtn) {
                startBtn.textContent = 'Stop Server';
                startBtn.style.background = '#c44';
            }
            this.setStatusBanner('✅ Pocket TTS is ready', 'success');
        } else {
            if (serverStatus) serverStatus.textContent = '⏹ Stopped';
            if (startBtn) {
                startBtn.textContent = 'Start Server';
                startBtn.style.background = '';
            }
            this.setStatusBanner('⏹ Server not running — start it to use Pocket TTS', 'info');
        }
    }

    setStatusBanner(text, type) {
        const banner = document.getElementById('pocketTtsStatusBanner');
        const textEl = document.getElementById('pocketTtsStatusText');
        if (!banner || !textEl) return;

        textEl.innerHTML = text;
        const colors = {
            success: 'rgba(76, 175, 80, 0.15)',
            error: 'rgba(244, 67, 54, 0.15)',
            warning: 'rgba(255, 152, 0, 0.15)',
            info: 'rgba(33, 150, 243, 0.15)',
        };
        banner.style.background = colors[type] || colors.info;
    }

    async populateVoices() {
        const invoke = window.__TAURI__.core.invoke;
        const select = document.getElementById('pocketTtsVoice');
        if (!select) return;

        try {
            const result = await invoke('pocket_tts_voices');
            const voices = result.voices || [];
            select.innerHTML = '';
            for (const v of voices) {
                const opt = document.createElement('option');
                opt.value = v.name;
                const label = v.type === 'custom' ? `${v.name} (custom)` : v.name;
                const loaded = v.loaded ? ' ✓' : '';
                opt.textContent = label + loaded;
                select.appendChild(opt);
            }
            if (this._savedVoice) select.value = this._savedVoice;
        } catch (e) {
            console.warn('[PocketTTS] Failed to load voices:', e);
        }
    }
}

// Global functions called from onclick handlers
let _pocketTtsInstallUnlisteners = [];

async function pocketTtsInstall() {
    const invoke = window.__TAURI__.core.invoke;
    const listen = window.__TAURI__.event.listen;
    const btn = document.getElementById('pocketTtsInstallBtn');
    const status = document.getElementById('pocketTtsInstallStatus');
    const log = document.getElementById('pocketTtsInstallLog');

    // Switch button to Cancel
    if (btn) {
        btn.textContent = '✕ Cancel';
        btn.style.background = '#c44';
        btn.style.color = 'white';
        btn.onclick = pocketTtsCancelInstall;
    }
    if (status) status.textContent = 'Installing...';
    if (log) { log.style.display = 'block'; log.textContent = '$ pip install pocket-tts\n'; }

    // Listen for streaming output lines
    try {
        const unlisten1 = await listen('pocket_tts_install_output', (event) => {
            if (log) {
                log.textContent += event.payload + '\n';
                log.scrollTop = log.scrollHeight;
            }
        });
        _pocketTtsInstallUnlisteners.push(unlisten1);

        const unlisten2 = await listen('pocket_tts_install_done', async (event) => {
            const data = event.payload;
            // Clean up listeners
            _pocketTtsInstallUnlisteners.forEach(fn => fn());
            _pocketTtsInstallUnlisteners.length = 0;

            if (data.success) {
                if (status) status.textContent = '✅ Installed';
                if (log) log.textContent += '\n✅ ' + data.message + '\n';
                // Update config to mark as installed
                try {
                    const config = await invoke('get_config');
                    config.pocket_tts = config.pocket_tts || {};
                    config.pocket_tts.installed = true;
                    if (data.python_path) config.pocket_tts.python_path = data.python_path;
                    await invoke('save_config', { config });
                } catch (e) {
                    console.warn('[PocketTTS] Failed to update config after install:', e);
                }
            } else {
                if (status) status.textContent = '❌ ' + (data.message || 'Failed');
                if (log) log.textContent += '\n❌ ' + data.message + '\n';
            }

            // Restore button
            if (btn) {
                btn.textContent = data.success ? 'Reinstall' : 'Retry Install';
                btn.style.background = '';
                btn.style.color = '';
                btn.onclick = pocketTtsInstall;
                btn.disabled = false;
            }

            // Refresh status
            const mod = settingsManager.modules.find(m => m.id === 'pocket-tts');
            if (mod) mod.refreshStatus();
        });
        _pocketTtsInstallUnlisteners.push(unlisten2);
    } catch (e) {
        console.error('[PocketTTS] Failed to set up event listeners:', e);
    }

    // Kick off the install (returns immediately now)
    try {
        await invoke('pocket_tts_install');
    } catch (e) {
        if (status) status.textContent = '❌ ' + e;
        if (log) log.textContent += '\nERROR: ' + e + '\n';
        // Restore button
        if (btn) {
            btn.textContent = 'Retry Install';
            btn.style.background = '';
            btn.style.color = '';
            btn.onclick = pocketTtsInstall;
            btn.disabled = false;
        }
        // Clean up listeners
        _pocketTtsInstallUnlisteners.forEach(fn => fn());
        _pocketTtsInstallUnlisteners.length = 0;
    }
}

async function pocketTtsCancelInstall() {
    const invoke = window.__TAURI__.core.invoke;
    const btn = document.getElementById('pocketTtsInstallBtn');
    const status = document.getElementById('pocketTtsInstallStatus');

    if (btn) { btn.disabled = true; btn.textContent = 'Cancelling...'; }

    try {
        await invoke('pocket_tts_cancel_install');
        if (status) status.textContent = 'Cancelled';
    } catch (e) {
        if (status) status.textContent = '⚠️ ' + e;
    }

    // Button will be restored by the pocket_tts_install_done event handler
}

async function pocketTtsToggleServer() {
    const invoke = window.__TAURI__.core.invoke;
    const btn = document.getElementById('pocketTtsStartBtn');
    const status = document.getElementById('pocketTtsServerStatus');

    const mod = settingsManager.modules.find(m => m.id === 'pocket-tts');
    const isRunning = mod?._status?.server_running;

    if (btn) btn.disabled = true;

    try {
        if (isRunning) {
            if (status) status.textContent = 'Stopping...';
            await invoke('pocket_tts_stop');
        } else {
            if (status) status.textContent = 'Starting... (loading model, may take 10-30s)';
            if (btn) btn.textContent = 'Starting...';
            await invoke('pocket_tts_start');
        }
    } catch (e) {
        if (status) status.textContent = '❌ ' + e;
    }

    if (btn) btn.disabled = false;
    if (mod) mod.refreshStatus();
}

let _pocketTtsTestAudio = null;

async function pocketTtsTest() {
    const invoke = window.__TAURI__.core.invoke;
    const btn = document.getElementById('pocketTtsTestBtn');
    const spinner = document.getElementById('pocketTtsTestSpinner');
    const status = document.getElementById('pocketTtsTestStatus');

    // If already playing, stop
    if (_pocketTtsTestAudio) {
        _pocketTtsTestAudio.pause();
        _pocketTtsTestAudio = null;
        if (btn) { btn.textContent = '🔊 Test Voice'; btn.style.display = ''; }
        if (spinner) spinner.style.display = 'none';
        if (status) status.textContent = 'Stopped';
        return;
    }

    // Hide button, show spinner
    if (btn) btn.style.display = 'none';
    if (spinner) spinner.style.display = '';
    if (status) status.textContent = '';

    try {
        const config = await invoke('get_config');
        const port = config.pocket_tts?.port || 9877;
        const voice = document.getElementById('pocketTtsVoice')?.value || 'alba';

        const resp = await fetch(`http://127.0.0.1:${port}/tts`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                text: 'Hello! I am your Kiro assistant, using Pocket TTS for high quality speech.',
                voice: voice,
                stream: false,
            }),
        });

        if (!resp.ok) {
            const err = await resp.json().catch(() => ({ error: resp.statusText }));
            throw new Error(err.error || 'TTS request failed');
        }

        const blob = await resp.blob();
        const url = URL.createObjectURL(blob);
        _pocketTtsTestAudio = new Audio(url);

        _pocketTtsTestAudio.onended = () => {
            URL.revokeObjectURL(url);
            _pocketTtsTestAudio = null;
            if (btn) { btn.textContent = '🔊 Test Voice'; btn.style.display = ''; }
            if (spinner) spinner.style.display = 'none';
            if (status) status.textContent = '✅ Done';
        };
        _pocketTtsTestAudio.onerror = () => {
            URL.revokeObjectURL(url);
            _pocketTtsTestAudio = null;
            if (btn) { btn.textContent = '🔊 Test Voice'; btn.style.display = ''; }
            if (spinner) spinner.style.display = 'none';
            if (status) status.textContent = '❌ Playback error';
        };

        // Show stop button, hide spinner
        if (spinner) spinner.style.display = 'none';
        if (btn) { btn.textContent = '⏹ Stop'; btn.style.display = ''; }
        if (status) status.textContent = '🔊 Playing...';
        _pocketTtsTestAudio.play();
    } catch (e) {
        _pocketTtsTestAudio = null;
        if (btn) { btn.textContent = '🔊 Test Voice'; btn.style.display = ''; }
        if (spinner) spinner.style.display = 'none';
        if (status) status.textContent = '❌ ' + e.message;
    }
}

async function pocketTtsAddVoice() {
    const invoke = window.__TAURI__.core.invoke;
    const urlInput = document.getElementById('pocketTtsVoiceUrl');
    const nameInput = document.getElementById('pocketTtsVoiceName');
    const btn = document.getElementById('pocketTtsAddVoiceBtn');
    const status = document.getElementById('pocketTtsAddVoiceStatus');

    const voiceUrl = urlInput?.value.trim() || '';
    let voiceName = nameInput?.value.trim() || '';

    if (!voiceUrl) {
        if (status) status.textContent = '⚠️ Paste a voice URL first';
        return;
    }

    // Auto-generate name from URL if not provided
    if (!voiceName) {
        // Extract a name from the URL path
        const parts = voiceUrl.replace(/\/$/, '').split('/');
        const lastPart = parts[parts.length - 1] || 'custom';
        voiceName = lastPart.replace(/\.(wav|mp3|safetensors)$/i, '').replace(/[^a-zA-Z0-9_-]/g, '_');
    }

    if (btn) { btn.disabled = true; btn.textContent = 'Loading...'; }
    if (status) status.textContent = '⏳ Downloading and processing voice...';

    try {
        const config = await invoke('get_config');
        const port = config.pocket_tts?.port || 9877;

        // Use the server's load-voice endpoint with the URL as the voice name
        // The server will download it, cache it, and make it available
        const resp = await fetch(`http://127.0.0.1:${port}/load-voice`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ voice: voiceUrl }),
        });

        if (!resp.ok) {
            const err = await resp.json().catch(() => ({ error: 'Failed' }));
            throw new Error(err.error || 'Failed to load voice');
        }

        // Now export it with the friendly name
        const exportResp = await fetch(`http://127.0.0.1:${port}/export-voice`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ wav_path: voiceUrl, output_name: voiceName }),
        });

        if (exportResp.ok) {
            if (status) status.textContent = `✅ Voice "${voiceName}" added and cached`;
        } else {
            if (status) status.textContent = `✅ Voice loaded (export may have failed — it will still work but load slower next time)`;
        }

        // Clear inputs
        if (urlInput) urlInput.value = '';
        if (nameInput) nameInput.value = '';

        // Refresh voice list
        const mod = settingsManager.modules.find(m => m.id === 'pocket-tts');
        if (mod) mod.populateVoices();

    } catch (e) {
        if (status) status.textContent = '❌ ' + e.message;
    } finally {
        if (btn) { btn.disabled = false; btn.textContent = 'Add'; }
    }
}

async function pocketTtsOpenVoicesDir() {
    // Open the custom voices directory in the file explorer
    const invoke = window.__TAURI__.core.invoke;
    try {
        const config = await invoke('get_config');
        // The voices dir is inside the pocket-tts data dir
        // We'll use the open_path command to open it
        let basePath;
        if (navigator.platform.startsWith('Win')) {
            basePath = (await invoke('get_user_info')).home || '';
            basePath += '\\AppData\\Local\\kiro-assistant\\pocket-tts\\voices';
        } else if (navigator.platform === 'MacIntel') {
            basePath = '~/Library/Application Support/kiro-assistant/pocket-tts/voices';
        } else {
            basePath = '~/.local/share/kiro-assistant/pocket-tts/voices';
        }
        await invoke('open_path', { path: basePath });
    } catch (e) {
        console.warn('[PocketTTS] Failed to open voices dir:', e);
    }
}
