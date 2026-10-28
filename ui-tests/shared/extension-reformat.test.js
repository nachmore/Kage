/**
 * Formatter re-run on request (`context.formatter.requestReformat`): the
 * extension manager re-runs only the requesting extension's formatter, on
 * still-attached messages it already formatted, coalescing bursts.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { installExtensionUiMethods } from '../../ui/js/shared/extension-manager/ui.js';

class FakeManager {
    constructor(extensions) {
        this.extensions = new Map(extensions);
    }
    _isEnabled() {
        return true;
    }
    _wireExtActionsFor() {}
}
installExtensionUiMethods(FakeManager);

function formatterExt(fn) {
    return { sandbox: { hasFormatter: true, call: vi.fn(async (_m, args) => fn(args)) } };
}

describe('extension formatter reformat', () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => {
        vi.useRealTimers();
        document.body.innerHTML = '';
    });

    it('re-runs only the requesting formatter on formatted, attached messages', async () => {
        let ready = false;
        const cards = formatterExt(({ html }) => ({
            html: html.replace(/<i>(loading|done)<\/i>|$/, ready ? '<i>done</i>' : '<i>loading</i>'),
        }));
        const other = formatterExt(() => null);
        const mgr = new FakeManager([
            ['cards', cards],
            ['other', other],
        ]);

        const attached = document.createElement('div');
        attached.innerHTML = '<p>hi</p>';
        document.body.appendChild(attached);
        const detached = document.createElement('div');
        detached.innerHTML = '<p>gone</p>';

        await mgr.formatMessage(attached, { streaming: false });
        await mgr.formatMessage(detached, { streaming: false });
        expect(attached.innerHTML).toBe('<p>hi</p><i>loading</i>');
        const callsBefore = cards.sandbox.call.mock.calls.length;
        const otherBefore = other.sandbox.call.mock.calls.length;

        ready = true;
        mgr.requestReformat('cards');
        mgr.requestReformat('cards'); // coalesced
        await vi.advanceTimersByTimeAsync(150);

        expect(attached.innerHTML).toBe('<p>hi</p><i>done</i>');
        expect(cards.sandbox.call.mock.calls.length - callsBefore).toBe(1);
        expect(other.sandbox.call.mock.calls.length).toBe(otherBefore);
    });

    it('drops formatter images that fail to load, hiding a wrapper left empty', async () => {
        const cards = formatterExt(() => ({
            html:
                '<a class="card" href="https://x.test/"><span class="hero">' +
                '<img src="https://img.test/hero.png"></span>' +
                '<span class="icon"><img src="https://img.test/i.png">X</span></a>',
        }));
        const mgr = new FakeManager([['cards', cards]]);
        const el = document.createElement('div');
        document.body.appendChild(el);
        el.innerHTML = '<p>hi</p>';
        await mgr.formatMessage(el, { streaming: false });

        const [hero, icon] = el.querySelectorAll('img');
        hero.dispatchEvent(new Event('error'));
        icon.dispatchEvent(new Event('error'));

        expect(el.querySelectorAll('img').length).toBe(0);
        expect(el.querySelector('.hero').style.display).toBe('none');
        // Still has text, so it stays visible.
        expect(el.querySelector('.icon').style.display).toBe('');
    });

    it('does not track streaming renders', async () => {
        const cards = formatterExt(() => ({ html: '<p>x</p>' }));
        cards.sandbox.formatterOptsInStreaming = true;
        const mgr = new FakeManager([['cards', cards]]);
        const el = document.createElement('div');
        el.innerHTML = '<p>s</p>';
        document.body.appendChild(el);
        await mgr.formatMessage(el, { streaming: true });
        const before = cards.sandbox.call.mock.calls.length;
        mgr.requestReformat('cards');
        await vi.advanceTimersByTimeAsync(150);
        expect(cards.sandbox.call.mock.calls.length).toBe(before);
    });
});
