// Window management and resizing.
//
// Sizing model:
//   1. CSS computes the natural layout. The OS window's required height is
//      the sum of the .speech-bubble's flow children (loading dots, content
//      area, extension bars, input, toolbar, suggestions, …).
//   2. A ResizeObserver watches every flow child (and any added later via
//      MutationObserver). Whenever any of them changes size, we recompute
//      and animate the OS window to the new natural height.
//   3. Animation reads the *actual* current OS window height each time it
//      starts, so it never animates from a stale cached value. This is what
//      caused the visible "jump up, then back down" on every chunk render
//      after a manual drag / new-message reset / DPI change reset the OS
//      window without updating the cached _currentHeight.
//
// The public API (resizeWindow, resetHeightForNewMessage, userSetHeight) is
// preserved so the many explicit call sites in app.js / suggestions / timers
// keep working as nudges — they coalesce with observer-driven reflows.

import { BODY_PADDING, DEFAULT_HEIGHT, MAX_HEIGHT_PERCENT } from './window-sizing-config.js';

export class WindowManager {
    constructor(invoke) {
        this.invoke = invoke;
        this.userSetHeight = null; // physical px — set by manual resize handle
        this.isResizing = false; // user dragging the corner handle
        this.isDragging = false; // user dragging the ghost
        this._animSeq = 0;
        this._animFrame = null;
        this._inputAnimCleanup = null; // pending animateInputResize cleanup, if mid-flight
        this._scheduled = false;
        this._suspended = false; // pause auto-resize (e.g. permission modal)
        this._lastTarget = 0; // last target we actually requested
        this._observer = null;
        this._mutationObserver = null;
    }

    /**
     * Sum the natural height of the bubble's in-flow children.
     *
     * A naive `child.scrollHeight` works for flex-grow:0 elements but breaks
     * for `flex: 1; overflow-y: auto` elements like `.content-area`: when the
     * bubble is taller than the content, the element gets stretched, and
     * `scrollHeight` returns max(content, clientHeight) — i.e. the stretched
     * height. Using that as the target creates a runaway loop:
     *   type → input grows → window grows → content-area stretches further
     *   → scrollHeight grows → window grows → ...
     * For flex-stretching elements we recurse into the children + padding,
     * floored by the element's own min-height.
     */
    _measureNaturalHeight() {
        const bubble = document.querySelector('.speech-bubble');
        if (!bubble) return DEFAULT_HEIGHT;
        return this._measureChildSum(bubble) + BODY_PADDING;
    }

    _measureChildSum(parent) {
        let sum = 0;
        for (const child of parent.children) {
            sum += this._measureFlow(child);
        }
        return sum;
    }

    /**
     * True when a real response is showing (as opposed to an empty or
     * banner-only content-area). Governs whether the resize floor reserves
     * a line of response space.
     *
     * Keys off the content-area being `.visible` AND carrying response text —
     * NOT `responseText.textContent` alone. `resetUI()` (clear-ux, hide/show)
     * removes `.visible` but leaves the last answer's text in the DOM, so a
     * textContent-only check stayed true after a reset and the floor kept
     * reserving a response line — the empty launcher then refused to shrink
     * back to input height. Banner-only mode isn't a response either.
     */
    _hasResponseContent() {
        const ca = document.getElementById('contentArea');
        if (!ca?.classList.contains('visible') || ca.classList.contains('banner-only')) {
            return false;
        }
        const rt = document.getElementById('responseText');
        return !!rt && rt.textContent.trim().length > 0;
    }

    /**
     * Logical-px minimum the manual resize handle should allow, per the
     * layout: the fixed chrome (input box + every floating bar — offline,
     * extension/status bars, toolbar, suggestions, loading dots) plus, when
     * a response is showing, one input-box-height of response content (with
     * the content-area's own padding so the reserved line isn't cramped).
     *
     * An empty / banner-only content-area reserves nothing, so after `clear`
     * the floor is just input + bars. A response present adds exactly one
     * line — content taller than that scrolls inside `.content-area`
     * (overflow-y:auto), so the window can still be dragged down close to
     * the input while the answer stays scrollable.
     *
     * Layout-dependent (reads offsetHeight / computed padding) so, like
     * `_measureNaturalHeight`, it isn't unit-tested — jsdom has no layout.
     * The scale + launcher-minimum step is factored into the pure
     * `_resizeFloor` below.
     *
     * @returns {number} logical-px floor for the drag
     */
    _measureResizeFloorLogical() {
        const bubble = document.querySelector('.speech-bubble');
        if (!bubble) return DEFAULT_HEIGHT;
        const contentArea = document.getElementById('contentArea');

        // Fixed chrome: every bubble flow-child except the response area.
        let sum = 0;
        for (const child of bubble.children) {
            if (child === contentArea) continue;
            sum += this._measureFlow(child);
        }

        // Reserve one input-box-height of response when a response is showing.
        if (contentArea && this._hasResponseContent()) {
            const input = document.getElementById('promptInput');
            const inputH = input ? input.offsetHeight : 0;
            const cs = getComputedStyle(contentArea);
            const pad = (parseFloat(cs.paddingTop) || 0) + (parseFloat(cs.paddingBottom) || 0);
            sum += inputH + pad;
        }

        return sum + BODY_PADDING;
    }

    /**
     * Convert a logical-px floor to physical px, never below the collapsed
     * launcher height. Pure arithmetic, extracted for unit testing (jsdom
     * has no real layout).
     *
     * @param {number} floorLogical - logical-px floor (see _measureResizeFloorLogical)
     * @param {number} scaleFactor - device pixel ratio
     * @returns {number} physical-px floor for the drag
     */
    _resizeFloor(floorLogical, scaleFactor) {
        return Math.max(
            Math.floor(DEFAULT_HEIGHT * scaleFactor),
            Math.floor(floorLogical * scaleFactor)
        );
    }

    /**
     * How tall the suggestions list may be when the bubble's natural
     * height overflows the screen ceiling. `uncappedH` is the list's
     * un-capped rendered height, `overflowLogical` the logical pixels by
     * which the whole bubble exceeds the ceiling. Returns the capped
     * height, or null when the list would become too small to be usable
     * (< 40px) — in that case we leave it uncapped and let the window
     * hit the ceiling (the list scrolls inside the bubble instead).
     *
     * MUST be a pure function of the un-capped layout: deriving it from
     * an already-capped `offsetHeight` made the cap ratchet down (or
     * oscillate against the clear-cap branch) on every observer pass.
     * Pure arithmetic, extracted for unit testing.
     */
    _suggestionCap(uncappedH, overflowLogical) {
        const capped = Math.floor(uncappedH - overflowLogical);
        return capped > 40 ? capped : null;
    }

    /**
     * Resolve the physical-px window target from the measured content height.
     *
     * Two regimes:
     *   - No manual size: auto-fit the content, floored at the collapsed
     *     launcher minimum and capped at `maxPhys` (~65% of the monitor).
     *     Past the cap the response scrolls inside `.content-area`
     *     (overflow-y:auto).
     *   - Manual size set (`userSetHeight`): honour it as the budget for
     *     everything EXCEPT the suggestions dropdown, then add the live
     *     dropdown height on top (still capped at `maxPhys`). The dropdown is
     *     transient typeahead — it must expand the window as the user types
     *     and collapse back when the list closes, rather than eat into the
     *     size they chose for the response. Response content still scrolls
     *     within the budget; we do NOT grow to fit it (that snap-to-content
     *     was what jumped the window to full-screen the instant the user
     *     tried to shrink below a long answer). `userSetHeight` is captured
     *     dropdown-free by the resize handle, so adding `suggestionsPhys`
     *     here can't double-count.
     *
     * Pure arithmetic, extracted for unit testing (jsdom has no real layout).
     *
     * @param {number} naturalPhys - physical-px measured content height
     * @param {number} minPhys - physical-px collapsed launcher minimum
     * @param {number} maxPhys - physical-px screen ceiling
     * @param {number} suggestionsPhys - physical-px of the visible dropdown (0 if hidden)
     * @returns {number} physical-px window target
     */
    _targetHeight(naturalPhys, minPhys, maxPhys, suggestionsPhys = 0) {
        if (this.userSetHeight) {
            return Math.max(minPhys, Math.min(maxPhys, this.userSetHeight + suggestionsPhys));
        }
        return Math.max(minPhys, Math.min(maxPhys, naturalPhys));
    }

    /**
     * Physical-px height the visible suggestions dropdown contributes (0 when
     * hidden). Callers add this to `userSetHeight` so the transient typeahead
     * list expands/collapses the window on top of the user's chosen size, and
     * the resize handle subtracts it when capturing a manual resize so the
     * stored budget stays dropdown-free.
     *
     * @param {number} scale - device pixel ratio
     * @returns {number} physical-px dropdown height
     */
    _suggestionsPhys(scale) {
        const el = document.getElementById('appSuggestions');
        if (!el?.classList.contains('visible')) return 0;
        return Math.round(this._measureFlow(el) * scale);
    }

    _measureFlow(el) {
        const cs = getComputedStyle(el);
        if (cs.display === 'none') return 0;
        if (cs.position === 'absolute' || cs.position === 'fixed') return 0;

        const mt = parseFloat(cs.marginTop) || 0;
        const mb = parseFloat(cs.marginBottom) || 0;

        const flexGrow = parseFloat(cs.flexGrow) || 0;
        if (flexGrow > 0) {
            const pt = parseFloat(cs.paddingTop) || 0;
            const pb = parseFloat(cs.paddingBottom) || 0;
            const minH = parseFloat(cs.minHeight) || 0;
            const inner = this._measureChildSum(el);
            return mt + mb + Math.max(minH, pt + pb + inner);
        }

        // For scrollable containers with max-height, scrollHeight reports the
        // full content height (including overflow). Use offsetHeight which
        // respects the max-height cap and gives the actual rendered size.
        const overflow = cs.overflowY;
        if ((overflow === 'auto' || overflow === 'scroll') && cs.maxHeight !== 'none') {
            return el.offsetHeight + mt + mb;
        }

        return el.scrollHeight + mt + mb;
    }

    async getMaxHeight() {
        try {
            const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
            const monitor = await appWindow.currentMonitor();
            if (monitor?.size) {
                return Math.floor(monitor.size.height * MAX_HEIGHT_PERCENT);
            }
        } catch {}
        const scale = window.devicePixelRatio || 1;
        return Math.floor(window.screen.height * scale * MAX_HEIGHT_PERCENT);
    }

    /** Compute the target physical height and animate. Single source of truth. */
    async _applyNaturalHeight() {
        if (this.isResizing || this._suspended) return;

        // Don't auto-resize while the permission modal is open — it manages its own size
        const permModal = document.getElementById('permissionModal');
        if (permModal && permModal.style.display !== 'none') return;

        const scale = window.devicePixelRatio || 1;
        const minPhys = Math.round(DEFAULT_HEIGHT * scale);
        // Resolve the ceiling BEFORE touching the DOM: the block below must
        // run without an intervening await, or a frame can paint mid-flip.
        const maxPhys = await this.getMaxHeight();

        // Measure with our own inline suggestions cap removed. The cap set
        // below shrinks the list; if the next pass measured that shrunken
        // layout it would decide "no overflow", CLEAR the cap, grow the
        // list back, re-trigger the observer, re-cap — an endless visible
        // jitter whenever a tall response + suggestions together exceed
        // the screen ceiling (e.g. typing `focus` right after a focus AI
        // summary). Measuring uncapped makes the overflow decision
        // deterministic: the same inputs re-derive the same cap, and
        // re-applying an identical cap is a layout no-op, so the observer
        // settles after one pass. Clear + measure + re-cap all happen
        // synchronously within this task — no frame paints uncapped.
        const appSuggestions = document.getElementById('appSuggestions');
        if (appSuggestions) appSuggestions.style.maxHeight = '';
        const naturalLogical = this._measureNaturalHeight();
        const naturalPhys = Math.round(naturalLogical * scale);

        // Sentinel: catch a stuck-large window. If we're measuring a tall
        // natural height but the content area is effectively empty (no real
        // response/suggestions rendered), something is inflating the measure
        // — historically an orphaned inline `height`/`flex:none` lock left by
        // an interrupted animateInputResize. Warn with the offending inline
        // styles so it's diagnosable from app.jsonl rather than a silent
        // "why is the launcher huge" report.
        if (naturalLogical > DEFAULT_HEIGHT * 3) {
            const ca = document.getElementById('contentArea');
            const rt = document.getElementById('responseText');
            const suggVisible = !!appSuggestions?.classList.contains('visible');
            const hasContent = !!rt?.textContent.trim() || suggVisible;
            if (ca && !hasContent && (ca.style.height || ca.style.flex === 'none')) {
                console.warn(
                    `[WindowManager] tall natural height (${Math.round(naturalLogical)}px) with empty ` +
                        `content — leaked contentArea lock? flex=${ca.style.flex || '(unset)'} ` +
                        `height=${ca.style.height || '(unset)'} inputAnimating=${this._inputAnimating}`
                );
            }
        }

        // The dropdown expands the window ON TOP OF a user-set budget (it's
        // transient typeahead), so it's added to userSetHeight rather than
        // eating into it. Measured here (maxHeight cleared above) so it's the
        // uncapped list height, consistent with naturalPhys.
        const suggestionsPhys = this._suggestionsPhys(scale);
        let target = this._targetHeight(naturalPhys, minPhys, maxPhys, suggestionsPhys);

        // If suggestions list would push us past the cap, let it scroll.
        if (
            appSuggestions?.classList.contains('visible') &&
            naturalPhys > maxPhys &&
            !this.userSetHeight
        ) {
            const overflowLogical = naturalLogical - maxPhys / scale;
            const uncappedH = appSuggestions.offsetHeight;
            const cappedH = this._suggestionCap(uncappedH, overflowLogical);
            if (cappedH !== null) {
                appSuggestions.style.maxHeight = cappedH + 'px';
                // The window target must account for the list we just
                // shrank — naturalPhys measured the uncapped layout.
                target = Math.min(target, maxPhys);
            }
        }

        if (Math.abs(target - this._lastTarget) < 2) return;
        this._lastTarget = target;

        await this._animateTo(target);
        await this._ensureOnScreen();
    }

    /**
     * Animate the OS window height to `target`. Reads the *actual* current
     * window height each invocation — never relies on a cached value, which
     * was the source of the visual glitch after manual resize / DPI / reset
     * paths bypassed the cache.
     *
     * Growing always snaps: while the content-area is `flex: 1`, animating
     * the OS window up over time leaves a window of frames where the input
     * has already grown but the OS hasn't, so the content-area is squeezed
     * and overflows — visible as a "jump up, scrollbar flash, jump back"
     * during typing. Snapping eliminates that.
     *
     * Shrinking can animate freely: content already fits, so there's no
     * squeeze. We use it for the response → collapsed transition.
     *
     *   - diff < 4 px: skip
     *   - target > from: snap (avoid squeeze)
     *   - target < from: ease-out cubic over 80–220 ms scaled by magnitude
     */
    async _animateTo(target) {
        if (this._animFrame) {
            cancelAnimationFrame(this._animFrame);
            this._animFrame = null;
        }

        let from = target;
        try {
            const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
            const size = await appWindow.innerSize();
            from = size.height;
        } catch {}

        const diff = Math.abs(target - from);
        if (diff < 4) return;

        if (target >= from) {
            try {
                await this.invoke('resize_floating_window', { height: target });
            } catch {}
            return;
        }

        const duration = Math.min(220, 80 + diff * 0.25);
        const start = performance.now();
        const me = ++this._animSeq;

        return new Promise((resolve) => {
            const step = async (now) => {
                if (me !== this._animSeq) {
                    resolve();
                    return;
                }
                const t = Math.min((now - start) / duration, 1);
                const eased = 1 - (1 - t) ** 3;
                const h = Math.round(from + (target - from) * eased);
                try {
                    await this.invoke('resize_floating_window', { height: h });
                } catch {}
                if (t < 1 && me === this._animSeq) {
                    this._animFrame = requestAnimationFrame(step);
                } else {
                    this._animFrame = null;
                    resolve();
                }
            };
            this._animFrame = requestAnimationFrame(step);
        });
    }

    /**
     * Animate textarea height and OS window size in lockstep over 80ms.
     * Used by the input handler when a line wraps/unwraps.
     *
     * Why lockstep, not snap+IPC: snapping the textarea height instantly
     * while the OS window catches up async makes the flex `.content-area`
     * absorb the delta for one paint — every element between content-area
     * and the textarea bounces (up if growing, down if shrinking) and then
     * returns. With both animations on the same linear curve, the math
     * `content-area = bubble - input - others` is invariant: content-area
     * stays constant, nothing bounces.
     *
     * Observer-driven resizes are gated by `_inputAnimating` so they don't
     * fight the in-flight animation.
     */
    async animateInputResize(input, fromInput, toInput) {
        const delta = toInput - fromInput;
        if (Math.abs(delta) < 1) {
            input.style.height = toInput + 'px';
            return;
        }

        if (this._animFrame) {
            cancelAnimationFrame(this._animFrame);
            this._animFrame = null;
        }
        // A previous input animation may still be mid-flight (two line-wraps
        // within the 80ms window). Cancelling its rAF above skips its cleanup,
        // so restore its locks now — otherwise the capture below would read the
        // still-locked `flex:none; height:<px>` as this animation's "originals"
        // and restore to them, leaking the lock forever.
        if (this._inputAnimCleanup) this._inputAnimCleanup();
        const me = ++this._animSeq;
        this._inputAnimating = true;

        // Lock content-area + suggestions at their current height so flex
        // redistribution can't squeeze them when input grows. Without this,
        // every input wrap leaves a 1-frame gap where the OS window IPC has
        // not landed but the textarea has grown — content-area absorbs the
        // delta, its content overflows, scrollbar flashes, response shifts.
        // With the lock, bubble's natural height = locked + input + others,
        // so it can only fit by growing the OS window — which we do in
        // lockstep below.
        const contentArea = document.getElementById('contentArea');
        const suggestions = document.getElementById('appSuggestions');
        const lockedItems = [];
        const tryLock = (el) => {
            if (!el) return;
            const cs = getComputedStyle(el);
            if (cs.display === 'none') return;
            lockedItems.push({
                el,
                flex: el.style.flex || '',
                height: el.style.height || '',
                overflowY: el.style.overflowY || '',
            });
            el.style.flex = 'none';
            el.style.height = el.offsetHeight + 'px';
            el.style.overflowY = 'hidden';
        };
        tryLock(contentArea);
        tryLock(suggestions);

        // The textarea's own scrollbar flashes during the animation: its
        // content reflows to the wrapped layout instantly, but we're
        // interpolating its `height` over 80ms — so for ~half the animation
        // it's shorter than its content. Mask it for the duration.
        const inputPrevOverflowY = input.style.overflowY || '';
        input.style.overflowY = 'hidden';

        const scale = window.devicePixelRatio || 1;
        const fromOS = Math.round(window.innerHeight * scale);
        const toOS = fromOS + Math.round(delta * scale);

        const duration = 80;
        const start = performance.now();

        // Idempotent — may be invoked either by the animation finishing/aborting
        // OR out-of-band by suspendAutoResize() when a permission modal opens
        // mid-animation and cancels our rAF loop. Without the out-of-band path,
        // the inline `height`/`flex:none` lock on content-area would be orphaned:
        // an empty `flex:none` element with an explicit height still reports that
        // height via scrollHeight, so _measureNaturalHeight would stay stuck at
        // the large value forever — the window freezes large with no content.
        let cleanedUp = false;
        const cleanup = () => {
            if (cleanedUp) return;
            cleanedUp = true;
            this._inputAnimCleanup = null;
            this._lastTarget = toOS;
            this._inputAnimating = false;
            for (const item of lockedItems) {
                item.el.style.flex = item.flex;
                item.el.style.height = item.height;
                item.el.style.overflowY = item.overflowY;
            }
            input.style.overflowY = inputPrevOverflowY;
        };
        this._inputAnimCleanup = cleanup;

        return new Promise((resolve) => {
            const step = (now) => {
                if (me !== this._animSeq) {
                    cleanup();
                    resolve();
                    return;
                }
                const t = Math.min((now - start) / duration, 1);
                input.style.height = fromInput + delta * t + 'px';
                const osH = Math.round(fromOS + (toOS - fromOS) * t);
                this.invoke('resize_floating_window', { height: osH }).catch(() => {});
                if (t < 1 && me === this._animSeq) {
                    this._animFrame = requestAnimationFrame(step);
                } else {
                    this._animFrame = null;
                    cleanup();
                    resolve();
                }
            };
            this._animFrame = requestAnimationFrame(step);
        });
    }

    /**
     * Public nudge — tells the manager "DOM may have changed, recompute".
     * The ResizeObserver already covers most cases, but legacy call sites
     * still call this and it's free to honor them: rAF-coalesced, so 10
     * callers in one frame still produce one resize.
     */
    resizeWindow() {
        if (this._scheduled) return;
        if (this._inputAnimating) return; // animateInputResize is the source of truth
        this._scheduled = true;
        requestAnimationFrame(() => {
            this._scheduled = false;
            if (this._inputAnimating) return;
            this._applyNaturalHeight().catch((e) => console.warn('[WindowManager] resize:', e));
        });
    }

    /** Forget the cached target so the next observer fire re-animates from the OS height. */
    async resetHeightForNewMessage() {
        const permModal = document.getElementById('permissionModal');
        if (permModal && permModal.style.display !== 'none') return;
        this._lastTarget = 0;
        this.resizeWindow();
    }

    /** Suspend automatic resizing — used by the permission modal which sizes itself. */
    suspendAutoResize() {
        this._suspended = true;
        if (this._animFrame) {
            cancelAnimationFrame(this._animFrame);
            this._animFrame = null;
        }
        this._animSeq++; // invalidate any in-flight step()
        // If animateInputResize was mid-flight, cancelling its rAF loop above
        // means its own cleanup() never runs — restore the content-area /
        // suggestions inline locks now, or they'd freeze the window at the
        // in-progress size (empty content, huge bubble) until a full reset.
        if (this._inputAnimCleanup) this._inputAnimCleanup();
    }

    resumeAutoResize() {
        this._suspended = false;
        this._lastTarget = 0; // force a recompute
        this.resizeWindow();
    }

    /**
     * Watch every flow child of the bubble. Any size change triggers a
     * coalesced resize. New children added later (extension bars, source
     * chip rows, banners) are picked up by the MutationObserver.
     */
    setupObserver() {
        const bubble = document.querySelector('.speech-bubble');
        if (!bubble) {
            console.warn('[WindowManager] .speech-bubble not found — observer not installed');
            return;
        }

        const ro = new ResizeObserver(() => this.resizeWindow());
        for (const child of bubble.children) {
            const cs = getComputedStyle(child);
            if (cs.position === 'absolute' || cs.position === 'fixed') continue;
            ro.observe(child);
        }
        this._observer = ro;

        const mo = new MutationObserver((muts) => {
            for (const m of muts) {
                for (const node of m.addedNodes) {
                    if (node.nodeType !== 1) continue;
                    const cs = getComputedStyle(node);
                    if (cs.position === 'absolute' || cs.position === 'fixed') continue;
                    ro.observe(node);
                }
            }
            this.resizeWindow();
        });
        mo.observe(bubble, { childList: true });
        this._mutationObserver = mo;
    }

    /** Nudge the window position if it overflows the current monitor bounds. */
    async _ensureOnScreen() {
        try {
            const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
            const pos = await appWindow.outerPosition();
            const size = await appWindow.outerSize();

            const centerX = pos.x + Math.round(size.width / 2);
            const centerY = pos.y + Math.round(size.height / 2);

            let monX = 0,
                monY = 0,
                monW,
                monH;
            try {
                const monitors = await window.__TAURI__.window.availableMonitors();
                if (monitors && monitors.length > 0) {
                    let best = null;
                    for (const m of monitors) {
                        const mx = m.position.x,
                            my = m.position.y;
                        const mw = m.size.width,
                            mh = m.size.height;
                        if (
                            centerX >= mx &&
                            centerX < mx + mw &&
                            centerY >= my &&
                            centerY < my + mh
                        ) {
                            best = m;
                            break;
                        }
                    }
                    if (!best) best = await appWindow.currentMonitor();
                    if (best) {
                        monX = best.position.x;
                        monY = best.position.y;
                        monW = best.size.width;
                        const scale = best.scaleFactor || 1;
                        monH = Math.min(
                            best.size.height,
                            Math.round(window.screen.availHeight * scale)
                        );
                    }
                }
            } catch {}
            if (!monW || !monH) {
                const scale = window.devicePixelRatio || 1;
                monW = Math.round(window.screen.availWidth * scale);
                monH = Math.round(window.screen.availHeight * scale);
            }

            let x = pos.x;
            let y = pos.y;
            let moved = false;

            if (y + size.height > monY + monH) {
                y = monY + monH - size.height;
                moved = true;
            }
            if (x + size.width > monX + monW) {
                x = monX + monW - size.width;
                moved = true;
            }
            if (x < monX) {
                x = monX;
                moved = true;
            }
            if (y < monY) {
                y = monY;
                moved = true;
            }

            if (moved) {
                await appWindow.setPosition(new window.__TAURI__.window.PhysicalPosition(x, y));
            }
        } catch (e) {
            console.warn('[Window] ensureOnScreen error:', e);
        }
    }

    setupDragging(ghostContainer) {
        const DRAG_THRESHOLD = 5;
        let startX = 0,
            startY = 0;
        let pendingDrag = false;
        let moveHandler = null;

        ghostContainer.addEventListener('mousedown', (e) => {
            startX = e.screenX;
            startY = e.screenY;
            pendingDrag = true;

            moveHandler = async (me) => {
                if (!pendingDrag) return;
                const dx = Math.abs(me.screenX - startX);
                const dy = Math.abs(me.screenY - startY);
                if (dx > DRAG_THRESHOLD || dy > DRAG_THRESHOLD) {
                    pendingDrag = false;
                    document.removeEventListener('mousemove', moveHandler);
                    this.isDragging = true;
                    try {
                        await this.invoke('start_drag_window');
                    } catch (error) {
                        console.error('Error starting drag:', error);
                    }
                }
            };
            document.addEventListener('mousemove', moveHandler);
        });

        ghostContainer.addEventListener('dblclick', (e) => {
            e.preventDefault();
            pendingDrag = false;
            if (moveHandler) document.removeEventListener('mousemove', moveHandler);
            if (this._onDoubleClick) this._onDoubleClick();
        });

        document.addEventListener('mouseup', () => {
            pendingDrag = false;
            if (moveHandler) document.removeEventListener('mousemove', moveHandler);
            setTimeout(() => {
                this.isDragging = false;
            }, 200);
        });
    }

    /** Re-layout when the display scale factor changes (e.g. undocking from a monitor). */
    setupScaleChangeListener() {
        const appWindow = window.__TAURI__?.webviewWindow?.getCurrentWebviewWindow?.();
        if (!appWindow) return;

        appWindow.onScaleChanged(async ({ payload }) => {
            const { scaleFactor } = payload;
            console.log(`[WindowManager] Scale changed: factor=${scaleFactor}`);
            this.userSetHeight = null;
            this._lastTarget = 0;
            try {
                const newWidth = Math.round(570 * scaleFactor);
                const newHeight = Math.round(DEFAULT_HEIGHT * scaleFactor);
                await this.invoke('resize_floating_window', { width: newWidth, height: newHeight });
                setTimeout(() => this.resizeWindow(), 200);
            } catch (e) {
                console.warn('[WindowManager] DPI resize failed:', e);
            }
        });
    }

    setupResizeHandle(resizeHandle) {
        let startX = 0;
        let startY = 0;
        let startWidth = 0;
        let startHeight = 0;
        let scaleFactor = 1;

        const onMouseMove = async (e) => {
            const maxWidth = Math.floor(window.screen.availWidth * 0.95);
            let maxHeight;
            try {
                const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
                const monitor = await appWindow.currentMonitor();
                if (monitor?.size) {
                    maxHeight = monitor.size.height;
                } else {
                    maxHeight = window.screen.availHeight * scaleFactor;
                }
            } catch {
                maxHeight = window.screen.availHeight * scaleFactor;
            }
            const dx = (e.screenX - startX) * scaleFactor;
            const dy = (e.screenY - startY) * scaleFactor;
            const minWidth = Math.floor(570 * scaleFactor);
            // Floor the drag at [input + floating bars] plus one line of
            // response when a response is showing. Content taller than that
            // scrolls inside .content-area, and _targetHeight honours
            // userSetHeight exactly, so there's no clipping or snap-back.
            const minHeight = this._resizeFloor(this._measureResizeFloorLogical(), scaleFactor);
            const newWidth = Math.max(minWidth, Math.min(maxWidth * scaleFactor, startWidth + dx));
            const newHeight = Math.max(minHeight, Math.min(maxHeight, startHeight + dy));
            // Store the budget WITHOUT the dropdown: _targetHeight adds the
            // live dropdown height back on top of userSetHeight, so if the
            // list is open mid-resize we'd otherwise bake it into the budget
            // and double-count it (window would jump taller on the next
            // reflow). _lastTarget stays at the physical height shown, which
            // equals userSetHeight + dropdown — so the observer doesn't fight.
            this.userSetHeight = newHeight - this._suggestionsPhys(scaleFactor);
            this._lastTarget = newHeight; // observer would otherwise fight us
            try {
                await this.invoke('resize_floating_window', {
                    width: Math.round(newWidth),
                    height: Math.round(newHeight),
                });
            } catch {}
        };

        const onMouseUp = async () => {
            this.isResizing = false;
            this._resizeEndedAt = Date.now();
            document.removeEventListener('mousemove', onMouseMove);
            document.removeEventListener('mouseup', onMouseUp);
            try {
                const config = await this.invoke('get_config');
                if (config.ui?.remember_launcher_size) {
                    const scale = window.devicePixelRatio || 1;
                    const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
                    const size = await appWindow.innerSize();
                    config.ui.launcher_width = Math.round(size.width / scale);
                    config.ui.launcher_height = Math.round(size.height / scale);
                    await this.invoke('save_config', { config });
                }
            } catch {}
        };

        resizeHandle.addEventListener('mousedown', async (e) => {
            e.preventDefault();
            e.stopPropagation();
            this.isResizing = true;
            startX = e.screenX;
            startY = e.screenY;
            try {
                const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
                const size = await appWindow.innerSize();
                startWidth = size.width;
                startHeight = size.height;
                const monitor = await appWindow.currentMonitor();
                scaleFactor = monitor?.scaleFactor || window.devicePixelRatio || 1;
            } catch {
                startWidth = document.documentElement.offsetWidth;
                startHeight = document.documentElement.offsetHeight;
                scaleFactor = window.devicePixelRatio || 1;
            }
            document.addEventListener('mousemove', onMouseMove);
            document.addEventListener('mouseup', onMouseUp);
        });
    }
}
