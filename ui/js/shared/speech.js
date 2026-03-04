/**
 * Speech module — speech-to-text and text-to-speech.
 * Reusable across floating and chat windows.
 *
 * Usage:
 *   import { SpeechController } from './speech.js';
 *   this.speech = new SpeechController({ invoke, elements, onSend, onVisibilityUpdate });
 *   this.speech.setup();
 */

import { TtsStreamer } from './tts-streamer.js';

export class SpeechController {
    /**
     * @param {Object} opts
     * @param {Function} opts.invoke - Tauri invoke function
     * @param {Object} opts.elements - { input, speechBtn, speechWave } DOM refs
     * @param {Function} opts.onSend - called with (text) to send a message
     * @param {Function} [opts.onVisibilityUpdate] - called when speech state changes (for datetime etc.)
     * @param {HTMLElement} [opts.barContainer] - Element to insert TTS playback bar before
     */
    constructor({ invoke, elements, onSend, onVisibilityUpdate, barContainer }) {
        this.invoke = invoke;
        this.elements = elements;
        this.onSend = onSend;
        this.onVisibilityUpdate = onVisibilityUpdate || (() => {});
        this.barContainer = barContainer || null;

        this.recognition = null;
        this.isListening = false;
        this.readBack = false;
        this.silenceTimeout = 2000;
        this.voiceName = '';
        this.usedSpeechForLastMessage = false;
        this._silenceTimer = null;
        // Pocket TTS state
        this.pocketTtsEnabled = false;
        this.pocketTtsPort = 9877;
        this.pocketTtsVoice = 'alba';
        this._pocketTtsAudio = null;
        this._ttsStreamer = null;
        this._streamedThisResponse = false;
    }

    setup() {
        if (this.elements.speechBtn) {
            this.elements.speechBtn.addEventListener('click', () => this.toggle());
        }
        this.updateVisibility();
    }

    async updateVisibility() {
        try {
            const config = await this.invoke('get_config');
            const show = config.ui?.show_speech_button === true;
            this.readBack = config.ui?.speech_read_back === true;
            this.silenceTimeout = (config.ui?.speech_silence_timeout ?? 2.0) * 1000;
            this.voiceName = config.ui?.speech_voice || '';
            // Pocket TTS config
            this.pocketTtsEnabled = config.pocket_tts?.enabled === true;
            this.pocketTtsPort = config.pocket_tts?.port || 9877;
            this.pocketTtsVoice = config.pocket_tts?.voice || 'alba';
            if (this.elements.speechBtn) {
                this.elements.speechBtn.style.display = show ? '' : 'none';
                this.elements.speechBtn.dataset.configVisible = show ? 'true' : 'false';
            }
            this.onVisibilityUpdate();
        } catch (e) {
            console.warn('[Speech] updateVisibility failed:', e);
        }
    }

    toggle() {
        if (this.isListening) {
            this.stop();
        } else {
            this.start();
        }
    }

    start() {
        const SpeechRecognition = window.SpeechRecognition || window.webkitSpeechRecognition;
        if (!SpeechRecognition) {
            document.dispatchEvent(new CustomEvent('kiro-show-response', {
                detail: 'Speech recognition is not supported in this environment.'
            }));
            return;
        }

        const recognition = new SpeechRecognition();
        recognition.continuous = true;
        recognition.interimResults = true;
        recognition.lang = navigator.language || 'en-US';

        let finalTranscript = '';

        recognition.onresult = (event) => {
            let interimTranscript = '';
            for (let i = event.resultIndex; i < event.results.length; i++) {
                if (event.results[i].isFinal) {
                    finalTranscript += event.results[i][0].transcript;
                } else {
                    interimTranscript += event.results[i][0].transcript;
                }
            }
            const input = this.elements.input;
            if (input) {
                input.value = finalTranscript + interimTranscript;
                input.style.height = 'auto';
                input.style.height = input.scrollHeight + 'px';
            }
            this.onVisibilityUpdate();

            // Reset silence timer — auto-submit after configured silence period
            clearTimeout(this._silenceTimer);
            if (this.silenceTimeout > 0 && finalTranscript.trim()) {
                this._silenceTimer = setTimeout(() => {
                    if (this.isListening && finalTranscript.trim()) {
                        this.usedSpeechForLastMessage = true;
                        this.stop();
                        this.onSend(finalTranscript.trim());
                    }
                }, this.silenceTimeout);
            }
        };

        recognition.onerror = (event) => {
            console.error('Speech recognition error:', event.error);
            if (event.error === 'not-allowed') {
                document.dispatchEvent(new CustomEvent('kiro-show-response', {
                    detail: 'Microphone access denied. Please allow microphone access in your system settings.'
                }));
            }
            this.stop();
        };

        recognition.onend = () => {
            clearTimeout(this._silenceTimer);
            if (this.isListening) {
                this.isListening = false;
                this._updateUI(false);
                this.onVisibilityUpdate();
                if (finalTranscript.trim()) {
                    this.usedSpeechForLastMessage = true;
                    this.onSend(finalTranscript.trim());
                }
            } else {
                this._updateUI(false);
                this.onVisibilityUpdate();
                if (this.elements.input?.value.trim()) {
                    this.usedSpeechForLastMessage = true;
                }
            }
        };

        this.recognition = recognition;
        this.isListening = true;
        this._updateUI(true);
        this.onVisibilityUpdate();

        try {
            recognition.start();
        } catch (e) {
            console.error('Failed to start speech recognition:', e);
            this.isListening = false;
            this._updateUI(false);
            this.onVisibilityUpdate();
        }
    }

    stop() {
        clearTimeout(this._silenceTimer);
        if (this.recognition) {
            this.isListening = false;
            this.recognition.stop();
            this.recognition = null;
        }
        this._updateUI(false);
        this.onVisibilityUpdate();
    }

    _updateUI(listening) {
        if (this.elements.speechBtn) {
            this.elements.speechBtn.classList.toggle('listening', listening);
            this.elements.speechBtn.title = listening ? 'Stop listening' : 'Voice input';
        }
        if (this.elements.speechWave) {
            this.elements.speechWave.style.display = listening ? 'flex' : 'none';
        }
    }

    /** Call after a response completes to read it back if speech was used. */
    speakResponse(text) {
        if (!this.readBack || !this.usedSpeechForLastMessage) return;
        this.usedSpeechForLastMessage = false;

        // If streaming already handled TTS, skip
        if (this._streamedThisResponse) {
            this._streamedThisResponse = false;
            return;
        }

        speechSynthesis.cancel();

        const clean = text.replace(/```[\s\S]*?```/g, ' code block ')
            .replace(/`([^`]+)`/g, '$1')
            .replace(/[#*_~>\[\]()]/g, '')
            .replace(/\n+/g, '. ')
            .trim();

        if (!clean) return;

        // For non-streaming (batch) responses, use Pocket TTS if enabled
        if (this.pocketTtsEnabled && this.pocketTtsPort) {
            const streamer = new TtsStreamer({
                port: this.pocketTtsPort,
                voice: this.pocketTtsVoice,
                barContainer: this.barContainer,
                onBarChange: this.onVisibilityUpdate,
            });
            streamer.finishText(text);
            return;
        }

        this._speakWithBrowser(clean);
    }

    /**
     * Feed accumulated streaming text. Call on every streaming chunk.
     * Starts TTS generation for complete sentences as they arrive.
     */
    feedStreamingText(accumulatedText) {
        if (!this.readBack || !this.usedSpeechForLastMessage) return;
        if (!this.pocketTtsEnabled || !this.pocketTtsPort) return;

        // Create streamer on first call
        if (!this._ttsStreamer) {
            this._ttsStreamer = new TtsStreamer({
                port: this.pocketTtsPort,
                voice: this.pocketTtsVoice,
                barContainer: this.barContainer,
                onBarChange: this.onVisibilityUpdate,
            });
        }

        this._ttsStreamer.feedText(accumulatedText);
    }

    /**
     * Called when streaming is complete. Sends the final sentence chunk.
     */
    finishStreamingText(finalText) {
        if (this._ttsStreamer) {
            this._ttsStreamer.finishText(finalText);
            this._ttsStreamer = null;
            this._streamedThisResponse = true; // flag so speakResponse skips
        }
    }

    /** Fallback: speak with browser speechSynthesis. */
    _speakWithBrowser(text) {
        const utterance = new SpeechSynthesisUtterance(text);
        utterance.rate = 1.0;
        utterance.pitch = 1.0;
        utterance.volume = 1.0;
        utterance.lang = navigator.language || 'en-US';

        if (this.voiceName) {
            const voice = speechSynthesis.getVoices().find(v => v.name === this.voiceName);
            if (voice) utterance.voice = voice;
        }

        speechSynthesis.speak(utterance);
    }

    /** Cancel any ongoing TTS playback. */
    cancelSpeech() {
        speechSynthesis.cancel();
        if (this._pocketTtsAudio) {
            this._pocketTtsAudio.pause();
            this._pocketTtsAudio = null;
        }
        if (this._ttsStreamer) {
            this._ttsStreamer.stop();
            this._ttsStreamer = null;
        }
    }

    /** Returns true if speech or TTS is active (for Escape key handling). */
    get isActive() {
        return this.isListening || speechSynthesis.speaking
            || (this._pocketTtsAudio && !this._pocketTtsAudio.paused)
            || (this._ttsStreamer && this._ttsStreamer.isActive);
    }
}
