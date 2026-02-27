/**
 * Timer & Stopwatch for the floating window.
 * Supports one timer + one stopwatch running simultaneously.
 * Triggers: "timer 5m", "timer 1h30m", "timer 5:00", "timer 90s", "stopwatch"/"sw"
 */

// --- Dual state: one timer slot + one stopwatch slot ---
const _slots = {
    timer: null,     // { running, startTime, elapsed, duration, pausedAt, intervalId, onTick, onComplete }
    stopwatch: null, // { running, startTime, elapsed, pausedAt, intervalId, onTick }
};

function _newSlot() {
    return { running: false, startTime: 0, elapsed: 0, duration: 0, pausedAt: 0, intervalId: null, onTick: null, onComplete: null };
}

// --- Parsing ---

export function parseTimerCommand(input) {
    const trimmed = input.trim().toLowerCase();
    // Prefix match for "stopwatch" / "sw"
    if ('stopwatch'.startsWith(trimmed) && trimmed.length >= 2) return { type: 'stopwatch' };
    if (trimmed === 'sw') return { type: 'stopwatch' };
    // Prefix match for "timer" (no duration yet)
    if ('timer'.startsWith(trimmed) && trimmed.length >= 3 && trimmed !== 'timer') return { type: 'hint' };
    const timerMatch = trimmed.match(/^timer\s+(.+)$/);
    if (!timerMatch) {
        if (trimmed === 'timer') return { type: 'hint' };
        return null;
    }
    const ms = parseDuration(timerMatch[1].trim());
    if (ms && ms > 0) return { type: 'timer', durationMs: ms };
    return null;
}

function parseDuration(str) {
    const colonMatch = str.match(/^(\d+):(\d{2})(?::(\d{2}))?$/);
    if (colonMatch) {
        if (colonMatch[3] !== undefined) return (parseInt(colonMatch[1]) * 3600 + parseInt(colonMatch[2]) * 60 + parseInt(colonMatch[3])) * 1000;
        return (parseInt(colonMatch[1]) * 60 + parseInt(colonMatch[2])) * 1000;
    }
    let totalMs = 0;
    const hMatch = str.match(/(\d+)\s*h/);
    const mMatch = str.match(/(\d+)\s*m(?!s)/);
    const sMatch = str.match(/(\d+)\s*s/);
    if (hMatch) totalMs += parseInt(hMatch[1]) * 3600000;
    if (mMatch) totalMs += parseInt(mMatch[1]) * 60000;
    if (sMatch) totalMs += parseInt(sMatch[1]) * 1000;
    if (totalMs === 0 && /^\d+$/.test(str)) totalMs = parseInt(str) * 60000;
    return totalMs || null;
}

// --- Engine ---

export function startTimer(durationMs, onTick, onComplete) {
    stopSlot('timer');
    const s = _newSlot();
    s.duration = durationMs;
    s.running = true;
    s.startTime = Date.now();
    s.onTick = onTick;
    s.onComplete = onComplete;
    s.intervalId = setInterval(() => _tick('timer'), 100);
    _slots.timer = s;
    _tick('timer');
}

export function startStopwatch(onTick) {
    stopSlot('stopwatch');
    const s = _newSlot();
    s.running = true;
    s.startTime = Date.now();
    s.onTick = onTick;
    s.intervalId = setInterval(() => _tick('stopwatch'), 100);
    _slots.stopwatch = s;
    _tick('stopwatch');
}

export function pauseResumeSlot(slotName) {
    const s = _slots[slotName];
    if (!s) return;
    if (s.running) {
        s.running = false;
        s.pausedAt = s.elapsed;
        if (s.intervalId) { clearInterval(s.intervalId); s.intervalId = null; }
    } else {
        s.running = true;
        s.startTime = Date.now();
        s.intervalId = setInterval(() => _tick(slotName), 100);
        _tick(slotName);
    }
}

export function stopSlot(slotName) {
    const s = _slots[slotName];
    if (!s) return;
    if (s.intervalId) { clearInterval(s.intervalId); s.intervalId = null; }
    _slots[slotName] = null;
}

export function addTimeToTimer(ms) {
    const s = _slots.timer;
    if (s) { s.duration += ms; _tick('timer'); }
}

export function getSlotState(slotName) {
    const s = _slots[slotName];
    if (!s) return { active: false, running: false };
    return { active: true, running: s.running };
}

function _tick(slotName) {
    const s = _slots[slotName];
    if (!s) return;
    if (s.running) s.elapsed = s.pausedAt + (Date.now() - s.startTime);

    if (slotName === 'timer') {
        const remaining = Math.max(0, s.duration - s.elapsed);
        const progress = 1 - (remaining / s.duration);
        if (s.onTick) s.onTick(formatMs(remaining), progress);
        if (remaining <= 0) {
            const cb = s.onComplete;
            stopSlot('timer');
            if (cb) cb();
        }
    } else {
        if (s.onTick) s.onTick(formatMs(s.elapsed), 0);
    }
}

function formatMs(ms) {
    const totalSecs = Math.floor(ms / 1000);
    const h = Math.floor(totalSecs / 3600);
    const m = Math.floor((totalSecs % 3600) / 60);
    const s = totalSecs % 60;
    if (h > 0) return `${h}:${String(m).padStart(2,'0')}:${String(s).padStart(2,'0')}`;
    return `${m}:${String(s).padStart(2,'0')}`;
}

// --- Suggestion rendering ---

export function renderTimerSuggestion(parsed, container, currentMatches, resizeWindow) {
    container.innerHTML = '';
    container.scrollTop = 0;
    currentMatches.length = 0;

    const item = document.createElement('div');
    item.className = 'app-suggestion-item selected';

    if (parsed.type === 'timer') {
        const display = formatMs(parsed.durationMs);
        const existing = getSlotState('timer');
        currentMatches.push({ type: 'start_timer', durationMs: parsed.durationMs });
        const replaceNote = existing.active ? ' (replaces current)' : '';
        item.innerHTML = `
            <div class="app-icon">⏱️</div>
            <div class="app-info">
                <div class="app-name">Start ${display} timer${replaceNote}</div>
                <div class="app-description">Press Enter to start countdown</div>
            </div>
        `;
    } else if (parsed.type === 'hint') {
        currentMatches.push({ type: 'timer_hint' });
        const existing = getSlotState('timer');
        const replaceNote = existing.active ? ' · replaces current timer' : '';
        item.innerHTML = `
            <div class="app-icon">⏱️</div>
            <div class="app-info">
                <div class="app-name">Timer</div>
                <div class="app-description">timer 5m · timer 1h30m · timer 90s · timer 5:00${replaceNote}</div>
            </div>
        `;
    } else {
        const sw = getSlotState('stopwatch');
        if (sw.active && sw.running) {
            currentMatches.push({ type: 'pause_stopwatch' });
            item.innerHTML = `
                <div class="app-icon">⏸</div>
                <div class="app-info">
                    <div class="app-name">Pause Stopwatch</div>
                    <div class="app-description">Press Enter to pause</div>
                </div>
            `;
        } else if (sw.active && !sw.running) {
            currentMatches.push({ type: 'stop_stopwatch' });
            item.innerHTML = `
                <div class="app-icon">⏹</div>
                <div class="app-info">
                    <div class="app-name">Stop Stopwatch</div>
                    <div class="app-description">Press Enter to stop (currently paused)</div>
                </div>
            `;
        } else {
            currentMatches.push({ type: 'start_stopwatch' });
            item.innerHTML = `
                <div class="app-icon">⏱️</div>
                <div class="app-info">
                    <div class="app-name">Start Stopwatch</div>
                    <div class="app-description">Press Enter to start counting up</div>
                </div>
            `;
        }
    }

    container.appendChild(item);
    container.classList.add('visible');
    setTimeout(() => resizeWindow(), 10);
    return 0;
}

// --- Persistent timer bars (one per slot) ---

export function updateTimerBar(slotName, displayStr, progress, running) {
    const barId = `timerBar_${slotName}`;
    let bar = document.getElementById(barId);

    if (!displayStr && !_slots[slotName]) {
        if (bar) { bar.style.display = 'none'; bar.remove(); }
        return;
    }

    if (!bar) {
        bar = document.createElement('div');
        bar.id = barId;
        bar.className = 'timer-bar';
        const icon = slotName === 'stopwatch' ? '⏱️' : '⏳';
        const showAdd = slotName === 'timer' ? '' : ' style="display:none"';
        bar.innerHTML = `
            <div class="timer-bar-progress" id="${barId}_progress"></div>
            <span class="timer-bar-icon">${icon}</span>
            <span class="timer-bar-time" id="${barId}_time"></span>
            <div class="timer-bar-controls">
                <button class="timer-btn" id="${barId}_add" title="+1 minute"${showAdd}>+1m</button>
                <button class="timer-btn" id="${barId}_pause" title="Pause/Resume">⏸</button>
                <button class="timer-btn" id="${barId}_stop" title="Stop">⏹</button>
            </div>
        `;
        // Prevent buttons from stealing focus (which triggers window blur → hide)
        bar.querySelectorAll('button').forEach(btn => {
            btn.addEventListener('mousedown', e => e.preventDefault());
        });
        const inputContainer = document.querySelector('.input-container');
        if (inputContainer) inputContainer.parentNode.insertBefore(bar, inputContainer);
    }

    bar.style.display = 'flex';
    document.getElementById(`${barId}_time`).textContent = displayStr;
    document.getElementById(`${barId}_pause`).textContent = running ? '⏸' : '▶';

    const progressEl = document.getElementById(`${barId}_progress`);
    if (slotName === 'timer' && progressEl) {
        progressEl.style.width = `${Math.min(100, progress * 100)}%`;
        progressEl.style.display = '';
    } else if (progressEl) {
        progressEl.style.display = 'none';
    }
}

export function setupTimerBarControls(slotName, onStop, onResize) {
    setTimeout(() => {
        const barId = `timerBar_${slotName}`;
        const pauseBtn = document.getElementById(`${barId}_pause`);
        const stopBtn = document.getElementById(`${barId}_stop`);
        const addBtn = document.getElementById(`${barId}_add`);

        if (pauseBtn) pauseBtn.onclick = () => {
            pauseResumeSlot(slotName);
            const s = getSlotState(slotName);
            pauseBtn.textContent = s.running ? '⏸' : '▶';
        };
        if (stopBtn) stopBtn.onclick = () => {
            stopSlot(slotName);
            const bar = document.getElementById(barId);
            // Delay removal so the click event completes before the DOM changes
            // (removing the clicked element's ancestor triggers window blur)
            if (bar) setTimeout(() => { bar.style.display = 'none'; bar.remove(); if (onResize) onResize(); }, 50);
            if (onStop) onStop();
        };
        if (addBtn) addBtn.onclick = () => addTimeToTimer(60000);
    }, 50);
}
