/**
 * Floating-window host for the month-calendar date picker.
 *
 * Openers:
 *   - the built-in clock/date in the input bar (`#datetimeDate`), and
 *   - extension date labels marked `<button data-kage-date>` (the calendar
 *     extension's next-meeting bar and its result rows), wired by
 *     ExtensionManager._wireDateAffordances.
 *
 * The popup is rendered INSIDE the floating webview, as an in-flow row of the
 * speech bubble placed right under the opener's row. In-flow matters: the OS
 * window is sized to the bubble's flow children (see window.js), so the
 * window grows to fit the calendar automatically, works over full-screen apps
 * exactly like the rest of the launcher, and never steals focus from the
 * window (a separate popup window would trip the floating window's
 * blur-to-hide).
 *
 * Picking a day closes the popup and routes to the provider extension's own
 * launcher query (e.g. `cal 2026-11-05`) by filling the input, so the day's
 * meetings render through the normal search-result list — empty state
 * included — rather than a second, parallel list.
 */

import { DatePicker } from '../shared/date-picker.js';
import { createMarkerCache, emptyMarkers } from '../shared/date-markers.js';
import { activeLanguage, isRtl, systemLanguage } from '../shared/i18n.js';
import {
    isValidIsoDate,
    resolvePickerLocale,
    todayIso,
    weekStartFor,
} from '../shared/month-grid.js';

/** Body class while the popup sits over (and hides) the result list. */
const OVER_SUGGESTIONS_CLASS = 'kage-date-picker-over-suggestions';

/** Gap kept between the popup and the bubble's edges, logical px. */
const EDGE_GAP = 8;

export class FloatingCalendarPicker {
    /**
     * @param {object} o
     * @param {object} o.extensionManager  ExtensionManager (date provider methods)
     * @param {HTMLTextAreaElement} o.input  the launcher input
     * @param {Document} [o.document]
     * @param {() => Date} [o.now]
     * @param {() => string} [o.getLocale]   override the locale (tests)
     * @param {() => boolean} [o.getRtl]
     */
    constructor({ extensionManager, input, document: doc, now, getLocale, getRtl }) {
        this.extMgr = extensionManager;
        this.input = input;
        this.doc = doc || globalThis.document;
        this.now = now || (() => new Date());
        this.getLocale =
            getLocale || (() => resolvePickerLocale(activeLanguage(), systemLanguage()));
        this.getRtl = getRtl || (() => isRtl());
        this.picker = null;
        this.row = null;
        this.anchor = null;
        this.extensionId = null;
        this.markerCache = createMarkerCache({
            fetch: (extId, start, end) => this.extMgr.fetchDateMarkers(extId, start, end),
        });
        this._onDocPointerDown = (e) => this._handleOutsidePointer(e);
        this.extMgr?.setDateAffordanceHandler?.((req) => this.open(req));
    }

    isOpen() {
        return !!this.picker;
    }

    /**
     * Turn the built-in date label into the picker's opener. It opens on
     * today, routed to whichever extension provides calendar data.
     */
    attachBuiltInDate(button) {
        if (!button) return;
        button.addEventListener('click', (e) => {
            e.preventDefault();
            e.stopPropagation();
            this.open({
                anchor: button,
                date: todayIso(this.now()),
                extensionId: this.extMgr?.getDateProvider?.() || null,
            });
        });
    }

    /**
     * Open (or, for the same opener, toggle closed) the popup.
     * @param {{ anchor: HTMLElement, date?: string, extensionId?: string|null }} req
     */
    open({ anchor, date, extensionId = null }) {
        if (this.picker && anchor === this.anchor) {
            this.close();
            return;
        }
        if (this.picker) this.close({ restoreFocus: false, immediate: true });
        const today = todayIso(this.now());
        const locale = this.getLocale();
        this.anchor = anchor || null;
        this.extensionId = extensionId || null;
        this.picker = new DatePicker({
            locale,
            rtl: this.getRtl(),
            weekStart: weekStartFor(locale),
            today,
            selected: isValidIsoDate(date) ? date : today,
            loadMarkers: (start, end) => this._loadMarkers(start, end),
            onSelect: (iso) => this._select(iso),
            onClose: () => this.close(),
            document: this.doc,
        });
        this._mount();
        this.anchor?.setAttribute?.('aria-expanded', 'true');
        this.picker.focus();
        this.doc.addEventListener('pointerdown', this._onDocPointerDown, true);
    }

    /**
     * Close the popup. Focus goes back to the opener (or the input) BEFORE
     * the popup leaves the DOM, and the removal waits a tick so the click
     * that triggered it has finished dispatching — removing a focused or
     * just-clicked subtree can blur the whole window in WebView2.
     */
    close({ restoreFocus = true, immediate = false } = {}) {
        const picker = this.picker;
        const row = this.row;
        if (!picker) return;
        this.picker = null;
        this.row = null;
        this.doc.removeEventListener('pointerdown', this._onDocPointerDown, true);
        if (restoreFocus && picker.containsFocus()) {
            const anchor = this.anchor;
            const target =
                anchor?.isConnected && anchor.offsetParent !== null ? anchor : this.input;
            target?.focus?.({ preventScroll: true });
        }
        this.anchor?.setAttribute?.('aria-expanded', 'false');
        this.anchor = null;
        this.doc.body?.classList.remove(OVER_SUGGESTIONS_CLASS);
        const remove = () => row?.remove();
        if (immediate) remove();
        else setTimeout(remove, 0);
    }

    async _select(iso) {
        const extensionId = this.extensionId;
        this.close({ restoreFocus: true });
        if (!extensionId) return;
        const query = await this.extMgr.resolveDateQuery(extensionId, iso);
        if (!query || !this.input) return;
        this.input.value = query;
        this.input.focus();
        try {
            this.input.setSelectionRange(query.length, query.length);
        } catch {
            /* not a text control in tests */
        }
        // Same path as typing: autogrow + the debounced unified search.
        this.input.dispatchEvent(new Event('input', { bubbles: true }));
    }

    _loadMarkers(start, end) {
        if (!this.extensionId) return Promise.resolve(emptyMarkers('no_provider'));
        return this.markerCache.get(this.extensionId, start, end);
    }

    _handleOutsidePointer(e) {
        const target = e.target;
        if (!this.picker || !target) return;
        if (this.row?.contains(target)) return;
        // The opener toggles itself in its click handler.
        if (this.anchor?.contains?.(target)) return;
        this.close({ restoreFocus: false });
    }

    /**
     * Insert the popup row into the speech bubble directly below the
     * opener's row, and line it up horizontally with the opener.
     */
    _mount() {
        const doc = this.doc;
        const bubble = doc.querySelector('.speech-bubble');
        const row = doc.createElement('div');
        row.className = 'kage-date-popover-row';
        row.appendChild(this.picker.element);
        this.row = row;

        if (!bubble) {
            doc.body.appendChild(row);
            return;
        }
        const suggestions = doc.getElementById('appSuggestions');
        const anchorRow = this.anchor
            ? [...bubble.children].find((c) => c.contains(this.anchor))
            : null;
        // Measure the opener now: hiding the result list below would leave a
        // row-date opener with an empty rect.
        const anchorRect = this.anchor?.isConnected ? this.anchor.getBoundingClientRect() : null;
        if (anchorRow && anchorRow === suggestions) {
            // A date inside the result list: the calendar takes the list's
            // place (picking a day replaces the list anyway). Inserting it in
            // the list would fight the list's own diffing renderer.
            bubble.insertBefore(row, suggestions);
            doc.body.classList.add(OVER_SUGGESTIONS_CLASS);
        } else {
            const after =
                anchorRow || bubble.querySelector('.input-container') || bubble.lastElementChild;
            after.after(row);
        }
        this._align(bubble, anchorRect);
    }

    /** Horizontal alignment with the opener, in physical px (row is LTR). */
    _align(bubble, a) {
        const el = this.picker.element;
        const bubbleRect = bubble.getBoundingClientRect();
        const width = el.offsetWidth;
        const max = Math.max(EDGE_GAP, bubbleRect.width - width - EDGE_GAP);
        let left;
        if (a && a.width > 0) {
            const anchorCenter = a.left + a.width / 2 - bubbleRect.left;
            // Openers on the right half align their right edges; on the left
            // half, their left edges — so the popup hangs under the opener.
            left =
                anchorCenter > bubbleRect.width / 2
                    ? a.right - bubbleRect.left - width
                    : a.left - bubbleRect.left;
        } else {
            left = this.getRtl() ? EDGE_GAP : max;
        }
        el.style.marginLeft = `${Math.round(Math.min(max, Math.max(EDGE_GAP, left)))}px`;
    }
}
