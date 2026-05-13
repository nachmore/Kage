/**
 * Speech module — speech-to-text and text-to-speech.
 * Reusable across floating and chat windows.
 *
 * Usage:
 *   import { SpeechController } from './speech.js';
 *   this.speech = new SpeechController({ invoke, elements, onSend, onVisibilityUpdate });
 *   this.speech.setup();
 */

import { TtsStreamer, TtsPlaybackBar, cleanForTts, preloadEmojiNames } from './tts-streamer.js';
import { EchoCancelledVAD } from './echo-canceller.js';

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
        // TTS state machine: 'idle' | 'warming' | 'speaking'
        this._ttsState = 'idle';
        this._ttsServerReady = false;
        this._ttsPendingText = null;
        this._warmupBar = null;
        // Voice conversation mode — mic stays hot between exchanges
        this.voiceMode = false;
        this._vad = null;
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
            // Preload emoji name data if any TTS path is active
            if (this.pocketTtsEnabled || this.readBack) {
                preloadEmojiNames();
            }
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
            document.dispatchEvent(
                new CustomEvent('kage-show-response', {
                    detail: 'Speech recognition is not supported in this environment.',
                })
            );
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

            // Voice mode: interrupt TTS when user starts speaking
            // (only works when mic is active, which is when TTS is NOT playing)

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
                        if (this.voiceMode) {
                            // Voice mode: stop recognition (triggers onend which restarts)
                            // but keep isListening true so onend takes the restart path
                            if (this.recognition) {
                                this.recognition.stop();
                                this.recognition = null;
                            }
                        } else {
                            this.stop();
                            this.onSend(finalTranscript.trim());
                        }
                    }
                }, this.silenceTimeout);
            }
        };

        recognition.onerror = (event) => {
            console.error('Speech recognition error:', event.error);
            if (event.error === 'not-allowed') {
                document.dispatchEvent(
                    new CustomEvent('kage-show-response', {
                        detail: 'Microphone access denied. Please allow microphone access in your system settings.',
                    })
                );
                this.voiceMode = false;
                this.stop();
            } else if (
                this.voiceMode &&
                (event.error === 'no-speech' || event.error === 'aborted')
            ) {
                // Transient errors in voice mode — onend will restart
            } else {
                this.stop();
            }
        };

        recognition.onend = () => {
            clearTimeout(this._silenceTimer);
            const hadTranscript = finalTranscript.trim();

            if (this.isListening) {
                this.isListening = false;
                this._updateUI(false);
                this.onVisibilityUpdate();
                if (hadTranscript) {
                    this.usedSpeechForLastMessage = true;
                    this.onSend(finalTranscript.trim());
                }
                // Voice mode: restart mic immediately for next utterance
                if (this.voiceMode) {
                    setTimeout(() => {
                        if (this.voiceMode && !this.isListening) {
                            // Clear input for next utterance
                            if (this.elements.input) {
                                this.elements.input.value = '';
                                this.elements.input.style.height = 'auto';
                            }
                            this.start();
                        }
                    }, 300);
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
        // Pause VAD while recognition is active (avoid double-triggering)
        if (this._vad) this._vad.pause();

        try {
            recognition.start();
        } catch (e) {
            console.error('Failed to start speech recognition:', e);
            this.isListening = false;
            this._updateUI(false);
            this.onVisibilityUpdate();
            return;
        }

        // Pre-warm the TTS server so it's ready when the response arrives
        if (this.pocketTtsEnabled) {
            this._ensurePocketTtsRunning().catch(() => {});
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

    /** Exit voice conversation mode entirely. */
    stopVoiceMode() {
        this.voiceMode = false;
        if (this._vad) {
            this._vad.stop();
            this._vad = null;
        }
        this.stop();
    }

    /**
     * Restart the mic after TTS finishes (voice mode).
     */
    _scheduleVoiceModeRestart() {
        if (!this.voiceMode || this.isListening) return;
        setTimeout(() => {
            if (this.voiceMode && !this.isListening) {
                if (this.elements.input) {
                    this.elements.input.value = '';
                    this.elements.input.style.height = 'auto';
                }
                this.start();
            }
        }, 300);
    }

    /**
     * Called when TTS playback finishes. In voice mode, restarts the mic.
     */
    onTtsFinished() {
        if (this.voiceMode) {
            // Stop VAD — mic is about to start
            if (this._vad) this._vad.pause();
            this._scheduleVoiceModeRestart();
        }
    }

    /**
     * Start/resume the echo-cancelled VAD for voice mode interruption detection.
     */
    async _startVoiceModeVAD() {
        if (!this.voiceMode) return;
        if (!this._vad) {
            this._vad = new EchoCancelledVAD({
                onSpeechDetected: () => {
                    this.cancelSpeech();
                    // Brief delay for audio to fully release, then start recognition
                    setTimeout(() => {
                        if (!this.isListening && this.voiceMode) {
                            if (this.elements.input) {
                                this.elements.input.value = '';
                                this.elements.input.style.height = 'auto';
                            }
                            this.start();
                        }
                    }, 200);
                },
            });
            await this._vad.start();
        } else {
            this._vad.resume();
        }
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
        if (!this.readBack || !this.usedSpeechForLastMessage) {
            return;
        }
        this.usedSpeechForLastMessage = false;

        // If streaming already handled TTS, skip
        if (this._streamedThisResponse) {
            this._streamedThisResponse = false;
            return;
        }

        const clean = text
            .replace(/```[\s\S]*?```/g, ' code block ')
            .replace(/`([^`]+)`/g, '$1')
            .replace(/[#*_~>[\]()]/g, '')
            .replace(/\n+/g, '. ')
            .trim();

        // For Pocket TTS — use the queue system (handles warmup, dedup, replacement)
        if (this.pocketTtsEnabled && this.pocketTtsPort) {
            this._queuePocketTts(text);
            return;
        }

        // Browser TTS — cancel and restart
        this.cancelSpeech();
        this._speakWithBrowser(cleanForTts(clean));
    }

    /**
     * Queue a Pocket TTS request. Handles server warmup, deduplication, and replacement.
     * All UI updates are immediate — Rust calls are fire-and-forget.
     * NEVER removes/recreates bars — reuses or replaces in place.
     */
    _queuePocketTts(text) {
        // If warming up, just replace the pending text (latest click wins, no UI change)
        if (this._ttsState === 'warming') {
            this._ttsPendingText = text;
            return;
        }

        // If already speaking, stop the current generation (aborts fetch + calls /stop)
        if (this._ttsState === 'speaking' && this._ttsStreamer) {
            this._ttsStreamer.stop();
            this._ttsStreamer = null;
        }

        // Server is known ready — speak immediately
        if (this._ttsServerReady) {
            // Small delay if we just stopped a generation, to let server release the lock
            if (this._ttsState === 'speaking') {
                setTimeout(() => this._startPocketTtsSpeech(text), 200);
            } else {
                this._startPocketTtsSpeech(text);
            }
            return;
        }

        // Server status unknown — show warmup bar immediately (frontend only)
        this._ttsState = 'warming';
        this._ttsPendingText = text;
        this._showWarmupBar();

        // Fire-and-forget: check status then start if needed
        this._warmUpServer();
    }

    /** Show the warmup bar immediately — pure frontend, no Rust calls */
    _showWarmupBar() {
        // Clean up any existing bars first
        if (this._warmupBar) {
            this._warmupBar.hide();
            this._warmupBar = null;
        }
        if (this._ttsStreamer) {
            this._ttsStreamer.stop();
            this._ttsStreamer = null;
        }

        if (this.barContainer) {
            this._warmupBar = new TtsPlaybackBar(this.barContainer, this.onVisibilityUpdate, {
                onPause: () => {},
                onStop: () => {
                    this._ttsState = 'idle';
                    this._ttsPendingText = null;
                    if (this._warmupBar) {
                        this._warmupBar.hide();
                        this._warmupBar = null;
                    }
                },
            });
            this._warmupBar.show();
            this._warmupBar.setStatus('Starting voice server...');
        }
    }

    /** Fire-and-forget server warmup — runs async, updates state when done */
    async _warmUpServer() {
        try {
            // Fast check: hit the HTTP /status endpoint directly (instant if server is up,
            // fails fast if not). Avoids the slow Tauri pocket_tts_check_install call which spawns
            // a Python subprocess to check installation status — irrelevant here.
            try {
                const resp = await fetch(`http://127.0.0.1:${this.pocketTtsPort}/status`);
                if (resp.ok) {
                    const data = await resp.json();
                    if (data.model_loaded) {
                        if (
                            data.voices_loaded?.length > 0 &&
                            !data.voices_loaded.includes(this.pocketTtsVoice)
                        ) {
                            console.log(
                                `[Speech] Configured voice "${this.pocketTtsVoice}" not loaded, using "${data.voices_loaded[0]}" instead`
                            );
                            this.pocketTtsVoice = data.voices_loaded[0];
                        }
                        this._ttsServerReady = true;
                        this._onServerReady();
                        return;
                    }
                }
            } catch {
                /* server not running — fall through to start it */
            }

            // Start the server (fire-and-forget — this blocks in Rust until ready)
            if (this._warmupBar) this._warmupBar.setStatus('Loading voice model...');
            this.invoke('pocket_tts_start')
                .then((_result) => {
                    // Server is ready — poll the /status endpoint to confirm
                    this._pollServerReady();
                })
                .catch((e) => {
                    console.warn('[Speech] Failed to start Pocket TTS:', e);
                    this._ttsState = 'idle';
                    if (this._warmupBar) {
                        this._warmupBar.setStatus('Failed to start');
                        setTimeout(() => {
                            if (this._warmupBar) {
                                this._warmupBar.hide();
                                this._warmupBar = null;
                            }
                        }, 2000);
                    }
                });
        } catch (e) {
            console.warn('[Speech] Failed during warmup:', e);
            this._ttsState = 'idle';
            if (this._warmupBar) {
                this._warmupBar.hide();
                this._warmupBar = null;
            }
        }
    }

    /** Poll the TTS server's /status endpoint until it reports model_loaded */
    async _pollServerReady(attempts = 0) {
        if (this._ttsState !== 'warming') return; // Cancelled
        if (attempts > 30) {
            // Give up after ~30 seconds
            console.warn('[Speech] TTS server did not become ready after 30 attempts');
            this._ttsState = 'idle';
            if (this._warmupBar) {
                this._warmupBar.setStatus('Server timeout');
                setTimeout(() => {
                    if (this._warmupBar) {
                        this._warmupBar.hide();
                        this._warmupBar = null;
                    }
                }, 2000);
            }
            return;
        }

        try {
            const resp = await fetch(`http://127.0.0.1:${this.pocketTtsPort}/status`);
            if (resp.ok) {
                const data = await resp.json();
                if (data.model_loaded) {
                    // Use a voice that's already loaded if our configured voice isn't available
                    if (
                        data.voices_loaded?.length > 0 &&
                        !data.voices_loaded.includes(this.pocketTtsVoice)
                    ) {
                        console.log(
                            `[Speech] Configured voice "${this.pocketTtsVoice}" not loaded, using "${data.voices_loaded[0]}" instead`
                        );
                        this.pocketTtsVoice = data.voices_loaded[0];
                    }
                    this._ttsServerReady = true;
                    this._onServerReady();
                    return;
                }
            }
        } catch {
            /* server not ready yet */
        }

        // Retry in 1 second
        if (this._warmupBar) this._warmupBar.setStatus(`Loading voice model... (${attempts + 1}s)`);
        setTimeout(() => this._pollServerReady(attempts + 1), 1000);
    }

    /** Called when server is confirmed ready — speak the pending text */
    _onServerReady() {
        // Hide warmup bar
        if (this._warmupBar) {
            this._warmupBar.hide();
            this._warmupBar = null;
        }

        // Speak the pending text (latest click wins)
        const text = this._ttsPendingText;
        this._ttsPendingText = null;
        this._ttsState = 'idle';

        if (text) {
            this._startPocketTtsSpeech(text);
        }
    }

    /** Actually start Pocket TTS speech — server is known to be running */
    _startPocketTtsSpeech(text) {
        this._ttsState = 'speaking';
        // In voice mode, stop mic and start VAD for interruption detection
        if (this.voiceMode) {
            if (this.isListening) {
                this.stop();
            }
            this._startVoiceModeVAD();
        }
        this._ttsStreamer = new TtsStreamer({
            port: this.pocketTtsPort,
            voice: this.pocketTtsVoice,
            barContainer: this.barContainer,
            onBarChange: this.onVisibilityUpdate,
            onFinished: () => {
                this._ttsState = 'idle';
                this._ttsStreamer = null;
                this.onTtsFinished();
            },
        });
        this._ttsStreamer.finishText(text);
    }

    /**
     * Ensure the Pocket TTS server is running, starting it on demand if needed.
     * Non-blocking — used by streaming path.
     */
    async _ensurePocketTtsRunning() {
        if (!this.invoke) return;
        if (this._ttsState === 'warming') return; // Already starting
        if (this._ttsServerReady) return; // Already running
        try {
            // Fast HTTP check instead of slow Tauri invoke (avoids Python subprocess)
            const resp = await fetch(`http://127.0.0.1:${this.pocketTtsPort}/status`);
            if (resp.ok) {
                const data = await resp.json();
                if (data.model_loaded) {
                    this._ttsServerReady = true;
                    return;
                }
            }
        } catch {
            /* server not running */
        }

        try {
            this.invoke('pocket_tts_start')
                .then(() => {
                    setTimeout(() => {
                        this._ttsServerReady = true;
                    }, 1500);
                })
                .catch((e) => console.warn('[Speech] Failed to start Pocket TTS:', e));
        } catch (e) {
            console.warn('[Speech] Failed to start Pocket TTS:', e);
        }
    }

    /**
     * Feed accumulated streaming text. Call on every streaming chunk.
     * Starts TTS generation for complete sentences as they arrive.
     */
    feedStreamingText(accumulatedText) {
        if (!this.readBack || !this.usedSpeechForLastMessage) return;
        if (!this.pocketTtsEnabled || !this.pocketTtsPort) return;

        // Create streamer on first call, ensuring server is running
        if (!this._ttsStreamer) {
            // Start server on demand if needed (fire and forget — streamer will retry)
            this._ensurePocketTtsRunning().catch(() => {});
            // In voice mode, stop mic and start VAD for interruption detection
            if (this.voiceMode) {
                if (this.isListening) {
                    this.stop();
                }
                this._startVoiceModeVAD();
            }
            this._ttsStreamer = new TtsStreamer({
                port: this.pocketTtsPort,
                voice: this.pocketTtsVoice,
                barContainer: this.barContainer,
                onBarChange: this.onVisibilityUpdate,
                onFinished: () => {
                    this._ttsState = 'idle';
                    this._ttsStreamer = null;
                    this.onTtsFinished();
                },
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
            // Don't null the streamer here — it's still playing audio from its queue.
            // It will be nulled by cancelSpeech() or by the onFinished callback.
            this._streamedThisResponse = true; // flag so speakResponse skips
        }
    }

    /** Fallback: speak with browser speechSynthesis, with playback bar. */
    _speakWithBrowser(text) {
        const utterance = new SpeechSynthesisUtterance(text);
        utterance.rate = 1.0;
        utterance.pitch = 1.0;
        utterance.volume = 1.0;
        utterance.lang = navigator.language || 'en-US';

        if (this.voiceName) {
            const voice = speechSynthesis.getVoices().find((v) => v.name === this.voiceName);
            if (voice) utterance.voice = voice;
        }

        // Show playback bar
        if (this.barContainer) {
            this._browserBar = new TtsPlaybackBar(this.barContainer, this.onVisibilityUpdate, {
                onPause: () => {
                    if (speechSynthesis.paused) {
                        speechSynthesis.resume();
                        this._browserBar.setPauseIcon(false);
                        this._browserBar.setStatus('Speaking...');
                    } else {
                        speechSynthesis.pause();
                        this._browserBar.setPauseIcon(true);
                        this._browserBar.setStatus('Paused');
                    }
                },
                onStop: () => {
                    speechSynthesis.cancel();
                },
            });
            this._browserBar.show();
            this._browserBar.setStatus('Speaking...');
        }

        utterance.onend = () => {
            if (this._browserBar) {
                this._browserBar.hideAfterDelay();
                this._browserBar = null;
            }
        };
        utterance.onerror = () => {
            if (this._browserBar) {
                this._browserBar.hide();
                this._browserBar = null;
            }
        };

        speechSynthesis.speak(utterance);
    }

    /** Cancel any ongoing TTS playback. */
    cancelSpeech() {
        speechSynthesis.cancel();
        this._ttsState = 'idle';
        this._ttsPendingText = null;
        if (this._warmupBar) {
            this._warmupBar.hide();
            this._warmupBar = null;
        }
        if (this._browserBar) {
            this._browserBar.hide();
            this._browserBar = null;
        }
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
        return (
            this.isListening ||
            speechSynthesis.speaking ||
            (this._pocketTtsAudio && !this._pocketTtsAudio.paused) ||
            this._ttsStreamer?.isActive ||
            this._browserBar?.visible
        );
    }
}
