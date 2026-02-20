// Window management and resizing

const DEFAULT_HEIGHT = 60;
const MAX_HEIGHT_PERCENT = 0.5;

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
                // Only resize height, keep width as-is
                let contentHeight = 0;
                
                const loadingDots = document.getElementById('loadingDots');
                const contentArea = document.getElementById('contentArea');
                const responseText = document.getElementById('responseText');
                const appSuggestions = document.getElementById('appSuggestions');
                
                if (loadingDots?.classList.contains('visible')) {
                    contentHeight += loadingDots.offsetHeight;
                }
                
                if (contentArea?.classList.contains('visible')) {
                    contentHeight += responseText.scrollHeight + 32;
                }
                
                contentHeight += document.querySelector('.input-container')?.offsetHeight || 0;
                
                if (appSuggestions?.classList.contains('visible')) {
                    contentHeight += appSuggestions.offsetHeight;
                }
                
                let height = Math.max(DEFAULT_HEIGHT, Math.min(this.maxHeight, contentHeight));
                
                if (contentHeight > DEFAULT_HEIGHT && !this.userSetHeight) {
                    this.autoGrowHeight = height;
                }
                
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
            // Only reset height, keep width unchanged
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
}
