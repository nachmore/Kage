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
                        // User manually set a height — restore to that
                        await this.invoke('resize_floating_window', { height: Math.round(this.userSetHeight) });
                    } else {
                        const inputHeight = inputContainer?.offsetHeight || 0;
                        const baseHeight = Math.round(DEFAULT_HEIGHT * scale);
                        const neededHeight = Math.round((inputHeight + BODY_PADDING) * scale);
                        // Grow beyond default if input area needs it (e.g. attachments)
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
                
                contentHeight += inputContainer?.offsetHeight || 0;
                
                if (suggestionsVisible) {
                    contentHeight += appSuggestions.offsetHeight;
                }

                // Convert logical pixels to physical pixels
                const physicalHeight = Math.round((contentHeight + BODY_PADDING) * scale);

                const maxHeight = await this.getMaxHeight();
                let height = Math.max(Math.round(DEFAULT_HEIGHT * scale), Math.min(maxHeight, physicalHeight));
                
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

    setupResizeHandle(resizeHandle) {
        let startX = 0;
        let startY = 0;
        let startWidth = 0;
        let startHeight = 0;
        let scaleFactor = 1;

        const onMouseMove = async (e) => {
            const maxWidth = Math.floor(window.screen.availWidth * 0.8);
            const maxHeight = await this.getMaxHeight();
            // Mouse deltas are in logical pixels, convert to physical
            const dx = (e.screenX - startX) * scaleFactor;
            const dy = (e.screenY - startY) * scaleFactor;
            const minWidth = Math.floor(516 * scaleFactor);
            const minHeight = Math.floor(DEFAULT_HEIGHT * scaleFactor);
            const newWidth = Math.max(minWidth, Math.min(maxWidth * scaleFactor, startWidth + dx));
            const newHeight = Math.max(minHeight, Math.min(maxHeight * scaleFactor, startHeight + dy));
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
