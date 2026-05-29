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

import { describe, it, expect, beforeAll, beforeEach, vi } from 'vitest';
import {
    bindEditForm,
    connectionFromDetected,
    describeIssue,
    listPresets,
    readEditForm,
    pickAgentType,
    renderDetected,
    renderEditForm,
    uuidLite,
    validateMode,
} from '../../ui/js/shared/agent-connections.js';
import { initI18n } from '../../ui/js/shared/i18n.js';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));

// Load the canonical EN catalog so t() inside agent-connections.js resolves
// to real strings instead of returning the literal key. Without this every
// localised assertion would have to compare against the key path.
beforeAll(async () => {
    const catalogPath = join(__dirname, '..', '..', 'locales', 'en', 'messages.json');
    const catalog = JSON.parse(readFileSync(catalogPath, 'utf-8'));
    await initI18n(async (cmd) => {
        if (cmd === 'get_i18n_catalog') {
            return { language: 'en', rtl: false, catalog, fallback: catalog };
        }
        throw new Error(`unexpected invoke: ${cmd}`);
    });
});

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

// --- renderDetected --------------------------------------------------------
// Two-button row: "Use this agent" (lock in + caller advances the wizard)
// and an optional pencil ✏️ (open the manual editor pre-populated). Pre-fix
// "Use this agent" silently did both — populating the form AND showing the
// editor — which surprised users in the welcome flow.

function stubInvoke(detectAgentsResult) {
    window.__TAURI__ = {
        core: {
            invoke: (name) => {
                if (name === 'detect_agents') {
                    return Promise.resolve(detectAgentsResult);
                }
                return Promise.resolve(undefined);
            },
        },
    };
}

const SAMPLE_AGENT = Object.freeze({
    name: 'Kiro',
    preset_id: 'kiro',
    path: '/usr/local/bin/kiro',
    spawn_command: '/usr/local/bin/kiro acp',
    version: '0.5.1',
});

describe('renderDetected', () => {
    it('renders one card per detected agent with the Use button', async () => {
        stubInvoke([SAMPLE_AGENT, { ...SAMPLE_AGENT, name: 'Claude Code' }]);
        const container = document.createElement('div');
        document.body.appendChild(container);

        const agents = await renderDetected(container, { onSelect: () => {} });
        expect(agents.length).toBe(2);
        const useButtons = container.querySelectorAll('.agent-use-btn');
        expect(useButtons.length).toBe(2);
        expect(container.textContent).toContain('Kiro');
        expect(container.textContent).toContain('Claude Code');
    });

    it('omits the pencil button when onEdit is not supplied', async () => {
        // The pencil is opt-in. The settings page may want detect cards
        // without an "edit before continuing" flow, so the helper must
        // not assume both callbacks are always wanted.
        stubInvoke([SAMPLE_AGENT]);
        const container = document.createElement('div');
        document.body.appendChild(container);

        await renderDetected(container, { onSelect: () => {} });
        expect(container.querySelector('.agent-edit-btn')).toBeNull();
    });

    it('renders the pencil button per card when onEdit is supplied', async () => {
        stubInvoke([SAMPLE_AGENT, { ...SAMPLE_AGENT, name: 'Codex' }]);
        const container = document.createElement('div');
        document.body.appendChild(container);

        await renderDetected(container, { onSelect: () => {}, onEdit: () => {} });
        expect(container.querySelectorAll('.agent-edit-btn').length).toBe(2);
    });

    it('"Use this agent" calls onSelect (NOT onEdit) and passes the agent through', async () => {
        // The bug: pre-fix the welcome flow's onSelect ALSO opened the
        // manual editor and scrolled to it. The shared helper now
        // separates the two paths; the caller is expected to lock in
        // the selection and advance the wizard.
        stubInvoke([SAMPLE_AGENT]);
        const container = document.createElement('div');
        document.body.appendChild(container);
        const onSelect = vi.fn();
        const onEdit = vi.fn();

        await renderDetected(container, { onSelect, onEdit });
        container.querySelector('.agent-use-btn').click();

        expect(onSelect).toHaveBeenCalledTimes(1);
        expect(onSelect).toHaveBeenCalledWith(SAMPLE_AGENT);
        expect(onEdit).not.toHaveBeenCalled();
    });

    it('the pencil button calls onEdit (NOT onSelect)', async () => {
        stubInvoke([SAMPLE_AGENT]);
        const container = document.createElement('div');
        document.body.appendChild(container);
        const onSelect = vi.fn();
        const onEdit = vi.fn();

        await renderDetected(container, { onSelect, onEdit });
        container.querySelector('.agent-edit-btn').click();

        expect(onEdit).toHaveBeenCalledTimes(1);
        expect(onEdit).toHaveBeenCalledWith(SAMPLE_AGENT);
        expect(onSelect).not.toHaveBeenCalled();
    });

    it('clicking the second card invokes the callback with that agent (not the first)', async () => {
        // Walking the agents list by data-idx — a regression here would
        // pin every "Use this agent" click to the first card.
        const second = { ...SAMPLE_AGENT, name: 'Second', spawn_command: '/bin/second' };
        stubInvoke([SAMPLE_AGENT, second]);
        const container = document.createElement('div');
        document.body.appendChild(container);
        const onSelect = vi.fn();

        await renderDetected(container, { onSelect });
        const buttons = container.querySelectorAll('.agent-use-btn');
        buttons[1].click();

        expect(onSelect).toHaveBeenCalledWith(second);
    });

    it('falls back to a friendly empty-state when no agents are detected', async () => {
        stubInvoke([]);
        const container = document.createElement('div');
        document.body.appendChild(container);

        const agents = await renderDetected(container, { onSelect: () => {} });
        expect(agents).toEqual([]);
        expect(container.querySelector('.agent-not-found')).not.toBeNull();
        expect(container.querySelector('.agent-use-btn')).toBeNull();
    });

    // Wrapper-needed entries are how we surface "Claude is installed but
    // doesn't speak ACP — install the npm wrapper". The bare-claude
    // detection hint emits these. The card has different chrome (no
    // "Use this agent" button, an "Install ACP wrapper" button, an
    // install-status slot) so a regression that re-renders these as
    // ready-to-use entries would silently let users save a useless
    // connection.
    const WRAPPER_NEEDED_AGENT = Object.freeze({
        name: 'Claude Code',
        preset_id: 'claude-code',
        path: 'C:\\Users\\me\\AppData\\Local\\Programs\\claude.exe',
        spawn_command: 'claude-code-acp',
        version: null,
        needs_wrapper_npm_package: '@zed-industries/claude-code-acp',
    });

    it('renders Install ACP wrapper button (not Use this agent) for wrapper-needed entries', async () => {
        stubInvoke([WRAPPER_NEEDED_AGENT]);
        const container = document.createElement('div');
        document.body.appendChild(container);

        await renderDetected(container, { onSelect: () => {}, onEdit: () => {} });

        expect(container.querySelector('.agent-install-wrapper-btn')).not.toBeNull();
        expect(container.querySelector('.agent-use-btn')).toBeNull();
        expect(container.querySelector('.agent-edit-btn')).toBeNull();
        expect(container.textContent).toContain('@zed-industries/claude-code-acp');
    });

    it('install button falls back to a manual command when npm is missing', async () => {
        // No npm → we don't try `install_acp_wrapper`; we tell the user
        // to install Node.js and surface the exact command.
        const invoke = vi.fn().mockImplementation((name) => {
            if (name === 'detect_agents') return Promise.resolve([WRAPPER_NEEDED_AGENT]);
            if (name === 'check_npm_available') return Promise.resolve({ available: false });
            return Promise.resolve(undefined);
        });
        window.__TAURI__ = { core: { invoke } };

        const container = document.createElement('div');
        document.body.appendChild(container);
        await renderDetected(container, {});

        const btn = container.querySelector('.agent-install-wrapper-btn');
        btn.click();
        // Allow the async chain inside the click handler to settle.
        await new Promise((r) => setTimeout(r, 0));
        await new Promise((r) => setTimeout(r, 0));

        expect(invoke).toHaveBeenCalledWith('check_npm_available');
        expect(invoke).not.toHaveBeenCalledWith('install_acp_wrapper', expect.anything());
        const status = container.querySelector('.agent-install-status');
        expect(status.textContent).toContain('npm');
        expect(status.textContent).toContain('install -g @zed-industries/claude-code-acp');
    });
});
