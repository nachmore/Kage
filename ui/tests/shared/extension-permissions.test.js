import { describe, it, expect, vi } from 'vitest';
import {
    COMMAND_CAPABILITIES,
    CAPABILITIES,
    KNOWN_CAPABILITIES,
    normalizePermissions,
    decideInvoke,
} from '../../js/shared/extension-permissions.js';

describe('COMMAND_CAPABILITIES table', () => {
    it('maps every value to either a known capability or null', () => {
        const known = new Set(KNOWN_CAPABILITIES);
        for (const [cmd, cap] of Object.entries(COMMAND_CAPABILITIES)) {
            if (cap === null) continue;
            expect(known.has(cap), `${cmd} → ${cap} not in KNOWN_CAPABILITIES`).toBe(true);
        }
    });

    it('explicitly blocks dangerous commands', () => {
        const mustBeBlocked = [
            'save_config',
            'quit_app',
            'restart_app',
            'execute_system_command',
            'install_extension_from_path',
            'uninstall_extension',
            'remove_tool_permission',
            'update_tool_policy',
            'send_permission_response',
            'read_extension_file',
            'open_devtools',
        ];
        for (const cmd of mustBeBlocked) {
            expect(COMMAND_CAPABILITIES[cmd]).toBeNull();
        }
    });
});

describe('CAPABILITIES metadata', () => {
    it('has icon, label, description for every known capability', () => {
        for (const cap of KNOWN_CAPABILITIES) {
            const meta = CAPABILITIES[cap];
            expect(meta, `missing metadata for ${cap}`).toBeDefined();
            expect(typeof meta.icon).toBe('string');
            expect(typeof meta.label).toBe('string');
            expect(typeof meta.description).toBe('string');
            expect(meta.description.length).toBeGreaterThan(10);
        }
    });

    it('KNOWN_CAPABILITIES matches keys of CAPABILITIES', () => {
        expect(KNOWN_CAPABILITIES.slice().sort())
            .toEqual(Object.keys(CAPABILITIES).slice().sort());
    });
});

describe('normalizePermissions', () => {
    it('returns empty array for non-array input', () => {
        expect(normalizePermissions(null, 'x')).toEqual([]);
        expect(normalizePermissions(undefined, 'x')).toEqual([]);
        expect(normalizePermissions('storage', 'x')).toEqual([]);
        expect(normalizePermissions(42, 'x')).toEqual([]);
    });

    it('trims, lowercases, and dedupes', () => {
        const result = normalizePermissions([' Storage ', 'storage', 'CLIPBOARD', 'shell'], 'x');
        expect(result).toEqual(['storage', 'clipboard', 'shell']);
    });

    it('drops unknown capabilities with a warning', () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const result = normalizePermissions(['storage', 'not-a-real-cap'], 'my-ext');
        expect(result).toEqual(['storage']);
        expect(warn).toHaveBeenCalledWith(expect.stringContaining('not-a-real-cap'));
        warn.mockRestore();
    });

    it('ignores non-string entries', () => {
        const result = normalizePermissions([null, 42, 'storage', {}], 'x');
        expect(result).toEqual(['storage']);
    });
});

describe('decideInvoke', () => {
    it('allows commands whose capability is held', () => {
        const held = new Set(['clipboard']);
        const d = decideInvoke('read_clipboard', held);
        expect(d.allow).toBe(true);
    });

    it('denies commands requiring a capability the extension does not hold', () => {
        const held = new Set(['storage']);
        const d = decideInvoke('read_clipboard', held);
        expect(d.allow).toBe(false);
        expect(d.reason).toMatch(/missing capability 'clipboard'/);
    });

    it('denies commands explicitly marked null even with all capabilities', () => {
        const held = new Set(KNOWN_CAPABILITIES);
        for (const forbidden of ['quit_app', 'save_config', 'execute_system_command']) {
            const d = decideInvoke(forbidden, held);
            expect(d.allow, forbidden).toBe(false);
            expect(d.reason, forbidden).toMatch(/never callable from an extension/);
        }
    });

    it('denies unknown commands (fail closed)', () => {
        const held = new Set(['storage']);
        const d = decideInvoke('no_such_command', held);
        expect(d.allow).toBe(false);
        expect(d.reason).toMatch(/not available to extensions/);
    });

    it('denies non-string command names', () => {
        const held = new Set(['storage']);
        expect(decideInvoke(42, held).allow).toBe(false);
        expect(decideInvoke(null, held).allow).toBe(false);
        expect(decideInvoke(undefined, held).allow).toBe(false);
    });
});
