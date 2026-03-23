/**
 * Echo-cancelled voice activity detector (VAD).
 * 
 * Uses WebRTC's getUserMedia with echoCancellation to get a clean audio stream,
 * then monitors volume levels to detect real human speech even while TTS is playing.
 * 
 * This solves the problem of the microphone picking up speaker output:
 * - WebRTC's AEC removes the TTS audio from the mic signal
 * - Only genuine human speech triggers the activity callback
 * 
 * Usage:
 *   const vad = new EchoCancelledVAD({ onSpeechDetected: () => { ... } });
 *   await vad.start();
 *   vad.stop();
 */

const DEFAULT_THRESHOLD = 0.04;   // RMS threshold — higher to reject echo residual
const CHECK_INTERVAL_MS = 50;     // How often to check audio levels
const CONFIRM_FRAMES = 5;         // Consecutive frames above threshold (~250ms of real speech)

export class EchoCancelledVAD {
    /**
     * @param {Object} opts
     * @param {Function} opts.onSpeechDetected - Called when real speech is detected
     * @param {number} [opts.threshold] - RMS volume threshold (0-1, default 0.015)
     */
    constructor({ onSpeechDetected, threshold }) {
        this.onSpeechDetected = onSpeechDetected;
        this.threshold = threshold || DEFAULT_THRESHOLD;
        this._stream = null;
        this._audioCtx = null;
        this._analyser = null;
        this._checkInterval = null;
        this._consecutiveFrames = 0;
        this._active = false;
        this._paused = false;
        this._cooldownUntil = 0;
    }

    async start() {
        if (this._active) return;
        try {
            this._stream = await navigator.mediaDevices.getUserMedia({
                audio: {
                    echoCancellation: true,
                    noiseSuppression: true,
                    autoGainControl: true,
                }
            });

            this._audioCtx = new AudioContext();
            const source = this._audioCtx.createMediaStreamSource(this._stream);
            this._analyser = this._audioCtx.createAnalyser();
            this._analyser.fftSize = 512;
            source.connect(this._analyser);
            // Don't connect to destination — we don't want to play the mic audio

            this._active = true;
            this._consecutiveFrames = 0;
            this._startMonitoring();
            console.log('[VAD] Echo-cancelled voice activity detector started');
        } catch (e) {
            console.warn('[VAD] Failed to start:', e);
        }
    }

    stop() {
        this._active = false;
        this._paused = false;
        if (this._checkInterval) {
            clearInterval(this._checkInterval);
            this._checkInterval = null;
        }
        if (this._stream) {
            this._stream.getTracks().forEach(t => t.stop());
            this._stream = null;
        }
        if (this._audioCtx) {
            this._audioCtx.close().catch(() => {});
            this._audioCtx = null;
        }
        this._analyser = null;
        console.log('[VAD] Stopped');
    }

    /** Pause detection (e.g. while user is already speaking via SpeechRecognition) */
    pause() { this._paused = true; }

    /** Resume detection (e.g. when TTS starts playing) */
    resume() {
        this._paused = false;
        this._consecutiveFrames = 0;
    }

    get isActive() { return this._active; }

    _startMonitoring() {
        const dataArray = new Float32Array(this._analyser.fftSize);

        this._checkInterval = setInterval(() => {
            if (!this._active || this._paused || !this._analyser) return;
            if (Date.now() < this._cooldownUntil) return;

            this._analyser.getFloatTimeDomainData(dataArray);

            // Calculate RMS volume
            let sum = 0;
            for (let i = 0; i < dataArray.length; i++) {
                sum += dataArray[i] * dataArray[i];
            }
            const rms = Math.sqrt(sum / dataArray.length);

            if (rms > this.threshold) {
                this._consecutiveFrames++;
                if (this._consecutiveFrames >= CONFIRM_FRAMES) {
                    console.log(`[VAD] Speech confirmed (RMS: ${rms.toFixed(4)}, ${this._consecutiveFrames} frames)`);
                    this._consecutiveFrames = 0;
                    this._paused = true;
                    this._cooldownUntil = Date.now() + 3000; // 3s cooldown
                    this.onSpeechDetected();
                }
            } else {
                this._consecutiveFrames = 0;
            }
        }, CHECK_INTERVAL_MS);
    }
}
