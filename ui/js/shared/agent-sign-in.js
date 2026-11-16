/**
 * "Sign in" on a signed-out agent error: runs the agent's own login
 * (`kiro-cli login`) in a terminal inside the window that showed the
 * error, with input enabled, then restarts the agent and sends the failed
 * message again.
 *
 * Only a deliberate click on Kage's own button starts it, and the button
 * is drawn only when (a) the error is an object whose `kind` is one of
 * SIGN_IN_KINDS — plain-string errors never get one — and (b) the
 * backend's `agent_sign_in_context` says this agent can be signed in to
 * from here (local Kiro CLI). What runs is worked out by the backend
 * from Config: nothing here names a command. See docs/SECURITY_MODEL.md
 * § "Signing in to the agent".
 *
 * Each window hands in a host adapter:
 *   - `slot`: the element the panel goes in (outside the error, so a later
 *     error or a session switch can't remove a live run);
 *   - `rows` (terminal height), `compact` (the floating window: one more
 *     hint line), `buttonClass` (the window's error-button class);
 *   - `invoke` (optional; else window.__TAURI__.core.invoke);
 *   - `layoutChanged()`, `onActiveChange(active)`,
 *     `onEnded({ state, outcome, agent })`,
 *     `adoptSession(sid, failedSid)` (after every restart that came back:
 *     the backend pinned the window to `sid`, the failed session `failedSid`
 *     as reloaded; the window follows it while it still shows `failedSid`,
 *     else keeps the session it shows),
 *     `canResend(sid)`, `resend(sid)` → 'sent' | 'none', `clearError()`.
 *     The window decides what to resend, by the same rules as its
 *     Reconnect button: only the failed turn the error names (`turnId`),
 *     only when the error says it may be sent again (isResendablePayload),
 *     only while the window still shows its session and nothing was sent
 *     since, and at most once; the resend goes out as a turn of its own.
 *
 * SIGN_IN_KINDS are the serialized names of ErrorKind::AuthRequired /
 * AgentUnreachable (src/error.rs).
 *
 * On Linux and Windows the backend has kiro-cli sign in with the device
 * flow only (`ctx.deviceFlowOnly`), so it prints a link and a code and
 * starts no browser of its own. An organization can turn that flow off;
 * when a run then fails with kiro-cli's message saying so, the panel says
 * sign-in from Kage needs the device flow here and offers the command
 * (`ctx.manualCommand`) for the user's own terminal, where the browser
 * sign-in works, and Reconnect for afterwards. The output is only looked at here, in memory, for that
 * message (createDeviceFlowWatch); it is never logged or kept.
 */

import { errMessage } from './error-message.js';
import { t } from './i18n.js';
import { onDeliberateClick } from './shell-run-view.js';
import { createTerminalView, ensureXterm } from './terminal-view.js';
import { copyButton, el, icon } from './tool-call-dom.js';

export const SIGN_IN_KINDS = Object.freeze(['auth_required', 'agent_unreachable']);

/** How long a window trusts `agent_sign_in_context` before asking again. */
const CONTEXT_TTL_MS = 30_000;
/** "Starting…" turns into "Waiting for you…" after this, or on first output. */
const RUNNING_AFTER_MS = 1000;
/** Terminal line height in px (CSS: 12px font). */
const LINE_PX = 17;
/**
 * Stop pressed before the backend has claimed the run (xterm still
 * loading, or the start call in flight): ask again this often, until the
 * run is stopped or ends, for at most STOP_RETRY_LIMIT tries.
 */
const STOP_RETRY_MS = 200;
const STOP_RETRY_LIMIT = 100;

/**
 * What kiro-cli (3.3.0) prints when the organization has turned device-flow
 * sign-in off, matched whole on output with escapes stripped, whitespace
 * collapsed and case folded: "Your organization has disabled device flow
 * sign-in. Use browser sign-in.", "Every available profile has device flow
 * sign-in disabled", "None of the sign-in methods your organization permits
 * work without a browser". Whole sentences, so the notice a sign-in that is
 * still succeeding can print ("N profile(s) hidden: your organization has
 * disabled device flow sign-in for them") never matches.
 */
const DEVICE_FLOW_DISABLED = [
    /your organization has disabled device flow sign-in\. use browser sign-in/,
    /every available profile has device flow sign-in disabled/,
    /none of the sign-in methods your organization permits work without a browser/,
];
/** Longest wait for output messages still in flight when the run ends. */
const OUTPUT_SETTLE_MS = 1000;
/** Output kept from one message to the next, so a split message still matches. */
const WATCH_TAIL_CHARS = 512;

// The control characters are spliced in from char codes (as in
// shell-commands.js) so Biome's `noControlCharactersInRegex` stays quiet:
// matching them is the point. CSI, OSC (ended by BEL or ESC \) and the
// other two-character escapes; then any other control character.
const ESC = String.fromCharCode(0x1b);
const BEL = String.fromCharCode(0x07);
const ESCAPES = new RegExp(
    `${ESC}\\[[0-?]*[ -/]*[@-~]|${ESC}\\][^${BEL}${ESC}]*(?:${BEL}|${ESC}\\\\)|${ESC}[@-Z\\\\-_]`,
    'g'
);
const CONTROLS = new RegExp(
    `[${String.fromCharCode(0)}-${String.fromCharCode(0x1f)}${String.fromCharCode(0x7f)}]+`,
    'g'
);

/** Whether terminal `text` holds kiro-cli's "device flow turned off" message. */
export function outputSaysDeviceFlowDisabled(text) {
    if (typeof text !== 'string' || !text) return false;
    const plain = text
        .replace(ESCAPES, '')
        .replace(CONTROLS, ' ')
        .replace(/\s+/g, ' ')
        .toLowerCase();
    return DEVICE_FLOW_DISABLED.some((re) => re.test(plain));
}

/**
 * Watches one run's output for that message, message by message: keeps
 * only the last WATCH_TAIL_CHARS of it, so a message split between two
 * output messages (or inside an escape) still matches. Nothing it sees
 * leaves it but `matched`.
 */
export function createDeviceFlowWatch() {
    let tail = '';
    let matched = false;
    return {
        push(text) {
            if (matched || typeof text !== 'string' || !text) return;
            const joined = tail + text;
            if (outputSaysDeviceFlowDisabled(joined)) {
                matched = true;
                tail = '';
                return;
            }
            tail = joined.slice(-WATCH_TAIL_CHARS);
        },
        get matched() {
            return matched;
        },
        get tailLength() {
            return tail.length;
        },
    };
}

let ctxCache = null;
/** The window's sign-in, from the click until its last status. */
let active = null;

/** Whether `payload` (a MESSAGE_ERROR payload) is a signed-out error. */
export function canOfferSignIn(payload) {
    return (
        !!payload &&
        typeof payload === 'object' &&
        typeof payload.kind === 'string' &&
        SIGN_IN_KINDS.includes(payload.kind)
    );
}

/** Whether this window has a sign-in going (its run, restart or resend). */
export function isAgentSignInActive() {
    return !!active;
}

/** Put the keyboard back in the live terminal, if there is one. */
export function focusAgentSignIn() {
    active?.view?.focus();
}

export function _resetAgentSignInForTests() {
    clearTimeout(active?.runningTimer);
    active?.view?.dispose();
    active = null;
    ctxCache = null;
}

/**
 * The panel last opened in each slot, by its slot: how to let go of its
 * terminal. A finished sign-in (failed, stopped, timed out) leaves its
 * panel up with the terminal's last screen; whoever empties the slot must
 * dispose that terminal, or its emulator, buffer and theme observers stay
 * behind with every attempt.
 */
const panelDisposers = new WeakMap();

/**
 * Empty `slot` and dispose the sign-in terminal it held, unless a sign-in
 * is still going in this window. Returns whether it emptied it.
 */
export function clearAgentSignInSlot(slot) {
    if (!slot || active) return false;
    panelDisposers.get(slot)?.();
    panelDisposers.delete(slot);
    slot.textContent = '';
    return true;
}

/**
 * `reconnect_acp`'s answer (`ReconnectOutcome`) as `{ reconnected,
 * sessionId, sessionReloaded }`. A bare boolean (what it returned before it
 * reloaded the session) still reads as "same session".
 */
export function normalizeReconnect(result, fallbackSid) {
    const sid = fallbackSid ?? null;
    if (typeof result === 'boolean') {
        return { reconnected: result, sessionId: sid, sessionReloaded: true };
    }
    if (result && typeof result === 'object') {
        return {
            reconnected: !!result.reconnected,
            sessionId: result.sessionId || sid,
            sessionReloaded: result.sessionReloaded !== false,
        };
    }
    return { reconnected: false, sessionId: sid, sessionReloaded: false };
}

function invokeFor(host) {
    const invoke = host?.invoke || window.__TAURI__?.core?.invoke;
    return typeof invoke === 'function' ? invoke : null;
}

async function signInContext(invoke) {
    const now = Date.now();
    if (ctxCache && now - ctxCache.at < CONTEXT_TTL_MS) return ctxCache.promise;
    const entry = { at: now, promise: null };
    entry.promise = Promise.resolve()
        .then(() => invoke('agent_sign_in_context'))
        .catch(() => null)
        .then((ctx) => {
            // Don't remember a failure.
            if (!ctx && ctxCache === entry) ctxCache = null;
            return ctx;
        });
    ctxCache = entry;
    return entry.promise;
}

/** Same scheme as shell-run-view.js's run ids, with its own prefix. */
function newRunId() {
    const bytes = new Uint8Array(12);
    globalThis.crypto.getRandomValues(bytes);
    return `kage-term-${Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('')}`;
}

function button(className, label, iconName) {
    const btn = el('button', `shell-cmd-copy ${className}`);
    btn.type = 'button';
    if (iconName) btn.append(icon(iconName, 'shell-cmd-icon', 12));
    const span = el('span', 'shell-cmd-copy-label');
    span.dir = 'auto';
    span.textContent = label;
    btn.append(span);
    return btn;
}

function onClick(btn, fn) {
    btn.addEventListener('click', (e) => {
        e.stopPropagation();
        fn();
    });
}

/**
 * Add "Sign in" to an error's actions row when `payload` is a signed-out
 * error and the backend can sign in to the active agent from here.
 * Resolves to the button, or null (nothing is drawn or asked for a
 * plain-string error).
 */
export async function mountSignInAction(actionsEl, payload, host) {
    if (!actionsEl || !canOfferSignIn(payload)) return null;
    const invoke = invokeFor(host);
    if (!invoke) return null;
    const ctx = await signInContext(invoke);
    if (!ctx?.available) return null;
    // The error may have been replaced while we asked.
    if (!actionsEl.isConnected) return null;
    const existing = actionsEl.querySelector('.agent-sign-in-btn');
    if (existing) return existing;
    const btn = el('button', `agent-sign-in-btn ${host?.buttonClass || ''}`.trim());
    btn.type = 'button';
    btn.textContent = t('shared.agent_sign_in.btn.sign_in');
    onDeliberateClick(btn, () => {
        if (active) {
            focusAgentSignIn();
            return;
        }
        const slot = typeof host.slot === 'function' ? host.slot() : host.slot;
        if (!slot) return;
        openAgentSignIn({ slot, payload, host, ctx });
    });
    actionsEl.prepend(btn);
    host?.layoutChanged?.();
    return btn;
}

/**
 * Open the sign-in panel in `slot` and start a run. Resolves once the
 * sign-in reaches a final status (or right away when one is going).
 */
export function openAgentSignIn({ slot, payload, host, ctx }) {
    if (active) {
        focusAgentSignIn();
        return Promise.resolve(null);
    }
    const session = createSession({ slot, payload, host, ctx });
    active = session;
    host?.onActiveChange?.(true);
    return session.startRun();
}

function createSession({ slot, payload, host, ctx }) {
    const invoke = invokeFor(host);
    const sessionId = payload?.sessionId ?? null;
    const rows = host?.rows || 12;
    const agent = ctx?.agent || 'Kiro CLI';
    const command = ctx?.command || 'kiro-cli login';
    const timeoutMinutes = ctx?.timeoutMinutes || 15;

    // A finished panel from an earlier attempt: let go of its terminal.
    panelDisposers.get(slot)?.();
    slot.textContent = '';
    const panel = el('div', 'agent-sign-in');
    const head = el('div', 'agent-sign-in-head shell-run-head');
    const mark = el('span', 'agent-sign-in-mark shell-run-mark');
    const title = el('span', 'agent-sign-in-title');
    title.dir = 'auto';
    title.textContent = t('shared.agent_sign_in.title', { agent });
    const cmd = el('code', 'agent-sign-in-cmd');
    cmd.dir = 'ltr';
    cmd.textContent = command;
    const stopBtn = button('agent-sign-in-stop', t('shared.agent_sign_in.btn.stop'), 'stop');
    head.append(mark, title, cmd, stopBtn);

    const hints = [];
    const hint = el('p', 'agent-sign-in-hint');
    hint.dir = 'auto';
    hint.textContent = ctx?.deviceFlowOnly
        ? t('shared.agent_sign_in.hint_device_flow')
        : t('shared.agent_sign_in.hint');
    hints.push(hint);
    if (host?.compact) {
        const hides = el('p', 'agent-sign-in-hint agent-sign-in-hint-hides');
        hides.dir = 'auto';
        hides.textContent = t('shared.agent_sign_in.hint_hides');
        hints.push(hides);
    }
    const termEl = el('div', 'agent-sign-in-term');
    termEl.style.height = `${rows * LINE_PX + 8}px`;
    const status = el('div', 'agent-sign-in-status');
    status.setAttribute('role', 'status');
    status.dir = 'auto';
    // The command for the user's own terminal, when the device flow is off.
    const manual = el('div', 'agent-sign-in-manual');
    manual.hidden = true;
    const actions = el('div', 'agent-sign-in-actions');
    panel.append(head, ...hints, termEl, status, manual, actions);
    slot.append(panel);

    const session = {
        view: null,
        runId: null,
        done: null,
        resolveDone: null,
        // The current run reached a final status.
        settled: false,
        // start_agent_sign_in has been called for the current run.
        startCalled: false,
        // Stop was pressed for this run id and hasn't taken yet.
        stopRequested: null,
        // The current run's output, watched for "device flow turned off".
        watch: null,
    };

    const layout = () => host?.layoutChanged?.();

    function setMark(state) {
        mark.textContent = '';
        if (state === 'running') mark.append(el('span', 'shell-cmd-spinner'));
        else if (state === 'success') mark.append(icon('check', 'shell-cmd-icon', 12));
        else if (state === 'failed') mark.append(icon('cross', 'shell-cmd-icon', 12));
        else if (state === 'stopped') mark.append(icon('dash', 'shell-cmd-icon', 12));
    }

    function setStatus(state, text) {
        status.dataset.state = state;
        panel.dataset.state = state;
        status.textContent = text;
        setMark(state);
        layout();
    }

    function setActions(list) {
        actions.textContent = '';
        for (const btn of list) actions.append(btn);
        actions.hidden = list.length === 0;
    }

    /**
     * A final status after the terminal had the keyboard: hand it to the
     * first action (Sign in again, …) so Tab and Esc work without a mouse.
     */
    function moveFocusToActions(hadFocus) {
        if (!hadFocus) return;
        const first = actions.querySelector('button');
        if (first) first.focus();
        else if (panel.contains(document.activeElement)) document.activeElement.blur?.();
    }

    function focusInPanel() {
        return panel.contains(document.activeElement);
    }

    function disposeView() {
        clearTimeout(session.runningTimer);
        session.view?.dispose();
        session.view = null;
        termEl.hidden = true;
        for (const h of hints) h.hidden = true;
    }
    panelDisposers.set(slot, disposeView);

    function close() {
        disposeView();
        hideManual();
        panel.remove();
        finish(null);
        layout();
    }

    /** The sign-in reached a final status: it no longer holds the window. */
    function finish(state) {
        if (active === session) {
            active = null;
            host?.onActiveChange?.(false);
        }
        const resolve = session.resolveDone;
        session.resolveDone = null;
        resolve?.(state);
    }

    function closeButton() {
        const btn = button('agent-sign-in-close', t('shared.agent_sign_in.btn.close'));
        onClick(btn, close);
        return btn;
    }

    function reconnectButton() {
        const btn = button('agent-sign-in-reconnect', t('chat.error.btn.reconnect'));
        onDeliberateClick(btn, () => {
            if (active && active !== session) return;
            if (!active) {
                active = session;
                host?.onActiveChange?.(true);
            }
            setActions([]);
            recover();
        });
        return btn;
    }

    function terminalButton() {
        const btn = button(
            'agent-sign-in-open-terminal',
            t('shared.agent_sign_in.btn.open_terminal'),
            'terminal'
        );
        onDeliberateClick(btn, async () => {
            btn.disabled = true;
            try {
                await invoke('agent_sign_in_in_terminal', { runId: newRunId(), sessionId });
                disposeView();
                setStatus('running', t('shared.agent_sign_in.status.terminal_opened'));
                setActions([reconnectButton(), closeButton()]);
                // The user finishes elsewhere; Reconnect picks it up.
                finish('terminal');
            } catch (e) {
                btn.disabled = false;
                setStatus('failed', errMessage(e));
            }
        });
        return btn;
    }

    function againButton() {
        const btn = button('agent-sign-in-again', t('shared.agent_sign_in.btn.sign_in_again'));
        onDeliberateClick(btn, () => {
            if (active && active !== session) return;
            if (!active) {
                active = session;
                host?.onActiveChange?.(true);
            }
            session.startRun();
        });
        return btn;
    }

    function hideManual() {
        manual.hidden = true;
        manual.textContent = '';
    }

    function showManual(text) {
        manual.textContent = '';
        const code = el('code', 'agent-sign-in-manual-cmd');
        code.dir = 'ltr';
        code.textContent = text;
        manual.append(
            code,
            copyButton(
                () => t('shared.shell.copy_command'),
                () => text
            )
        );
        manual.hidden = false;
    }

    /**
     * The organization has turned device-flow sign-in off, and that is the
     * only way Kage signs in here: say so, and offer the command for the
     * user's own terminal. Signing in again here would fail the same way.
     */
    function deviceFlowDisabled(hadFocus) {
        setStatus('failed', t('shared.agent_sign_in.status.device_flow_disabled', { agent }));
        showManual(ctx?.manualCommand || command);
        // Signed in from their own terminal: Reconnect picks it up.
        const list = [reconnectButton()];
        if (ctx?.terminal) list.push(terminalButton());
        list.push(closeButton());
        setActions(list);
        layout();
        if (hadFocus) manual.querySelector('button')?.focus();
        host?.onEnded?.({ state: 'device_flow_disabled', outcome: 'exited', agent });
        finish('device_flow_disabled');
    }

    function failureActions() {
        const list = [againButton()];
        if (ctx?.terminal) list.push(terminalButton());
        list.push(closeButton());
        return list;
    }

    /** Restart the agent, then send the failed message again. */
    async function recover() {
        setStatus('running', t('shared.agent_sign_in.status.restarting'));
        let r;
        try {
            r = normalizeReconnect(await invoke('reconnect_acp', { sessionId }), sessionId);
        } catch (e) {
            setStatus(
                'failed',
                t('shared.agent_sign_in.status.restart_failed', { error: errMessage(e) })
            );
            setActions([reconnectButton(), closeButton()]);
            finish('restart_failed');
            return;
        }
        if (!r.reconnected) {
            setStatus(
                'failed',
                t('shared.agent_sign_in.status.restart_failed', {
                    error: t('chat.error.reconnect_failed'),
                })
            );
            setActions([reconnectButton(), closeButton()]);
            finish('restart_failed');
            return;
        }
        const sid = r.sessionId || sessionId;
        // Even with the same id: the restart pinned the window to it.
        if (sid) host?.adoptSession?.(sid, sessionId);
        if (!sid || !host?.canResend?.(sid)) {
            setStatus('success', t('shared.agent_sign_in.status.ready'));
            setActions([]);
            finish('ready');
            return;
        }
        setStatus('running', t('shared.agent_sign_in.status.resending'));
        let sent;
        try {
            sent = await host.resend(sid);
        } catch (e) {
            setStatus(
                'failed',
                t('shared.agent_sign_in.status.resend_failed', { error: errMessage(e) })
            );
            setActions([closeButton()]);
            finish('resend_failed');
            return;
        }
        if (sent === 'sent') {
            host?.clearError?.();
            setStatus(
                'success',
                r.sessionReloaded
                    ? t('shared.agent_sign_in.status.done')
                    : t('shared.agent_sign_in.status.done_new_session')
            );
            setActions([]);
            finish('done');
            return;
        }
        setStatus('success', t('shared.agent_sign_in.status.ready'));
        setActions([]);
        finish('ready');
    }

    /** The run ended with `report` (`outcome`, `exitCode`, `signal`). */
    function ended(report) {
        // A Stop the user pressed for this run wins over a clean exit the
        // backend reports, even one that finished before the Stop took: it
        // must not restart the agent or resend.
        if (
            session.stopRequested &&
            session.stopRequested === session.runId &&
            report?.outcome === 'exited' &&
            report.exitCode === 0
        ) {
            report = { ...report, outcome: 'cancelled', exitCode: null };
        }
        session.settled = true;
        const hadFocus = focusInPanel();
        session.view?.setInputEnabled(false);
        stopBtn.hidden = true;
        const outcome = report?.outcome || null;
        if (outcome === 'exited' && report.exitCode === 0) {
            disposeView();
            host?.onEnded?.({ state: 'success', outcome, agent });
            setActions([]);
            recover();
            return;
        }
        if (
            outcome === 'exited' &&
            Number.isInteger(report.exitCode) &&
            report.exitCode !== 0 &&
            ctx?.deviceFlowOnly &&
            session.watch?.matched
        ) {
            deviceFlowDisabled(hadFocus);
            return;
        }
        let state = 'failed';
        if (outcome === 'cancelled') {
            state = 'stopped';
            setStatus('stopped', t('shared.agent_sign_in.status.stopped'));
        } else if (outcome === 'timed_out') {
            state = 'timed_out';
            setStatus(
                'failed',
                t('shared.agent_sign_in.status.timed_out', { minutes: timeoutMinutes })
            );
        } else if (report?.signal) {
            setStatus(
                'failed',
                t('shared.agent_sign_in.status.failed_signal', { signal: report.signal })
            );
        } else {
            setStatus(
                'failed',
                t('shared.agent_sign_in.status.failed_exit', { code: report?.exitCode ?? '?' })
            );
        }
        setActions(failureActions());
        moveFocusToActions(hadFocus);
        host?.onEnded?.({ state, outcome, agent });
        finish(state);
    }

    function startFailed(error) {
        session.settled = true;
        const hadFocus = focusInPanel();
        session.view?.setInputEnabled(false);
        stopBtn.hidden = true;
        if (!session.view) termEl.hidden = true;
        setStatus(
            'failed',
            t('shared.agent_sign_in.status.start_failed', { error: errMessage(error) })
        );
        const list = [];
        if (ctx?.terminal) list.push(terminalButton());
        list.push(closeButton());
        setActions(list);
        moveFocusToActions(hadFocus);
        host?.onEnded?.({ state: 'start_failed', outcome: null, agent });
        finish('start_failed');
    }

    /**
     * Stop. `stop_user_terminal` answers false while the backend hasn't
     * claimed the run yet; the request is remembered (`stopRequested`, read
     * before the start call) and asked again until it takes or the run ends.
     */
    onClick(stopBtn, async () => {
        const runId = session.runId;
        if (!runId || session.settled) return;
        stopBtn.disabled = true;
        session.stopRequested = runId;
        setStatus('running', t('shared.agent_sign_in.status.stopping'));
        // Not started yet: startRun sees stopRequested and never starts it.
        if (!session.startCalled) return;
        const stillThisRun = () =>
            session.runId === runId && !session.settled && session.stopRequested === runId;
        try {
            for (let tries = 0; stillThisRun(); tries++) {
                if (await invoke('stop_user_terminal', { runId })) return;
                if (tries + 1 >= STOP_RETRY_LIMIT) break;
                await new Promise((r) => setTimeout(r, STOP_RETRY_MS));
            }
        } catch (e) {
            console.warn('[sign-in] stop failed:', errMessage(e));
        }
        if (stillThisRun()) {
            session.stopRequested = null;
            stopBtn.disabled = false;
        }
    });

    session.startRun = async function startRun() {
        const done = new Promise((resolve) => {
            session.resolveDone = resolve;
        });
        disposeView();
        termEl.hidden = false;
        for (const h of hints) h.hidden = false;
        stopBtn.hidden = false;
        stopBtn.disabled = false;
        setActions([]);
        hideManual();
        setStatus('running', t('shared.agent_sign_in.status.starting', { command }));
        const runId = newRunId();
        session.runId = runId;
        session.settled = false;
        session.startCalled = false;
        session.stopRequested = null;
        const watch = createDeviceFlowWatch();
        session.watch = watch;

        if (!invoke || !(await ensureXterm())) {
            startFailed(new Error('The terminal view could not be loaded'));
            return done;
        }
        let running = false;
        const markRunning = () => {
            if (running || session.runId !== runId || stopBtn.disabled) return;
            running = true;
            setStatus('running', t('shared.agent_sign_in.status.running'));
        };
        const view = createTerminalView(termEl, {
            rows,
            ariaLabel: t('shared.agent_sign_in.terminal_label'),
            onData: (data) => {
                invoke('write_user_terminal', { runId, data }).catch((e) =>
                    // Never log what was typed.
                    console.warn('[sign-in] input refused:', errMessage(e))
                );
            },
            onResize: (cols, termRows) => {
                invoke('resize_user_terminal', { runId, cols, rows: termRows }).catch(() => {});
            },
            onLink: (url) => {
                invoke('open_user_terminal_link', { runId, url }).catch((e) =>
                    console.warn('[sign-in] link not opened:', errMessage(e))
                );
            },
        });
        session.view = view;
        view.focus();
        layout();

        const Channel = window.__TAURI__?.core?.Channel;
        if (typeof Channel !== 'function') {
            startFailed(new Error('Tauri API unavailable'));
            return done;
        }
        const channel = new Channel();
        // Output arrives as `{ seq, data }`. Each message is acknowledged
        // once xterm has written it (or dropped, if this run's view is
        // gone): the backend sends at most 64 KiB ahead of the last ack,
        // so a stalled window holds the program back instead of piling up
        // output in Kage.
        const ack = (seq) => {
            if (!Number.isSafeInteger(seq) || seq < 0) return;
            invoke('ack_user_terminal', { runId, seq }).catch(() => {
                /* the run ended */
            });
        };
        let received = 0;
        let settle = null;
        channel.onmessage = (msg) => {
            const seq = msg?.seq;
            received += 1;
            if (settle && received >= settle.want) settle.resolve();
            if (session.runId === runId) watch.push(msg?.data);
            if (session.runId !== runId || session.view !== view || !msg?.data) {
                ack(seq);
                return;
            }
            view.write(msg.data, () => ack(seq));
            markRunning();
        };
        // Stop was pressed while xterm loaded: nothing was started.
        if (session.stopRequested === runId) {
            ended({ outcome: 'cancelled', exitCode: null, signal: null });
            return done;
        }
        const runningTimer = setTimeout(markRunning, RUNNING_AFTER_MS);
        // Kept on the session so a teardown can cancel it.
        session.runningTimer = runningTimer;
        const { cols, rows: termRows } = view.size();
        let report;
        session.startCalled = true;
        try {
            report = await invoke('start_agent_sign_in', {
                runId,
                cols,
                rows: termRows,
                sessionId,
                onOutput: channel,
            });
        } catch (e) {
            clearTimeout(runningTimer);
            startFailed(e);
            return done;
        }
        clearTimeout(runningTimer);
        // The reply can overtake the last output messages; the result is
        // read off them (createDeviceFlowWatch), so wait for every one the
        // backend sent, for at most OUTPUT_SETTLE_MS.
        const want = Number.isSafeInteger(report?.outputMessages) ? report.outputMessages : 0;
        if (received < want) {
            await new Promise((resolve) => {
                const timer = setTimeout(resolve, OUTPUT_SETTLE_MS);
                settle = {
                    want,
                    resolve: () => {
                        clearTimeout(timer);
                        resolve();
                    },
                };
            });
            settle = null;
        }
        ended(report);
        return done;
    };

    return session;
}
