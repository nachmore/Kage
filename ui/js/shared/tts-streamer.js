/**
 * TTS Streamer — sentence-chunked streaming TTS with audio queue and playback bar.
 *
 * Splits incoming text into sentences as it streams in, sends each sentence
 * to the Pocket TTS server for generation, and plays audio chunks sequentially.
 *
 * Usage:
 *   import { TtsStreamer } from './tts-streamer.js';
 *   const streamer = new TtsStreamer({ port: 9877, voice: 'alba', barContainer: document.body });
 *   // During streaming:
 *   streamer.feedText(accumulatedText);
 *   // When stream completes:
 *   streamer.finishText(finalText);
 *   // To stop:
 *   streamer.stop();
 */

// Sentence boundary regex — splits on . ! ? followed by space or end, but not
// inside abbreviations like "Dr." or numbers like "3.14"
const SENTENCE_RE = /(?<=[.!?])\s+(?=[A-Z\u00C0-\u024F"])/;

/**
 * Split text into sentence chunks. Returns an array of strings.
 * Tries to keep chunks at natural sentence boundaries.
 */
function splitSentences(text) {
    // First strip markdown noise
    const clean = text
        .replace(/```[\s\S]*?```/g, ' code block ')
        .replace(/`([^`]+)`/g, '$1')
        .replace(/[#*_~>\[\]()]/g, '')
        .replace(/\n+/g, '. ')
        .trim();

    if (!clean) return [];

    const parts = clean.split(SENTENCE_RE).filter(s => s.trim().length > 0);

    // Merge very short fragments into the previous chunk
    const merged = [];
    for (const part of parts) {
        if (merged.length > 0 && part.trim().length < 20) {
            merged[merged.length - 1] += ' ' + part.trim();
        } else {
            merged.push(part.trim());
        }
    }
    return merged;
}


export class TtsStreamer {
    /**
     * @param {Object} opts
     * @param {number} opts.port - Pocket TTS server port
     * @param {string} opts.voice - Voice name
     * @param {HTMLElement} opts.barContainer - Element to insert the playback bar before
     * @param {Function} [opts.onBarChange] - Called when bar is shown/hidden (for resize)
     */
    constructor({ port, voice, barContainer, onBarChange }) {
        this.port = port;
        this.voice = voice;
        this.barContainer = barContainer;
        this.onBarChange = onBarChange || (() => {});

        // Sentence tracking
        this._sentencesSent = 0;    // how many sentences we've dispatched to TTS
        this._lastSentences = [];   // sentences extracted so far
        this._finished = false;     // true once finishText() is called

        // Audio queue
        this._audioQueue = [];      // Array of { blob, url, sentence }
        this._currentAudio = null;
        this._isPlaying = false;
        this._isPaused = false;
        this._stopped = false;

        // Fetch tracking (for concurrent generation)
        this._pendingFetches = 0;

        // Playback bar
        this._bar = null;
        this._totalChunks = 0;
        this._playedChunks = 0;
    }

    /**
     * Feed accumulated text during streaming. Call on every streaming chunk.
     * Extracts new complete sentences and queues them for TTS.
     */
    feedText(accumulatedText) {
        if (this._stopped) return;

        const sentences = splitSentences(accumulatedText);

        // Find new sentences we haven't sent yet
        while (this._sentencesSent < sentences.length - 1) {
            // All sentences except the last are "complete" (the last might still be growing)
            const sentence = sentences[this._sentencesSent];
            this._dispatchSentence(sentence);
            this._sentencesSent++;
        }

        this._lastSentences = sentences;
    }

    /**
     * Called when the stream is complete. Sends the final sentence.
     */
    finishText(finalText) {
        if (this._stopped) return;
        this._finished = true;

        const sentences = splitSentences(finalText);

        // Send any remaining sentences
        while (this._sentencesSent < sentences.length) {
            const sentence = sentences[this._sentencesSent];
            this._dispatchSentence(sentence);
            this._sentencesSent++;
        }
    }

    /**
     * Send a sentence to the TTS server and queue the result.
     * Uses streaming PCM when available for lower latency.
     */
    async _dispatchSentence(sentence) {
        if (this._stopped || !sentence.trim()) return;

        this._totalChunks++;
        this._pendingFetches++;
        this._showBar();

        try {
            const resp = await fetch(`http://127.0.0.1:${this.port}/tts`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ text: sentence, voice: this.voice, stream: true }),
            });

            if (this._stopped) return;

            if (!resp.ok) {
                console.warn('[TtsStreamer] TTS request failed:', resp.status);
                return;
            }

            const contentType = resp.headers.get('Content-Type') || '';

            if (contentType.includes('octet-stream')) {
                // Streaming PCM — read chunks and build a WAV when done
                const sampleRate = parseInt(resp.headers.get('X-Sample-Rate') || '24000', 10);
                const chunks = [];
                const reader = resp.body.getReader();

                while (true) {
                    const { done, value } = await reader.read();
                    if (done || this._stopped) break;
                    chunks.push(value);
                }

                if (this._stopped) return;

                // Combine chunks into a single PCM buffer
                const totalLen = chunks.reduce((sum, c) => sum + c.byteLength, 0);
                const pcm = new Uint8Array(totalLen);
                let offset = 0;
                for (const chunk of chunks) {
                    pcm.set(new Uint8Array(chunk.buffer || chunk), offset);
                    offset += chunk.byteLength;
                }

                // Build WAV in memory
                const wavBlob = this._pcmToWav(pcm, sampleRate);
                const url = URL.createObjectURL(wavBlob);
                this._audioQueue.push({ url, sentence });
            } else {
                // Non-streaming WAV response (fallback)
                const blob = await resp.blob();
                if (this._stopped) return;
                const url = URL.createObjectURL(blob);
                this._audioQueue.push({ url, sentence });
            }

            // Start playing if not already
            if (!this._isPlaying && !this._isPaused) {
                this._playNext();
            }
        } catch (e) {
            console.warn('[TtsStreamer] TTS fetch error:', e);
        } finally {
            this._pendingFetches--;
        }
    }

    /**
     * Convert raw 16-bit PCM bytes to a WAV Blob for Audio playback.
     */
    _pcmToWav(pcmBytes, sampleRate) {
        const numChannels = 1;
        const bitsPerSample = 16;
        const byteRate = sampleRate * numChannels * bitsPerSample / 8;
        const blockAlign = numChannels * bitsPerSample / 8;
        const dataSize = pcmBytes.byteLength;
        const buffer = new ArrayBuffer(44 + dataSize);
        const view = new DataView(buffer);

        // RIFF header
        this._writeString(view, 0, 'RIFF');
        view.setUint32(4, 36 + dataSize, true);
        this._writeString(view, 8, 'WAVE');
        // fmt chunk
        this._writeString(view, 12, 'fmt ');
        view.setUint32(16, 16, true);
        view.setUint16(20, 1, true); // PCM
        view.setUint16(22, numChannels, true);
        view.setUint32(24, sampleRate, true);
        view.setUint32(28, byteRate, true);
        view.setUint16(32, blockAlign, true);
        view.setUint16(34, bitsPerSample, true);
        // data chunk
        this._writeString(view, 36, 'data');
        view.setUint32(40, dataSize, true);
        new Uint8Array(buffer, 44).set(pcmBytes);

        return new Blob([buffer], { type: 'audio/wav' });
    }

    _writeString(view, offset, str) {
        for (let i = 0; i < str.length; i++) {
            view.setUint8(offset + i, str.charCodeAt(i));
        }
    }

    /**
     * Play the next audio chunk in the queue.
     */
    _playNext() {
        if (this._stopped || this._isPaused) return;

        if (this._audioQueue.length === 0) {
            // Nothing to play right now
            this._isPlaying = false;
            // If we're done generating, hide the bar
            if (this._finished && this._pendingFetches === 0) {
                this._onPlaybackComplete();
            }
            return;
        }

        this._isPlaying = true;
        const chunk = this._audioQueue.shift();
        this._currentAudio = new Audio(chunk.url);

        this._currentAudio.onended = () => {
            URL.revokeObjectURL(chunk.url);
            this._currentAudio = null;
            this._playedChunks++;
            this._updateBar();
            this._playNext();
        };

        this._currentAudio.onerror = () => {
            URL.revokeObjectURL(chunk.url);
            this._currentAudio = null;
            this._playedChunks++;
            this._updateBar();
            this._playNext();
        };

        this._updateBar();
        this._currentAudio.play().catch(e => {
            console.warn('[TtsStreamer] Audio play error:', e);
            this._playNext();
        });
    }

    /** Pause playback. */
    pause() {
        if (this._currentAudio && this._isPlaying) {
            this._currentAudio.pause();
            this._isPaused = true;
            this._updateBarControls();
        }
    }

    /** Resume playback. */
    resume() {
        if (this._isPaused) {
            this._isPaused = false;
            if (this._currentAudio) {
                this._currentAudio.play().catch(() => {});
            } else {
                this._playNext();
            }
            this._updateBarControls();
        }
    }

    /** Toggle pause/resume. */
    togglePause() {
        if (this._isPaused) this.resume();
        else this.pause();
    }

    /** Stop all playback and clear the queue. */
    stop() {
        this._stopped = true;
        this._isPaused = false;
        this._isPlaying = false;

        if (this._currentAudio) {
            this._currentAudio.pause();
            this._currentAudio = null;
        }

        // Clean up queued audio URLs
        for (const chunk of this._audioQueue) {
            URL.revokeObjectURL(chunk.url);
        }
        this._audioQueue = [];

        this._hideBar();
    }

    /** Returns true if actively playing or has queued audio. */
    get isActive() {
        return this._isPlaying || this._audioQueue.length > 0 || this._pendingFetches > 0;
    }


    // ── Playback Bar UI ──

    _showBar() {
        if (this._bar) return;

        this._bar = document.createElement('div');
        this._bar.id = 'ttsPlaybackBar';
        this._bar.className = 'tts-bar';
        this._bar.innerHTML = `
            <div class="tts-bar-progress" id="ttsBarProgress"></div>
            <span class="tts-bar-icon">🔊</span>
            <span class="tts-bar-status" id="ttsBarStatus">Generating...</span>
            <div class="tts-bar-controls">
                <button class="timer-btn" id="ttsBarPause" title="Pause/Resume">⏸</button>
                <button class="timer-btn" id="ttsBarStop" title="Stop">⏹</button>
            </div>
        `;

        // Prevent buttons from stealing focus
        this._bar.querySelectorAll('button').forEach(btn => {
            btn.addEventListener('mousedown', e => e.preventDefault());
        });

        // Insert before the bar container (same pattern as timer bar)
        if (this.barContainer) {
            this.barContainer.parentNode.insertBefore(this._bar, this.barContainer);
        }

        // Wire up controls
        const pauseBtn = this._bar.querySelector('#ttsBarPause');
        const stopBtn = this._bar.querySelector('#ttsBarStop');
        if (pauseBtn) pauseBtn.onclick = () => this.togglePause();
        if (stopBtn) stopBtn.onclick = () => this.stop();

        this._bar.style.display = 'flex';
        this.onBarChange();
    }

    _updateBar() {
        if (!this._bar) return;

        const progress = this._bar.querySelector('#ttsBarProgress');
        const status = this._bar.querySelector('#ttsBarStatus');

        if (progress && this._totalChunks > 0) {
            const pct = Math.min(100, (this._playedChunks / this._totalChunks) * 100);
            progress.style.width = `${pct}%`;
        }

        if (status) {
            if (this._isPlaying && !this._isPaused) {
                const remaining = this._totalChunks - this._playedChunks;
                const pending = this._pendingFetches > 0 ? ` (+${this._pendingFetches} generating)` : '';
                status.textContent = `Playing ${this._playedChunks + 1}/${this._totalChunks}${pending}`;
            } else if (this._isPaused) {
                status.textContent = 'Paused';
            } else if (this._pendingFetches > 0) {
                status.textContent = 'Generating...';
            }
        }
    }

    _updateBarControls() {
        if (!this._bar) return;
        const pauseBtn = this._bar.querySelector('#ttsBarPause');
        if (pauseBtn) {
            pauseBtn.textContent = this._isPaused ? '▶' : '⏸';
        }
    }

    _hideBar() {
        if (this._bar) {
            // Delay removal so click events complete
            const bar = this._bar;
            this._bar = null;
            setTimeout(() => {
                bar.style.display = 'none';
                bar.remove();
                this.onBarChange();
            }, 50);
        }
    }

    _onPlaybackComplete() {
        // Brief delay so the user sees "complete" state
        if (!this._bar) return;
        const status = this._bar.querySelector('#ttsBarStatus');
        const progress = this._bar.querySelector('#ttsBarProgress');
        if (status) status.textContent = 'Done';
        if (progress) progress.style.width = '100%';

        setTimeout(() => {
            if (!this._stopped) this._hideBar();
        }, 2000);
    }
}
