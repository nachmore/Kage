/**
 * Launcher result menus: which results get one, and how the actions the
 * backend reports become grouped, OS-worded menu items.
 */

import { describe, it, expect } from 'vitest';
import {
    buildResultMenuItems,
    resultActionLabelKey,
    resultMenuTarget,
    unavailableLabelKey,
} from '../../ui/js/shared/result-menu.js';
import { normalizeMenuItems } from '../../ui/js/shared/popup-menu.js';

const shape = (items) => items.map((i) => (i.separator ? '-' : i.id));

describe('resultMenuTarget', () => {
    it('maps app, file and path results', () => {
        expect(resultMenuTarget({ type: 'app', label: 'Slack', data: { name: 'Slack' } })).toEqual({
            kind: 'app',
            target: 'Slack',
        });
        expect(resultMenuTarget({ type: 'file', data: { path: '/Users/me/a.pdf' } })).toEqual({
            kind: 'path',
            target: '/Users/me/a.pdf',
        });
        expect(resultMenuTarget({ type: 'path', data: { value: 'C:\\Users' } })).toEqual({
            kind: 'path',
            target: 'C:\\Users',
        });
    });

    it('ignores other kinds, empty targets and extension rows', () => {
        expect(resultMenuTarget({ type: 'url', data: { value: 'https://x' } })).toBeNull();
        expect(resultMenuTarget({ type: 'shortcut', label: 'x' })).toBeNull();
        expect(resultMenuTarget({ type: 'file', data: { path: '  ' } })).toBeNull();
        expect(resultMenuTarget({ type: 'app', data: {} })).toBeNull();
        expect(
            resultMenuTarget({ type: 'file', _extensionId: 'evil', data: { path: '/etc/hosts' } })
        ).toBeNull();
        expect(resultMenuTarget(null)).toBeNull();
    });
});

describe('resultActionLabelKey', () => {
    it('uses each OS file manager wording', () => {
        expect(resultActionLabelKey('reveal', 'macos')).toBe('result_menu.reveal_finder');
        expect(resultActionLabelKey('reveal', 'windows')).toBe('result_menu.reveal_explorer');
        expect(resultActionLabelKey('reveal', 'linux')).toBe('result_menu.reveal_folder');
        expect(resultActionLabelKey('trash', 'windows')).toBe('result_menu.move_to_recycle_bin');
        expect(resultActionLabelKey('trash', 'macos')).toBe('result_menu.move_to_trash');
        expect(resultActionLabelKey('copy_path', 'windows')).toBe('result_menu.copy_as_path');
        expect(resultActionLabelKey('copy_path', 'linux')).toBe('result_menu.copy_path');
        expect(resultActionLabelKey('uninstall', 'macos')).toBe('result_menu.uninstall');
    });
});

describe('buildResultMenuItems', () => {
    it('groups a Windows app menu like the Start menu', () => {
        const items = buildResultMenuItems({
            platform: 'windows',
            kind: 'app',
            name: 'Git Bash',
            actions: ['open', 'run_as_admin', 'reveal', 'copy_path', 'uninstall', 'properties'],
        });
        expect(shape(items)).toEqual([
            'open',
            'run_as_admin',
            '-',
            'reveal',
            '-',
            'copy_path',
            '-',
            'uninstall',
            '-',
            'properties',
        ]);
        expect(items.find((i) => i.id === 'uninstall').danger).toBe(true);
        expect(items.find((i) => i.id === 'open').shortcut).toBe('Enter');
        expect(items.find((i) => i.id === 'reveal').labelKey).toBe('result_menu.reveal_explorer');
    });

    it('builds a macOS file menu without cut', () => {
        const items = buildResultMenuItems({
            platform: 'macos',
            kind: 'file',
            name: 'a.pdf',
            actions: ['open', 'reveal', 'copy_file', 'copy_path', 'trash'],
        });
        expect(shape(items)).toEqual([
            'open',
            '-',
            'reveal',
            '-',
            'copy_file',
            'copy_path',
            '-',
            'trash',
        ]);
        expect(items[0].shortcut).toBe('↩');
        expect(items.at(-1)).toMatchObject({ danger: true, labelKey: 'result_menu.move_to_trash' });
        expect(items[0].labelVars).toEqual({ name: 'a.pdf' });
    });

    it('orders Windows file actions like Explorer', () => {
        const items = buildResultMenuItems({
            platform: 'windows',
            name: 'a.txt',
            actions: ['properties', 'trash', 'copy_path', 'copy_file', 'cut_file', 'reveal', 'open'],
        });
        expect(shape(items)).toEqual([
            'open',
            '-',
            'reveal',
            '-',
            'cut_file',
            'copy_file',
            'copy_path',
            '-',
            'trash',
            '-',
            'properties',
        ]);
    });

    it('lists an unavailable action disabled, with why, in its usual place', () => {
        const items = buildResultMenuItems({
            platform: 'windows',
            name: 'deep.txt',
            actions: ['open', 'reveal', 'cut_file', 'copy_file', 'copy_path', 'properties'],
            unavailable: [{ action: 'trash', reason: 'path_too_long' }],
        });
        expect(shape(items)).toEqual([
            'open',
            '-',
            'reveal',
            '-',
            'cut_file',
            'copy_file',
            'copy_path',
            '-',
            'trash',
            '-',
            'properties',
        ]);
        const trash = items.find((i) => i.id === 'trash');
        expect(trash).toMatchObject({
            enabled: false,
            danger: false,
            labelKey: 'result_menu.unavailable.trash_path_too_long',
        });
        expect(unavailableLabelKey('trash', 'path_too_long')).toBe(trash.labelKey);
        expect(normalizeMenuItems(items)).toEqual(items);
    });

    it('ignores unknown reasons and an action that is also available', () => {
        const items = buildResultMenuItems({
            platform: 'windows',
            name: 'a.txt',
            actions: ['open', 'trash'],
            unavailable: [
                { action: 'trash', reason: 'path_too_long' },
                { action: 'copy_path', reason: 'because' },
                null,
            ],
        });
        expect(shape(items)).toEqual(['open', '-', 'trash']);
        expect(items.at(-1).enabled).toBe(true);
    });

    it('ignores unknown actions and survives empty input', () => {
        expect(buildResultMenuItems({ platform: 'linux', actions: ['rm_rf'] })).toEqual([]);
        expect(buildResultMenuItems(null)).toEqual([]);
    });

    it('produces items the menu protocol accepts unchanged', () => {
        const items = buildResultMenuItems({
            platform: 'linux',
            name: 'x',
            actions: ['open', 'reveal', 'copy_path', 'trash'],
        });
        expect(normalizeMenuItems(items)).toEqual(items);
    });
});
