/**
 * Background agent calls (docs/design/EXTENSION_AGENT_CALLS.md §3.1,
 * §3.5, §3.6): the permission table entries and the Rust command
 * signatures they map to, so the sandbox host and Settings send the
 * argument names Tauri expects.
 */

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import {
    CAPABILITIES,
    COMMAND_CAPABILITIES,
    decideInvoke,
    KNOWN_CAPABILITIES,
} from '../../ui/js/shared/extension-permissions.js';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

const EXTENSION_COMMANDS = [
    'agent_call_reserve',
    'agent_call_ready',
    'agent_call_send',
    'agent_call_cancel',
    'agent_call_status',
];
const HOST_COMMANDS = [
    'get_extension_agent_access',
    'set_extension_agent_access',
    'stop_extension_agent_calls',
    'get_extension_agent_request_detail',
];

/** `#[tauri::command]` fns in a Rust file → their non-injected arg names. */
function tauriCommandArgs(file) {
    const text = readFileSync(path.join(repoRoot, file), 'utf8');
    const out = new Map();
    const re = /#\[tauri::command\]\s*pub\s+(?:async\s+)?fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(([\s\S]*?)\)\s*->/g;
    for (const [, name, list] of text.matchAll(re)) {
        const args = list
            .split(/,(?![^<]*>)/)
            .map((part) => part.trim().match(/^(\w+)\s*:\s*(.+)$/))
            .filter((m) => m && !/State\s*<|AppHandle/.test(m[2]))
            .map((m) => m[1]);
        out.set(name, args);
    }
    return out;
}

describe('agent_background capability', () => {
    it('is known, with its install-prompt text', () => {
        expect(KNOWN_CAPABILITIES).toContain('agent_background');
        expect(CAPABILITIES.agent_background.icon).toBe('🌙');
        expect(CAPABILITIES.agent_background.label).toBe('Background AI');
        expect(CAPABILITIES.agent_background.description).toMatch(/confirm which agent/);
    });

    it('gates exactly the five agent_call commands', () => {
        const gated = Object.entries(COMMAND_CAPABILITIES)
            .filter(([, cap]) => cap === 'agent_background')
            .map(([cmd]) => cmd)
            .sort();
        expect(gated).toEqual([...EXTENSION_COMMANDS].sort());
    });

    it('keeps the panel commands host-only', () => {
        const all = new Set(KNOWN_CAPABILITIES);
        for (const cmd of HOST_COMMANDS) {
            expect(Object.hasOwn(COMMAND_CAPABILITIES, cmd), cmd).toBe(true);
            expect(COMMAND_CAPABILITIES[cmd], cmd).toBeNull();
            expect(decideInvoke(cmd, all).allow, cmd).toBe(false);
        }
    });

    it('needs the capability, not `agent`, and never runs in a view', () => {
        for (const cmd of EXTENSION_COMMANDS) {
            expect(decideInvoke(cmd, new Set(['agent_background'])).allow, cmd).toBe(true);
            expect(decideInvoke(cmd, new Set(['agent', 'storage'])).reason, cmd).toMatch(
                /missing capability 'agent_background'/
            );
            const view = decideInvoke(cmd, new Set(['agent_background']), { surface: 'view' });
            expect(view.allow, cmd).toBe(false);
        }
    });
});

describe('agent call commands — Rust ↔ JS alignment', () => {
    const cmds = tauriCommandArgs('src/commands/agent_calls.rs');

    it('defines all nine commands and nothing else', () => {
        expect([...cmds.keys()].sort()).toEqual([...EXTENSION_COMMANDS, ...HOST_COMMANDS].sort());
    });

    it('takes the extension id the sandbox host injects, and an `args` object', () => {
        for (const cmd of ['agent_call_reserve', 'agent_call_ready', 'agent_call_send', 'agent_call_cancel']) {
            expect(cmds.get(cmd), cmd).toEqual(['extension_id', 'args']);
        }
        expect(cmds.get('agent_call_status')).toEqual(['extension_id']);
    });

    it('takes the arguments Settings sends', () => {
        expect(cmds.get('get_extension_agent_access')).toEqual(['extension_id']);
        expect(cmds.get('set_extension_agent_access')).toEqual([
            'extension_id',
            'enabled',
            'consent_token',
            'allowance',
        ]);
        expect(cmds.get('stop_extension_agent_calls')).toEqual(['extension_id']);
        expect(cmds.get('get_extension_agent_request_detail')).toEqual(['extension_id', 'call_id']);
    });

    it('registers every one of them', () => {
        const registry = readFileSync(path.join(repoRoot, 'src/commands/registry.rs'), 'utf8');
        for (const cmd of cmds.keys()) {
            expect(registry, cmd).toMatch(new RegExp(`crate::commands::${cmd},`));
        }
    });
});
