/**
 * "Run it here" and "Run in Terminal" on a shell block's sandbox notice —
 * the DOM side of shell-run.js, wired in by shell-command-view.js (which
 * hands over its helpers, so the two modules don't import each other).
 *
 * The agent's sandbox refused a command; the user may run it themselves.
 * Clicking a run button opens a confirm inside the notice (in the window,
 * never a native dialog) with the whole command — the exact text that
 * runs — where it runs, a warning that it runs outside the agent's sandbox
 * with the user's full permissions, a caution line per risk
 * (shell-run.js `shellCommandRisks`), and "Tell the agent the result".
 * Only its Run button starts a run, once: nothing is remembered, so every
 * run needs its own click, and nothing the agent prints can click it.
 *
 * While a run goes, the notice shows its output (a tail view), the time
 * it has taken and Stop; then its exit status, and — when asked — the
 * result goes to the agent as the user's next prompt, queued while a turn
 * is in progress (`setShellRunHost`: each window says how it sends).
 *
 * A run belongs to the shell record, not the block: a block rebuilt for
 * the same record (the chip list re-rendering) shows it too.
 *
 * The same run view shows commands an agent asked Kage to run outside its
 * sandbox (`attachAgentRun`, driven by host-command-view.js), and a window
 * can name a tray (`setRunChipTray`) where a run whose views have all left
 * the page (the floating window's reset) stays visible as a compact chip
 * with its Stop button until it ends.
 */

import { errMessage } from './error-message.js';
import { EVT } from './events.js';
import { t } from './i18n.js';
import {
    isPlainShellCommand,
    sanitizeTerminalText,
    shellCommandDisplayText,
} from './shell-commands.js';
import {
    AGENT_OUTPUT_CAP,
    agentShellHint,
    createAgentResultQueue,
    formatRunResultMessage,
    RUN_COMMAND_MAX_BYTES,
    RUN_OUTPUT_CAP,
    RUN_TIMEOUT_MINUTES,
    shellCommandRisks,
    tailOf,
    utf8Length,
} from './shell-run.js';
import { copyButton, el, icon, layoutChanged, uid } from './tool-call-dom.js';

// --- The window's agent hook --------------------------------------------------

let host = null;
let queue = null;

/**
 * How this window hands a result to the agent:
 *   - `sessionId()`: the session it shows now (the one a run reports to);
 *   - `canSend(sessionId)`: that session is in view and no turn is in
 *     progress, so a prompt can go now;
 *   - `send(sessionId, text)`: send `text` as the user's next prompt,
 *     shown in the chat like one they typed.
 * Without a host, the confirm offers no "Tell the agent" checkbox.
 */
export function setShellRunHost(next) {
    host = next || null;
}

function resultQueue() {
    if (!queue) {
        queue = createAgentResultQueue({
            canSend: (sessionId) => !!host && !!sessionId && !!host.canSend(sessionId),
            send: async ({ sessionId, text }) => {
                if (!host) throw new Error('This window can no longer reach the agent');
                await host.send(sessionId, text);
            },
        });
    }
    return queue;
}

function hostSessionId() {
    try {
        return host?.sessionId?.() || null;
    } catch {
        return null;
    }
}

function tauriInvoke() {
    const invoke = window.__TAURI__?.core?.invoke;
    if (typeof invoke !== 'function') throw new Error('Tauri API unavailable');
    return invoke;
}

/**
 * Terminal.app can be handed `command` (macOS). It types the command into
 * the user's interactive shell as keystrokes, so a tab would trigger
 * completion and insert text nobody approved: such a command is copied
 * instead. (Every other control character already keeps the run buttons
 * away — `isPlainShellCommand` — and the backend refuses them too.)
 */
function canRunInTerminal(command) {
    return (
        typeof navigator !== 'undefined' &&
        /^Mac/.test(navigator.platform || '') &&
        typeof command === 'string' &&
        !command.includes('\t')
    );
}

/**
 * A run id no other window will pick: output events reach every Kage
 * window that listens (Tauri delivers `emit_to` to any-target listeners),
 * so a per-window counter could feed one window's run another's output.
 */
function newRunId() {
    const bytes = new Uint8Array(12);
    globalThis.crypto.getRandomValues(bytes);
    return `kage-run-${Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('')}`;
}

// --- Runs ---------------------------------------------------------------------

/** shell record → its latest run. */
const runsByRecord = new WeakMap();
/** run id → run, while it's running (output events find it here). */
const runsById = new Map();
let listening = null;

/**
 * Whether one of the user's own runs is going in this window (the backend
 * allows one; runs an agent asked for have slots of their own).
 */
export function isShellRunActive() {
    for (const run of runsById.values()) if (run.origin !== 'agent') return true;
    return false;
}

/** Test hook: forget runs, the host, the queue and the chip tray. */
export function _resetShellRunsForTests() {
    for (const run of runsById.values()) clearInterval(run.timer);
    runsById.clear();
    queue?.dispose();
    queue = null;
    host = null;
    listening = null;
    chipTray = null;
    chips.clear();
}

function ensureOutputListener() {
    if (listening) return listening;
    const listen = window.__TAURI__?.event?.listen;
    if (typeof listen !== 'function') return Promise.resolve();
    listening = Promise.resolve(
        listen(EVT.USER_SHELL_OUTPUT, (event) => onRunOutput(event?.payload))
    ).catch((e) => {
        listening = null;
        console.warn('[shell-run] output listener failed:', e);
    });
    return listening;
}

function onRunOutput(payload) {
    const run = runsById.get(payload?.runId);
    if (!run || run.phase !== 'running' || typeof payload.text !== 'string') return;
    if (Number(payload.skipped) > 0) run.trimmed = true;
    run.output += payload.text;
    if (run.output.length > RUN_OUTPUT_CAP) {
        let cut = run.output.length - RUN_OUTPUT_CAP;
        const c = run.output.charCodeAt(cut);
        if (c >= 0xdc00 && c <= 0xdfff) cut += 1;
        run.output = run.output.slice(cut);
        run.trimmed = true;
    }
    renderRun(run, { output: true });
}

function formatElapsed(ms) {
    const total = Math.max(0, Math.floor(ms / 1000));
    const h = Math.floor(total / 3600);
    const m = Math.floor((total % 3600) / 60);
    const s = String(total % 60).padStart(2, '0');
    return h ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
}

// --- Controls on the notice --------------------------------------------------------

function button(className, label, iconName) {
    const btn = el('button', `shell-cmd-copy ${className}`);
    btn.type = 'button';
    if (iconName) btn.append(icon(iconName, 'shell-cmd-icon', 12));
    const span = el('span', 'shell-cmd-copy-label');
    span.dir = 'auto';
    span.textContent = label;
    btn.append(span);
    btn._kageLabel = span;
    return btn;
}

function onClick(btn, fn) {
    btn.addEventListener('click', (e) => {
        e.stopPropagation();
        fn();
    });
}

/**
 * The confirm's Run: a single, deliberate click. The second click of a
 * double-click (`detail` 2+) is ignored — the confirm opens under the
 * pointer, and the window can scroll or resize as it does, so the rest
 * of a double-click on "Run it here" must not land on Run.
 */
export function onDeliberateClick(btn, fn) {
    btn.addEventListener('click', (e) => {
        e.stopPropagation();
        if (e.detail > 1) return;
        fn();
    });
}

/** Lines the confirm's command box shows before it scrolls (max-height in CSS). */
const CONFIRM_VISIBLE_LINES = 8;
/** Characters that fill that box in a narrow (floating) window. */
const CONFIRM_VISIBLE_CHARS = 480;

/** Lines in `command`, ignoring a trailing newline. */
function lineCount(command) {
    return command.replace(/\n+$/, '').split('\n').length;
}

/**
 * The run buttons for `root`'s sandbox notice and the slot their panels
 * open in. `deps`: `{ appendCommandText(parent, command), setStatus(root,
 * text, isError), layoutChanged(node), copyAndOpenTerminal(root, button) }`
 * from shell-command-view.js.
 */
export function createRunControls(root, deps) {
    const runHere = button(
        'shell-run-btn shell-run-here',
        t('shared.shell.sandbox.run_here'),
        'play'
    );
    const terminal = button(
        'shell-run-btn shell-run-terminal',
        t('shared.shell.sandbox.run_terminal'),
        'terminal'
    );
    terminal.title = t('shared.shell.sandbox.run_terminal_title');
    const slot = el('div', 'shell-run-slot');
    const controls = {
        runHere,
        terminal,
        slot,
        deps,
        confirm: null,
        opening: false,
        live: null,
        liveRun: null,
    };
    onClick(runHere, () => openConfirm(root, 'here'));
    onClick(terminal, () => openConfirm(root, 'terminal'));
    return controls;
}

/**
 * Where a view shows its run: a shell block's notice controls, or the
 * holder a host-command card carries (`{ slot, live, liveRun, deps }`).
 */
function controlsOf(root) {
    return root?._kageShell?.notice?.run || root?._kageRunHolder || null;
}

/** Whether `root` still shows `run` (a shell block may have moved on to another record). */
function viewShows(root, run) {
    if (root?._kageRunHolder) return root._kageRunHolder.run === run;
    return runsByRecord.get(root?._kageShell?.record) === run;
}

/**
 * Bring `root`'s run controls up to date with its record: which buttons
 * show (only for a command that displays exactly as written and fits the
 * backend's limit; Terminal on macOS only), a confirm for a command that
 * changed is closed, and a run of this record shows here.
 */
export function syncRunControls(root) {
    const rc = controlsOf(root);
    if (!rc) return;
    const record = root._kageShell.record;
    const command = record?.command || '';
    const runnable =
        !!command && isPlainShellCommand(command) && utf8Length(command) <= RUN_COMMAND_MAX_BYTES;
    if (rc.confirm && rc.confirm.command !== command) closeConfirm(root, false);
    const run = runsByRecord.get(record) || null;
    rc.runHere.hidden = !runnable;
    rc.terminal.hidden = !runnable || !canRunInTerminal(command);
    const busy = !!rc.confirm || rc.opening || run?.phase === 'running';
    rc.runHere.disabled = busy;
    rc.terminal.disabled = busy;
    rc.runHere._kageLabel.textContent = run
        ? t('shared.shell.sandbox.run_again')
        : t('shared.shell.sandbox.run_here');
    if (run) {
        run.views.add(root);
        if (rc.liveRun !== run) renderRunView(run, root);
    } else if (rc.live) {
        rc.live.el.remove();
        rc.live = null;
        rc.liveRun = null;
    }
}

// --- The confirm ------------------------------------------------------------------

const RISK_TEXT = Object.freeze({
    sudo: () => t('shared.shell.sandbox.risk.sudo'),
    rm_rf: () => t('shared.shell.sandbox.risk.rm_rf'),
    pipe_to_shell: () => t('shared.shell.sandbox.risk.pipe_to_shell'),
    recursive_root: () => t('shared.shell.sandbox.risk.recursive_root'),
    shell_rc: () => t('shared.shell.sandbox.risk.shell_rc'),
});

/**
 * The confirm's command box: the whole command, exactly the text that
 * runs, with hidden characters marked by `appendCommandText` (shell-
 * command-view's `appendShellCommandText`), and the note that says when
 * there's more of it than the box shows. Shared with the prompt for
 * commands an agent asks to run (host-command-view.js).
 */
export function buildConfirmCommand(command, appendCommandText) {
    const pre = el('pre', 'shell-run-confirm-command');
    pre.dir = 'ltr';
    pre.tabIndex = 0;
    pre.setAttribute('role', 'group');
    pre.setAttribute('aria-label', t('shared.shell.command_label'));
    const prompt = el('span', 'shell-cmd-prompt');
    prompt.setAttribute('aria-hidden', 'true');
    prompt.textContent = '$ ';
    const text = el('span', 'shell-run-confirm-command-text');
    appendCommandText(text, command);
    pre.append(prompt, text);
    // More command than the box shows: the rest is only a scroll away,
    // and it runs too.
    const lines = lineCount(command);
    const longNote = el('p', 'shell-run-confirm-long');
    longNote.textContent = t('shared.shell.sandbox.confirm.long', { lines });
    longNote.hidden = lines <= CONFIRM_VISIBLE_LINES && command.length <= CONFIRM_VISIBLE_CHARS;
    return { pre, longNote };
}

/** Say when the box holds more command than it shows (call once it's in the page). */
export function revealLongNote({ pre, longNote }) {
    if (longNote.hidden && pre.scrollHeight > pre.clientHeight + 2) longNote.hidden = false;
}

/**
 * A caution line per risk `shellCommandRisks` finds in `command`, as a
 * list (hidden when there are none).
 */
export function buildRiskList(command) {
    const list = el('ul', 'shell-run-confirm-risks');
    for (const id of shellCommandRisks(command)) {
        const li = el('li', 'shell-run-risk');
        li.dataset.risk = id;
        li.textContent = RISK_TEXT[id]();
        list.append(li);
    }
    list.hidden = !list.childElementCount;
    return list;
}

async function openConfirm(root, mode) {
    const rc = controlsOf(root);
    if (!rc || rc.confirm || rc.opening) return;
    const st = root._kageShell;
    const record = st.record;
    const command = record?.command || '';
    if (!command || !isPlainShellCommand(command)) return;
    if (mode === 'terminal' && !canRunInTerminal(command)) return;
    rc.opening = true;
    syncRunControls(root);
    rc.deps.setStatus(root, '');
    let ctx = null;
    let ctxError = null;
    // Read once: the shell the confirm names is the one that runs it.
    const shellHint = agentShellHint(record);
    try {
        ctx = await tauriInvoke()('user_shell_run_context', {
            cwd: typeof record.cwd === 'string' && record.cwd ? record.cwd : null,
            shell: shellHint,
        });
    } catch (e) {
        ctxError = errMessage(e);
    }
    rc.opening = false;
    // The block moved on meanwhile (another command, notice gone).
    if (controlsOf(root) !== rc || (st.record?.command || '') !== command) {
        syncRunControls(root);
        return;
    }
    rc.confirm = buildConfirm(root, rc, { mode, command, shellHint, record, ctx, ctxError });
    rc.slot.prepend(rc.confirm.el);
    // A command taller than its box: say so, since (macOS overlay
    // scrollbars) nothing else may show there's more of it.
    revealLongNote(rc.confirm);
    syncRunControls(root);
    rc.deps.layoutChanged(root);
    rc.confirm.cancel.focus();
}

function closeConfirm(root, focusBack) {
    const rc = controlsOf(root);
    if (!rc?.confirm) return;
    const { mode } = rc.confirm;
    rc.confirm.el.remove();
    rc.confirm = null;
    syncRunControls(root);
    rc.deps.layoutChanged(root);
    if (focusBack) (mode === 'terminal' ? rc.terminal : rc.runHere).focus();
}

function buildConfirm(root, rc, { mode, command, shellHint, record, ctx, ctxError }) {
    const st = root._kageShell;
    const box = el('div', 'shell-run-confirm');
    box.setAttribute('role', 'group');
    const title = el('div', 'shell-run-confirm-title');
    title.id = uid(`${st.id}-run-confirm`);
    title.textContent =
        mode === 'terminal'
            ? t('shared.shell.sandbox.confirm.title_terminal')
            : t('shared.shell.sandbox.confirm.title');
    box.setAttribute('aria-labelledby', title.id);

    const { pre, longNote } = buildConfirmCommand(command, rc.deps.appendCommandText);

    const where = el('div', 'shell-run-confirm-where');
    if (ctx) {
        const label = el('span');
        label.textContent = t('shared.shell.sandbox.confirm.cwd_label');
        const path = el('code', 'shell-run-confirm-path');
        path.dir = 'ltr';
        path.textContent = ctx.cwd;
        where.append(label, ' ', path);
        if (mode === 'here' && ctx.shell) {
            const shell = el('span', 'shell-run-confirm-shell');
            shell.textContent = t('shared.shell.sandbox.confirm.shell', { shell: ctx.shell });
            where.append(' ', shell);
        }
    }
    where.hidden = !ctx;

    const warn = el('p', 'shell-run-confirm-warn');
    warn.textContent =
        mode === 'terminal'
            ? t('shared.shell.sandbox.confirm.warning_terminal')
            : t('shared.shell.sandbox.confirm.warning');

    const risks = shellCommandRisks(command);
    // sudo asks for a password, and a run here has no terminal to ask in.
    const sudoHere = mode === 'here' && risks.includes('sudo');
    const terminalOk = canRunInTerminal(command);
    const list = el('ul', 'shell-run-confirm-risks');
    for (const id of risks) {
        const li = el('li', 'shell-run-risk');
        li.dataset.risk = id;
        if (id === 'sudo' && sudoHere) {
            li.textContent = terminalOk
                ? t('shared.shell.sandbox.risk.sudo_here')
                : t('shared.shell.sandbox.risk.sudo_here_copy');
        } else {
            li.textContent = RISK_TEXT[id]();
        }
        list.append(li);
    }
    list.hidden = !risks.length;

    let tell = null;
    const tellRow = el('label', 'shell-run-confirm-tell');
    if (mode === 'here' && !sudoHere && host && hostSessionId()) {
        tell = el('input');
        tell.type = 'checkbox';
        tell.checked = true;
        const span = el('span');
        span.textContent = t('shared.shell.sandbox.confirm.tell_agent');
        tellRow.append(tell, span);
    }
    tellRow.hidden = !tell;

    const error = el('p', 'shell-run-confirm-error');
    const busy = mode === 'here' && !sudoHere && isShellRunActive();
    if (ctxError)
        error.textContent = t('shared.shell.sandbox.confirm.cwd_error', { error: ctxError });
    else if (busy) error.textContent = t('shared.shell.sandbox.confirm.busy');
    error.hidden = !error.textContent;

    const actions = el('div', 'shell-run-confirm-actions');
    const cancel = button('shell-run-cancel', t('shared.shell.sandbox.confirm.cancel'));
    const confirm = {
        el: box,
        mode,
        command,
        shellHint,
        ctx,
        tell,
        cancel,
        pre,
        longNote,
        primary: null,
        used: false,
    };
    let primary;
    if (sudoHere && terminalOk) {
        primary = button('shell-run-go', t('shared.shell.sandbox.run_terminal'), 'terminal');
        primary.title = t('shared.shell.sandbox.run_terminal_title');
        primary.disabled = !ctx;
        onDeliberateClick(primary, () => runInTerminal(root, confirm));
    } else if (sudoHere) {
        // No terminal Kage can hand it to: copy it, open the terminal app.
        primary = button('shell-run-go', t('shared.shell.sandbox.open_terminal'), 'terminal');
        onDeliberateClick(primary, () => {
            if (confirm.used) return;
            confirm.used = true;
            closeConfirm(root, false);
            rc.deps.copyAndOpenTerminal(root, primary);
        });
    } else {
        primary = button(
            'shell-run-go',
            mode === 'terminal'
                ? t('shared.shell.sandbox.run_terminal')
                : t('shared.shell.sandbox.confirm.run'),
            mode === 'terminal' ? 'terminal' : 'play'
        );
        if (mode === 'terminal') primary.title = t('shared.shell.sandbox.run_terminal_title');
        primary.disabled = !ctx || busy;
        onDeliberateClick(primary, () =>
            mode === 'terminal' ? runInTerminal(root, confirm) : runHere(root, confirm, record)
        );
    }
    confirm.primary = primary;
    onClick(cancel, () => closeConfirm(root, true));
    actions.append(cancel, primary);

    box.append(title, pre, longNote, where, warn, list, tellRow, error, actions);
    box.addEventListener('keydown', (e) => {
        if (e.key === 'Escape') {
            e.preventDefault();
            e.stopPropagation();
            closeConfirm(root, true);
        }
    });
    return confirm;
}

/** The confirm's Run: exactly one run for this click. */
function runHere(root, confirm, record) {
    if (confirm.used || !confirm.ctx) return;
    confirm.used = true;
    confirm.primary.disabled = true;
    confirm.cancel.disabled = true;
    const tellAgent = !!confirm.tell?.checked;
    const sessionId = hostSessionId();
    closeConfirm(root, false);
    startRun(root, record, {
        command: confirm.command,
        cwd: confirm.ctx.cwd,
        shellHint: confirm.shellHint,
        tellAgent,
        sessionId,
    });
}

/** Hand the command to Terminal.app: one window per click. */
async function runInTerminal(root, confirm) {
    if (confirm.used || !confirm.ctx) return;
    confirm.used = true;
    confirm.primary.disabled = true;
    confirm.cancel.disabled = true;
    const rc = controlsOf(root);
    closeConfirm(root, false);
    rc?.deps.setStatus(root, t('shared.shell.sandbox.terminal.opening'));
    try {
        await tauriInvoke()('run_command_in_terminal', {
            command: confirm.command,
            cwd: confirm.ctx.cwd,
            sessionId: hostSessionId(),
        });
        rc?.deps.setStatus(root, t('shared.shell.sandbox.terminal.sent'));
    } catch (e) {
        rc?.deps.setStatus(
            root,
            t('shared.shell.sandbox.terminal.failed', { error: errMessage(e) }),
            true
        );
    }
}

// --- Running and its view -------------------------------------------------------

async function startRun(root, record, opts) {
    const run = {
        runId: newRunId(),
        record,
        command: opts.command,
        cwd: opts.cwd,
        sessionId: opts.sessionId,
        tellAgent: opts.tellAgent,
        phase: 'running',
        startedAt: Date.now(),
        endedAt: null,
        output: '',
        trimmed: false,
        result: null,
        error: null,
        stopping: false,
        agent: null,
        agentError: null,
        queueId: null,
        views: new Set([root]),
        timer: null,
    };
    runsByRecord.set(record, run);
    runsById.set(run.runId, run);
    syncRunControls(root);
    run.timer = setInterval(() => tick(run), 1000);
    try {
        await ensureOutputListener();
        const result = await tauriInvoke()('run_user_shell_command', {
            command: run.command,
            runId: run.runId,
            cwd: run.cwd,
            shell: opts.shellHint,
            sessionId: run.sessionId,
        });
        run.result = result || {};
        run.phase = 'done';
        // The backend's tail is the authoritative output.
        if (typeof run.result.output === 'string') run.output = run.result.output;
        if (run.result.truncated) run.trimmed = true;
    } catch (e) {
        run.phase = 'failed';
        run.error = errMessage(e);
    } finally {
        clearInterval(run.timer);
        run.timer = null;
        run.endedAt = Date.now();
        runsById.delete(run.runId);
    }
    // The run buttons come back (each next run needs its own confirm).
    for (const view of [...run.views]) syncRunControls(view);
    renderRun(run);
    syncRunChips();
    // A stopped run isn't reported on its own: the user stepped in.
    if (run.phase === 'done' && run.tellAgent && run.result.outcome !== 'cancelled') {
        tellAgent(run);
    }
}

async function stopRun(run, btn) {
    if (run.phase !== 'running' || run.stopping) return;
    run.stopping = true;
    btn.disabled = true;
    renderRun(run);
    try {
        await tauriInvoke()('cancel_user_shell_command', { runId: run.runId });
    } catch (e) {
        run.stopping = false;
        console.warn('[shell-run] stop failed:', errMessage(e));
        renderRun(run);
    }
}

/** Send (or queue) the result to the agent as the user's next prompt. */
function tellAgent(run) {
    if (!host || !run.sessionId || run.phase !== 'done') return;
    if (run.agent === 'pending' || run.agent === 'queued' || run.agent === 'sent') return;
    const text = formatRunResultMessage({
        command: run.command,
        cwd: run.cwd,
        result: run.result,
    });
    run.agent = 'pending';
    run.agentError = null;
    run.queueId = resultQueue().submit({
        sessionId: run.sessionId,
        text,
        onStatus: (state, error) => {
            run.agent = state;
            run.agentError = error ? errMessage(error) : null;
            renderRun(run);
        },
    });
    renderRun(run);
}

/** Once a second while a run goes: its elapsed time, and its chip. */
function tick(run) {
    renderRun(run, { elapsed: true });
    syncRunChips();
}

function renderRun(run, what = {}) {
    for (const root of run.views) {
        const rc = controlsOf(root);
        if (!rc || !viewShows(root, run)) {
            run.views.delete(root);
            continue;
        }
        if (rc.liveRun !== run) renderRunView(run, root);
        else if (what.elapsed) updateElapsed(run, rc.live);
        else if (what.output) updateOutput(run, rc.live, rc);
        else renderRunView(run, root);
    }
}

function stateText(run) {
    if (run.phase === 'running') {
        return run.stopping
            ? t('shared.shell.sandbox.run.stopping')
            : t('shared.shell.sandbox.run.running');
    }
    if (run.phase === 'failed') return t('shared.shell.sandbox.run.failed', { error: run.error });
    const r = run.result;
    if (r.outcome === 'cancelled') return t('shared.shell.sandbox.run.cancelled');
    if (r.outcome === 'timed_out') {
        return t('shared.shell.sandbox.run.timed_out', {
            minutes: run.timeoutMinutes ?? RUN_TIMEOUT_MINUTES,
        });
    }
    if (r.exitCode === 0) return t('shared.shell.sandbox.run.succeeded');
    if (Number.isInteger(r.exitCode)) {
        return t('shared.shell.sandbox.run.exit_code', { code: r.exitCode });
    }
    if (Number.isInteger(r.signal))
        return t('shared.shell.sandbox.run.signal', { signal: r.signal });
    return t('shared.shell.sandbox.run.unknown');
}

function stateKind(run) {
    if (run.phase === 'running') return 'running';
    if (run.phase === 'failed') return 'failed';
    if (run.result.outcome === 'exited' && run.result.exitCode === 0) return 'success';
    return 'failed';
}

/** Build (or rebuild) `root`'s view of `run`. */
function renderRunView(run, root) {
    const rc = controlsOf(root);
    if (!rc) return;
    if (!rc.live) {
        const live = el('div', 'shell-run-live');
        const head = el('div', 'shell-run-head');
        const mark = el('span', 'shell-run-mark');
        const state = el('span', 'shell-run-state');
        state.setAttribute('role', 'status');
        state.dir = 'auto';
        const elapsed = el('span', 'shell-run-elapsed');
        elapsed.dir = 'ltr';
        const stop = button('shell-run-stop', t('shared.shell.sandbox.run.stop'), 'stop');
        head.append(mark, state, elapsed, stop);
        const output = el('pre', 'shell-run-output');
        output.dir = 'ltr';
        output.tabIndex = 0;
        output.setAttribute('role', 'group');
        output.setAttribute('aria-label', t('shared.shell.output_label'));
        const agent = el('div', 'shell-run-agent');
        agent.dir = 'auto';
        const agentText = el('span', 'shell-run-agent-text');
        const agentActions = el('span', 'shell-run-agent-actions');
        agent.append(agentText, agentActions);
        live.append(head, output, agent);
        rc.slot.append(live);
        rc.live = { el: live, mark, state, elapsed, stop, output, agent, agentText, agentActions };
    }
    const live = rc.live;
    rc.liveRun = run;
    live.el.dataset.state = stateKind(run);

    live.mark.textContent = '';
    if (run.phase === 'running') {
        const spinner = el('span', 'shell-cmd-spinner');
        spinner.setAttribute('aria-hidden', 'true');
        live.mark.append(spinner);
    } else {
        live.mark.append(
            icon(stateKind(run) === 'success' ? 'check' : 'cross', 'shell-cmd-icon', 12)
        );
    }
    live.state.textContent = stateText(run);
    live.stop.hidden = run.phase !== 'running';
    live.stop.disabled = run.stopping;
    live.stop.onclick = (e) => {
        e.stopPropagation();
        stopRun(run, live.stop);
    };
    updateElapsed(run, live);
    updateOutput(run, live, rc);
    renderAgentLine(run, live);
    rc.deps.layoutChanged(root);
}

function updateElapsed(run, live) {
    if (!live) return;
    live.elapsed.textContent = formatElapsed((run.endedAt ?? Date.now()) - run.startedAt);
}

/** The output's tail, as text, kept scrolled to the end unless the reader scrolled up. */
function updateOutput(run, live, rc) {
    if (!live) return;
    const pre = live.output;
    const atEnd = pre.scrollHeight - pre.scrollTop - pre.clientHeight < 8;
    const clean = sanitizeTerminalText(run.output);
    const tail = tailOf(clean, AGENT_OUTPUT_CAP);
    pre.textContent = '';
    if (tail.cut || run.trimmed) {
        const marker = el('span', 'shell-cmd-omitted');
        marker.dir = 'auto';
        marker.textContent = t('shared.shell.sandbox.run.earlier_output');
        pre.append(marker);
    }
    if (clean) {
        pre.append(document.createTextNode(tail.text));
    } else {
        const empty = el('span', 'shell-cmd-empty');
        empty.dir = 'auto';
        empty.textContent =
            run.phase === 'running'
                ? t('shared.shell.sandbox.run.no_output_yet')
                : t('shared.shell.sandbox.run.no_output');
        pre.append(empty);
    }
    if (atEnd) pre.scrollTop = pre.scrollHeight;
    rc?.deps.layoutChanged(pre);
}

function renderAgentLine(run, live) {
    const { agent, agentText, agentActions } = live;
    agentActions.textContent = '';
    let text = '';
    let canTell = false;
    if (run.origin === 'agent') {
        // An agent's run: its result goes back as the tool's result.
        if (run.phase !== 'running') text = t('shared.host_command.result_returned');
        if (run.phase === 'done' && run.output) {
            agentActions.append(
                copyButton(
                    () => t('shared.shell.copy_output'),
                    () => sanitizeTerminalText(run.output)
                )
            );
        }
    } else if (run.phase === 'done') {
        switch (run.agent) {
            case 'pending':
                text = t('shared.shell.sandbox.agent.sending');
                break;
            case 'queued': {
                text = t('shared.shell.sandbox.agent.queued');
                const dont = button(
                    'shell-run-dont-tell',
                    t('shared.shell.sandbox.agent.dont_tell')
                );
                onClick(dont, () => resultQueue().cancel(run.queueId));
                agentActions.append(dont);
                break;
            }
            case 'sent':
                text = t('shared.shell.sandbox.agent.sent');
                break;
            case 'failed':
                text = t('shared.shell.sandbox.agent.failed', { error: run.agentError || '' });
                canTell = true;
                break;
            case 'cancelled':
                text = t('shared.shell.sandbox.agent.not_told');
                canTell = true;
                break;
            default:
                canTell = true;
                if (run.tellAgent && run.result.outcome === 'cancelled') {
                    text = t('shared.shell.sandbox.agent.stopped_not_told');
                }
        }
        if (canTell && host && run.sessionId) {
            const tell = button('shell-run-tell', t('shared.shell.sandbox.agent.tell'));
            onClick(tell, () => tellAgent(run));
            agentActions.append(tell);
        }
        if (run.output) {
            agentActions.append(
                copyButton(
                    () => t('shared.shell.copy_output'),
                    () => sanitizeTerminalText(run.output)
                )
            );
        }
    }
    agentText.textContent = text;
    agentText.hidden = !text;
    agent.hidden = !text && !agentActions.childElementCount;
}

// --- Runs an agent asked for ----------------------------------------------------

/**
 * Show a run Kage started for an agent (`run_outside_sandbox`) in `card`:
 * output arrives as the same `user_shell_output` events, Stop is the same
 * `cancel_user_shell_command`, and the backend's `finished` / `failed`
 * update ends it (`finishAgentRun` / `failAgentRun`). `card` gets a
 * holder whose `slot` the run view renders into; it stays the run's view
 * until the card leaves the page (then the chip tray shows the run).
 */
export function attachAgentRun({ runId, command, cwd, timeoutSeconds, card, slot, layout }) {
    const run = {
        runId,
        origin: 'agent',
        record: null,
        command,
        cwd,
        sessionId: null,
        tellAgent: false,
        phase: 'running',
        startedAt: Date.now(),
        endedAt: null,
        output: '',
        trimmed: false,
        result: null,
        error: null,
        stopping: false,
        agent: null,
        agentError: null,
        queueId: null,
        timeoutMinutes: Number.isFinite(timeoutSeconds)
            ? Math.max(1, Math.round(timeoutSeconds / 60))
            : RUN_TIMEOUT_MINUTES,
        views: new Set([card]),
        timer: null,
    };
    card._kageRunHolder = {
        run,
        slot,
        live: null,
        liveRun: null,
        deps: { layoutChanged: (node) => layout?.(node) },
    };
    runsById.set(runId, run);
    ensureOutputListener();
    run.timer = setInterval(() => tick(run), 1000);
    renderRun(run);
    return run;
}

function endAgentRun(run) {
    clearInterval(run.timer);
    run.timer = null;
    run.endedAt = Date.now();
    runsById.delete(run.runId);
    renderRun(run);
    syncRunChips();
}

/** The backend's result for an agent's run (a `RunReport`). */
export function finishAgentRun(run, result) {
    if (!run || run.phase !== 'running') return;
    run.result = result || {};
    run.phase = 'done';
    if (typeof run.result.output === 'string') run.output = run.result.output;
    if (run.result.truncated) run.trimmed = true;
    endAgentRun(run);
}

/** An agent's run that couldn't start. */
export function failAgentRun(run, error) {
    if (!run || run.phase !== 'running') return;
    run.phase = 'failed';
    run.error = typeof error === 'string' ? error : errMessage(error);
    endAgentRun(run);
}

// --- Runs nothing shows any more --------------------------------------------------

let chipTray = null;
/** run id → its chip in the tray. */
const chips = new Map();
const CHIP_COMMAND_MAX = 120;

/**
 * Where this window keeps runs nothing else shows (the floating window
 * drops its response on a new prompt or Ctrl/⌘+L): each running one, user
 * or agent, gets a compact chip with its command, time and Stop until it
 * ends.
 */
export function setRunChipTray(tray) {
    for (const chip of chips.values()) chip.el.remove();
    chips.clear();
    chipTray = tray || null;
    syncRunChips();
}

/** Whether any of `run`'s views is still in the page. */
function shownInPage(run) {
    for (const view of run.views) {
        if (view?.isConnected && controlsOf(view) && viewShows(view, run)) return true;
    }
    return false;
}

/** Add a chip for each running run with no view in the page; drop the rest. */
export function syncRunChips() {
    for (const [runId, chip] of chips) {
        const run = runsById.get(runId);
        if (!chipTray || !run || run.phase !== 'running' || shownInPage(run)) {
            chip.el.remove();
            chips.delete(runId);
        }
    }
    if (!chipTray) return;
    let changed = false;
    for (const run of runsById.values()) {
        if (run.phase !== 'running' || shownInPage(run)) continue;
        let chip = chips.get(run.runId);
        if (!chip) {
            chip = buildChip(run);
            chips.set(run.runId, chip);
            chipTray.append(chip.el);
            changed = true;
        }
        chip.elapsed.textContent = formatElapsed(Date.now() - run.startedAt);
        chip.state.textContent = run.stopping
            ? t('shared.shell.sandbox.run.stopping')
            : t('shared.shell.sandbox.run.running');
        chip.stop.disabled = run.stopping;
    }
    chipTray.hidden = chips.size === 0;
    if (changed) layoutChanged(chipTray);
}

function buildChip(run) {
    const chip = el('div', 'shell-run-chip');
    chip.setAttribute('role', 'group');
    chip.dir = 'auto';
    const spinner = el('span', 'shell-cmd-spinner');
    spinner.setAttribute('aria-hidden', 'true');
    const state = el('span', 'shell-run-chip-state');
    state.setAttribute('role', 'status');
    const command = el('code', 'shell-run-chip-command');
    command.dir = 'ltr';
    const shown =
        run.command.length > CHIP_COMMAND_MAX
            ? `${run.command.slice(0, CHIP_COMMAND_MAX)}…`
            : run.command;
    command.textContent = `$ ${shellCommandDisplayText(shown).replace(/\s+/g, ' ')}`;
    command.title = shellCommandDisplayText(run.command.slice(0, 500));
    const elapsed = el('span', 'shell-run-elapsed');
    elapsed.dir = 'ltr';
    const stop = button('shell-run-stop', t('shared.shell.sandbox.run.stop'), 'stop');
    onClick(stop, () => stopRun(run, stop));
    chip.append(spinner, state, command, elapsed, stop);
    chip.setAttribute('aria-label', t('shared.shell.run_chip.label'));
    return { el: chip, state, elapsed, stop };
}
