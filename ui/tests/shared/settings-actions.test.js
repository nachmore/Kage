/**
 * Tests for ui/js/settings/actions.js — the delegated event dispatcher
 * that replaced the dozen-plus inline `onclick="globalFn(...)"` attributes
 * in the settings window.
 *
 * Imports the module fresh per test, then verifies:
 *   - data-action="X" on a clicked element calls the registered handler
 *   - data-arg is forwarded
 *   - data-action-change fires only on `change`, not on `click`
 *   - clicks on inner spans bubble up via closest()
 *   - unknown actions warn instead of throwing
 *   - re-importing the module is idempotent (the dispatcher install
 *     guard means we don't accumulate listeners across reloads)
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';

let registerSettingsActions;
let dispatchSettingsAction;

beforeEach(async () => {
    document.body.innerHTML = '';

    // Force a fresh module evaluation per test so the install-once
    // guard inside actions.js doesn't leak listeners between specs.
    vi.resetModules();
    const mod = await import('../../js/settings/actions.js');
    registerSettingsActions = mod.registerSettingsActions;
    dispatchSettingsAction = mod.dispatchSettingsAction;
});

// ---- click dispatch ---------------------------------------------------------

describe('click dispatch', () => {
    it('calls the registered handler with the data-arg value', () => {
        const handler = vi.fn();
        registerSettingsActions({ 'test.run': handler });

        const btn = document.createElement('button');
        btn.dataset.action = 'test.run';
        btn.dataset.arg = 'payload-123';
        document.body.appendChild(btn);

        btn.click();

        expect(handler).toHaveBeenCalledTimes(1);
        const [arg, el] = handler.mock.calls[0];
        expect(arg).toBe('payload-123');
        expect(el).toBe(btn);
    });

    it('walks up to the nearest [data-action] ancestor when clicking inner content', () => {
        // Buttons frequently wrap an icon span — the click target is the
        // span, not the button. closest() must still find the action.
        const handler = vi.fn();
        registerSettingsActions({ 'test.bubble': handler });

        const btn = document.createElement('button');
        btn.dataset.action = 'test.bubble';
        btn.dataset.arg = 'outer';
        const span = document.createElement('span');
        span.textContent = '🛍️';
        btn.appendChild(span);
        document.body.appendChild(btn);

        // Synthesise a click whose target is the inner span.
        span.dispatchEvent(new MouseEvent('click', { bubbles: true }));

        expect(handler).toHaveBeenCalledTimes(1);
        expect(handler.mock.calls[0][0]).toBe('outer');
    });

    it('ignores clicks on elements without data-action', () => {
        // Plain elements should NOT fire any handler — there's no warning
        // and no error.
        const handler = vi.fn();
        registerSettingsActions({ 'test.x': handler });

        const div = document.createElement('div');
        document.body.appendChild(div);
        div.click();

        expect(handler).not.toHaveBeenCalled();
    });

    it('warns and does not throw when action is unregistered', () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});

        const btn = document.createElement('button');
        btn.dataset.action = 'this.does.not.exist';
        document.body.appendChild(btn);

        expect(() => btn.click()).not.toThrow();
        expect(warn).toHaveBeenCalled();
        expect(warn.mock.calls[0].join(' ')).toContain('this.does.not.exist');
        warn.mockRestore();
    });

    it('catches handler exceptions and logs without breaking the page', () => {
        const error = vi.spyOn(console, 'error').mockImplementation(() => {});
        registerSettingsActions({
            'test.boom': () => {
                throw new Error('boom');
            },
        });

        const btn = document.createElement('button');
        btn.dataset.action = 'test.boom';
        document.body.appendChild(btn);

        expect(() => btn.click()).not.toThrow();
        expect(error).toHaveBeenCalled();
        error.mockRestore();
    });
});

// ---- change dispatch --------------------------------------------------------

describe('change dispatch', () => {
    it('fires data-action-change handler on change events with current value', () => {
        const handler = vi.fn();
        registerSettingsActions({ 'test.policy': handler });

        const select = document.createElement('select');
        select.dataset.actionChange = 'test.policy';
        select.dataset.arg = '42';
        const opt1 = document.createElement('option');
        opt1.value = 'a';
        const opt2 = document.createElement('option');
        opt2.value = 'b';
        select.append(opt1, opt2);
        document.body.appendChild(select);

        select.value = 'b';
        select.dispatchEvent(new Event('change', { bubbles: true }));

        expect(handler).toHaveBeenCalledTimes(1);
        const [arg, el] = handler.mock.calls[0];
        expect(arg).toBe('42');
        expect(el).toBe(select);
        expect(el.value).toBe('b');
    });

    it('does not fire data-action-change handler on click', () => {
        // Routing rule: change-only handlers must not be confused with click
        // handlers. A select with only data-action-change must stay quiet
        // when clicked (clicks bubble through its options on jsdom).
        const handler = vi.fn();
        registerSettingsActions({ 'test.changeonly': handler });

        const select = document.createElement('select');
        select.dataset.actionChange = 'test.changeonly';
        document.body.appendChild(select);

        select.click();

        expect(handler).not.toHaveBeenCalled();
    });
});

// ---- registry -------------------------------------------------------------

describe('registry', () => {
    it('overwrites a previously-registered handler with the same name', () => {
        const a = vi.fn();
        const b = vi.fn();
        registerSettingsActions({ 'test.replace': a });
        registerSettingsActions({ 'test.replace': b });

        const btn = document.createElement('button');
        btn.dataset.action = 'test.replace';
        document.body.appendChild(btn);
        btn.click();

        expect(a).not.toHaveBeenCalled();
        expect(b).toHaveBeenCalledTimes(1);
    });

    it('dispatchSettingsAction is callable directly (not just via DOM)', () => {
        // Useful for tests of higher-level modules and for any code that
        // already has the action name + element in hand and wants to skip
        // the click round-trip.
        const handler = vi.fn();
        registerSettingsActions({ 'test.direct': handler });

        const fakeEl = document.createElement('div');
        dispatchSettingsAction('test.direct', 'arg', fakeEl, null);

        expect(handler).toHaveBeenCalledWith('arg', fakeEl, null);
    });

    it('exposes window globals for the soft back-compat shim', () => {
        // While we finish migrating off classic scripts, the module
        // also writes registerSettingsActions / dispatchSettingsAction
        // onto window. Lock that in so nothing depending on it
        // silently breaks.
        expect(typeof window.registerSettingsActions).toBe('function');
        expect(typeof window.dispatchSettingsAction).toBe('function');
    });
});
