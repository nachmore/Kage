/**
 * Tests for ui/js/shared/agent-connections.js — the form rendering and
 * detect/validate helpers shared between the welcome wizard and the
 * Agent Connection settings page.
 *
 * Coverage focuses on the pieces that are easy to break and hard to
 * notice:
 *   - uuidLite returns the expected `c-...-...` shape (the
 *     manager keys connections by id, so a regression here corrupts
 *     persisted state)
 *   - connectionFromDetected lifts every needed field off the agent
 *   - describeIssue stays in sync with the codes the backend returns
 *   - renderEditForm honours the `style: 'settings' | 'wizard'` switch
 *     and the `includeSessionsDirectory: false` opt-out
 *   - readEditForm round-trips the values rendered by renderEditForm
 *   - validateMode + listPresets degrade gracefully when invoke is
 *     missing (the wizard runs without Tauri ready in unit tests)
 *
 * escapeHtml / escapeAttr coverage lives in tool-utils.test.js — the
 * canonical home after consolidation.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
    bindEditForm,
    connectionFromDetected,
    describeIssue,
    listPresets,
    readEditForm,
    pickAgentType,
    renderEditForm,
    uuidLite,
    validateMode,
} from '../../ui/js/shared/agent-connections.js';

beforeEach(() => {
    document.body.innerHTML = '';
    delete window.__TAURI__;
});

describe('uuidLite', () => {
    it('produces a c-XXXX-YYYY shape', () => {
        const id = uuidLite();
        expect(id).toMatch(/^c-[a-z0-9]+-[a-z0-9]+$/);
    });

    it('produces unique ids on rapid calls', () => {
        const ids = new Set();
        for (let i = 0; i < 50; i++) ids.add(uuidLite());
        // 50 calls in quick succession — the timestamp suffix alone
        // wouldn't be enough; the random prefix is what guarantees
        // uniqueness here.
        expect(ids.size).toBe(50);
    });
});

describe('connectionFromDetected', () => {
    it('builds a local connection record from a detected agent', () => {
        const conn = connectionFromDetected({
            name: 'Kiro CLI',
            spawn_command: 'C:\\path\\to\\kiro.exe acp',
            preset_id: 'kiro',
            path: 'C:\\path\\to\\kiro.exe',
        });
        expect(conn.name).toBe('Kiro CLI');
        expect(conn.preset_id).toBe('kiro');
        expect(conn.mode).toEqual({
            type: 'local',
            spawn_command: 'C:\\path\\to\\kiro.exe acp',
        });
        expect(conn.sessions_directory).toBeNull();
        expect(conn.id).toMatch(/^c-/);
    });

    it('falls back to "Detected agent" when name is missing', () => {
        const conn = connectionFromDetected({ spawn_command: 'foo' });
        expect(conn.name).toBe('Detected agent');
    });
});

describe('describeIssue', () => {
    // Every code returned by validate_agent_connection must have a
    // friendly string. If we add a new code on the backend without
    // updating this map, the user sees the raw enum.
    it('translates known issue codes into human-readable copy', () => {
        expect(describeIssue('empty')).toContain('empty');
        expect(describeIssue('binary-not-found')).toContain('not found');
        expect(describeIssue('host-empty')).toContain('Host');
        expect(describeIssue('port-invalid')).toContain('Port');
        expect(describeIssue('validation-failed')).toContain('validate');
    });

    it('returns the raw code for unknown issues so we still see something', () => {
        expect(describeIssue('something-new')).toBe('something-new');
    });
});

describe('renderEditForm', () => {
    it('renders the local-mode block visible by default', () => {
        const html = renderEditForm({
            id: 'x',
            name: 'My agent',
            preset_id: null,
            mode: { type: 'local', spawn_command: 'foo' },
            sessions_directory: null,
        });
        expect(html).toContain('id="connEditMode"');
        expect(html).toContain('id="connEditLocalSettings"');
        // Local visible, remote hidden.
        expect(html).toMatch(/connEditLocalSettings"\s+>/);
        expect(html).toMatch(/connEditRemoteSettings"\s+style="display:none;"/);
    });

    it('renders remote-mode visible when mode.type === "remote"', () => {
        const html = renderEditForm({
            id: 'x',
            name: 'Remote',
            mode: { type: 'remote', host: '10.0.0.1', port: 9999, timeout_ms: 1000 },
        });
        expect(html).toMatch(/connEditLocalSettings"\s+style="display:none;"/);
        expect(html).toContain('value="10.0.0.1"');
        expect(html).toContain('value="9999"');
    });

    it('honours the wizard layout class names', () => {
        // The wizard uses `.form-group` / `.section-label`; the
        // settings page uses `.setting-row` / `.setting-label`. The
        // form must match its surrounding chrome.
        const wiz = renderEditForm({ mode: { type: 'local' } }, { style: 'wizard' });
        expect(wiz).toContain('class="form-group"');
        expect(wiz).toContain('class="section-label"');
        expect(wiz).not.toContain('class="setting-row"');

        const set = renderEditForm({ mode: { type: 'local' } }, { style: 'settings' });
        expect(set).toContain('class="setting-row"');
        expect(set).toContain('class="setting-label"');
    });

    it('omits the sessions-directory field when includeSessionsDirectory is false', () => {
        // The welcome wizard (where the user has no per-session
        // workflow yet) hides this row to keep step 1 short.
        const html = renderEditForm({}, { includeSessionsDirectory: false });
        expect(html).not.toContain('SessionsDir');
    });
});

describe('readEditForm + bindEditForm', () => {
    function mountForm(connection, opts) {
        const root = document.createElement('div');
        root.innerHTML = renderEditForm(connection, opts);
        document.body.appendChild(root);
    }

    it('reads back the values rendered by renderEditForm (local)', () => {
        mountForm({
            id: 'foo',
            name: 'My agent',
            mode: { type: 'local', spawn_command: 'bar' },
            sessions_directory: null,
        });
        const out = readEditForm('connEdit', { id: 'foo' });
        expect(out.id).toBe('foo');
        expect(out.name).toBe('My agent');
        expect(out.mode).toEqual({ type: 'local', spawn_command: 'bar' });
    });

    it('reads back the remote values when mode is remote', () => {
        mountForm({
            id: 'r',
            name: 'Remote',
            mode: { type: 'remote', host: '127.0.0.1', port: 8765, timeout_ms: 30000 },
        });
        const out = readEditForm('connEdit', { id: 'r' });
        expect(out.mode).toEqual({
            type: 'remote',
            host: '127.0.0.1',
            port: 8765,
            timeout_ms: 30000,
        });
    });

    it('returns null if the form was never rendered', () => {
        expect(readEditForm('connEdit')).toBeNull();
    });

    it('bindEditForm switches local/remote visibility on mode change', () => {
        mountForm({ mode: { type: 'local' } });
        bindEditForm('connEdit');
        const sel = document.getElementById('connEditMode');
        const local = document.getElementById('connEditLocalSettings');
        const remote = document.getElementById('connEditRemoteSettings');
        sel.value = 'remote';
        sel.dispatchEvent(new Event('change'));
        expect(local.style.display).toBe('none');
        expect(remote.style.display).toBe('');
    });
});

describe('validateMode + listPresets', () => {
    // These wrap Tauri commands but must keep working in tests where
    // window.__TAURI__ isn't set yet (the wizard's first paint can
    // race the Tauri-ready signal).

    it('validateMode returns a "no issues" verdict when invoke is missing', async () => {
        const result = await validateMode({ type: 'local', spawn_command: 'foo' });
        expect(result.ok).toBe(true);
        expect(result.issues).toEqual([]);
    });

    it('validateMode forwards the mode to validate_agent_connection', async () => {
        const invoke = vi.fn().mockResolvedValue({ ok: true, issues: [], resolved_path: 'X' });
        window.__TAURI__ = { core: { invoke } };
        const result = await validateMode({ type: 'remote', host: 'x', port: 1 });
        expect(invoke).toHaveBeenCalledWith('validate_agent_connection', {
            mode: { type: 'remote', host: 'x', port: 1 },
        });
        expect(result.resolved_path).toBe('X');
    });

    it('validateMode reports validation-failed when the backend rejects the call', async () => {
        const invoke = vi.fn().mockRejectedValue(new Error('boom'));
        window.__TAURI__ = { core: { invoke } };
        const result = await validateMode({ type: 'local' });
        expect(result.ok).toBe(false);
        expect(result.issues).toContain('validation-failed');
    });

    it('listPresets returns [] when invoke is unavailable', async () => {
        await expect(listPresets()).resolves.toEqual([]);
    });

    it('listPresets returns the backend list verbatim', async () => {
        const invoke = vi.fn().mockResolvedValue([{ id: 'kiro', display_name: 'Kiro' }]);
        window.__TAURI__ = { core: { invoke } };
        await expect(listPresets()).resolves.toEqual([{ id: 'kiro', display_name: 'Kiro' }]);
    });
});

describe('pickAgentType', () => {
    it('renders an overlay with the four expected options', () => {
        // Don't await — just verify the DOM after the synchronous mount.
        pickAgentType();
        const cards = document.querySelectorAll('.agent-type-card');
        const kinds = [...cards].map((c) => c.getAttribute('data-kind'));
        expect(kinds).toEqual(['detect', 'ollama', 'acp_preset', 'custom']);
        // Cleanup so other tests in the file start fresh.
        document.querySelector('.agent-type-cancel').click();
    });

    it('resolves with the clicked card kind', async () => {
        const promise = pickAgentType();
        document.querySelector('.agent-type-card[data-kind="ollama"]').click();
        await expect(promise).resolves.toBe('ollama');
        // Overlay removed on resolve.
        expect(document.querySelector('.agent-type-picker-overlay')).toBeNull();
    });

    it('resolves null when Cancel is clicked', async () => {
        const promise = pickAgentType();
        document.querySelector('.agent-type-cancel').click();
        await expect(promise).resolves.toBeNull();
    });

    it('resolves null when Escape is pressed', async () => {
        const promise = pickAgentType();
        document.dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
        );
        await expect(promise).resolves.toBeNull();
    });

    it('resolves null when the backdrop is clicked', async () => {
        const promise = pickAgentType();
        const overlay = document.querySelector('.agent-type-picker-overlay');
        // Dispatch a click whose target is the overlay itself, not a child.
        overlay.dispatchEvent(new MouseEvent('click', { bubbles: true }));
        await expect(promise).resolves.toBeNull();
    });
});
