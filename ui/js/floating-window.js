// Window management and resizing

const DEFAULT_HEIGHT = 76;
const MAX_HEIGHT_PERCENT = 0.5;
const MAX_INPUT_ONLY_HEIGHT = 200; // Cap for when only the input is visible
const BODY_PADDING = 16; // 8px padding on each side

export class WindowManager {
    constructor(invoke) {
        this.invoke = invoke;
        this.userSetHeight = null;
        this.autoGrowHeight = null;
        this.resizeTimeout = null;
        this.maxHeight = this.calculateMaxHeight();
    }

    calculateMaxHeight() {
        // Calculate 50% of screen height
        const screenHeight = window.screen.height;
        const maxHeight = Math.floor(screenHeight * MAX_HEIGHT_PERCENT);
        console.log('Screen height:', screenHeight, 'Max height (50%):', maxHeight);
        return maxHeight;
    }

    async resizeWindow() {
        if (this.resizeTimeout) {
            clearTimeout(this.resizeTimeout);
        }
        
        this.resizeTimeout = setTimeout(async () => {
            try {
                let contentHeight = 0;
                
                const loadingDots = document.getElementById('loadingDots');
                const contentArea = document.getElementById('contentArea');
                const responseText = document.getElementById('responseText');
                const appSuggestions = document.getElementById('appSuggestions');
                const inputContainer = document.querySelector('.input-container');
                
                const loadingVisible = loadingDots?.classList.contains('visible');
                const contentVisible = contentArea?.classList.contains('visible');
                const suggestionsVisible = appSuggestions?.classList.contains('visible');
                
                if (loadingVisible) {
                    contentHeight += loadingDots.offsetHeight;
                }
                
                if (contentVisible) {
                    contentHeight += responseText.scrollHeight + 32;
                }
                
                const inputHeight = inputContainer?.offsetHeight || 0;
                contentHeight += inputHeight;
                
                if (suggestionsVisible) {
                    contentHeight += appSuggestions.offsetHeight;
                }
                
                const nothingExpanded = !loadingVisible && !contentVisible && !suggestionsVisible;

                // Don't resize if nothing is expanded and input fits in default height
                if (nothingExpanded && contentHeight + BODY_PADDING <= DEFAULT_HEIGHT) {
                    return;
                }

                let maxForState = nothingExpanded ? MAX_INPUT_ONLY_HEIGHT : this.maxHeight;
                let height = Math.max(DEFAULT_HEIGHT, Math.min(maxForState, contentHeight + BODY_PADDING));
                
                if (contentHeight > DEFAULT_HEIGHT && !this.userSetHeight) {
                    this.autoGrowHeight = height;
                }

                const currentSize = await window.__TAURI__.webviewWindow.getCurrentWebviewWindow().innerSize();
                
                console.log('[RESIZE DEBUG]', {
                    contentHeight,
                    inputHeight,
                    loadingVisible,
                    contentVisible,
                    suggestionsVisible,
                    calculatedHeight: height,
                    currentWindowHeight: currentSize.height,
                    nothingExpanded
                });
                
                console.log('Resizing window height:', { contentHeight, height, maxHeight: this.maxHeight });
                // Only pass height, width will remain unchanged
                await this.invoke('resize_floating_window', { height });
            } catch (error) {
                console.error('Error resizing window:', error);
            }
        }, 50);
    }

    async resetHeightForNewMessage() {
        try {
            let height;
            
            if (this.userSetHeight) {
                height = this.userSetHeight;
            } else if (this.autoGrowHeight) {
                height = DEFAULT_HEIGHT;
                this.autoGrowHeight = null;
            } else {
                height = DEFAULT_HEIGHT;
            }
            
            console.log('[RESIZE DEBUG] resetHeightForNewMessage:', { height, userSetHeight: this.userSetHeight, autoGrowHeight: this.autoGrowHeight });
            
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
            // Small delay so blur handler sees isDragging before it's cleared
            setTimeout(() => { this.isDragging = false; }, 200);
        });
    }
}
