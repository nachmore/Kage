// Window management and resizing

const DEFAULT_HEIGHT = 76;
const MAX_HEIGHT_PERCENT = 0.5;
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
                        // User manually set a height — but ensure it's at least enough for content
                        const inputHeight = inputContainer?.offsetHeight || 0;
                        let extraHeight = 0;
                        document.querySelectorAll('.timer-bar').forEach(bar => {
                            if (bar.style.display !== 'none') extraHeight += bar.offsetHeight;
                        });
                        const minNeeded = Math.round((inputHeight + extraHeight + BODY_PADDING) * scale);
                        const height = Math.max(this.userSetHeight, minNeeded);
                        await this.invoke('resize_floating_window', { height: Math.round(height) });
                    } else {
                        const inputHeight = inputContainer?.offsetHeight || 0;
                        let extraHeight = 0;
                        document.querySelectorAll('.timer-bar').forEach(bar => {
                            if (bar.style.display !== 'none') extraHeight += bar.offsetHeight;
                        });
                        const baseHeight = Math.round(DEFAULT_HEIGHT * scale);
                        const neededHeight = Math.round((inputHeight + extraHeight + BODY_PADDING) * scale);
                        // Grow beyond default if input area needs it (e.g. attachments, timer)
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

                // Timer/stopwatch bars (persistent, above input container)
                document.querySelectorAll('.timer-bar').forEach(bar => {
                    if (bar.style.display !== 'none') contentHeight += bar.offsetHeight;
                });

                if (suggestionsVisible) {
                    contentHeight += appSuggestions.offsetHeight;
                }

                // Convert logical pixels to physical pixels
                const physicalHeight = Math.round((contentHeight + BODY_PADDING) * scale);

                const maxHeight = await this.getMaxHeight();
                // Auto-grow cap: use the default max, but if the user manually resized
                // larger, fill up to their set height (don't shrink their window)
                const autoMaxHeight = this.userSetHeight
                    ? Math.max(maxHeight, this.userSetHeight)
                    : maxHeight;
                let height = Math.max(Math.round(DEFAULT_HEIGHT * scale), Math.min(autoMaxHeight, physicalHeight));
                
                if (physicalHeight > DEFAULT_HEIGHT * scale && !this.userSetHeight) {
                    this.autoGrowHeight = height;
                }
                
                await this.invoke('resize_floating_window', { height });
            } catch (error) {
                console.error('Error resizing window:', error);
            }
        }, 50);
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
        ghostContainer.addEventListener('mousedown', async (e) => {
            this.isDragging = true;
            try {
                await this.invoke('start_drag_window');
            } catch (error) {
                console.error('Error starting drag:', error);
            }
        });
        
        document.addEventListener('mouseup', () => {
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
            // Dynamic minimum height: at least enough for input container + timer bars
            const inputContainer = document.querySelector('.input-container');
            const inputH = inputContainer?.offsetHeight || 44;
            let minContentH = inputH + BODY_PADDING;
            document.querySelectorAll('.timer-bar').forEach(bar => {
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
