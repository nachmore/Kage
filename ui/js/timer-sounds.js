/**
 * Timer notification sounds — synthesized via AudioContext.
 * Each sound is a function that plays immediately.
 */

const SOUNDS = {
    'two-tone': {
        name: 'Two-Tone Beep',
        play: () => {
            const ctx = new AudioContext();
            _tone(ctx, 800, 0, 0.25, 0.3);
            _tone(ctx, 1000, 0.3, 0.25, 0.3);
        }
    },
    'chime': {
        name: 'Chime',
        play: () => {
            const ctx = new AudioContext();
            _tone(ctx, 523, 0, 0.15, 0.25);
            _tone(ctx, 659, 0.15, 0.15, 0.25);
            _tone(ctx, 784, 0.3, 0.15, 0.25);
            _tone(ctx, 1047, 0.45, 0.3, 0.3);
        }
    },
    'alert': {
        name: 'Alert',
        play: () => {
            const ctx = new AudioContext();
            _tone(ctx, 880, 0, 0.1, 0.35);
            _tone(ctx, 880, 0.15, 0.1, 0.35);
            _tone(ctx, 880, 0.3, 0.1, 0.35);
        }
    },
    'gentle': {
        name: 'Gentle',
        play: () => {
            const ctx = new AudioContext();
            const osc = ctx.createOscillator();
            const gain = ctx.createGain();
            osc.type = 'sine';
            osc.frequency.value = 440;
            gain.gain.setValueAtTime(0, ctx.currentTime);
            gain.gain.linearRampToValueAtTime(0.2, ctx.currentTime + 0.1);
            gain.gain.linearRampToValueAtTime(0, ctx.currentTime + 0.8);
            osc.connect(gain);
            gain.connect(ctx.destination);
            osc.start();
            osc.stop(ctx.currentTime + 0.8);
        }
    },
    'bell': {
        name: 'Bell',
        play: () => {
            const ctx = new AudioContext();
            [1, 2.4, 3, 4.5].forEach((mult, i) => {
                const osc = ctx.createOscillator();
                const gain = ctx.createGain();
                osc.frequency.value = 600 * mult;
                gain.gain.setValueAtTime(0.2 / (i + 1), ctx.currentTime);
                gain.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + 1.2);
                osc.connect(gain);
                gain.connect(ctx.destination);
                osc.start();
                osc.stop(ctx.currentTime + 1.2);
            });
        }
    },
    'success': {
        name: 'Success',
        play: () => {
            const ctx = new AudioContext();
            _tone(ctx, 523, 0, 0.12, 0.25);
            _tone(ctx, 659, 0.12, 0.12, 0.25);
            _tone(ctx, 784, 0.24, 0.2, 0.3);
        }
    },
};

function _tone(ctx, freq, startTime, duration, volume) {
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.frequency.value = freq;
    gain.gain.value = volume;
    gain.gain.setValueAtTime(volume, ctx.currentTime + startTime);
    gain.gain.linearRampToValueAtTime(0, ctx.currentTime + startTime + duration);
    osc.connect(gain);
    gain.connect(ctx.destination);
    osc.start(ctx.currentTime + startTime);
    osc.stop(ctx.currentTime + startTime + duration + 0.05);
}

// --- Playback state for stop support ---
let _activeInterval = null;
let _activeAudio = null;

/**
 * Stop any currently playing sound.
 */
export function stopTimerSound() {
    if (_activeInterval) { clearInterval(_activeInterval); _activeInterval = null; }
    if (_activeAudio) { try { _activeAudio.pause(); } catch {} _activeAudio = null; }
}

/**
 * @returns {boolean} true if a sound is currently playing
 */
export function isSoundPlaying() {
    return _activeInterval !== null || _activeAudio !== null;
}

/**
 * Play a sound by ID. If 'custom', plays the file at customPath.
 * @param {string} soundId - Sound identifier
 * @param {string} customPath - Path to custom audio file (when soundId is 'custom')
 * @param {number} repeats - Number of times to play (default 3)
 * @param {function} onDone - Optional callback when all repeats finish
 */
export function playTimerSound(soundId, customPath, repeats = 3, onDone) {
    stopTimerSound();
    if (soundId === 'custom' && customPath) {
        _playCustomRepeated(customPath, repeats, onDone);
        return;
    }
    const sound = SOUNDS[soundId || 'two-tone'];
    if (sound) {
        _playSynthRepeated(sound, repeats, onDone);
    }
}

function _playSynthRepeated(sound, repeats, onDone) {
    let i = 1;
    try { sound.play(); } catch {}
    if (repeats <= 1) { if (onDone) setTimeout(onDone, 500); return; }
    _activeInterval = setInterval(() => {
        try { sound.play(); } catch {}
        i++;
        if (i >= repeats) {
            clearInterval(_activeInterval);
            _activeInterval = null;
            if (onDone) setTimeout(onDone, 500);
        }
    }, 800);
}

function _playCustomRepeated(path, repeats, onDone) {
    let i = 0;
    function playOnce() {
        if (i >= repeats || _activeAudio === null && i > 0) {
            _activeAudio = null;
            if (onDone) onDone();
            return;
        }
        try {
            _activeAudio = new Audio(path);
            _activeAudio.onended = () => { i++; setTimeout(playOnce, 300); };
            _activeAudio.play().catch(() => { _activeAudio = null; if (onDone) onDone(); });
        } catch { _activeAudio = null; if (onDone) onDone(); }
    }
    playOnce();
}

/**
 * Get the list of available built-in sounds for the settings UI.
 */
export function getAvailableSounds() {
    return Object.entries(SOUNDS).map(([id, s]) => ({ id, name: s.name }));
}
