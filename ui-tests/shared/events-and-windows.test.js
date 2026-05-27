/**
 * Verify the JS event-name and window-label constants in
 * `ui/js/shared/events.js` and `ui/js/shared/window-labels.js`
 * agree with the Rust constants in `src/events.rs` and
 * `src/window_labels.rs`.
 *
 * If the two sides drift, events go to listeners that aren't there —
 * silently. The Rust side has its own snake_case test
 * (`events::tests::all_event_names_are_snake_case`); this file pairs
 * with that to catch cross-language mismatches.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { EVT } from '../../ui/js/shared/events.js';
import {
    WINDOW,
    CHAT_PREFIX,
    chatLabel,
    isChatLabel,
    isSessionHostLabel,
} from '../../ui/js/shared/window-labels.js';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

/// Pull every `pub const FOO: &str = "bar";` line out of a Rust source
/// file. Returns a Map<name, value>. Good enough for this check — the
/// constants modules are deliberately simple.
function parseRustStringConsts(rustPath) {
    const text = readFileSync(rustPath, 'utf8');
    const map = new Map();
    const re = /pub const (\w+): &str = "([^"]+)";/g;
    let m;
    while ((m = re.exec(text)) !== null) {
        map.set(m[1], m[2]);
    }
    return map;
}

describe('events.js / window-labels.js — Rust alignment', () => {
    it('every JS EVT.* constant has a matching Rust constant with the same value', () => {
        const rustEvents = parseRustStringConsts(path.join(repoRoot, 'src/events.rs'));
        const issues = [];
        for (const [jsName, jsValue] of Object.entries(EVT)) {
            // CONTEXT_MENU_ACTION is JS-only (no Rust caller); permit it
            // as long as it's still snake_case.
            if (jsName === 'CONTEXT_MENU_ACTION') continue;
            const rustValue = rustEvents.get(jsName);
            if (rustValue === undefined) {
                issues.push(
                    `JS exports EVT.${jsName} but src/events.rs has no const ${jsName}`
                );
                continue;
            }
            if (rustValue !== jsValue) {
                issues.push(
                    `EVT.${jsName} = ${JSON.stringify(jsValue)} but Rust ${jsName} = ${JSON.stringify(rustValue)}`
                );
            }
        }
        expect(issues, issues.join('\n')).toEqual([]);
    });

    it('every JS WINDOW.* constant has a matching Rust constant with the same value', () => {
        const rustLabels = parseRustStringConsts(path.join(repoRoot, 'src/window_labels.rs'));
        const issues = [];
        for (const [jsName, jsValue] of Object.entries(WINDOW)) {
            const rustValue = rustLabels.get(jsName);
            if (rustValue === undefined) {
                issues.push(
                    `JS exports WINDOW.${jsName} but src/window_labels.rs has no const ${jsName}`
                );
                continue;
            }
            if (rustValue !== jsValue) {
                issues.push(
                    `WINDOW.${jsName} = ${JSON.stringify(jsValue)} but Rust ${jsName} = ${JSON.stringify(rustValue)}`
                );
            }
        }
        expect(issues, issues.join('\n')).toEqual([]);
    });

    it('CHAT_PREFIX matches the Rust constant', () => {
        const rustLabels = parseRustStringConsts(path.join(repoRoot, 'src/window_labels.rs'));
        expect(rustLabels.get('CHAT_PREFIX')).toBe(CHAT_PREFIX);
    });

    it('every JS event name is snake_case (matches the Rust convention)', () => {
        for (const [name, value] of Object.entries(EVT)) {
            expect(value, `${name} contains a hyphen`).not.toMatch(/-/);
            expect(value, `${name} has uppercase or non-[a-z_] chars`).toMatch(/^[a-z][a-z0-9_]*$/);
        }
    });
});

describe('window-labels.js — helpers', () => {
    it('chatLabel prefixes the uuid', () => {
        expect(chatLabel('abc')).toBe('chat-abc');
    });

    it('isChatLabel matches the prefix and rejects others', () => {
        expect(isChatLabel(chatLabel('11111111-2222-3333-4444-555555555555'))).toBe(true);
        for (const label of Object.values(WINDOW)) {
            expect(isChatLabel(label)).toBe(false);
        }
    });

    it('isSessionHostLabel = MAIN || isChatLabel', () => {
        expect(isSessionHostLabel(WINDOW.MAIN)).toBe(true);
        expect(isSessionHostLabel(chatLabel('xyz'))).toBe(true);
        // Floating, settings, inline-assist, etc. don't qualify
        expect(isSessionHostLabel(WINDOW.FLOATING)).toBe(false);
        expect(isSessionHostLabel(WINDOW.SETTINGS)).toBe(false);
        expect(isSessionHostLabel(WINDOW.INLINE_ASSIST)).toBe(false);
    });

    it('isChatLabel is defensive against non-string input', () => {
        // Important because chat/app.js reads from appWindow?.label which
        // can be undefined during the very early bootstrap.
        expect(isChatLabel(null)).toBe(false);
        expect(isChatLabel(undefined)).toBe(false);
        expect(isChatLabel(123)).toBe(false);
    });
});
