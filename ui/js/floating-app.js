// Main application logic
import { renderShortcutSuggestion, renderShortcutSuggestions, renderUrlSuggestion, renderPathSuggestion, renderSuggestions, updateSelection } from './floating-suggestions.js';
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
        this.shortcuts = [];
        
        this.elements = {};
    }

    async init() {
        this.cacheElements();
        this.setupEventListeners();
        this.setupStreamingListeners();
        this.setupVisibilityTracking();
        this.windowManager.setupDragging(this.elements.ghostContainer);
        
        await this.loadShortcuts();
        
        // Listen for config updates
        this.listen('config_updated', async () => {
            console.log('Config updated, reloading shortcuts...');
            await this.loadShortcuts();
        });
        
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
        this.listen('tool_call_update', (event) => this.handleToolCallUpdate(event));
        this.toolSources = [];
    }

    setupVisibilityTracking() {
        let lastVisibilityState = false;
        
        setInterval(async () => {
            try {
                const isVisible = await this.appWindow.isVisible();
                if (isVisible && !lastVisibilityState) {
                    // Don't reset UI if permission modal is open
                    const permissionModal = document.getElementById('permissionModal');
                    if (!permissionModal || permissionModal.style.display === 'none') {
                        // Check if we should preserve the last response
                        try {
                            const config = await this.invoke('get_config');
                            if (config.ui?.preserve_last_response === false) {
                                setTimeout(() => this.resetUI(), 50);
                            } else {
                                // Just focus the input, keep the response
                                setTimeout(() => this.elements.input.focus(), 50);
                            }
                        } catch (e) {
                            // Fallback: preserve by default
                            setTimeout(() => this.elements.input.focus(), 50);
                        }
                    }
                }
                lastVisibilityState = isVisible;
            } catch (error) {
                // Ignore errors
            }
        }, 100);
        
        this.appWindow.listen('tauri://focus', async () => {
            setTimeout(() => this.elements.input.focus(), 50);
        });
        
        this.appWindow.listen('tauri://blur', async () => {
            // Don't hide if permission modal is open
            const permissionModal = document.getElementById('permissionModal');
            if (permissionModal && permissionModal.style.display !== 'none') {
                return;
            }
            // Don't hide if dragging the window
            if (this.windowManager.isDragging) {
                return;
            }
            // Don't hide if context menu is open
            const contextMenu = document.querySelector('.context-menu');
            if (contextMenu && contextMenu.style.display !== 'none') {
                return;
            }
            // Don't hide if context menu popup window is open
            if (window._contextMenuOpen) {
                return;
            }
            await this.appWindow.hide();
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
        this.toolSources = [];
        const sourcesEl = document.getElementById('toolSources');
        if (sourcesEl) sourcesEl.remove();
        const compactEl = document.getElementById('toolSourcesCompact');
        if (compactEl) compactEl.remove();
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
    async loadShortcuts() {
        try {
            const config = await this.invoke('get_config');
            this.shortcuts = config.shortcuts || [];
            console.log('Loaded shortcuts:', this.shortcuts);
        } catch (error) {
            console.error('Failed to load shortcuts:', error);
            this.shortcuts = [];
        }
    }

    matchShortcut(input) {
        const parts = input.split(/\s+/);
        const trigger = parts[0].toLowerCase();
        const args = parts.slice(1);

        // Find all shortcuts with matching trigger
        const matches = this.shortcuts.filter(s => s.shortcut.toLowerCase() === trigger);
        if (matches.length === 0) return null;

        // Score each match based on argument compatibility
        const scoredMatches = matches.map(shortcut => {
            const score = this.scoreShortcutMatch(shortcut, args);
            return { shortcut, args, score };
        });

        // Sort by score (highest first)
        scoredMatches.sort((a, b) => b.score - a.score);

        return scoredMatches;
    }

    scoreShortcutMatch(shortcut, args) {
        const actionType = shortcut.action_type || 'run_program';
        const argCount = args.length;

        // For open_url, check if URL has argument placeholders
        if (actionType === 'open_url') {
            const url = shortcut.url || '';
            
            // Count specific placeholders {0}, {1}, etc.
            const placeholderCount = (url.match(/\{\d+\}/g) || []).length;
            
            if (placeholderCount > 0) {
                // Has specific placeholders - prefer exact match
                if (argCount === placeholderCount) return 100; // Perfect match
                if (argCount > placeholderCount) return 80;    // Extra args ignored
                return 60;                                      // Missing args
            }
            
            if (url.includes('{*}')) {
                // Wildcard - accepts any args but lower priority than exact match
                return argCount > 0 ? 90 : 50; // Prefer if args provided, but less than exact
            }
            
            // No placeholders - prefer if no args
            return argCount === 0 ? 100 : 50;
        }

        // For run_program
        const argTemplate = shortcut.arguments || '';
        
        if (!argTemplate) {
            // No argument template - prefer if no args
            return argCount === 0 ? 100 : 50;
        }

        // Count specific placeholders {0}, {1}, etc.
        const placeholderCount = (argTemplate.match(/\{\d+\}/g) || []).length;
        
        if (placeholderCount > 0) {
            // Has specific placeholders - prefer exact match
            if (argCount === placeholderCount) return 100; // Perfect match
            if (argCount > placeholderCount) return 80;    // Extra args ignored
            return 60;                                      // Missing args
        }
        
        if (argTemplate.includes('{*}')) {
            // Wildcard - accepts any args but lower priority than exact match
            return argCount > 0 ? 90 : 50; // Prefer if args provided, but less than exact
        }

        // Template exists but no placeholders - prefer if no args
        return argCount === 0 ? 100 : 50;
    }

    buildShortcutCommand(shortcut, args) {
        const actionType = shortcut.action_type || 'run_program';
        
        if (actionType === 'open_url') {
            let url = shortcut.url || '';
            
            // Handle {*} - all arguments
            if (url.includes('{*}')) {
                // Join args with spaces and encode the entire result
                const allArgs = args.join(' ');
                url = url.replace('{*}', encodeURIComponent(allArgs));
            } else {
                // Handle {0}, {1}, etc. - specific arguments
                for (let i = 0; i < args.length; i++) {
                    url = url.replace(new RegExp(`\\{${i}\\}`, 'g'), encodeURIComponent(args[i]));
                }
            }
            
            return { type: 'open_url', url };
        } else {
            // Run program
            if (!shortcut.arguments) {
                return { type: 'run_program', path: shortcut.path, args: [], workDir: shortcut.working_directory };
            }

            const argTemplate = shortcut.arguments;

            // Handle {*} - all arguments
            if (argTemplate.includes('{*}')) {
                const processedArgs = argTemplate.replace('{*}', args.join(' ')).split(/\s+/).filter(a => a);
                return { type: 'run_program', path: shortcut.path, args: processedArgs, workDir: shortcut.working_directory };
            }

            // Handle {0}, {1}, etc. - specific arguments
            let processedArgs = argTemplate;
            for (let i = 0; i < args.length; i++) {
                processedArgs = processedArgs.replace(new RegExp(`\\{${i}\\}`, 'g'), args[i]);
            }

            return {
                type: 'run_program',
                path: shortcut.path,
                args: processedArgs.split(/\s+/).filter(a => a && !a.match(/^\{\d+\}$/)),
                workDir: shortcut.working_directory
            };
        }
    }

    async executeShortcut(command) {
        try {
            if (command.type === 'open_url') {
                await this.openUrl(command.url);
            } else {
                await this.invoke('execute_shortcut', {
                    path: command.path,
                    args: command.args,
                    workingDirectory: command.workDir || null
                });
            }
            this.resetUI();
            await this.appWindow.hide();
        } catch (error) {
            console.error('Failed to execute shortcut:', error);
            this.showError('Failed to execute shortcut: ' + error);
        }
    }

    async handleInputChange(event) {
        const query = this.elements.input.value.trim();
        
        this.elements.input.style.height = 'auto';
        this.elements.input.style.height = Math.min(this.elements.input.scrollHeight, 100) + 'px';
        
        // Resize window to fit the growing input
        await this.windowManager.resizeWindow();
        
        if (this.searchTimeout) {
            clearTimeout(this.searchTimeout);
        }
        
        if (query.length === 0) {
            this.elements.appSuggestions.classList.remove('visible');
            this.currentMatches = [];
            this.selectedIndex = -1;
            return;
        }
        
        this.searchTimeout = setTimeout(async () => {
            await this.performSearch(query);
        }, 150);
    }

    async performSearch(query) {
        console.log('Searching for apps:', query);
        
        // Check for shortcut matches first
        const shortcutMatches = this.matchShortcut(query);
        if (shortcutMatches && shortcutMatches.length > 0) {
            if (shortcutMatches.length === 1) {
                // Single match - show it directly
                const match = shortcutMatches[0];
                const command = this.buildShortcutCommand(match.shortcut, match.args);
                this.selectedIndex = renderShortcutSuggestion(
                    match.shortcut,
                    match.args,
                    this.elements.appSuggestions,
                    this.currentMatches,
                    () => this.executeShortcut(command),
                    () => this.windowManager.resizeWindow()
                );
            } else {
                // Multiple matches - show all with scores
                this.currentMatches = shortcutMatches.map(match => ({
                    type: 'shortcut',
                    shortcut: match.shortcut,
                    args: match.args,
                    score: match.score
                }));
                this.selectedIndex = 0;
                renderShortcutSuggestions(
                    shortcutMatches,
                    this.elements.appSuggestions,
                    this.selectedIndex,
                    (match) => {
                        const command = this.buildShortcutCommand(match.shortcut, match.args);
                        this.executeShortcut(command);
                    },
                    () => this.windowManager.resizeWindow()
                );
            }
            return;
        }
        
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
            if (selected.type === 'shortcut') {
                const command = this.buildShortcutCommand(selected.shortcut, selected.args);
                await this.executeShortcut(command);
            } else if (selected.type === 'url') {
                await this.openUrl(selected.value);
            } else if (selected.type === 'path') {
                await this.openPath(selected.value);
            } else {
                await this.launchApp(selected.name);
            }
            return;
        }
        
        // Check if the message itself is a shortcut (without suggestion selected)
        const shortcutMatches = this.matchShortcut(message);
        if (shortcutMatches && shortcutMatches.length > 0) {
            // Use the best match (first one, already sorted by score)
            const bestMatch = shortcutMatches[0];
            const command = this.buildShortcutCommand(bestMatch.shortcut, bestMatch.args);
            await this.executeShortcut(command);
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
        this.toolSources = [];
        const sourcesEl2 = document.getElementById('toolSources');
        if (sourcesEl2) sourcesEl2.remove();
        const compactEl2 = document.getElementById('toolSourcesCompact');
        if (compactEl2) compactEl2.remove();
        
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
            
            // Transition compact sources to full (bottom) layout
            const compactEl = document.getElementById('toolSourcesCompact');
            if (compactEl) {
                compactEl.remove();
                if (this.toolSources.length > 0) {
                    this.renderSources();
                }
            }
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

    handleToolCallUpdate(event) {
        const notification = event.payload;
        const update = notification?.params?.update;
        if (!update) return;
        
        const kind = update.kind;
        const rawOutput = update.rawOutput;
        
        // Extract URLs from search results in rawOutput (arrives on tool_call_update with status=completed)
        if (rawOutput && (kind === 'search' || update.title?.toLowerCase().includes('search'))) {
            this.extractSources(rawOutput);
        }
        
        // Also check content for URLs
        if (update.content && Array.isArray(update.content)) {
            for (const item of update.content) {
                if (item.type === 'content' && item.content?.text) {
                    this.extractSourcesFromText(item.content.text);
                }
            }
        }
        
        if (this.toolSources.length > 0) {
            // If no text content yet, show compact sources in place of loading dots
            if (!this.currentResponse || this.currentResponse.trim().length === 0) {
                this.renderSourcesCompact();
            } else {
                this.renderSources();
            }
        }
    }

    extractSources(rawOutput) {
        // rawOutput structure from web_search:
        // { items: [{ Json: { results: [{ url, title, domain, ... }] } }] }
        if (rawOutput && rawOutput.items && Array.isArray(rawOutput.items)) {
            for (const item of rawOutput.items) {
                const results = item?.Json?.results || item?.results;
                if (Array.isArray(results)) {
                    for (const result of results) {
                        if (result.url) {
                            this.addSource(result.url, result.title, result.domain);
                        }
                    }
                }
            }
        } else if (Array.isArray(rawOutput)) {
            for (const result of rawOutput) {
                if (result.url) this.addSource(result.url, result.title, result.domain);
            }
        } else if (typeof rawOutput === 'object') {
            const results = rawOutput.results || rawOutput.searchResults;
            if (Array.isArray(results)) {
                for (const result of results) {
                    if (result.url) this.addSource(result.url, result.title, result.domain);
                }
            }
        }
    }

    extractSourcesFromText(text) {
        // Extract URLs from markdown-style links [title](url) or plain URLs
        const linkRegex = /\[([^\]]*)\]\((https?:\/\/[^\s)]+)\)/g;
        let match;
        while ((match = linkRegex.exec(text)) !== null) {
            this.addSource(match[2], match[1]);
        }
    }

    addSource(url, title, domainHint) {
        try {
            const parsed = new URL(url);
            const domain = domainHint || parsed.hostname.replace(/^www\./, '');
            // Deduplicate by domain
            if (!this.toolSources.find(s => s.domain === domain)) {
                const initials = domain.split('.')[0].substring(0, 2).toUpperCase();
                // Generate a consistent color from domain name
                let hash = 0;
                for (let i = 0; i < domain.length; i++) {
                    hash = domain.charCodeAt(i) + ((hash << 5) - hash);
                }
                const hue = Math.abs(hash) % 360;
                const color = `hsl(${hue}, 55%, 45%)`;
                
                this.toolSources.push({
                    url: url,
                    domain: domain,
                    title: title || domain,
                    initials: initials,
                    color: color,
                    favicon: `https://www.google.com/s2/favicons?domain=${domain}&sz=32`
                });
            }
        } catch (e) {
            // Invalid URL, skip
        }
    }

    renderSources() {
        // Remove compact version if it exists
        const compactEl = document.getElementById('toolSourcesCompact');
        if (compactEl) compactEl.remove();
        
        let sourcesEl = document.getElementById('toolSources');
        if (!sourcesEl) {
            sourcesEl = document.createElement('div');
            sourcesEl.id = 'toolSources';
            sourcesEl.className = 'tool-sources';
            const contentArea = this.elements.contentArea;
            if (contentArea) {
                contentArea.appendChild(sourcesEl);
            }
        }
        
        if (this.toolSources.length === 0) {
            sourcesEl.style.display = 'none';
            return;
        }
        
        sourcesEl.style.display = 'flex';
        this.elements.contentArea.classList.add('visible');
        
        sourcesEl.innerHTML = this.toolSources.map(source => `
            <a class="source-chip" href="#" onclick="event.preventDefault(); window.__TAURI__.core.invoke('open_url', { url: '${source.url.replace(/'/g, "\\'")}' })" title="${source.title}">
                <span class="source-icon-wrapper">
                    <span class="source-initials" style="background:${source.color}">${source.initials}</span>
                    <img class="source-favicon" src="${source.favicon}" alt="" onload="this.previousElementSibling.style.display='none'" onerror="this.style.display='none'">
                </span>
                <span class="source-domain">${source.domain}</span>
            </a>
        `).join('');
        
        this.windowManager.resizeWindow();
    }

    renderSourcesCompact() {
        // Hide loading dots and show compact source icons in their place
        this.elements.loadingDots.classList.remove('visible');
        this.elements.ghostContainer.classList.remove('thinking');
        
        let compactEl = document.getElementById('toolSourcesCompact');
        if (!compactEl) {
            compactEl = document.createElement('div');
            compactEl.id = 'toolSourcesCompact';
            compactEl.className = 'tool-sources-compact';
            // Insert where loading dots are — inside the speech bubble, before content area
            const speechBubble = document.querySelector('.speech-bubble');
            if (speechBubble) {
                speechBubble.insertBefore(compactEl, this.elements.contentArea);
            }
        }
        
        compactEl.style.display = 'flex';
        compactEl.innerHTML = this.toolSources.map((source, i) => `
            <a class="source-bubble" href="#" onclick="event.preventDefault(); window.__TAURI__.core.invoke('open_url', { url: '${source.url.replace(/'/g, "\\'")}' })" title="${source.title}" style="animation-delay: ${i * 0.08}s">
                <span class="source-icon-wrapper">
                    <span class="source-initials" style="background:${source.color}">${source.initials}</span>
                    <img class="source-favicon" src="${source.favicon}" alt="" onload="this.previousElementSibling.style.display='none'" onerror="this.style.display='none'">
                </span>
            </a>
        `).join('');
        
        this.windowManager.resizeWindow();
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
        // Don't hide if the permission modal is open
        const permissionModal = document.getElementById('permissionModal');
        if (permissionModal && permissionModal.style.display !== 'none') {
            return;
        }
        
        const container = document.querySelector('.floating-container');
        if (container && !container.contains(event.target)) {
            await this.appWindow.hide();
        }
    }
}
