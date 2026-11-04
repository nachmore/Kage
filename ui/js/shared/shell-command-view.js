/**
 * Shell command blocks — the DOM side of shell-commands.js, shared by the
 * floating and chat windows.
 *
 * A block is a terminal-style row: `$ command` (one line, ellipsised) with a
 * status mark (spinner / check / cross + exit code), and a real <button>
 * header that expands a panel with the full command, the output (stdout,
 * then stderr), the exit status and copy buttons.
 *
 * Blocks are keyed by tool call id and updated in place, never rebuilt, so
 * an expanded panel (and its scroll position) survives later updates and
 * streaming re-renders. They live outside the markdown render target for
 * the same reason:
 *   - the "activity slot" (`showShellActivity` & co.) is where a running
 *     command replaces the generic tool spinner while the turn streams;
 *   - `renderToolSources` renders the tool/source chip row, where all of a
 *     response's shell calls collapse into one "shell ×N" toggle chip that
 *     reveals the list of commands.
 *
 * Command and output are untrusted: they only ever reach the DOM through
 * textContent / text nodes. Expanding or collapsing dispatches a bubbling
 * SHELL_LAYOUT_EVENT so a window that sizes itself to its content (the
 * floating window) can re-measure.
 */

import { t } from './i18n.js';
import {
    getShellCalls,
    joinCapped,
    settleShellCalls,
    shellCallState,
    shellOutputOf,
} from './shell-commands.js';
import { renderSourceChipsHtml, renderToolChipsHtml } from './streaming-utils.js';

/** Bubbling event fired from a block or the chip when its height changes. */
export const SHELL_LAYOUT_EVENT = 'kage:shell-layout';

// Constant, trusted SVG markup only — never interpolate data into these.
const ICONS = Object.freeze({
    terminal: '<polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/>',
    check: '<polyline points="20 6 9 17 4 12"/>',
    cross: '<line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>',
    dash: '<circle cx="12" cy="12" r="9"/><line x1="8" y1="12" x2="16" y2="12"/>',
    chevron: '<polyline points="9 18 15 12 9 6"/>',
    copy: '<rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>',
});

let _uid = 0;
const COPIED_FLASH_MS = 1500;
const TOOLTIP_MAX = 500;

function el(tag, className) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    return node;
}

function icon(name, className, size = 14) {
    const span = el('span', className || 'shell-cmd-icon');
    span.setAttribute('aria-hidden', 'true');
    span.innerHTML = `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${ICONS[name]}</svg>`;
    return span;
}

function layoutChanged(node) {
    node.dispatchEvent(new CustomEvent(SHELL_LAYOUT_EVENT, { bubbles: true }));
}

function omittedMarker(count) {
    return t('shared.shell.omitted', { count });
}

function displayCommand(record) {
    return record?.command || record?.title || t('shared.shell.unknown_command');
}

/** stdout then stderr as one string (omission markers included). */
export function shellOutputText(record) {
    const out = shellOutputOf(record);
    if (!out) return '';
    return [joinCapped(out.stdout, omittedMarker), joinCapped(out.stderr, omittedMarker)]
        .filter(Boolean)
        .join('\n');
}

function writeClipboard(text) {
    try {
        const p = navigator.clipboard?.writeText(text);
        return p ? p.then(() => true).catch(() => false) : Promise.resolve(false);
    } catch {
        return Promise.resolve(false);
    }
}

function copyButton(getLabel, getText) {
    const btn = el('button', 'shell-cmd-copy');
    btn.type = 'button';
    const label = el('span', 'shell-cmd-copy-label');
    label.dir = 'auto';
    label.textContent = getLabel();
    btn.append(icon('copy', 'shell-cmd-icon', 12), label);
    btn.addEventListener('click', (e) => {
        e.stopPropagation();
        const text = getText();
        if (!text) return;
        writeClipboard(text).then((ok) => {
            if (!ok) return;
            label.textContent = t('shared.shell.copied');
            btn.classList.add('copied');
            clearTimeout(btn._kageCopiedTimer);
            btn._kageCopiedTimer = setTimeout(() => {
                label.textContent = getLabel();
                btn.classList.remove('copied');
            }, COPIED_FLASH_MS);
        });
    });
    return btn;
}

function renderStatus(statusEl, state, out) {
    statusEl.textContent = '';
    statusEl.dataset.state = state;
    const code = out?.exitCode ?? null;
    let label;
    if (state === 'running') {
        const spinner = el('span', 'shell-cmd-spinner');
        spinner.setAttribute('aria-hidden', 'true');
        statusEl.append(spinner);
        label = t('shared.shell.status.running');
    } else if (state === 'success') {
        statusEl.append(icon('check'));
        label = t('shared.shell.status.success');
    } else if (state === 'failed') {
        statusEl.append(icon('cross'));
        if (code !== null) {
            const codeEl = el('span', 'shell-cmd-code');
            codeEl.setAttribute('aria-hidden', 'true');
            codeEl.textContent = t('shared.shell.exit_short', { code });
            statusEl.append(codeEl);
            label = t('shared.shell.status.failed_code', { code });
        } else {
            label = t('shared.shell.status.failed');
        }
    } else {
        statusEl.append(icon('dash'));
        label = t('shared.shell.status.unknown');
    }
    statusEl.setAttribute('aria-label', label);
    statusEl.title = out?.exitSignal ? `${label} — ${out.exitSignal}` : label;
}

function segmentPre(className, seg) {
    const pre = el('pre', className);
    pre.append(document.createTextNode(seg.head));
    if (seg.omitted) {
        const marker = el('span', 'shell-cmd-omitted');
        marker.dir = 'auto';
        marker.textContent = omittedMarker(seg.omitted);
        pre.append(marker);
    }
    if (seg.tail) pre.append(document.createTextNode(seg.tail));
    return pre;
}

function buildPanel(root) {
    const st = root._kageShell;
    const full = el('pre', 'shell-cmd-full');
    const prompt = el('span', 'shell-cmd-prompt');
    prompt.setAttribute('aria-hidden', 'true');
    prompt.textContent = '$ ';
    const fullText = document.createTextNode('');
    full.append(prompt, fullText);

    const output = el('div', 'shell-cmd-output');
    output.tabIndex = 0;
    output.setAttribute('role', 'group');
    output.setAttribute('aria-label', t('shared.shell.output_label'));

    const footer = el('div', 'shell-cmd-footer');
    const exit = el('span', 'shell-cmd-exit');
    exit.dir = 'auto';
    const actions = el('span', 'shell-cmd-actions');
    const copyCommand = copyButton(
        () => t('shared.shell.copy_command'),
        () => st.record?.command || ''
    );
    const copyOutput = copyButton(
        () => t('shared.shell.copy_output'),
        () => shellOutputText(st.record)
    );
    actions.append(copyCommand, copyOutput);
    footer.append(exit, actions);

    st.panel.append(full, output, footer);
    st.parts = { full, fullText, output, exit, copyCommand, copyOutput };
}

function renderPanel(root) {
    const st = root._kageShell;
    if (!st.parts) buildPanel(root);
    const { full, fullText, output, exit, copyCommand, copyOutput } = st.parts;
    const record = st.record;
    const state = shellCallState(record);
    const out = shellOutputOf(record);

    fullText.data = displayCommand(record);
    full.classList.toggle('is-placeholder', !record.command);
    copyCommand.hidden = !record.command;

    // Keep the reader's place when output grows under an open panel.
    const prevTop = output.scrollTop;
    output.textContent = '';
    if (out?.stdout) output.append(segmentPre('shell-cmd-stdout', out.stdout));
    if (out?.stderr) {
        const pre = segmentPre('shell-cmd-stderr', out.stderr);
        const tag = el('span', 'shell-cmd-stream-label');
        tag.textContent = t('shared.shell.stderr_label');
        pre.prepend(tag);
        output.append(pre);
    }
    const hasOutput = !!(out?.stdout || out?.stderr);
    if (!hasOutput) {
        // Translated text inside an LTR block: let it take its own direction.
        const msg = el('div', 'shell-cmd-empty');
        msg.dir = 'auto';
        if (out?.dropped) msg.textContent = t('shared.shell.output_dropped');
        else if (out?.terminal) msg.textContent = t('shared.shell.output_terminal');
        else if (state === 'running') msg.textContent = t('shared.shell.still_running');
        else msg.textContent = t('shared.shell.no_output');
        output.append(msg);
    }
    output.scrollTop = prevTop;
    copyOutput.hidden = !hasOutput;

    let exitLabel = '';
    if (out?.exitCode !== null && out?.exitCode !== undefined) {
        exitLabel = t('shared.shell.exit_code', { code: out.exitCode });
    } else if (out?.exitSignal) {
        exitLabel = out.exitSignal;
    }
    exit.textContent = exitLabel;
    exit.hidden = !exitLabel;
    st.panelRev = record.rev;
}

/** Whether a block's details panel is open. */
export function isShellCommandExpanded(root) {
    return root?._kageShell?.header.getAttribute('aria-expanded') === 'true';
}

/** Open or close a block's details panel. */
export function setShellCommandExpanded(root, open) {
    const st = root?._kageShell;
    if (!st) return;
    const want = !!open;
    if (want === isShellCommandExpanded(root)) return;
    st.header.setAttribute('aria-expanded', String(want));
    st.panel.hidden = !want;
    root.classList.toggle('expanded', want);
    if (want && st.panelRev !== st.record.rev) renderPanel(root);
    layoutChanged(root);
}

/** Repaint a block from (a newer revision of) its record. */
export function updateShellCommandBlock(root, record) {
    const st = root?._kageShell;
    if (!st || !record) return;
    st.record = record;
    const state = shellCallState(record);
    if (st.renderedRev === record.rev && st.renderedState === state) return;
    const out = shellOutputOf(record);
    root.dataset.state = state;
    const command = displayCommand(record);
    st.text.textContent = command;
    st.text.classList.toggle('is-placeholder', !record.command);
    st.header.title = command.length > TOOLTIP_MAX ? `${command.slice(0, TOOLTIP_MAX)}…` : command;
    renderStatus(st.status, state, out);
    if (isShellCommandExpanded(root)) renderPanel(root);
    st.renderedRev = record.rev;
    st.renderedState = state;
}

/** A new block for `record` (collapsed). */
export function createShellCommandBlock(record) {
    const id = `kage-shell-${++_uid}`;
    const root = el('div', 'shell-cmd');
    root.dir = 'ltr';

    const header = el('button', 'shell-cmd-header');
    header.type = 'button';
    header.id = `${id}-toggle`;
    header.setAttribute('aria-expanded', 'false');
    header.setAttribute('aria-controls', `${id}-panel`);
    const prompt = el('span', 'shell-cmd-prompt');
    prompt.setAttribute('aria-hidden', 'true');
    prompt.textContent = '$';
    const text = el('span', 'shell-cmd-text');
    const status = el('span', 'shell-cmd-status');
    status.setAttribute('role', 'img');
    header.append(prompt, text, status, icon('chevron', 'shell-cmd-chevron', 12));

    const panel = el('div', 'shell-cmd-panel');
    panel.id = `${id}-panel`;
    panel.hidden = true;
    panel.setAttribute('role', 'region');
    panel.setAttribute('aria-labelledby', header.id);

    root.append(header, panel);
    root._kageShell = {
        record,
        header,
        text,
        status,
        panel,
        parts: null,
        renderedRev: -1,
        renderedState: null,
        panelRev: -1,
    };
    header.addEventListener('click', () => {
        setShellCommandExpanded(root, !isShellCommandExpanded(root));
    });
    updateShellCommandBlock(root, record);
    return root;
}

function blocksOf(container) {
    if (!container) return [];
    return Array.from(container.children).filter((c) => c._kageShell);
}

function findBlock(container, toolCallId) {
    return blocksOf(container).find((b) => b._kageShell.record.toolCallId === toolCallId) || null;
}

// --- Activity slot ---------------------------------------------------------

/** An empty container for running-command blocks. Hosts place it. */
export function createShellActivitySlot() {
    const slot = el('div', 'shell-activity');
    slot.dir = 'ltr';
    return slot;
}

/**
 * Drop blocks that have nothing more to show live: finished and collapsed
 * (they stay listed under the response's "shell" chip). Running and
 * expanded blocks stay. `keepId` is never dropped.
 */
export function pruneShellActivity(slot, keepId = null) {
    for (const b of blocksOf(slot)) {
        const rec = b._kageShell.record;
        if (rec.toolCallId === keepId) continue;
        if (isShellCommandExpanded(b) || shellCallState(rec) === 'running') continue;
        b.remove();
    }
}

/**
 * Show `record` in the activity slot (adding or updating its block). The
 * newest command replaces finished, collapsed ones — the way the generic
 * tool spinner line used to be replaced by the next tool.
 */
export function showShellActivity(slot, record) {
    if (!slot || !record) return null;
    let block = findBlock(slot, record.toolCallId);
    if (block) {
        updateShellCommandBlock(block, record);
    } else {
        block = createShellCommandBlock(record);
        slot.append(block);
    }
    pruneShellActivity(slot, record.toolCallId);
    return block;
}

/**
 * The turn ended: calls that never reported a terminal status stop showing
 * as running, every block repaints and finished collapsed ones go.
 */
export function finishShellActivity(slot, toolUsages) {
    settleShellCalls(toolUsages);
    for (const b of blocksOf(slot)) updateShellCommandBlock(b, b._kageShell.record);
    pruneShellActivity(slot);
}

// --- Chip row --------------------------------------------------------------

function aggregateState(calls) {
    let failed = false;
    for (const rec of calls) {
        const s = shellCallState(rec);
        if (s === 'running') return 'running';
        if (s === 'failed') failed = true;
    }
    return failed ? 'failed' : 'done';
}

function createShellChip(listId) {
    const chip = el('button', 'source-chip tool-chip shell-chip');
    chip.type = 'button';
    chip.setAttribute('aria-expanded', 'false');
    chip.setAttribute('aria-controls', listId);
    const label = el('span', 'source-domain shell-chip-label');
    const count = el('span', 'tool-chip-count');
    const dot = el('span', 'shell-chip-dot');
    dot.setAttribute('aria-hidden', 'true');
    chip.append(
        icon('terminal', 'tool-chip-icon shell-chip-icon', 14),
        label,
        count,
        dot,
        icon('chevron', 'shell-chip-chevron', 10)
    );
    return chip;
}

/**
 * Keep the "shell ×N" toggle chip and its command list in `container` (a
 * `.tool-sources` row) in sync with `toolUsages`. Blocks are reconciled by
 * tool call id, so open panels and the list's open state survive.
 */
export function syncShellCommandList(container, toolUsages) {
    if (!container) return;
    const calls = getShellCalls(toolUsages);
    let chip = container.querySelector(':scope > .shell-chip');
    let list = container.querySelector(':scope > .shell-cmd-list');
    if (!calls.length) {
        chip?.remove();
        list?.remove();
        return;
    }
    if (!chip || !list) {
        chip?.remove();
        list?.remove();
        const listId = `kage-shell-list-${++_uid}`;
        chip = createShellChip(listId);
        list = el('div', 'shell-cmd-list');
        list.id = listId;
        list.hidden = true;
        const chipRef = chip;
        const listRef = list;
        chip.addEventListener('click', () => {
            const open = chipRef.getAttribute('aria-expanded') !== 'true';
            chipRef.setAttribute('aria-expanded', String(open));
            listRef.hidden = !open;
            layoutChanged(chipRef);
        });
        container.prepend(chip);
        container.append(list);
    }

    const n = calls.length;
    chip.querySelector('.shell-chip-label').textContent = t('shared.shell.chip_label');
    chip.querySelector('.tool-chip-count').textContent = n > 1 ? `×${n}` : '';
    const title = t('shared.shell.chip_title', { count: n });
    chip.title = title;
    chip.setAttribute('aria-label', title);
    chip.dataset.state = aggregateState(calls);
    list.setAttribute('aria-label', title);

    const existing = new Map(blocksOf(list).map((b) => [b._kageShell.record.toolCallId, b]));
    calls.forEach((rec, i) => {
        let block = existing.get(rec.toolCallId);
        if (block) {
            existing.delete(rec.toolCallId);
            updateShellCommandBlock(block, rec);
        } else {
            block = createShellCommandBlock(rec);
        }
        const at = list.children[i] || null;
        if (at !== block) list.insertBefore(block, at);
    });
    for (const stale of existing.values()) stale.remove();
}

/**
 * Render a response's tool/source chip row into `container`: non-shell
 * tool chips and source chips exactly as before (one wrapper, re-rendered
 * each time), plus the persistent shell chip and command list. Pair with
 * `attachSourceClickHandler(container, invoke)` for the source links.
 */
export function renderToolSources(container, toolUsages, toolSources) {
    if (!container) return;
    const usages = Array.isArray(toolUsages) ? toolUsages : [];
    let chips = container.querySelector(':scope > .tool-sources-static');
    if (!chips) {
        // Anything else in here predates this renderer (plain innerHTML).
        container.textContent = '';
        chips = el('div', 'tool-sources-static');
        container.append(chips);
    }
    chips.innerHTML =
        renderToolChipsHtml(usages.filter((u) => !u?.shell)) +
        renderSourceChipsHtml(Array.isArray(toolSources) ? toolSources : []);
    syncShellCommandList(container, usages);
}
