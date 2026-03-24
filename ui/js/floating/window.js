// Window management and resizing

const DEFAULT_HEIGHT = 76;
const MAX_HEIGHT_PERCENT = 0.65;
const BODY_PADDING = 16; // 8px padding on each side

export class WindowManager {
    constructor(invoke) {
        this.invoke = invoke;
        this.userSetHeight = null;
        this.autoGrowHeight = null;
        this.resizeTimeout = null;
        this.isResizing = false; // true while the user is dragging the resize handle
    }

    /**
     * Get the max height dynamically based on the current monitor.
     * Uses the Tauri window API to get the current monitor's size.
     */
    async getMaxHeight() {
        try {
            const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
            const monitor = await appWindow.currentMonitor();
            if (monitor && monitor.size) {
                // Use logical height if available, fall back to physical
                const scaleFactor = monitor.scaleFactor || 1;
                const logicalHeight = monitor.size.height / scaleFactor;
                return Math.floor(logicalHeight * MAX_HEIGHT_PERCENT);
            }
        } catch (e) {
            // fallback
        }
        return Math.floor(window.screen.height * MAX_HEIGHT_PERCENT);
    }

    async resizeWindow() {
        // Don't auto-resize while the user is manually dragging the resize handle
        if (this.isResizing) return;

        if (this.resizeTimeout) {
            clearTimeout(this.resizeTimeout);
        }
        
        this.resizeTimeout = setTimeout(async () => {
            try {
                // Force layout reflow before measuring
                void document.body.offsetHeight;

                const loadingDots = document.getElementById('loadingDots');
                const contentArea = document.getElementById('contentArea');
                const responseText = document.getElementById('responseText');
                const appSuggestions = document.getElementById('appSuggestions');
                const inputContainer = document.querySelector('.input-container');
                
                const loadingVisible = loadingDots?.classList.contains('visible');
                const contentVisible = contentArea?.classList.contains('visible');
                const suggestionsVisible = appSuggestions?.classList.contains('visible');
                
                const nothingExpanded = !loadingVisible && !contentVisible && !suggestionsVisible;

                // When nothing is expanded, snap back to the appropriate base height
                if (nothingExpanded) {
                    const scale = window.devicePixelRatio || 1;
                    if (this.userSetHeight) {
                        const inputHeight = inputContainer?.offsetHeight || 0;
                        let extraHeight = 0;
                        document.querySelectorAll('.extension-bar').forEach(bar => {
                            if (bar.style.display !== 'none') extraHeight += bar.offsetHeight;
                        });
                        const toolbar = document.getElementById('floatingToolbar');
                        if (toolbar && toolbar.style.display !== 'none') extraHeight += toolbar.offsetHeight;
                        const minNeeded = Math.round((inputHeight + extraHeight + BODY_PADDING) * scale);
                        const height = Math.max(this.userSetHeight, minNeeded);
                        await this.invoke('resize_floating_window', { height: Math.round(height) });
                    } else {
                        const inputHeight = inputContainer?.offsetHeight || 0;
                        let extraHeight = 0;
                        document.querySelectorAll('.extension-bar').forEach(bar => {
                            if (bar.style.display !== 'none') extraHeight += bar.offsetHeight;
                        });
                        const toolbar = document.getElementById('floatingToolbar');
                        if (toolbar && toolbar.style.display !== 'none') extraHeight += toolbar.offsetHeight;
                        const baseHeight = Math.round(DEFAULT_HEIGHT * scale);
                        const neededHeight = Math.round((inputHeight + extraHeight + BODY_PADDING) * scale);
                        const height = Math.max(baseHeight, neededHeight);
                        this.autoGrowHeight = height > baseHeight ? height : null;
                        await this.invoke('resize_floating_window', { height });
                    }
                    return;
                }

                // All DOM measurements are in CSS/logical pixels.
                // resize_floating_window expects physical pixels, so we scale at the end.
                const scale = window.devicePixelRatio || 1;

                let contentHeight = 0;

                if (loadingVisible) {
                    contentHeight += loadingDots.offsetHeight;
                }
                
                if (contentVisible) {
                    // Measure response text + any tool/source pills inside content area
                    contentHeight += responseText.scrollHeight + 16; // 16px content area padding top
                    // Floating response action buttons (copy, speak)
                    const floatingActions = document.getElementById('floatingResponseActions');
                    if (floatingActions && floatingActions.style.display !== 'none') {
                        contentHeight += floatingActions.offsetHeight;
                    }
                    const toolSourcesEl = document.getElementById('toolSources');
                    if (toolSourcesEl && toolSourcesEl.offsetHeight > 0) {
                        contentHeight += toolSourcesEl.offsetHeight + 8; // 8px gap
                    }
                    // Account for floating banner if visible
                    const floatingBanner = document.getElementById('floatingBanner');
                    if (floatingBanner && floatingBanner.style.display !== 'none') {
                        contentHeight += floatingBanner.offsetHeight + 8; // 8px margin
                    }
                    contentHeight += 16; // content area padding bottom
                }

                // Compact source bubbles (shown before content area during streaming)
                const compactSources = document.getElementById('toolSourcesCompact');
                if (compactSources && compactSources.offsetHeight > 0) {
                    contentHeight += compactSources.offsetHeight;
                }

                // Response quick action chips
                const responseActions = document.getElementById('responseActionsContainer');
                if (responseActions && responseActions.style.display !== 'none') {
                    contentHeight += responseActions.offsetHeight;
                }
                
                contentHeight += inputContainer?.offsetHeight || 0;

                // Extension bars (persistent, above input container)
                document.querySelectorAll('.extension-bar').forEach(bar => {
                    if (bar.style.display !== 'none') contentHeight += bar.offsetHeight;
                });

                // Floating toolbar (attach file/image)
                const toolbar = document.getElementById('floatingToolbar');
                if (toolbar && toolbar.style.display !== 'none') {
                    contentHeight += toolbar.offsetHeight;
                }

                if (suggestionsVisible) {
                    contentHeight += appSuggestions.offsetHeight;
                }

                // Convert logical pixels to physical pixels
                const physicalHeight = Math.round((contentHeight + BODY_PADDING) * scale);

                const maxHeight = await this.getMaxHeight();
                const autoMaxHeight = this.userSetHeight
                    ? Math.max(maxHeight, this.userSetHeight)
                    : maxHeight;
                let height = Math.max(Math.round(DEFAULT_HEIGHT * scale), Math.min(autoMaxHeight, physicalHeight));

                // If content exceeds max, cap the suggestions area so it scrolls
                if (suggestionsVisible && physicalHeight > autoMaxHeight) {
                    const nonSuggestionsHeight = contentHeight - appSuggestions.offsetHeight;
                    const availableForSuggestions = (autoMaxHeight / scale) - nonSuggestionsHeight - BODY_PADDING;
                    if (availableForSuggestions > 40) {
                        appSuggestions.style.maxHeight = Math.floor(availableForSuggestions) + 'px';
                    }
                } else if (suggestionsVisible) {
                    appSuggestions.style.maxHeight = '';
                }
                
                if (physicalHeight > DEFAULT_HEIGHT * scale && !this.userSetHeight) {
                    this.autoGrowHeight = height;
                }
                
                await this.invoke('resize_floating_window', { height });
                // After resizing, ensure the window is still fully on-screen
                await this._ensureOnScreen();
            } catch (error) {
                console.error('Error resizing window:', error);
            }
        }, 50);
    }

    /** Nudge the window position if it overflows the current monitor bounds. */
    async _ensureOnScreen() {
        try {
            const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
            const pos = await appWindow.outerPosition();
            const size = await appWindow.outerSize();

            // Find the monitor that contains the window center — more reliable than
            // currentMonitor() which can return the wrong monitor on multi-display setups.
            const centerX = pos.x + Math.round(size.width / 2);
            const centerY = pos.y + Math.round(size.height / 2);

            let monX = 0, monY = 0, monW, monH;
            try {
                const monitors = await window.__TAURI__.window.availableMonitors();
                if (monitors && monitors.length > 0) {
                    let best = null;
                    for (const m of monitors) {
                        const mx = m.position.x, my = m.position.y;
                        const mw = m.size.width, mh = m.size.height;
                        if (centerX >= mx && centerX < mx + mw && centerY >= my && centerY < my + mh) {
                            best = m;
                            break;
                        }
                    }
                    // Fallback to currentMonitor if center isn't inside any monitor bounds
                    if (!best) best = await appWindow.currentMonitor();
                    if (best) {
                        monX = best.position.x;
                        monY = best.position.y;
                        monW = best.size.width;
                        const scale = best.scaleFactor || 1;
                        monH = Math.min(best.size.height, Math.round(window.screen.availHeight * scale));
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
            if (x < monX) { x = monX; moved = true; }
            if (y < monY) { y = monY; moved = true; }

            if (moved) {
                await appWindow.setPosition(new window.__TAURI__.window.PhysicalPosition(x, y));
            }
        } catch (e) {
            console.warn('[Window] ensureOnScreen error:', e);
        }
    }

    async resetHeightForNewMessage() {
        try {
            const scale = window.devicePixelRatio || 1;
            let height;
            
            if (this.userSetHeight) {
                height = this.userSetHeight;
            } else if (this.autoGrowHeight) {
                height = Math.round(DEFAULT_HEIGHT * scale);
                this.autoGrowHeight = null;
            } else {
                height = Math.round(DEFAULT_HEIGHT * scale);
            }
            
            await this.invoke('resize_floating_window', { height });
        } catch (error) {
            console.error('Error resetting height:', error);
        }
    }

    setupDragging(ghostContainer) {
        const DRAG_THRESHOLD = 5; // pixels of movement before starting drag
        let startX = 0, startY = 0;
        let pendingDrag = false;
        let moveHandler = null;

        ghostContainer.addEventListener('mousedown', (e) => {
            startX = e.screenX;
            startY = e.screenY;
            pendingDrag = true;

            // Listen for mouse movement to detect drag vs click
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

        // Double-click — fires when two clicks happen without significant movement
        ghostContainer.addEventListener('dblclick', (e) => {
            e.preventDefault();
            pendingDrag = false;
            if (moveHandler) document.removeEventListener('mousemove', moveHandler);
            if (this._onDoubleClick) this._onDoubleClick();
        });

        document.addEventListener('mouseup', () => {
            pendingDrag = false;
            if (moveHandler) document.removeEventListener('mousemove', moveHandler);
            setTimeout(() => { this.isDragging = false; }, 200);
        });
    }

    /** Re-layout when the display scale factor changes (e.g. undocking from a monitor). */
    setupScaleChangeListener() {
        const appWindow = window.__TAURI__?.webviewWindow?.getCurrentWebviewWindow?.();
        if (!appWindow) {
            console.warn('[WindowManager] No appWindow — cannot listen for scale changes');
            return;
        }
        console.log('[WindowManager] Listening for scale factor changes');

        appWindow.onScaleChanged(async ({ payload }) => {
            const { scaleFactor, size } = payload;
            console.log(`[WindowManager] Scale changed: factor=${scaleFactor}, size=${size.width}x${size.height}`);

            // Reset cached heights — they were in the old scale's physical pixels
            this.userSetHeight = null;
            this.autoGrowHeight = null;

            // Recalculate the window size at the new scale
            try {
                const newWidth = Math.round(516 * scaleFactor);
                const newHeight = Math.round(DEFAULT_HEIGHT * scaleFactor);
                console.log(`[WindowManager] Resizing to ${newWidth}x${newHeight} physical px`);
                await this.invoke('resize_floating_window', { width: newWidth, height: newHeight });
                // Give the layout a moment to settle, then auto-size to content
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
            // No max height cap for manual resize — let user go full screen if they want
            let maxHeight;
            try {
                const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
                const monitor = await appWindow.currentMonitor();
                if (monitor && monitor.size) {
                    const sf = monitor.scaleFactor || 1;
                    maxHeight = monitor.size.height; // full monitor height in physical pixels
                } else {
                    maxHeight = window.screen.availHeight * scaleFactor;
                }
            } catch {
                maxHeight = window.screen.availHeight * scaleFactor;
            }
            // Mouse deltas are in logical pixels, convert to physical
            const dx = (e.screenX - startX) * scaleFactor;
            const dy = (e.screenY - startY) * scaleFactor;
            const minWidth = Math.floor(516 * scaleFactor);
            // Dynamic minimum height: at least enough for input container + extension bars
            const inputContainer = document.querySelector('.input-container');
            const inputH = inputContainer?.offsetHeight || 44;
            let minContentH = inputH + BODY_PADDING;
            document.querySelectorAll('.extension-bar').forEach(bar => {
                if (bar.style.display !== 'none') minContentH += bar.offsetHeight;
            });
            const minHeight = Math.max(Math.floor(DEFAULT_HEIGHT * scaleFactor), Math.floor(minContentH * scaleFactor));
            const newWidth = Math.max(minWidth, Math.min(maxWidth * scaleFactor, startWidth + dx));
            const newHeight = Math.max(minHeight, Math.min(maxHeight, startHeight + dy));
            this.userSetHeight = newHeight;
            try {
                await this.invoke('resize_floating_window', { width: Math.round(newWidth), height: Math.round(newHeight) });
            } catch (err) {
                // ignore resize errors during drag
            }
        };

        const onMouseUp = () => {
            this.isResizing = false;
            // Set a brief grace period so the subsequent click event
            // (mouseup → click) doesn't trigger handleOutsideClick
            this._resizeEndedAt = Date.now();
            document.removeEventListener('mousemove', onMouseMove);
            document.removeEventListener('mouseup', onMouseUp);
        };

        resizeHandle.addEventListener('mousedown', async (e) => {
            e.preventDefault();
            e.stopPropagation();
            this.isResizing = true;
            startX = e.screenX;
            startY = e.screenY;
            // Get the actual physical window size from Tauri so we don't jump
            try {
                const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
                const size = await appWindow.innerSize();
                startWidth = size.width;
                startHeight = size.height;
                const monitor = await appWindow.currentMonitor();
                scaleFactor = monitor?.scaleFactor || window.devicePixelRatio || 1;
            } catch (err) {
                startWidth = document.documentElement.offsetWidth;
                startHeight = document.documentElement.offsetHeight;
                scaleFactor = window.devicePixelRatio || 1;
            }
            document.addEventListener('mousemove', onMouseMove);
            document.addEventListener('mouseup', onMouseUp);
        });
    }
}
