/**
 * Reading a file the user picks: the window's side
 * (docs/design/EXTENSION_FILE_TEXT.md §3.9, §4.3, §4.4, §5.1–§5.3, §12).
 *
 *   - the click lease: set by each `userGesture` call site, used once,
 *     gone after 10 s, never set by match/getTools/tool calls;
 *   - click RPC timers pause while a request runs and resume with the
 *     time they had left;
 *   - the ask bar: Choose file…, Not now, Escape, the timeout, the
 *     cooldown, one per window, withdrawn by cancel and by destroy;
 *   - `no_window` in a hidden window, `busy` with a request in flight;
 *   - the dialog guard order and the progress chip with its Cancel;
 *   - the view rule;
 *   - host-raised errors carry `errorKind` and `errorKey`.
 *
 * The sandbox is the real ExtensionSandbox with a fake port, as in
 * extension-sandbox-host.test.js. Rust is a mock `invoke`.
 */

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
    ASK_BAR_ARM_MS,
    ASK_BAR_TIMEOUT_MS,
    CLICK_LEASE_MS,
    ClickLease,
    DECLINE_COOLDOWN_MS,
    installFileTextBroker,
    PROGRESS_CHIP_DELAY_MS,
    rpcTimer,
    USER_ACTIVATION_MS,
} from '../../ui/js/shared/extension-file-text.js';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';
import { ExtensionSandbox } from '../../ui/js/shared/extension-sandbox-host.js';
import { runSettingsHostSideEffect } from '../../ui/js/shared/settings-host-effects.js';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const EN = JSON.parse(readFileSync(path.join(repoRoot, 'locales/en/messages.json'), 'utf8'));

/** The EN catalog with `{name}` substitution, enough for these strings. */
function tr(key, vars = {}) {
    const template = EN[key]?.message ?? key;
    return template.replace(/\{(\w+)\}/g, (_, name) => String(vars[name] ?? ''));
}

function deferred() {
    let resolve;
    let reject;
    const promise = new Promise((res, rej) => {
        resolve = res;
        reject = rej;
    });
    return { promise, resolve, reject };
}

const flush = () => vi.advanceTimersByTimeAsync(0);

const FILE = {
    name: 'Q3 plan.pdf',
    extension: 'pdf',
    format: 'pdf',
    text: 'Hello',
    chars: 5,
};

/**
 * Rust as a mock: `pick_file_text` answers from `picks` (a deferred each
 * call), `cancel_file_text` answers true.
 */
function fakeRust() {
    const picks = [];
    const invoke = vi.fn((command) => {
        if (command === 'pick_file_text') {
            const d = deferred();
            picks.push(d);
            return d.promise;
        }
        if (command === 'cancel_file_text') return Promise.resolve(true);
        return Promise.resolve(null);
    });
    return { invoke, picks };
}

function makeSandbox(invoke, spec = {}) {
    const sb = new ExtensionSandbox(
        {
            extensionId: 'mindmap',
            capabilities: ['storage', 'file_text'],
            config: {},
            sources: {},
            displayName: 'Mind Map',
            displayIcon: '🧠',
            ...spec,
        },
        invoke,
        document.body
    );
    const sent = [];
    sb._port = {
        postMessage: (m) => sent.push(m),
        close() {},
    };
    sb._ready = true;
    return { sb, sent };
}

/** This window's broker with a fake `listen`; `emit` sends progress. */
function broker({ visible = true, guard, activationLease } = {}) {
    let handler = null;
    const order = [];
    const listen = vi.fn(async (event, fn) => {
        expect(event).toBe('file_text_progress');
        handler = fn;
        return () => {
            handler = null;
        };
    });
    const b = installFileTextBroker({
        listen,
        isVisible: () => visible,
        guard: guard ?? {
            begin: () => order.push('begin'),
            end: () => order.push('end'),
        },
        activationLease,
        t: tr,
    });
    const emit = (payload, extensionId = 'mindmap') =>
        handler?.({ payload: { extension_id: extensionId, ...payload } });
    return { broker: b, order, emit, listen };
}

function pick(sb, args = {}, id = 1) {
    return sb._handleInvoke({ id, command: 'pick_file_text', args });
}

const askBar = () => document.querySelector('.kage-file-ask');
const chip = () => document.querySelector('.kage-file-chip');

beforeEach(() => {
    vi.useFakeTimers();
});

afterEach(() => {
    vi.useRealTimers();
    document.body.innerHTML = '';
    installFileTextBroker({ isVisible: () => true });
});

// --- The click lease ------------------------------------------------------------

describe('the click lease', () => {
    it('is used once and is gone after 10 s', () => {
        const lease = new ClickLease();
        expect(lease.consume()).toBe(false);
        lease.grant();
        expect(lease.has()).toBe(true);
        expect(lease.consume()).toBe(true);
        expect(lease.consume()).toBe(false);
        lease.grant();
        vi.advanceTimersByTime(CLICK_LEASE_MS);
        expect(lease.has()).toBe(false);
    });

    it('comes only from userGesture RPCs, not match, getTools or tool calls', () => {
        const { sb } = makeSandbox(vi.fn());
        for (const method of ['match', 'matchAsync', 'getTools', 'executeTool', 'renderWidget']) {
            sb.call(method, {}).catch(() => {});
        }
        expect(sb.hasClickLease()).toBe(false);
        sb.call('execute', {}, { userGesture: true }).catch(() => {});
        expect(sb.hasClickLease()).toBe(true);
        // Peeking (the agent calls design's `when: 'now'`) doesn't use it.
        expect(sb.hasClickLease()).toBe(true);
        expect(sb.consumeClickLease()).toBe(true);
        expect(sb.consumeClickLease()).toBe(false);
        sb.destroy();
    });

    it('is set by every userGesture call site', async () => {
        const opts = [];
        const sandbox = {
            hasSearch: true,
            hasToolbar: true,
            hasDateProvider: true,
            widgetIds: [],
            call: vi.fn(async (method, _params, o) => {
                opts.push([method, o?.userGesture === true]);
                if (method === 'onDateSelected') return { query: 'cal 2026-11-26' };
                if (method === 'getKeywords') return [{ keyword: 'cal' }];
                return null;
            }),
        };
        const mgr = new ExtensionManager(async () => undefined, { hostFeatures: ['date-picker'] });
        mgr._configCache = { extension_states: { mindmap: true }, extensions: {} };
        mgr.extensions.set('mindmap', {
            manifest: { id: 'mindmap', name: 'Mind Map' },
            capabilities: ['calendar'],
            sandbox,
        });

        await mgr.executeResult({ _extensionId: 'mindmap', id: 'r1' });
        await mgr._runWidgetAction(
            { extensionId: 'mindmap', widgetId: 'w', host: document.createElement('div') },
            'add'
        );
        await mgr.runToolbarClick('mindmap', 'b', {});
        await mgr.resolveDateQuery('mindmap', '2026-11-26');
        const root = document.createElement('div');
        root.innerHTML = '<button data-ext-action="go">Go</button>';
        mgr._wireExtActionsFor('mindmap', root, 'formatter');
        root.querySelector('button').click();
        await flush();

        const renderer = {
            sandbox,
            currentValues: () => ({}),
            log: { warn() {} },
        };
        await runSettingsHostSideEffect(
            renderer,
            { type: 'pick_file', action: 'import' },
            { setStatus() {} },
            async () => ({ filename: 'a.txt', text: 'x' })
        );

        const gestures = Object.fromEntries(opts.filter(([m]) => m !== 'getKeywords'));
        expect(gestures).toEqual({
            execute: true,
            onWidgetAction: true,
            onToolbarClick: true,
            onDateSelected: true,
            onResultAction: true,
            onFileSelected: true,
        });
        expect(opts.find(([m]) => m === 'getKeywords')).toEqual(['getKeywords', false]);
    });
});

// --- Pausable timers ----------------------------------------------------------------

describe('click RPC timers', () => {
    it('rpcTimer pauses and resumes with the time it had left', () => {
        const fired = vi.fn();
        const timer = rpcTimer(10_000, fired, { pausable: true });
        vi.advanceTimersByTime(4_000);
        timer.pause();
        vi.advanceTimersByTime(60_000);
        expect(fired).not.toHaveBeenCalled();
        timer.resume();
        vi.advanceTimersByTime(5_999);
        expect(fired).not.toHaveBeenCalled();
        vi.advanceTimersByTime(1);
        expect(fired).toHaveBeenCalledTimes(1);
    });

    it('a timer that is not pausable ignores pause', () => {
        const fired = vi.fn();
        const timer = rpcTimer(1_000, fired);
        timer.pause();
        vi.advanceTimersByTime(1_000);
        expect(fired).toHaveBeenCalledTimes(1);
    });

    it('pause while a pick runs: the click RPC survives a long dialog, others time out', async () => {
        const rust = fakeRust();
        broker();
        const { sb } = makeSandbox(rust.invoke);
        const execute = sb.call('execute', {}, { userGesture: true });
        const match = sb.call('match', {});
        const executeDone = vi.fn();
        const matchFailed = vi.fn();
        execute.then(executeDone, () => executeDone('rejected'));
        match.catch(matchFailed);

        vi.advanceTimersByTime(3_000);
        const run = pick(sb);
        await flush();
        expect(rust.picks).toHaveLength(1);
        // The user browses folders for 40 s.
        await vi.advanceTimersByTimeAsync(40_000);
        expect(matchFailed).toHaveBeenCalledTimes(1);
        expect(executeDone).not.toHaveBeenCalled();

        rust.picks[0].resolve(FILE);
        await run;
        // 7 s were left when the pick started.
        await vi.advanceTimersByTimeAsync(6_999);
        expect(executeDone).not.toHaveBeenCalled();
        await vi.advanceTimersByTimeAsync(1);
        expect(executeDone).toHaveBeenCalledWith('rejected');
    });
});

// --- With a click lease -------------------------------------------------------------------

describe('a request after a click', () => {
    it('opens the dialog at once with the host identity, guard first', async () => {
        const rust = fakeRust();
        const { order, emit } = broker();
        rust.invoke.mockImplementation((command, args) => {
            order.push(command);
            if (command === 'pick_file_text') {
                expect(args).toEqual({
                    args: { formats: ['pdf'] },
                    extensionId: 'mindmap',
                    displayName: 'Mind Map',
                });
                const d = deferred();
                rust.picks.push(d);
                return d.promise;
            }
            return Promise.resolve(true);
        });
        const { sb, sent } = makeSandbox(rust.invoke);
        sb.call('execute', {}, { userGesture: true }).catch(() => {});
        const run = pick(sb, {
            args: { formats: ['pdf'] },
            extensionId: 'other',
            displayName: 'Trust me',
        });
        await flush();
        expect(askBar()).toBeNull();
        expect(order).toEqual(['begin', 'pick_file_text']);
        emit({ phase: 'dialog_closed', picked: true });
        await flush();
        expect(order).toEqual(['begin', 'pick_file_text', 'end']);
        rust.picks[0].resolve(FILE);
        await run;
        // `end` ran once, on dialog_closed.
        expect(order.filter((x) => x === 'end')).toHaveLength(1);
        expect(sent.at(-1)).toEqual({ type: 'invoke-response', id: 1, result: FILE });
        // The lease is used up: the next request asks.
        pick(sb, {}, 2);
        await flush();
        expect(askBar()).not.toBeNull();
    });

    it('calls end when Rust answers without a dialog_closed', async () => {
        const rust = fakeRust();
        const { order } = broker();
        const { sb, sent } = makeSandbox(rust.invoke);
        sb.call('execute', {}, { userGesture: true }).catch(() => {});
        const run = pick(sb);
        await flush();
        rust.picks[0].reject({
            kind: 'rate_limited',
            key: 'errors.file_text.too_soon',
            message: 'Wait a moment before choosing another file.',
        });
        await run;
        expect(order).toEqual(['begin', 'end']);
        expect(sent.at(-1)).toMatchObject({
            errorKind: 'rate_limited',
            errorKey: 'errors.file_text.too_soon',
        });
    });

    it('a lease older than 10 s no longer counts', async () => {
        const rust = fakeRust();
        broker();
        const { sb } = makeSandbox(rust.invoke);
        sb.call('execute', {}, { userGesture: true, timeoutMs: 0 }).catch(() => {});
        await vi.advanceTimersByTimeAsync(CLICK_LEASE_MS + 1);
        pick(sb);
        await flush();
        expect(askBar()).not.toBeNull();
        expect(rust.picks).toHaveLength(0);
    });
});

// --- Refusals -----------------------------------------------------------------------

describe('refusals', () => {
    it('no_window in a hidden window, with errorKind and errorKey', async () => {
        const rust = fakeRust();
        broker({ visible: false });
        const { sb, sent } = makeSandbox(rust.invoke);
        sb.call('execute', {}, { userGesture: true }).catch(() => {});
        await pick(sb);
        expect(rust.invoke).not.toHaveBeenCalled();
        expect(askBar()).toBeNull();
        expect(sent.at(-1)).toEqual({
            type: 'invoke-response',
            id: 1,
            error: 'Open Kage to choose a file.',
            errorKind: 'cancelled',
            errorKey: 'errors.file_text.no_window',
        });
        // The lease wasn't used: the window's state, not the click, refused.
        expect(sb.hasClickLease()).toBe(true);
    });

    it('busy while another request in this window runs, without touching the guard', async () => {
        const rust = fakeRust();
        const { order } = broker();
        const a = makeSandbox(rust.invoke);
        const b = makeSandbox(rust.invoke, { extensionId: 'notes', displayName: 'Notes' });
        a.sb.call('execute', {}, { userGesture: true }).catch(() => {});
        b.sb.call('execute', {}, { userGesture: true }).catch(() => {});
        const run = pick(a.sb);
        await flush();
        await pick(b.sb, {}, 7);
        expect(b.sent.at(-1)).toMatchObject({
            id: 7,
            errorKind: 'rate_limited',
            errorKey: 'errors.file_text.busy',
        });
        expect(b.sb.hasClickLease()).toBe(true);
        expect(order).toEqual(['begin']);
        rust.picks[0].resolve(null);
        await run;
        expect(a.sent.at(-1)).toMatchObject({ id: 1, result: null });
    });

    it('a second request while the ask bar shows is busy', async () => {
        const rust = fakeRust();
        broker();
        const a = makeSandbox(rust.invoke);
        const b = makeSandbox(rust.invoke, { extensionId: 'notes' });
        pick(a.sb);
        await flush();
        await pick(b.sb, {}, 2);
        expect(document.querySelectorAll('.kage-file-ask')).toHaveLength(1);
        expect(b.sent.at(-1)).toMatchObject({ errorKey: 'errors.file_text.busy' });
    });
});

// --- The ask bar ---------------------------------------------------------------------

describe('the ask bar', () => {
    it('names the extension, takes focus and opens the dialog on Choose file…', async () => {
        const rust = fakeRust();
        const { order } = broker();
        const input = document.createElement('input');
        document.body.appendChild(input);
        input.focus();
        const { sb, sent } = makeSandbox(rust.invoke);
        const run = pick(sb);
        await flush();
        const bar = askBar();
        expect(bar.getAttribute('role')).toBe('alertdialog');
        const label = document.getElementById(bar.getAttribute('aria-labelledby'));
        expect(label.textContent).toBe('Mind Map wants to read a file you choose.');
        const bdi = label.querySelector('strong > bdi');
        expect(bdi.textContent).toBe('Mind Map');
        expect(bdi.dir).toBe('auto');
        expect(bar.querySelector('.kage-file-ask-icon').textContent).toBe('🧠');
        const choose = bar.querySelector('.kage-file-ask-choose');
        expect(choose.textContent).toBe('Choose file…');
        expect(bar.querySelector('.kage-file-ask-dismiss').textContent).toBe('Not now');
        expect(document.activeElement).toBe(choose);

        // A click that lands as the bar appears doesn't count.
        choose.click();
        await flush();
        expect(rust.picks).toHaveLength(0);
        await vi.advanceTimersByTimeAsync(ASK_BAR_ARM_MS);
        choose.click();
        await flush();
        expect(askBar()).toBeNull();
        expect(document.activeElement).toBe(input);
        expect(order).toEqual(['begin']);
        expect(rust.picks).toHaveLength(1);
        rust.picks[0].resolve(FILE);
        await run;
        expect(sent.at(-1)).toMatchObject({ result: FILE });
    });

    it('Not now resolves null and starts the cooldown', async () => {
        const rust = fakeRust();
        broker();
        const { sb, sent } = makeSandbox(rust.invoke);
        const run = pick(sb);
        await flush();
        askBar().querySelector('.kage-file-ask-dismiss').click();
        await run;
        expect(askBar()).toBeNull();
        expect(sent.at(-1)).toEqual({ type: 'invoke-response', id: 1, result: null });
        expect(rust.invoke).not.toHaveBeenCalled();

        await pick(sb, {}, 2);
        expect(sent.at(-1)).toMatchObject({
            id: 2,
            error: 'You chose "Not now". Try again in a moment.',
            errorKind: 'rate_limited',
            errorKey: 'errors.file_text.declined',
        });
        expect(askBar()).toBeNull();

        // A click still opens the dialog during the cooldown.
        sb.call('execute', {}, { userGesture: true }).catch(() => {});
        pick(sb, {}, 3);
        await flush();
        expect(rust.picks).toHaveLength(1);
        rust.picks[0].resolve(null);
        await flush();

        // Once it's over, the bar shows again.
        await vi.advanceTimersByTimeAsync(DECLINE_COOLDOWN_MS);
        pick(sb, {}, 4);
        await flush();
        expect(askBar()).not.toBeNull();
    });

    it('Escape is Not now', async () => {
        const rust = fakeRust();
        broker();
        const { sb, sent } = makeSandbox(rust.invoke);
        const run = pick(sb);
        await flush();
        const outside = vi.fn();
        document.addEventListener('keydown', outside);
        askBar().dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
        await run;
        document.removeEventListener('keydown', outside);
        expect(outside).not.toHaveBeenCalled();
        expect(sent.at(-1)).toMatchObject({ result: null });
        await pick(sb, {}, 2);
        expect(sent.at(-1)).toMatchObject({ errorKey: 'errors.file_text.declined' });
    });

    it('goes after 60 s without an answer', async () => {
        const rust = fakeRust();
        broker();
        const { sb, sent } = makeSandbox(rust.invoke);
        const run = pick(sb);
        await vi.advanceTimersByTimeAsync(ASK_BAR_TIMEOUT_MS - 1);
        expect(askBar()).not.toBeNull();
        await vi.advanceTimersByTimeAsync(1);
        await run;
        expect(askBar()).toBeNull();
        expect(sent.at(-1)).toMatchObject({ result: null });
        await pick(sb, {}, 2);
        expect(sent.at(-1)).toMatchObject({ errorKey: 'errors.file_text.declined' });
    });

    it('cancel_file_text from the extension withdraws it, without Rust', async () => {
        const rust = fakeRust();
        broker();
        const { sb, sent } = makeSandbox(rust.invoke);
        const run = pick(sb);
        await flush();
        await sb._handleInvoke({ id: 9, command: 'cancel_file_text', args: {} });
        await run;
        expect(askBar()).toBeNull();
        expect(rust.invoke).not.toHaveBeenCalled();
        expect(sent).toEqual([
            { type: 'invoke-response', id: 9, result: true },
            { type: 'invoke-response', id: 1, result: null },
        ]);
        // Withdrawn isn't turned down: no cooldown.
        pick(sb, {}, 2);
        await flush();
        expect(askBar()).not.toBeNull();
    });

    it('goes with its sandbox', async () => {
        const rust = fakeRust();
        broker();
        const { sb } = makeSandbox(rust.invoke);
        pick(sb);
        await flush();
        sb.destroy();
        await flush();
        expect(askBar()).toBeNull();
        expect(rust.invoke).not.toHaveBeenCalled();
    });

    it('in a window with a mount, goes at the top of it and reports the layout', async () => {
        const rust = fakeRust();
        const mount = document.createElement('div');
        mount.innerHTML = '<p>first</p>';
        document.body.appendChild(mount);
        const onLayoutChange = vi.fn();
        installFileTextBroker({ isVisible: () => true, mount: () => mount, onLayoutChange, t: tr });
        const { sb } = makeSandbox(rust.invoke);
        const run = pick(sb);
        await flush();
        expect(mount.firstElementChild.className).toBe('kage-file-ask');
        expect(onLayoutChange).toHaveBeenCalledTimes(1);
        askBar().querySelector('.kage-file-ask-dismiss').click();
        await run;
        expect(onLayoutChange).toHaveBeenCalledTimes(2);
        expect(document.getElementById('kage-file-text-layer')).toBeNull();
    });
});

// --- Progress chip -----------------------------------------------------------------

describe('the progress chip', () => {
    async function started() {
        const rust = fakeRust();
        const b = broker();
        const { sb, sent } = makeSandbox(rust.invoke);
        sb.call('execute', {}, { userGesture: true, timeoutMs: 0 }).catch(() => {});
        const run = pick(sb);
        await flush();
        b.emit({ phase: 'dialog_closed', picked: true });
        return { rust, ...b, sb, sent, run };
    }

    it('shows after 400 ms of reading, with the name isolated, and goes on done', async () => {
        const { rust, emit, run } = await started();
        emit({ phase: 'reading', file: 'Q3 plan.pdf' });
        await vi.advanceTimersByTimeAsync(PROGRESS_CHIP_DELAY_MS - 1);
        expect(chip()).toBeNull();
        await vi.advanceTimersByTimeAsync(1);
        expect(chip().querySelector('.kage-file-chip-text').textContent).toBe('Reading Q3 plan.pdf…');
        expect(chip().querySelector('bdi').dir).toBe('auto');
        emit({ phase: 'extracting', file: 'Q3 plan.pdf' });
        expect(chip().textContent).toContain('Reading Q3 plan.pdf…');
        emit({ phase: 'done', file: 'Q3 plan.pdf' });
        expect(chip()).toBeNull();
        rust.picks[0].resolve(FILE);
        await run;
    });

    it('never shows for a quick read', async () => {
        const { rust, emit, run } = await started();
        emit({ phase: 'reading', file: 'a.txt' });
        await vi.advanceTimersByTimeAsync(100);
        emit({ phase: 'done', file: 'a.txt' });
        rust.picks[0].resolve(FILE);
        await run;
        await vi.advanceTimersByTimeAsync(1_000);
        expect(chip()).toBeNull();
    });

    it('says Downloading while a cloud drive fetches the file', async () => {
        const { rust, emit, run } = await started();
        emit({ phase: 'downloading', file: 'Q3 plan.pdf' });
        await vi.advanceTimersByTimeAsync(PROGRESS_CHIP_DELAY_MS);
        expect(chip().textContent).toContain('Downloading Q3 plan.pdf…');
        rust.picks[0].resolve(FILE);
        await run;
        expect(chip()).toBeNull();
    });

    it('Cancel calls cancel_file_text for this extension, once', async () => {
        const { rust, emit, run, sent } = await started();
        emit({ phase: 'extracting', file: 'big.pdf' });
        await vi.advanceTimersByTimeAsync(PROGRESS_CHIP_DELAY_MS);
        const cancel = chip().querySelector('.kage-file-chip-cancel');
        expect(cancel.textContent).toBe('Cancel');
        cancel.click();
        cancel.click();
        expect(rust.invoke.mock.calls.filter(([c]) => c === 'cancel_file_text')).toEqual([
            ['cancel_file_text', { extensionId: 'mindmap' }],
        ]);
        rust.picks[0].reject({
            kind: 'cancelled',
            key: 'errors.file_text.cancelled',
            message: 'Reading big.pdf was cancelled.',
        });
        await run;
        expect(chip()).toBeNull();
        expect(sent.at(-1)).toMatchObject({ errorKey: 'errors.file_text.cancelled' });
    });

    it("ignores another extension's progress", async () => {
        const { rust, emit, run } = await started();
        emit({ phase: 'reading', file: 'x.txt' }, 'notes');
        await vi.advanceTimersByTimeAsync(PROGRESS_CHIP_DELAY_MS);
        expect(chip()).toBeNull();
        rust.picks[0].resolve(null);
        await run;
    });
});

// --- Destroy, guard and the view rule -------------------------------------------------

describe('destroy, the guard and views', () => {
    it('a sandbox destroyed mid-request cancels it in Rust', async () => {
        const rust = fakeRust();
        const { order, emit } = broker();
        const { sb, sent } = makeSandbox(rust.invoke);
        sb.call('execute', {}, { userGesture: true, timeoutMs: 0 }).catch(() => {});
        const run = pick(sb);
        await flush();
        emit({ phase: 'dialog_closed', picked: true });
        emit({ phase: 'reading', file: 'a.pdf' });
        await vi.advanceTimersByTimeAsync(PROGRESS_CHIP_DELAY_MS);
        expect(chip()).not.toBeNull();
        sb.destroy();
        expect(chip()).toBeNull();
        expect(rust.invoke).toHaveBeenCalledWith('cancel_file_text', { extensionId: 'mindmap' });
        rust.picks[0].resolve(null);
        await run;
        expect(order).toEqual(['begin', 'end']);
        // The port is gone: nothing was posted after the destroy.
        expect(sent.some((m) => m.type === 'invoke-response')).toBe(false);
    });

    it('the guard ends before the answer reaches the sandbox', async () => {
        const rust = fakeRust();
        const events = [];
        const { sb } = makeSandbox(rust.invoke);
        sb._port.postMessage = (m) => events.push(m.type);
        broker({
            guard: {
                begin: async () => events.push('begin'),
                end: async () => {
                    await Promise.resolve();
                    events.push('end');
                },
            },
        });
        sb.call('execute', {}, { userGesture: true, timeoutMs: 0 }).catch(() => {});
        const run = pick(sb);
        await flush();
        rust.picks[0].resolve(FILE);
        await run;
        expect(events).toEqual(['rpc', 'begin', 'end', 'invoke-response']);
    });

    it('a view request counts as a click with a recent user activation', async () => {
        const rust = fakeRust();
        let active = true;
        broker({ activationLease: () => active });
        const { sb } = makeSandbox(rust.invoke);
        sb.surface = 'view';
        const run = pick(sb);
        await flush();
        expect(askBar()).toBeNull();
        expect(rust.picks).toHaveLength(1);
        rust.picks[0].resolve(null);
        await run;

        active = false;
        pick(sb, {}, 2);
        await flush();
        expect(askBar()).not.toBeNull();
    });

    it('a click on Not now, Choose file… or Cancel opens no dialog in a view', async () => {
        const rust = fakeRust();
        let active = false;
        const { emit } = broker({ activationLease: () => active });
        const { sb, sent } = makeSandbox(rust.invoke);
        sb.surface = 'view';

        // No activation: the bar. The click on Not now activates the page.
        const first = pick(sb);
        await flush();
        active = true;
        askBar().querySelector('.kage-file-ask-dismiss').click();
        await first;
        // The extension asks again at once: still Not now, no dialog.
        pick(sb, {}, 2);
        await flush();
        expect(rust.picks).toHaveLength(0);
        expect(sent.at(-1)).toMatchObject({ id: 2, errorKey: 'errors.file_text.declined' });

        // Choose file… opens one dialog; asking again right after it
        // closed shows the bar.
        await vi.advanceTimersByTimeAsync(DECLINE_COOLDOWN_MS);
        active = false;
        const third = pick(sb, {}, 3);
        await flush();
        active = true;
        await vi.advanceTimersByTimeAsync(ASK_BAR_ARM_MS);
        askBar().querySelector('.kage-file-ask-choose').click();
        await flush();
        expect(rust.picks).toHaveLength(1);
        rust.picks[0].resolve(null);
        await third;
        pick(sb, {}, 4);
        await flush();
        expect(rust.picks).toHaveLength(1);
        expect(askBar()).not.toBeNull();
        askBar().querySelector('.kage-file-ask-dismiss').click();
        await flush();

        // The chip's Cancel: the next request doesn't open the dialog.
        await vi.advanceTimersByTimeAsync(DECLINE_COOLDOWN_MS);
        const fifth = pick(sb, {}, 5);
        await flush();
        expect(rust.picks).toHaveLength(2);
        emit({ phase: 'dialog_closed', picked: true });
        emit({ phase: 'extracting', file: 'big.pdf' });
        await vi.advanceTimersByTimeAsync(USER_ACTIVATION_MS);
        chip().querySelector('.kage-file-chip-cancel').click();
        rust.picks[1].resolve(null);
        await fifth;
        pick(sb, {}, 6);
        await flush();
        expect(rust.picks).toHaveLength(2);
        expect(askBar()).not.toBeNull();
    });

    it('one activation in a view opens one dialog', async () => {
        const rust = fakeRust();
        broker({ activationLease: () => true });
        const { sb } = makeSandbox(rust.invoke);
        sb.surface = 'view';
        const first = pick(sb);
        await flush();
        expect(rust.picks).toHaveLength(1);
        rust.picks[0].resolve(null);
        await first;

        // The user closed the dialog at once, and the activation still holds.
        const second = pick(sb, {}, 2);
        await flush();
        expect(rust.picks).toHaveLength(1);
        expect(askBar()).not.toBeNull();
        askBar().querySelector('.kage-file-ask-dismiss').click();
        await second;

        // A later activation counts again (a click lease beats the cooldown).
        await vi.advanceTimersByTimeAsync(USER_ACTIVATION_MS);
        pick(sb, {}, 3);
        await flush();
        expect(rust.picks).toHaveLength(2);
    });

    it('the activation rule is for views only', async () => {
        const rust = fakeRust();
        broker({ activationLease: () => true });
        const { sb } = makeSandbox(rust.invoke);
        pick(sb);
        await flush();
        expect(askBar()).not.toBeNull();
        expect(rust.picks).toHaveLength(0);
    });
});
