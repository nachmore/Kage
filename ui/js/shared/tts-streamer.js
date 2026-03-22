/**
 * TTS Streamer — sentence-chunked streaming TTS with audio queue and playback bar.
 *
 * Also exports TtsPlaybackBar for reuse with browser speechSynthesis.
 *
 * Usage:
 *   import { TtsStreamer, TtsPlaybackBar } from './tts-streamer.js';
 */

// Lazy-loaded emoji name map — only fetched when TTS actually needs it
let _emojiNames = null;
let _emojiNamesLoading = false;

/** Trigger lazy load of emoji names. Call early (e.g. on TTS warmup) so data
 *  is ready by the time text needs cleaning. Non-blocking. */
export function preloadEmojiNames() {
    if (_emojiNames || _emojiNamesLoading) return;
    _emojiNamesLoading = true;
    import('../../vendor/lib/emoji-names.js').then(mod => {
        _emojiNames = mod.emojiNames;
    }).catch(() => {
        _emojiNames = {}; // Fallback: emojis will just be stripped
    });
}

// Sentence boundary regex
const SENTENCE_RE = /(?<=[.!?])\s+(?=[A-Z\u00C0-\u024F"])/;

// ─── TTS Text Preprocessing ───

/** Common symbols that TTS engines mispronounce or skip */
const SYMBOL_MAP = {
    '→': ' then ',
    '←': ' back to ',
    '↔': ' between ',
    '⇒': ' therefore ',
    '⇐': ' implied by ',
    '≥': ' greater than or equal to ',
    '≤': ' less than or equal to ',
    '≠': ' not equal to ',
    '≈': ' approximately ',
    '±': ' plus or minus ',
    '×': ' times ',
    '÷': ' divided by ',
    '•': ', ',
    '·': ', ',
    '…': '...',
    '—': ', ',
    '–': ' to ',
    '©': ' copyright ',
    '®': ' registered ',
    '™': ' trademark ',
    '°': ' degrees ',
    '✓': ' check ',
    '✗': ' cross ',
    '✔': ' check ',
    '✘': ' cross ',
    '★': ' star ',
    '☆': ' star ',
    '❤': ' heart ',
    '∞': ' infinity ',
};

/**
 * Clean text for TTS consumption:
 * - Replace common symbols with spoken equivalents
 * - Convert emojis to their spoken names (e.g. 👋 → "waving hand")
 * - Clean up leftover whitespace
 */
export function cleanForTts(text) {
    // Replace known symbols
    for (const [sym, spoken] of Object.entries(SYMBOL_MAP)) {
        text = text.replaceAll(sym, spoken);
    }
    // Replace emoji sequences with their spoken names, wrapped in commas for a natural pause.
    // Consecutive emojis are grouped (e.g. 🤣🤣🤣 → ", rolling on the floor laughing x3,")
    const emojiUnit = /(\p{Emoji_Presentation}|\p{Emoji}\uFE0F)(\u200D(\p{Emoji_Presentation}|\p{Emoji}\uFE0F))*/gu;
    // Match one or more consecutive emoji (possibly separated by whitespace)
    const emojiGroup = new RegExp(`(${emojiUnit.source})(\\s*(${emojiUnit.source}))*`, 'gu');
    text = text.replace(emojiGroup, (match) => {
        // Split the group into individual emoji
        const singles = [...match.matchAll(emojiUnit)].map(m => m[0]);
        // Count consecutive duplicates and build spoken parts
        const parts = [];
        let i = 0;
        while (i < singles.length) {
            const emoji = singles[i];
            let count = 1;
            while (i + count < singles.length && singles[i + count] === emoji) count++;
            const name = _emojiNames?.[emoji];
            if (name) {
                parts.push(count > 1 ? `${name} times ${count}` : name);
            }
            i += count;
        }
        return parts.length ? `, ${parts.join(', ')}, ` : '';
    });
    // Collapse multiple spaces/commas from removals
    text = text.replace(/\s{2,}/g, ' ').replace(/,\s*,/g, ',').trim();
    return text;
}

function splitSentences(text) {
    const clean = text
        .replace(/```[\s\S]*?```/g, ' code block ')
        .replace(/`([^`]+)`/g, '$1')
        .replace(/[#*_~>\[\]()]/g, '')
        .replace(/\n+/g, '. ')
        .trim();
    if (!clean) return [];
    // Apply TTS-specific symbol/emoji cleanup
    const ttsReady = cleanForTts(clean);
    if (!ttsReady) return [];
    const parts = ttsReady.split(SENTENCE_RE).filter(s => s.trim().length > 0);
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


// ─── Reusable Playback Bar ───

export class TtsPlaybackBar {
    /**
     * @param {HTMLElement} barContainer - Element to insert the bar before
     * @param {Function} [onBarChange] - Called when bar is shown/hidden
     * @param {Object} callbacks - { onPause, onStop }
     */
    constructor(barContainer, onBarChange, callbacks) {
        this.barContainer = barContainer;
        this.onBarChange = onBarChange || (() => {});
        this.callbacks = callbacks || {};
        this._el = null;
    }

    show() {
        if (this._el) return;
        this._el = document.createElement('div');
        this._el.id = 'ttsPlaybackBar';
        this._el.className = 'tts-bar';
        this._el.innerHTML = `
            <div class="tts-bar-progress" id="ttsBarProgress"></div>
            <span class="tts-bar-icon">🔊</span>
            <span class="tts-bar-status" id="ttsBarStatus">Speaking...</span>
            <div class="tts-bar-controls">
                <button class="timer-btn" id="ttsBarPause" title="Pause/Resume">⏸</button>
                <button class="timer-btn" id="ttsBarStop" title="Stop">⏹</button>
                <button class="timer-btn tts-settings-btn" id="ttsBarSettings" title="Speech settings">
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
                </button>
            </div>
        `;
        this._el.querySelectorAll('button').forEach(btn => {
            btn.addEventListener('mousedown', e => e.preventDefault());
        });
        if (this.barContainer) {
            this.barContainer.parentNode.insertBefore(this._el, this.barContainer);
        }
        this._el.querySelector('#ttsBarPause').onclick = () => { if (this.callbacks.onPause) this.callbacks.onPause(); };
        this._el.querySelector('#ttsBarStop').onclick = () => { if (this.callbacks.onStop) this.callbacks.onStop(); };
        this._el.querySelector('#ttsBarSettings').onclick = () => {
            if (window.__TAURI__?.core) {
                window.__TAURI__.core.invoke('open_settings_window', { section: 'speech' }).catch((e) => { console.warn('[TTS] Failed to open settings window:', e); });
            }
        };
        this._el.style.display = 'flex';
        this.onBarChange();
    }

    setStatus(text) {
        if (!this._el) return;
        const s = this._el.querySelector('#ttsBarStatus');
        if (s) s.textContent = text;
    }

    setProgress(fraction) {
        if (!this._el) return;
        const p = this._el.querySelector('#ttsBarProgress');
        if (p) p.style.width = `${Math.min(100, fraction * 100)}%`;
    }

    setPauseIcon(isPaused) {
        if (!this._el) return;
        const btn = this._el.querySelector('#ttsBarPause');
        if (btn) btn.textContent = isPaused ? '▶' : '⏸';
    }

    hide() {
        if (!this._el) return;
        const el = this._el;
        this._el = null;
        setTimeout(() => { el.style.display = 'none'; el.remove(); this.onBarChange(); }, 50);
    }

    hideAfterDelay(ms = 2000) {
        this.setStatus('Done');
        this.setProgress(1);
        setTimeout(() => this.hide(), ms);
    }

    get visible() { return !!this._el; }
}


// ─── TTS Streamer (Pocket TTS) ───

export class TtsStreamer {
    constructor({ port, voice, barContainer, onBarChange, onFinished }) {
        this.port = port;
        this.voice = voice;
        this._onFinished = onFinished || null;
        this._sentencesSent = 0;
        this._lastSentences = [];
        this._finished = false;
        this._audioQueue = [];
        this._currentAudio = null;
        this._isPlaying = false;
        this._isPaused = false;
        this._stopped = false;
        this._pendingFetches = 0;
        this._totalChunks = 0;
        this._playedChunks = 0;
        this._abortControllers = [];

        this._bar = new TtsPlaybackBar(barContainer, onBarChange, {
            onPause: () => this.togglePause(),
            onStop: () => this.stop(),
        });
    }

    feedText(accumulatedText) {
        if (this._stopped) return;
        const sentences = splitSentences(accumulatedText);
        while (this._sentencesSent < sentences.length - 1) {
            this._dispatchSentence(sentences[this._sentencesSent]);
            this._sentencesSent++;
        }
        this._lastSentences = sentences;
    }

    finishText(finalText) {
        if (this._stopped) return;
        this._finished = true;
        const sentences = splitSentences(finalText);
        while (this._sentencesSent < sentences.length) {
            this._dispatchSentence(sentences[this._sentencesSent]);
            this._sentencesSent++;
        }
    }

    async _dispatchSentence(sentence) {
        if (this._stopped || !sentence.trim()) return;
        this._totalChunks++;
        this._pendingFetches++;
        this._bar.show();
        this._updateBarStatus();

        try {
        // Retry loop — server may still be starting up on first request
        const maxRetries = 15;
        let lastError = null;
        for (let attempt = 0; attempt <= maxRetries; attempt++) {
            if (this._stopped) return;
            try {
                const controller = new AbortController();
                this._abortControllers.push(controller);
                const resp = await fetch(`http://127.0.0.1:${this.port}/tts`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ text: sentence, voice: this.voice, stream: true }),
                    signal: controller.signal,
                });
                if (this._stopped) return;
                if (!resp.ok) {
                    // 503 = model not loaded yet — retry
                    if (resp.status === 503 && attempt < maxRetries) {
                        this._bar.setStatus(`Waiting for voice model... (${attempt + 1}s)`);
                        await new Promise(r => setTimeout(r, 1000));
                        continue;
                    }
                    let errorMsg = `TTS server error (${resp.status})`;
                    try { const body = await resp.json(); errorMsg = body.error || errorMsg; } catch {}
                    console.warn('[TtsStreamer] TTS failed:', resp.status, errorMsg);
                    this._bar.setStatus(`Error: ${errorMsg}`);
                    setTimeout(() => this._bar.hideAfterDelay(3000), 0);
                    return;
                }

                const contentType = resp.headers.get('Content-Type') || '';
                if (contentType.includes('octet-stream')) {
                    const sampleRate = parseInt(resp.headers.get('X-Sample-Rate') || '24000', 10);
                    console.log(`[TtsStreamer] Streaming response, sampleRate=${sampleRate}, contentType=${contentType}`);
                    const chunks = [];
                    const reader = resp.body.getReader();
                    while (true) {
                        const { done, value } = await reader.read();
                        if (done || this._stopped) break;
                        chunks.push(value);
                        console.log(`[TtsStreamer] Received chunk: ${value.byteLength} bytes`);
                    }
                    if (this._stopped) return;
                    const totalLen = chunks.reduce((sum, c) => sum + c.byteLength, 0);
                    console.log(`[TtsStreamer] Total PCM: ${totalLen} bytes (${chunks.length} chunks)`);
                    const pcm = new Uint8Array(totalLen);
                    let offset = 0;
                    for (const chunk of chunks) { pcm.set(new Uint8Array(chunk.buffer || chunk), offset); offset += chunk.byteLength; }
                    console.log(`[TtsStreamer] First 20 bytes: ${Array.from(pcm.slice(0, 20)).map(b => b.toString(16).padStart(2, '0')).join(' ')}`);
                    const url = URL.createObjectURL(_pcmToWav(pcm, sampleRate));
                    this._audioQueue.push({ url, sentence });
                } else {
                    const blob = await resp.blob();
                    if (this._stopped) return;
                    this._audioQueue.push({ url: URL.createObjectURL(blob), sentence });
                }
                if (!this._isPlaying && !this._isPaused) this._playNext();
                return; // Success — exit retry loop
            } catch (e) {
                lastError = e;
                if (attempt < maxRetries) {
                    this._bar.setStatus(`Waiting for voice server... (${attempt + 1}s)`);
                    await new Promise(r => setTimeout(r, 1000));
                    continue;
                }
            }
        }
        // All retries exhausted
        console.warn('[TtsStreamer] TTS fetch error after retries:', lastError);
        this._bar.setStatus('Voice server connection failed');
        setTimeout(() => this._bar.hideAfterDelay(3000), 0);
        } finally {
            this._pendingFetches--;
        }
    }
    _playNext() {
        if (this._stopped || this._isPaused) return;
        if (this._audioQueue.length === 0) {
            this._isPlaying = false;
            if (this._finished && this._pendingFetches === 0) { this._bar.hideAfterDelay(); if (this._onFinished) this._onFinished(); }
            return;
        }
        this._isPlaying = true;
        const chunk = this._audioQueue.shift();
        this._currentAudio = new Audio(chunk.url);
        this._currentAudio.onended = () => { URL.revokeObjectURL(chunk.url); this._currentAudio = null; this._playedChunks++; this._updateBarStatus(); this._playNext(); };
        this._currentAudio.onerror = () => { URL.revokeObjectURL(chunk.url); this._currentAudio = null; this._playedChunks++; this._updateBarStatus(); this._playNext(); };
        this._updateBarStatus();
        this._currentAudio.play().catch(() => this._playNext());
    }

    pause() { if (this._currentAudio && this._isPlaying) { this._currentAudio.pause(); this._isPaused = true; this._updateBarStatus(); this._bar.setPauseIcon(true); } }
    resume() { if (this._isPaused) { this._isPaused = false; if (this._currentAudio) this._currentAudio.play().catch(() => {}); else this._playNext(); this._bar.setPauseIcon(false); this._updateBarStatus(); } }
    togglePause() { if (this._isPaused) this.resume(); else this.pause(); }

    stop() {
        console.log('[TtsStreamer] stop() called — playing:', this._isPlaying, 'queue:', this._audioQueue.length, 'pending:', this._pendingFetches, 'abortControllers:', this._abortControllers.length);
        this._stopped = true; this._isPaused = false; this._isPlaying = false;
        if (this._currentAudio) { this._currentAudio.pause(); this._currentAudio.src = ''; this._currentAudio = null; }
        for (const c of this._audioQueue) URL.revokeObjectURL(c.url);
        this._audioQueue = [];
        // Abort all in-flight fetch requests
        for (const ac of this._abortControllers) { try { ac.abort(); } catch {} }
        this._abortControllers = [];
        // Tell the server to cancel any ongoing generation
        fetch(`http://127.0.0.1:${this.port}/stop`, { method: 'POST' }).catch(() => {});
        this._bar.hide();
    }

    get isActive() { return this._isPlaying || this._audioQueue.length > 0 || this._pendingFetches > 0; }

    _updateBarStatus() {
        if (this._isPaused) this._bar.setStatus('Paused');
        else if (this._isPlaying) this._bar.setStatus('Speaking...');
        else if (this._pendingFetches > 0) this._bar.setStatus('Generating...');
        if (this._totalChunks > 0) this._bar.setProgress(this._playedChunks / this._totalChunks);
    }
}


// ─── Helpers ───

function _pcmToWav(pcmBytes, sampleRate) {
    const numChannels = 1, bitsPerSample = 16;
    const byteRate = sampleRate * numChannels * bitsPerSample / 8;
    const blockAlign = numChannels * bitsPerSample / 8;
    const dataSize = pcmBytes.byteLength;
    const buffer = new ArrayBuffer(44 + dataSize);
    const view = new DataView(buffer);
    _writeStr(view, 0, 'RIFF'); view.setUint32(4, 36 + dataSize, true); _writeStr(view, 8, 'WAVE');
    _writeStr(view, 12, 'fmt '); view.setUint32(16, 16, true); view.setUint16(20, 1, true);
    view.setUint16(22, numChannels, true); view.setUint32(24, sampleRate, true);
    view.setUint32(28, byteRate, true); view.setUint16(32, blockAlign, true); view.setUint16(34, bitsPerSample, true);
    _writeStr(view, 36, 'data'); view.setUint32(40, dataSize, true);
    new Uint8Array(buffer, 44).set(pcmBytes);
    return new Blob([buffer], { type: 'audio/wav' });
}

function _writeStr(view, offset, str) {
    for (let i = 0; i < str.length; i++) view.setUint8(offset + i, str.charCodeAt(i));
}
