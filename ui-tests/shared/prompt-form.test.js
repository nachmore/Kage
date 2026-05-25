import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mountPromptForm } from '../../ui/js/shared/prompt-form.js';

function makeFormCmd(missing, extra = {}) {
    return {
        type: 'prompt_form',
        shortcut: { name: 'Translate', shortcut: 'tr' },
        args: [],
        prefilled: {},
        missing,
        ...extra,
    };
}

describe('mountPromptForm', () => {
    let container;

    beforeEach(() => {
        container = document.createElement('div');
        document.body.appendChild(container);
    });

    it('renders one input per missing placeholder', () => {
        mountPromptForm(container, makeFormCmd([{ name: 'lang' }, { name: 'level' }]));
        const inputs = container.querySelectorAll('.prompt-form-input');
        expect(inputs.length).toBe(2);
        expect(inputs[0].dataset.paramName).toBe('lang');
        expect(inputs[1].dataset.paramName).toBe('level');
    });

    it('marks optional placeholders in the label', () => {
        mountPromptForm(container, makeFormCmd([{ name: 'name', optional: true }]));
        const label = container.querySelector('.prompt-form-label');
        expect(label.textContent).toContain('optional');
    });

    it('throws when given the wrong command type', () => {
        expect(() => mountPromptForm(container, { type: 'prompt' })).toThrow();
        expect(() => mountPromptForm(null, makeFormCmd([{ name: 'x' }]))).toThrow();
    });

    it('calls onSubmit with collected values when Send is clicked', () => {
        const onSubmit = vi.fn();
        mountPromptForm(
            container,
            makeFormCmd([{ name: 'lang' }, { name: 'level' }]),
            { onSubmit }
        );
        const inputs = container.querySelectorAll('.prompt-form-input');
        inputs[0].value = 'Spanish';
        inputs[1].value = 'beginner';
        container.querySelector('.setting-button').click();
        expect(onSubmit).toHaveBeenCalledWith({ lang: 'Spanish', level: 'beginner' });
    });

    it('blocks submit when a required value is empty', () => {
        const onSubmit = vi.fn();
        mountPromptForm(container, makeFormCmd([{ name: 'lang' }]), { onSubmit });
        container.querySelector('.setting-button').click();
        expect(onSubmit).not.toHaveBeenCalled();
        const input = container.querySelector('.prompt-form-input');
        expect(input.classList.contains('prompt-form-input-error')).toBe(true);
    });

    it('allows submit when only an optional value is empty', () => {
        const onSubmit = vi.fn();
        mountPromptForm(
            container,
            makeFormCmd([
                { name: 'lang' },
                { name: 'note', optional: true },
            ]),
            { onSubmit }
        );
        const inputs = container.querySelectorAll('.prompt-form-input');
        inputs[0].value = 'Spanish';
        // note left blank intentionally
        container.querySelector('.setting-button').click();
        expect(onSubmit).toHaveBeenCalled();
        const calledWith = onSubmit.mock.calls[0][0];
        expect(calledWith.lang).toBe('Spanish');
        // Empty optional values should still reach the substitution
        // layer so {note?} can be stripped — it's the absence of the
        // KEY that signals "leave it for the regex to drop."
        // We allow either presence-with-empty-string or absence.
    });

    it('Enter on a row submits', () => {
        const onSubmit = vi.fn();
        mountPromptForm(container, makeFormCmd([{ name: 'lang' }]), { onSubmit });
        const input = container.querySelector('.prompt-form-input');
        input.value = 'Spanish';
        const evt = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true });
        input.dispatchEvent(evt);
        expect(onSubmit).toHaveBeenCalled();
    });

    it('Escape calls onCancel', () => {
        const onCancel = vi.fn();
        mountPromptForm(container, makeFormCmd([{ name: 'lang' }]), { onCancel });
        const input = container.querySelector('.prompt-form-input');
        input.dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
        );
        expect(onCancel).toHaveBeenCalled();
    });

    it('Cancel button calls onCancel', () => {
        const onCancel = vi.fn();
        mountPromptForm(container, makeFormCmd([{ name: 'lang' }]), { onCancel });
        container.querySelector('.prompt-form-cancel').click();
        expect(onCancel).toHaveBeenCalled();
    });

    it('seeds inputs from prefilled values', () => {
        mountPromptForm(
            container,
            makeFormCmd([{ name: 'lang' }, { name: 'level' }], {
                prefilled: { lang: 'French' },
            })
        );
        const inputs = container.querySelectorAll('.prompt-form-input');
        expect(inputs[0].value).toBe('French');
        expect(inputs[1].value).toBe('');
    });

    it('does not allow user content to inject HTML into labels', () => {
        // Defensive: we use textContent, not innerHTML, on the label.
        // If anyone refactors to innerHTML this test will catch it.
        mountPromptForm(container, makeFormCmd([{ name: '<script>alert(1)</script>' }]));
        expect(container.querySelectorAll('script').length).toBe(0);
    });
});
