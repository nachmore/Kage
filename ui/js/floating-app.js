// Main application logic
import { renderShortcutSuggestion, renderShortcutSuggestions, renderUrlSuggestion, renderPathSuggestion, renderSuggestions, updateSelection, appendSendHint } from './floating-suggestions.js';
import { WindowManager } from './floating-window.js';
import { renderMarkdown } from './floating-markdown.js';
import { matchCommands, matchSlashCommands, matchCommandsByName, loadSlashCommands, renderCommandSuggestions, executeCommand } from './floating-commands.js';
import { AttachmentManager, handlePasteEvent, renderAttachmentPreviews } from './attachments.js';
import { evaluateMath } from './math-eval.js';
import { processToolCallUpdate, renderToolChipsHtml, renderSourceChipsHtml, renderSourceBubblesHtml, getSessionResetMessage } from './streaming-utils.js';
import { sendAppNotification } from './notify.js';

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
        // Track the length at which pattern matching last failed (returned "chat").
        // While the input only grows beyond this length, skip redundant backend calls.
        this._noMatchSinceLen = 0;
        this.toolUsages = [];
        this.attachmentManager = new AttachmentManager();
        this.mathConfig = { enabled: true, precision: 0, auto_copy: true, thousands_separator: false };
        this.lastSelection = null;
        
        this.elements = {};
    }

    async init() {
        this.cacheElements();
        this.setupEventListeners();
        this.setupStreamingListeners();
        this.setupVisibilityTracking();
        this.windowManager.setupDragging(this.elements.ghostContainer);
        this.windowManager.setupResizeHandle(document.getElementById('resizeHandle'));
        
        await this.loadShortcuts();
        await loadSlashCommands(this.invoke);
        await this.loadMathConfig();
        
        // Listen for config updates
        this.listen('config_updated', async () => {
            console.log('Config updated, reloading shortcuts...');
            await this.loadShortcuts();
            await this.loadMathConfig();
        });

        // Listen for slash commands from ACP
        this.listen('slash_commands_available', async () => {
            console.log('Slash commands updated, reloading...');
            await loadSlashCommands(this.invoke);
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
            stopBtn: document.getElementById('stopGeneratingBtn'),
            floatingStopBtn: document.getElementById('floatingStopBtn'),
            ghostContainer: document.querySelector('.ghost-container'),
            attachmentPreviews: document.getElementById('attachmentPreviews')
        };
    }

    setupEventListeners() {
            this.elements.input.addEventListener('input', (e) => this.handleInputChange(e));
            this.elements.input.addEventListener('keydown', (e) => this.handleKeyDown(e));
            this.elements.expandBtn.addEventListener('click', () => this.handleExpandClick());
            this.elements.stopBtn.addEventListener('click', () => this.stopGenerating());
            this.elements.floatingStopBtn.addEventListener('click', () => this.stopGenerating());
            document.addEventListener('click', (e) => this.handleOutsideClick(e));

            // Global keyboard shortcuts
            document.addEventListener('keydown', (e) => {
                // Escape — stop generating first, then hide
                if (e.key === 'Escape') {
                    if (this.isWaitingForResponse) {
                        e.preventDefault();
                        this.stopGenerating();
                        return;
                    }
                    if (this._justStoppedGenerating) {
                        e.preventDefault();
                        return;
                    }
                    this.appWindow.hide();
                    return;
                }
                // Ctrl+, — open settings
                if (e.ctrlKey && e.key === ',') {
                    e.preventDefault();
                    this.invoke('open_settings_window');
                    return;
                }
                // Ctrl+E — expand to full chat
                if (e.ctrlKey && e.key === 'e') {
                    e.preventDefault();
                    this.handleExpandClick();
                    return;
                }
                // Ctrl+L — clear/reset
                if (e.ctrlKey && e.key === 'l') {
                    e.preventDefault();
                    this.resetUI();
                    this.windowManager.userSetHeight = null;
                    this.windowManager.resizeWindow();
                    return;
                }
                // Ctrl+Shift+C — copy last response
                if (e.ctrlKey && e.shiftKey && e.key === 'C') {
                    e.preventDefault();
                    if (this.currentResponse) {
                        navigator.clipboard.writeText(this.currentResponse).catch(() => {});
                    }
                    return;
                }
                // Ctrl+W — hide window
                if (e.ctrlKey && e.key === 'w') {
                    e.preventDefault();
                    this.appWindow.hide();
                    return;
                }
            });

            // Paste handler for images
            this.elements.input.addEventListener('paste', (e) => handlePasteEvent(e, this.attachmentManager));

            // Re-render previews when attachments change and resize window
            this.attachmentManager.onChange((attachments) => {
                renderAttachmentPreviews(this.elements.attachmentPreviews, attachments, this.attachmentManager);
                this.windowManager.resizeWindow();
            });
        }

    setupStreamingListeners() {
        this.listen('message_chunk', (event) => this.handleMessageChunk(event));
        this.listen('message_complete', () => this.handleMessageComplete());
        this.listen('message_error', (event) => this.handleMessageError(event));
        this.listen('tool_call_update', (event) => this.handleToolCallUpdate(event));
        this.listen('session_reset', (event) => this.handleSessionReset(event));
        this.toolSources = [];

        // Listen for selection captured from previous window
        this.listen('selection_captured', async (event) => {
            const hasSelection = event.payload;
            const indicator = document.getElementById('selectionIndicator');
            const checkbox = document.getElementById('useSelectionCheckbox');
            if (hasSelection) {
                try {
                    const raw = await this.invoke('get_last_selection');
                    this.lastSelection = raw?.trim() || null;
                } catch { this.lastSelection = null; }
                if (this.lastSelection) {
                    if (indicator) indicator.style.display = '';
                    if (checkbox) checkbox.checked = true;
                    this.windowManager.resizeWindow();
                    return;
                }
            }
            this.lastSelection = null;
            if (indicator) indicator.style.display = 'none';
        });

        document.addEventListener('kiro-clear', () => {
            this.resetUI();
            this.windowManager.userSetHeight = null;
            this.windowManager.resizeWindow();
        });

        document.addEventListener('kiro-show-response', (e) => {
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
            this.elements.appSuggestions.classList.remove('visible');
            this.currentMatches = [];
            this.selectedIndex = -1;
            this.currentResponse = e.detail;
            renderMarkdown(e.detail, this.elements.responseText);
            this.elements.contentArea.classList.add('visible');
            this.windowManager.resizeWindow();
        });

        document.addEventListener('kiro-show-selection', (e) => {
            const { command, options } = e.detail;
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
            this.elements.contentArea.classList.remove('visible');

            // Show options as selectable items in the suggestions dropdown
            this.currentMatches = options.map(opt => ({
                type: 'selection',
                name: opt.label,
                value: opt.value,
                current: opt.current,
                command: command
            }));
            this.selectedIndex = options.findIndex(o => o.current);
            if (this.selectedIndex < 0) this.selectedIndex = 0;

            const container = this.elements.appSuggestions;
            container.innerHTML = '';
            container.scrollTop = 0;

            options.forEach((opt, index) => {
                const item = document.createElement('div');
                item.className = 'app-suggestion-item' + (index === this.selectedIndex ? ' selected' : '');
                const currentBadge = opt.current ? '<span class="selection-current">●</span>' : '';
                item.innerHTML = `
                    <div class="app-icon">${opt.current ? '✓' : '○'}</div>
                    <div class="app-info">
                        <div class="app-name">${opt.label}${currentBadge}</div>
                        <div class="app-description">${opt.value}</div>
                    </div>
                `;
                item.addEventListener('click', () => this.executeSelection(command, opt.value));
                container.appendChild(item);
            });

            container.classList.add('visible');
            // Defer scroll-to-selected until after layout is complete
            this.windowManager.resizeWindow();
            setTimeout(() => updateSelection(container, this.selectedIndex), 20);
        });
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
                                // Just focus and select the input, keep the response
                                setTimeout(() => {
                                    this.elements.input.focus();
                                    this.elements.input.select();
                                }, 50);
                            }
                        } catch (e) {
                            // Fallback: preserve by default
                            setTimeout(() => {
                                this.elements.input.focus();
                                this.elements.input.select();
                            }, 50);
                        }
                    }
                }
                lastVisibilityState = isVisible;
            } catch (error) {
                // Ignore errors
            }
        }, 100);
        
        this.appWindow.listen('tauri://focus', async () => {
            setTimeout(() => {
                this.elements.input.focus();
                this.elements.input.select();
                // Re-show datetime on window focus
                const dtDisplay = document.getElementById('datetimeDisplay');
                if (dtDisplay) dtDisplay.style.opacity = '1';
            }, 50);
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
            // Don't hide if currently generating a response
            if (this.isWaitingForResponse || this._justStoppedGenerating) {
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
        this._noMatchSinceLen = 0;
        this.toolUsages = [];
        this.attachmentManager.clear();
        this.elements.contentArea.classList.remove('visible');
        this.stopThinking();
        this.elements.expandBtn.classList.remove('visible');
        this.currentResponse = '';
        this.toolSources = [];
        this.elements.floatingStopBtn.style.display = 'none';
        const sourcesEl = document.getElementById('toolSources');
        if (sourcesEl) sourcesEl.remove();
        const compactEl = document.getElementById('toolSourcesCompact');
        if (compactEl) compactEl.remove();
        this.elements.input.focus();
        // Re-show datetime when input is cleared
        const dtDisplay = document.getElementById('datetimeDisplay');
        if (dtDisplay) { dtDisplay.style.display = ''; dtDisplay.style.opacity = '1'; }
    }

    startThinking() {
        this.elements.ghostContainer.classList.add('thinking');
        this.elements.loadingDots.classList.add('visible');
        this.elements.stopBtn.style.display = 'none'; // Show after first chunk
        // Show inline stop button in input area, hide datetime
        const dtDisplay = document.getElementById('datetimeDisplay');
        if (dtDisplay) dtDisplay.style.display = 'none';
        this.elements.floatingStopBtn.style.display = '';
    }

    stopThinking() {
        this.elements.ghostContainer.classList.remove('thinking');
        this.elements.loadingDots.classList.remove('visible');
    }

    stopGenerating() {
        if (!this.isWaitingForResponse) return;
        this.isWaitingForResponse = false;
        this._justStoppedGenerating = true;
        setTimeout(() => { this._justStoppedGenerating = false; }, 300);
        this.stopThinking();
        this.elements.stopBtn.style.display = 'none';
        this.elements.floatingStopBtn.style.display = 'none';
        // Restore datetime display
        const dtDisplay = document.getElementById('datetimeDisplay');
        if (dtDisplay) { dtDisplay.style.display = ''; dtDisplay.style.opacity = '1'; }
        // Remove streaming indicator
        const indicator = this.elements.responseText.querySelector('.streaming-indicator');
        if (indicator) indicator.remove();
        // Re-render final markdown cleanly
        if (this.currentResponse) {
            renderMarkdown(this.currentResponse, this.elements.responseText);
        }
        this.windowManager.resizeWindow();
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

    async loadMathConfig() {
        try {
            const config = await this.invoke('get_config');
            this.mathConfig = config.math || { enabled: true, precision: 0, auto_copy: true, thousands_separator: false };
        } catch (error) {
            console.error('Failed to load math config:', error);
        }
    }

    formatMathResult(display) {
        if (this.mathConfig.thousands_separator) {
            const parts = display.split('.');
            parts[0] = parts[0].replace(/\B(?=(\d{3})+(?!\d))/g, ',');
            return parts.join('.');
        }
        return display;
    }

    tryEvaluateMath(query) {
        if (!this.mathConfig.enabled) return null;
        return evaluateMath(query, this.mathConfig.precision);
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

            // Helper: substitute {*} and {0},{1},... in a template string
            const substitute = (template, encode = false) => {
                if (!template) return '';
                let result = template;
                // {selection} — currently selected text from previous window
                const useSelection = document.getElementById('useSelectionCheckbox')?.checked;
                const sel = useSelection && this.lastSelection ? this.lastSelection : '';
                result = result.replace(/\{selection\}/g, encode ? encodeURIComponent(sel) : sel);
                if (result.includes('{*}')) {
                    const all = args.join(' ');
                    result = result.replace('{*}', encode ? encodeURIComponent(all) : all);
                } else {
                    for (let i = 0; i < args.length; i++) {
                        const val = encode ? encodeURIComponent(args[i]) : args[i];
                        result = result.replace(new RegExp(`\\{${i}\\}`, 'g'), val);
                    }
                }
                return result;
            };

            if (actionType === 'open_url') {
                return { type: 'open_url', url: substitute(shortcut.url || '', true) };
            }

            if (actionType === 'prompt') {
                return { type: 'prompt', message: substitute(shortcut.prompt || '{*}') };
            }

            if (actionType === 'script') {
                try {
                    const fn = new Function('...args', shortcut.script || 'return args.join(" ")');
                    const result = fn(...args);
                    if (result === null || result === undefined) {
                        return { type: 'noop' };
                    }
                    const scriptAction = shortcut.script_action || 'text';

                    if (scriptAction === 'run_program') {
                        if (!Array.isArray(result)) {
                            return { type: 'error', message: 'Script must return an array [cmd, workDir, ...args] for Run as Command' };
                        }
                        return {
                            type: 'run_program',
                            path: result[0] || '',
                            workDir: result[1] || null,
                            args: result.slice(2).map(String)
                        };
                    }

                    // All other actions expect a string
                    if (typeof result !== 'string') {
                        return { type: 'error', message: 'Script must return a string, got ' + typeof result };
                    }
                    if (scriptAction === 'open_url') return { type: 'open_url', url: result };
                    if (scriptAction === 'prompt') return { type: 'prompt', message: result };
                    return { type: 'text', message: result };
                } catch (e) {
                    return { type: 'error', message: `Script ${e.constructor?.name || 'Error'}: ${e.message}` };
                }
            }

            // run_program (default)
            if (!shortcut.arguments) {
                return { type: 'run_program', path: shortcut.path, args: [], workDir: shortcut.working_directory };
            }
            const processedArgs = substitute(shortcut.arguments).split(/\s+/).filter(a => a && !a.match(/^\{\d+\}$/));
            return { type: 'run_program', path: shortcut.path, args: processedArgs, workDir: shortcut.working_directory };
        }

    async executeShortcut(command) {
            try {
                if (command.type === 'error') {
                    this.showError(command.message);
                    return;
                }
                if (command.type === 'noop') {
                    this.elements.input.value = '';
                    this.elements.input.style.height = 'auto';
                    this.clearSuggestions();
                    return;
                }
                if (command.type === 'open_url') {
                    await this.openUrl(command.url);
                    this.resetUI();
                    await this.appWindow.hide();
                } else if (command.type === 'prompt') {
                    // Send to agent as if the user typed it
                    await this.sendChatMessage(command.message);
                } else if (command.type === 'text') {
                    // Display result in the response area
                    this.elements.input.value = '';
                    this.elements.input.style.height = 'auto';
                    this.clearSuggestions();
                    this.currentResponse = command.message;
                    renderMarkdown(command.message, this.elements.responseText);
                    this.elements.contentArea.classList.add('visible');
                    this.windowManager.resizeWindow();
                } else {
                    // run_program
                    await this.invoke('execute_shortcut', {
                        path: command.path,
                        args: command.args,
                        workingDirectory: command.workDir || null
                    });
                    this.resetUI();
                    await this.appWindow.hide();
                }
            } catch (error) {
                console.error('Failed to execute shortcut:', error);
                this.showError('Failed to execute shortcut: ' + error);
            }
        }

    async handleInputChange(event) {
        const query = this.elements.input.value.trim();
        
        this.elements.input.style.height = 'auto';
        this.elements.input.style.height = Math.min(this.elements.input.scrollHeight, 100) + 'px';
        
        // Reset tab cycle state when user types
        this._tabCycleActive = false;
        
        // Resize window to fit the growing input
        await this.windowManager.resizeWindow();
        
        if (this.searchTimeout) {
            clearTimeout(this.searchTimeout);
        }
        
        if (query.length === 0) {
            this.elements.appSuggestions.classList.remove('visible');
            this.currentMatches = [];
            this.selectedIndex = -1;
            this._noMatchSinceLen = 0;
            return;
        }
        
        // Check for math expression
        const mathResult = this.tryEvaluateMath(query);
        if (mathResult) {
            const formatted = this.formatMathResult(mathResult.display);
            this.currentMatches = [{ type: 'math', value: formatted, raw: mathResult.result }];
            this.selectedIndex = 0;
            const container = this.elements.appSuggestions;
            container.innerHTML = '';
            container.scrollTop = 0;
            const item = document.createElement('div');
            item.className = 'app-suggestion-item selected';
            item.innerHTML = `
                <div class="app-icon">🧮</div>
                <div class="app-info">
                    <div class="app-name math-result-value">= ${formatted}</div>
                    <div class="app-description">Press Enter to copy result</div>
                </div>
            `;
            item.addEventListener('click', async () => {
                await navigator.clipboard.writeText(formatted);
                this.elements.input.value = '';
                this.elements.input.style.height = 'auto';
                this.clearSuggestions();
            });
            container.appendChild(item);
            container.classList.add('visible');
            setTimeout(() => this.windowManager.resizeWindow(), 10);
            return;
        }

        // Check for > command prefix
        if (query.startsWith('>')) {
            this._noMatchSinceLen = 0;
            const commands = matchCommands(query);
            if (commands && commands.length > 0) {
                this.currentMatches = commands.map(cmd => ({ type: 'command', ...cmd }));
                this.selectedIndex = 0;
                renderCommandSuggestions(
                    commands,
                    this.elements.appSuggestions,
                    this.selectedIndex,
                    (cmd) => this.executeCommandAction(cmd),
                    () => this.windowManager.resizeWindow()
                );
            } else {
                this.clearSuggestions();
            }
            return;
        }

        // Check for / slash command prefix (ACP commands)
        if (query.startsWith('/')) {
            this._noMatchSinceLen = 0;
            const slashCmds = matchSlashCommands(query);
            if (slashCmds && slashCmds.length > 0) {
                this.currentMatches = slashCmds;
                this.selectedIndex = 0;
                renderCommandSuggestions(
                    slashCmds,
                    this.elements.appSuggestions,
                    this.selectedIndex,
                    (cmd) => this.executeCommandAction(cmd),
                    () => this.windowManager.resizeWindow()
                );
            } else {
                this.clearSuggestions();
            }
            return;
        }

        this.searchTimeout = setTimeout(async () => {
            await this.performSearch(query);
            // Show send hint if suggestions are visible
            if (this.elements.appSuggestions.classList.contains('visible')) {
                appendSendHint(this.elements.appSuggestions);
                this.windowManager.resizeWindow();
            }
        }, 150);
    }

    async performSearch(query) {
        // Check for matching commands (without > prefix) to show at top
        const cmdMatches = matchCommandsByName(query);

        // If the input grew past a length where we already know there's no pattern
        // match, skip the backend call — it will just return "chat" again.
        if (this._noMatchSinceLen > 0 && query.length >= this._noMatchSinceLen) {
            // Still show command matches if any
            if (cmdMatches.length > 0) {
                this.currentMatches = cmdMatches;
                this.selectedIndex = 0;
                renderCommandSuggestions(
                    cmdMatches, this.elements.appSuggestions, this.selectedIndex,
                    (cmd) => this.executeCommandAction(cmd),
                    () => this.windowManager.resizeWindow(),
                    true
                );
            } else {
                this.clearSuggestions();
            }
            return;
        }
        // Input was shortened — reset and re-evaluate
        if (query.length < this._noMatchSinceLen) {
            this._noMatchSinceLen = 0;
        }
        
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
                // No pattern match — remember this length so we skip future calls
                // while the user keeps typing (input only grows).
                this._noMatchSinceLen = query.length;
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

    async executeCommandAction(cmd) {
        this.elements.input.value = '';
        this.elements.input.style.height = 'auto';
        this.clearSuggestions();
        await cmd.execute(this.invoke, this.appWindow);
    }

    async executeSelection(command, value) {
        this.clearSuggestions();
        try {
            // For selection commands, use the convention: arg key is commandName + "Name"
            // e.g. "model" command → { modelName: value }
            const argKey = command + 'Name';
            const result = await this.invoke('execute_slash_command', {
                command: command,
                args: { [argKey]: value }
            });
            const msg = result?.message || `Selected: ${value}`;
            document.dispatchEvent(new CustomEvent('kiro-show-response', { detail: msg }));
        } catch (e) {
            document.dispatchEvent(new CustomEvent('kiro-show-response', { detail: 'Error: ' + e }));
        }
    }

    async handleKeyDown(event) {
        if (event.key === 'Tab') {
            event.preventDefault();
            // Cycle through suggestions on repeated Tab presses
            if (this.currentMatches.length > 0) {
                if (this._tabCycleActive) {
                    this._tabCycleIndex = (this._tabCycleIndex + 1) % this.currentMatches.length;
                } else {
                    this._tabCycleIndex = 0;
                    this._tabCycleActive = true;
                }
                const pick = this.currentMatches[this._tabCycleIndex];
                if (pick.type === 'command') {
                    this.elements.input.value = '>' + pick.name + ' ';
                } else if (pick.type === 'slash') {
                    this.elements.input.value = pick.name + ' ';
                } else if (pick.name) {
                    this.elements.input.value = pick.name;
                }
                this.selectedIndex = this._tabCycleIndex;
                updateSelection(this.elements.appSuggestions, this.selectedIndex);
            }
        } else if (event.key === 'ArrowDown') {
            if (this.currentMatches.length > 0) {
                event.preventDefault();
                this.selectedIndex = (this.selectedIndex + 1) % this.currentMatches.length;
                updateSelection(this.elements.appSuggestions, this.selectedIndex);
            }
            // When no suggestions, let the default behavior handle cursor movement in textarea
        } else if (event.key === 'ArrowUp') {
            if (this.currentMatches.length > 0) {
                event.preventDefault();
                this.selectedIndex = this.selectedIndex <= 0 ? this.currentMatches.length - 1 : this.selectedIndex - 1;
                updateSelection(this.elements.appSuggestions, this.selectedIndex);
            }
            // When no suggestions, let the default behavior handle cursor movement in textarea
        } else if (event.key === 'Escape') {
            if (this.isWaitingForResponse) {
                event.preventDefault();
                this.stopGenerating();
            } else if (this._justStoppedGenerating) {
                event.preventDefault();
            } else {
                await this.appWindow.hide();
            }
        } else if (event.key === 'Enter' && event.ctrlKey) {
            // Ctrl+Enter: send directly to agent, bypassing suggestions and input classification
            event.preventDefault();
            const message = this.elements.input.value.trim();
            if (message) {
                await this.clearSuggestions();
                await this.sendChatMessage(message, { forceChat: true });
            }
        } else if (event.key === 'Enter' && !event.shiftKey && !event.ctrlKey) {
            event.preventDefault();
            await this.handleEnterKey();
        }
    }

    async handleEnterKey() {
        const message = this.elements.input.value.trim();
        const hasAttachments = this.attachmentManager.hasAttachments();

        // Allow Enter to work on selection lists even when input is empty
        if (!message && !hasAttachments && !(this.currentMatches.length > 0 && this.selectedIndex >= 0)) return;
        
        if (this.isWaitingForResponse) {
            console.log('Interrupting current response with new question');
            this.stopThinking();
            this.isWaitingForResponse = false;
        }
        
        // Handle math result
        if (this.currentMatches.length > 0 && this.currentMatches[0].type === 'math') {
            const mathMatch = this.currentMatches[0];
            const formatted = mathMatch.value;
            
            // Show result in response area
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
            this.clearSuggestions();
            this.currentResponse = formatted;
            renderMarkdown(`\`= ${formatted}\``, this.elements.responseText);
            this.elements.contentArea.classList.add('visible');
            this.windowManager.resizeWindow();
            
            // Auto-copy to clipboard
            if (this.mathConfig.auto_copy) {
                try {
                    await navigator.clipboard.writeText(formatted);
                } catch (e) {
                    console.error('Failed to copy math result:', e);
                }
            }
            return;
        }

        // Handle > commands
        if (message.startsWith('>')) {
            const cmdName = message.substring(1).trim();
            if (await executeCommand(cmdName, this.invoke, this.appWindow)) {
                this.elements.input.value = '';
                this.elements.input.style.height = 'auto';
                this.clearSuggestions();
                return;
            }
        }

        // Handle / slash commands
        if (message.startsWith('/')) {
            const slashCmds = matchSlashCommands(message);
            if (slashCmds && slashCmds.length === 1) {
                this.elements.input.value = '';
                this.elements.input.style.height = 'auto';
                this.clearSuggestions();
                await slashCmds[0].execute(this.invoke, this.appWindow);
                return;
            }
        }

        // Handle selected suggestion (command, slash, or otherwise)
        if (this.currentMatches.length > 0 && this.selectedIndex >= 0) {
            const selected = this.currentMatches[this.selectedIndex];
            if (selected.type === 'command' || selected.type === 'slash') {
                await this.executeCommandAction(selected);
                return;
            } else if (selected.type === 'selection') {
                await this.executeSelection(selected.command, selected.value);
                return;
            } else if (selected.type === 'shortcut') {
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

    async sendChatMessage(message, options = {}) {
        const attachments = this.attachmentManager.toContentBlocks();
        this.attachmentManager.clear();

        // Include selected text as context if checkbox is checked
        const useSelection = document.getElementById('useSelectionCheckbox')?.checked;
        if (useSelection && this.lastSelection && this.lastSelection.trim()) {
            message = `The following text is currently selected in my active window:\n\`\`\`\n${this.lastSelection.trim()}\n\`\`\`\n\n${message}`;
        }
        // Hide selection indicator after use
        const indicator = document.getElementById('selectionIndicator');
        if (indicator) indicator.style.display = 'none';
        this.lastSelection = null;

        this.elements.input.value = '';
        this.elements.input.style.height = 'auto';
        this.elements.appSuggestions.classList.remove('visible');
        this.currentMatches = [];
        this.selectedIndex = -1;
        this.elements.contentArea.classList.remove('visible');
        this.toolSources = [];
        this.toolUsages = [];
        const sourcesEl2 = document.getElementById('toolSources');
        if (sourcesEl2) sourcesEl2.remove();
        const compactEl2 = document.getElementById('toolSourcesCompact');
        if (compactEl2) compactEl2.remove();
        
        await this.windowManager.resetHeightForNewMessage();
        this.startThinking();
        this.elements.expandBtn.classList.remove('visible');
        await this.windowManager.resizeWindow();

        // Dismiss any pending permission request from the main chat window
        // so the session isn't stalled waiting for a response.
        try {
            await this.invoke('dismiss_pending_permission');
        } catch (e) {
            console.log('No pending permission to dismiss:', e);
        }
        
        try {
            // If forceChat, attachments present, or we already know there's no match, skip classification
            let result;
            if (options.forceChat || attachments) {
                result = 'chat';
            } else if (this._noMatchSinceLen > 0 && message.length >= this._noMatchSinceLen) {
                result = 'chat';
            } else {
                result = await this.invoke('handle_floating_input', { input: message });
            }
            this._noMatchSinceLen = 0;
            
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
                await this.invoke('send_message_streaming', { message, attachments });
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
            this.elements.stopBtn.style.display = '';
            
            // Transition compact sources to full (bottom) layout
            const compactEl = document.getElementById('toolSourcesCompact');
            if (compactEl) {
                compactEl.remove();
                if (this.toolSources.length > 0 || this.toolUsages.length > 0) {
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
            this.elements.stopBtn.style.display = 'none';
            this.elements.floatingStopBtn.style.display = 'none';
            // Restore datetime display
            const dtDisplay = document.getElementById('datetimeDisplay');
            if (dtDisplay) { dtDisplay.style.display = ''; dtDisplay.style.opacity = '1'; }
            const streamingIndicator = this.elements.responseText.querySelector('.streaming-indicator');
            if (streamingIndicator) streamingIndicator.remove();

            renderMarkdown(this.currentResponse, this.elements.responseText);
            await this.windowManager.resizeWindow();
            this.isWaitingForResponse = false;

            // Notify if window is hidden
            try {
                const isVisible = await this.appWindow.isVisible();
                if (!isVisible && this.currentResponse) {
                    const preview = this.currentResponse.substring(0, 100).replace(/[#*`\n]/g, ' ').trim();
                    await sendAppNotification(this.invoke, 'Kiro Assistant', preview || 'Response ready', 'floating');
                }
            } catch { /* ignore */ }
        }

    async handleMessageError(event) {
        if (!this.isWaitingForResponse) return;
        
        this.showError('Error: ' + event.payload);
        this.isWaitingForResponse = false;
        this.elements.floatingStopBtn.style.display = 'none';
        // Restore datetime display
        const dtDisplay = document.getElementById('datetimeDisplay');
        if (dtDisplay) { dtDisplay.style.display = ''; dtDisplay.style.opacity = '1'; }
    }

    handleSessionReset(event) {
            this.isWaitingForResponse = false;
            this.elements.floatingStopBtn.style.display = 'none';
            // Restore datetime display
            const dtDisplay = document.getElementById('datetimeDisplay');
            if (dtDisplay) { dtDisplay.style.display = ''; dtDisplay.style.opacity = '1'; }
            this.showError(getSessionResetMessage(event.payload));
        }

    handleToolCallUpdate(event) {
            if (!this.isWaitingForResponse) return;
            const { updated } = processToolCallUpdate(event, this);
            if (updated && (this.toolSources.length > 0 || this.toolUsages.length > 0)) {
                if (!this.currentResponse || this.currentResponse.trim().length === 0) {
                    this.renderSourcesCompact();
                } else {
                    this.renderSources();
                }
            }
        }






    renderSources() {
            const compactEl = document.getElementById('toolSourcesCompact');
            if (compactEl) compactEl.remove();

            let sourcesEl = document.getElementById('toolSources');
            if (!sourcesEl) {
                sourcesEl = document.createElement('div');
                sourcesEl.id = 'toolSources';
                sourcesEl.className = 'tool-sources';
                if (this.elements.contentArea) this.elements.contentArea.appendChild(sourcesEl);
            }

            if (this.toolSources.length === 0 && this.toolUsages.length === 0) {
                sourcesEl.style.display = 'none';
                return;
            }

            sourcesEl.style.display = 'flex';
            this.elements.contentArea.classList.add('visible');
            sourcesEl.innerHTML = renderToolChipsHtml(this.toolUsages) + renderSourceChipsHtml(this.toolSources);
            this.windowManager.resizeWindow();
        }

    renderSourcesCompact() {
            this.elements.loadingDots.classList.remove('visible');
            this.elements.ghostContainer.classList.remove('thinking');

            let compactEl = document.getElementById('toolSourcesCompact');
            if (!compactEl) {
                compactEl = document.createElement('div');
                compactEl.id = 'toolSourcesCompact';
                compactEl.className = 'tool-sources-compact';
                const speechBubble = document.querySelector('.speech-bubble');
                if (speechBubble) speechBubble.insertBefore(compactEl, this.elements.contentArea);
            }

            compactEl.style.display = 'flex';
            compactEl.innerHTML = renderSourceBubblesHtml(this.toolUsages, this.toolSources);
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
