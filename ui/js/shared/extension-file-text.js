/**
 * Reading a file the user picks, for an extension: the window's side
 * (docs/design/EXTENSION_FILE_TEXT.md §3.9, §5.1–§5.3).
 *
 * An extension holding `file_text` invokes `pick_file_text`. After its
 * usual checks the sandbox host hands the request to this window's
 * {@link FileTextBroker}, which:
 *   - refuses in a hidden window (`no_window`);
 *   - opens the dialog at once when the request follows a click the host
 *     saw (the sandbox's click lease) or, in a view window, a recent user
 *     activation; otherwise it shows the ask bar and waits for the user;
 *   - pauses the sandbox's click RPC timers while the request runs;
 *   - runs the window's dialog guard around the dialog (the launcher
 *     hides itself);
 *   - shows a progress chip with Cancel when reading takes a while.
 *
 * The click lease and the pausable RPC timers live here too, so the
 * sandbox host only calls into them. The ask bar and the chip are drawn
 * by extension-file-text-ui.js; the argument check is in
 * extension-url-policy.js with the other invoke argument checks. Rust
 * checks everything again.
 */

import { EVT } from './events.js';
import { cleanDisplayName, FileTextSurface } from './extension-file-text-ui.js';
import { t as hostT } from './i18n.js';

export { ASK_BAR_ARM_MS, cleanDisplayIcon, cleanDisplayName } from './extension-file-text-ui.js';

export const PICK_FILE_TEXT = 'pick_file_text';
export const CANCEL_FILE_TEXT = 'cancel_file_text';

/** A click the host saw allows one dialog within this long (§5.2). */
export const CLICK_LEASE_MS = 10_000;
/** The ask bar waits this long for an answer (§4.3). */
export const ASK_BAR_TIMEOUT_MS = 60_000;
/** After the user turned the ask bar down, the same sandbox waits this long. */
export const DECLINE_COOLDOWN_MS = 30_000;
/** The progress chip shows once reading has taken this long (§4.4). */
export const PROGRESS_CHIP_DELAY_MS = 400;
/**
 * How long a click or key press counts as a recent user activation (HTML's
 * transient activation: 5 s in Chromium and WebKit). In a view window an
 * activation opens one dialog. A click on the ask bar or the chip
 * activates the view page too, so it opens none (§5.2).
 */
export const USER_ACTIVATION_MS = 5_000;

// --- The click lease and pausable RPC timers (§5.2) ---------------------------

/** One sandbox's click lease: one dialog within {@link CLICK_LEASE_MS} of a click. */
export class ClickLease {
    constructor() {
        this._until = 0;
    }

    grant() {
        this._until = Date.now() + CLICK_LEASE_MS;
    }

    has() {
        return this._until > Date.now();
    }

    /** Whether a lease was held; it is gone either way. */
    consume() {
        const held = this.has();
        this._until = 0;
        return held;
    }
}

/**
 * An RPC timeout. A pausable one (for RPCs sent because of a click) can be
 * paused while a file dialog is open and resumed with the time it had left.
 *
 * @param {number} ms
 * @param {() => void} onFire
 * @param {{pausable?: boolean, paused?: boolean}} [opts] - `paused`: start
 *   paused (a pausable timer created while its sandbox's timers are paused)
 */
export function rpcTimer(ms, onFire, { pausable = false, paused = false } = {}) {
    let left = ms;
    let startedAt = 0;
    let handle = null;
    let over = false;
    const arm = () => {
        startedAt = Date.now();
        handle = setTimeout(() => {
            handle = null;
            over = true;
            onFire();
        }, left);
    };
    if (!(pausable && paused)) arm();
    return {
        clear() {
            over = true;
            if (handle !== null) clearTimeout(handle);
            handle = null;
        },
        pause() {
            if (!pausable || over || handle === null) return;
            clearTimeout(handle);
            handle = null;
            left = Math.max(0, left - (Date.now() - startedAt));
        },
        resume() {
            if (pausable && !over && handle === null) arm();
        },
    };
}

// --- Window helpers -------------------------------------------------------------

function currentWebviewWindow() {
    try {
        return globalThis.window?.__TAURI__?.webviewWindow?.getCurrentWebviewWindow?.() ?? null;
    } catch {
        return null;
    }
}

/**
 * Whether a window is on screen: visible and not minimised. Without a
 * window handle, or when asking fails, the document's visibility decides.
 */
export async function isWindowShown(appWindow) {
    try {
        if (appWindow && typeof appWindow.isVisible === 'function') {
            if ((await appWindow.isVisible()) === false) return false;
            if (typeof appWindow.isMinimized === 'function' && (await appWindow.isMinimized())) {
                return false;
            }
            return true;
        }
    } catch {}
    return typeof document === 'undefined' || document.visibilityState !== 'hidden';
}

/**
 * The view rule (§5.2): the page has focus and the engine reports a recent
 * click or key press. An engine without `navigator.userActivation` never
 * passes, so its views get the ask bar.
 */
export function strictUserActivation() {
    try {
        if (!document.hasFocus()) return false;
    } catch {
        return false;
    }
    return globalThis.navigator?.userActivation?.isActive === true;
}

// --- The broker -------------------------------------------------------------------

/**
 * One per window.
 *
 * @param {object} [opts]
 * @param {Function} [opts.invoke] - default: the asking sandbox's raw invoke
 * @param {Function} [opts.listen] - Tauri's `listen`, for `file_text_progress`
 * @param {() => Promise<boolean>|boolean} [opts.isVisible] - default: the
 *   current window is visible and not minimised
 * @param {{begin?: Function, end?: Function}} [opts.guard] - the dialog guard
 *   (§5.3); default no-op
 * @param {Element|(() => Element|null)} [opts.mount] - where the ask bar and
 *   the chip go (see FileTextSurface)
 * @param {() => void} [opts.onLayoutChange] - the bar or chip came or went
 * @param {() => boolean} [opts.activationLease] - a view sandbox's request
 *   counts as following a click when this says so (§5.2)
 * @param {Function} [opts.t] - the translator; default the host's
 */
export class FileTextBroker {
    constructor(opts = {}) {
        this._invoke = typeof opts.invoke === 'function' ? opts.invoke : null;
        this._listenFn =
            typeof opts.listen === 'function'
                ? opts.listen
                : globalThis.window?.__TAURI__?.event?.listen;
        this._isVisible =
            typeof opts.isVisible === 'function'
                ? opts.isVisible
                : () => isWindowShown(currentWebviewWindow());
        const guard = opts.guard || {};
        this._guard = {
            begin: typeof guard.begin === 'function' ? guard.begin : () => {},
            end: typeof guard.end === 'function' ? guard.end : () => {},
        };
        this._activationLease =
            typeof opts.activationLease === 'function' ? opts.activationLease : null;
        this._t = typeof opts.t === 'function' ? opts.t : hostT;
        this._ui = new FileTextSurface({
            mount: opts.mount,
            onLayoutChange: opts.onLayoutChange,
            t: this._t,
        });
        /** The ask bar or the request in flight; one per window. */
        this._current = null;
        /** @type {WeakMap<object, number>} sandbox → end of its cooldown */
        this._declined = new WeakMap();
        /** Until then, a view's user activation opens no dialog. */
        this._activationSpentUntil = 0;
        this._listening = null;
        this._unlisten = null;
    }

    /**
     * `pick_file_text` from `sandbox`, with the host's forwarded args.
     * Resolves the Rust answer (`FileText` or null), or null when the user
     * turned the ask bar down. Rejects with `{ kind, key }` errors.
     */
    async request(sandbox, args) {
        if (!(await this._visibleNow())) throw this._error('cancelled', 'no_window');
        if (sandbox._destroyed) return null;
        if (this._current) throw this._error('rate_limited', 'busy');
        if (!this._takeLease(sandbox)) {
            if ((this._declined.get(sandbox) || 0) > Date.now()) {
                throw this._error('rate_limited', 'declined');
            }
            const answer = await this._ask(sandbox);
            // The user's click on the bar activated the page.
            if (answer === 'choose' || answer === 'dismiss') this._spendActivation();
            if (answer === 'dismiss' || answer === 'timeout') {
                this._declined.set(sandbox, Date.now() + DECLINE_COOLDOWN_MS);
            }
            if (answer !== 'choose') return null;
        }
        return this._run(sandbox, args);
    }

    /**
     * `cancel_file_text` from `sandbox`: withdraws its ask bar, else asks
     * Rust to cancel its request.
     */
    async cancel(sandbox, args) {
        const cur = this._current;
        if (cur?.kind === 'ask' && cur.sandbox === sandbox) {
            cur.settle('cancel');
            return true;
        }
        return this._invokeFor(sandbox)(CANCEL_FILE_TEXT, args);
    }

    /** The sandbox is going away: drop its ask bar, or cancel its request. */
    abandon(sandbox) {
        const cur = this._current;
        if (!cur || cur.sandbox !== sandbox) return;
        if (cur.kind === 'ask') {
            cur.settle('abandoned');
            return;
        }
        cur.abandoned = true;
        this._hideChip(cur);
        this._sendCancel(cur);
    }

    /** Stop listening and drop an ask bar (a replaced broker). */
    dispose() {
        if (this._current?.kind === 'ask') this._current.settle('abandoned');
        try {
            this._unlisten?.();
        } catch {}
        this._unlisten = null;
        this._listening = null;
    }

    // --- internals ---------------------------------------------------------

    _invokeFor(sandbox) {
        return this._invoke || sandbox._rawInvoke;
    }

    _error(kind, code) {
        const key = `errors.file_text.${code}`;
        const err = new Error(this._t(key, {}));
        err.kind = kind;
        err.key = key;
        return err;
    }

    async _visibleNow() {
        try {
            return (await this._isVisible()) === true;
        } catch {
            return false;
        }
    }

    _takeLease(sandbox) {
        if (typeof sandbox.consumeClickLease === 'function' && sandbox.consumeClickLease()) {
            return true;
        }
        if (sandbox.surface !== 'view' || !this._activationLease) return false;
        if (Date.now() < this._activationSpentUntil) return false;
        let active = false;
        try {
            active = this._activationLease() === true;
        } catch {}
        if (active) this._spendActivation();
        return active;
    }

    /**
     * The page's current user activation can't open a dialog: it was used,
     * or it came from a click on the ask bar or the chip.
     */
    _spendActivation() {
        this._activationSpentUntil = Date.now() + USER_ACTIVATION_MS;
    }

    /**
     * The ask bar for `sandbox`: 'choose', 'dismiss', 'timeout', 'cancel'
     * (the extension withdrew it) or 'abandoned' (its sandbox went away).
     */
    async _ask(sandbox) {
        const shown = this._ui.showAsk({
            name: cleanDisplayName(sandbox.displayName, sandbox.extensionId || ''),
            icon: sandbox.displayIcon,
            timeoutMs: ASK_BAR_TIMEOUT_MS,
        });
        const ask = { kind: 'ask', sandbox, settle: shown.settle };
        this._current = ask;
        try {
            return await shown.answer;
        } finally {
            if (this._current === ask) this._current = null;
        }
    }

    async _run(sandbox, args) {
        const flight = {
            kind: 'flight',
            sandbox,
            extensionId: sandbox.extensionId,
            ended: null,
            chip: null,
            chipTimer: null,
            phase: 'reading',
            file: '',
            done: false,
            abandoned: false,
            recancelled: false,
        };
        this._current = flight;
        sandbox.pauseGestureTimers?.();
        try {
            await this._listenOnce();
            try {
                await this._guard.begin();
            } catch (e) {
                console.warn('[file-text] dialog guard begin failed:', e);
            }
            if (flight.abandoned) return null;
            const result = await this._invokeFor(sandbox)(PICK_FILE_TEXT, args);
            return result ?? null;
        } finally {
            this._hideChip(flight);
            await this._end(flight);
            sandbox.resumeGestureTimers?.();
            if (this._current === flight) this._current = null;
        }
    }

    _end(flight) {
        if (!flight.ended) {
            flight.ended = Promise.resolve()
                .then(() => this._guard.end())
                .catch((e) => console.warn('[file-text] dialog guard end failed:', e));
        }
        return flight.ended;
    }

    _sendCancel(flight) {
        try {
            Promise.resolve(
                this._invokeFor(flight.sandbox)(CANCEL_FILE_TEXT, {
                    extensionId: flight.extensionId,
                })
            ).catch(() => {});
        } catch {}
    }

    _listenOnce() {
        if (this._listening) return this._listening;
        if (typeof this._listenFn !== 'function') return Promise.resolve();
        this._listening = Promise.resolve()
            .then(() => this._listenFn(EVT.FILE_TEXT_PROGRESS, (e) => this._onProgress(e?.payload)))
            .then((unlisten) => {
                this._unlisten = typeof unlisten === 'function' ? unlisten : null;
            })
            .catch((e) => {
                console.warn('[file-text] progress events unavailable:', e);
            });
        return this._listening;
    }

    /** `file_text_progress` for this window (§3.10). */
    _onProgress(p) {
        const f = this._current;
        if (f?.kind !== 'flight' || !p || p.extension_id !== f.extensionId) return;
        if (f.abandoned) {
            // The sandbox went away before Rust held the request: cancel again.
            if (!f.recancelled) {
                f.recancelled = true;
                this._sendCancel(f);
            }
            return;
        }
        switch (p.phase) {
            case 'dialog_closed':
                this._end(f);
                break;
            case 'reading':
            case 'downloading':
            case 'extracting':
                if (f.done) return;
                if (p.phase !== 'extracting') f.phase = p.phase;
                if (typeof p.file === 'string') f.file = p.file;
                if (f.chip) f.chip.paint(f.phase, f.file);
                else if (!f.chipTimer) {
                    f.chipTimer = setTimeout(() => {
                        f.chipTimer = null;
                        this._showChip(f);
                    }, PROGRESS_CHIP_DELAY_MS);
                }
                break;
            case 'done':
                this._hideChip(f);
                break;
            default:
                break;
        }
    }

    _showChip(f) {
        if (f.chip || f.done || f.abandoned || this._current !== f) return;
        f.chip = this._ui.showChip({
            onCancel: () => {
                this._spendActivation();
                this._sendCancel(f);
            },
        });
        f.chip.paint(f.phase, f.file);
    }

    _hideChip(f) {
        f.done = true;
        if (f.chipTimer) clearTimeout(f.chipTimer);
        f.chipTimer = null;
        f.chip?.remove();
        f.chip = null;
    }
}

// --- This window's broker ---------------------------------------------------------

let windowBroker = null;

/** Set up this window's broker (replacing any earlier one) and return it. */
export function installFileTextBroker(opts = {}) {
    windowBroker?.dispose();
    windowBroker = new FileTextBroker(opts);
    return windowBroker;
}

/** This window's broker; a default one (no guard, fixed layer) if none was installed. */
export function fileTextBroker() {
    if (!windowBroker) windowBroker = new FileTextBroker();
    return windowBroker;
}

/**
 * The sandbox host's hand-off: the broker's answer for `pick_file_text` and
 * `cancel_file_text`, null for any other command.
 */
export function routeFileTextInvoke(sandbox, command, args) {
    if (command === PICK_FILE_TEXT) return fileTextBroker().request(sandbox, args);
    if (command === CANCEL_FILE_TEXT) return fileTextBroker().cancel(sandbox, args);
    return null;
}

/** A sandbox is being destroyed: drop its ask bar or cancel its request. */
export function abandonFileText(sandbox) {
    windowBroker?.abandon(sandbox);
}
