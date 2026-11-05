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
 * included — rather than a second, parallel list. The fill is an undoable
 * edit, so a draft it replaced comes back with Ctrl/⌘+Z.
 *
 * Keyboard: the openers aren't in the input's Tab order (Tab cycles the
 * suggestions), so Alt+↓ / ⌥↓ in the input opens the popup — the WAI-ARIA
 * "open the date picker" key; see `openFromKeyboard`.
 */

import { DatePicker } from '../shared/date-picker.js';
import { createMarkerCache, emptyMarkers } from '../shared/date-markers.js';
import { activeLanguage, isRtl, systemFirstWeekday, systemLanguage } from '../shared/i18n.js';
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
     * @param {(locale: string) => number} [o.getWeekStart]  override (tests)
     * @param {number} [o.markerDebounceMs]  see DatePicker
     */
    constructor({
        extensionManager,
        input,
        document: doc,
        now,
        getLocale,
        getRtl,
        getWeekStart,
        markerDebounceMs,
    }) {
        this.extMgr = extensionManager;
        this.input = input;
        this.doc = doc || globalThis.document;
        this.now = now || (() => new Date());
        this.getLocale =
            getLocale || (() => resolvePickerLocale(activeLanguage(), systemLanguage()));
        this.getRtl = getRtl || (() => isRtl());
        // The OS "first day of week" setting wins over the locale's CLDR
        // default — an en-US Mac set to Monday weeks still says `en-US`.
        this.getWeekStart =
            getWeekStart || ((locale) => systemFirstWeekday() ?? weekStartFor(locale));
        this.markerDebounceMs = markerDebounceMs;
        this.picker = null;
        this.row = null;
        this.anchor = null;
        /** The date an extension opener was for (to re-find it after a re-render). */
        this.anchorDate = null;
        this.extensionId = null;
        /** Marker status when no extension can answer (see `open`). */
        this._missingStatus = 'no_provider';
        this.builtInButton = null;
        this.markerCache = createMarkerCache({
            fetch: (extId, start, end) => this.extMgr.fetchDateMarkers(extId, start, end),
        });
        this._onDocPointerDown = (e) => this._handleOutsidePointer(e);
        this._onDocKeyDown = (e) => this._handleDocKeyDown(e);
        this.extMgr?.setDateAffordanceHandler?.((req) => this.open(req), {
            // A widget refresh (the calendar bar re-renders every minute)
            // replaces its opener while the popup may be open; adopt the new
            // button so it toggles the popup and shows as expanded.
            onWired: (el, info) => this._adoptAnchor(el, info),
        });
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
        this.builtInButton = button;
        button.setAttribute('aria-keyshortcuts', 'Alt+ArrowDown');
        button.addEventListener('click', (e) => {
            e.preventDefault();
            e.stopPropagation();
            this._openBuiltIn(button);
        });
    }

    /** Open on today, routed to whichever extension provides calendar data. */
    _openBuiltIn(anchor) {
        this.open({
            anchor,
            date: todayIso(this.now()),
            extensionId: this.extMgr?.getDateProvider?.() || null,
        });
    }

    /**
     * Alt+↓ in the launcher input. Opens from the highlighted result row's
     * date when it has one; otherwise, on an empty input, from the built-in
     * date (or anchorless under the input when the date display is hidden or
     * turned off). Returns false — leaving the key to the textarea, where
     * ⌥↓ moves the caret — when the input has text and no dated row is
     * highlighted. With the popup already open it just refocuses it.
     *
     * @param {{ selectedRow?: Element|null }} [o]
     * @returns {boolean} whether the key was handled
     */
    openFromKeyboard({ selectedRow = null } = {}) {
        if (this.picker) {
            this.picker.focus();
            return true;
        }
        const chip = selectedRow?.querySelector?.('button.kage-date-affordance');
        if (chip) {
            chip.click();
            return true;
        }
        if (this.input?.value) return false;
        const b = this.builtInButton;
        this._openBuiltIn(b?.isConnected && b.offsetParent !== null ? b : null);
        return true;
    }

    /**
     * Open (or, for the same opener, toggle closed) the popup.
     * @param {{ anchor: HTMLElement|null, date?: string, extensionId?: string|null }} req
     */
    open({ anchor, date, extensionId = null }) {
        if (this.picker && anchor && anchor === this.anchor) {
            this.close();
            return;
        }
        if (this.picker) this.close({ restoreFocus: false, immediate: true });
        const today = todayIso(this.now());
        const locale = this.getLocale();
        this.anchor = anchor || null;
        this.anchorDate = anchor && anchor !== this.builtInButton ? date || null : null;
        this.extensionId = extensionId || null;
        // No provider: tell "nothing installed/enabled" apart from "the
        // calendar extension is too old to answer" for the picker's hint.
        this._missingStatus = this.extensionId
            ? 'no_provider'
            : this.extMgr?.missingDateProviderStatus?.() || 'no_provider';
        this.picker = new DatePicker({
            locale,
            rtl: this.getRtl(),
            weekStart: this.getWeekStart(locale),
            today,
            selected: isValidIsoDate(date) ? date : today,
            loadMarkers: (start, end) => this._loadMarkers(start, end),
            peekMarkers: (start, end) => this._peekMarkers(start, end),
            markerDebounceMs: this.markerDebounceMs,
            onSelect: (iso) => this._select(iso),
            onClose: () => this.close(),
            document: this.doc,
        });
        this._mount();
        this.picker.fitWeekdayHeaders();
        this.anchor?.setAttribute?.('aria-expanded', 'true');
        this.picker.focus();
        this.doc.addEventListener('pointerdown', this._onDocPointerDown, true);
        this.doc.addEventListener('keydown', this._onDocKeyDown, true);
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
        this.doc.removeEventListener('keydown', this._onDocKeyDown, true);
        // Unhide the result list first: a result-row opener inside it has no
        // layout (offsetParent null) until then and would lose the focus.
        this.doc.body?.classList.remove(OVER_SUGGESTIONS_CLASS);
        if (restoreFocus && picker.containsFocus()) {
            const anchor = this.anchor;
            const target =
                anchor?.isConnected && anchor.offsetParent !== null ? anchor : this.input;
            target?.focus?.({ preventScroll: true });
        }
        this.anchor?.setAttribute?.('aria-expanded', 'false');
        this.anchor = null;
        this.anchorDate = null;
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
        this._fillInput(query);
    }

    /**
     * Put `query` in the launcher input and run it like typing (autogrow +
     * the debounced unified search, via the `input` event). The widget-bar
     * date stays clickable while the user types, so this may replace a
     * draft: it's done as a native edit (`insertText` over the selection),
     * which keeps the draft on the undo stack — Ctrl/⌘+Z restores it.
     * Assigning `.value` would drop the draft AND the undo history.
     */
    _fillInput(query) {
        const input = this.input;
        input.focus();
        let edited = false;
        try {
            input.select();
            // insertText fires the `input` event itself.
            edited = !!this.doc.execCommand?.('insertText', false, query) && input.value === query;
        } catch {
            edited = false;
        }
        if (!edited) {
            input.value = query;
            input.dispatchEvent(new Event('input', { bubbles: true }));
        }
        try {
            input.setSelectionRange(query.length, query.length);
        } catch {
            /* not a text control in tests */
        }
    }

    _loadMarkers(start, end) {
        if (!this.extensionId) return Promise.resolve(emptyMarkers(this._missingStatus));
        return this.markerCache.get(this.extensionId, start, end);
    }

    /** Synchronous: cached markers for the range, or null. */
    _peekMarkers(start, end) {
        if (!this.extensionId) return emptyMarkers(this._missingStatus);
        return this.markerCache.peek(this.extensionId, start, end);
    }

    _handleOutsidePointer(e) {
        const target = e.target;
        if (!this.picker || !target) return;
        // The picker, not its row: the row spans the bubble's full width, and
        // the empty space beside the popup must count as outside.
        if (this.picker.element.contains(target)) return;
        // The opener toggles itself in its click handler.
        if (this.anchor?.contains?.(target)) return;
        this.close({ restoreFocus: false });
    }

    /**
     * Capture-phase Escape while open: close the popup, whatever has focus.
     * Without this, an Escape with focus outside the picker (e.g. on <body>)
     * would reach the floating window's own Escape handler and hide the
     * launcher with the popup still open.
     */
    _handleDocKeyDown(e) {
        if (e.key !== 'Escape' || e.isComposing || !this.picker) return;
        e.preventDefault();
        e.stopPropagation();
        this.close();
    }

    /** A re-rendered opener for the date we're open on: it's the anchor now. */
    _adoptAnchor(el, { date, extensionId } = {}) {
        if (!this.picker || !this.anchorDate || this.anchor?.isConnected) return;
        if (date !== this.anchorDate || extensionId !== this.extensionId) return;
        this.anchor = el;
        el.setAttribute('aria-expanded', 'true');
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
