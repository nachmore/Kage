import { t } from '../shared/i18n.js';

export function renderSpeechSettings(module) {
    return `
            <div class="settings-section" id="${module.id}-section">
                <h2 class="settings-section-header">${module.icon} ${module.title}</h2>

                <div class="setting-section-label">${t('settings.speech.voice_input.section')}</div>

                <!-- Voice Input -->
                ${module.createCheckboxRow(
                    t('settings.speech.show_button.label'),
                    t('settings.speech.show_button.description'),
                    'showSpeechButton',
                    false
                )}

                <div id="speechSilenceRow">
                ${module.createControlRow(
                    t('settings.speech.silence.label'),
                    t('settings.speech.silence.description'),
                    '<div class="range-container"><input type="range" class="range-slider" id="speechSilenceTimeout" min="0" max="5" step="0.5" value="2"><span class="range-value" id="speechSilenceValue">2.0s</span></div>'
                )}
                </div>

                <!-- Read Back -->
                <div class="setting-section-label">${t('settings.speech.agent_voice.section')}</div>

                <div id="speechReadBackRow">
                ${module.createCheckboxRow(
                    t('settings.speech.read_back.label'),
                    t('settings.speech.read_back.description'),
                    'speechReadBack',
                    false
                )}
                </div>

                <!-- TTS Engine -->
                <div id="ttsEngineSection">
                ${module.createControlRow(
                    t('settings.speech.tts_engine.label'),
                    t('settings.speech.tts_engine.description'),
                    `<select class="setting-select" id="ttsEngine"><option value="system">${t('settings.speech.tts_engine.system')}</option><option value="pocket-tts">${t('settings.speech.tts_engine.pocket')}</option></select>`
                )}
                </div>

                <!-- System Voice (shown when engine = system) -->
                <div id="systemVoiceSection">
                ${module.createControlRow(
                    t('settings.speech.system_voice.label'),
                    t('settings.speech.system_voice.description'),
                    `<select class="setting-select" id="speechVoice"><option value="">${t('settings.speech.system_voice.default')}</option></select>`
                )}
                </div>

                <!-- Pocket TTS Section (shown when engine = pocket-tts) -->
                <div id="pocketTtsSection" style="display:none;">

                    <!-- Status + Setup (collapsible) -->
                    <div id="pocketTtsSetupSection">
                        <div id="pocketTtsStatusBanner" class="setting-row" style="border-radius:8px;background:var(--kage-bg-secondary);margin-bottom:8px;cursor:pointer;" data-action="speech.togglePocketTtsSetup">
                            <span id="pocketTtsStatusText" style="font-size:12px;">${t('settings.speech.pocket.checking')}</span>
                            <span id="pocketTtsSetupToggle" style="float:right;font-size:11px;color:var(--kage-text-muted);">${t('settings.speech.pocket.setup_collapse')}</span>
                        </div>
                        <div id="pocketTtsSetupSteps" style="display:none;">
                            <div id="pocketTtsStep1" class="setting-row" style="display:none;">
                                <div class="setting-label">${t('settings.speech.pocket.python.label')}</div>
                                <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                                    <span id="pocketTtsPythonStatus" style="font-size:12px;">${t('settings.speech.pocket.checking')}</span>
                                </div>
                            </div>
                            <div id="pocketTtsStep2" class="setting-row" style="display:none;">
                                <div class="setting-label">${t('settings.speech.pocket.install.label')}</div>
                                <div class="setting-description">${t('settings.speech.pocket.install.description')}</div>
                                <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                                    <button class="setting-button" id="pocketTtsInstallBtn" data-action="speech.pocketTtsInstall">${t('settings.speech.pocket.install.button')}</button>
                                    <span id="pocketTtsInstallStatus" style="font-size:12px;"></span>
                                </div>
                                <pre id="pocketTtsInstallLog" style="display:none;font-size:11px;max-height:150px;overflow-y:auto;background:var(--kage-bg-tertiary);padding:8px;border-radius:6px;margin-top:8px;white-space:pre-wrap;word-break:break-all;"></pre>
                            </div>
                            <div id="pocketTtsStep3" class="setting-row" style="display:none;">
                                <div class="setting-label">${t('settings.speech.pocket.server.label')}</div>
                                <div class="setting-description">${t('settings.speech.pocket.server.description')}</div>
                                <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                                    <button class="setting-button" id="pocketTtsStartBtn" data-action="speech.pocketTtsToggleServer">${t('settings.speech.pocket.server.start')}</button>
                                    <span id="pocketTtsServerStatus" style="font-size:12px;"></span>
                                </div>
                            </div>
                        </div>
                    </div>

                    <!-- Pocket TTS Config (shown when installed) -->
                    <div id="pocketTtsConfig" style="display:none;">

                        ${module.createCheckboxRow(
                            t('settings.speech.pocket.auto_start.label'),
                            t('settings.speech.pocket.auto_start.description'),
                            'pocketTtsAutoStart',
                            false
                        )}

                        <!-- Voice & Generation -->
                        <div class="setting-row"><div class="setting-label" style="font-size:11px;text-transform:uppercase;letter-spacing:0.5px;color:var(--kage-text-muted);margin-bottom:4px;">${t('settings.speech.pocket.voice_gen.section')}</div></div>

                        ${module.createControlRow(
                            t('settings.speech.pocket.voice.label'),
                            t('settings.speech.pocket.voice.description'),
                            '<select class="setting-select" id="pocketTtsVoice"></select>'
                        )}

                        ${module.createControlRow(
                            t('settings.speech.pocket.temp.label'),
                            t('settings.speech.pocket.temp.description'),
                            '<div class="range-container"><input type="range" class="range-slider" id="pocketTtsTemp" min="0.3" max="1.0" step="0.1" value="0.7"><span class="range-value" id="pocketTtsTempValue">0.7</span></div>'
                        )}

                        ${module.createControlRow(
                            t('settings.speech.pocket.eos.label'),
                            t('settings.speech.pocket.eos.description'),
                            '<div class="range-container"><input type="range" class="range-slider" id="pocketTtsEos" min="-8.0" max="-1.0" step="0.5" value="-4.0"><span class="range-value" id="pocketTtsEosValue">-4.0</span></div>'
                        )}

                        <!-- Test -->
                        <div class="setting-row">
                            <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                                <button class="setting-button" id="pocketTtsTestBtn" data-action="speech.pocketTtsTest">${t('settings.speech.pocket.test_btn')}</button>
                                <span id="pocketTtsTestSpinner" style="display:none;font-size:12px;">${t('settings.speech.pocket.test_generating')}</span>
                                <span id="pocketTtsTestStatus" style="font-size:12px;"></span>
                            </div>
                        </div>

                        <!-- Custom Voices -->
                        <div class="setting-row"><div class="setting-label" style="font-size:11px;text-transform:uppercase;letter-spacing:0.5px;color:var(--kage-text-muted);margin-bottom:4px;">${t('settings.speech.pocket.custom.section')}</div></div>

                        <div class="setting-row">
                            <div class="setting-description">
                                ${t('settings.speech.pocket.custom.help_html')}
                            </div>
                            <div class="setting-control" style="display:flex;gap:8px;align-items:center;">
                                <input type="text" class="setting-input" id="pocketTtsVoiceUrl" placeholder="${t('settings.speech.pocket.custom.url_placeholder')}" style="flex:1;">
                                <input type="text" class="setting-input" id="pocketTtsVoiceName" placeholder="${t('settings.speech.pocket.custom.name_placeholder')}" style="width:100px;">
                                <button class="setting-button" id="pocketTtsAddVoiceBtn" data-action="speech.pocketTtsAddVoice">${t('settings.speech.pocket.custom.add_btn')}</button>
                            </div>
                            <div id="pocketTtsAddVoiceStatus" style="font-size:12px;margin-top:4px;"></div>
                        </div>

                        <div class="setting-row">
                            <div class="setting-description">${t('settings.speech.pocket.custom.local_help')}</div>
                            <div class="setting-control">
                                <button class="setting-button" data-action="speech.pocketTtsOpenVoicesDir">${t('settings.speech.pocket.custom.open_dir')}</button>
                            </div>
                        </div>

                        <!-- Advanced -->
                        <div class="setting-row"><div class="setting-label" style="font-size:11px;text-transform:uppercase;letter-spacing:0.5px;color:var(--kage-text-muted);margin-bottom:4px;">${t('settings.speech.pocket.advanced.section')}</div></div>

                        ${module.createControlRow(
                            t('settings.speech.pocket.port.label'),
                            t('settings.speech.pocket.port.description'),
                            '<input type="number" class="setting-input" id="pocketTtsPort" min="1024" max="65535" value="9877" style="width:100px;">'
                        )}
                    </div>
                </div>
            </div>
        `;
}
