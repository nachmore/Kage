/**
 * Tool calls other than shell commands — pure data layer (no DOM).
 *
 * Shell runs have their own record (shell-commands.js). Every other ACP
 * tool call gets one normalised record here (`entry.call`), kept up to date
 * across the call's `tool_call` / `tool_call_update` notifications, that
 * the shared tool-call view (tool-call-view.js) renders as an expandable
 * block:
 *
 *   - category: what the call does — `edit` (ACP kinds edit / delete /
 *     move), `read`, `search`, `fetch`, `think`, `mcp` (a tool from an MCP
 *     server, by the same title rules the permission handler uses — see
 *     `parseMcpTitle`) or `other`.
 *   - input: the call's arguments (`rawInput`), a capped copy.
 *   - diffs: compact unified diffs, from ACP `diff` content
 *     (`{ type: "diff", path, oldText, newText }`) or, for agents that
 *     only send their edit tool's arguments (kiro's `write`, Claude Code's
 *     Edit / Write / MultiEdit), from those.
 *   - output: what the call returned, from `rawOutput` (kiro:
 *     `{ items: [{ Text } | { Json }] }`, possibly in a `Success` /
 *     `Error` wrapper; MCP results `{ content: [...], isError }`) or ACP
 *     content text blocks — text, or JSON when it isn't text.
 *   - locations: ACP `locations` (`{ path, line }`).
 *
 * Everything here is untrusted agent data and is only ever rendered as
 * text. Output text is sanitised the way shell output is (escape
 * sequences, control and bidi characters stripped — `sanitizeAndCap`);
 * what a call operates on (paths, URLs, queries, arguments, diff lines) is
 * kept as sent and shown with hidden characters made visible
 * (`shellCommandDisplayParts`), like a command line. Sizes are capped per
 * call and, together with shell output, per response.
 */

import { sanitizeAndCap, sanitizeTerminalText } from './shell-commands.js';

/** Output (result text / JSON / diff lines) kept per call, in characters. */
export const TOOL_OUTPUT_CAP = 64 * 1024;
/** Arguments kept per call, in characters. */
export const TOOL_INPUT_CAP = 16 * 1024;

const KNOWN_STATUSES = new Set(['pending', 'in_progress', 'completed', 'failed']);

// --- MCP tool titles (mirror of src/config/permissions.rs) -----------------

const MAX_MCP_SERVER_NAME_LEN = 64;
const MAX_MCP_TOOL_NAME_LEN = 128;
const KIRO_MCP_TITLE_PREFIX = 'Running: @';
const CLAUDE_MCP_NAME_PREFIX = 'mcp__';
// Claude Code's ACP adapter runs its own built-in tools through an
// in-process MCP server of this name — never a third-party server.
const CLAUDE_ADAPTER_BUILTIN_SERVER = 'acp';
const MCP_IDENTIFIER = /^[A-Za-z0-9._-]+$/;

function isMcpIdentifier(s, maxLen) {
    return typeof s === 'string' && s.length > 0 && s.length <= maxLen && MCP_IDENTIFIER.test(s);
}

/** `is_valid_mcp_server_name`: 1–64 ASCII letters, digits, `-`, `_`, `.`. */
export function isValidMcpServerName(name) {
    return isMcpIdentifier(name, MAX_MCP_SERVER_NAME_LEN);
}

/**
 * `{ server, tool }` when `title` is exactly an agent's MCP tool form —
 * kiro-cli's `Running: @<server>/<tool>` or Claude Code's
 * `mcp__<server>__<tool>` — else `null`. The same rules as
 * `mcp_server_from_title` in src/config/permissions.rs (the parity test
 * reads that file's cases): both parts plain identifiers, nothing around
 * them, one unambiguous separator, and Claude's built-in `acp` server
 * excluded.
 */
export function parseMcpTitle(title) {
    if (typeof title !== 'string') return null;
    if (title.startsWith(KIRO_MCP_TITLE_PREFIX)) {
        const rest = title.slice(KIRO_MCP_TITLE_PREFIX.length);
        const i = rest.indexOf('/');
        if (i < 0) return null;
        const server = rest.slice(0, i);
        const tool = rest.slice(i + 1);
        return isValidMcpServerName(server) && isMcpIdentifier(tool, MAX_MCP_TOOL_NAME_LEN)
            ? { server, tool }
            : null;
    }
    if (title.startsWith(CLAUDE_MCP_NAME_PREFIX)) {
        const rest = title.slice(CLAUDE_MCP_NAME_PREFIX.length);
        // Rust: `rest.matches("__").count() != 1 || rest.contains("___")`.
        if (rest.split('__').length - 1 !== 1 || rest.includes('___')) return null;
        const i = rest.indexOf('__');
        const server = rest.slice(0, i);
        const tool = rest.slice(i + 2);
        if (server === CLAUDE_ADAPTER_BUILTIN_SERVER) return null;
        return isValidMcpServerName(server) && isMcpIdentifier(tool, MAX_MCP_TOOL_NAME_LEN)
            ? { server, tool }
            : null;
    }
    return null;
}

/**
 * The MCP tool a call is, or `null` — `classify_mcp_server` in
 * src/commands/messaging/notifications.rs: the kind must be absent or
 * `other` (kiro titles a shell run `Running: <command>`, so a command
 * spelled `@server/tool` has kind `execute` and is never MCP), and every
 * title we have must name the same server.
 */
export function classifyMcpCall(firstTitle, latestTitle, kind) {
    if (kind && kind !== 'other') return null;
    const first = parseMcpTitle(firstTitle);
    if (!first) return null;
    if (typeof latestTitle === 'string' && latestTitle && latestTitle !== firstTitle) {
        if (parseMcpTitle(latestTitle)?.server !== first.server) return null;
    }
    return first;
}

// --- Categories -------------------------------------------------------------

/** ACP tool kinds (agent-client-protocol `ToolKind`). */
export const ACP_TOOL_KINDS = Object.freeze([
    'read',
    'edit',
    'delete',
    'move',
    'search',
    'execute',
    'think',
    'fetch',
    'switch_mode',
    'other',
]);

// Built-in tool names of the agents we know (kiro-cli, Claude Code), for a
// call whose kind is missing or `other`, and for session history (which
// only has the tool's name).
const TOOL_NAME_KINDS = Object.freeze({
    shell: 'execute',
    execute_bash: 'execute',
    execute_cmd: 'execute',
    Bash: 'execute',
    read: 'read',
    fs_read: 'read',
    Read: 'read',
    NotebookRead: 'read',
    write: 'edit',
    fs_write: 'edit',
    Edit: 'edit',
    MultiEdit: 'edit',
    Write: 'edit',
    NotebookEdit: 'edit',
    grep: 'search',
    glob: 'search',
    Grep: 'search',
    Glob: 'search',
    LS: 'search',
    code: 'search',
    knowledge: 'search',
    web_search: 'search',
    WebSearch: 'search',
    web_fetch: 'fetch',
    WebFetch: 'fetch',
    todo_list: 'think',
    TodoWrite: 'think',
    thinking: 'think',
});

/** The ACP kind of a built-in tool named `name`, or `null`. */
export function kindForToolName(name) {
    return typeof name === 'string' && Object.hasOwn(TOOL_NAME_KINDS, name)
        ? TOOL_NAME_KINDS[name]
        : null;
}

const KIND_CATEGORY = Object.freeze({
    read: 'read',
    edit: 'edit',
    delete: 'edit',
    move: 'edit',
    search: 'search',
    fetch: 'fetch',
    think: 'think',
});

/**
 * The effective ACP kind of a call: the agent's, else (missing or `other`)
 * what its built-in tool name implies, else `other`.
 */
function effectiveKind(kind, firstTitle) {
    if (kind && kind !== 'other') return kind;
    return kindForToolName(firstTitle) || kind || 'other';
}

/**
 * `{ category, kind, mcp }` for a call that isn't a shell command:
 * category per the module docs, the effective ACP kind, and the MCP tool
 * (`{ server, tool }`) when the call is one.
 */
export function classifyToolCall(kind, firstTitle, latestTitle) {
    const mcp = classifyMcpCall(firstTitle, latestTitle, kind);
    if (mcp) return { category: 'mcp', kind: kind || 'other', mcp };
    const k = effectiveKind(kind, firstTitle);
    return { category: KIND_CATEGORY[k] || 'other', kind: k, mcp: null };
}

// A tool the agent titles with a plain name we don't know (`deploy`) keeps
// a chip of its own, named so.
const PLAIN_TOOL_NAME = /^[A-Za-z0-9_.-]{1,40}$/;

/**
 * Which chip a record is listed under: `{ key, label, kind, mcp }`.
 * MCP calls group by server (`builder-mcp ×2`); everything else by its
 * effective kind (`read`, `edit`, `search` …; `label` is `null` and the
 * view shows the kind's translated name) — except a call titled with a
 * plain tool name that isn't a known built-in, which groups by that name.
 *
 * Built-in tools group by kind however they're titled: kiro-cli titles
 * them descriptively while it streams (`Reading /src/app.rs`, `Searching
 * for '…' in /src`), its session log only has the name (`read`, `grep`),
 * and a reopened response must show the chips the live one did. A record
 * rebuilt from the log is marked `builtin` when the log says the tool was
 * one (tool-call-history.js).
 */
export function toolChipGroup(rec) {
    if (!rec) return null;
    if (rec.category === 'mcp' && rec.mcp) {
        return { key: `mcp:${rec.mcp.server}`, label: rec.mcp.server, kind: 'mcp', mcp: true };
    }
    const first = rec.firstTitle;
    if (
        !rec.builtin &&
        typeof first === 'string' &&
        PLAIN_TOOL_NAME.test(first) &&
        !kindForToolName(first)
    ) {
        return { key: `tool:${first}`, label: first, kind: rec.kind, mcp: false };
    }
    return { key: `kind:${rec.kind}`, label: null, kind: rec.kind, mcp: false };
}

// --- Capping ------------------------------------------------------------

/** Move `i` off the low half of a surrogate pair so a cut never splits one. */
function safeCut(s, i) {
    if (i > 0 && i < s.length) {
        const c = s.charCodeAt(i);
        if (c >= 0xdc00 && c <= 0xdfff) return i - 1;
    }
    return i;
}

/**
 * A copy of `s` that doesn't reference the string it was cut from (see
 * shell-commands.js: a slice keeps its whole parent alive).
 */
function detach(s) {
    return s.length > 64 ? JSON.parse(JSON.stringify(s)) : s;
}

function detachSegment(seg) {
    if (!seg) return seg;
    return { head: detach(seg.head), tail: detach(seg.tail), omitted: seg.omitted };
}

function segmentLength(seg) {
    return seg ? seg.head.length + seg.tail.length : 0;
}

/** First `max` characters of `s` (detached), and whether it was cut. */
function clipString(s, max) {
    if (s.length <= max) return { s: detach(s), cut: false };
    return { s: detach(s.slice(0, safeCut(s, Math.max(0, max)))), cut: true };
}

const JSON_MAX_DEPTH = 8;
const JSON_MAX_ENTRIES = 200;
const JSON_MAX_STRING = 8 * 1024;
const JSON_MAX_KEY = 256;
/** Stand-in for a value cut by the caps (the view says the data was shortened). */
export const JSON_CUT = '…';

/**
 * A capped deep copy of a JSON value: at most `budget` characters of
 * strings (plus a little per value), `JSON_MAX_ENTRIES` entries per object
 * or array, `JSON_MAX_DEPTH` levels; anything cut is replaced by
 * `JSON_CUT` and `truncated` is set. Objects are null-prototype, so a
 * `__proto__` key is just a key. `clean` (output) sanitises strings like
 * shell output; inputs keep them as sent. Returns `{ value, size,
 * truncated }`, `size` in characters.
 */
export function capJson(value, budget, { clean = false } = {}) {
    const st = { left: Math.max(0, Math.floor(budget)), truncated: false };
    const v = capValue(value, 0, st, clean);
    return { value: v, size: Math.max(0, Math.floor(budget)) - st.left, truncated: st.truncated };
}

function capValue(v, depth, st, clean) {
    if (v === null || typeof v === 'boolean') {
        st.left -= 4;
        return v;
    }
    if (typeof v === 'number') {
        st.left -= 8;
        return Number.isFinite(v) ? v : null;
    }
    if (typeof v === 'string') {
        const text = clean ? sanitizeTerminalText(v) : v;
        const room = Math.min(JSON_MAX_STRING, Math.max(0, st.left));
        const { s, cut } = clipString(text, room);
        st.left -= s.length + 2;
        if (cut) {
            st.truncated = true;
            return s + JSON_CUT;
        }
        return s;
    }
    if (typeof v !== 'object') return null;
    if (depth >= JSON_MAX_DEPTH || st.left <= 0) {
        st.truncated = true;
        st.left -= 1;
        return JSON_CUT;
    }
    if (Array.isArray(v)) {
        const out = [];
        for (let i = 0; i < v.length; i++) {
            if (i >= JSON_MAX_ENTRIES || st.left <= 0) {
                st.truncated = true;
                out.push(JSON_CUT);
                break;
            }
            out.push(capValue(v[i], depth + 1, st, clean));
        }
        st.left -= 2;
        return out;
    }
    const out = Object.create(null);
    let n = 0;
    for (const key of Object.keys(v)) {
        if (n >= JSON_MAX_ENTRIES || st.left <= 0) {
            st.truncated = true;
            out[JSON_CUT] = JSON_CUT;
            break;
        }
        const k = clipString(clean ? sanitizeTerminalText(key) : key, JSON_MAX_KEY).s;
        st.left -= k.length + 3;
        out[k] = capValue(v[key], depth + 1, st, clean);
        n += 1;
    }
    st.left -= 2;
    return out;
}

// --- Diffs --------------------------------------------------------------

/** Lines of context kept around each change. */
export const DIFF_CONTEXT = 3;
/** Diff lines (changes + context) a diff shows at most. */
export const DIFF_MAX_SHOWN_LINES = 400;
/** Characters a single shown diff line keeps. */
export const DIFF_LINE_MAX = 2000;
// Inputs past these aren't compared at all (the view says so).
const DIFF_MAX_INPUT_CHARS = 2 * 1024 * 1024;
const DIFF_MAX_INPUT_LINES = 50000;
// Myers search depth; a bigger change is shown as one replaced block.
const DIFF_MAX_EDIT_DISTANCE = 1000;

function splitLines(s) {
    if (typeof s !== 'string' || !s) return [];
    const lines = s.replace(/\r\n/g, '\n').split('\n');
    if (lines[lines.length - 1] === '') lines.pop();
    return lines;
}

/**
 * Myers' O((N+M)·D) diff of two line arrays: an array of `[op, aIndex,
 * bIndex]` (`op` is `=`, `-` or `+`) in order, or `null` when the edit
 * distance exceeds `maxD`.
 */
function myers(a, b, maxD) {
    const n = a.length;
    const m = b.length;
    const max = Math.min(n + m, maxD);
    // v[k] for the current d lives at v[k + off].
    const off = max + 1;
    const v = new Int32Array(2 * max + 3);
    const trace = [];
    let found = -1;
    for (let d = 0; d <= max && found < 0; d++) {
        // Snapshot the band this step reads (k in [-d-1, d+1]).
        trace.push(v.slice(off - d - 1, off + d + 2));
        for (let k = -d; k <= d; k += 2) {
            let x;
            if (k === -d || (k !== d && v[off + k - 1] < v[off + k + 1])) x = v[off + k + 1];
            else x = v[off + k - 1] + 1;
            let y = x - k;
            while (x < n && y < m && a[x] === b[y]) {
                x += 1;
                y += 1;
            }
            v[off + k] = x;
            if (x >= n && y >= m) {
                found = d;
                break;
            }
        }
    }
    if (found < 0) return null;
    const ops = [];
    let x = n;
    let y = m;
    for (let d = found; d >= 0; d--) {
        const band = trace[d];
        // band index for k is k + d + 1.
        const at = (k) => band[k + d + 1];
        const k = x - y;
        const prevK = k === -d || (k !== d && at(k - 1) < at(k + 1)) ? k + 1 : k - 1;
        const prevX = d === 0 ? 0 : at(prevK);
        const prevY = prevX - prevK;
        while (x > prevX && y > prevY) {
            x -= 1;
            y -= 1;
            ops.push(['=', x, y]);
        }
        if (d > 0) {
            if (x === prevX) ops.push(['+', x, prevY]);
            else ops.push(['-', prevX, y]);
        }
        x = prevX;
        y = prevY;
    }
    ops.reverse();
    return ops;
}

function clipLine(s) {
    if (s.length <= DIFF_LINE_MAX) return { text: detach(s), cut: false };
    return { text: detach(s.slice(0, safeCut(s, DIFF_LINE_MAX))), cut: true };
}

/**
 * A compact unified diff of `oldText` → `newText`: hunks of changed lines
 * with `DIFF_CONTEXT` lines of context, long unchanged stretches between
 * them folded away, at most `DIFF_MAX_SHOWN_LINES` lines and `budget`
 * characters shown (the rest counted in `omittedLines`).
 *
 * Returns `{ path, isNew, numbered, added, removed, hunks, omittedLines,
 * tooLarge, size }`; each hunk is `{ oldStart, newStart, gapBefore, lines:
 * [{ op, text, oldNo, newNo, cut }] }` (`gapBefore`: unchanged lines
 * folded before it), plus `gapAfter` on the diff. `oldText` `null` means a
 * new file. `numbered: false` (a snippet edit, not the whole file) hides
 * line numbers.
 */
export function buildDiff(
    path,
    oldText,
    newText,
    { budget = TOOL_OUTPUT_CAP, numbered = true } = {}
) {
    const diff = {
        path: typeof path === 'string' && path ? clipString(path, 4096).s : null,
        isNew: oldText === null || oldText === undefined,
        numbered,
        added: 0,
        removed: 0,
        hunks: [],
        gapAfter: 0,
        omittedLines: 0,
        tooLarge: false,
        size: 0,
    };
    const oldS = typeof oldText === 'string' ? oldText : '';
    const newS = typeof newText === 'string' ? newText : '';
    if (oldS.length > DIFF_MAX_INPUT_CHARS || newS.length > DIFF_MAX_INPUT_CHARS) {
        diff.tooLarge = true;
        return diff;
    }
    const a = splitLines(oldS);
    const b = splitLines(newS);
    if (a.length > DIFF_MAX_INPUT_LINES || b.length > DIFF_MAX_INPUT_LINES) {
        diff.tooLarge = true;
        return diff;
    }
    // Common prefix / suffix never need the O(ND) search.
    let pre = 0;
    while (pre < a.length && pre < b.length && a[pre] === b[pre]) pre += 1;
    let suf = 0;
    while (
        suf < a.length - pre &&
        suf < b.length - pre &&
        a[a.length - 1 - suf] === b[b.length - 1 - suf]
    ) {
        suf += 1;
    }
    const aMid = a.slice(pre, a.length - suf);
    const bMid = b.slice(pre, b.length - suf);
    let mid = myers(aMid, bMid, DIFF_MAX_EDIT_DISTANCE);
    if (!mid) {
        // Too different to align: the whole middle was replaced.
        mid = [];
        for (let i = 0; i < aMid.length; i++) mid.push(['-', i, 0]);
        for (let j = 0; j < bMid.length; j++) mid.push(['+', aMid.length, j]);
    }

    // Walk the full op stream: prefix, middle, suffix.
    const ops = [];
    for (let i = 0; i < pre; i++) ops.push(['=', i, i]);
    for (const [op, ai, bi] of mid) ops.push([op, pre + ai, pre + bi]);
    for (let i = 0; i < suf; i++) ops.push(['=', a.length - suf + i, b.length - suf + i]);

    let left = Math.max(0, Math.floor(budget));
    let shown = 0;
    let full = false;
    let hunk = null;
    let lastEnd = -1; // index into ops just past the last shown op
    const changeAt = [];
    for (let i = 0; i < ops.length; i++) if (ops[i][0] !== '=') changeAt.push(i);
    for (const op of ops) {
        if (op[0] === '-') diff.removed += 1;
        else if (op[0] === '+') diff.added += 1;
    }
    // Ranges to show: each change ± context, merged when they touch.
    const ranges = [];
    for (const i of changeAt) {
        const from = Math.max(0, i - DIFF_CONTEXT);
        const to = Math.min(ops.length, i + DIFF_CONTEXT + 1);
        const last = ranges[ranges.length - 1];
        if (last && from <= last[1]) last[1] = Math.max(last[1], to);
        else ranges.push([from, to]);
    }
    for (const [from, to] of ranges) {
        if (!full && (shown >= DIFF_MAX_SHOWN_LINES || left <= 0)) full = true;
        if (full) {
            for (let i = from; i < to; i++) if (ops[i][0] !== '=') diff.omittedLines += 1;
            continue;
        }
        hunk = {
            oldStart: ops[from][1] + 1,
            newStart: ops[from][2] + 1,
            gapBefore: from - (lastEnd < 0 ? 0 : lastEnd),
            lines: [],
        };
        diff.hunks.push(hunk);
        for (let i = from; i < to; i++) {
            const [op, ai, bi] = ops[i];
            if (full || shown >= DIFF_MAX_SHOWN_LINES || left <= 0) {
                full = true;
                if (op !== '=') diff.omittedLines += 1;
                continue;
            }
            const src = op === '+' ? b[bi] : a[ai];
            const { text, cut } = clipLine(src);
            hunk.lines.push({
                op,
                text,
                cut,
                oldNo: op === '+' ? null : ai + 1,
                newNo: op === '-' ? null : bi + 1,
            });
            shown += 1;
            left -= text.length + 1;
            diff.size += text.length + 1;
        }
        lastEnd = to;
    }
    diff.gapAfter = lastEnd < 0 ? 0 : ops.length - lastEnd;
    return diff;
}

// --- Inputs ----------------------------------------------------------------

function str(v) {
    return typeof v === 'string' && v ? v : null;
}

/** First string among `keys` of `obj`. */
function pick(obj, keys) {
    if (!obj || typeof obj !== 'object') return null;
    for (const k of keys) {
        const s = str(obj[k]);
        if (s) return s;
    }
    return null;
}

const PATH_KEYS = ['path', 'file_path', 'filePath', 'notebook_path', 'target_file', 'file'];

// Edit-tool argument keys whose text the diff shows — not repeated in the
// argument list.
const EDIT_TEXT_KEYS = new Set([
    'content',
    'file_text',
    'oldStr',
    'newStr',
    'old_str',
    'new_str',
    'old_string',
    'new_string',
    'edits',
    'new_source',
]);

/**
 * Diffs from an edit tool's arguments, for agents that don't send ACP
 * diff content: kiro's `write` (`command` create / str_replace / insert /
 * append) and Claude Code's Edit / MultiEdit / Write. Snippet diffs
 * (str_replace, Edit) aren't numbered — they don't say where in the file
 * the text is.
 */
export function diffsFromEditInput(input, budget = TOOL_OUTPUT_CAP) {
    if (!input || typeof input !== 'object' || Array.isArray(input)) return [];
    const path = pick(input, PATH_KEYS);
    const out = [];
    const oldStr = pick(input, ['oldStr', 'old_str', 'old_string']);
    const newStr =
        typeof (input.newStr ?? input.new_str ?? input.new_string) === 'string'
            ? (input.newStr ?? input.new_str ?? input.new_string)
            : null;
    const content = pick(input, ['content', 'file_text']);
    const command = str(input.command);
    if (Array.isArray(input.edits)) {
        let left = budget;
        for (const e of input.edits.slice(0, 20)) {
            const o = pick(e, ['old_string', 'oldStr', 'old_str']);
            const n = e && typeof e === 'object' ? (e.new_string ?? e.newStr ?? e.new_str) : null;
            if (o === null && typeof n !== 'string') continue;
            const d = buildDiff(path, o ?? '', typeof n === 'string' ? n : '', {
                budget: left,
                numbered: false,
            });
            d.isNew = false;
            left -= d.size;
            out.push(d);
        }
        return out;
    }
    if (oldStr !== null || (newStr !== null && command !== 'insert')) {
        const d = buildDiff(path, oldStr ?? '', newStr ?? '', { budget, numbered: false });
        d.isNew = false;
        return [d];
    }
    if (command === 'insert' && newStr !== null) {
        const d = buildDiff(path, '', newStr, { budget, numbered: false });
        d.isNew = false;
        return [d];
    }
    if (content !== null) {
        // create / append / Write: the text written. Whether the file was
        // new isn't known from the arguments alone.
        const d = buildDiff(path, '', content, { budget, numbered: command !== 'append' });
        d.isNew = false;
        return [d];
    }
    return [];
}

/** The input to list as arguments: edit text the diff shows is left out. */
function displayInput(input, hasDiff) {
    if (!hasDiff || !input || typeof input !== 'object' || Array.isArray(input)) return input;
    // Null-prototype: assigning a `__proto__` argument must add a key, not
    // set the copy's prototype (and drop the argument from the list).
    const out = Object.create(null);
    for (const k of Object.keys(input)) if (!EDIT_TEXT_KEYS.has(k)) out[k] = input[k];
    return out;
}

// --- Outputs ---------------------------------------------------------------

const PARSE_DEPTH_LIMIT = 10;

function isContentBlock(x) {
    return (
        !!x &&
        typeof x === 'object' &&
        !Array.isArray(x) &&
        typeof x.type === 'string' &&
        ['text', 'image', 'audio', 'resource', 'resource_link', 'content'].includes(x.type)
    );
}

function collectOutput(raw, acc, depth) {
    if (raw == null || depth > PARSE_DEPTH_LIMIT) return;
    if (typeof raw === 'string') {
        const s = raw.trim();
        if (!s) return;
        if (s[0] === '{' || s[0] === '[') {
            try {
                collectOutput(JSON.parse(s), acc, depth + 1);
                return;
            } catch {
                /* plain text */
            }
        }
        acc.texts.push(raw);
        return;
    }
    if (typeof raw !== 'object') {
        acc.json.push(raw);
        return;
    }
    if (Array.isArray(raw)) {
        if (raw.length && raw.every((x) => isContentBlock(x) || isItem(x))) {
            for (const x of raw) collectOutput(x, acc, depth + 1);
        } else {
            acc.json.push(raw);
        }
        return;
    }
    const keys = Object.keys(raw);
    if (keys.length === 1 && keys[0] === 'Success') {
        collectOutput(raw.Success, acc, depth + 1);
        return;
    }
    for (const k of ['Error', 'Failure', 'error']) {
        if (!(k in raw) || raw[k] == null || raw[k] === false) continue;
        if (keys.length !== 1 && k === 'error' && typeof raw[k] !== 'string') continue;
        acc.failed = true;
        const e = raw[k];
        if (typeof e === 'string') acc.texts.push(e);
        else if (e && typeof e === 'object' && typeof e.Custom === 'string')
            acc.texts.push(e.Custom);
        else if (e && typeof e === 'object' && typeof e.message === 'string') {
            acc.texts.push(e.message);
        } else collectOutput(e, acc, depth + 1);
        return;
    }
    if (Array.isArray(raw.items) && keys.length === 1) {
        for (const item of raw.items) collectOutput(item, acc, depth + 1);
        return;
    }
    if (keys.length === 1 && keys[0] === 'Json') {
        collectOutput(raw.Json, acc, depth + 1);
        return;
    }
    if (keys.length === 1 && keys[0] === 'Text') {
        if (typeof raw.Text === 'string' && raw.Text.trim()) acc.texts.push(raw.Text);
        return;
    }
    if (raw.type === 'text' && typeof raw.text === 'string') {
        if (raw.text.trim()) acc.texts.push(raw.text);
        return;
    }
    if (raw.type === 'content' && raw.content && typeof raw.content === 'object') {
        collectOutput(raw.content, acc, depth + 1);
        return;
    }
    if (isContentBlock(raw)) return; // image / resource: nothing to show as text
    if (Array.isArray(raw.content) && raw.content.every(isContentBlock)) {
        // MCP tool result: { content: [...], isError?, structuredContent? }.
        for (const block of raw.content) collectOutput(block, acc, depth + 1);
        if (raw.isError === true) acc.failed = true;
        if (raw.structuredContent && typeof raw.structuredContent === 'object') {
            acc.json.push(raw.structuredContent);
        }
        return;
    }
    acc.json.push(raw);
}

function isItem(x) {
    if (!x || typeof x !== 'object' || Array.isArray(x)) return false;
    const keys = Object.keys(x);
    return keys.length === 1 && (keys[0] === 'Text' || keys[0] === 'Json');
}

/**
 * Parse an ACP `rawOutput` (or content) value into `{ text, json, failed }`
 * — `text` the joined text parts (unsanitised), `json` the structured part
 * (one value, or an array of them) — or `null` when there's nothing.
 */
export function parseToolOutput(raw) {
    const acc = { texts: [], json: [], failed: false };
    collectOutput(raw, acc, 0);
    if (!acc.texts.length && !acc.json.length && !acc.failed) return null;
    return {
        text: acc.texts.join('\n'),
        json: acc.json.length === 0 ? undefined : acc.json.length === 1 ? acc.json[0] : acc.json,
        failed: acc.failed,
    };
}

/**
 * ACP content blocks of a call: text blocks (as output) and diff blocks.
 * Terminal blocks only belong to shell calls.
 */
function parseContentBlocks(content) {
    const texts = [];
    const diffs = [];
    if (!Array.isArray(content)) return { texts, diffs };
    for (const block of content) {
        if (!block || typeof block !== 'object') continue;
        if (block.type === 'diff') diffs.push(block);
        else if (block.type === 'content' && block.content?.type === 'text') {
            if (typeof block.content.text === 'string' && block.content.text.trim()) {
                texts.push(block.content.text);
            }
        } else if (block.type === 'text' && typeof block.text === 'string' && block.text.trim()) {
            texts.push(block.text);
        }
    }
    return { texts, diffs };
}

/** Strip one fence wrapping the whole text (```lang … ```). */
function unfence(text) {
    const m = /^\s*```[\w+-]*[ \t]*\n([\s\S]*?)\n?```\s*$/.exec(text);
    return m ? m[1] : text;
}

/**
 * The stored form of a parsed output within `budget` characters:
 * `{ text, json, jsonTruncated, failed, dropped, size }` — `text` a capped
 * segment (sanitised) or `null`, `json` a capped clean copy or `undefined`.
 */
function storeOutput(parsed, budget) {
    const out = {
        text: null,
        json: undefined,
        jsonTruncated: false,
        failed: !!parsed.failed,
        dropped: false,
        size: 0,
    };
    let left = Math.max(0, Math.floor(budget));
    const text = parsed.text ? unfence(parsed.text) : '';
    if (text && left === 0) out.dropped = !!text.trim();
    else if (text) {
        out.text = detachSegment(sanitizeAndCap(text, left));
        out.size += segmentLength(out.text);
        left -= out.size;
    }
    if (parsed.json !== undefined) {
        if (left <= 0) out.dropped = true;
        else {
            const c = capJson(parsed.json, left, { clean: true });
            out.json = c.value;
            out.jsonTruncated = c.truncated;
            out.size += c.size;
        }
    }
    return out;
}

function outputSize(o) {
    return o ? o.size : 0;
}

// --- Records ------------------------------------------------------------

const MAX_LOCATIONS = 20;

function storeLocations(locations) {
    if (!Array.isArray(locations)) return null;
    const out = [];
    for (const loc of locations) {
        if (out.length >= MAX_LOCATIONS) break;
        const path = str(loc?.path);
        if (!path) continue;
        const line = Number.isInteger(loc.line) && loc.line >= 0 ? loc.line : null;
        out.push({ path: clipString(path, 4096).s, line });
    }
    return out;
}

/** Characters of output (and arguments) a record holds. */
export function toolCallSize(rec) {
    if (!rec) return 0;
    let n = rec.inputSize || 0;
    n += outputSize(rec.rawOut) + outputSize(rec.contentOut);
    for (const d of rec.diffs || []) n += d.size;
    return n;
}

/**
 * Merge one ACP `tool_call` / `tool_call_update` into a tool-usage `entry`
 * (from `processToolCallUpdate`) that isn't a shell command, keeping
 * `entry.call` — the normalised record — up to date, and return it. ACP
 * semantics: fields an update omits are unchanged; `rawInput`,
 * `rawOutput`, `content` and `locations` replace the previous value.
 *
 * `opts.outputBudget` caps what this call may store (the caller deducts
 * what the response's other calls already hold).
 */
export function applyToolUpdate(entry, update, opts = {}) {
    if (!entry || !update) return null;
    let rec = entry.call || null;
    if (!rec) {
        rec = {
            toolCallId: entry.toolCallId,
            firstTitle: typeof entry.title === 'string' ? entry.title : '',
            title: typeof entry.title === 'string' ? entry.title : '',
            rawKind: entry.kind || null,
            category: 'other',
            kind: 'other',
            mcp: null,
            // A built-in tool of the agent (known from the session log).
            builtin: false,
            status: null,
            input: undefined,
            inputTruncated: false,
            inputSize: 0,
            purpose: '',
            diffs: [],
            diffsFromContent: false,
            locations: null,
            rawOut: null,
            contentOut: null,
            settled: false,
            rev: 0,
        };
        entry.call = rec;
    }
    const budget = Math.min(TOOL_OUTPUT_CAP, Math.max(0, opts.outputBudget ?? TOOL_OUTPUT_CAP));
    // What's left of this call's budget once the field being replaced is
    // cleared (callers clear it first).
    const room = () => Math.max(0, budget - toolCallSize(rec));
    let changed = rec.rev === 0;

    if (typeof update.title === 'string' && update.title && update.title !== rec.title) {
        rec.title = update.title;
        changed = true;
    }
    if (update.kind && update.kind !== rec.rawKind) {
        rec.rawKind = update.kind;
        changed = true;
    }
    const cls = classifyToolCall(rec.rawKind || entry.kind, rec.firstTitle, rec.title);
    if (
        cls.category !== rec.category ||
        cls.kind !== rec.kind ||
        cls.mcp?.server !== rec.mcp?.server ||
        cls.mcp?.tool !== rec.mcp?.tool
    ) {
        rec.category = cls.category;
        rec.kind = cls.kind;
        rec.mcp = cls.mcp;
        changed = true;
    }
    if (KNOWN_STATUSES.has(update.status) && update.status !== rec.status) {
        rec.status = update.status;
        changed = true;
    }
    if ('locations' in update) {
        rec.locations = storeLocations(update.locations);
        changed = true;
    }

    const content = 'content' in update ? parseContentBlocks(update.content) : null;
    if (content?.diffs.length) {
        // ACP diff content wins over diffs read from the arguments.
        rec.diffs = [];
        rec.diffsFromContent = true;
        for (const d of content.diffs.slice(0, 20)) {
            rec.diffs.push(buildDiff(d.path, d.oldText ?? null, d.newText, { budget: room() }));
        }
        changed = true;
    }

    if ('rawInput' in update && update.rawInput !== undefined) {
        const input = update.rawInput;
        if (input && typeof input === 'object' && typeof input.__tool_use_purpose === 'string') {
            rec.purpose = clipString(sanitizeTerminalText(input.__tool_use_purpose), 1000).s.trim();
        }
        rec.input = undefined;
        rec.inputSize = 0;
        if (!rec.diffsFromContent && rec.category === 'edit') {
            rec.diffs = [];
            rec.diffs = diffsFromEditInput(input, room());
        }
        let shown = displayInput(input, rec.diffs.length > 0);
        if (shown && typeof shown === 'object' && !Array.isArray(shown)) {
            shown = { ...shown };
            delete shown.__tool_use_purpose;
        }
        const c = capJson(shown, Math.min(TOOL_INPUT_CAP, room()));
        rec.input = c.value;
        rec.inputTruncated = c.truncated;
        rec.inputSize = c.size;
        changed = true;
    }

    if ('rawOutput' in update) {
        const parsed = parseToolOutput(update.rawOutput);
        rec.rawOut = null;
        rec.rawOut = parsed ? storeOutput(parsed, room()) : null;
        if (hasStored(rec.rawOut)) {
            // rawOutput is what shows; don't hold content's copy as well.
            rec.contentOut = null;
        }
        changed = true;
    }
    if (content) {
        rec.contentOut = null;
        if (content.texts.length && !hasStored(rec.rawOut)) {
            rec.contentOut = storeOutput({ text: content.texts.join('\n') }, room());
        }
        changed = true;
    }
    if (changed) rec.rev += 1;
    return rec;
}

function hasStored(o) {
    return !!(o && (o.text || o.json !== undefined));
}

/**
 * The output to show for a record: `rawOutput`'s, else content's. `null`
 * when neither has arrived.
 */
export function toolOutputOf(rec) {
    if (!rec) return null;
    const raw = rec.rawOut;
    if (raw && (raw.text || raw.json !== undefined || raw.dropped)) return raw;
    const content = rec.contentOut;
    if (content) return { ...content, failed: content.failed || !!raw?.failed };
    return raw || null;
}

/**
 * Display state of a record: `running`, `success`, `failed`, or `unknown`
 * (the turn ended without the agent saying how the call finished).
 */
export function toolCallState(rec) {
    if (!rec) return 'unknown';
    if (rec.status === 'failed' || toolOutputOf(rec)?.failed) return 'failed';
    if (rec.status === 'completed') return 'success';
    return rec.settled ? 'unknown' : 'running';
}

/** Non-shell tool records of a response's tool usages, in call order. */
export function getToolCalls(toolUsages) {
    if (!Array.isArray(toolUsages)) return [];
    const out = [];
    for (const u of toolUsages) if (u?.call && !u.shell) out.push(u.call);
    return out;
}

/**
 * The turn is over: any call still without a terminal status never will
 * get one. Returns whether any changed.
 */
export function settleToolCalls(toolUsages) {
    let changed = false;
    for (const rec of getToolCalls(toolUsages)) {
        if (!rec.settled && rec.status !== 'completed' && rec.status !== 'failed') {
            rec.settled = true;
            rec.rev += 1;
            changed = true;
        }
    }
    return changed;
}

/**
 * A response's calls grouped by chip, in order of first appearance:
 * `[{ key, label, kind, mcp, calls }]` (see `toolChipGroup`).
 */
export function groupToolCalls(toolUsages) {
    const groups = new Map();
    for (const rec of getToolCalls(toolUsages)) {
        const g = toolChipGroup(rec);
        if (!g) continue;
        let group = groups.get(g.key);
        if (!group) {
            group = { ...g, calls: [] };
            groups.set(g.key, group);
        }
        group.calls.push(rec);
    }
    return Array.from(groups.values());
}

// --- What a call is about (for the view) ---------------------------------

function inputObj(rec) {
    const v = rec?.input;
    return v && typeof v === 'object' && !Array.isArray(v) ? v : null;
}

/** File paths a call names: diffs, then arguments, then ACP locations. */
export function toolCallPaths(rec) {
    const out = [];
    const add = (p) => {
        if (typeof p === 'string' && p && p !== JSON_CUT && !out.includes(p)) out.push(p);
    };
    for (const d of rec?.diffs || []) add(d.path);
    const input = inputObj(rec);
    if (input) {
        for (const k of PATH_KEYS) add(input[k]);
        for (const k of ['source', 'destination', 'from', 'to', 'old_path', 'new_path']) {
            add(input[k]);
        }
        if (Array.isArray(input.operations)) {
            for (const op of input.operations) add(pick(op, PATH_KEYS));
        }
        if (Array.isArray(input.paths)) for (const p of input.paths) add(p);
    }
    for (const loc of rec?.locations || []) add(loc.path);
    return out.slice(0, MAX_LOCATIONS);
}

function intOf(v) {
    return Number.isInteger(v) ? v : null;
}

function countOf(v) {
    return Number.isInteger(v) && v >= 0 ? v : null;
}

/**
 * Whether `op` is one of kiro-cli's `read` operations in line mode (`{
 * mode: "Line", path, offset?, limit? }`), whose `offset` is the number of
 * lines skipped — `offset: 1275` starts at line 1276 (checked against a
 * real read) — and `limit` the number of lines read.
 */
function isKiroLineRead(op) {
    return op.mode === 'Line' && !!pick(op, PATH_KEYS);
}

/**
 * The 1-based line range of a kiro line-mode read's `offset` / `limit`, or
 * `null` when it reads from the top with no limit (the whole file).
 */
function kiroLineRange(op) {
    const offset = countOf(op.offset);
    const limit = countOf(op.limit);
    if (!offset && !limit) return null;
    const start = (offset ?? 0) + 1;
    return { start, end: limit ? start + limit - 1 : null };
}

/**
 * Line ranges a read names, when it says so unambiguously:
 * `[{ path, start, end }]` (1-based, inclusive; either end may be `null`)
 * from `start_line` / `end_line` (Amazon Q / older kiro), kiro-cli's
 * line-mode `operations` (`offset` / `limit`, see `isKiroLineRead`), or an
 * ACP location's line. Other agents' `offset` / `limit` arguments stay in
 * the argument list — they count them differently (Claude Code's `offset`
 * is the first line's number).
 */
export function readRanges(rec) {
    const out = [];
    const input = inputObj(rec);
    const kiroOps = !!input && Array.isArray(input.operations);
    const ops = input
        ? kiroOps
            ? input.operations.filter((o) => o && typeof o === 'object' && !Array.isArray(o))
            : [input]
        : [];
    for (const op of ops) {
        const path = pick(op, PATH_KEYS);
        if (!path) continue;
        const start = intOf(op.start_line ?? op.startLine);
        const end = intOf(op.end_line ?? op.endLine);
        if (start !== null || end !== null) out.push({ path, start, end });
        else if (kiroOps && isKiroLineRead(op)) {
            const r = kiroLineRange(op);
            if (r) out.push({ path, ...r });
        }
    }
    if (!out.length) {
        for (const loc of rec?.locations || []) {
            if (loc.line !== null) out.push({ path: loc.path, start: loc.line, end: null });
        }
    }
    return out;
}

/** What a search looked for and where: `{ query, scope }`. */
export function searchQueryOf(rec) {
    const input = inputObj(rec);
    const queryKey = pickKey(input, [
        'query',
        'pattern',
        'search_terms',
        'q',
        'regex',
        'symbol_name',
    ]);
    const scopeKey = pickKey(input, ['path', 'include', 'glob', 'directory', 'domain']);
    return {
        query: queryKey ? input[queryKey] : null,
        scope: scopeKey ? input[scopeKey] : null,
        // The argument names they came from (the view doesn't list them twice).
        keys: [queryKey, scopeKey].filter(Boolean),
    };
}

/** The first of `keys` that names a non-empty string in `obj`. */
function pickKey(obj, keys) {
    if (!obj) return null;
    for (const k of keys) if (str(obj[k]) && obj[k] !== JSON_CUT) return k;
    return null;
}

// A read operation's own keys that the view already shows (file, range).
const READ_SHOWN_KEYS = new Set([
    ...PATH_KEYS,
    'mode',
    'start_line',
    'end_line',
    'startLine',
    'endLine',
]);
// ...and, for kiro's line mode, the range's `offset` / `limit`.
const KIRO_LINE_SHOWN_KEYS = new Set(['offset', 'limit']);

/**
 * Whether a read's `operations` argument says nothing the view doesn't
 * show already (every operation only names a file, line range and mode).
 */
export function readOperationsShown(rec) {
    const ops = inputObj(rec)?.operations;
    if (!Array.isArray(ops) || !ops.length) return false;
    return ops.every(
        (op) =>
            op &&
            typeof op === 'object' &&
            !Array.isArray(op) &&
            pick(op, PATH_KEYS) &&
            Object.keys(op).every(
                (k) =>
                    READ_SHOWN_KEYS.has(k) ||
                    (KIRO_LINE_SHOWN_KEYS.has(k) &&
                        isKiroLineRead(op) &&
                        (op[k] === null || countOf(op[k]) !== null))
            )
    );
}

/** URLs a fetch names. */
export function fetchUrlsOf(rec) {
    const out = [];
    const input = inputObj(rec);
    const add = (u) => {
        if (typeof u === 'string' && u && u !== JSON_CUT && !out.includes(u)) out.push(u);
    };
    if (input) {
        add(input.url);
        add(input.uri);
        if (Array.isArray(input.urls)) for (const u of input.urls) add(u);
        if (Array.isArray(input.inputs)) for (const u of input.inputs) add(u);
    }
    return out.slice(0, MAX_LOCATIONS);
}

const MAX_SOURCES = 50;

/**
 * Result sources of a search, from its JSON output: web results (`{ title,
 * url }`), grep files (`{ file, count }`) and glob paths. `[{ title, url,
 * path, count }]`, at most 50; `more` counts the rest.
 */
export function searchSourcesOf(rec) {
    const out = [];
    let more = 0;
    const json = toolOutputOf(rec)?.json;
    const lists = [];
    const walk = (v, depth) => {
        if (!v || typeof v !== 'object' || depth > 3) return;
        if (Array.isArray(v)) {
            for (const x of v) walk(x, depth + 1);
            return;
        }
        for (const k of ['results', 'searchResults', 'filePaths', 'files', 'matches']) {
            if (Array.isArray(v[k])) lists.push(v[k]);
        }
    };
    walk(json, 0);
    for (const list of lists) {
        for (const item of list) {
            if (item === JSON_CUT) continue;
            let src = null;
            if (typeof item === 'string') src = { path: item };
            else if (item && typeof item === 'object') {
                const url = str(item.url) || str(item.link);
                const path = str(item.file) || str(item.path) || str(item.filePath);
                if (url) src = { url, title: str(item.title) };
                else if (path) {
                    src = { path, count: Number.isInteger(item.count) ? item.count : null };
                }
            }
            if (!src) continue;
            if (out.length >= MAX_SOURCES) more += 1;
            else out.push(src);
        }
    }
    return { sources: out, more };
}
