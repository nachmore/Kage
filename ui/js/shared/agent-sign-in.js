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
 *     `onEnded({ state, outcome, agent })`, `adoptSession(sid)`,
 *     `canResend(sid)`, `resend(sid)` → 'sent' | 'none', `clearError()`.
 *
 * MERGE: SIGN_IN_KINDS must equal the serialized names of
 * ErrorKind::AuthRequired / AgentUnreachable (src/error.rs).
 */

import { errMessage } from './error-message.js';
import { t } from './i18n.js';
import { onDeliberateClick } from './shell-run-view.js';
import { createTerminalView, ensureXterm } from './terminal-view.js';
import { el, icon } from './tool-call-dom.js';

export const SIGN_IN_KINDS = Object.freeze(['auth_required', 'agent_unreachable']);

/** How long a window trusts `agent_sign_in_context` before asking again. */
const CONTEXT_TTL_MS = 30_000;
/** "Starting…" turns into "Waiting for you…" after this, or on first output. */
const RUNNING_AFTER_MS = 1000;
/** Terminal line height in px (CSS: 12px font). */
const LINE_PX = 17;

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
    active?.view?.dispose();
    active = null;
    ctxCache = null;
}

/**
 * `reconnect_acp`'s answer as `{ reconnected, sessionId, sessionReloaded }`.
 * Today it's a boolean (the session isn't reloaded or replaced); the
 * agent-recovery branch returns the object.
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
    hint.textContent = t('shared.agent_sign_in.hint');
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
    const actions = el('div', 'agent-sign-in-actions');
    panel.append(head, ...hints, termEl, status, actions);
    slot.append(panel);

    const session = {
        view: null,
        runId: null,
        done: null,
        resolveDone: null,
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

    function disposeView() {
        session.view?.dispose();
        session.view = null;
        termEl.hidden = true;
        for (const h of hints) h.hidden = true;
    }

    function close() {
        disposeView();
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
        if (sid && sid !== sessionId) host?.adoptSession?.(sid);
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
        host?.onEnded?.({ state, outcome, agent });
        finish(state);
    }

    function startFailed(error) {
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
        host?.onEnded?.({ state: 'start_failed', outcome: null, agent });
        finish('start_failed');
    }

    onClick(stopBtn, async () => {
        const runId = session.runId;
        if (!runId) return;
        stopBtn.disabled = true;
        setStatus('running', t('shared.agent_sign_in.status.stopping'));
        try {
            await invoke('stop_user_terminal', { runId });
        } catch (e) {
            console.warn('[sign-in] stop failed:', errMessage(e));
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
        setStatus('running', t('shared.agent_sign_in.status.starting', { command }));
        const runId = newRunId();
        session.runId = runId;

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
        channel.onmessage = (msg) => {
            if (session.runId !== runId || session.view !== view) return;
            if (msg?.data) {
                view.write(msg.data);
                markRunning();
            }
        };
        const runningTimer = setTimeout(markRunning, RUNNING_AFTER_MS);
        const { cols, rows: termRows } = view.size();
        let report;
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
        ended(report);
        return done;
    };

    return session;
}
