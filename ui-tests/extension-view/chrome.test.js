/**
 * The view page's own UI (docs/design/EXTENSION_VIEWS.md §8.1, §8.2,
 * §18.2, §18.3): the markup, the host bar, the panels and the banner.
 */

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
    createChrome,
    NOTICE_MS,
    PANEL_ACTIONS,
    windowTitle,
} from '../../ui/js/extension-view/chrome.js';
import { fakeT, mountViewHtml, repoRoot, VIEW_HTML } from './fixture.js';

const EN = JSON.parse(readFileSync(path.join(repoRoot, 'locales/en/messages.json'), 'utf8'));

describe('ui/extension-view.html', () => {
    const doc = new DOMParser().parseFromString(VIEW_HTML, 'text/html');

    it('loads the shared styles, its own CSS and the page module (§8.1)', () => {
        const sheets = [...doc.querySelectorAll('link[rel="stylesheet"]')].map((l) =>
            l.getAttribute('href')
        );
        expect(sheets).toEqual([
            'css/shared-kage-tokens.css',
            'css/shared-components.css',
            'css/shared-rtl.css',
            'css/extension-view.css',
        ]);
        const scripts = [...doc.querySelectorAll('script')];
        expect(scripts).toHaveLength(1);
        expect(scripts[0].getAttribute('type')).toBe('module');
        expect(scripts[0].getAttribute('src')).toBe('js/extension-view/main.js');
    });

    it('blocks page zoom and has no CSP of its own (the srcdoc frame would inherit it)', () => {
        const viewport = doc.querySelector('meta[name="viewport"]').getAttribute('content');
        expect(viewport).toMatch(/user-scalable=no/);
        expect(doc.querySelector('meta[http-equiv="Content-Security-Policy" i]')).toBeNull();
    });

    it('draws the bar outside the stage, so the view can never cover it (§8.2)', () => {
        const bar = doc.querySelector('header.kv-bar');
        const stage = doc.querySelector('main.kv-stage');
        expect(bar).not.toBeNull();
        expect(stage.contains(bar)).toBe(false);
        expect(bar.compareDocumentPosition(stage) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
        expect(stage.querySelector('.kv-frame')).not.toBeNull();
    });

    it('labels every bar button, and every static key is in the EN catalog', () => {
        const buttons = [...doc.querySelectorAll('.kv-bar button')];
        expect(buttons.map((b) => b.getAttribute('data-kv-action'))).toEqual([
            'reload',
            'settings',
            'close',
        ]);
        for (const b of buttons) {
            expect(b.getAttribute('type')).toBe('button');
            expect(b.getAttribute('aria-label')).toBeTruthy();
            expect(b.getAttribute('data-i18n-aria-label')).toBeTruthy();
            expect(b.getAttribute('data-i18n-title')).toBeTruthy();
        }
        const keys = new Set();
        for (const el of doc.querySelectorAll('*')) {
            for (const attr of el.getAttributeNames()) {
                if (attr.startsWith('data-i18n') && attr !== 'data-i18n-args') {
                    keys.add(el.getAttribute(attr));
                }
            }
        }
        expect(keys.size).toBeGreaterThan(0);
        for (const key of keys) expect(EN[key], key).toBeTruthy();
    });

    it('marks extension-supplied text for bidi isolation', () => {
        for (const sel of ['.kv-bar-ext', '.kv-bar-view', '.kv-bar-sub', '.kv-state-text']) {
            expect(doc.querySelector(sel).getAttribute('dir')).toBe('auto');
        }
    });
});

describe('createChrome', () => {
    let onAction;
    let chrome;

    beforeEach(() => {
        mountViewHtml();
        onAction = vi.fn();
        chrome = createChrome({ t: fakeT, onAction });
    });

    afterEach(() => {
        vi.useRealTimers();
        document.body.innerHTML = '';
    });

    it('shows the identity: icon, extension name, "·", view title, badge tooltip', () => {
        chrome.setIdentity({ icon: '🗺️', extensionName: 'Mind Map', viewTitle: 'Explorer' });
        expect(document.querySelector('.kv-bar-icon').textContent).toBe('🗺️');
        expect(document.querySelector('.kv-bar-ext').textContent).toBe('Mind Map');
        expect(document.querySelector('.kv-bar-view').textContent).toBe('Explorer');
        expect(document.querySelector('.kv-bar-sep').hidden).toBe(false);
        expect(document.querySelector('.kv-badge').title).toBe(
            'ext_view.badge_tip {"extension":"Mind Map"}'
        );
    });

    it('puts extension text in as text, never as HTML', () => {
        chrome.setIdentity({
            extensionName: '<img src=x onerror=alert(1)>',
            viewTitle: '<b>x</b>',
        });
        chrome.setSubtitle('<script>1</script>');
        expect(document.querySelector('.kv-bar img')).toBeNull();
        expect(document.querySelector('.kv-bar b')).toBeNull();
        expect(document.querySelector('.kv-bar-ext').textContent).toBe(
            '<img src=x onerror=alert(1)>'
        );
        expect(document.querySelector('.kv-bar-sub').textContent).toBe('<script>1</script>');
    });

    it('hides the icon and separator when there is nothing to show', () => {
        chrome.setIdentity({ extensionName: 'Mind Map', viewTitle: '' });
        expect(document.querySelector('.kv-bar-icon').hidden).toBe(true);
        expect(document.querySelector('.kv-bar-sep').hidden).toBe(true);
    });

    it('shows and clears the subtitle', () => {
        expect(chrome.subtitle).toBeNull();
        chrome.setSubtitle('Bingo');
        expect(document.querySelector('.kv-bar-sub').hidden).toBe(false);
        expect(chrome.subtitle).toBe('Bingo');
        chrome.setSubtitle(null);
        expect(document.querySelector('.kv-bar-sub').hidden).toBe(true);
        expect(chrome.subtitle).toBeNull();
    });

    it('loading state: "Opening {view}…" until hidden', () => {
        chrome.showLoading('Explorer');
        const panel = document.querySelector('.kv-loading');
        expect(panel.hidden).toBe(false);
        expect(panel.getAttribute('aria-busy')).toBe('true');
        expect(panel.textContent).toContain('ext_view.loading {"view":"Explorer"}');
        chrome.hideLoading();
        expect(panel.hidden).toBe(true);
        expect(panel.hasAttribute('aria-busy')).toBe(false);
    });

    it('state panel: message and buttons in a fixed order, replacing the loading panel', () => {
        chrome.showLoading('Explorer');
        chrome.showState({ message: 'Explorer couldn’t open.', actions: ['settings', 'reload'] });
        expect(document.querySelector('.kv-loading').hidden).toBe(true);
        expect(chrome.stateVisible).toBe(true);
        expect(document.querySelector('.kv-state-text').textContent).toBe(
            'Explorer couldn’t open.'
        );
        const buttons = [...document.querySelectorAll('.kv-state-actions button')];
        expect(buttons.map((b) => b.getAttribute('data-kv-action'))).toEqual([
            'reload',
            'settings',
        ]);
        expect(buttons.map((b) => b.textContent)).toEqual([
            'ext_view.bar.reload',
            'ext_view.bar.settings',
        ]);
        expect(buttons.every((b) => b.type === 'button')).toBe(true);

        chrome.showState({ message: 'needs approval', actions: ['approve', 'close', 'bogus'] });
        const again = [...document.querySelectorAll('.kv-state-actions button')];
        expect(again.map((b) => b.getAttribute('data-kv-action'))).toEqual(['approve', 'close']);
        expect(again[0].textContent).toBe('ext_view.state.approve');
        expect(again[0].classList.contains('kv-btn-primary')).toBe(true);

        chrome.hideState();
        expect(chrome.stateVisible).toBe(false);
        expect(document.querySelectorAll('.kv-state-actions button')).toHaveLength(0);
    });

    it('routes every host button through onAction', () => {
        for (const action of ['reload', 'settings', 'close']) {
            document.querySelector(`.kv-bar [data-kv-action="${action}"]`).click();
        }
        chrome.showState({ message: 'x', actions: ['approve'] });
        document.querySelector('.kv-state-actions [data-kv-action="approve"]').click();
        chrome.showBanner({ message: 'y', actions: ['reload'] });
        document.querySelector('.kv-banner [data-kv-action="reload"]').click();
        expect(onAction.mock.calls.map((c) => c[0])).toEqual([
            'reload',
            'settings',
            'close',
            'approve',
            'reload',
        ]);
        expect(PANEL_ACTIONS).toEqual(['approve', 'reload', 'settings', 'close']);
    });

    it('ignores a click on anything that is not a known action', () => {
        const odd = document.createElement('button');
        odd.setAttribute('data-kv-action', 'uninstall');
        document.body.appendChild(odd);
        odd.click();
        document.querySelector('.kv-bar-ext').click();
        expect(onAction).not.toHaveBeenCalled();
    });

    it('banner: message and actions, then hidden', () => {
        chrome.showBanner({ message: 'Explorer isn’t responding.', actions: ['reload', 'close'] });
        expect(chrome.bannerVisible).toBe(true);
        expect(document.querySelector('.kv-banner-text').textContent).toBe(
            'Explorer isn’t responding.'
        );
        expect(document.querySelectorAll('.kv-banner-actions button')).toHaveLength(2);
        chrome.hideBanner();
        expect(chrome.bannerVisible).toBe(false);
    });

    it('a notice shows in the bar for 3 s', () => {
        vi.useFakeTimers();
        expect(NOTICE_MS).toBe(3_000);
        chrome.flashNotice('Updated to 1.2.0.');
        const notice = document.querySelector('.kv-bar-notice');
        expect(notice.hidden).toBe(false);
        expect(notice.textContent).toBe('Updated to 1.2.0.');
        vi.advanceTimersByTime(2_999);
        expect(notice.hidden).toBe(false);
        vi.advanceTimersByTime(1);
        expect(notice.hidden).toBe(true);
    });

    it('exposes the stage and the iframe in it', () => {
        expect(chrome.stage).toBe(document.querySelector('.kv-frame'));
        expect(chrome.frame).toBeNull();
        const frame = document.createElement('iframe');
        chrome.stage.appendChild(frame);
        expect(chrome.frame).toBe(frame);
    });
});

describe('windowTitle (§18.2)', () => {
    it('ends with the extension name; a subtitle goes in front', () => {
        expect(windowTitle(fakeT, { view: 'Explorer', extension: 'Mind Map' })).toBe(
            'ext_view.window_title {"view":"Explorer","extension":"Mind Map"}'
        );
        expect(
            windowTitle(fakeT, { view: 'Explorer', extension: 'Mind Map', subtitle: 'Bingo' })
        ).toBe(
            'ext_view.window_title_sub {"subtitle":"Bingo","view":"Explorer","extension":"Mind Map"}'
        );
    });

    it('formats with the real EN templates', () => {
        const en = (key, vars) =>
            EN[key].message.replace(/\{(\w+)\}/g, (_, name) => String(vars[name]));
        expect(windowTitle(en, { view: 'Explorer', extension: 'Mind Map' })).toBe(
            'Explorer — Mind Map'
        );
        expect(
            windowTitle(en, { view: 'Explorer', extension: 'Mind Map', subtitle: 'Bingo' })
        ).toBe('Bingo — Explorer — Mind Map');
    });
});
