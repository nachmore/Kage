import { describe, it, expect, vi } from 'vitest';
import {
    extractSelectionOptions,
    selectionSubmitArgs,
    loadSelection,
    submitSelection,
} from '../../ui/js/shared/slash-selection.js';

// Replies below are trimmed copies of REAL kiro-cli output captured via
// scripts/probe_slash.py — keep them shaped like the agent's actual contract.

const AGENT_REPLY = {
    success: true,
    message: '→ kiro_default - The default agent\n  kiro_planner - Planning agent',
    data: {
        agents: [
            { name: 'kiro_default', description: 'The default agent', source: 'Built-in' },
            { name: 'kiro_planner', description: 'Planning agent', source: 'Built-in' },
        ],
        current: 'kiro_default',
    },
};

const MODEL_REPLY = {
    success: true,
    message: '→ auto (auto)\n  claude-opus-4.8 (claude-opus-4.8)',
    data: {
        models: [
            { id: 'auto', displayName: 'auto', description: 'Auto mode' },
            { id: 'claude-opus-4.8', displayName: 'claude-opus-4.8', description: 'Opus' },
        ],
        current: 'auto',
    },
};

describe('extractSelectionOptions', () => {
    it('extracts agents from data.agents and flags current', () => {
        const { options, current } = extractSelectionOptions(AGENT_REPLY);
        expect(current).toBe('kiro_default');
        expect(options).toHaveLength(2);
        expect(options[0]).toMatchObject({
            label: 'kiro_default',
            value: 'kiro_default',
            description: 'The default agent',
            current: true,
        });
        expect(options[1].current).toBe(false);
    });

    it('extracts models using id as value and displayName as label', () => {
        const { options, current } = extractSelectionOptions(MODEL_REPLY);
        expect(current).toBe('auto');
        expect(options[1]).toMatchObject({
            label: 'claude-opus-4.8',
            value: 'claude-opus-4.8',
            current: false,
        });
        expect(options[0].current).toBe(true);
    });

    it('returns empty options when reply has no structured data', () => {
        // e.g. /feedback "Opening in browser…", /prompts "Use the selection menu…"
        expect(extractSelectionOptions({ success: true, message: 'Opening in browser...' })).toEqual(
            { options: [], current: null }
        );
        expect(extractSelectionOptions(null)).toEqual({ options: [], current: null });
    });

    it('handles a generic options array with explicit label/value', () => {
        const { options } = extractSelectionOptions({
            data: { options: [{ label: 'A', value: 'a' }], current: 'a' },
        });
        expect(options[0]).toMatchObject({ label: 'A', value: 'a', current: true });
    });
});

describe('selectionSubmitArgs', () => {
    it('builds {<command>Name: value} — the shape the agent accepts', () => {
        expect(selectionSubmitArgs('agent', 'kiro_planner')).toEqual({ agentName: 'kiro_planner' });
        expect(selectionSubmitArgs('model', 'auto')).toEqual({ modelName: 'auto' });
    });
});

describe('loadSelection', () => {
    it('classifies a structured list reply as kind:options', async () => {
        const invoke = vi.fn().mockResolvedValue(AGENT_REPLY);
        const res = await loadSelection(invoke, 'sess-1', 'agent');
        expect(invoke).toHaveBeenCalledWith('execute_slash_command', {
            sessionId: 'sess-1',
            command: 'agent',
            args: null,
        });
        expect(res.kind).toBe('options');
        expect(res.command).toBe('agent');
        expect(res.options).toHaveLength(2);
    });

    it('classifies a list-less reply as kind:message with the agent text', async () => {
        const invoke = vi.fn().mockResolvedValue({ success: true, message: 'Opening in browser...' });
        const res = await loadSelection(invoke, null, 'feedback');
        expect(res).toEqual({ kind: 'message', text: 'Opening in browser...' });
    });
});

describe('submitSelection', () => {
    it('sends the correct arg-shape and returns the reply message', async () => {
        const invoke = vi.fn().mockResolvedValue({ success: true, message: 'Agent changed to kiro_planner' });
        const msg = await submitSelection(invoke, 'sess-1', 'agent', 'kiro_planner');
        expect(invoke).toHaveBeenCalledWith('execute_slash_command', {
            sessionId: 'sess-1',
            command: 'agent',
            args: { agentName: 'kiro_planner' },
        });
        expect(msg).toBe('Agent changed to kiro_planner');
    });

    it('returns empty string when the reply has no message', async () => {
        const invoke = vi.fn().mockResolvedValue({ success: true });
        expect(await submitSelection(invoke, null, 'model', 'auto')).toBe('');
    });
});
