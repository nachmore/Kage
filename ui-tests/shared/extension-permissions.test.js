import { describe, it, expect, vi } from 'vitest';
import {
    COMMAND_CAPABILITIES,
    CAPABILITIES,
    KNOWN_CAPABILITIES,
    normalizePermissions,
    grantCovers,
    decideInvoke,
    VIEW_SURFACE_DENIED_COMMANDS,
} from '../../ui/js/shared/extension-permissions.js';
import {
    HOST_FEATURE_SYNC_KEYS,
    HOST_FEATURE_SYNC_LISTS,
    HOST_FEATURE_SYNC_RECORDS,
    hostFeaturesFor,
} from '../../ui/js/shared/extension-sandbox-host.js';

const EXT_VIEW_COMMANDS = [
    'ext_view_open',
    'ext_view_boot',
    'ext_view_compose',
    'ext_view_copy_text',
    'ext_view_focus_state',
    'ext_view_list',
    'ext_view_reset_geometry',
];

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
            'get_config', // leaks the entire app config; extensions use get_extension_config
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
            // Launcher frecency is a shared global file, not identity-scoped —
            // exposing it would leak usage patterns and let one extension
            // clobber the launcher ranking for everyone.
            'save_frecency',
            'load_frecency',
            // The frecency HMAC key: with it, hashed ids could be dictionary-checked.
            'get_frecency_key',
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
        const result = normalizePermissions([' Storage ', 'storage', 'CLIPBOARD', 'urls'], 'x');
        expect(result).toEqual(['storage', 'clipboard', 'urls']);
    });

    it('drops unknown capabilities with a warning', () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const result = normalizePermissions(['storage', 'not-a-real-cap'], 'my-ext');
        expect(result).toEqual(['storage']);
        expect(warn).toHaveBeenCalledWith(expect.stringContaining('not-a-real-cap'));
        warn.mockRestore();
    });

    it('drops the now-removed legacy "shell" alias', () => {
        // Pre-launch we ripped out the back-compat path: 'shell' is no
        // longer recognised. A manifest declaring it gets the unknown-cap
        // warning treatment, same as any typo. Tests guard against the
        // alias accidentally returning.
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const result = normalizePermissions(['storage', 'shell'], 'old-ext');
        expect(result).toEqual(['storage']);
        expect(warn).toHaveBeenCalledWith(expect.stringContaining('shell'));
        warn.mockRestore();
    });

    it('ignores non-string entries', () => {
        const result = normalizePermissions([null, 42, 'storage', {}], 'x');
        expect(result).toEqual(['storage']);
    });
});

describe('grantCovers', () => {
    it('is true only when the recorded grant holds every requested capability', () => {
        expect(grantCovers({ granted: ['network', 'storage'] }, ['storage', 'network'], 'x')).toBe(
            true
        );
        expect(grantCovers({ granted: ['storage'] }, ['storage', 'network'], 'x')).toBe(false);
        expect(grantCovers(undefined, ['storage'], 'x')).toBe(false);
        expect(grantCovers(null, ['storage'], 'x')).toBe(false);
    });

    it('asking for nothing needs no grant', () => {
        expect(grantCovers(undefined, [], 'x')).toBe(true);
        expect(grantCovers(undefined, undefined, 'x')).toBe(true);
    });

    it('compares the normalised sets', () => {
        expect(grantCovers({ granted: [' Storage '] }, ['STORAGE'], 'x')).toBe(true);
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

describe('views capability', () => {
    it('is a known capability with its install-prompt text', () => {
        expect(KNOWN_CAPABILITIES).toContain('views');
        expect(CAPABILITIES.views.icon).toBe('🖼️');
        expect(CAPABILITIES.views.label).toBe('Its own windows');
        expect(CAPABILITIES.views.description).toMatch(/What you type in those windows/);
        expect(normalizePermissions(['Views'], 'x')).toEqual(['views']);
    });

    it('maps no command to views: it gates host behaviour only', () => {
        expect(Object.values(COMMAND_CAPABILITIES)).not.toContain('views');
    });

    it('every view window command is host-only (null)', () => {
        for (const cmd of EXT_VIEW_COMMANDS) {
            expect(Object.hasOwn(COMMAND_CAPABILITIES, cmd), cmd).toBe(true);
            expect(COMMAND_CAPABILITIES[cmd], cmd).toBeNull();
            const d = decideInvoke(cmd, new Set(KNOWN_CAPABILITIES), { surface: 'view' });
            expect(d.allow, cmd).toBe(false);
        }
    });
});

describe('decideInvoke view-surface denylist', () => {
    const all = new Set(KNOWN_CAPABILITIES);

    it('lists the window-bound commands and the background agent calls', () => {
        expect([...VIEW_SURFACE_DENIED_COMMANDS].sort()).toEqual(
            [
                'send_message_streaming',
                'cancel_generation',
                'send_steering_message',
                'extension_tool_response',
                'get_window_session',
                'resize_floating_window',
                'set_floating_opacity',
                'apply_chat_window_size',
                'save_window_position',
                'save_chat_window_geometry',
                'notify_frontend_ready',
                'agent_call_reserve',
                'agent_call_ready',
                'agent_call_send',
                'agent_call_cancel',
                'agent_call_status',
            ].sort()
        );
        expect(Object.isFrozen(VIEW_SURFACE_DENIED_COMMANDS)).toBe(true);
    });

    it('refuses every listed command on the view surface, whatever is held', () => {
        for (const cmd of VIEW_SURFACE_DENIED_COMMANDS) {
            const d = decideInvoke(cmd, all, { surface: 'view' });
            expect(d.allow, cmd).toBe(false);
            expect(d.reason, cmd).toMatch(/isn't available in an extension window/);
        }
    });

    it('leaves the hidden surface unchanged', () => {
        for (const cmd of VIEW_SURFACE_DENIED_COMMANDS) {
            const expected = decideInvoke(cmd, all);
            expect(decideInvoke(cmd, all, { surface: 'hidden' }), cmd).toEqual(expected);
            expect(decideInvoke(cmd, all, {}), cmd).toEqual(expected);
        }
        expect(decideInvoke('send_message_streaming', new Set(['agent'])).allow).toBe(true);
        expect(decideInvoke('resize_floating_window', new Set(['window'])).allow).toBe(true);
    });

    it('applies the normal rules to everything else in a view', () => {
        const view = { surface: 'view' };
        expect(decideInvoke('open_chat_with_message', new Set(['agent']), view).allow).toBe(true);
        expect(decideInvoke('read_clipboard', new Set(['clipboard']), view).allow).toBe(true);
        expect(decideInvoke('read_clipboard', new Set(['storage']), view).reason).toMatch(
            /missing capability 'clipboard'/
        );
        expect(decideInvoke('quit_app', all, view).reason).toMatch(/never callable/);
        expect(decideInvoke('no_such_command', all, view).reason).toMatch(/not available/);
        expect(decideInvoke(42, all, view).allow).toBe(false);
    });
});

describe('folder sync (contracts only)', () => {
    const all = new Set(KNOWN_CAPABILITIES);
    const syncCommands = Object.keys(COMMAND_CAPABILITIES).filter(
        (c) => c.startsWith('sync_') || c.startsWith('ext_sync_') || c === 'get_config_snapshot'
    );

    it('registers every sync command as host-only until the sync capability lands', () => {
        expect(syncCommands.length).toBe(38);
        for (const cmd of syncCommands) {
            expect(COMMAND_CAPABILITIES[cmd], cmd).toBeNull();
            expect(decideInvoke(cmd, all).allow, cmd).toBe(false);
        }
        expect(KNOWN_CAPABILITIES).not.toContain('sync');
    });

    it('names the sync host features but never offers them yet', () => {
        const offered = [HOST_FEATURE_SYNC_KEYS, HOST_FEATURE_SYNC_LISTS, HOST_FEATURE_SYNC_RECORDS];
        expect(offered).toEqual(['sync.keys', 'sync.lists', 'sync.records']);
        expect(hostFeaturesFor(offered, ['storage', 'sync'])).toEqual([]);
    });
});
