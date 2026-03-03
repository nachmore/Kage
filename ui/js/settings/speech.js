/**
 * Speech Settings Module
 */
class SpeechSettingsModule extends SettingsModule {
    constructor() {
        super('speech', 'Speech', '🎙️');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                ${this.createCheckboxRow(
                    'Show Speech Button',
                    'Display a microphone button in the assistant launcher for voice input using speech-to-text.',
                    'showSpeechButton',
                    false
                )}

                <div id="speechReadBackRow">
                ${this.createCheckboxRow(
                    'Read Back Agent Responses',
                    'Use text-to-speech to read agent responses aloud when voice input was used.',
                    'speechReadBack',
                    false
                )}
                </div>

                <div id="speechVoiceRow">
                ${this.createControlRow(
                    'Voice',
                    'Select the voice used for reading back responses.',
                    `<select class="setting-select" id="speechVoice">
                        <option value="">System Default</option>
                    </select>`
                )}
                </div>

                <div id="speechSilenceRow">
                ${this.createControlRow(
                    'Auto-Submit Silence Delay',
                    'Automatically send the message after this many seconds of silence. Set to 0 to disable (you\'ll need to press Enter or click the mic again).',
                    `<div class="range-container">
                        <input type="range" class="range-slider" id="speechSilenceTimeout" min="0" max="5" step="0.5" value="2">
                        <span class="range-value" id="speechSilenceValue">2.0s</span>
                    </div>`
                )}
                </div>
            </div>
        `;
    }

    load(config) {
        if (!config.ui) return;
        const showSpeech = document.getElementById('showSpeechButton');
        const readBack = document.getElementById('speechReadBack');
        const silence = document.getElementById('speechSilenceTimeout');
        const silenceValue = document.getElementById('speechSilenceValue');
        if (showSpeech) showSpeech.checked = config.ui.show_speech_button === true;
        if (readBack) readBack.checked = config.ui.speech_read_back === true;
        if (silence) {
            silence.value = config.ui.speech_silence_timeout ?? 2.0;
            if (silenceValue) silenceValue.textContent = (config.ui.speech_silence_timeout ?? 2.0).toFixed(1) + 's';
        }
        this._savedVoice = config.ui.speech_voice || '';
        this.populateVoices();
        this.toggleReadBack();
    }

    save(config) {
        config.ui = config.ui || {};
        config.ui.show_speech_button = document.getElementById('showSpeechButton')?.checked ?? false;
        config.ui.speech_read_back = document.getElementById('speechReadBack')?.checked ?? false;
        config.ui.speech_silence_timeout = parseFloat(document.getElementById('speechSilenceTimeout')?.value ?? '2');
        config.ui.speech_voice = document.getElementById('speechVoice')?.value || null;
    }

    initialize() {
        document.getElementById('showSpeechButton')?.addEventListener('change', () => this.toggleReadBack());
        const silence = document.getElementById('speechSilenceTimeout');
        const silenceValue = document.getElementById('speechSilenceValue');
        if (silence && silenceValue) {
            silence.addEventListener('input', (e) => {
                const v = parseFloat(e.target.value);
                silenceValue.textContent = v === 0 ? 'Off' : v.toFixed(1) + 's';
            });
        }
        // Voices may load asynchronously
        if (speechSynthesis.onvoiceschanged !== undefined) {
            speechSynthesis.onvoiceschanged = () => this.populateVoices();
        }
    }

    populateVoices() {
        const select = document.getElementById('speechVoice');
        if (!select) return;
        const voices = speechSynthesis.getVoices();
        // Keep the default option, clear the rest
        select.innerHTML = '<option value="">System Default</option>';
        for (const voice of voices) {
            const opt = document.createElement('option');
            opt.value = voice.name;
            opt.textContent = `${voice.name} (${voice.lang})`;
            select.appendChild(opt);
        }
        if (this._savedVoice) select.value = this._savedVoice;
    }

    toggleReadBack() {
        const showSpeech = document.getElementById('showSpeechButton')?.checked;
        const row = document.getElementById('speechReadBackRow');
        if (row) row.style.opacity = showSpeech ? '1' : '0.4';
        const readBack = document.getElementById('speechReadBack');
        if (readBack) readBack.disabled = !showSpeech;
        const voiceRow = document.getElementById('speechVoiceRow');
        if (voiceRow) voiceRow.style.opacity = showSpeech ? '1' : '0.4';
        const voice = document.getElementById('speechVoice');
        if (voice) voice.disabled = !showSpeech;
        const silenceRow = document.getElementById('speechSilenceRow');
        if (silenceRow) silenceRow.style.opacity = showSpeech ? '1' : '0.4';
        const silence = document.getElementById('speechSilenceTimeout');
        if (silence) silence.disabled = !showSpeech;
    }

    validate() {
        return { valid: true };
    }
}
