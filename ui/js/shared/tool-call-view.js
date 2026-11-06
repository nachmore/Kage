/**
 * Tool call blocks and the tool chip row — the DOM side of tool-calls.js,
 * shared by the floating window, the chat window and reopened sessions.
 *
 * Every tool chip under a response ("read ×3", "builder-mcp ×2", …) is a
 * toggle for the list of that response's calls of that tool (or MCP
 * server, or kind); each call is a block like the shell command block: a
 * one-line <button> header (what the call was about, status mark, chevron)
 * that expands a panel with per-kind details:
 *
 *   - edit / delete / move: the file, a compact unified diff (ACP diff
 *     content, or the edit tool's own arguments), other arguments.
 *   - read: the file(s), the line range when the agent says so, the
 *     content returned.
 *   - search: what was searched for and where, the result sources
 *     (web results, matching files) as text, else the output.
 *   - fetch: the URL(s) and what came back.
 *   - MCP tools: server and tool, arguments as key / value (nested values
 *     as collapsed pretty JSON), the result as text or collapsible JSON.
 *   - think / other: arguments and output.
 *
 * Shell commands are the `execute` kind of the same row and keep their own
 * terminal block (shell-command-view.js). Blocks and chips are reconciled
 * by tool call id / chip, never rebuilt, so open lists and panels survive
 * later updates and streaming re-renders; a closed panel holds no output
 * in the DOM.
 *
 * All tool data is untrusted: it reaches the DOM only as text nodes. Paths,
 * URLs, queries, arguments and diff lines are shown with hidden characters
 * made visible (`appendShellCommandText`); output was sanitised when it was
 * stored. Nothing is linkified — a URL is text. Blocks are `dir="ltr"`;
 * translated labels inside them take their own direction.
 */

import { t } from './i18n.js';
import { chipStateLabel, copyButton, el, icon, layoutChanged, uid } from './tool-call-dom.js';
import { appendShellCommandText, syncShellCommandList } from './shell-command-view.js';
import { joinCapped, shellCommandDisplayText } from './shell-commands.js';
import { renderSourceChipsHtml, renderToolChipsHtml } from './streaming-utils.js';
import {
    fetchUrlsOf,
    groupToolCalls,
    JSON_CUT,
    readOperationsShown,
    readRanges,
    searchQueryOf,
    searchSourcesOf,
    toolCallPaths,
    toolCallState,
    toolOutputOf,
} from './tool-calls.js';

const HEADER_MAX = 400;
const TOOLTIP_MAX = 500;
const PREVIEW_MAX = 120;
// A JSON result this many lines or shorter starts expanded.
const JSON_OPEN_LINES = 12;

const CATEGORY_ICONS = Object.freeze({
    edit: 'edit',
    read: 'read',
    search: 'search',
    fetch: 'fetch',
    mcp: 'mcp',
    think: 'think',
    other: 'other',
});

function clip(s, max) {
    if (s.length <= max) return s;
    const c = s.charCodeAt(max - 1);
    return s.slice(0, c >= 0xd800 && c <= 0xdbff ? max - 1 : max);
}

function oneLine(s) {
    return String(s).replace(/\s+/g, ' ').trim();
}

function omittedMarker(count) {
    return t('shared.shell.omitted', { count });
}

/** A translated label (its own direction inside the LTR block). */
function label(className, text) {
    const node = el('span', className);
    node.dir = 'auto';
    node.textContent = text;
    return node;
}

function kindLabel(kind) {
    switch (kind) {
        case 'read':
            return t('shared.tool.kind.read');
        case 'edit':
            return t('shared.tool.kind.edit');
        case 'delete':
            return t('shared.tool.kind.delete');
        case 'move':
            return t('shared.tool.kind.move');
        case 'search':
            return t('shared.tool.kind.search');
        case 'fetch':
            return t('shared.tool.kind.fetch');
        case 'think':
            return t('shared.tool.kind.think');
        case 'switch_mode':
            return t('shared.tool.kind.switch_mode');
        case 'execute':
            // A shell tool whose kind hasn't arrived yet (it moves to the
            // shell chip when it does): named like that chip.
            return t('shared.shell.chip_label');
        default:
            return t('shared.tool.kind.other');
    }
}

// --- What a block shows in its header ----------------------------------

function firstStringArg(rec) {
    const input = rec.input;
    if (!input || typeof input !== 'object' || Array.isArray(input)) return null;
    for (const k of Object.keys(input)) {
        const v = input[k];
        if (typeof v === 'string' && v.trim() && v !== JSON_CUT) return { key: k, value: v };
    }
    return null;
}

function diffTotals(rec) {
    let added = 0;
    let removed = 0;
    for (const d of rec.diffs || []) {
        added += d.added;
        removed += d.removed;
    }
    return { added, removed, any: (rec.diffs || []).length > 0 };
}

function rangeText(r) {
    if (r.start !== null && r.end !== null) {
        return r.start === r.end ? String(r.start) : `${r.start}–${r.end}`;
    }
    if (r.start !== null) return `${r.start}–`;
    return `–${r.end}`;
}

/**
 * `{ text, placeholder, meta, metaLabel }` for a block's header: the main
 * subject (raw, shown with marks), a short muted note, and that note's
 * accessible wording.
 */
function headerOf(rec) {
    const title = rec.title || rec.firstTitle || '';
    const out = { text: '', placeholder: false, meta: '', metaLabel: '' };
    switch (rec.category) {
        case 'edit': {
            const paths = toolCallPaths(rec);
            out.text = paths[0] || title;
            const d = diffTotals(rec);
            if (d.any) {
                out.meta = `+${d.added} −${d.removed}`;
                out.metaLabel = t('shared.tool.diff.stat', {
                    added: d.added,
                    removed: d.removed,
                });
            } else if (paths.length > 1) {
                out.meta = `+${paths.length - 1}`;
                out.metaLabel = t('shared.tool.more_files', { count: paths.length - 1 });
            }
            break;
        }
        case 'read': {
            const paths = toolCallPaths(rec);
            out.text = paths[0] || title;
            const ranges = readRanges(rec);
            if (ranges.length === 1 && ranges[0].path === paths[0] && paths.length === 1) {
                out.meta = rangeText(ranges[0]);
                out.metaLabel = t('shared.tool.lines', { range: out.meta });
            } else if (paths.length > 1) {
                out.meta = `+${paths.length - 1}`;
                out.metaLabel = t('shared.tool.more_files', { count: paths.length - 1 });
            }
            break;
        }
        case 'search': {
            const { query } = searchQueryOf(rec);
            out.text = query || title;
            const { sources, more } = searchSourcesOf(rec);
            const n = sources.length + more;
            if (n) {
                out.meta = String(n);
                out.metaLabel = t('shared.tool.result_count', { count: n });
            }
            break;
        }
        case 'fetch': {
            const urls = fetchUrlsOf(rec);
            out.text = urls[0] || title;
            if (urls.length > 1) {
                out.meta = `+${urls.length - 1}`;
                out.metaLabel = t('shared.tool.more_urls', { count: urls.length - 1 });
            }
            break;
        }
        case 'mcp': {
            out.text = rec.mcp?.tool || title;
            const arg = firstStringArg(rec);
            if (arg) {
                out.meta = clip(oneLine(arg.value), PREVIEW_MAX);
                out.metaLabel = out.meta;
            }
            break;
        }
        default: {
            out.text = title;
            if (title === rec.firstTitle) {
                const arg = firstStringArg(rec);
                if (arg) {
                    out.meta = clip(oneLine(arg.value), PREVIEW_MAX);
                    out.metaLabel = out.meta;
                }
            }
        }
    }
    if (!out.text) {
        out.text = t('shared.tool.unknown_call');
        out.placeholder = true;
    }
    return out;
}

// --- Status --------------------------------------------------------------

function renderStatus(statusEl, state) {
    statusEl.textContent = '';
    statusEl.dataset.state = state;
    let text;
    if (state === 'running') {
        const spinner = el('span', 'shell-cmd-spinner');
        spinner.setAttribute('aria-hidden', 'true');
        statusEl.append(spinner);
        text = t('shared.tool.status.running');
    } else if (state === 'success') {
        statusEl.append(icon('check'));
        text = t('shared.tool.status.success');
    } else if (state === 'failed') {
        statusEl.append(icon('cross'));
        text = t('shared.tool.status.failed');
    } else {
        statusEl.append(icon('dash'));
        text = t('shared.tool.status.unknown');
    }
    statusEl.setAttribute('aria-label', text);
    statusEl.title = text;
}

// --- Panel pieces ------------------------------------------------------

/** A value the call operates on (path, URL, query): LTR, marks visible. */
function subjectValue(text, className = 'tool-call-value') {
    const node = el('span', className);
    node.dir = 'ltr';
    appendShellCommandText(node, text);
    return node;
}

function field(name, valueNode) {
    const row = el('div', 'tool-call-field');
    row.append(label('tool-call-label', name), valueNode);
    return row;
}

function section(name, ...children) {
    const sec = el('div', 'tool-call-section');
    sec.append(label('tool-call-section-label', name), ...children);
    return sec;
}

function note(text) {
    const n = el('div', 'shell-cmd-empty tool-call-note');
    n.dir = 'auto';
    n.textContent = text;
    return n;
}

function jsonCount(v) {
    if (Array.isArray(v)) return t('shared.tool.json.items', { count: v.length });
    if (v && typeof v === 'object') {
        return t('shared.tool.json.keys', { count: Object.keys(v).length });
    }
    return '';
}

function prettyJson(v) {
    try {
        return JSON.stringify(v, null, 2) ?? String(v);
    } catch {
        return '';
    }
}

/**
 * Make a scroll box reachable from the keyboard (WebKit can't scroll one
 * that can't take focus), named for screen readers: by the element with id
 * `labelledBy`, else by `name`.
 */
function scrollable(node, focusKey, { labelledBy = null, name = null } = {}) {
    node.tabIndex = 0;
    node.setAttribute('role', 'group');
    if (labelledBy) node.setAttribute('aria-labelledby', labelledBy);
    else if (name) node.setAttribute('aria-label', name);
    node.dataset.focus = focusKey;
    return node;
}

/**
 * A collapsible JSON value: `<details>` whose summary counts the keys /
 * items and whose body is the pretty-printed JSON (marks visible when
 * `marks`, i.e. for arguments). `key` lets an open one stay open when the
 * panel is rebuilt. With `labelledBy` / `name` the body scrolls on its own
 * and takes focus (outside a result box, which scrolls instead).
 */
function jsonDetails(
    value,
    key,
    { open = false, marks = true, labelledBy = null, name = null } = {}
) {
    const details = el('details', 'tool-json');
    details.dataset.key = key;
    if (open) details.open = true;
    const summary = el('summary', 'tool-json-summary');
    summary.dataset.focus = `json:${key}`;
    summary.append(label('tool-json-count', jsonCount(value)));
    const pre = el('pre', 'tool-json-body');
    pre.dir = 'ltr';
    if (labelledBy || name) scrollable(pre, `json-body:${key}`, { labelledBy, name });
    const text = prettyJson(value);
    if (marks) appendShellCommandText(pre, text);
    else pre.textContent = text;
    details.append(summary, pre);
    return details;
}

function scalarText(v) {
    if (typeof v === 'string') return v;
    return prettyJson(v);
}

/**
 * Arguments as key / value rows; nested values as collapsed JSON. Values
 * scroll past a few lines, so each can take focus (named by its key).
 */
function argumentsList(input, keyPrefix) {
    const list = el('div', 'tool-call-args');
    if (!input || typeof input !== 'object' || Array.isArray(input)) {
        list.append(
            jsonDetails(input, `${keyPrefix}:all`, {
                open: true,
                name: t('shared.tool.label.arguments'),
            })
        );
        return list;
    }
    for (const k of Object.keys(input)) {
        const v = input[k];
        const row = el('div', 'tool-call-arg');
        const keyEl = el('span', 'tool-call-arg-key');
        keyEl.id = uid('kage-tool-arg');
        keyEl.dir = 'ltr';
        appendShellCommandText(keyEl, k);
        row.append(keyEl);
        if (v && typeof v === 'object') {
            row.append(jsonDetails(v, `${keyPrefix}:${k}`, { labelledBy: keyEl.id }));
        } else {
            const val = el('span', 'tool-call-arg-value');
            val.dir = 'ltr';
            scrollable(val, `arg:${k}`, { labelledBy: keyEl.id });
            appendShellCommandText(val, scalarText(v));
            row.append(val);
        }
        list.append(row);
    }
    return list;
}

function argsWithout(input, keys) {
    if (!input || typeof input !== 'object' || Array.isArray(input)) return input;
    // Null-prototype, like the stored input: a `__proto__` argument is a key
    // (a plain `{}` would take it as its prototype and drop it from the list).
    const out = Object.create(null);
    let n = 0;
    for (const k of Object.keys(input)) {
        if (keys.includes(k)) continue;
        out[k] = input[k];
        n += 1;
    }
    return n ? out : null;
}

function segmentPre(seg) {
    const pre = el('pre', 'tool-call-text');
    pre.dir = 'ltr';
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

function outputBox(focusKey, ...children) {
    const box = el('div', 'tool-call-output');
    scrollable(box, focusKey, { name: t('shared.tool.result_label') });
    box.append(...children);
    return box;
}

/** The result section: text and/or JSON, or a note when there's none. */
function resultSection(rec, state, { skipJson = false } = {}) {
    const out = toolOutputOf(rec);
    const parts = [];
    if (out?.text) parts.push(segmentPre(out.text));
    if (out?.json !== undefined && !skipJson) {
        const lines = prettyJson(out.json).split('\n').length;
        parts.push(
            jsonDetails(out.json, 'result:json', { open: lines <= JSON_OPEN_LINES, marks: false })
        );
        if (out.jsonTruncated) parts.push(note(t('shared.tool.result_shortened')));
    }
    const name = state === 'failed' ? t('shared.tool.error_label') : t('shared.tool.result_label');
    if (parts.length) return section(name, outputBox('output:result', ...parts));
    if (skipJson && out?.json !== undefined) return null;
    let msg;
    if (out?.dropped) msg = t('shared.tool.output_dropped');
    else if (state === 'running') msg = t('shared.tool.still_running');
    else msg = t('shared.tool.no_output');
    return section(name, note(msg));
}

function diffLine(line, numbered) {
    const row = el('div', 'tool-diff-line');
    row.dataset.op = line.op === '+' ? 'add' : line.op === '-' ? 'del' : 'ctx';
    if (numbered) {
        const num = el('span', 'tool-diff-num');
        num.setAttribute('aria-hidden', 'true');
        num.textContent = String(line.op === '-' ? line.oldNo : line.newNo);
        row.append(num);
    }
    const sign = el('span', 'tool-diff-sign');
    sign.setAttribute('aria-hidden', 'true');
    sign.textContent = line.op === '+' ? '+' : line.op === '-' ? '−' : ' ';
    row.append(sign);
    if (line.op !== '=') {
        const sr = el('span', 'tool-sr-only');
        sr.textContent =
            line.op === '+' ? t('shared.tool.diff.added_line') : t('shared.tool.diff.removed_line');
        row.append(sr);
    }
    const text = el('span', 'tool-diff-text');
    appendShellCommandText(text, line.text);
    if (line.cut) text.append(document.createTextNode('…'));
    row.append(text);
    return row;
}

function diffGap(count) {
    const gap = el('div', 'tool-diff-gap');
    gap.dir = 'auto';
    gap.textContent = t('shared.tool.diff.unchanged', { count });
    return gap;
}

/** One diff: its file (when the call has several or it's new) and lines. */
function diffView(diff, showPath, index) {
    const wrap = el('div', 'tool-diff-file');
    if (showPath || diff.isNew) {
        const head = el('div', 'tool-diff-head');
        if (showPath && diff.path) head.append(subjectValue(diff.path, 'tool-diff-path'));
        if (diff.isNew) head.append(label('tool-diff-badge', t('shared.tool.diff.new_file')));
        wrap.append(head);
    }
    if (diff.tooLarge) {
        wrap.append(note(t('shared.tool.diff.too_large')));
        return wrap;
    }
    if (!diff.hunks.length) {
        wrap.append(note(t('shared.tool.diff.no_changes')));
        return wrap;
    }
    const box = el('div', 'tool-diff');
    box.dir = 'ltr';
    box.tabIndex = 0;
    box.setAttribute('role', 'group');
    box.dataset.focus = `diff:${index}`;
    if (diff.numbered) {
        // One gutter width for the whole diff, from its largest line number.
        let maxNo = 1;
        for (const h of diff.hunks) {
            for (const l of h.lines) maxNo = Math.max(maxNo, l.oldNo || 0, l.newNo || 0);
        }
        box.style.setProperty('--tool-diff-num-width', `${String(maxNo).length}ch`);
    }
    box.setAttribute(
        'aria-label',
        t('shared.tool.diff.stat', { added: diff.added, removed: diff.removed })
    );
    diff.hunks.forEach((h, i) => {
        if (h.gapBefore > 0 && (diff.numbered || i > 0)) box.append(diffGap(h.gapBefore));
        for (const line of h.lines) box.append(diffLine(line, diff.numbered));
    });
    if (diff.omittedLines > 0) {
        const more = el('div', 'shell-cmd-omitted tool-diff-more');
        more.dir = 'auto';
        more.textContent = t('shared.tool.diff.more_changes', { count: diff.omittedLines });
        box.append(more);
    } else if (diff.gapAfter > 0 && diff.numbered) {
        box.append(diffGap(diff.gapAfter));
    }
    wrap.append(box);
    return wrap;
}

function sourcesList(sources, more) {
    const list = el('ul', 'tool-call-sources');
    for (const s of sources) {
        const li = el('li', 'tool-call-source');
        if (s.url) {
            if (s.title) {
                const ttl = el('span', 'tool-call-source-title');
                ttl.dir = 'auto';
                ttl.textContent = oneLine(s.title);
                li.append(ttl);
            }
            li.append(subjectValue(s.url, 'tool-call-source-url'));
        } else {
            li.append(subjectValue(s.path, 'tool-call-source-path'));
            if (s.count !== null && s.count !== undefined) {
                li.append(
                    label('tool-call-source-count', t('shared.tool.matches', { count: s.count }))
                );
            }
        }
        list.append(li);
    }
    const box = outputBox('output:sources', list);
    if (more > 0) box.append(note(t('shared.tool.more_results', { count: more })));
    return box;
}

const PATH_ARG_KEYS = ['path', 'file_path', 'filePath', 'notebook_path', 'target_file', 'file'];

/** The panel body for `rec`, per category. */
function buildBody(rec, state) {
    const body = el('div', 'tool-call-body');
    if (rec.purpose) {
        const p = el('div', 'tool-call-purpose');
        p.dir = 'auto';
        p.textContent = rec.purpose;
        body.append(p);
    }
    const fields = el('div', 'tool-call-fields');
    let args = rec.input;
    let copy = null;

    switch (rec.category) {
        case 'edit': {
            const paths = toolCallPaths(rec);
            for (const p of paths)
                fields.append(field(t('shared.tool.label.file'), subjectValue(p)));
            if (paths.length) copy = { label: () => t('shared.tool.copy_path'), text: paths[0] };
            args = argsWithout(args, PATH_ARG_KEYS);
            break;
        }
        case 'read': {
            const paths = toolCallPaths(rec);
            const ranges = readRanges(rec);
            for (const p of paths) {
                fields.append(field(t('shared.tool.label.file'), subjectValue(p)));
                for (const r of ranges.filter((x) => x.path === p)) {
                    fields.append(field(t('shared.tool.label.lines'), subjectValue(rangeText(r))));
                }
            }
            if (paths.length) copy = { label: () => t('shared.tool.copy_path'), text: paths[0] };
            args = argsWithout(args, [
                ...PATH_ARG_KEYS,
                ...(readOperationsShown(rec) ? ['operations'] : []),
            ]);
            break;
        }
        case 'search': {
            const { query, scope, keys } = searchQueryOf(rec);
            if (query) fields.append(field(t('shared.tool.label.query'), subjectValue(query)));
            if (scope) fields.append(field(t('shared.tool.label.scope'), subjectValue(scope)));
            args = argsWithout(args, keys);
            break;
        }
        case 'fetch': {
            const urls = fetchUrlsOf(rec);
            for (const u of urls) fields.append(field(t('shared.tool.label.url'), subjectValue(u)));
            if (urls.length) copy = { label: () => t('shared.tool.copy_url'), text: urls[0] };
            args = argsWithout(args, ['url', 'uri', 'urls', 'inputs']);
            break;
        }
        case 'mcp': {
            fields.append(field(t('shared.tool.label.server'), subjectValue(rec.mcp.server)));
            fields.append(field(t('shared.tool.label.tool'), subjectValue(rec.mcp.tool)));
            break;
        }
        default:
            break;
    }
    if (fields.childElementCount) body.append(fields);

    if (rec.category === 'edit') {
        const many = rec.diffs.length > 1;
        rec.diffs.forEach((d, i) => body.append(diffView(d, many, i)));
    }

    const hasArgs =
        args !== undefined &&
        args !== null &&
        !(typeof args === 'object' && !Array.isArray(args) && !Object.keys(args).length);
    if (hasArgs) {
        const sec = section(t('shared.tool.label.arguments'), argumentsList(args, 'arg'));
        if (rec.inputTruncated) sec.append(note(t('shared.tool.arguments_shortened')));
        body.append(sec);
    }

    if (rec.category === 'search') {
        const { sources, more } = searchSourcesOf(rec);
        if (sources.length) {
            body.append(section(t('shared.tool.label.sources'), sourcesList(sources, more)));
            const rest = resultSection(rec, state, { skipJson: true });
            if (rest) body.append(rest);
        } else {
            body.append(resultSection(rec, state));
        }
    } else if (rec.category !== 'edit' || toolOutputOf(rec) || !rec.diffs.length) {
        body.append(resultSection(rec, state));
    }

    const footer = el('div', 'shell-cmd-footer tool-call-footer');
    const statusText = el('span', 'shell-cmd-exit');
    statusText.dir = 'auto';
    if (state === 'failed') statusText.textContent = t('shared.tool.status.failed');
    statusText.hidden = state !== 'failed';
    const actions = el('span', 'shell-cmd-actions');
    const addCopy = (focusKey, getLabel, getText) => {
        const btn = copyButton(getLabel, getText);
        btn.dataset.focus = focusKey;
        actions.append(btn);
    };
    if (copy) addCopy('copy:subject', copy.label, () => copy.text);
    if (hasArgs && rec.category !== 'edit') {
        addCopy(
            'copy:arguments',
            () => t('shared.tool.copy_arguments'),
            () => prettyJson(args)
        );
    }
    const resultText = toolResultText(rec);
    if (resultText) {
        addCopy(
            'copy:result',
            () => t('shared.tool.copy_result'),
            () => toolResultText(rec)
        );
    }
    footer.append(statusText, actions);
    if (actions.childElementCount || !statusText.hidden) body.append(footer);
    return body;
}

/** A call's result as one string (text, then JSON), omission markers included. */
export function toolResultText(rec) {
    const out = toolOutputOf(rec);
    if (!out) return '';
    const parts = [];
    if (out.text) parts.push(joinCapped(out.text, omittedMarker));
    if (out.json !== undefined) parts.push(prettyJson(out.json));
    return parts.join('\n');
}

// --- Blocks ---------------------------------------------------------------

/**
 * Put focus back on the element keyed `key` (`data-focus`) inside `panel`
 * after a rebuild, else — it's gone — on the block's header, so a keyboard
 * or screen reader user is never thrown back to the top of the page.
 */
function restoreFocus(panel, key, fallback) {
    const target = Array.from(panel.querySelectorAll('[data-focus]')).find(
        (n) => n.dataset.focus === key
    );
    (target || fallback).focus({ preventScroll: true });
}

function renderPanel(root) {
    const st = root._kageTool;
    const rec = st.record;
    // Keep the reader's place: focus, open JSON values and scroll offsets.
    const active = document.activeElement;
    const focusKey =
        active && st.panel.contains(active)
            ? active.closest('[data-focus]')?.dataset.focus || ''
            : null;
    const open = new Set(
        Array.from(st.panel.querySelectorAll('details[open][data-key]'), (d) => d.dataset.key)
    );
    const closed = new Set(
        Array.from(st.panel.querySelectorAll('details[data-key]:not([open])'), (d) => d.dataset.key)
    );
    const scrolls = Array.from(
        st.panel.querySelectorAll('.tool-call-output, .tool-diff'),
        (n) => n.scrollTop
    );
    st.panel.textContent = '';
    st.panel.append(buildBody(rec, toolCallState(rec)));
    for (const d of st.panel.querySelectorAll('details[data-key]')) {
        if (open.has(d.dataset.key)) d.open = true;
        else if (closed.has(d.dataset.key)) d.open = false;
    }
    st.panel.querySelectorAll('.tool-call-output, .tool-diff').forEach((n, i) => {
        if (scrolls[i]) n.scrollTop = scrolls[i];
    });
    if (focusKey !== null) restoreFocus(st.panel, focusKey, st.header);
    st.panelRev = rec.rev;
}

/** Whether a block's details panel is open. */
export function isToolCallExpanded(root) {
    return root?._kageTool?.header.getAttribute('aria-expanded') === 'true';
}

/** Open or close a block's details panel. */
export function setToolCallExpanded(root, open) {
    const st = root?._kageTool;
    if (!st) return;
    const want = !!open;
    if (want === isToolCallExpanded(root)) return;
    st.header.setAttribute('aria-expanded', String(want));
    st.panel.hidden = !want;
    root.classList.toggle('expanded', want);
    if (want) renderPanel(root);
    else {
        // A closed panel doesn't hold the call's output in the DOM.
        st.panel.textContent = '';
        st.panelRev = -1;
    }
    layoutChanged(root);
}

/** Repaint a block from (a newer revision of) its record. */
export function updateToolCallBlock(root, rec) {
    const st = root?._kageTool;
    if (!st || !rec) return;
    st.record = rec;
    const state = toolCallState(rec);
    if (st.renderedRev === rec.rev && st.renderedState === state) return;
    root.dataset.state = state;
    root.dataset.category = rec.category;
    if (st.renderedCategory !== rec.category) {
        st.icon.replaceWith((st.icon = icon(CATEGORY_ICONS[rec.category], 'tool-call-icon', 13)));
        st.renderedCategory = rec.category;
    }
    const head = headerOf(rec);
    st.text.textContent = '';
    if (head.placeholder) st.text.textContent = head.text;
    else appendShellCommandText(st.text, head.text, HEADER_MAX);
    st.text.classList.toggle('is-placeholder', head.placeholder);
    st.meta.textContent = '';
    st.meta.hidden = !head.meta;
    if (head.meta) {
        const vis = el('span');
        vis.setAttribute('aria-hidden', 'true');
        appendShellCommandText(vis, head.meta);
        const sr = el('span', 'tool-sr-only');
        sr.textContent = head.metaLabel;
        st.meta.append(vis, sr);
    }
    const tip = head.placeholder
        ? head.text
        : shellCommandDisplayText(clip(head.text, TOOLTIP_MAX)) +
          (head.text.length > TOOLTIP_MAX ? '…' : '');
    st.header.title = tip;
    renderStatus(st.status, state);
    if (isToolCallExpanded(root)) renderPanel(root);
    st.renderedRev = rec.rev;
    st.renderedState = state;
}

/** A new block for `rec` (collapsed). */
export function createToolCallBlock(rec) {
    const id = uid('kage-tool');
    const root = el('div', 'tool-call');
    root.dir = 'ltr';

    const header = el('button', 'tool-call-header');
    header.type = 'button';
    header.id = `${id}-toggle`;
    header.setAttribute('aria-expanded', 'false');
    header.setAttribute('aria-controls', `${id}-panel`);
    const ic = icon(CATEGORY_ICONS[rec.category], 'tool-call-icon', 13);
    const text = el('span', 'tool-call-summary');
    const meta = el('span', 'tool-call-meta');
    const status = el('span', 'shell-cmd-status');
    status.setAttribute('role', 'img');
    header.append(ic, text, meta, status, icon('chevron', 'shell-cmd-chevron', 12));

    const panel = el('div', 'tool-call-panel');
    panel.id = `${id}-panel`;
    panel.hidden = true;
    panel.setAttribute('role', 'region');
    panel.setAttribute('aria-labelledby', header.id);

    root.append(header, panel);
    root._kageTool = {
        record: rec,
        header,
        icon: ic,
        text,
        meta,
        status,
        panel,
        renderedRev: -1,
        renderedState: null,
        renderedCategory: rec.category,
        panelRev: -1,
    };
    header.addEventListener('click', () => {
        setToolCallExpanded(root, !isToolCallExpanded(root));
    });
    updateToolCallBlock(root, rec);
    return root;
}

// --- Chips -------------------------------------------------------------

function aggregateState(calls) {
    let failed = false;
    for (const rec of calls) {
        const s = toolCallState(rec);
        if (s === 'running') return 'running';
        if (s === 'failed') failed = true;
    }
    return failed ? 'failed' : 'done';
}

function setListOpen(chip, list, open) {
    chip.setAttribute('aria-expanded', String(open));
    list.hidden = !open;
}

function createChip(key, listId) {
    const chip = el('button', 'source-chip tool-chip tool-call-chip');
    chip.type = 'button';
    chip.dataset.group = key;
    chip.setAttribute('aria-expanded', 'false');
    chip.setAttribute('aria-controls', listId);
    const lbl = el('span', 'source-domain tool-call-chip-label');
    const count = el('span', 'tool-chip-count');
    const dot = el('span', 'shell-chip-dot');
    dot.setAttribute('aria-hidden', 'true');
    chip.append(
        icon('other', 'tool-chip-icon tool-call-chip-icon', 13),
        lbl,
        count,
        dot,
        icon('chevron', 'shell-chip-chevron', 10)
    );
    return chip;
}

function updateChip(chip, list, group) {
    const n = group.calls.length;
    const category = group.mcp ? 'mcp' : group.calls[0]?.category || 'other';
    if (chip.dataset.category !== category) {
        chip.dataset.category = category;
        chip.querySelector('.tool-call-chip-icon').replaceWith(
            icon(CATEGORY_ICONS[category], 'tool-chip-icon tool-call-chip-icon', 13)
        );
    }
    const lbl = chip.querySelector('.tool-call-chip-label');
    const name = group.label ?? kindLabel(group.kind);
    lbl.textContent = name;
    // A tool / server name is an identifier; a kind name is translated text.
    lbl.dir = group.label ? 'ltr' : 'auto';
    chip.querySelector('.tool-chip-count').textContent = n > 1 ? `×${n}` : '';
    const title = group.mcp
        ? t('shared.tool.chip_title_mcp', { count: n, server: name })
        : t('shared.tool.chip_title', { count: n, name });
    const state = aggregateState(group.calls);
    // The red dot is decorative; the name says it too.
    const named = chipStateLabel(title, state);
    chip.title = named;
    chip.setAttribute('aria-label', named);
    chip.dataset.state = state;
    list.setAttribute('aria-label', title);
}

/**
 * Keep the tool chips and their call lists in `container` (a
 * `.tool-sources` row) in sync with `toolUsages`: one toggle chip per
 * tool / MCP server / kind, in order of first use. Chips, lists and
 * blocks are reconciled in place — a block that moves to another chip
 * (its kind or title arrived late) is moved, not rebuilt — so open lists
 * and panels survive.
 */
export function syncToolCallLists(container, toolUsages) {
    if (!container) return;
    // Moving a node (a block that changed chip, a reordered chip) drops
    // focus inside it; put it back afterwards.
    const active = document.activeElement;
    const groups = groupToolCalls(toolUsages);
    let chipsWrap = container.querySelector(':scope > .tool-call-chips');
    let listsWrap = container.querySelector(':scope > .tool-call-lists');
    if (!groups.length) {
        chipsWrap?.remove();
        listsWrap?.remove();
        return;
    }
    if (!chipsWrap) {
        chipsWrap = el('div', 'tool-call-chips');
        const anchor = container.querySelector(':scope > .tool-sources-static');
        container.insertBefore(chipsWrap, anchor);
    }
    if (!listsWrap) {
        listsWrap = el('div', 'tool-call-lists');
        container.append(listsWrap);
    }
    const blocks = new Map();
    for (const b of listsWrap.querySelectorAll('.tool-call')) {
        blocks.set(b._kageTool.record.toolCallId, b);
    }
    const chips = new Map(Array.from(chipsWrap.children, (c) => [c.dataset.group, c]));
    const lists = new Map(Array.from(listsWrap.children, (l) => [l.dataset.group, l]));

    groups.forEach((group, i) => {
        let chip = chips.get(group.key);
        let list = lists.get(group.key);
        chips.delete(group.key);
        lists.delete(group.key);
        if (!chip || !list) {
            chip?.remove();
            list?.remove();
            const listId = uid('kage-tool-list');
            chip = createChip(group.key, listId);
            list = el('div', 'shell-cmd-list tool-call-list');
            list.id = listId;
            list.dataset.group = group.key;
            list.hidden = true;
            const chipRef = chip;
            const listRef = list;
            chip.addEventListener('click', () => {
                setListOpen(chipRef, listRef, chipRef.getAttribute('aria-expanded') !== 'true');
                layoutChanged(chipRef);
            });
        }
        if (chipsWrap.children[i] !== chip)
            chipsWrap.insertBefore(chip, chipsWrap.children[i] || null);
        if (listsWrap.children[i] !== list)
            listsWrap.insertBefore(list, listsWrap.children[i] || null);
        updateChip(chip, list, group);

        const want = new Set();
        group.calls.forEach((rec, j) => {
            want.add(rec.toolCallId);
            let block = blocks.get(rec.toolCallId);
            if (block) updateToolCallBlock(block, rec);
            else {
                block = createToolCallBlock(rec);
                blocks.set(rec.toolCallId, block);
            }
            if (list.children[j] !== block) list.insertBefore(block, list.children[j] || null);
        });
        for (const child of Array.from(list.children)) {
            if (!want.has(child._kageTool?.record.toolCallId)) child.remove();
        }
    });
    for (const stale of chips.values()) stale.remove();
    for (const stale of lists.values()) stale.remove();
    if (active && active !== document.activeElement && active.isConnected) {
        if (container.contains(active)) active.focus({ preventScroll: true });
    }
}

/**
 * Render a response's tool/source chip row into `container`: the shell
 * chip and command list (shell-command-view.js), a toggle chip and call
 * list per other tool, then extension-tool chips and source chips (one
 * wrapper, re-rendered each time). Everything but that wrapper is
 * reconciled in place. Pair with `attachSourceClickHandler(container,
 * invoke)` for the source links. `opts` goes to `syncShellCommandList`
 * (`revealSandboxBlocked` for a response drawn whole, e.g. from history).
 */
export function renderToolSources(container, toolUsages, toolSources, opts = {}) {
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
        renderToolChipsHtml(usages.filter((u) => !u?.shell && !u?.call)) +
        renderSourceChipsHtml(Array.isArray(toolSources) ? toolSources : []);
    syncShellCommandList(container, usages, opts);
    syncToolCallLists(container, usages);
}

/** Re-sync every tool list in `container` after records changed in place. */
export function syncToolSources(container, toolUsages) {
    if (!container) return;
    syncShellCommandList(container, toolUsages);
    syncToolCallLists(container, toolUsages);
}
