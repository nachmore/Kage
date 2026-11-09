/**
 * Shell command tool calls — pure data layer (no DOM).
 *
 * Agents report shell runs as ACP tool calls of kind `execute`. This module
 * recognises them, pulls out the command line, and parses whatever output
 * the agent reports into one normalised record per call, so the floating
 * and chat windows can render the same command block (see
 * `shell-command-view.js`).
 *
 * Output shapes handled (everything else yields no output — raw JSON is
 * never dumped):
 *   - kiro-cli: `rawOutput = { items: [{ Json: { exit_status: "exit status: 0",
 *     stdout, stderr } }] }` (its session log stores exactly that), also
 *     `{ items: [{ Text }] }` and a `{ Success: { items } }` wrapper.
 *   - Flat objects: `{ stdout, stderr, exit_code | exitCode | exit_status }`,
 *     `{ output | aggregated_output | formatted_output }`.
 *   - Plain-string `rawOutput`, or a string holding one of the JSON shapes.
 *   - ACP content blocks: `[{ type: "content", content: { type: "text",
 *     text } }]` (Claude Code's adapter fences it as ```console), and
 *     `[{ type: "terminal", terminalId }]` — Kage doesn't implement ACP
 *     terminals, so that only flags "output not available".
 *
 * Command and output are untrusted agent data, and the view only ever
 * renders them as text. Output is stripped of ANSI escapes and control
 * characters and capped in size. A command is never altered: it is stored
 * as the agent sent it (Copy command copies exactly that), and shown with
 * `shellCommandDisplayParts`, which makes every control, bidi, zero-width
 * and other invisible character visible instead of removing or applying
 * it — what the user reads is what runs.
 */

/** Output kept per tool call (stdout + stderr), in characters. */
export const SHELL_OUTPUT_CAP = 64 * 1024;
/** Output kept across all shell calls of one response, in characters. */
export const SHELL_RESPONSE_OUTPUT_CAP = 512 * 1024;

/** ACP tool kind for shell / CLI commands (see src/config/permissions.rs). */
const KIND_EXECUTE = 'execute';
const KNOWN_STATUSES = new Set(['pending', 'in_progress', 'completed', 'failed']);
// kiro-cli titles shell runs `Running: <command>` and MCP tool calls
// `Running: @<server>/<tool>` — the latter is never a shell command.
const KIRO_RUNNING_PREFIX = /^Running:\s+/;

// --- Sanitising -----------------------------------------------------------

// The control characters are spliced in from char codes (as in
// chat-export.js) so Biome's `noControlCharactersInRegex` stays quiet —
// matching control characters is the whole point here.
const ESC = String.fromCharCode(0x1b);
const BEL = String.fromCharCode(0x07);
const CSI8 = String.fromCharCode(0x9b);
const ctrlRange = (from, to) => `${String.fromCharCode(from)}-${String.fromCharCode(to)}`;

// OSC (ESC ] … BEL | ESC \). An unterminated one stops at the next ESC
// rather than swallowing the rest of the output.
const RE_OSC = new RegExp(`${ESC}\\][^${BEL}${ESC}]*(?:${BEL}|${ESC}\\\\)?`, 'g');
// DCS / SOS / PM / APC strings (ESC P|X|^|_ … ESC \).
const RE_STRING_SEQ = new RegExp(`${ESC}[PX^_][^${ESC}]*(?:${ESC}\\\\)?`, 'g');
// CSI (ESC [ params intermediates final), plus the 8-bit C1 form.
const RE_CSI = new RegExp(`(?:${ESC}\\[|${CSI8})[0-?]*[ -/]*[@-~]`, 'g');
// Any other escape: nF (ESC intermediates final) and two-byte Fp/Fe/Fs.
const RE_OTHER_ESC = new RegExp(`${ESC}[ -/]*[0-~]`, 'g');
// C0/C1 controls except \t and \n (\r is resolved before this runs).
const RE_CONTROL = new RegExp(
    `[${ctrlRange(0x00, 0x08)}${ctrlRange(0x0b, 0x1f)}${ctrlRange(0x7f, 0x9f)}]`,
    'g'
);
// Bidi embedding/override/isolate controls: they can make displayed text
// read differently from what runs ("Trojan Source").
const RE_BIDI = /[\u202a-\u202e\u2066-\u2069]/g;

/**
 * Overlay `\r`-separated segments the way a terminal does: each segment
 * overwrites the start of the line (progress bars, spinners).
 */
function resolveCarriageReturns(line) {
    if (!line.includes('\r')) return line;
    let out = '';
    for (const seg of line.split('\r')) {
        out = seg + out.slice(seg.length);
    }
    return out;
}

/**
 * Strip terminal escape sequences (CSI, OSC, DCS/APC, other ESC) and control
 * characters from `s`, keeping `\n` and `\t`. Carriage returns are resolved
 * terminal-style. Non-strings yield `''`.
 */
export function sanitizeTerminalText(s) {
    if (typeof s !== 'string' || !s) return '';
    let out = s
        .replace(RE_OSC, '')
        .replace(RE_STRING_SEQ, '')
        .replace(RE_CSI, '')
        .replace(RE_OTHER_ESC, '');
    out = out.replace(/\r\n/g, '\n');
    if (out.includes('\r')) out = out.split('\n').map(resolveCarriageReturns).join('\n');
    return out.replace(RE_CONTROL, '').replace(RE_BIDI, '');
}

// --- Capping --------------------------------------------------------------

/** Move `i` off the low half of a surrogate pair so a cut never splits one. */
function safeCut(s, i) {
    if (i > 0 && i < s.length) {
        const c = s.charCodeAt(i);
        if (c >= 0xdc00 && c <= 0xdfff) return i - 1;
    }
    return i;
}

/**
 * Cap `s` at `max` characters, keeping its head and tail. Returns
 * `{ head, tail, omitted }` — `tail` is `''` and `omitted` is 0 when `s`
 * fits. Cuts snap to a nearby line break so the omission marker sits on
 * its own line, and never split a surrogate pair.
 */
export function capText(s, max) {
    const text = typeof s === 'string' ? s : '';
    const limit = Math.max(0, Math.floor(max));
    if (text.length <= limit) return { head: text, tail: '', omitted: 0 };
    if (limit === 0) return { head: '', tail: '', omitted: text.length };
    let headEnd = Math.ceil(limit / 2);
    let tailStart = text.length - (limit - headEnd);
    const SNAP = Math.min(200, Math.floor(limit / 8));
    const nl = text.lastIndexOf('\n', headEnd);
    if (nl > 0 && headEnd - nl <= SNAP) headEnd = nl + 1;
    const nl2 = text.indexOf('\n', tailStart);
    if (nl2 !== -1 && nl2 - tailStart <= SNAP) tailStart = nl2 + 1;
    headEnd = safeCut(text, headEnd);
    tailStart = safeCut(text, tailStart);
    if (tailStart < headEnd) tailStart = headEnd;
    return {
        head: text.slice(0, headEnd),
        tail: text.slice(tailStart),
        omitted: tailStart - headEnd,
    };
}

/** Characters a capped segment keeps. */
export function cappedLength(seg) {
    return seg ? seg.head.length + seg.tail.length : 0;
}

/**
 * Sanitise then cap. Huge input is trimmed to a head and tail slice first,
 * so the escape-stripping regexes never run over megabytes. Returns `null`
 * when nothing printable is left.
 */
export function sanitizeAndCap(raw, max) {
    if (typeof raw !== 'string' || !raw) return null;
    const limit = Math.max(0, Math.floor(max));
    const pre = limit * 2 + 8192;
    if (raw.length <= pre) {
        const clean = sanitizeTerminalText(raw);
        if (!clean.trim()) return null;
        return capText(clean, limit);
    }
    const half = Math.floor(pre / 2);
    const headClean = sanitizeTerminalText(raw.slice(0, safeCut(raw, half)));
    const tailClean = sanitizeTerminalText(raw.slice(safeCut(raw, raw.length - half)));
    const keepHead = Math.ceil(limit / 2);
    const keepTail = limit - keepHead;
    const head = headClean.slice(0, safeCut(headClean, keepHead));
    const tail =
        keepTail > 0 ? tailClean.slice(safeCut(tailClean, tailClean.length - keepTail)) : '';
    const omitted = raw.length - head.length - tail.length;
    return { head, tail, omitted: Math.max(1, omitted) };
}

/** Full text of a capped segment, with `marker(omitted)` between head and tail. */
export function joinCapped(seg, marker) {
    if (!seg) return '';
    if (!seg.omitted) return seg.head + seg.tail;
    const m = marker ? marker(seg.omitted) : `… ${seg.omitted} characters omitted …`;
    const sep1 = seg.head && !seg.head.endsWith('\n') ? '\n' : '';
    const sep2 = seg.tail ? '\n' : '';
    return `${seg.head}${sep1}${m}${sep2}${seg.tail}`;
}

// --- Recognising shell calls and their command ---------------------------

/**
 * Whether a tool call is a shell command: ACP kind `execute`, or — for an
 * agent that leaves the kind out — kiro-cli's `Running: <command>` title.
 * kiro titles MCP calls `Running: @server/tool`; those (and any other
 * explicit kind) are never shell.
 */
export function isShellToolCall(kind, title) {
    if (kind === KIND_EXECUTE) return true;
    if (kind && kind !== 'other') return false;
    if (typeof title !== 'string') return false;
    const m = KIRO_RUNNING_PREFIX.exec(title);
    return !!m && title.length > m[0].length && title[m[0].length] !== '@';
}

const SAFE_ARG = /^[\w@%+=:,./-]+$/;

function quoteArg(a) {
    if (a === '') return "''";
    return SAFE_ARG.test(a) ? a : `'${a.replace(/'/g, "'\\''")}'`;
}

const POSIX_SHELL = /^(?:ba|z|da|k|fi)?sh$/;
// Where a system / package-manager shell lives. A shell anywhere else (say
// /tmp/x/bash) is a different program: its argv is shown in full.
const SHELL_DIRS = new Set(['/bin', '/usr/bin', '/usr/local/bin', '/opt/homebrew/bin']);

function isSystemShell(arg) {
    if (POSIX_SHELL.test(arg)) return true;
    const slash = arg.lastIndexOf('/');
    return (
        slash > 0 && SHELL_DIRS.has(arg.slice(0, slash)) && POSIX_SHELL.test(arg.slice(slash + 1))
    );
}

/**
 * `["bash", "-lc", "ls"]` → `ls`: a system shell's `-c` / `-lc` script is
 * the command (also via `/usr/bin/env bash`, and `pwsh` / `powershell
 * -Command`). Any other argv — an unknown interpreter path, extra flags or
 * arguments — is shell-quoted and joined in full, so the program that runs
 * is always on screen.
 */
function argvToCommand(argv) {
    const parts = argv.filter((a) => typeof a === 'string');
    if (parts.length !== argv.length || parts.length === 0) return '';
    const rest =
        parts.length === 4 && /^(?:\/usr\/bin\/)?env$/.test(parts[0]) ? parts.slice(1) : parts;
    if (rest.length === 3) {
        const [shell, flag, script] = rest;
        if (isSystemShell(shell) && (flag === '-c' || flag === '-lc')) return script;
        if (
            rest === parts &&
            /^(?:pwsh|powershell)(?:\.exe)?$/i.test(shell) &&
            /^-(?:c|Command)$/i.test(flag)
        ) {
            return script;
        }
    }
    return parts.map(quoteArg).join(' ');
}

/**
 * The command line of a shell call: `rawInput.command` (string or argv
 * array), else the title — kiro's `Running: <command>`, or Claude Code's
 * backticked `` `command` ``. `''` when neither names a command (e.g. a
 * bare `shell` title before the input arrives).
 */
export function extractShellCommand(rawInput, title) {
    if (rawInput && typeof rawInput === 'object') {
        let c = rawInput.command ?? rawInput.cmd;
        if (Array.isArray(c)) c = argvToCommand(c);
        if (typeof c === 'string' && c.trim()) return c;
    }
    if (typeof title === 'string') {
        const s = title.trim();
        const m = KIRO_RUNNING_PREFIX.exec(s);
        if (m) {
            const rest = s.slice(m[0].length);
            return rest.startsWith('@') ? '' : rest;
        }
        const b = /^`([\s\S]+)`$/.exec(s);
        if (b) return b[1].replace(/\\`/g, '`');
    }
    return '';
}

const CWD_FIELDS = ['cwd', 'working_dir', 'workdir', 'workingDirectory', 'working_directory'];
// POSIX absolute, `~` / `~/…`, a Windows drive path or a UNC path.
const ABSOLUTE_DIR = /^(?:\/|~(?:\/|$)|[A-Za-z]:[\\/]|\\\\)/;

/**
 * The directory a shell call ran in, when its input says (Codex's `cwd`,
 * other agents' `working_dir` / `workdir` / …): an absolute path, else
 * `''`. Where "Run it here" runs the command (the backend checks it
 * exists; without one it runs in the home folder).
 */
export function extractShellCwd(rawInput) {
    if (!rawInput || typeof rawInput !== 'object') return '';
    for (const key of CWD_FIELDS) {
        const v = rawInput[key];
        if (typeof v !== 'string') continue;
        const dir = v.trim();
        if (dir && dir.length <= 4096 && ABSOLUTE_DIR.test(dir)) return dir;
    }
    return '';
}

// --- Displaying commands ---------------------------------------------------

// Characters a command line can't be shown as-is, because they're invisible
// or make the text read differently from what the shell runs: C0/C1
// controls except \t and \n (ESC, CR, BEL, …) and DEL; format characters
// (bidi controls, zero-width characters, BOM, soft hyphen, tag characters,
// …); line/paragraph separators (they look like line breaks, the shell
// doesn't treat them as such); spaces other than U+0020 (they look like a
// word break and aren't one); Hangul fillers; variation selectors; lone
// surrogates. One run of them becomes one mark.
const RE_HIDDEN_RUN = new RegExp(
    `[${ctrlRange(0x00, 0x08)}${ctrlRange(0x0b, 0x1f)}${ctrlRange(0x7f, 0x9f)}` +
        '\\p{Cf}\\p{Zl}\\p{Zp}\\p{Cs}\\u00a0\\u1680\\u2000-\\u200a\\u202f\\u205f\\u3000' +
        '\\u115f\\u1160\\u3164\\uffa0\\ufe00-\\ufe0f\\u{e0100}-\\u{e01ef}]+',
    'gu'
);
// Two or more blank lines in a row: shown as one counted mark, so a run of
// empty lines can't push the rest of a command out of view.
const RE_BLANK_LINES = /\n(?:[ \t]*\n){2,}/g;
const HIDDEN_RUN_LIMIT = 8;

function codePointLabel(cp) {
    return `U+${cp.toString(16).toUpperCase().padStart(4, '0')}`;
}

/** Visible stand-in for one hidden character: a Control Picture or `<U+XXXX>`. */
function hiddenGlyph(cp) {
    if (cp < 0x20) return String.fromCharCode(0x2400 + cp);
    if (cp === 0x7f) return '␡';
    return `<${codePointLabel(cp)}>`;
}

/** A mark for a run of hidden characters (repeats of one character counted). */
function hiddenRunMark(run) {
    const groups = [];
    for (const ch of run) {
        const cp = ch.codePointAt(0);
        const last = groups[groups.length - 1];
        if (last && last.cp === cp) last.count += 1;
        else groups.push({ cp, count: 1 });
    }
    const mark = groups
        .map((g) =>
            g.count > 3 ? `${hiddenGlyph(g.cp)}×${g.count}` : hiddenGlyph(g.cp).repeat(g.count)
        )
        .join('');
    const shown = groups.slice(0, HIDDEN_RUN_LIMIT);
    let title = shown
        .map((g) => codePointLabel(g.cp) + (g.count > 1 ? ` ×${g.count}` : ''))
        .join(' ');
    if (groups.length > shown.length) title += ' …';
    return { mark, title };
}

function pushTextParts(parts, text) {
    let last = 0;
    RE_HIDDEN_RUN.lastIndex = 0;
    for (let m = RE_HIDDEN_RUN.exec(text); m; m = RE_HIDDEN_RUN.exec(text)) {
        if (m.index > last) parts.push({ text: text.slice(last, m.index) });
        parts.push(hiddenRunMark(m[0]));
        last = m.index + m[0].length;
    }
    if (last < text.length) parts.push({ text: text.slice(last) });
}

/**
 * Split a command line into display parts: `{ text }` runs to show as-is
 * and `{ mark, title }` stand-ins for characters that can't be — every
 * control character (ESC as `␛`, CR as `␍`, …), bidi / zero-width /
 * other invisible character (`<U+202E>`), and runs of blank lines (`⏎×N`).
 * Nothing is dropped or applied: unlike the output sanitiser, an escape
 * sequence or carriage return never hides what follows it. `\n` and `\t`
 * stay text. The view renders marks distinctly so they can't pass for
 * literal text.
 */
export function shellCommandDisplayParts(command) {
    const s = typeof command === 'string' ? command : '';
    const parts = [];
    let last = 0;
    RE_BLANK_LINES.lastIndex = 0;
    for (let m = RE_BLANK_LINES.exec(s); m; m = RE_BLANK_LINES.exec(s)) {
        pushTextParts(parts, s.slice(last, m.index));
        const breaks = m[0].split('\n').length - 1;
        parts.push({ text: '\n' });
        parts.push({ mark: `⏎×${breaks}`, title: `${codePointLabel(0x0a)} ×${breaks}` });
        parts.push({ text: '\n' });
        last = m.index + m[0].length;
    }
    pushTextParts(parts, s.slice(last));
    return parts;
}

/** Whether `command` displays exactly as written (no marks needed). */
export function isPlainShellCommand(command) {
    return shellCommandDisplayParts(command).every((p) => 'text' in p);
}

/** The displayed form of `command` as one string (marks inline), e.g. for a tooltip. */
export function shellCommandDisplayText(command) {
    return shellCommandDisplayParts(command)
        .map((p) => ('text' in p ? p.text : p.mark))
        .join('');
}

// --- Parsing output -------------------------------------------------------

/** `{ code }` / `{ signal }` from an exit status value, or `null`. */
export function parseExitStatus(v) {
    if (typeof v === 'number' && Number.isInteger(v)) return { code: v };
    if (typeof v !== 'string') return null;
    const s = v.trim();
    let m = /^(?:exit (?:status|code):?\s*)?(-?\d+)$/i.exec(s);
    if (m) return { code: Number.parseInt(m[1], 10) };
    m = /signal:?\s*(\d+)/i.exec(s);
    if (m) {
        const label = sanitizeTerminalText(s).slice(0, 64);
        return { signal: label || `signal ${m[1]}` };
    }
    return null;
}

const EXIT_FIELDS = [
    'exit_status',
    'exitStatus',
    'exit_code',
    'exitCode',
    'returncode',
    'returnCode',
];
const COMBINED_FIELDS = ['output', 'aggregated_output', 'formatted_output'];

function emptyParse() {
    return { stdout: '', stderr: '', exit: null, failed: false, terminal: false, found: false };
}

function mergeParse(into, part) {
    if (!part) return into;
    if (part.stdout) into.stdout = into.stdout ? `${into.stdout}\n${part.stdout}` : part.stdout;
    if (part.stderr) into.stderr = into.stderr ? `${into.stderr}\n${part.stderr}` : part.stderr;
    if (part.exit) into.exit = part.exit;
    into.failed = into.failed || part.failed;
    into.terminal = into.terminal || part.terminal;
    into.found = into.found || part.found;
    return into;
}

function textParse(s) {
    const p = emptyParse();
    p.stdout = s;
    p.found = true;
    return p;
}

/** Strip one fence wrapping the whole text (```console … ```). */
function unfence(text) {
    const m = /^\s*```[\w+-]*[ \t]*\n([\s\S]*?)\n?```\s*$/.exec(text);
    return m ? m[1] : text;
}

function parseOutputObject(obj) {
    const p = emptyParse();
    const str = (k) => (typeof obj[k] === 'string' ? obj[k] : null);
    const stdout = str('stdout');
    const stderr = str('stderr');
    if (stdout !== null || stderr !== null) {
        p.stdout = stdout || '';
        p.stderr = stderr || '';
        p.found = true;
    } else {
        for (const k of COMBINED_FIELDS) {
            if (str(k) !== null) {
                p.stdout = obj[k];
                p.found = true;
                break;
            }
        }
    }
    for (const k of EXIT_FIELDS) {
        const exit = parseExitStatus(obj[k]);
        if (exit) {
            p.exit = exit;
            p.found = true;
            break;
        }
    }
    return p.found ? p : null;
}

// Bounds recursion on hostile nesting. The deepest real shape — kiro's
// Success wrapper sent as a JSON string, with MCP-style content inside —
// reaches its text block at depth 7: string, { Success }, { items }, [ ],
// { Json }, [ content ], { type: "text" }.
const PARSE_DEPTH_LIMIT = 10;

function parseRaw(raw, depth) {
    if (raw == null || depth > PARSE_DEPTH_LIMIT) return null;
    if (typeof raw === 'string') {
        const s = raw.trim();
        if (!s) return null;
        if (s[0] === '{' || s[0] === '[') {
            try {
                // A JSON string: only its recognised shapes count.
                return parseRaw(JSON.parse(s), depth + 1);
            } catch {
                /* not JSON — plain text */
            }
        }
        return textParse(unfence(raw));
    }
    if (Array.isArray(raw)) {
        const p = emptyParse();
        for (const item of raw) mergeParse(p, parseRaw(item, depth + 1));
        return p.found ? p : null;
    }
    if (typeof raw !== 'object') return null;

    if ('Success' in raw) return parseRaw(raw.Success, depth + 1);
    for (const k of ['Error', 'Failure', 'error']) {
        if (typeof raw[k] === 'string' && raw[k]) {
            const p = emptyParse();
            p.stderr = raw[k];
            p.failed = true;
            p.found = true;
            return p;
        }
    }
    if (Array.isArray(raw.items)) return parseRaw(raw.items, depth + 1);
    if ('Json' in raw) return parseRaw(raw.Json, depth + 1);
    if (typeof raw.Text === 'string') return raw.Text.trim() ? textParse(raw.Text) : null;
    // A bare text block ({ type: "text", text }), e.g. inside MCP-style content.
    if (raw.type === 'text' && typeof raw.text === 'string') {
        return raw.text.trim() ? textParse(unfence(raw.text)) : null;
    }
    const flat = parseOutputObject(raw);
    if (flat) return flat;
    if (Array.isArray(raw.content)) {
        // MCP-style tool result: { content: [{ type: "text", text }], isError }.
        const p = parseRaw(raw.content, depth + 1);
        if (p && raw.isError === true) p.failed = true;
        return p;
    }
    return null;
}

/**
 * Parse an ACP `rawOutput` value into `{ stdout, stderr, exit, failed,
 * terminal }` (strings unsanitised), or `null` when the shape isn't one we
 * recognise.
 */
export function parseShellRawOutput(rawOutput) {
    const p = parseRaw(rawOutput, 0);
    if (!p) return null;
    const { found: _found, ...rest } = p;
    return rest;
}

/**
 * Parse an ACP tool-call `content` array: text blocks become output (one
 * wrapping code fence is stripped), a `terminal` block flags output Kage
 * can't show. `null` when there is neither.
 */
export function parseShellContent(content) {
    if (!Array.isArray(content)) return null;
    const p = emptyParse();
    const texts = [];
    for (const block of content) {
        if (!block || typeof block !== 'object') continue;
        if (block.type === 'terminal') {
            p.terminal = true;
            p.found = true;
        } else if (block.type === 'content' && block.content?.type === 'text') {
            if (typeof block.content.text === 'string' && block.content.text.trim()) {
                texts.push(unfence(block.content.text));
            }
        } else if (block.type === 'text' && typeof block.text === 'string' && block.text.trim()) {
            texts.push(unfence(block.text));
        }
    }
    if (texts.length) {
        p.stdout = texts.join('\n');
        p.found = true;
    }
    if (!p.found) return null;
    const { found: _found, ...rest } = p;
    return rest;
}

// --- Records --------------------------------------------------------------

/**
 * Cap a parsed output to `budget` characters (stdout + stderr) and sanitise
 * it into the stored form `{ stdout, stderr, exitCode, exitSignal, failed,
 * terminal, dropped }` where stdout/stderr are capped segments or `null`.
 */
function storeOutput(parsed, budget) {
    const out = {
        stdout: null,
        stderr: null,
        exitCode: parsed.exit && 'code' in parsed.exit ? parsed.exit.code : null,
        exitSignal: parsed.exit?.signal || null,
        failed: !!parsed.failed,
        terminal: !!parsed.terminal,
        dropped: false,
    };
    const so = parsed.stdout || '';
    const se = parsed.stderr || '';
    if (!so && !se) return out;
    const b = Math.max(0, Math.floor(budget));
    if (b === 0) {
        out.dropped = !!(so.trim() || se.trim());
        return out;
    }
    let soMax = b;
    let seMax = 0;
    if (se) {
        if (so.length + se.length <= b) {
            soMax = so.length;
            seMax = se.length;
        } else {
            const half = Math.floor(b / 2);
            seMax = Math.min(se.length, so.length < half ? b - so.length : half);
            soMax = b - seMax;
        }
    }
    out.stdout = so ? detachSegment(sanitizeAndCap(so, soMax)) : null;
    out.stderr = se ? detachSegment(sanitizeAndCap(se, seMax)) : null;
    return out;
}

/**
 * A copy of `s` that doesn't reference the string it was cut from. Engines
 * implement slice() as a view into the parent string (and replace() with
 * no match returns its input), so a capped head of a 10 MB output would
 * otherwise keep all 10 MB alive for as long as the record lives.
 */
function detach(s) {
    return s.length > 64 ? JSON.parse(JSON.stringify(s)) : s;
}

function detachSegment(seg) {
    if (!seg) return seg;
    return { head: detach(seg.head), tail: detach(seg.tail), omitted: seg.omitted };
}

function hasStoredText(out) {
    return !!(out && (out.stdout || out.stderr));
}

/** Characters of output a record holds. */
export function shellOutputSize(record) {
    if (!record) return 0;
    let n = 0;
    for (const o of [record.rawOut, record.contentOut]) {
        if (o) n += cappedLength(o.stdout) + cappedLength(o.stderr);
    }
    return n;
}

/**
 * The output to show for a record: what `rawOutput` reported, else what the
 * content blocks reported. `null` when neither has arrived.
 */
export function shellOutputOf(record) {
    if (!record) return null;
    const raw = record.rawOut;
    const content = record.contentOut;
    if (raw && (raw.stdout || raw.stderr || raw.dropped)) {
        // rawOutput carries the structured exit status; content may carry
        // the terminal flag.
        return { ...raw, terminal: raw.terminal || !!content?.terminal };
    }
    if (content) {
        return {
            ...content,
            exitCode: raw?.exitCode ?? content.exitCode,
            exitSignal: raw?.exitSignal || content.exitSignal,
            failed: !!raw?.failed || content.failed,
        };
    }
    return raw || null;
}

/**
 * Merge one ACP `tool_call` / `tool_call_update` into a tool-usage `entry`
 * (from `processToolCallUpdate`). Keeps `entry.shell` — the normalised
 * shell record — up to date and returns it, or returns `null` when the
 * call isn't a shell command. ACP semantics: fields an update omits are
 * unchanged; `rawOutput` and `content` replace the previous value.
 *
 * `opts.outputBudget` caps the output this call may store (the caller
 * deducts what the response's other calls already hold).
 */
export function applyShellUpdate(entry, update, opts = {}) {
    if (!entry || !update) return null;
    let rec = entry.shell || null;
    if (!rec) {
        const kind = update.kind || entry.kind;
        if (!isShellToolCall(kind, update.title || entry.title)) return null;
        // The call may only now turn out to be a shell run (its kind came
        // late): start from the title the entry was created with. Command
        // and title are kept exactly as sent (see shellCommandDisplayParts).
        const seedCommand = extractShellCommand(null, entry.title);
        rec = {
            toolCallId: entry.toolCallId,
            title: typeof entry.title === 'string' ? entry.title : '',
            command: seedCommand,
            commandFromTitle: !!seedCommand,
            cwd: '',
            description: '',
            status: null,
            rawOut: null,
            contentOut: null,
            settled: false,
            rev: 0,
        };
        entry.shell = rec;
    }
    const budget = Math.min(SHELL_OUTPUT_CAP, Math.max(0, opts.outputBudget ?? SHELL_OUTPUT_CAP));
    let changed = rec.rev === 0;

    if (typeof update.title === 'string' && update.title && update.title !== rec.title) {
        rec.title = update.title;
        changed = true;
    }
    const fromInput = extractShellCommand(update.rawInput, null);
    const fromTitle = extractShellCommand(null, update.title);
    const cmd = fromInput || (!rec.command || rec.commandFromTitle ? fromTitle : '');
    if (cmd) {
        if (cmd !== rec.command) {
            rec.command = cmd;
            changed = true;
        }
        rec.commandFromTitle = !fromInput;
    }
    if (update.rawInput && typeof update.rawInput.description === 'string') {
        rec.description = update.rawInput.description.trim();
    }
    const cwd = extractShellCwd(update.rawInput);
    if (cwd) rec.cwd = cwd;
    if (KNOWN_STATUSES.has(update.status) && update.status !== rec.status) {
        rec.status = update.status;
        changed = true;
    }
    const terminal = rec.status === 'completed' || rec.status === 'failed';

    if ('rawOutput' in update) {
        const parsed = parseShellRawOutput(update.rawOutput);
        rec.rawOut = parsed ? storeOutput(parsed, budget) : null;
        // rawOutput's text is what shows (shellOutputOf), so content's text
        // is dead weight now — drop it rather than hold the budget twice.
        if (hasStoredText(rec.rawOut) && hasStoredText(rec.contentOut)) {
            rec.contentOut = { ...rec.contentOut, stdout: null, stderr: null };
        }
        changed = true;
    }
    if ('content' in update) {
        let parsed = parseShellContent(update.content);
        // Before the call finishes, an agent's content can be a description
        // of the command rather than its output (Claude Code's adapter sends
        // the Bash tool's `description`) — never show that as output.
        if (parsed && !terminal && rec.description && parsed.stdout.trim() === rec.description) {
            parsed = parsed.terminal ? { ...parsed, stdout: '' } : null;
        }
        if (parsed || terminal) {
            const room = Math.max(0, budget - shellOutputSize({ rawOut: rec.rawOut }));
            rec.contentOut = parsed ? storeOutput(parsed, room) : null;
            changed = true;
        }
    }
    if (changed) rec.rev += 1;
    return rec;
}

/**
 * Display state of a record: `running`, `success`, `failed`, or `unknown`
 * (the turn ended without the agent reporting how the call finished).
 */
export function shellCallState(record) {
    if (!record) return 'unknown';
    const out = shellOutputOf(record);
    const exitCode = out?.exitCode ?? null;
    if (record.status === 'failed' || out?.failed || out?.exitSignal) return 'failed';
    if (exitCode !== null && exitCode !== 0) return 'failed';
    if (record.status === 'completed') return 'success';
    if (exitCode === 0) return 'success';
    return record.settled ? 'unknown' : 'running';
}

// --- Agent sandbox denials -------------------------------------------------

// Agents run shell commands inside a sandbox of their own (kiro-cli behind
// a managed wrapper, Codex with Seatbelt / Landlock, Claude Code's sandbox
// runtime). A command it blocks can work fine in the user's own terminal,
// and Kage's tool approvals can't change that — so a failure that reads
// as the agent's sandbox refusing the command gets a notice
// (shell-command-view.js).
//
// Only text a failing command doesn't print in a terminal counts. A bare
// "Operation not permitted" / "Permission denied" never does: SIP, privacy
// (TCC) prompts, file modes and root-owned paths fail exactly the same way
// outside any sandbox. Signatures are matched at the start of a line (after
// at most a few `name: ` prefixes such as `Error: `), so most quoted copies
// — a test diff, an assertion message, a log line with a timestamp in
// front — don't count either.
//
// This is a hint, not proof: the output is just text, and a command can
// print any of these lines itself (`echo 'sandbox-exec: …' >&2; exit 1`).
// So the notice never vouches for the command — it asks the user to read
// it before running it themselves — and Kage never runs it.

// Up to four `name: ` prefixes (`Error: `, `brew: `, `sandbox error: `).
// The class has no `:`, so each prefix ends at its own colon: no
// backtracking blow-up on long lines.
const LINE_PREFIX = String.raw`^[ \t]*(?:[A-Za-z][\w .-]{0,40}: ){0,4}`;

// A command that runs bubblewrap itself (directly, or through Flatpak /
// Steam): its own bwrap can fail the same way in a terminal.
const RUNS_BWRAP = /(?:^|[\s;&|(`/])(?:bwrap|flatpak(?:-builder)?|steam)(?=$|[\s;&|)`])/;

// `strict` signatures are specific enough to count even when the call
// reported success (see shellSandboxDenial); `unless` skips a signature for
// a command it can't be told apart from.
const SANDBOX_SIGNATURES = Object.freeze([
    // macOS Seatbelt, nested: the command (Homebrew, `swift build`, …)
    // tried to apply its own sandbox-exec profile inside the agent's
    // sandbox, which macOS refuses: "sandbox-exec: sandbox_apply: Operation
    // not permitted". In a terminal the same profile applies fine. The text
    // is specific enough that it also counts after any `…: ` — Homebrew
    // wraps it as `Error: wget (1.24.5): …`, `Error: firefox@nightly: …` or
    // `Error: Download failed on Cask 'x' with message: …`.
    {
        reason: 'seatbelt',
        strict: true,
        re: new RegExp(
            `(?:${LINE_PREFIX}|: sandbox-exec: )sandbox_apply\\w*: Operation not permitted`,
            'm'
        ),
    },
    // A Seatbelt violation report: "Sandbox: curl(4242) deny(1)
    // network-outbound …". The system log prints these with a timestamp
    // first (that's what Homebrew's own build sandbox shows in verbose
    // mode), which doesn't match.
    {
        reason: 'seatbelt',
        re: /^[ \t]*Sandbox: [^\n()]{1,80}\(\d+\) deny\(\d+\) [a-z][\w*-]*/m,
    },
    // Claude Code's sandbox runtime appends what it denied to stderr as a
    // <sandbox_violations> … </sandbox_violations> block.
    {
        reason: 'sandbox-runtime',
        re: /^[ \t]*<sandbox_violations>[ \t]*\n[\s\S]*?^[ \t]*<\/sandbox_violations>/m,
    },
    // …and its network proxy tags a refused request (curl -i / -v).
    {
        reason: 'sandbox-runtime',
        re: /^[ \t]*(?:< )?X-Proxy-Error: blocked-by-sandbox-runtime\b/im,
    },
    // bubblewrap (the Linux sandbox several agents wrap commands in)
    // failing to set up its namespaces or mounts. These aren't unique to a
    // sandbox: on a system that restricts unprivileged user namespaces
    // (Ubuntu 23.10+'s AppArmor default, a kernel with them switched off)
    // bwrap fails like this in a terminal too. When the command doesn't run
    // bwrap itself, the bwrap that failed is the agent's wrapper — and the
    // command, run in a terminal, needs no bwrap at all. A command that
    // does (`flatpak run …`) is skipped. (One that runs bwrap only
    // indirectly, through a build tool, isn't caught by that.)
    {
        reason: 'bubblewrap',
        unless: RUNS_BWRAP,
        re: new RegExp(
            '^[ \\t]*bwrap: (?:setting up [ug]id map|error writing to setgroups|' +
                'Creating new namespace failed|No permissions to create (?:a )?new namespace|' +
                'unshare (?:user|pid) ns|Setting userns2 failed|Failed to make / \\w|' +
                "Can't mount proc|mounting proc|pivot_root|loopback: Failed RTM_NEWADDR)",
            'm'
        ),
    },
    // Codex's own sandbox errors (codex-rs/protocol/src/error.rs).
    {
        reason: 'codex',
        re: new RegExp(
            `${LINE_PREFIX}(?:sandbox denied exec error|` +
                'Landlock was not able to fully enforce all sandbox rules)',
            'm'
        ),
    },
]);

const SANDBOX_LINE_MAX = 200;

/**
 * Whether command output shows the agent's own sandbox refusing the
 * command: `{ reason, line }` — `reason` one of `seatbelt`,
 * `sandbox-runtime`, `bubblewrap`, `codex`; `line` the (first) line with a
 * match, trimmed and clipped — or `null`. Pure; high precision by design
 * (see above): plain permission errors never match. `opts.strict` checks
 * only the signatures that also count for a call that reported success;
 * `opts.command` is the command that printed `output` (some signatures
 * don't count for some commands). Callers decide whether the call failed
 * (`shellSandboxDenial`).
 */
export function detectSandboxDenial(output, opts = {}) {
    if (typeof output !== 'string' || !output) return null;
    const strict = !!opts?.strict;
    const command = typeof opts?.command === 'string' ? opts.command : '';
    const text =
        output.includes(ESC) || output.includes('\r') ? sanitizeTerminalText(output) : output;
    for (const sig of SANDBOX_SIGNATURES) {
        if (strict && !sig.strict) continue;
        if (sig.unless && command && sig.unless.test(command)) continue;
        const m = sig.re.exec(text);
        if (!m) continue;
        // Report the whole line the match is on.
        const start = text.lastIndexOf('\n', m.index) + 1;
        const nl = text.indexOf('\n', m.index);
        const end = Math.min(nl === -1 ? text.length : nl, start + SANDBOX_LINE_MAX * 2);
        const line = text.slice(start, safeCut(text, end)).trim();
        return { reason: sig.reason, line: line.slice(0, safeCut(line, SANDBOX_LINE_MAX)) };
    }
    return null;
}

/**
 * Whether `command` has a `|` outside quotes — a pipeline (`a | b`,
 * `a |& b`) or an or-list (`a || b`). Either can end with status 0 when
 * the command that mattered failed: a pipeline exits with its last
 * command's status unless `pipefail` is set. A rough POSIX-shell reading:
 * quotes and backslashes are honoured; `#` comments end the scan.
 */
export function hasUnquotedPipe(command) {
    if (typeof command !== 'string') return false;
    let quote = '';
    for (let i = 0; i < command.length; i++) {
        const c = command[i];
        if (quote === "'") {
            if (c === "'") quote = '';
        } else if (c === '\\') {
            i += 1;
        } else if (quote === '"') {
            if (c === '"') quote = '';
        } else if (c === "'" || c === '"') {
            quote = c;
        } else if (c === '#' && (i === 0 || /\s/.test(command[i - 1]))) {
            const nl = command.indexOf('\n', i);
            if (nl === -1) return false;
            i = nl;
        } else if (c === '|') {
            return true;
        }
    }
    return false;
}

/**
 * `detectSandboxDenial` for a record: its stored output (stderr first,
 * head and tail of each) is what's checked. A failed call is checked
 * against every signature. A call that reported success only counts when
 * its command has an unquoted `|` — `brew install x 2>&1 | tail -20` exits
 * 0 when brew was refused — and then only on the strict signatures.
 */
export function shellSandboxDenial(record) {
    const state = shellCallState(record);
    const command = typeof record?.command === 'string' ? record.command : '';
    let strict = false;
    if (state !== 'failed') {
        if (state !== 'success' || !hasUnquotedPipe(command)) return null;
        strict = true;
    }
    const out = shellOutputOf(record);
    if (!out) return null;
    const opts = { strict, command };
    for (const seg of [out.stderr, out.stdout]) {
        if (!seg) continue;
        const hit = detectSandboxDenial(seg.head, opts) || detectSandboxDenial(seg.tail, opts);
        if (hit) return hit;
    }
    return null;
}

/** Shell records of a response's tool usages, in call order. */
export function getShellCalls(toolUsages) {
    if (!Array.isArray(toolUsages)) return [];
    const out = [];
    for (const u of toolUsages) if (u?.shell) out.push(u.shell);
    return out;
}

/**
 * The turn is over: any shell call still without a terminal status never
 * will get one — stop showing it as running. Returns whether any changed.
 */
export function settleShellCalls(toolUsages) {
    let changed = false;
    for (const rec of getShellCalls(toolUsages)) {
        if (!rec.settled && rec.status !== 'completed' && rec.status !== 'failed') {
            rec.settled = true;
            rec.rev += 1;
            changed = true;
        }
    }
    return changed;
}
