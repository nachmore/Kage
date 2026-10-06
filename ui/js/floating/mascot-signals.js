/**
 * Floating-window mascot signalling — the glue between app events and the
 * animation engine's `signal()` vocabulary (`ui/js/shared/mascot-engine.js`).
 *
 * Everything in here is deliberately defensive: `window._kageMascot` is null
 * in terminator mode and briefly absent during a theme-driven rebuild, and
 * the controller itself drops every signal when mascot animations are off.
 * Callers therefore never have to check anything first — a signal is always
 * safe to send.
 */

/**
 * Send one engine signal. Swallows everything: a mascot that throws must
 * never break the event handler it was called from.
 */
export function signalMascot(name, data) {
    try {
        window._kageMascot?.signal(name, data);
    } catch (e) {
        console.warn('[mascot] signal failed:', name, e);
    }
}

// ── generic activity hints (extensions + the host timer) ────────────────────

/** Fixed precedence. Earlier wins; anything not listed is rejected. */
const HINT_PRIORITY = ['meeting', 'timer', 'music'];

// Leases expire so a crashed, unloaded or hidden extension can't pin a pose
// forever — holders re-assert on every refresh. The bounds keep a buggy
// extension from either thrashing the arbiter or squatting for a whole day.
const HINT_TTL_MIN_MS = 15_000;
const HINT_TTL_MAX_MS = 600_000;
const HINT_TTL_DEFAULT_MS = 30_000;

/**
 * Reserved lease id for the floating window's own countdown timer, so the
 * host and the extensions arbitrate through one priority list instead of
 * overwriting each other's `hint:*` signals.
 */
const HOST_TIMER_ID = '__host_timer';

/**
 * The host timer re-asserts off its existing 100ms tick, so its lease only
 * has to outlive one tick. Pausing or stopping the countdown stops the ticks
 * and the lease lapses on its own — no extra teardown path to keep in sync.
 */
const HOST_TIMER_TTL_MS = 1500;

/** id → { activity, expiresAt, seq }. `seq` breaks same-activity ties. */
const _leases = new Map();
let _leaseSeq = 0;
let _expiryTimer = null;
let _currentHint = null;

function _rescheduleExpiry() {
    if (_expiryTimer !== null) {
        clearTimeout(_expiryTimer);
        _expiryTimer = null;
    }
    // One shared timer for every lease — armed for whichever expires first.
    let soonest = Infinity;
    for (const lease of _leases.values()) {
        if (lease.expiresAt < soonest) soonest = lease.expiresAt;
    }
    if (!Number.isFinite(soonest)) return;
    _expiryTimer = setTimeout(_recomputeHint, Math.max(16, soonest - Date.now()));
}

function _recomputeHint() {
    const now = Date.now();
    let best = null;
    for (const [id, lease] of _leases) {
        if (lease.expiresAt <= now) {
            _leases.delete(id);
            continue;
        }
        const rank = HINT_PRIORITY.indexOf(lease.activity);
        if (!best || rank < best.rank || (rank === best.rank && lease.seq > best.seq)) {
            best = { rank, seq: lease.seq, activity: lease.activity };
        }
    }
    const next = best?.activity ?? null;
    // Only signal on *change*. Re-sending the same `hint:x` restarts the
    // engine's hint loop from frame zero, and the host timer re-asserts ten
    // times a second — that would be a permanent stutter.
    if (next === _currentHint) {
        _rescheduleExpiry();
        return;
    }
    _currentHint = next;
    signalMascot(next ? `hint:${next}` : 'hint:none');
    _rescheduleExpiry();
}

function _setLease(id, activity, ttlMs) {
    if (!id) return;
    if (!activity || !HINT_PRIORITY.includes(activity)) {
        if (!_leases.delete(id)) return; // nothing to clear, nothing changed
        _recomputeHint();
        return;
    }
    _leases.set(id, { activity, expiresAt: Date.now() + ttlMs, seq: ++_leaseSeq });
    _recomputeHint();
}

/**
 * Publish the arbiter globals the extension sandbox host calls into. Install
 * once, early — before any extension can run — so a hint declared during
 * extension startup isn't dropped on the floor.
 */
export function installMascotHintArbiter() {
    // Called by the extension sandbox host when an extension declares (or
    // clears) a generic mascot activity. Leases expire so a crashed or hidden
    // extension can't pin a pose forever; the extension re-asserts on each
    // refresh. Highest priority wins: meeting > timer > music.
    window.__kageMascotHint = (
        extensionId,
        activity /* 'music'|'meeting'|'timer'|null */,
        ttlMs
    ) => {
        const ttl = Number.isFinite(ttlMs)
            ? Math.min(HINT_TTL_MAX_MS, Math.max(HINT_TTL_MIN_MS, ttlMs))
            : HINT_TTL_DEFAULT_MS;
        _setLease(String(extensionId ?? ''), activity || null, ttl);
    };
    window.__kageMascotHintClear = (extensionId) => {
        _setLease(String(extensionId ?? ''), null, 0);
    };
}

/**
 * Assert (or drop) the host countdown timer's hint lease. Goes through the
 * same arbiter as extension hints under a reserved pseudo-id so the two can
 * never fight over `hint:*`.
 */
export function setHostTimerHint(running) {
    _setLease(HOST_TIMER_ID, running ? 'timer' : null, HOST_TIMER_TTL_MS);
}

// ── agent errors ───────────────────────────────────────────────────────────

// `message_error` arrives as a plain string: every Rust emitter formats its
// own text (see src/commands/messaging/*.rs), so unlike a command rejection
// there is no AppError `kind` to switch on. Match the rate-limit wording
// instead — and still honour a `{kind, message}` shape in case a future
// emitter starts forwarding the struct.
const RATE_LIMIT_RE = /rate[\s_-]?limit|too many requests|\b429\b|quota exceeded/i;

/** Route a `message_error` payload to `rateLimit` or the generic `error`. */
export function signalMascotError(payload) {
    const isObj = payload !== null && typeof payload === 'object';
    const kind = isObj ? payload.kind : null;
    const text = typeof payload === 'string' ? payload : isObj ? payload.message || '' : '';
    signalMascot(kind === 'rate_limited' || RATE_LIMIT_RE.test(text) ? 'rateLimit' : 'error');
}

/**
 * The agent is answering again: clear both stuck situations. Each is a no-op
 * in the engine when that situation wasn't active, so this is safe to call on
 * every successful send.
 */
export function signalMascotRecovered() {
    signalMascot('recover');
    signalMascot('rateLimitDone');
}

// ── tool activity ──────────────────────────────────────────────────────────

// ACP tool-call statuses that mean "this call is over". Anything else (including
// a missing status) counts as still in flight.
const TERMINAL_TOOL_STATUSES = new Set([
    'completed',
    'failed',
    'error',
    'cancelled',
    'canceled',
    'aborted',
    'rejected',
]);

/**
 * Turns the stream of `tool_call_update` notifications into `tool:<kind>` /
 * `tool:none` signals. In-flight call ids are tracked so overlapping calls
 * don't clear the activity pose early.
 */
export function createToolActivityTracker() {
    const inFlight = new Set();
    let lastKind = null;

    return {
        /** @param {{toolCallId?: string, kind?: string, status?: string}} update */
        onUpdate(update) {
            if (!update) return;
            const id = update.toolCallId || '';
            const status = String(update.status || '').toLowerCase();
            const kind = String(update.kind || '').toLowerCase();

            if (status && TERMINAL_TOOL_STATUSES.has(status)) {
                if (id) inFlight.delete(id);
                if (inFlight.size > 0) return; // another call is still working
                if (lastKind === null) return;
                lastKind = null;
                signalMascot('tool:none');
                return;
            }

            if (id) inFlight.add(id);
            // Progress-only updates carry no `kind` — keep the pose we already
            // adopted rather than dropping back to the plain thinking hop.
            if (!kind || kind === lastKind) return;
            lastKind = kind;
            // The engine owns kind→activity mapping (TOOL_ACTIVITY); pass the
            // backend's raw kind straight through.
            signalMascot(`tool:${kind}`);
        },

        /** Forget everything — the turn ended, so nothing is in flight. */
        reset() {
            inFlight.clear();
            lastKind = null;
        },
    };
}

// ── hover & drag ───────────────────────────────────────────────────────────

/**
 * How long after the last observed drag sample we call the drag over. The OS
 * owns the mouse during a window drag, so there is no reliable pointerup to
 * wait for.
 */
const DRAG_IDLE_MS = 180;

/**
 * Feed the mascot's live hover gaze and drag lean.
 *
 * Purely observational: no `preventDefault`, no `stopPropagation`, all
 * listeners passive — `WindowManager.setupDragging` owns the same element's
 * mousedown and must keep working untouched.
 *
 * @param {HTMLElement|null} container the `.mascot-container` drag handle
 * @param {{windowManager?: {isDragging?: boolean}, appWindow?: object}} deps
 */
export function initMascotPointerSignals(container, { windowManager, appWindow } = {}) {
    if (!container) return;

    let dragging = false;
    let lastX = 0;
    let lastT = 0;
    let idleTimer = null;

    const endDrag = () => {
        if (idleTimer !== null) {
            clearTimeout(idleTimer);
            idleTimer = null;
        }
        if (!dragging) return;
        dragging = false;
        signalMascot('dragEnd');
    };

    const feedDrag = (x, ts) => {
        if (!dragging) {
            // First sample only establishes the baseline — no velocity yet.
            dragging = true;
        } else if (ts > lastT) {
            signalMascot('drag', (x - lastX) / (ts - lastT));
        }
        lastX = x;
        lastT = ts;
        if (idleTimer !== null) clearTimeout(idleTimer);
        idleTimer = setTimeout(endDrag, DRAG_IDLE_MS);
    };

    container.addEventListener(
        'pointermove',
        (e) => {
            // No gaze tracking mid-drag: the window travels with the cursor,
            // so clientX barely moves and the eyes would just jitter.
            if (windowManager?.isDragging) return;
            const rect = container.getBoundingClientRect();
            if (!rect.width) return;
            const x = (e.clientX - rect.left) / rect.width;
            signalMascot('hover', Math.min(1, Math.max(0, x)));
        },
        { passive: true }
    );
    container.addEventListener('pointerleave', () => signalMascot('unhover'), { passive: true });

    // `start_drag_window` hands the mouse to the OS move loop, so the webview
    // sees no pointermove while dragging — and clientX wouldn't move anyway.
    // Window position deltas are the only observable horizontal velocity.
    appWindow
        ?.onMoved?.(({ payload }) => {
            if (!windowManager?.isDragging) return;
            const dpr = window.devicePixelRatio || 1;
            feedDrag(payload.x / dpr, performance.now());
        })
        ?.catch?.((e) => console.warn('[mascot] onMoved listen failed:', e));

    window.addEventListener('pointerup', endDrag, { passive: true });
    window.addEventListener('pointercancel', endDrag, { passive: true });
}
