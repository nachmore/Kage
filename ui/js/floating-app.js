// Main application logic
import { renderUrlSuggestion, renderPathSuggestion, renderSuggestions, updateSelection } from './floating-suggestions.js';
import { WindowManager } from './floating-window.js';
import { renderMarkdown } from './floating-markdown.js';

export class FloatingApp {
    constructor(invoke, appWindow, listen) {
        this.invoke = invoke;
        this.appWindow = appWindow;
        this.listen = listen;
        this.windowManager = new WindowManager(invoke);
        
        this.currentMatches = [];
        this.selectedIndex = -1;
        this.searchTimeout = null;
        this.currentResponse = '';
        this.isWaitingForResponse = false;
        
        this.elements = {};
    }

    init() {
        this.cacheElements();
        this.setupEventListeners();
        this.setupStreamingListeners();
        this.setupVisibilityTracking();
        this.windowManager.setupDragging(this.elements.ghostContainer);
        
        setTimeout(() => this.elements.input.focus(), 100);
        console.log('Initialization complete!');
    }

    cacheElements() {
        this.elements = {
            input: document.getElementById('promptInput'),
            appSuggestions: document.getElementById('appSuggestions'),
            contentArea: document.getElementById('contentArea'),
            responseText: document.getElementById('responseText'),
            loadingDots: document.getElementById('loadingDots'),
            expandBtn: document.getElementById('expandBtn'),
            ghostContainer: document.querySelector('.ghost-container')
        };
    }

    setupEventListeners() {
        this.elements.input.addEventListener('input', (e) => this.handleInputChange(e));
        this.elements.input.addEventListener('keydown', (e) => this.handleKeyDown(e));
        this.elements.expandBtn.addEventListener('click', () => this.handleExpandClick());
        document.addEventListener('click', (e) => this.handleOutsideClick(e));
    }

    setupStreamingListeners() {
        this.listen('message_chunk', (event) => this.handleMessageChunk(event));
        this.listen('message_complete', () => this.handleMessageComplete());
        this.listen('message_error', (event) => this.handleMessageError(event));
    }

    setupVisibilityTracking() {
        let lastVisibilityState = false;
        
        setInterval(async () => {
            try {
                const isVisible = await this.appWindow.isVisible();
                if (isVisible && !lastVisibilityState) {
                    setTimeout(() => this.resetUI(), 50);
                }
                lastVisibilityState = isVisible;
            } catch (error) {
                // Ignore errors
            }
        }, 100);
        
        this.appWindow.listen('tauri://focus', async () => {
            setTimeout(() => this.elements.input.focus(), 50);
        });
    }

    resetUI() {
        this.elements.input.value = '';
        this.elements.input.style.height = 'auto';
        this.elements.appSuggestions.classList.remove('visible');
        this.currentMatches = [];
        this.selectedIndex = -1;
        this.elements.contentArea.classList.remove('visible');
        this.stopThinking();
        this.elements.expandBtn.classList.remove('visible');
        this.currentResponse = '';
        this.elements.input.focus();
    }

    startThinking() {
        this.elements.ghostContainer.classList.add('thinking');
        this.elements.loadingDots.classList.add('visible');
    }

    stopThinking() {
        this.elements.ghostContainer.classList.remove('thinking');
        this.elements.loadingDots.classList.remove('visible');
    }

    async handleInputChange(event) {
        const query = this.elements.input.value.trim();
        
        this.elements.input.style.height = 'auto';
        this.elements.input.style.height = Math.min(this.elements.input.scrollHeight, 100) + 'px';
        
        if (this.searchTimeout) {
            clearTimeout(this.searchTimeout);
        }
        
        if (query.length === 0) {
            this.elements.appSuggestions.classList.remove('visible');
            this.currentMatches = [];
            this.selectedIndex = -1;
            await this.windowManager.resizeWindow();
            return;
        }
        
        this.searchTimeout = setTimeout(async () => {
            await this.performSearch(query);
        }, 150);
    }

    async performSearch(query) {
        console.log('Searching for apps:', query);
        try {
            const result = await this.invoke('handle_floating_input', { input: query });
            console.log('Search result:', result);
            
            if (result.startsWith('url:')) {
                const url = result.substring(4);
                this.selectedIndex = renderUrlSuggestion(
                    url, 
                    this.elements.appSuggestions, 
                    this.currentMatches,
                    (u) => this.openUrl(u),
                    () => this.windowManager.resizeWindow()
                );
            } else if (result.startsWith('path:')) {
                const pathInfo = result.substring(5);
                const colonIndex = pathInfo.indexOf(':');
                const type = pathInfo.substring(0, colonIndex);
                const path = pathInfo.substring(colonIndex + 1);
                this.selectedIndex = renderPathSuggestion(
                    type,
                    path,
                    this.elements.appSuggestions,
                    this.currentMatches,
                    (p) => this.openPath(p),
                    () => this.windowManager.resizeWindow()
                );
            } else if (result.startsWith('multiple:') || result.startsWith('launched:')) {
                const jsonStr = result.substring(result.indexOf(':') + 1);
                const apps = JSON.parse(jsonStr);
                if (apps.length > 0) {
                    this.currentMatches = apps;
                    this.selectedIndex = 0;
                    renderSuggestions(
                        apps,
                        this.elements.appSuggestions,
                        this.selectedIndex,
                        (name) => this.launchApp(name),
                        () => this.windowManager.resizeWindow()
                    );
                } else {
                    this.clearSuggestions();
                }
            } else {
                this.clearSuggestions();
            }
        } catch (error) {
            console.error('Error searching apps:', error);
        }
    }

    async clearSuggestions() {
        this.elements.appSuggestions.classList.remove('visible');
        this.currentMatches = [];
        this.selectedIndex = -1;
        await this.windowManager.resizeWindow();
    }

    async handleKeyDown(event) {
        if (event.key === 'ArrowDown') {
            event.preventDefault();
            if (this.currentMatches.length > 0) {
                this.selectedIndex = (this.selectedIndex + 1) % this.currentMatches.length;
                updateSelection(this.elements.appSuggestions, this.selectedIndex);
            }
        } else if (event.key === 'ArrowUp') {
            event.preventDefault();
            if (this.currentMatches.length > 0) {
                this.selectedIndex = this.selectedIndex <= 0 ? this.currentMatches.length - 1 : this.selectedIndex - 1;
                updateSelection(this.elements.appSuggestions, this.selectedIndex);
            }
        } else if (event.key === 'Escape') {
            await this.appWindow.hide();
        } else if (event.key === 'Enter' && !event.shiftKey) {
            event.preventDefault();
            await this.handleEnterKey();
        }
    }

    async handleEnterKey() {
        const message = this.elements.input.value.trim();
        if (!message) return;
        
        if (this.isWaitingForResponse) {
            console.log('Interrupting current response with new question');
            this.stopThinking();
            this.isWaitingForResponse = false;
        }
        
        if (this.currentMatches.length > 0 && this.selectedIndex >= 0) {
            const selected = this.currentMatches[this.selectedIndex];
            if (selected.type === 'url') {
                await this.openUrl(selected.value);
            } else if (selected.type === 'path') {
                await this.openPath(selected.value);
            } else {
                await this.launchApp(selected.name);
            }
            return;
        }
        
        await this.sendChatMessage(message);
    }

    async sendChatMessage(message) {
        this.elements.input.value = '';
        this.elements.input.style.height = 'auto';
        this.elements.appSuggestions.classList.remove('visible');
        this.currentMatches = [];
        this.selectedIndex = -1;
        this.elements.contentArea.classList.remove('visible');
        
        await this.windowManager.resetHeightForNewMessage();
        this.startThinking();
        this.elements.expandBtn.classList.remove('visible');
        await this.windowManager.resizeWindow();
        
        try {
            const result = await this.invoke('handle_floating_input', { input: message });
            
            if (result.startsWith('url:')) {
                await this.openUrl(result.substring(4));
                this.stopThinking();
            } else if (result.startsWith('path:')) {
                const pathInfo = result.substring(5);
                const colonIndex = pathInfo.indexOf(':');
                const path = pathInfo.substring(colonIndex + 1);
                await this.openPath(path);
                this.stopThinking();
            } else if (result.startsWith('launched:')) {
                const apps = JSON.parse(result.substring(9));
                await this.launchApp(apps[0].name);
                this.stopThinking();
            } else if (result === 'chat') {
                this.currentResponse = '';
                this.elements.responseText.textContent = this.currentResponse;
                this.elements.contentArea.classList.add('visible');
                this.elements.expandBtn.classList.add('visible');
                this.isWaitingForResponse = true;
                await this.windowManager.resizeWindow();
                await this.invoke('send_message_streaming', { message });
            }
        } catch (error) {
            console.error('Error handling input:', error);
            this.showError('Error: ' + error);
        }
    }

    handleMessageChunk(event) {
        if (!this.isWaitingForResponse) return;
        
        this.currentResponse = event.payload;
        
        if (this.currentResponse && this.currentResponse.trim().length > 0) {
            this.elements.loadingDots.classList.remove('visible');
            this.elements.ghostContainer.classList.remove('thinking');
        }
        
        renderMarkdown(this.currentResponse, this.elements.responseText);
        
        if (this.elements.responseText.lastChild) {
            let streamingIndicator = this.elements.responseText.querySelector('.streaming-indicator');
            if (!streamingIndicator) {
                streamingIndicator = document.createElement('span');
                streamingIndicator.className = 'streaming-indicator';
                streamingIndicator.textContent = '...';
                this.elements.responseText.appendChild(streamingIndicator);
            }
        }
        
        this.windowManager.resizeWindow();
    }

    async handleMessageComplete() {
        if (!this.isWaitingForResponse) return;
        
        this.stopThinking();
        const streamingIndicator = this.elements.responseText.querySelector('.streaming-indicator');
        if (streamingIndicator) {
            streamingIndicator.remove();
        }
        
        renderMarkdown(this.currentResponse, this.elements.responseText);
        await this.windowManager.resizeWindow();
        this.isWaitingForResponse = false;
    }

    async handleMessageError(event) {
        if (!this.isWaitingForResponse) return;
        
        this.showError('Error: ' + event.payload);
        this.isWaitingForResponse = false;
    }

    showError(message) {
        this.stopThinking();
        this.currentResponse = message;
        this.elements.responseText.textContent = message;
        this.elements.contentArea.classList.add('visible');
        this.elements.expandBtn.classList.add('visible');
        this.windowManager.resizeWindow();
    }

    async openUrl(url) {
        try {
            await this.invoke('open_url', { url });
            await this.clearSuggestions();
            this.elements.input.value = '';
        } catch (error) {
            console.error('Error opening URL:', error);
        }
    }

    async openPath(path) {
        try {
            await this.invoke('open_path', { path });
            await this.clearSuggestions();
            this.elements.input.value = '';
        } catch (error) {
            console.error('Error opening path:', error);
        }
    }

    async launchApp(appName) {
        try {
            await this.invoke('launch_app_by_name', { appName });
            await this.clearSuggestions();
            this.elements.input.value = '';
        } catch (error) {
            console.error('Error launching app:', error);
        }
    }

    async handleExpandClick() {
        try {
            await this.invoke('open_chat_window');
            await this.appWindow.hide();
        } catch (error) {
            console.error('Error opening chat window:', error);
        }
    }

    async handleOutsideClick(event) {
        const container = document.querySelector('.floating-container');
        if (container && !container.contains(event.target)) {
            await this.appWindow.hide();
        }
    }
}
