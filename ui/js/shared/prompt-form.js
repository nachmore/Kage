/**
 * Prompt form — collects values for unfilled named placeholders in a
 * prompt-type Quick Command.
 *
 * Shown when buildShortcutCommand returns `{ type: 'prompt_form', ... }`.
 * The launcher (floating window) renders this into the response area
 * so the user can fill in the missing pieces and hit Enter to send.
 *
 * Why a tiny custom form rather than reusing the LineEditor: the
 * LineEditor is one-input-per-row plus reorder/delete buttons, which
 * isn't the UX we want here. A prompt form is N labelled inputs that
 * become a single submit. Trying to fold that into LineEditor would
 * sprout flags fast.
 *
 * Lifecycle:
 *   - `mount(container, formCmd, callbacks)` builds the DOM, focuses
 *     the first input, and returns a controller with `unmount()`.
 *   - On Enter (anywhere), `callbacks.onSubmit(paramsByName)` fires.
 *   - On Esc / Cancel, `callbacks.onCancel()` fires.
 *
 * Markup-builds use textContent / setAttribute (no innerHTML on user
 * data) so user-supplied placeholder names can't smuggle markup.
 */

import { t } from './i18n.js';

export function mountPromptForm(container, formCmd, callbacks = {}) {
    if (!container) throw new Error('mountPromptForm: container required');
    if (!formCmd || formCmd.type !== 'prompt_form') {
        throw new Error('mountPromptForm: expected a prompt_form command');
    }

    const root = document.createElement('div');
    root.className = 'prompt-form-root';

    const title = document.createElement('div');
    title.className = 'prompt-form-title';
    const shortcutName =
        formCmd.shortcut?.name ||
        formCmd.shortcut?.shortcut ||
        t('shared.prompt_form.default_title');
    title.textContent = shortcutName;
    root.appendChild(title);

    const subtitle = document.createElement('div');
    subtitle.className = 'prompt-form-subtitle';
    const missingCount = formCmd.missing.length;
    subtitle.textContent =
        missingCount === 1
            ? t('shared.prompt_form.subtitle.one')
            : t('shared.prompt_form.subtitle.many', { count: missingCount });
    root.appendChild(subtitle);

    const list = document.createElement('div');
    list.className = 'prompt-form-list';
    root.appendChild(list);

    const inputsByName = {};
    for (const slot of formCmd.missing) {
        const row = document.createElement('label');
        row.className = 'prompt-form-row';
        const labelText = document.createElement('span');
        labelText.className = 'prompt-form-label';
        labelText.textContent = slot.optional
            ? t('shared.prompt_form.optional_suffix', { name: slot.name })
            : slot.name;
        const input = document.createElement('input');
        input.type = 'text';
        input.className = 'setting-input prompt-form-input';
        input.spellcheck = false;
        input.dataset.paramName = slot.name;
        // Pre-fill with anything already known (from positional args
        // earlier in the chain). Most of the time these slots are
        // actually empty — that's what put them in `missing` — but
        // the API supports prefilled-from-args for symmetry.
        if (formCmd.prefilled && formCmd.prefilled[slot.name] !== undefined) {
            input.value = formCmd.prefilled[slot.name];
        }
        row.appendChild(labelText);
        row.appendChild(input);
        list.appendChild(row);
        inputsByName[slot.name] = input;
    }

    const actions = document.createElement('div');
    actions.className = 'prompt-form-actions';
    const submitBtn = document.createElement('button');
    submitBtn.type = 'button';
    submitBtn.className = 'setting-button';
    submitBtn.textContent = t('shared.prompt_form.send_btn');
    const cancelBtn = document.createElement('button');
    cancelBtn.type = 'button';
    cancelBtn.className = 'setting-button prompt-form-cancel';
    cancelBtn.textContent = t('shared.prompt_form.cancel_btn');
    actions.appendChild(submitBtn);
    actions.appendChild(cancelBtn);
    root.appendChild(actions);

    container.innerHTML = '';
    container.appendChild(root);

    function collect() {
        const out = { ...(formCmd.prefilled || {}) };
        for (const [name, input] of Object.entries(inputsByName)) {
            const v = input.value;
            if (v !== '' || formCmd.missing.find((m) => m.name === name)?.optional) {
                out[name] = v;
            }
        }
        return out;
    }

    function submit() {
        // Required-empty rows block submit. Optional empties pass
        // through (the substitution layer drops {name?} when blank).
        for (const slot of formCmd.missing) {
            if (slot.optional) continue;
            const input = inputsByName[slot.name];
            if (!input.value.trim()) {
                input.focus();
                input.classList.add('prompt-form-input-error');
                setTimeout(() => input.classList.remove('prompt-form-input-error'), 800);
                return;
            }
        }
        callbacks.onSubmit?.(collect());
    }

    function cancel() {
        callbacks.onCancel?.();
    }

    submitBtn.addEventListener('click', submit);
    cancelBtn.addEventListener('click', cancel);
    list.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') {
            e.preventDefault();
            submit();
        } else if (e.key === 'Escape') {
            e.preventDefault();
            cancel();
        }
    });

    // Focus the first input so the user can start typing without
    // clicking — same pattern as the existing dialogs.
    setTimeout(() => {
        const first = Object.values(inputsByName)[0];
        if (first) first.focus();
    }, 0);

    return {
        unmount() {
            container.innerHTML = '';
        },
        focus() {
            const first = Object.values(inputsByName)[0];
            if (first) first.focus();
        },
    };
}
