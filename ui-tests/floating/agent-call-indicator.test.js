/**
 * The launcher's Background AI chip (docs/design/EXTENSION_AGENT_CALLS.md
 * §4.4, §12 "floating chip"): it shows while a call runs, with "+n
 * waiting", hides when nothing runs, names the extension as text only,
 * and its Stop and Settings buttons call the host commands for the running
 * extension.
 */

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import {
    AgentCallIndicator,
    chipState,
    installAgentCallIndicator,
} from '../../ui/js/floating/agent-call-indicator.js';
import * as i18n from '../../ui/js/shared/i18n.js';
import { initEnglish, repoRoot } from '../helpers/sync-i18n.js';

const RUNNING = {
    running: [
        {
            extension_id: 'mindmap',
            name: 'Mind Map',
            purpose: 'organize',
            label: 'Organize 12 notes',
            started_at: '2026-11-09T10:00:00+01:00',
        },
    ],
    waiting: 2,
};

let root;
let invoke;

beforeAll(async () => {
    await initEnglish(i18n);
});
beforeEach(() => {
    document.body.innerHTML = '<div class="agent-call-indicator" id="agentCallIndicator" hidden></div>';
    root = document.getElementById('agentCallIndicator');
    invoke = vi.fn(async () => null);
});
afterEach(() => {
    vi.restoreAllMocks();
    document.body.innerHTML = '';
});

const chip = () => root.querySelector('.agent-call-chip');
const actions = () => root.querySelector('.agent-call-actions');

describe('chipState', () => {
    it('is the running call, or null', () => {
        expect(chipState(RUNNING)).toEqual({ extensionId: 'mindmap', name: 'Mind Map', waiting: 2 });
        expect(chipState({ running: [], waiting: 3 })).toBeNull();
        expect(chipState(null)).toBeNull();
        expect(chipState({ running: [{ name: 'no id' }] })).toBeNull();
        expect(
            chipState({ running: [{ extension_id: 'x' }], waiting: -1 })
        ).toEqual({ extensionId: 'x', name: 'x', waiting: 0 });
    });

    it('keeps the name plain and short', () => {
        const rlo = String.fromCharCode(0x202e);
        const s = chipState({ running: [{ extension_id: 'x', name: `Ev${rlo}il${'n'.repeat(90)}` }] });
        expect(s.name.includes(rlo)).toBe(false);
        expect(Array.from(s.name)).toHaveLength(60);
        expect(s.name.endsWith('…')).toBe(true);
    });
});

describe('the chip', () => {
    it('shows while a call runs and hides when nothing does', () => {
        const ind = new AgentCallIndicator(root, { invoke });
        expect(root.hidden).toBe(true);
        ind.update(RUNNING);
        expect(root.hidden).toBe(false);
        expect(root.querySelector('.agent-call-chip-text').textContent).toBe(
            'Mind Map is using your agent'
        );
        const waiting = root.querySelector('.agent-call-chip-waiting');
        expect(waiting.textContent).toBe('+2 waiting');
        expect(waiting.hidden).toBe(false);
        ind.update({ ...RUNNING, waiting: 0 });
        expect(waiting.hidden).toBe(true);
        ind.update({ running: [], waiting: 1 });
        expect(root.hidden).toBe(true);
    });

    it('names the extension as text only', () => {
        const ind = new AgentCallIndicator(root, { invoke });
        ind.update({ running: [{ extension_id: 'x', name: '<img src=x onerror=alert(1)>' }] });
        expect(root.querySelector('img')).toBeNull();
        expect(root.textContent).toContain('<img src=x onerror=alert(1)>');
    });

    it('a click shows Stop and Settings', () => {
        const ind = new AgentCallIndicator(root, { invoke });
        ind.update(RUNNING);
        expect(actions().hidden).toBe(true);
        expect(chip().getAttribute('aria-expanded')).toBe('false');
        chip().click();
        expect(actions().hidden).toBe(false);
        expect(chip().getAttribute('aria-expanded')).toBe('true');
        expect(chip().getAttribute('aria-controls')).toBe(actions().id);
        expect([...actions().querySelectorAll('button')].map((b) => b.textContent)).toEqual([
            'Stop',
            'Settings',
        ]);
        chip().click();
        expect(actions().hidden).toBe(true);
    });

    it('Stop stops the running extension', async () => {
        const ind = new AgentCallIndicator(root, { invoke });
        ind.update(RUNNING);
        chip().click();
        root.querySelector('[data-agent-call="stop"]').click();
        await Promise.resolve();
        expect(invoke).toHaveBeenCalledWith('stop_extension_agent_calls', {
            extensionId: 'mindmap',
        });
        expect(actions().hidden).toBe(true);
    });

    it("Settings opens the extension's page", async () => {
        const ind = new AgentCallIndicator(root, { invoke });
        ind.update(RUNNING);
        chip().click();
        root.querySelector('[data-agent-call="settings"]').click();
        await Promise.resolve();
        expect(invoke).toHaveBeenCalledWith('open_settings_window', { section: 'ext-mindmap' });
    });

    it('folds the menu away when the call ends or another starts', () => {
        const ind = new AgentCallIndicator(root, { invoke });
        ind.update(RUNNING);
        chip().click();
        ind.update({ running: [{ extension_id: 'weather', name: 'Weather' }], waiting: 0 });
        expect(actions().hidden).toBe(true);
        chip().click();
        ind.update({ running: [], waiting: 0 });
        ind.update(RUNNING);
        expect(actions().hidden).toBe(true);
    });

    it('a failed Stop is logged, not thrown', async () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const ind = new AgentCallIndicator(root, {
            invoke: vi.fn(async () => {
                throw { kind: 'internal', message: 'nope' };
            }),
        });
        ind.update(RUNNING);
        await ind.stop();
        expect(warn).toHaveBeenCalled();
    });
});

describe('installAgentCallIndicator', () => {
    it('follows agent_calls_activity', async () => {
        const handlers = {};
        const listen = vi.fn(async (event, fn) => {
            handlers[event] = fn;
            return () => {};
        });
        await installAgentCallIndicator({ root, invoke, listen });
        expect(listen).toHaveBeenCalledWith('agent_calls_activity', expect.any(Function));
        handlers.agent_calls_activity({ payload: RUNNING });
        expect(root.hidden).toBe(false);
        handlers.agent_calls_activity({ payload: { running: [], waiting: 0 } });
        expect(root.hidden).toBe(true);
    });
});

describe('floating.html', () => {
    const html = readFileSync(path.join(repoRoot, 'ui/floating.html'), 'utf8');

    it('has the row, hidden, before the toolbar, and loads the script', () => {
        const row = html.indexOf('id="agentCallIndicator"');
        expect(row).toBeGreaterThan(0);
        expect(row).toBeLessThan(html.indexOf('id="floatingToolbar"'));
        expect(html).toMatch(/id="agentCallIndicator" hidden/);
        expect(html).toContain('src="js/floating/agent-call-indicator.js"');
    });

    it('styles it with logical properties', () => {
        const css = readFileSync(path.join(repoRoot, 'ui/css/floating-components.css'), 'utf8');
        const start = css.indexOf('.speech-bubble > .agent-call-indicator {');
        const end = css.indexOf('/* Floating toolbar');
        const block = css.slice(start, end);
        expect(start).toBeGreaterThan(0);
        expect(block).toContain('padding-inline');
        expect(block).not.toMatch(/\b(margin|padding)-(left|right)\b/);
        expect(block).toContain('.speech-bubble > .agent-call-indicator[hidden]');
    });
});
