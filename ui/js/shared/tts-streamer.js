/**
 * TTS Streamer — sentence-chunked streaming TTS with audio queue and playback bar.
 *
 * Also exports TtsPlaybackBar for reuse with browser speechSynthesis.
 *
 * Usage:
 *   import { TtsStreamer, TtsPlaybackBar } from './tts-streamer.js';
 */

import { t } from './i18n.js';

// Lazy-loaded emoji name map — only fetched when TTS actually needs it
let _emojiNames = null;
let _emojiNamesLoading = false;

/** Trigger lazy load of emoji names. Call early (e.g. on TTS warmup) so data
 *  is ready by the time text needs cleaning. Non-blocking. */
export function preloadEmojiNames() {
    if (_emojiNames || _emojiNamesLoading) return;
    _emojiNamesLoading = true;
    import('../../vendor/lib/emoji-names.js')
        .then((mod) => {
            _emojiNames = mod.emojiNames;
        })
        .catch(() => {
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

// Hoisted so cleanForTts (run on every streaming commit) doesn't rebuild them.
// Only used via replace()/matchAll(), which reset/copy lastIndex.
const EMOJI_UNIT_RE =
    /(\p{Emoji_Presentation}|\p{Emoji}\uFE0F)(\u200D(\p{Emoji_Presentation}|\p{Emoji}\uFE0F))*/gu;
// Match one or more consecutive emoji (possibly separated by whitespace)
const EMOJI_GROUP_RE = new RegExp(
    `(${EMOJI_UNIT_RE.source})(\\s*(${EMOJI_UNIT_RE.source}))*`,
    'gu'
);

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
    text = text.replace(EMOJI_GROUP_RE, (match) => {
        // Split the group into individual emoji
        const singles = [...match.matchAll(EMOJI_UNIT_RE)].map((m) => m[0]);
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
    text = text
        .replace(/\s{2,}/g, ' ')
        .replace(/,\s*,/g, ',')
        .trim();
    return text;
}

function splitSentences(text) {
    const clean = text
        .replace(/```[\s\S]*?```/g, ' code block ')
        .replace(/`([^`]+)`/g, '$1')
        .replace(/[#*_~>[\]()]/g, '')
        .replace(/\n+/g, '. ')
        .trim();
    if (!clean) return [];
    // Apply TTS-specific symbol/emoji cleanup
    const ttsReady = cleanForTts(clean);
    if (!ttsReady) return [];
    const parts = ttsReady.split(SENTENCE_RE).filter((s) => s.trim().length > 0);
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

// Raw-text tokens that matter for choosing a safe streaming commit point.
const COMMIT_SCAN_RE = /```|`|\n|[.!?]\s+(?=[A-Z\u00C0-\u024F"])/g;
// Don't commit tiny fragments on their own; they'd become choppy one-word requests.
const MIN_COMMIT_CHARS = 20;

/**
 * Raw-text offset up to which `text` holds only complete sentences that can be
 * spoken now: the last newline / sentence boundary that is not inside an open
 * ``` fence or inline code span. 0 if there is none yet.
 */
function rawCommitEnd(text) {
    let end = 0;
    let inFence = false;
    let inCode = false;
    for (const m of text.matchAll(COMMIT_SCAN_RE)) {
        const tok = m[0];
        if (tok === '```') {
            inFence = !inFence;
            continue;
        }
        if (inFence) continue;
        if (tok === '`') {
            inCode = !inCode;
            continue;
        }
        // Inline code doesn't span lines in practice; a stray backtick must
        // not block committing for the rest of the reply. Sentence-boundary
        // tokens swallow trailing newlines ('.\n\n'), so check for any '\n'.
        if (tok.includes('\n')) inCode = false;
        if (inCode) continue;
        end = m.index + tok.length;
    }
    return end;
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
                <button class="extension-bar-btn" id="ttsBarPause" title="${t('shared.tts.bar.pause')}">⏸</button>
                <button class="extension-bar-btn" id="ttsBarStop" title="${t('shared.tts.bar.stop')}">⏹</button>
                <button class="extension-bar-btn tts-settings-btn" id="ttsBarSettings" title="${t('shared.tts.bar.settings')}">
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
                </button>
            </div>
        `;
        this._el.querySelectorAll('button').forEach((btn) => {
            btn.addEventListener('mousedown', (e) => e.preventDefault());
        });
        if (this.barContainer) {
            this.barContainer.parentNode.insertBefore(this._el, this.barContainer);
        }
        this._el.querySelector('#ttsBarPause').onclick = () => {
            if (this.callbacks.onPause) this.callbacks.onPause();
        };
        this._el.querySelector('#ttsBarStop').onclick = () => {
            if (this.callbacks.onStop) this.callbacks.onStop();
        };
        this._el.querySelector('#ttsBarSettings').onclick = () => {
            if (window.__TAURI__?.core) {
                window.__TAURI__.core
                    .invoke('open_settings_window', { section: 'speech' })
                    .catch((e) => {
                        console.warn('[TTS] Failed to open settings window:', e);
                    });
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
        setTimeout(() => {
            el.style.display = 'none';
            el.remove();
            this.onBarChange();
        }, 50);
    }

    hideAfterDelay(ms = 2000) {
        this.setStatus('Done');
        this.setProgress(1);
        setTimeout(() => this.hide(), ms);
    }

    get visible() {
        return !!this._el;
    }
}

// ─── TTS Streamer (Pocket TTS) ───

export class TtsStreamer {
    constructor({ port, voice, barContainer, onBarChange, onFinished, onUserStop }) {
        this.port = port;
        this.voice = voice;
        this._onFinished = onFinished || null;
        // Fired when the user presses Stop on the playback bar. stop() itself
        // never reports completion (callers that stop programmatically already
        // know), but the owner must hear about a bar-initiated stop or it stays
        // in its 'speaking' state with voice mode's mic never resuming.
        this._onUserStop = onUserStop || null;
        // Raw-text offset of the prefix already split and enqueued. Only text
        // past it is processed, so sent sentences are never re-cleaned or
        // re-indexed when later text (e.g. a closing ``` fence) changes the split.
        this._rawOffset = 0;
        // The raw text up to _rawOffset, used to detect the caller resetting
        // its accumulator mid-turn (e.g. after an extension tool call) so the
        // new text is read from its start instead of from a stale offset.
        this._committedPrefix = '';
        this._finished = false;
        // Latched once onFinished has fired so no late callback (a duplicate
        // audio error, a trailing failed fetch) can report completion twice.
        this._completed = false;
        this._audioQueue = [];
        this._currentAudio = null;
        this._isPlaying = false;
        this._isPaused = false;
        this._stopped = false;
        this._pendingFetches = 0;
        this._totalChunks = 0;
        this._playedChunks = 0;
        this._abortControllers = [];
        // Sequential dispatch queue — ensures sentences are fetched in order
        this._dispatchQueue = [];
        this._dispatching = false;

        this._bar = new TtsPlaybackBar(barContainer, onBarChange, {
            onPause: () => this.togglePause(),
            onStop: () => {
                this.stop();
                this._onUserStop?.();
            },
        });
    }

    _syncOffset(text) {
        if (!text.startsWith(this._committedPrefix)) {
            this._rawOffset = 0;
            this._committedPrefix = '';
        }
    }

    feedText(accumulatedText) {
        if (this._stopped) return;
        this._syncOffset(accumulatedText);
        const tail = accumulatedText.slice(this._rawOffset);
        const end = rawCommitEnd(tail);
        if (end === 0 || tail.slice(0, end).trim().length < MIN_COMMIT_CHARS) return;
        this._rawOffset += end;
        this._committedPrefix = accumulatedText.slice(0, this._rawOffset);
        for (const sentence of splitSentences(tail.slice(0, end))) {
            this._enqueueSentence(sentence);
        }
    }

    finishText(finalText) {
        if (this._stopped) return;
        this._finished = true;
        this._syncOffset(finalText);
        const tail = finalText.slice(this._rawOffset);
        this._rawOffset = finalText.length;
        this._committedPrefix = finalText;
        for (const sentence of splitSentences(tail)) {
            this._enqueueSentence(sentence);
        }
        // Everything may already have been committed (and played) during
        // streaming; kick the queue so the finished path still runs.
        if (!this._isPlaying && !this._dispatching) this._playNext();
    }

    _enqueueSentence(sentence) {
        if (this._stopped || !sentence.trim()) return;
        this._dispatchQueue.push(sentence);
        this._processQueue();
    }

    async _processQueue() {
        if (this._dispatching || this._stopped) return;
        this._dispatching = true;
        while (this._dispatchQueue.length > 0 && !this._stopped) {
            const sentence = this._dispatchQueue.shift();
            await this._dispatchSentence(sentence);
        }
        this._dispatching = false;
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
                            await new Promise((r) => setTimeout(r, 1000));
                            continue;
                        }
                        let errorMsg = `TTS server error (${resp.status})`;
                        try {
                            const body = await resp.json();
                            errorMsg = body.error || errorMsg;
                        } catch {}
                        console.warn('[TtsStreamer] TTS failed:', resp.status, errorMsg);
                        this._bar.setStatus(`Error: ${errorMsg}`);
                        setTimeout(() => this._bar.hideAfterDelay(3000), 0);
                        return;
                    }

                    const contentType = resp.headers.get('Content-Type') || '';
                    if (contentType.includes('octet-stream')) {
                        const sampleRate = parseInt(
                            resp.headers.get('X-Sample-Rate') || '24000',
                            10
                        );
                        const chunks = [];
                        const reader = resp.body.getReader();
                        while (true) {
                            const { done, value } = await reader.read();
                            if (done || this._stopped) break;
                            chunks.push(value);
                        }
                        if (this._stopped) return;
                        const totalLen = chunks.reduce((sum, c) => sum + c.byteLength, 0);
                        const pcm = new Uint8Array(totalLen);
                        let offset = 0;
                        for (const chunk of chunks) {
                            pcm.set(new Uint8Array(chunk.buffer || chunk), offset);
                            offset += chunk.byteLength;
                        }
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
                    // stop() aborts in-flight fetches; don't sit out a retry
                    // delay for a cancelled request.
                    if (this._stopped) return;
                    lastError = e;
                    if (attempt < maxRetries) {
                        this._bar.setStatus(`Waiting for voice server... (${attempt + 1}s)`);
                        await new Promise((r) => setTimeout(r, 1000));
                    }
                }
            }
            // All retries exhausted
            console.warn('[TtsStreamer] TTS fetch error after retries:', lastError);
            this._bar.setStatus('Voice server connection failed');
            setTimeout(() => this._bar.hideAfterDelay(3000), 0);
        } finally {
            this._pendingFetches--;
            // A failed fetch queues no audio, so nothing else would call
            // _playNext. If it was the last sentence and earlier audio has
            // already finished, completion would never fire (speaking state
            // stuck, voice-mode mic never resumes). Kick the queue so the
            // failed chunk is skipped; a success already started playback.
            if (!this._stopped && !this._isPlaying && !this._isPaused) this._playNext();
        }
    }
    _playNext() {
        if (this._stopped || this._isPaused) return;
        if (this._audioQueue.length === 0) {
            this._isPlaying = false;
            // _dispatchQueue: a failed fetch kicks the queue from inside the
            // dispatch loop, before the next queued sentence is counted in
            // _pendingFetches.
            if (
                this._finished &&
                this._pendingFetches === 0 &&
                this._dispatchQueue.length === 0 &&
                !this._completed
            ) {
                this._completed = true;
                this._bar.hideAfterDelay();
                if (this._onFinished) this._onFinished();
            }
            return;
        }
        this._isPlaying = true;
        const chunk = this._audioQueue.shift();
        const audio = new Audio(chunk.url);
        this._currentAudio = audio;
        // A decode failure can surface as both an `error` event and a
        // rejected play(); advance at most once per chunk so the following
        // chunk isn't skipped (or two played at once).
        let settled = false;
        const advance = () => {
            if (settled) return;
            settled = true;
            URL.revokeObjectURL(chunk.url);
            if (this._currentAudio === audio) this._currentAudio = null;
            if (this._stopped) return;
            this._playedChunks++;
            this._updateBarStatus();
            this._playNext();
        };
        audio.onended = advance;
        audio.onerror = advance;
        this._updateBarStatus();
        audio.play().catch((e) => {
            // AbortError = play() interrupted by pause()/stop(); the chunk
            // is still current and resume() replays it, so don't skip it.
            if (e?.name !== 'AbortError') advance();
        });
    }

    pause() {
        if (this._currentAudio && this._isPlaying) {
            this._currentAudio.pause();
            this._isPaused = true;
            this._updateBarStatus();
            this._bar.setPauseIcon(true);
        }
    }
    resume() {
        if (this._isPaused) {
            this._isPaused = false;
            if (this._currentAudio) this._currentAudio.play().catch(() => {});
            else this._playNext();
            this._bar.setPauseIcon(false);
            this._updateBarStatus();
        }
    }
    togglePause() {
        if (this._isPaused) this.resume();
        else this.pause();
    }

    stop() {
        this._stopped = true;
        this._isPaused = false;
        this._isPlaying = false;
        if (this._currentAudio) {
            this._currentAudio.pause();
            this._currentAudio.src = '';
            this._currentAudio = null;
        }
        for (const c of this._audioQueue) URL.revokeObjectURL(c.url);
        this._audioQueue = [];
        // Abort all in-flight fetch requests
        for (const ac of this._abortControllers) {
            try {
                ac.abort();
            } catch {}
        }
        this._abortControllers = [];
        // Tell the server to cancel any ongoing generation
        fetch(`http://127.0.0.1:${this.port}/stop`, { method: 'POST' }).catch(() => {});
        this._bar.hide();
    }

    get isActive() {
        return this._isPlaying || this._audioQueue.length > 0 || this._pendingFetches > 0;
    }

    _updateBarStatus() {
        if (this._isPaused) this._bar.setStatus('Paused');
        else if (this._isPlaying) this._bar.setStatus('Speaking...');
        else if (this._pendingFetches > 0) this._bar.setStatus('Generating...');
        if (this._totalChunks > 0) this._bar.setProgress(this._playedChunks / this._totalChunks);
    }
}

// ─── Helpers ───

function _pcmToWav(pcmBytes, sampleRate) {
    const numChannels = 1,
        bitsPerSample = 16;
    const byteRate = (sampleRate * numChannels * bitsPerSample) / 8;
    const blockAlign = (numChannels * bitsPerSample) / 8;
    const dataSize = pcmBytes.byteLength;
    const buffer = new ArrayBuffer(44 + dataSize);
    const view = new DataView(buffer);
    _writeStr(view, 0, 'RIFF');
    view.setUint32(4, 36 + dataSize, true);
    _writeStr(view, 8, 'WAVE');
    _writeStr(view, 12, 'fmt ');
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true);
    view.setUint16(22, numChannels, true);
    view.setUint32(24, sampleRate, true);
    view.setUint32(28, byteRate, true);
    view.setUint16(32, blockAlign, true);
    view.setUint16(34, bitsPerSample, true);
    _writeStr(view, 36, 'data');
    view.setUint32(40, dataSize, true);
    new Uint8Array(buffer, 44).set(pcmBytes);
    return new Blob([buffer], { type: 'audio/wav' });
}

function _writeStr(view, offset, str) {
    for (let i = 0; i < str.length; i++) view.setUint8(offset + i, str.charCodeAt(i));
}
