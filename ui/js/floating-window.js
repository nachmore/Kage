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
        const screenHeight = window.screen.height;
        return Math.floor(screenHeight * MAX_HEIGHT_PERCENT);
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

    setupResizeHandle(resizeHandle) {
        let startX = 0;
        let startY = 0;
        let startWidth = 0;
        let startHeight = 0;

        const onMouseMove = async (e) => {
            const maxWidth = Math.floor(window.screen.width * MAX_HEIGHT_PERCENT);
            const newWidth = Math.max(516, Math.min(maxWidth, startWidth + (e.screenX - startX)));
            const newHeight = Math.max(DEFAULT_HEIGHT, Math.min(this.maxHeight, startHeight + (e.screenY - startY)));
            this.userSetHeight = newHeight;
            try {
                await this.invoke('resize_floating_window', { width: newWidth, height: newHeight });
            } catch (err) {
                // ignore resize errors during drag
            }
        };

        const onMouseUp = () => {
            document.removeEventListener('mousemove', onMouseMove);
            document.removeEventListener('mouseup', onMouseUp);
        };

        resizeHandle.addEventListener('mousedown', (e) => {
            e.preventDefault();
            e.stopPropagation();
            startX = e.screenX;
            startY = e.screenY;
            startWidth = document.documentElement.offsetWidth;
            startHeight = document.documentElement.offsetHeight;
            document.addEventListener('mousemove', onMouseMove);
            document.addEventListener('mouseup', onMouseUp);
        });
    }
}
