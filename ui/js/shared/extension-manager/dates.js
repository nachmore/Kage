/**
 * Date-picker plumbing between the host's month-calendar popup and the
 * extension that provides calendar data (the calendar extension).
 *
 * The popup is HOST UI (ui/js/shared/date-picker.js). Extensions never render
 * into it and never get DOM access to it; they take part through three
 * narrow, validated channels, all gated on the window offering
 * `HOST_FEATURE_DATE_PICKER` and the extension holding the `calendar` grant:
 *
 *   1. Affordances — an extension marks a date label in its widget / result
 *      HTML as `<button data-kage-date="YYYY-MM-DD">`. The sanitizer keeps
 *      the attribute only on buttons with a real ISO date; here we wire a
 *      click that asks the window to open the popup anchored to it. If the
 *      extension may not use the picker, the button is unwrapped to plain
 *      text so it isn't a dead control.
 *   2. Markers — `getDateMarkers(start, end)` on the search provider answers
 *      which days have events. `normalizeMarkers` keeps only dates, counts
 *      and hex colours; the extension can't put text in the popup.
 *   3. Selection — `onDateSelected(date)` returns `{ query }`. The query
 *      must start with one of the provider's OWN registered keywords, so a
 *      date pick can only ever re-enter that extension's search results.
 */

import { applyMixin } from '../mixin.js';
import { emptyMarkers, normalizeMarkers } from '../date-markers.js';
import { HOST_FEATURE_DATE_PICKER } from '../extension-sandbox-host.js';
import { isValidIsoDate } from '../month-grid.js';
import { t } from '../i18n.js';

/** Attribute an extension puts on a `<button>` to make it a date opener. */
export const DATE_AFFORDANCE_ATTR = 'data-kage-date';

/** Longest query a provider may route a date pick to. */
const MAX_DATE_QUERY_LENGTH = 200;

/** A routed query must be one line of text: no C0 controls, no DEL. */
function hasControlChars(s) {
    for (let i = 0; i < s.length; i++) {
        const c = s.charCodeAt(i);
        if (c < 0x20 || c === 0x7f) return true;
    }
    return false;
}

/** Turn a disallowed affordance button back into inert text. */
function unwrapAffordance(el) {
    const span = el.ownerDocument.createElement('span');
    if (el.className) span.className = el.className;
    while (el.firstChild) span.appendChild(el.firstChild);
    el.replaceWith(span);
}

export function installExtensionDateMethods(ExtensionManager) {
    applyMixin(ExtensionManager.prototype, {
        /** Loaded, enabled, offered the picker, holds `calendar`, implements onDateSelected. */
        _isDateProvider(id) {
            const ext = this.extensions?.get(id);
            if (!ext?.sandbox?.hasDateProvider) return false;
            if (!this._isEnabled(id)) return false;
            return Array.isArray(ext.capabilities) && ext.capabilities.includes('calendar');
        },

        /**
         * The extension that answers for host-originated date picks (the
         * built-in clock/date in the floating window). First eligible one in
         * load order, or null when none is installed/enabled.
         */
        getDateProvider() {
            if (!this.extensions) return null;
            for (const id of this.extensions.keys()) {
                if (this._isDateProvider(id)) return id;
            }
            return null;
        },

        /**
         * Why `getDateProvider()` came back empty, as the picker's marker
         * status: `'provider_outdated'` when an enabled extension holding
         * `calendar` was offered the picker but doesn't implement it (a
         * Calendar extension from before the picker existed — "update it",
         * not "turn it on"), else `'no_provider'`.
         */
        missingDateProviderStatus() {
            if (!this.extensions) return 'no_provider';
            for (const [id, ext] of this.extensions) {
                const sb = ext?.sandbox;
                if (!sb?.hasSearch || sb.hasDateProvider || !this._isEnabled(id)) continue;
                if (!Array.isArray(ext.capabilities) || !ext.capabilities.includes('calendar')) {
                    continue;
                }
                if (sb.hostFeatures?.includes?.(HOST_FEATURE_DATE_PICKER)) {
                    return 'provider_outdated';
                }
            }
            return 'no_provider';
        },

        /**
         * Busy-day markers for `[start, end)` from provider `id`. Never
         * throws: no provider → `no_provider`, RPC failure → `error`, a
         * provider without `getDateMarkers` → `ok` with no days.
         */
        async fetchDateMarkers(id, start, end) {
            if (!id || !this._isDateProvider(id)) return emptyMarkers('no_provider');
            const ext = this.extensions.get(id);
            try {
                const raw = await ext.sandbox.call('getDateMarkers', { start, end });
                return raw == null ? emptyMarkers('ok') : normalizeMarkers(raw, start, end);
            } catch (e) {
                console.warn(`getDateMarkers() in '${id}' failed:`, e);
                return emptyMarkers('error');
            }
        },

        /**
         * Ask provider `id` which launcher query lists `iso`'s meetings.
         * Returns the query, or null when there's no provider, it declined,
         * or its answer isn't a single-line query that starts with one of
         * its own keywords.
         */
        async resolveDateQuery(id, iso) {
            if (!id || !isValidIsoDate(iso) || !this._isDateProvider(id)) return null;
            const ext = this.extensions.get(id);
            let out;
            try {
                out = await ext.sandbox.call('onDateSelected', { date: iso });
            } catch (e) {
                console.warn(`onDateSelected() in '${id}' failed:`, e);
                return null;
            }
            const query = typeof out?.query === 'string' ? out.query.trim() : '';
            if (!query || query.length > MAX_DATE_QUERY_LENGTH || hasControlChars(query)) {
                return null;
            }
            let defs = [];
            try {
                defs = await this.getKeywordDefinitions();
            } catch {
                return null;
            }
            const lower = query.toLowerCase();
            const ownKeyword = defs.some(
                (d) =>
                    d.extensionId === id &&
                    (lower === d.keyword || lower.startsWith(d.keyword + ' '))
            );
            if (!ownKeyword) {
                console.warn(`onDateSelected() in '${id}' returned a query outside its keywords`);
                return null;
            }
            return query;
        },

        /**
         * Register the window's "open the date picker" callback. Only windows
         * that host the popup (the floating window) set one; elsewhere every
         * affordance is unwrapped to text. `onWired` hears about every opener
         * as it's wired — re-renders (a widget's periodic refresh) replace
         * the button an open popup is anchored to.
         *
         * @param {((req: { anchor: HTMLElement, date: string, extensionId: string }) => void) | null} fn
         * @param {{ onWired?: (el: HTMLElement, info: { date: string, extensionId: string }) => void }} [opts]
         */
        setDateAffordanceHandler(fn, { onWired } = {}) {
            this._dateAffordanceHandler = typeof fn === 'function' ? fn : null;
            this._dateAffordanceOnWired =
                this._dateAffordanceHandler && typeof onWired === 'function' ? onWired : null;
        },

        /** Wire (or unwrap) every `data-kage-date` affordance under `root`. */
        _wireDateAffordances(id, root) {
            const nodes = root?.querySelectorAll?.(`[${DATE_AFFORDANCE_ATTR}]`);
            if (!nodes?.length) return;
            const allowed = !!this._dateAffordanceHandler && this._isDateProvider(id);
            for (const el of nodes) {
                const iso = el.getAttribute(DATE_AFFORDANCE_ATTR);
                if (!allowed || el.tagName !== 'BUTTON' || !isValidIsoDate(iso)) {
                    unwrapAffordance(el);
                    continue;
                }
                if (el.__kageDateWired) continue;
                el.__kageDateWired = true;
                el.setAttribute('type', 'button');
                el.classList.add('kage-date-affordance');
                el.setAttribute('aria-haspopup', 'dialog');
                const label = el.textContent.trim();
                el.setAttribute('aria-label', t('shared.date_picker.open_for', { date: label }));
                if (!el.getAttribute('title'))
                    el.setAttribute('title', t('shared.date_picker.open_title'));
                el.addEventListener('click', (ev) => {
                    // Don't let the row's own click (execute the result) or a
                    // widget action fire underneath.
                    ev.preventDefault();
                    ev.stopPropagation();
                    this._dateAffordanceHandler?.({ anchor: el, date: iso, extensionId: id });
                });
                this._dateAffordanceOnWired?.(el, { date: iso, extensionId: id });
            }
        },
    });
}
