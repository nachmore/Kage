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

const DEFAULT_HEIGHT = 76; // logical px — collapsed launcher
const MAX_HEIGHT_PERCENT = 0.65; // % of monitor height, auto-grow ceiling
const BODY_PADDING = 16; // 8px top + 8px bottom in floating-base.css

export class WindowManager {
    constructor(invoke) {
        this.invoke = invoke;
        this.userSetHeight = null; // physical px — set by manual resize handle
        this.isResizing = false; // user dragging the corner handle
        this.isDragging = false; // user dragging the ghost
        this._animSeq = 0;
        this._animFrame = null;
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
        const naturalLogical = this._measureNaturalHeight();
        const naturalPhys = Math.round(naturalLogical * scale);
        const minPhys = Math.round(DEFAULT_HEIGHT * scale);
        const maxPhys = await this.getMaxHeight();

        let target;
        if (this.userSetHeight) {
            // Honor user's manual size, but grow past it if content needs more.
            target = Math.max(this.userSetHeight, naturalPhys);
        } else {
            target = Math.max(minPhys, Math.min(maxPhys, naturalPhys));
        }

        // If suggestions list would push us past the cap, let it scroll.
        const appSuggestions = document.getElementById('appSuggestions');
        if (
            appSuggestions?.classList.contains('visible') &&
            naturalPhys > maxPhys &&
            !this.userSetHeight
        ) {
            const overflowLogical = naturalLogical - maxPhys / scale;
            const currentH = appSuggestions.offsetHeight;
            const cappedH = Math.floor(currentH - overflowLogical);
            if (cappedH > 40) appSuggestions.style.maxHeight = cappedH + 'px';
        } else if (appSuggestions) {
            appSuggestions.style.maxHeight = '';
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

        const cleanup = () => {
            this._lastTarget = toOS;
            this._inputAnimating = false;
            for (const item of lockedItems) {
                item.el.style.flex = item.flex;
                item.el.style.height = item.height;
                item.el.style.overflowY = item.overflowY;
            }
            input.style.overflowY = inputPrevOverflowY;
        };

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
            const inputContainer = document.querySelector('.input-container');
            const inputH = inputContainer?.offsetHeight || 44;
            let minContentH = inputH + BODY_PADDING;
            document.querySelectorAll('.extension-bar').forEach((bar) => {
                if (bar.style.display !== 'none') minContentH += bar.offsetHeight;
            });
            const minHeight = Math.max(
                Math.floor(DEFAULT_HEIGHT * scaleFactor),
                Math.floor(minContentH * scaleFactor)
            );
            const newWidth = Math.max(minWidth, Math.min(maxWidth * scaleFactor, startWidth + dx));
            const newHeight = Math.max(minHeight, Math.min(maxHeight, startHeight + dy));
            this.userSetHeight = newHeight;
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
