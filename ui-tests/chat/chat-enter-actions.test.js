/**
 * The chat window applies handleEnterAction's verdict the same way the
 * floating window does: a `replace_input` result keeps the text its
 * onReplaceInput callback put in the box (todos' delete question, math's
 * answer, keyword completions) instead of clearing it.
 */
import { describe, it, expect, vi } from 'vitest';
import { createChatActionsMixin } from '../../ui/js/chat/app/chat-actions.js';

function makeApp(result) {
    const Mixin = createChatActionsMixin({
        handleEnterAction: vi.fn(async () => result),
        executeResultShared: vi.fn(),
    });
    const input = { value: 'todo-', style: { height: '40px' } };
    const app = Object.assign(Object.create(Mixin.prototype), {
        elements: { chatInput: input },
        attachmentManager: { hasAttachments: () => false },
        currentSuggestions: [{ id: 'todo-1' }],
        suggestionIndex: 0,
        shortcuts: [],
        isWaitingForResponse: false,
        _getExecCtx: () => ({}),
        clearSuggestions: vi.fn(),
        sendMessage: vi.fn(),
        stopGenerating: vi.fn(),
    });
    return { app, input };
}

describe('chat handleEnterKey verdicts', () => {
    it('replace_input keeps the replaced text and the suggestions', async () => {
        const { app, input } = makeApp({ handled: true, action: 'replace_input' });
        input.value = 'todo-';
        await app.handleEnterKey();
        expect(input.value).toBe('todo-');
        expect(app.clearSuggestions).not.toHaveBeenCalled();
    });

    it('keep_suggestions clears the text but keeps the picker', async () => {
        const { app, input } = makeApp({ handled: true, action: 'keep_suggestions' });
        await app.handleEnterKey();
        expect(input.value).toBe('');
        expect(app.clearSuggestions).not.toHaveBeenCalled();
    });

    it('other handled results clear the input and the suggestions', async () => {
        const { app, input } = makeApp({ handled: true });
        await app.handleEnterKey();
        expect(input.value).toBe('');
        expect(app.clearSuggestions).toHaveBeenCalled();
    });

    it('unhandled results leave everything alone', async () => {
        const { app, input } = makeApp({ handled: false });
        await app.handleEnterKey();
        expect(input.value).toBe('todo-');
        expect(app.clearSuggestions).not.toHaveBeenCalled();
    });
});
