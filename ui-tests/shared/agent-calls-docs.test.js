/**
 * Background agent calls: the docs match the code
 * (docs/design/EXTENSION_AGENT_CALLS.md §13, WP-AC7).
 *
 * The guide for extension authors (docs/EXTENSIONS.md § Background agent
 * calls) lists every error key with its kind, every limit and every
 * command. This test reads them from the Rust sources and the EN catalog,
 * so a changed limit, a new key or a renamed command fails here until the
 * docs follow.
 */

import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { CAPABILITIES, COMMAND_CAPABILITIES } from '../../ui/js/shared/extension-permissions.js';
import { KNOWN_EVENTS } from '../../ui/js/shared/telemetry.js';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const read = (file) => readFileSync(path.join(repoRoot, file), 'utf8');

/** The text of one `## ` section of a Markdown file, up to the next one. */
function section(markdown, heading) {
    const start = markdown.indexOf(`\n## ${heading}\n`);
    if (start < 0) throw new Error(`no "## ${heading}" section`);
    const end = markdown.indexOf('\n## ', start + 1);
    return markdown.slice(start, end < 0 ? undefined : end);
}

/** The text of one `### ` subsection, up to the next heading of any level. */
function subsection(text, heading) {
    const start = text.indexOf(`\n### ${heading}\n`);
    if (start < 0) throw new Error(`no "### ${heading}" subsection`);
    const rest = text.slice(start + 1);
    const end = rest.search(/\n#{2,3} /);
    return end < 0 ? rest : rest.slice(0, end);
}

/** The cells of the table row whose first cell is `label`. */
function row(text, label) {
    const line = text.split('\n').find((l) => l.startsWith(`| ${label} |`));
    if (!line) throw new Error(`no row "${label}"`);
    return line
        .split('|')
        .slice(1, -1)
        .map((c) => c.trim());
}

const n = (value) => value.toLocaleString('en-US');

const ENGINE_FILES = [
    'src/agent_calls.rs',
    ...readdirSync(path.join(repoRoot, 'src/agent_calls'))
        .filter((f) => f.endsWith('.rs') && f !== 'tests.rs')
        .map((f) => `src/agent_calls/${f}`),
    'src/commands/agent_calls.rs',
];
const engine = ENGINE_FILES.map(read).join('\n');

/** `pub const NAME: T = <number or a * b>;` in a Rust file. */
function rustConsts(file) {
    const out = {};
    const re = /pub const (\w+): \w+ = ([\d_]+(?:\s*\*\s*[\d_]+)?);/g;
    for (const [, name, expr] of read(file).matchAll(re)) {
        out[name] = expr
            .split('*')
            .map((part) => Number(part.trim().replaceAll('_', '')))
            .reduce((a, b) => a * b, 1);
    }
    return out;
}

/** `ALLOWANCE_<LEVEL>` → `{ calls, chars }` from limits.rs. */
function allowances() {
    const out = {};
    const re =
        /pub const ALLOWANCE_(\w+): Allowance = Allowance \{\s*calls: ([\d_]+),\s*chars: ([\d_]+),\s*\};/g;
    for (const [, level, calls, chars] of read('src/agent_calls/limits.rs').matchAll(re)) {
        out[level] = {
            calls: Number(calls.replaceAll('_', '')),
            chars: Number(chars.replaceAll('_', '')),
        };
    }
    return out;
}

/** `ErrorKind::NotPermitted` → `not_permitted`. */
const snake = (variant) => variant.replace(/(?<!^)([A-Z])/g, '_$1').toLowerCase();

const extensionsMd = read('docs/EXTENSIONS.md');
const guide = section(extensionsMd, 'Background agent calls');

describe('the error keys', () => {
    const catalog = JSON.parse(read('locales/en/messages.json'));
    const prefix = 'errors.agent_call.';
    const catalogKeys = Object.keys(catalog)
        .filter((k) => k.startsWith(prefix))
        .map((k) => k.slice(prefix.length))
        .sort();

    /** Every `ErrorKind::X, "name"` pair the engine raises. */
    const rustKinds = new Map();
    for (const [, kind, name] of engine.matchAll(
        /ErrorKind::(\w+),\s*"(?:errors\.agent_call\.)?([a-z_]+)"/g
    )) {
        if (!rustKinds.has(name)) rustKinds.set(name, new Set());
        rustKinds.get(name).add(snake(kind));
    }

    const documented = new Map();
    for (const line of subsection(guide, 'Errors').split('\n')) {
        const m = line.match(/^\| `([a-z_]+)` \| `([a-z_]+)` \|/);
        if (m) documented.set(m[1], m[2]);
    }

    it('lists every catalog key, and only those', () => {
        expect([...documented.keys()].sort()).toEqual(catalogKeys);
    });

    it('lists only keys the engine raises', () => {
        expect([...rustKinds.keys()].sort()).toEqual(catalogKeys);
    });

    it('gives each key the kind Rust raises it with', () => {
        for (const [name, kinds] of rustKinds) {
            expect([...kinds], name).toEqual([documented.get(name)]);
        }
    });
});

describe('the limits', () => {
    const c = rustConsts('src/agent_calls/limits.rs');
    const req = rustConsts('src/agent_calls/request.rs');
    const redact = rustConsts('src/agent_calls/redact.rs');
    const a = allowances();
    const limits = subsection(guide, 'Limits');
    const value = (label) => row(limits, label)[1];

    it('reads the constants it checks', () => {
        expect(Object.keys(a).sort()).toEqual(['HIGH', 'LOW', 'NORMAL']);
        expect(c.MAX_SCHEMA_BYTES).toBe(8 * 1024);
    });

    it('match src/agent_calls/limits.rs', () => {
        expect(value('Waiting calls')).toContain(
            `${c.MAX_WAITING_PER_EXTENSION} for your extension`
        );
        expect(value('Time to hand the request over once the turn starts')).toContain(
            `${c.SEND_WINDOW_SECS} seconds`
        );
        expect(value('Pause after a call ends')).toContain(
            `${c.EXTENSION_COOLDOWN_SECS} seconds before your next call, ${c.GLOBAL_COOLDOWN_SECS} seconds before anyone's`
        );
        expect(value('Calls per hour')).toMatch(new RegExp(`^${c.CALLS_PER_HOUR} `));
        expect(value('Daily allowance, Low / Normal / High')).toContain(
            `${a.LOW.calls} / ${a.NORMAL.calls} / ${a.HIGH.calls} calls, and ` +
                `${n(a.LOW.chars)} / ${n(a.NORMAL.chars)} / ${n(a.HIGH.chars)} characters`
        );
        expect(value('All extensions together')).toBe(`${c.GLOBAL_DAILY_CALLS} calls a day`);
        expect(value('`instructions`')).toBe(`${n(c.MAX_INSTRUCTIONS_CHARS)} characters`);
        expect(value('`input`')).toBe(`${n(c.MAX_INPUT_CHARS)} characters`);
        expect(value('`schema`')).toBe(
            `${c.MAX_SCHEMA_BYTES / 1024} KB, ${c.MAX_SCHEMA_DEPTH} levels, ${c.MAX_SCHEMA_NODES} schema objects`
        );
        expect(value('Framed prompt')).toContain(`${n(c.MAX_PROMPT_CHARS)} characters`);
        expect(value('`label`')).toBe(`${c.MAX_LABEL_CHARS} characters`);
        expect(value('`dedupeKey`')).toContain(`1–${c.MAX_DEDUPE_KEY_CHARS} characters`);
        expect(value('Reply')).toBe(
            `${n(c.DEFAULT_RESPONSE_CHARS)} characters by default, at most ${n(c.MAX_RESPONSE_CHARS)}`
        );
        expect(value('Timeout')).toBe(
            `${c.DEFAULT_TIMEOUT_MS / 1000} seconds by default, ` +
                `${c.MIN_TIMEOUT_MS / 1000}–${c.MAX_TIMEOUT_MS / 1000} seconds`
        );
        expect(value('Failures in a row')).toMatch(
            new RegExp(`^${c.FAILURE_BREAKER_THRESHOLD} .* for ${c.FAILURE_PAUSE_MINUTES} minutes`)
        );
        expect(value('Tool attempts')).toMatch(new RegExp(`^${c.TOOL_BREAKER_THRESHOLD} in a day`));
        expect(c.STOP_PAUSE_MINUTES).toBe(60);
        expect(value('**Stop now**')).toContain('for 1 hour');
    });

    it('match the request fields', () => {
        const request = subsection(guide, '`context.agent`');
        const rule = (field) => row(request, `\`${field}\``)[2];
        expect(rule('instructions')).toContain(`At most ${n(c.MAX_INSTRUCTIONS_CHARS)} characters`);
        expect(rule('input')).toContain(`At most ${n(c.MAX_INPUT_CHARS)} characters`);
        expect(rule('schema')).toContain(`At most ${c.MAX_SCHEMA_BYTES / 1024} KB`);
        expect(rule('label')).toContain(`At most ${c.MAX_LABEL_CHARS} characters`);
        expect(rule('maxResponseChars')).toContain(
            `Default ${n(c.DEFAULT_RESPONSE_CHARS)}, clamped to 1–${n(c.MAX_RESPONSE_CHARS)}`
        );
        expect(rule('timeoutMs')).toContain(
            `Default ${n(c.DEFAULT_TIMEOUT_MS)}, clamped to ${n(c.MIN_TIMEOUT_MS)}–${n(c.MAX_TIMEOUT_MS)}`
        );
        expect(request).toContain(`You have ${c.SEND_WINDOW_SECS} seconds`);
    });

    it('match the manifest normaliser and the credential check', () => {
        const manifest = subsection(guide, 'Declaring it');
        expect(row(manifest, '`reason`')[2]).toContain(
            `at most ${req.MAX_REASON_CHARS} characters`
        );
        expect(row(manifest, '`purposes`')[2]).toContain(
            `1–${req.MAX_PURPOSES} ids matching \`^[a-z][a-z0-9_]{0,23}$\``
        );
        expect(read('src/agent_calls/request.rs')).toContain('p.len() <= 24');
        expect(row(manifest, '`maxPromptChars`')[2]).toContain(
            `${n(c.MAX_PROMPT_CHARS)}-character`
        );
        expect(row(manifest, '`dailyCalls`')[2]).toContain(`(${a.NORMAL.calls} calls)`);
        expect(guide).toContain(`a string of ${redact.MIN_SECRET_CHARS} or more characters`);
    });

    it('match the idle rule', () => {
        expect(subsection(guide, 'When calls run')).toMatch(
            new RegExp(`hasn't used Kage for\\s+${c.IDLE_AFTER_SECS} seconds`)
        );
    });
});

describe('the commands, capability, feature and effect', () => {
    const commands = [
        ...read('src/commands/agent_calls.rs').matchAll(
            /#\[tauri::command\]\s*pub\s+async\s+fn\s+(\w+)/g
        ),
    ].map((m) => m[1]);
    const forExtensions = commands.filter(
        (cmd) => COMMAND_CAPABILITIES[cmd] === 'agent_background'
    );
    const hostOnly = commands.filter((cmd) => COMMAND_CAPABILITIES[cmd] === null);

    it('splits the nine commands five and four', () => {
        expect(commands).toHaveLength(9);
        expect(forExtensions).toHaveLength(5);
        expect(hostOnly).toHaveLength(4);
    });

    it('names the five in the capability row, with its icon', () => {
        const cap = row(subsection(extensionsMd, 'Capability reference'), '`agent_background`');
        expect(cap[1]).toBe(CAPABILITIES.agent_background.icon);
        for (const cmd of forExtensions) expect(cap[2], cmd).toContain(`\`${cmd}\``);
    });

    it('names the four host-only ones in the guide and the security model', () => {
        const security = section(read('docs/SECURITY_MODEL.md'), 'Background agent calls');
        for (const cmd of hostOnly) {
            expect(guide, cmd).toContain(`\`${cmd}\``);
            expect(security, cmd).toContain(`\`${cmd}\``);
        }
    });

    it('lists the manifest field, the host feature and the settings effect', () => {
        expect(row(subsection(extensionsMd, 'Manifest Fields'), '`agentBackground`')).toBeTruthy();
        expect(
            row(subsection(extensionsMd, 'Feature detection'), '`agent.background`')
        ).toBeTruthy();
        expect(
            subsection(extensionsMd, 'Host side effects')
                .split('\n')
                .some((l) => /^\| `agent_access` +\|/.test(l))
        ).toBe(true);
    });
});

describe('telemetry', () => {
    const events = [
        'extension_agent_access_changed',
        'extension_agent_calls_daily',
        'extension_agent_calls_paused',
    ];

    it('lists, sends and discloses each event', () => {
        const privacy = read('docs/PRIVACY.md');
        for (const event of events) {
            expect(KNOWN_EVENTS, event).toContain(event);
            expect(engine, event).toContain(`"${event}"`);
            expect(privacy, event).toContain(`\`${event}\``);
        }
    });

    it('sends no other event', () => {
        const sent = new Set([...engine.matchAll(/\.telemetry\(\s*"([a-z_]+)"/g)].map((m) => m[1]));
        expect([...sent].sort()).toEqual(events);
    });
});

describe('the other docs', () => {
    it('cover background agent calls', () => {
        const security = read('docs/SECURITY_MODEL.md');
        expect(security).toMatch(/`views` ·\n`agent_background`/);
        expect(section(security, 'Summary of current guarantees')).toContain(
            'A background agent call appears in a chat or session list'
        );
        expect(section(security, 'Known gaps being worked on')).toContain(
            'Agents that run tools without asking the client'
        );
        expect(section(read('docs/TOOL_PERMISSIONS.md'), 'Background agent calls')).toContain(
            'reject_once'
        );
        expect(read('docs/DEBUG_MODE.md')).toContain('### Background agent calls are included');
        expect(read('docs/PRIVACY.md')).toContain('### Background AI for extensions');
    });
});
