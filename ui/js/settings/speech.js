/**
 * Unified Speech Settings Module
 * Combines voice input (STT), read-back (TTS), and Pocket TTS configuration.
 */
class SpeechSettingsModule extends SettingsModule {
    constructor() {
        super('speech', 'Speech', '🎙️');
        this._pocketStatus = null;
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                <div class="setting-section-label">Voice Input</div>

                <!-- Voice Input -->
                ${this.createCheckboxRow(
                    'Show Speech Button',
                    'Display a microphone button for voice input using speech-to-text.',
                    'showSpeechButton',
                    false
                )}

                <div id="speechSilenceRow">
                ${this.createControlRow(
                    'Auto-Submit Silence Delay',
                    'Automatically send the message after this many seconds of silence. Set to 0 to disable.',
                    '<div class="range-container"><input type="range" class="range-slider" id="speechSilenceTimeout" min="0" max="5" step="0.5" value="2"><span class="range-value" id="speechSilenceValue">2.0s</span></div>'
                )}
                </div>

                <!-- Read Back -->
                <div class="setting-section-label">Agent Voice</div>

                <div id="speechReadBackRow">
                ${this.createCheckboxRow(
                    'Read Back Agent Responses',
                    'Use text-to-speech to read agent responses aloud when voice input was used.',
                    'speechReadBack',
                    false
                )}
                </div>

                <!-- TTS Engine -->
                <div id="ttsEngineSection">
                ${this.createControlRow(
                    'TTS Engine',
                    'Choose which engine reads back responses.',
                    '<select class="setting-select" id="ttsEngine"><option value="system">System (Browser)</option><option value="pocket-tts">Pocket TTS (Local AI)</option></select>'
                )}
                </div>

                <!-- System Voice (shown when engine = system) -->
                <div id="systemVoiceSection">
                ${this.createControlRow(
                    'System Voice',
                    'Select the browser voice used for reading back responses.',
                    '<select class="setting-select" id="speechVoice"><option value="">System Default</option></select>'
                )}
                </div>

                <!-- Pocket TTS Section (shown when engine = pocket-tts) -->
                <div id="pocketTtsSection" style="display:none;">

                    <!-- Status + Setup (collapsible) -->
                    <div id="pocketTtsSetupSection">
                        <div id="pocketTtsStatusBanner" class="setting-row" style="border-radius:8px;background:var(--kage-bg-secondary);margin-bottom:8px;cursor:pointer;" onclick="togglePocketTtsSetup()">
                            <span id="pocketTtsStatusText" style="font-size:12px;">Checking...</span>
                            <span id="pocketTtsSetupToggle" style="float:right;font-size:11px;color:var(--kage-text-muted);">▼ Setup</span>
                        </div>
                        <div id="pocketTtsSetupSteps" style="display:none;">
                            <div id="pocketTtsStep1" class="setting-row" style="display:none;">
                                <div class="setting-label">Python</div>
                                <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                                    <span id="pocketTtsPythonStatus" style="font-size:12px;">Checking...</span>
                                </div>
                            </div>
                            <div id="pocketTtsStep2" class="setting-row" style="display:none;">
                                <div class="setting-label">Install pocket-tts</div>
                                <div class="setting-description">Installs the Python package (~400MB model download on first use).</div>
                                <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                                    <button class="setting-button" id="pocketTtsInstallBtn" onclick="pocketTtsInstall()">Install</button>
                                    <span id="pocketTtsInstallStatus" style="font-size:12px;"></span>
                                </div>
                                <pre id="pocketTtsInstallLog" style="display:none;font-size:11px;max-height:150px;overflow-y:auto;background:var(--kage-bg-tertiary);padding:8px;border-radius:6px;margin-top:8px;white-space:pre-wrap;word-break:break-all;"></pre>
                            </div>
                            <div id="pocketTtsStep3" class="setting-row" style="display:none;">
                                <div class="setting-label">Server</div>
                                <div class="setting-description">First start takes ~10-30s to load the model.</div>
                                <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                                    <button class="setting-button" id="pocketTtsStartBtn" onclick="pocketTtsToggleServer()">Start Server</button>
                                    <span id="pocketTtsServerStatus" style="font-size:12px;"></span>
                                </div>
                            </div>
                        </div>
                    </div>

                    <!-- Pocket TTS Config (shown when installed) -->
                    <div id="pocketTtsConfig" style="display:none;">

                        ${this.createCheckboxRow(
                            'Auto-Start Server',
                            'Pre-start the Pocket TTS server when Kage launches for faster first speech. If off, the server starts automatically on first use (with a brief delay).',
                            'pocketTtsAutoStart',
                            false
                        )}

                        <!-- Voice & Generation -->
                        <div class="setting-row"><div class="setting-label" style="font-size:11px;text-transform:uppercase;letter-spacing:0.5px;color:var(--kage-text-muted);margin-bottom:4px;">Voice & Generation</div></div>

                        ${this.createControlRow(
                            'Voice',
                            'Built-in voices are English. Add custom voices via URL or the voices folder.',
                            '<select class="setting-select" id="pocketTtsVoice"></select>'
                        )}

                        ${this.createControlRow(
                            'Temperature',
                            'Lower = consistent, higher = expressive. Default: 0.7.',
                            '<div class="range-container"><input type="range" class="range-slider" id="pocketTtsTemp" min="0.3" max="1.0" step="0.1" value="0.7"><span class="range-value" id="pocketTtsTempValue">0.7</span></div>'
                        )}

                        ${this.createControlRow(
                            'EOS Threshold',
                            'Lower = less likely to stop early. Default: -4.0.',
                            '<div class="range-container"><input type="range" class="range-slider" id="pocketTtsEos" min="-8.0" max="-1.0" step="0.5" value="-4.0"><span class="range-value" id="pocketTtsEosValue">-4.0</span></div>'
                        )}

                        <!-- Test -->
                        <div class="setting-row">
                            <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                                <button class="setting-button" id="pocketTtsTestBtn" onclick="pocketTtsTest()">🔊 Test Voice</button>
                                <span id="pocketTtsTestSpinner" style="display:none;font-size:12px;">⏳ Generating...</span>
                                <span id="pocketTtsTestStatus" style="font-size:12px;"></span>
                            </div>
                        </div>

                        <!-- Custom Voices -->
                        <div class="setting-row"><div class="setting-label" style="font-size:11px;text-transform:uppercase;letter-spacing:0.5px;color:var(--kage-text-muted);margin-bottom:4px;">Custom Voices</div></div>

                        <div class="setting-row">
                            <div class="setting-description">
                                Paste a <a href="https://huggingface.co/kyutai/tts-voices" target="_blank">HuggingFace voice URL</a> or HTTP URL to a .wav file.
                            </div>
                            <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                                <input type="text" class="setting-input" id="pocketTtsVoiceUrl" placeholder="hf://kyutai/tts-voices/..." style="flex:1;">
                                <input type="text" class="setting-input" id="pocketTtsVoiceName" placeholder="Name" style="width:100px;">
                                <button class="setting-button" id="pocketTtsAddVoiceBtn" onclick="pocketTtsAddVoice()">Add</button>
                            </div>
                            <div id="pocketTtsAddVoiceStatus" style="font-size:12px;margin-top:4px;"></div>
                        </div>

                        <div class="setting-row">
                            <div class="setting-description">Or place .wav files directly in the voices folder (5-20s clean audio works best).</div>
                            <div class="setting-control">
                                <button class="setting-button" onclick="pocketTtsOpenVoicesDir()">Open Voices Folder</button>
                            </div>
                        </div>

                        <!-- Advanced -->
                        <div class="setting-row"><div class="setting-label" style="font-size:11px;text-transform:uppercase;letter-spacing:0.5px;color:var(--kage-text-muted);margin-bottom:4px;">Advanced</div></div>

                        ${this.createControlRow(
                            'Server Port',
                            'Change only if 9877 conflicts with another service.',
                            '<input type="number" class="setting-input" id="pocketTtsPort" min="1024" max="65535" value="9877" style="width:100px;">'
                        )}
                    </div>
                </div>
            </div>
        `;
    }


    load(config) {
        // Speech input settings
        const ui = config.ui || {};
        const showSpeech = document.getElementById('showSpeechButton');
        const readBack = document.getElementById('speechReadBack');
        const silence = document.getElementById('speechSilenceTimeout');
        const silenceValue = document.getElementById('speechSilenceValue');
        if (showSpeech) showSpeech.checked = ui.show_speech_button === true;
        if (readBack) readBack.checked = ui.speech_read_back === true;
        if (silence) {
            silence.value = ui.speech_silence_timeout ?? 2.0;
            if (silenceValue) silenceValue.textContent = (ui.speech_silence_timeout ?? 2.0).toFixed(1) + 's';
        }
        this._savedSystemVoice = ui.speech_voice || '';

        // TTS engine selection
        const ptts = config.pocket_tts || {};
        const engine = document.getElementById('ttsEngine');
        if (engine) engine.value = ptts.enabled ? 'pocket-tts' : 'system';

        // Pocket TTS settings
        const autoStart = document.getElementById('pocketTtsAutoStart');
        const port = document.getElementById('pocketTtsPort');
        const temp = document.getElementById('pocketTtsTemp');
        const tempValue = document.getElementById('pocketTtsTempValue');
        const eos = document.getElementById('pocketTtsEos');
        const eosValue = document.getElementById('pocketTtsEosValue');
        if (autoStart) autoStart.checked = ptts.auto_start === true;
        if (port) port.value = ptts.port || 9877;
        if (temp) { temp.value = ptts.temp ?? 0.7; if (tempValue) tempValue.textContent = (ptts.temp ?? 0.7).toFixed(1); }
        if (eos) { eos.value = ptts.eos_threshold ?? -4.0; if (eosValue) eosValue.textContent = (ptts.eos_threshold ?? -4.0).toFixed(1); }
        this._savedPocketVoice = ptts.voice || 'alba';

        this._populateSystemVoices();
        this._toggleSections();
        this._refreshPocketStatus();
    }

    save(config) {
        // Speech input
        config.ui = config.ui || {};
        config.ui.show_speech_button = document.getElementById('showSpeechButton')?.checked ?? false;
        config.ui.speech_read_back = document.getElementById('speechReadBack')?.checked ?? false;
        config.ui.speech_silence_timeout = parseFloat(document.getElementById('speechSilenceTimeout')?.value ?? '2');
        config.ui.speech_voice = document.getElementById('speechVoice')?.value || null;

        // TTS engine
        const engine = document.getElementById('ttsEngine')?.value || 'system';
        config.pocket_tts = config.pocket_tts || {};
        config.pocket_tts.enabled = engine === 'pocket-tts';
        config.pocket_tts.auto_start = document.getElementById('pocketTtsAutoStart')?.checked ?? false;
        config.pocket_tts.voice = document.getElementById('pocketTtsVoice')?.value || 'alba';
        config.pocket_tts.port = parseInt(document.getElementById('pocketTtsPort')?.value || '9877', 10);
        config.pocket_tts.temp = parseFloat(document.getElementById('pocketTtsTemp')?.value || '0.7');
        config.pocket_tts.eos_threshold = parseFloat(document.getElementById('pocketTtsEos')?.value || '-4.0');
        if (this._pocketStatus) {
            config.pocket_tts.python_path = this._pocketStatus.python_path || null;
            config.pocket_tts.installed = this._pocketStatus.installed || false;
        }
    }

    validate() {
        const port = parseInt(document.getElementById('pocketTtsPort')?.value || '9877', 10);
        if (port < 1024 || port > 65535) return { valid: false, error: 'Pocket TTS port must be between 1024 and 65535.' };
        return { valid: true };
    }

    initialize() {
        // Engine toggle
        document.getElementById('ttsEngine')?.addEventListener('change', () => this._toggleSections());
        document.getElementById('showSpeechButton')?.addEventListener('change', () => this._toggleSections());

        // Silence slider
        const silence = document.getElementById('speechSilenceTimeout');
        const silenceValue = document.getElementById('speechSilenceValue');
        if (silence && silenceValue) {
            silence.addEventListener('input', () => { const v = parseFloat(silence.value); silenceValue.textContent = v === 0 ? 'Off' : v.toFixed(1) + 's'; });
        }

        // Pocket TTS sliders
        const temp = document.getElementById('pocketTtsTemp');
        const tempValue = document.getElementById('pocketTtsTempValue');
        if (temp && tempValue) temp.addEventListener('input', () => { tempValue.textContent = parseFloat(temp.value).toFixed(1); });
        const eos = document.getElementById('pocketTtsEos');
        const eosValue = document.getElementById('pocketTtsEosValue');
        if (eos && eosValue) eos.addEventListener('input', () => { eosValue.textContent = parseFloat(eos.value).toFixed(1); });

        // System voices may load async
        if (speechSynthesis.onvoiceschanged !== undefined) {
            speechSynthesis.onvoiceschanged = () => this._populateSystemVoices();
        }

        this._refreshPocketStatus();
    }

    destroy() {}

    // ── Internal ──

    _toggleSections() {
        const speechEnabled = document.getElementById('showSpeechButton')?.checked;
        const engine = document.getElementById('ttsEngine')?.value || 'system';

        // Dim read-back and everything below when speech is off
        const readBackRow = document.getElementById('speechReadBackRow');
        const silenceRow = document.getElementById('speechSilenceRow');
        const engineSection = document.getElementById('ttsEngineSection');
        const systemSection = document.getElementById('systemVoiceSection');
        const pocketSection = document.getElementById('pocketTtsSection');

        [readBackRow, silenceRow, engineSection].forEach(el => {
            if (el) { el.style.opacity = speechEnabled ? '1' : '0.4'; el.style.pointerEvents = speechEnabled ? '' : 'none'; }
        });

        // Show the right engine section
        if (systemSection) systemSection.style.display = (speechEnabled && engine === 'system') ? '' : 'none';
        if (pocketSection) pocketSection.style.display = (speechEnabled && engine === 'pocket-tts') ? '' : 'none';
    }

    _populateSystemVoices() {
        const select = document.getElementById('speechVoice');
        if (!select) return;
        const voices = speechSynthesis.getVoices();
        select.innerHTML = '<option value="">System Default</option>';
        for (const voice of voices) {
            const opt = document.createElement('option');
            opt.value = voice.name;
            opt.textContent = `${voice.name} (${voice.lang})`;
            select.appendChild(opt);
        }
        if (this._savedSystemVoice) select.value = this._savedSystemVoice;
    }

    async _refreshPocketStatus() {
        const invoke = window.__TAURI__.core.invoke;
        try {
            this._pocketStatus = await invoke('pocket_tts_check_install');
            this._updatePocketUI();
            this._populatePocketVoices();
        } catch (e) {
            console.warn('[Speech] Pocket TTS status check failed:', e);
        }
    }

    _updatePocketUI() {
        const s = this._pocketStatus;
        if (!s) return;

        const step1 = document.getElementById('pocketTtsStep1');
        const step2 = document.getElementById('pocketTtsStep2');
        const step3 = document.getElementById('pocketTtsStep3');
        const config = document.getElementById('pocketTtsConfig');
        const pythonStatus = document.getElementById('pocketTtsPythonStatus');
        const installStatus = document.getElementById('pocketTtsInstallStatus');
        const serverStatus = document.getElementById('pocketTtsServerStatus');
        const startBtn = document.getElementById('pocketTtsStartBtn');
        const setupSteps = document.getElementById('pocketTtsSetupSteps');
        const setupToggle = document.getElementById('pocketTtsSetupToggle');
        const statusText = document.getElementById('pocketTtsStatusText');

        if (step1) step1.style.display = '';

        if (!s.python_found) {
            if (pythonStatus) pythonStatus.innerHTML = '❌ Python 3 not found. <a href="https://www.python.org/downloads/" target="_blank">Download Python</a>';
            if (statusText) statusText.innerHTML = '❌ Python 3.10+ required';
            if (step2) step2.style.display = 'none';
            if (step3) step3.style.display = 'none';
            if (config) config.style.display = 'none';
            // Force setup open
            if (setupSteps) setupSteps.style.display = '';
            if (setupToggle) setupToggle.textContent = '▲ Setup';
            this._setStatusColor('error');
            return;
        }
        if (pythonStatus) pythonStatus.textContent = `✅ ${s.python_path || 'python3'}`;
        if (step2) step2.style.display = '';

        if (!s.installed) {
            if (installStatus) installStatus.textContent = 'Not installed';
            if (statusText) statusText.textContent = '📦 Not installed — expand setup below';
            if (step3) step3.style.display = 'none';
            if (config) config.style.display = 'none';
            if (setupSteps) setupSteps.style.display = '';
            if (setupToggle) setupToggle.textContent = '▲ Setup';
            this._setStatusColor('info');
            return;
        }

        if (installStatus) installStatus.textContent = '✅ Installed';
        const installBtn = document.getElementById('pocketTtsInstallBtn');
        if (installBtn) { installBtn.textContent = 'Reinstall'; installBtn.style.opacity = '0.7'; }
        if (step3) step3.style.display = '';
        if (config) config.style.display = '';

        if (s.server_running) {
            if (serverStatus) serverStatus.textContent = '✅ Running';
            if (startBtn) { startBtn.textContent = 'Stop Server'; startBtn.style.background = '#c44'; }
            if (statusText) statusText.textContent = '✅ Ready';
            this._setStatusColor('success');
            // Auto-collapse setup when everything is good
            if (setupSteps) setupSteps.style.display = 'none';
            if (setupToggle) setupToggle.textContent = '▼ Setup';
        } else {
            if (serverStatus) serverStatus.textContent = '⏹ Stopped';
            if (startBtn) { startBtn.textContent = 'Start Server'; startBtn.style.background = ''; }
            if (statusText) statusText.textContent = '⏹ Server stopped';
            this._setStatusColor('info');
            // Show setup so user can start server
            if (setupSteps) setupSteps.style.display = '';
            if (setupToggle) setupToggle.textContent = '▲ Setup';
        }
    }

    _setStatusColor(type) {
        const banner = document.getElementById('pocketTtsStatusBanner');
        if (!banner) return;
        const colors = { success: 'rgba(76,175,80,0.15)', error: 'rgba(244,67,54,0.15)', warning: 'rgba(255,152,0,0.15)', info: 'rgba(33,150,243,0.15)' };
        banner.style.background = colors[type] || colors.info;
    }

    async _populatePocketVoices() {
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
                opt.textContent = v.type === 'custom' ? `${v.name} (custom)` : v.name;
                if (v.loaded) opt.textContent += ' ✓';
                select.appendChild(opt);
            }
            if (this._savedPocketVoice) select.value = this._savedPocketVoice;
        } catch (e) { console.warn('[Speech] Failed to load pocket voices:', e); }
    }
}


// ── Global functions for onclick handlers ──

function togglePocketTtsSetup() {
    const steps = document.getElementById('pocketTtsSetupSteps');
    const toggle = document.getElementById('pocketTtsSetupToggle');
    if (!steps) return;
    const visible = steps.style.display !== 'none';
    steps.style.display = visible ? 'none' : '';
    if (toggle) toggle.textContent = visible ? '▼ Setup' : '▲ Setup';
}

let _pocketTtsInstallUnlisteners = [];

async function pocketTtsInstall() {
    const invoke = window.__TAURI__.core.invoke;
    const listen = window.__TAURI__.event.listen;
    const btn = document.getElementById('pocketTtsInstallBtn');
    const status = document.getElementById('pocketTtsInstallStatus');
    const log = document.getElementById('pocketTtsInstallLog');

    if (btn) { btn.textContent = '✕ Cancel'; btn.style.background = '#c44'; btn.style.color = 'white'; btn.onclick = pocketTtsCancelInstall; }
    if (status) status.textContent = 'Installing...';
    if (log) { log.style.display = 'block'; log.textContent = '$ pip install pocket-tts\n'; }

    try {
        const unlisten1 = await listen('pocket_tts_install_output', (event) => {
            if (log) { log.textContent += event.payload + '\n'; log.scrollTop = log.scrollHeight; }
        });
        _pocketTtsInstallUnlisteners.push(unlisten1);

        const unlisten2 = await listen('pocket_tts_install_done', async (event) => {
            const data = event.payload;
            _pocketTtsInstallUnlisteners.forEach(fn => fn());
            _pocketTtsInstallUnlisteners.length = 0;

            if (data.success) {
                if (status) status.textContent = '✅ Installed';
                if (log) log.textContent += '\n✅ ' + data.message + '\n';
                try {
                    const config = await invoke('get_config');
                    config.pocket_tts = config.pocket_tts || {};
                    config.pocket_tts.installed = true;
                    if (data.python_path) config.pocket_tts.python_path = data.python_path;
                    await invoke('save_config', { config });
                } catch (e) { console.warn('[Speech] Config update failed:', e); }
            } else {
                if (status) status.textContent = '❌ ' + (data.message || 'Failed');
                if (log) log.textContent += '\n❌ ' + data.message + '\n';
            }
            if (btn) { btn.textContent = data.success ? 'Reinstall' : 'Retry'; btn.style.background = ''; btn.style.color = ''; btn.onclick = pocketTtsInstall; btn.disabled = false; }
            const mod = settingsManager.modules.find(m => m.id === 'speech');
            if (mod) mod._refreshPocketStatus();
        });
        _pocketTtsInstallUnlisteners.push(unlisten2);
    } catch (e) { console.error('[Speech] Event listener setup failed:', e); }

    try { await invoke('pocket_tts_install'); } catch (e) {
        if (status) status.textContent = '❌ ' + e;
        if (btn) { btn.textContent = 'Retry'; btn.style.background = ''; btn.style.color = ''; btn.onclick = pocketTtsInstall; btn.disabled = false; }
        _pocketTtsInstallUnlisteners.forEach(fn => fn());
        _pocketTtsInstallUnlisteners.length = 0;
    }
}

async function pocketTtsCancelInstall() {
    const invoke = window.__TAURI__.core.invoke;
    const btn = document.getElementById('pocketTtsInstallBtn');
    if (btn) { btn.disabled = true; btn.textContent = 'Cancelling...'; }
    try { await invoke('pocket_tts_cancel_install'); } catch (e) { console.warn('[Speech] Failed to cancel Pocket TTS install:', e); }
}

async function pocketTtsToggleServer() {
    const invoke = window.__TAURI__.core.invoke;
    const btn = document.getElementById('pocketTtsStartBtn');
    const status = document.getElementById('pocketTtsServerStatus');
    const mod = settingsManager.modules.find(m => m.id === 'speech');
    const isRunning = mod?._pocketStatus?.server_running;
    if (btn) btn.disabled = true;
    try {
        if (isRunning) { if (status) status.textContent = 'Stopping...'; await invoke('pocket_tts_stop'); }
        else { if (status) status.textContent = 'Starting...'; if (btn) btn.textContent = 'Starting...'; await invoke('pocket_tts_start'); }
    } catch (e) { if (status) status.textContent = '❌ ' + e; }
    if (btn) btn.disabled = false;
    if (mod) mod._refreshPocketStatus();
}

let _pocketTtsTestAudio = null;
async function pocketTtsTest() {
    const invoke = window.__TAURI__.core.invoke;
    const btn = document.getElementById('pocketTtsTestBtn');
    const spinner = document.getElementById('pocketTtsTestSpinner');
    const status = document.getElementById('pocketTtsTestStatus');
    if (_pocketTtsTestAudio) { _pocketTtsTestAudio.pause(); _pocketTtsTestAudio = null; if (btn) { btn.textContent = '🔊 Test Voice'; btn.style.display = ''; } if (spinner) spinner.style.display = 'none'; if (status) status.textContent = 'Stopped'; return; }
    if (btn) btn.style.display = 'none'; if (spinner) spinner.style.display = ''; if (status) status.textContent = '';
    try {
        const config = await invoke('get_config');
        const port = config.pocket_tts?.port || 9877;
        const voice = document.getElementById('pocketTtsVoice')?.value || 'alba';
        const resp = await fetch(`http://127.0.0.1:${port}/tts`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ text: 'Hello! I am your Kage, using Pocket TTS for high quality speech.', voice, stream: false }) });
        if (!resp.ok) { const err = await resp.json().catch(() => ({ error: resp.statusText })); throw new Error(err.error || 'Failed'); }
        const blob = await resp.blob(); const url = URL.createObjectURL(blob);
        _pocketTtsTestAudio = new Audio(url);
        _pocketTtsTestAudio.onended = () => { URL.revokeObjectURL(url); _pocketTtsTestAudio = null; if (btn) { btn.textContent = '🔊 Test Voice'; btn.style.display = ''; } if (spinner) spinner.style.display = 'none'; if (status) status.textContent = '✅ Done'; };
        _pocketTtsTestAudio.onerror = () => { URL.revokeObjectURL(url); _pocketTtsTestAudio = null; if (btn) { btn.textContent = '🔊 Test Voice'; btn.style.display = ''; } if (spinner) spinner.style.display = 'none'; if (status) status.textContent = '❌ Error'; };
        if (spinner) spinner.style.display = 'none'; if (btn) { btn.textContent = '⏹ Stop'; btn.style.display = ''; } if (status) status.textContent = '🔊 Playing...';
        _pocketTtsTestAudio.play();
    } catch (e) { _pocketTtsTestAudio = null; if (btn) { btn.textContent = '🔊 Test Voice'; btn.style.display = ''; } if (spinner) spinner.style.display = 'none'; if (status) status.textContent = '❌ ' + e.message; }
}

async function pocketTtsAddVoice() {
    const invoke = window.__TAURI__.core.invoke;
    const urlInput = document.getElementById('pocketTtsVoiceUrl');
    const nameInput = document.getElementById('pocketTtsVoiceName');
    const btn = document.getElementById('pocketTtsAddVoiceBtn');
    const status = document.getElementById('pocketTtsAddVoiceStatus');
    const voiceUrl = urlInput?.value.trim() || '';
    let voiceName = nameInput?.value.trim() || '';
    if (!voiceUrl) { if (status) status.textContent = '⚠️ Paste a voice URL first'; return; }
    if (!voiceName) { const parts = voiceUrl.replace(/\/$/, '').split('/'); voiceName = (parts[parts.length - 1] || 'custom').replace(/\.(wav|mp3|safetensors)$/i, '').replace(/[^a-zA-Z0-9_-]/g, '_'); }
    if (btn) { btn.disabled = true; btn.textContent = 'Loading...'; }
    if (status) status.textContent = '⏳ Downloading...';
    try {
        const config = await invoke('get_config');
        const port = config.pocket_tts?.port || 9877;
        const resp = await fetch(`http://127.0.0.1:${port}/load-voice`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ voice: voiceUrl }) });
        if (!resp.ok) { const err = await resp.json().catch(() => ({ error: 'Failed' })); throw new Error(err.error || 'Failed'); }
        await fetch(`http://127.0.0.1:${port}/export-voice`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ wav_path: voiceUrl, output_name: voiceName }) });
        if (status) status.textContent = `✅ "${voiceName}" added`;
        if (urlInput) urlInput.value = ''; if (nameInput) nameInput.value = '';
        const mod = settingsManager.modules.find(m => m.id === 'speech');
        if (mod) mod._populatePocketVoices();
    } catch (e) { if (status) status.textContent = '❌ ' + e.message; }
    finally { if (btn) { btn.disabled = false; btn.textContent = 'Add'; } }
}

async function pocketTtsOpenVoicesDir() {
    const invoke = window.__TAURI__.core.invoke;
    try {
        let basePath;
        if (navigator.platform.startsWith('Win')) { basePath = ((await invoke('get_user_info')).home || '') + '\\AppData\\Local\\kage\\pocket-tts\\voices'; }
        else if (navigator.platform === 'MacIntel') { basePath = '~/Library/Application Support/kage/pocket-tts/voices'; }
        else { basePath = '~/.local/share/kage/pocket-tts/voices'; }
        await invoke('open_path', { path: basePath });
    } catch (e) { console.warn('[Speech] Failed to open voices dir:', e); }
}
