/**
 * Commands the user runs themselves — the pure side (no DOM) of the
 * sandbox notice's "Run it here" (shell-run-view.js): what to warn about
 * in a command, the message that hands the result back to the agent, and
 * the queue that holds that message until the agent's turn is over.
 *
 * The run itself happens in the backend (`run_user_shell_command`,
 * src/commands/system/user_shell.rs), only after the user's own click in
 * the confirm. Nothing here starts one.
 */

import { sanitizeTerminalText } from './shell-commands.js';

/** Longest command the backend runs, in UTF-8 bytes (runner.rs MAX_COMMAND_BYTES). */
export const RUN_COMMAND_MAX_BYTES = 32 * 1024;
/** Output a run keeps: the backend's last 64 KiB; the live view keeps as many characters. */
export const RUN_OUTPUT_CAP = 64 * 1024;
/** Output handed back to the agent, in characters (its tail). */
export const AGENT_OUTPUT_CAP = 8 * 1024;
/** Characters of the command echoed back to the agent. */
export const AGENT_COMMAND_CAP = 2000;
/** The backend's time limit for a run (runner.rs RUN_TIMEOUT), in minutes. */
export const RUN_TIMEOUT_MINUTES = 10;

/** UTF-8 length of `s`. */
export function utf8Length(s) {
    if (typeof s !== 'string') return 0;
    let n = 0;
    for (let i = 0; i < s.length; i++) {
        const c = s.charCodeAt(i);
        if (c < 0x80) n += 1;
        else if (c < 0x800) n += 2;
        else if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) {
            const d = s.charCodeAt(i + 1);
            if (d >= 0xdc00 && d <= 0xdfff) {
                n += 4;
                i += 1;
            } else n += 3;
        } else n += 3;
    }
    return n;
}

// --- What to warn about ------------------------------------------------------

/**
 * Risk ids `shellCommandRisks` reports, in the order the confirm lists them.
 *   - `sudo`: runs something with sudo / doas — it needs a password, and
 *     Kage has no terminal to ask for one.
 *   - `rm_rf`: `rm` with both recursive and force.
 *   - `pipe_to_shell`: downloads a script and runs it in one go
 *     (`curl … | sh`, `bash -c "$(curl …)"`, `source <(wget …)`).
 *   - `recursive_root`: `chmod` / `chown` / `chgrp -R` on `/`, a top-level
 *     folder (`/usr`, `/Users`, …) or the home folder.
 *   - `shell_rc`: writes a shell startup file (`>> ~/.zshrc`, `tee -a
 *     ~/.bashrc`, `sed -i … ~/.profile`, `cp x ~/.zprofile`).
 */
export const SHELL_RISKS = Object.freeze([
    'sudo',
    'rm_rf',
    'pipe_to_shell',
    'recursive_root',
    'shell_rc',
]);

const ASSIGNMENT = /^[A-Za-z_][A-Za-z0-9_]*=/;
// Words that come before the command they run (`time make`, `then rm …`).
const PREFIX_WORDS = new Set([
    '!',
    '{',
    '}',
    'if',
    'then',
    'elif',
    'else',
    'while',
    'until',
    'do',
    'command',
    'builtin',
    'exec',
    'nohup',
    'time',
    'nice',
    'caffeinate',
    'timeout',
    'stdbuf',
]);
const SUDO_LIKE = new Set(['sudo', 'doas']);
// sudo / doas options that take a value (`sudo -u root rm …`).
const SUDO_VALUE_OPTS = new Set(['-u', '-g', '-h', '-p', '-C', '-D', '-R', '-T', '-U', '-r', '-t']);
const XARGS_VALUE_OPTS = new Set(['-I', '-n', '-P', '-L', '-d', '-E', '-s', '-a']);
const REDIRECTS = new Set(['>', '>>', '<', '<<']);
const SEPARATORS = new Set([';', '&', '|', '(', ')', '\n']);

/** Index of the `)` closing a `(` whose content starts at `from` (or the end). */
function closingParen(s, from) {
    let depth = 1;
    let quote = '';
    for (let i = from; i < s.length; i++) {
        const c = s[i];
        if (quote) {
            if (c === '\\' && quote === '"') i += 1;
            else if (c === quote) quote = '';
        } else if (c === '\\') {
            i += 1;
        } else if (c === "'" || c === '"') {
            quote = c;
        } else if (c === '(') {
            depth += 1;
        } else if (c === ')') {
            depth -= 1;
            if (depth === 0) return i;
        }
    }
    return s.length;
}

/** Index of the backtick closing one whose content starts at `from` (or the end). */
function closingBacktick(s, from) {
    for (let i = from; i < s.length; i++) {
        if (s[i] === '\\') i += 1;
        else if (s[i] === '`') return i;
    }
    return s.length;
}

const MAX_NESTING = 4;

/**
 * Split a POSIX-ish command line into simple commands, each a list of
 * words with quotes removed: `a "b c" | d >> f` → `[['a', 'b c'], ['d',
 * '>>', 'f']]`. Separators are `; & | ( )` and newlines; redirection
 * operators become words of their own; `#` comments are dropped. A
 * command substitution (`$(…)`, backticks, `<(…)`) stays a placeholder
 * word in its command — `chown -R $(whoami) /usr` keeps its operands —
 * and its content is split into simple commands of its own. A rough
 * reading meant for warnings: it never decides what runs.
 */
export function splitSimpleCommands(command, depth = 0) {
    const s = typeof command === 'string' ? command : '';
    const segments = [];
    const nested = [];
    let words = [];
    let word = '';
    let inWord = false;
    let quote = '';
    const endWord = () => {
        if (inWord) words.push(word);
        word = '';
        inWord = false;
    };
    const endSegment = () => {
        endWord();
        if (words.length) segments.push(words);
        words = [];
    };
    // `$(`, `<(`, `>(` at i (content from i + 2) or a backtick (from i + 1):
    // a placeholder here, the content split on its own. Returns the index
    // of the closing character.
    const substitution = (i, backtick) => {
        const from = backtick ? i + 1 : i + 2;
        const close = backtick ? closingBacktick(s, from) : closingParen(s, from);
        if (depth < MAX_NESTING)
            nested.push(...splitSimpleCommands(s.slice(from, close), depth + 1));
        word += backtick ? '`…`' : `${s[i]}(…)`;
        inWord = true;
        return close;
    };
    for (let i = 0; i < s.length; i++) {
        const c = s[i];
        if (quote === "'") {
            if (c === "'") quote = '';
            else word += c;
            continue;
        }
        if (quote === '"') {
            if (c === '"') quote = '';
            else if (c === '\\' && i + 1 < s.length && '"\\$`\n'.includes(s[i + 1])) {
                i += 1;
                if (s[i] !== '\n') word += s[i];
            } else if (c === '`') i = substitution(i, true);
            else if (c === '$' && s[i + 1] === '(') i = substitution(i, false);
            else word += c;
            continue;
        }
        if (c === '\\') {
            if (i + 1 < s.length) {
                i += 1;
                if (s[i] !== '\n') {
                    word += s[i];
                    inWord = true;
                }
            }
            continue;
        }
        if (c === "'" || c === '"') {
            quote = c;
            inWord = true;
            continue;
        }
        if (c === '#' && !inWord) {
            const nl = s.indexOf('\n', i);
            if (nl === -1) break;
            i = nl - 1;
            continue;
        }
        if (c === '`') {
            i = substitution(i, true);
            continue;
        }
        if ((c === '$' || c === '<' || c === '>') && s[i + 1] === '(') {
            if (c !== '$') endWord();
            i = substitution(i, false);
            continue;
        }
        if (SEPARATORS.has(c)) {
            endSegment();
            continue;
        }
        if (c === '>' || c === '<') {
            endWord();
            let op = c;
            if (s[i + 1] === c) {
                op += c;
                i += 1;
            } else if (c === '>' && s[i + 1] === '|') {
                i += 1;
            }
            words.push(op);
            continue;
        }
        if (c === ' ' || c === '\t' || c === '\r') {
            endWord();
            continue;
        }
        word += c;
        inWord = true;
    }
    endSegment();
    return segments.concat(nested);
}

function basename(word) {
    const i = word.lastIndexOf('/');
    return i >= 0 ? word.slice(i + 1) : word;
}

/** Skip a wrapper's own options (`nice -n 10`, `timeout 5`, `xargs -I {}`). */
function skipOptions(words, i, valueOpts, numericArg) {
    let j = i;
    while (j < words.length && words[j].startsWith('-') && words[j] !== '-') {
        const opt = words[j];
        j += 1;
        if (opt === '--') break;
        if (valueOpts?.has(opt)) j += 1;
    }
    if (numericArg && j < words.length && /^\d/.test(words[j])) j += 1;
    return j;
}

/**
 * The programs one simple command runs, outermost first: `sudo -u root rm
 * -rf x` → `[{ name: 'sudo', args: [] }, { name: 'rm', args: ['-rf',
 * 'x'] }]`. Assignments and wrappers (`env`, `time`, `nohup`, `xargs`, …)
 * are looked through.
 */
function programsOf(words) {
    const out = [];
    let i = 0;
    while (i < words.length) {
        while (i < words.length && (ASSIGNMENT.test(words[i]) || PREFIX_WORDS.has(words[i]))) {
            const w = words[i];
            i += 1;
            if (w === 'nice' || w === 'timeout' || w === 'time' || w === 'stdbuf') {
                i = skipOptions(words, i, new Set(['-n']), w === 'timeout');
            }
        }
        if (i >= words.length) break;
        const name = basename(words[i]);
        i += 1;
        if (name === 'env') {
            while (i < words.length && (words[i].startsWith('-') || ASSIGNMENT.test(words[i]))) {
                i += words[i] === '-u' ? 2 : 1;
            }
            continue;
        }
        if (SUDO_LIKE.has(name)) {
            out.push({ name, args: [] });
            i = skipOptions(words, i, SUDO_VALUE_OPTS, false);
            continue;
        }
        if (name === 'xargs') {
            i = skipOptions(words, i, XARGS_VALUE_OPTS, false);
            continue;
        }
        out.push({ name, args: words.slice(i) });
        break;
    }
    return out;
}

/** `{ short, long, operands }` of a program's arguments (redirections left out). */
function parseArgs(args) {
    const short = new Set();
    const long = new Set();
    const operands = [];
    let ended = false;
    for (let i = 0; i < args.length; i++) {
        const a = args[i];
        if (REDIRECTS.has(a)) {
            i += 1;
            continue;
        }
        if (!ended && a === '--') {
            ended = true;
        } else if (!ended && a.startsWith('--')) {
            long.add(a.split('=')[0]);
        } else if (!ended && a.length > 1 && a.startsWith('-')) {
            for (const ch of a.slice(1)) short.add(ch);
        } else {
            operands.push(a);
        }
    }
    return { short, long, operands };
}

const HOME_ROOT = /^(?:~|\$HOME|\$\{HOME\})\/?\*?$/;

/** `/`, `/*`, a top-level folder (`/usr`, `/Users/`), or the home folder itself. */
function isRootish(path) {
    if (path === '/' || path === '/*' || path === '/.') return true;
    if (/^\/[^/]+\/?$/.test(path)) return true;
    return HOME_ROOT.test(path);
}

const RC_BASENAMES = new Set([
    '.bashrc',
    '.bash_profile',
    '.bash_login',
    '.bash_logout',
    '.profile',
    '.zshrc',
    '.zprofile',
    '.zshenv',
    '.zlogin',
    '.zlogout',
    '.kshrc',
    '.mkshrc',
    '.cshrc',
    '.tcshrc',
    '.login',
]);
const RC_SYSTEM = new Set([
    '/etc/profile',
    '/etc/bashrc',
    '/etc/bash.bashrc',
    '/etc/zshrc',
    '/etc/zprofile',
    '/etc/zshenv',
    '/etc/zlogin',
    '/etc/environment',
]);

/** A shell startup file (`~/.zshrc`, `$HOME/.profile`, `/etc/zshrc`, fish's `config.fish`). */
function isRcFile(path) {
    if (typeof path !== 'string' || !path) return false;
    const p = path.replace(/^of=/, '');
    const base = basename(p);
    if (RC_BASENAMES.has(base)) return true;
    if (RC_SYSTEM.has(p)) return true;
    return base === 'config.fish' && p.includes('fish');
}

// Programs that write their last operand (`cp x ~/.zshrc`).
const COPIERS = new Set(['cp', 'mv', 'ln', 'install', 'rsync', 'ditto']);
const INTERPRETERS = '(?:(?:ba|z|da|k|fi|mk)?sh|python[0-9.]*|perl|ruby|node)';
const FETCHERS = '(?:curl|wget)';
// `curl … | sh`, also through other pipeline stages and `sudo` / `env`.
const RE_PIPE_TO_SHELL = new RegExp(
    String.raw`(?:^|[\s;&|(\x60])(?:\S*/)?${FETCHERS}\b[^\n;&]*?(?<!\|)\|(?![|&])\s*` +
        String.raw`(?:(?:sudo|doas|env|command|exec)\s+(?:-\S+\s+|[A-Za-z_]\w*=\S*\s+)*)*` +
        String.raw`(?:\S*/)?${INTERPRETERS}(?![\w.-])`
);
// `bash -c "$(curl …)"`, `sh <(wget …)`, `source <(curl …)`, `eval "$(curl …)"`.
const RE_SUBST_TO_SHELL = new RegExp(
    String.raw`(?:^|[\s;&|(\x60])(?:(?:\S*/)?${INTERPRETERS}|eval|source|\.)(?=[\s"'])` +
        String.raw`[^\n;|&]*?(?:\$\(|<\(|\x60)\s*(?:\S*/)?${FETCHERS}\b`
);

/**
 * What the confirm should warn about in `command`: a subset of
 * `SHELL_RISKS`, in that order. Pure and deliberately generous — a false
 * alarm costs a line of text, a miss costs more — but it's a reading of
 * the text, not a guarantee: the user still reads the command.
 */
export function shellCommandRisks(command) {
    if (typeof command !== 'string' || !command.trim()) return [];
    const found = new Set();
    collectRisks(command, found, 0);
    return SHELL_RISKS.filter((r) => found.has(r));
}

const POSIX_SHELL_NAME = /^(?:ba|z|da|k|mk)?sh$/;

/** The script of `sh -c '…'` / `bash -lc '…'`, if `args` run one. */
function shellScriptArg(args) {
    for (let i = 0; i < args.length - 1; i++) {
        const a = args[i];
        if (a === '--') return null;
        if (/^-[a-zA-Z]*c[a-zA-Z]*$/.test(a)) return args[i + 1];
        if (!a.startsWith('-')) return null;
    }
    return null;
}

function collectRisks(command, found, depth) {
    if (RE_PIPE_TO_SHELL.test(command) || RE_SUBST_TO_SHELL.test(command)) {
        found.add('pipe_to_shell');
    }
    for (const words of splitSimpleCommands(command)) {
        for (let k = 0; k < words.length - 1; k++) {
            if ((words[k] === '>' || words[k] === '>>') && isRcFile(words[k + 1])) {
                found.add('shell_rc');
            }
        }
        for (const { name, args } of programsOf(words)) {
            if (SUDO_LIKE.has(name)) {
                found.add('sudo');
                continue;
            }
            const { short, long, operands } = parseArgs(args);
            if (POSIX_SHELL_NAME.test(name)) {
                // `bash -c 'rm -rf …'`: what the script runs counts too.
                const script = shellScriptArg(args);
                if (script && depth < MAX_NESTING) collectRisks(script, found, depth + 1);
            } else if (name === 'rm') {
                const recursive = short.has('r') || short.has('R') || long.has('--recursive');
                const force = short.has('f') || long.has('--force');
                if (recursive && force) found.add('rm_rf');
            } else if (name === 'chmod' || name === 'chown' || name === 'chgrp') {
                const recursive = short.has('R') || long.has('--recursive');
                if (recursive && operands.some(isRootish)) found.add('recursive_root');
            } else if (name === 'tee') {
                if (operands.some(isRcFile)) found.add('shell_rc');
            } else if (name === 'sed' || name === 'perl') {
                const inPlace =
                    short.has('i') || long.has('--in-place') || args.some((a) => /^-i/.test(a));
                if (inPlace && operands.some(isRcFile)) found.add('shell_rc');
            } else if (COPIERS.has(name) || name === 'dd') {
                const target =
                    name === 'dd' ? args.find((a) => a.startsWith('of=')) : operands.at(-1);
                if (isRcFile(target)) found.add('shell_rc');
            }
        }
    }
}

/**
 * On Windows, the shell the agent's tool used, when the record says:
 * kiro-cli's `execute_cmd` tool runs `cmd /C` (a reopened session's tool
 * use carries that name as its title). `null` means the backend's default
 * (PowerShell); other platforms ignore it.
 */
export function agentShellHint(record) {
    return record?.title === 'execute_cmd' ? 'cmd' : null;
}

// --- The message back to the agent -------------------------------------------

/** A backtick fence longer than any backtick run in `text`. */
export function fenceFor(text) {
    let longest = 0;
    for (const m of String(text).matchAll(/`+/g)) longest = Math.max(longest, m[0].length);
    return '`'.repeat(Math.max(3, longest + 1));
}

function fenced(text, lang = '') {
    const fence = fenceFor(text);
    return `${fence}${lang}\n${text.endsWith('\n') ? text : `${text}\n`}${fence}`;
}

/** The last `max` characters of `text`, starting on a line when one starts nearby. */
export function tailOf(text, max) {
    const s = typeof text === 'string' ? text : '';
    if (s.length <= max) return { text: s, cut: false };
    let start = s.length - max;
    const c = s.charCodeAt(start);
    if (c >= 0xdc00 && c <= 0xdfff) start += 1;
    const nl = s.indexOf('\n', start);
    if (nl !== -1 && nl - start < 200 && nl + 1 < s.length) start = nl + 1;
    return { text: s.slice(start), cut: true };
}

function formatCount(n) {
    return Number(n).toLocaleString('en-US');
}

function statusPhrase(result) {
    if (result?.outcome === 'timed_out') {
        return `timed out after ${RUN_TIMEOUT_MINUTES} minutes and was stopped`;
    }
    if (result?.outcome === 'cancelled') return 'I stopped it before it finished';
    if (Number.isInteger(result?.exitCode)) return `exit code ${result.exitCode}`;
    if (Number.isInteger(result?.signal)) return `ended by signal ${result.signal}`;
    return 'exit status unknown';
}

/**
 * The prompt that tells the agent how a command the user ran outside its
 * sandbox went, shown in the chat as the user's message:
 *
 *     I ran this outside your sandbox: `brew install awscli` — exit code 0
 *     Working directory: `/Users/me`
 *     Output (last 8,192 characters):
 *     ```
 *     …
 *     ```
 *
 * A multi-line, long or backticked command goes in a fenced block of its
 * own (clipped at `AGENT_COMMAND_CAP`). Output is stripped of escape
 * sequences and cut to its last `AGENT_OUTPUT_CAP` characters. English on
 * purpose, like the other agent-facing text Kage writes into prompts.
 *
 * @param {{ command: string, cwd?: string, result: object }} run
 *   `result` as `run_user_shell_command` returns it: `{ outcome,
 *   exitCode, signal, output, truncated, outputBytes }`.
 */
export function formatRunResultMessage({ command, cwd, result }) {
    const cmd = typeof command === 'string' ? command : '';
    const status = statusPhrase(result);
    const parts = [];
    const inline = cmd.length <= 200 && !/[`\n\r]/.test(cmd);
    if (inline) {
        parts.push(`I ran this outside your sandbox: \`${cmd}\` — ${status}`);
    } else {
        parts.push(`I ran this outside your sandbox — ${status}:`);
        const shown = cmd.length > AGENT_COMMAND_CAP ? cmd.slice(0, AGENT_COMMAND_CAP) : cmd;
        parts.push(fenced(shown, 'sh'));
        if (shown.length < cmd.length) {
            parts.push(
                `(command shortened here: ${formatCount(cmd.length - shown.length)} more characters)`
            );
        }
    }
    if (cwd && !/[`\n\r]/.test(cwd)) parts.push(`Working directory: \`${cwd}\``);

    const clean = sanitizeTerminalText(result?.output || '').replace(/\s+$/, '');
    if (!clean) {
        parts.push('It printed no output.');
    } else {
        const tail = tailOf(clean, AGENT_OUTPUT_CAP);
        const cut = tail.cut || !!result?.truncated;
        parts.push(cut ? `Output (last ${formatCount(tail.text.length)} characters):` : 'Output:');
        parts.push(fenced(tail.text));
    }
    return parts.join('\n');
}

// --- Holding the message until the agent is free ------------------------------

/**
 * A queue of messages for the agent, each sent as the user's next prompt
 * once `canSend(sessionId)` says that session is in view and idle — at
 * once if it already is, else when a turn in progress ends (checked every
 * `pollMs`, and on `flush()`). One send at a time, oldest first.
 *
 * `submit({ sessionId, text, onStatus })` returns an id; `onStatus(state,
 * error?)` hears `queued` (waiting), then `sent`, `failed` or `cancelled`.
 *
 * @param {{ canSend: (sessionId: string) => boolean,
 *           send: (item: { sessionId: string, text: string }) => Promise<void>,
 *           pollMs?: number }} opts
 */
export function createAgentResultQueue({ canSend, send, pollMs = 500 }) {
    const items = [];
    let timer = null;
    let sending = false;
    let seq = 0;

    const sendable = (item) => {
        try {
            return !!canSend(item.sessionId);
        } catch {
            return false;
        }
    };
    const schedule = () => {
        if (timer !== null || !items.length) return;
        timer = setTimeout(() => {
            timer = null;
            flush();
        }, pollMs);
    };
    async function flush() {
        if (sending) return;
        const idx = items.findIndex(sendable);
        if (idx < 0) {
            schedule();
            return;
        }
        const [item] = items.splice(idx, 1);
        sending = true;
        try {
            await send({ sessionId: item.sessionId, text: item.text });
            item.onStatus?.('sent');
        } catch (e) {
            item.onStatus?.('failed', e);
        } finally {
            sending = false;
        }
        schedule();
    }

    return {
        submit({ sessionId, text, onStatus }) {
            seq += 1;
            const item = { id: seq, sessionId, text, onStatus };
            items.push(item);
            if (sending || !sendable(item)) onStatus?.('queued');
            flush();
            return item.id;
        },
        cancel(id) {
            const i = items.findIndex((it) => it.id === id);
            if (i < 0) return false;
            const [item] = items.splice(i, 1);
            item.onStatus?.('cancelled');
            return true;
        },
        flush,
        get size() {
            return items.length;
        },
        dispose() {
            clearTimeout(timer);
            timer = null;
            items.length = 0;
        },
    };
}
