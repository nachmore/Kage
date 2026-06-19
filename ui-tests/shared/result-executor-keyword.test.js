/**
 * Tests for keyword completion hints in the shared result executor.
 *
 * A `type: 'ext_keyword'` row is a typeahead hint, not a real action:
 * selecting it must fill the input with the keyword (via onReplaceInput) and
 * report a `replace_input` verdict so the caller re-runs search instead of
 * trying to execute it as an extension result.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

// result-executor.js pulls in commands.js / shortcuts.js / search-engine.js,
// which carry Tauri dependencies. Stub them — the ext_keyword path doesn't
// touch any of them, but the imports must resolve at module load.
vi.mock('../../ui/js/shared/commands.js', () => ({ executeCommand: vi.fn() }));
vi.mock('../../ui/js/shared/shortcuts.js', () => ({ buildShortcutCommand: vi.fn() }));
vi.mock('../../ui/js/shared/kage-log.js', () => ({ kageLog: { info: vi.fn(), warn: vi.fn() } }));
vi.mock('../../ui/js/shared/search-engine.js', () => ({ recordSelection: vi.fn() }));

let executeResult;

beforeEach(async () => {
    vi.resetModules();
    vi.doMock('../../ui/js/shared/commands.js', () => ({ executeCommand: vi.fn() }));
    vi.doMock('../../ui/js/shared/shortcuts.js', () => ({ buildShortcutCommand: vi.fn() }));
    vi.doMock('../../ui/js/shared/kage-log.js', () => ({
        kageLog: { info: vi.fn(), warn: vi.fn() },
    }));
    vi.doMock('../../ui/js/shared/search-engine.js', () => ({ recordSelection: vi.fn() }));
    ({ executeResult } = await import('../../ui/js/shared/result-executor.js'));
});

const hint = (fill, extra = {}) => ({
    id: 'ext-keyword:calendar:cal-refresh',
    type: 'ext_keyword',
    label: 'Refresh calendar',
    data: { extensionId: 'calendar', keyword: 'cal-refresh', fill, ...extra },
});

describe('executeResult — ext_keyword hints', () => {
    it('fills the input with the keyword and returns replace_input', async () => {
        const onReplaceInput = vi.fn();
        const out = await executeResult(hint('cal-refresh'), 'cal-ref', {
            invoke: vi.fn(),
            onReplaceInput,
        });
        expect(onReplaceInput).toHaveBeenCalledWith('cal-refresh');
        expect(out).toEqual({ handled: true, action: 'replace_input' });
    });

    it('preserves the trailing space for arg-taking keywords', async () => {
        const onReplaceInput = vi.fn();
        await executeResult(hint('calendar ', { keyword: 'calendar' }), 'cal', {
            invoke: vi.fn(),
            onReplaceInput,
        });
        expect(onReplaceInput).toHaveBeenCalledWith('calendar ');
    });

    it('falls back to data.keyword when no fill is set', async () => {
        const onReplaceInput = vi.fn();
        const row = hint(undefined);
        delete row.data.fill;
        await executeResult(row, 'cal-ref', { invoke: vi.fn(), onReplaceInput });
        expect(onReplaceInput).toHaveBeenCalledWith('cal-refresh');
    });

    it('is handled (not a no-op send) even without an onReplaceInput callback', async () => {
        const out = await executeResult(hint('cal-refresh'), 'cal-ref', { invoke: vi.fn() });
        expect(out).toEqual({ handled: true });
    });

    it('never delegates a hint to the extension manager', async () => {
        const executeResultSpy = vi.fn();
        await executeResult(hint('cal-refresh'), 'cal-ref', {
            invoke: vi.fn(),
            onReplaceInput: vi.fn(),
            extensionManager: { executeResult: executeResultSpy },
        });
        expect(executeResultSpy).not.toHaveBeenCalled();
    });
});
