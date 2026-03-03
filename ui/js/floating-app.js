// Main application logic
import { renderShortcutSuggestion, renderShortcutSuggestions, renderUrlSuggestion, renderPathSuggestion, renderSuggestions, updateSelection, appendSendHint } from './floating-suggestions.js';
import { WindowManager } from './floating-window.js';
import { renderMarkdown } from './floating-markdown.js';
import { matchCommands, matchSlashCommands, matchCommandsByName, loadSlashCommands, renderCommandSuggestions, executeCommand } from './floating-commands.js';
import { AttachmentManager, handlePasteEvent, renderAttachmentPreviews } from './attachments.js';
import { processToolCallUpdate, renderToolChipsHtml, renderSourceChipsHtml, renderSourceBubblesHtml, getSessionResetMessage } from './streaming-utils.js';
import { sendAppNotification } from './notify.js';
import { getActionsForText, renderQuickActionChips } from './floating-quick-actions.js';
import { startTimer, startStopwatch, pauseResumeSlot, stopSlot, getSlotState, updateTimerBar, setupTimerBarControls } from './floating-timer.js';
import { playTimerSound } from './timer-sounds.js';
import { unifiedSearch, renderUnifiedResults, recordSelection, loadFrecency, setExtensionManager } from './floating-search-unified.js';
import { ExtensionManager } from './extension-manager.js';

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
        this.computerControlActive = false;
        this._promptGeneration = 0; // incremented each time we send a user message
        this.attachmentManager = new AttachmentManager();
        this.extensionManager = new ExtensionManager(invoke);
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
        await this.extensionManager.initialize();
        setExtensionManager(this.extensionManager);
        await loadFrecency(this.invoke);
        
        // Listen for config updates
        this.listen('config_updated', async () => {
            console.log('Config updated, reloading...');
            await this.loadShortcuts();
            await this.extensionManager.onConfigUpdate();
        });

        // Listen for slash commands from ACP
        this.listen('slash_commands_available', async () => {
            console.log('Slash commands updated, reloading...');
            await loadSlashCommands(this.invoke);
        });
        
        setTimeout(() => this.elements.input.focus(), 100);

        // Check if we were just updated and show the celebration banner
        this.checkForUpdateBanner();

        // Listen for banner events from the backend
        this.listen('show_floating_banner', (event) => {
            const { icon, text, action_label, action_type, action_data } = event.payload;
            this.showBanner(icon, text, action_label, action_type, action_data);
        });

        // Listen for update available events from the background checker
        this.listen('update_available', (event) => {
            const version = event.payload;
            this.showBanner(
                '⬆️',
                'Kiro Assistant v' + version + ' is available!',
                'Install now →',
                'update_install',
                ''
            );
        });
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
            floatingStopBtn: document.getElementById('floatingStopBtn'),
            ghostContainer: document.querySelector('.ghost-container'),
            attachmentPreviews: document.getElementById('attachmentPreviews'),
            datetimeDisplay: document.getElementById('datetimeDisplay')
        };
    }

    /**
     * Single source of truth for datetime visibility.
     * Call this instead of directly manipulating the datetime element.
     */
    updateDatetimeVisibility() {
        const dt = this.elements.datetimeDisplay;
        if (!dt) return;
        // Hide if: streaming, stop button visible, input has text, or quick actions visible
        const stopVisible = this.elements.floatingStopBtn.style.display !== 'none';
        const hasInput = this.elements.input.value.length > 0;
        const qaVisible = document.getElementById('quickActionsContainer')?.style.display === 'flex';
        if (this.isWaitingForResponse || stopVisible || hasInput || qaVisible) {
            dt.style.display = 'none';
        } else {
            dt.style.display = '';
            dt.style.opacity = '1';
        }
    }

    setupEventListeners() {
            this.elements.input.addEventListener('input', (e) => this.handleInputChange(e));
            this.elements.input.addEventListener('keydown', (e) => this.handleKeyDown(e));
            this.elements.expandBtn.addEventListener('click', () => this.handleExpandClick());
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
            const quickActionsContainer = document.getElementById('quickActionsContainer');
            if (hasSelection) {
                try {
                    const raw = await this.invoke('get_last_selection');
                    this.lastSelection = raw?.trim() || null;
                } catch { this.lastSelection = null; }
                if (this.lastSelection) {
                    if (indicator) indicator.style.display = '';
                    if (checkbox) checkbox.checked = true;
                    // Hide datetime to make room for quick actions
                    this.updateDatetimeVisibility();

                    // Show quick action chips based on text content
                    if (quickActionsContainer) {
                        try {
                            const config = await this.invoke('get_config');
                            const qaConfig = config.quick_actions || { enabled: true, custom_actions: [] };
                            const actions = getActionsForText(this.lastSelection, qaConfig);
                            renderQuickActionChips(actions, quickActionsContainer, (promptTemplate) => {
                                const prompt = promptTemplate.replace(/\{text\}/g, this.lastSelection);
                                this.sendChatMessage(prompt, { skipSelection: true });
                            });
                        } catch (e) {
                            console.error('Quick actions error:', e);
                            quickActionsContainer.style.display = 'none';
                        }
                    }

                    this.windowManager.resizeWindow();
                    return;
                }
            }
            this.lastSelection = null;
            if (indicator) indicator.style.display = 'none';
            if (quickActionsContainer) quickActionsContainer.style.display = 'none';
            // Restore datetime and resize back to normal
            this.updateDatetimeVisibility();
            this.windowManager.resizeWindow();
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
                    // Notify updater of activity
                    this.invoke('touch_floating_activity').catch(() => {});
                    // Clear any pending system command confirmations and re-trigger search
                    if (this.currentMatches.some(m => m.type === 'system_confirm')) {
                        const query = this.elements.input.value.trim();
                        if (query) {
                            this.clearSuggestions();
                            // Re-trigger unified search
                            const results = await unifiedSearch(query, this.invoke, this.shortcuts);
                            if (results.length > 0) {
                                this.selectedIndex = renderUnifiedResults(results, this.elements.appSuggestions, this.currentMatches, () => this.windowManager.resizeWindow());
                            }
                        } else {
                            this.clearSuggestions();
                        }
                    }
                    // Don't reset UI if permission modal is open
                    const permissionModal = document.getElementById('permissionModal');
                    if (!permissionModal || permissionModal.style.display === 'none') {
                        // Don't reset if we're waiting for a response
                        if (this.isWaitingForResponse) {
                            // Just focus the input
                            setTimeout(() => this.elements.input.focus(), 50);
                        } else {
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
                // Re-show datetime on window focus if appropriate
                this.updateDatetimeVisibility();
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
            // Don't hide if we just stopped generating (prevents accidental hide on Esc)
            if (this._justStoppedGenerating) {
                return;
            }
            // Don't hide if computer control is active — user needs to track progress
            if (this.computerControlActive) {
                return;
            }
            // Don't hide while waiting for a response
            if (this.isWaitingForResponse) {
                return;
            }
            await this.appWindow.hide();
            this.dismissBanner();
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
        this.updateDatetimeVisibility();
    }

    startThinking() {
        this.elements.ghostContainer.classList.add('thinking');
        this.elements.loadingDots.classList.add('visible');
        // Show inline stop button in input area, hide datetime
        this.updateDatetimeVisibility();
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
        this.elements.floatingStopBtn.style.display = 'none';
        this.updateDatetimeVisibility();
        const indicator = this.elements.responseText.querySelector('.streaming-indicator');
        if (indicator) indicator.remove();
        if (this.currentResponse) {
            renderMarkdown(this.currentResponse, this.elements.responseText);
        } else {
            // No response content — hide the content area entirely
            this.elements.contentArea.classList.remove('visible');
            this.elements.expandBtn.classList.remove('visible');
        }
        this.windowManager.resizeWindow();
        // Tell the agent to abort the current prompt turn
        this.invoke('cancel_generation').catch(e => console.log('Cancel:', e));
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

    _startTimerUI(durationMs) {
        startTimer(durationMs,
            (display, progress) => {
                updateTimerBar('timer', display, progress, true);
            },
            () => this._onTimerComplete()
        );
        setupTimerBarControls('timer', null, () => this.windowManager.resizeWindow());
        // Force resize after bar is in DOM
        setTimeout(() => this.windowManager.resizeWindow(), 60);
    }

    _startStopwatchUI() {
        startStopwatch((display) => {
            updateTimerBar('stopwatch', display, 0, true);
        });
        setupTimerBarControls('stopwatch', null, () => this.windowManager.resizeWindow());
        setTimeout(() => this.windowManager.resizeWindow(), 60);
    }

    async _onTimerComplete() {
        updateTimerBar('timer', '0:00', 1, false);

        let config = {};
        try {
            const fullConfig = await this.invoke('get_config');
            config = (fullConfig.extensions && fullConfig.extensions['timer']) || {};
        } catch {}

        if (config.show_window_on_complete !== false) {
            try {
                const isVisible = await this.appWindow.isVisible();
                if (!isVisible) {
                    await this.appWindow.show();
                    await this.appWindow.setFocus();
                }
            } catch {}
        }

        if (config.notify_on_complete !== false) {
            try {
                await sendAppNotification(this.invoke, 'Timer Complete', '⏱️ Your timer has finished!', 'floating');
            } catch {}
        }

        if (config.sound_on_complete !== false) {
            try {
                playTimerSound(config.sound_id || 'two-tone', config.custom_sound_path || '', config.sound_repeats || 3);
            } catch {}
        }

        // Auto-hide the timer bar after 5 seconds
        setTimeout(() => {
            const s = getSlotState('timer');
            if (!s.active) {
                const bar = document.getElementById('timerBar_timer');
                if (bar) { bar.style.display = 'none'; bar.remove(); }
                this.windowManager.resizeWindow();
            }
        }, 5000);
    }

    async checkForUpdateBanner() {
        try {
            const wasUpdated = await this.invoke('was_just_updated');
            if (wasUpdated) {
                this.showBanner('🎉', 'Kiro Assistant has been updated!', 'View changelog →', 'settings', 'updates');
                // Clear the flag so it only shows once
                this.invoke('clear_update_flag').catch(() => {});
            }
        } catch (e) {
            console.log('Update check failed:', e);
        }
    }

    /**
     * Show a banner at the top of the content area.
     * @param {string} icon - Emoji or text icon
     * @param {string} html - Banner message (supports HTML for keycaps etc.)
     * @param {string} actionLabel - Text for the action hint
     * @param {string} actionType - 'settings', 'url', or 'dismiss'
     * @param {string} actionData - Section name, URL, or ignored
     */
    showBanner(icon, html, actionLabel, actionType, actionData) {
        this._bannerVisible = true;
        this._bannerAction = { type: actionType, data: actionData };
        const banner = document.getElementById('floatingBanner');
        const iconEl = document.getElementById('bannerIcon');
        const textEl = document.getElementById('bannerText');
        const actionEl = document.getElementById('bannerAction');
        const contentArea = document.getElementById('contentArea');
        if (!banner) return;
        if (iconEl) iconEl.textContent = icon || '';
        if (textEl) textEl.innerHTML = html || '';
        if (actionEl) actionEl.textContent = actionLabel || '';
        banner.onclick = () => this.handleBannerClick();
        banner.style.display = 'flex';
        // Ensure the content area is visible so the banner shows
        if (contentArea) contentArea.classList.add('visible');
        // Resize the window to fit the banner after DOM updates
        requestAnimationFrame(() => this.windowManager.resizeWindow());
    }

    handleBannerClick() {
        const action = this._bannerAction;
        this.dismissBanner();
        if (!action) return;
        if (action.type === 'settings') {
            this.invoke('open_settings_window').then(() => {
                window.__TAURI__.event.emit('navigate_settings_section', action.data || 'updates');
            }).catch(() => {});
        } else if (action.type === 'url') {
            this.invoke('open_url', { url: action.data }).catch(() => {});
        } else if (action.type === 'update_install') {
            // Same flow as the "Install Now" button in settings
            this.showBanner('⬇️', 'Downloading and installing update...', '', 'dismiss', '');
            this.invoke('download_and_install_update').catch((e) => {
                this.showBanner('❌', 'Update failed: ' + e, 'Dismiss', 'dismiss', '');
            });
        } else {
            // 'dismiss' — reset the UI and refocus input
            this.resetUI();
            this.windowManager.userSetHeight = null;
            this.windowManager.resizeWindow();
        }
    }

    dismissBanner() {
        if (!this._bannerVisible) return;
        this._bannerVisible = false;
        const banner = document.getElementById('floatingBanner');
        if (banner) banner.style.display = 'none';
        // If the banner was the only content and we're not waiting for a response,
        // reset the UI to reclaim the space
        const responseText = document.getElementById('responseText');
        if (!this.isWaitingForResponse && (!responseText || !responseText.textContent.trim())) {
            this.resetUI();
            this.windowManager.userSetHeight = null;
            this.windowManager.resizeWindow();
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
            // Validate required params before building
            const validation = this._validateShortcutArgs(shortcut, args);
            if (!validation.valid) {
                return { type: 'error', message: validation.message };
            }

            const actionType = shortcut.action_type || 'run_program';

            // Helper: substitute {*}, {0},{1},... and {0?},{1?},... in a template string
            // {N} = required param, {N?} = optional param (replaced with empty string if missing)
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
                    // Replace optional params first {N?}
                    result = result.replace(/\{(\d+)\?\}/g, (_, idx) => {
                        const i = parseInt(idx);
                        const val = i < args.length ? args[i] : '';
                        return encode ? encodeURIComponent(val) : val;
                    });
                    // Replace required params {N}
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

    /**
     * Check if a shortcut has all required parameters provided.
     * Required: {0}, {1}, etc. Optional: {0?}, {1?}, etc.
     * {*} and {selection} don't count as numbered params.
     */
    _validateShortcutArgs(shortcut, args) {
        // Collect all templates that might contain params
        const templates = [
            shortcut.url, shortcut.prompt, shortcut.arguments, shortcut.script
        ].filter(Boolean).join(' ');

        // If template uses {*} or has no numbered params, no validation needed
        if (templates.includes('{*}')) return { valid: true };

        // Find all required params {N} (not {N?})
        const requiredParams = new Set();
        const paramRegex = /\{(\d+)\}/g;
        let match;
        while ((match = paramRegex.exec(templates)) !== null) {
            // Check it's not actually {N?} by looking at the char after
            const fullMatch = templates.substring(match.index, match.index + match[0].length + 1);
            if (!fullMatch.endsWith('?}')) {
                requiredParams.add(parseInt(match[1]));
            }
        }

        if (requiredParams.size === 0) return { valid: true };

        const maxRequired = Math.max(...requiredParams) + 1;
        if (args.length >= maxRequired) return { valid: true };

        const missing = maxRequired - args.length;
        return {
            valid: false,
            message: `This command requires ${maxRequired} parameter${maxRequired > 1 ? 's' : ''} (${missing} missing). Usage: ${shortcut.shortcut} <${Array.from(requiredParams).map(i => 'arg' + i).join('> <')}>`
        };
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
        
        // Update datetime visibility based on input state
        this.updateDatetimeVisibility();
        
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
        
        // Debounced unified search — queries all sources in parallel
        this.searchTimeout = setTimeout(async () => {
            const results = await unifiedSearch(query, this.invoke, this.shortcuts);
            if (results.length > 0) {
                this.selectedIndex = renderUnifiedResults(
                    results,
                    this.elements.appSuggestions,
                    this.currentMatches,
                    () => this.windowManager.resizeWindow()
                );
                // Show send hint for non-instant results
                if (!['color', 'math', 'devtool'].includes(results[0].type)) {
                    appendSendHint(this.elements.appSuggestions);
                    this.windowManager.resizeWindow();
                }
            } else {
                this.clearSuggestions();
            }
        }, 100);
    }

    async clearSuggestions() {
        this.elements.appSuggestions.classList.remove('visible');
        this.currentMatches = [];
        this.selectedIndex = -1;
        await this.windowManager.resizeWindow();
    }

    _renderSystemCommandSuggestion(cmdId, cmdLabel, needsConfirm) {
        const container = this.elements.appSuggestions;
        container.innerHTML = '';
        container.scrollTop = 0;

        const item = document.createElement('div');
        item.className = 'app-suggestion-item selected';

        const canElevate = ['terminal', 'taskmanager', 'filemanager'].includes(cmdId);

        if (needsConfirm) {
            item.innerHTML = `
                <div class="app-icon">⚠️</div>
                <div class="app-info">
                    <div class="app-name">${cmdLabel}</div>
                    <div class="app-description">Press Enter to select</div>
                </div>
            `;
        } else {
            item.innerHTML = `
                <div class="app-icon">${cmdLabel.split(' ')[0]}</div>
                <div class="app-info">
                    <div class="app-name">${cmdLabel.substring(cmdLabel.indexOf(' ') + 1)}</div>
                    <div class="app-description">${canElevate ? 'Enter to run · Ctrl+Shift+Enter as Admin' : 'Press Enter to execute'}</div>
                </div>
            `;
        }

        item.addEventListener('click', () => this._executeSystemCommand(cmdId, needsConfirm, false));
        container.appendChild(item);
        container.classList.add('visible');
        setTimeout(() => this.windowManager.resizeWindow(), 10);
    }

    async _executeSystemCommand(cmdId, needsConfirm, elevated = false) {
        if (needsConfirm) {
            const container = this.elements.appSuggestions;
            container.innerHTML = '';
            const confirmItem = document.createElement('div');
            confirmItem.className = 'app-suggestion-item selected';
            confirmItem.innerHTML = `
                <div class="app-icon">⚠️</div>
                <div class="app-info">
                    <div class="app-name">Are you sure?${elevated ? ' (as Admin)' : ''}</div>
                    <div class="app-description">Press Enter to confirm · Clear text to cancel</div>
                </div>
            `;
            confirmItem.addEventListener('click', async () => {
                try {
                    await this.invoke('execute_system_command', { commandId: cmdId, elevated });
                } catch (e) { console.error('System command failed:', e); }
                this.elements.input.value = '';
                this.elements.input.style.height = 'auto';
                this.clearSuggestions();
            });
            container.appendChild(confirmItem);

            this.currentMatches = [
                { type: 'system_confirm', cmdId, elevated }
            ];
            this.selectedIndex = 0;
            this.windowManager.resizeWindow();
            return;
        }

        try {
            await this.invoke('execute_system_command', { commandId: cmdId, elevated });
        } catch (e) { console.error('System command failed:', e); }
        this.elements.input.value = '';
        this.elements.input.style.height = 'auto';
        this.clearSuggestions();
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
        } else if (event.key === 'Enter' && event.ctrlKey && event.shiftKey) {
            // Ctrl+Shift+Enter: execute as elevated (admin) if it's a system command
            event.preventDefault();
            if (this.currentMatches.length > 0 && this.selectedIndex >= 0) {
                const selected = this.currentMatches[this.selectedIndex];
                if (selected.type === 'system') {
                    await this._executeSystemCommand(selected.cmdId, selected.needsConfirm, true);
                    return;
                }
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
        if (this.currentMatches.length > 0 && this.selectedIndex >= 0 && this.currentMatches[this.selectedIndex].type === 'math') {
            const selected = this.currentMatches[this.selectedIndex];
            const formatted = selected.data?.value || selected.value || selected.label;
            recordSelection(message, selected.id, this.invoke);
            
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
            this.clearSuggestions();
            this.currentResponse = formatted;
            renderMarkdown(`\`= ${formatted}\``, this.elements.responseText);
            this.elements.contentArea.classList.add('visible');
            this.windowManager.resizeWindow();
            
            if (this.extensionManager?._configCache?.extensions?.['math']?.auto_copy !== false) {
                try { await navigator.clipboard.writeText(formatted); } catch {}
            }
            return;
        }

        // Handle color result
        if (this.currentMatches.length > 0 && this.selectedIndex >= 0 && this.currentMatches[this.selectedIndex].type === 'color') {
            const selected = this.currentMatches[this.selectedIndex];
            recordSelection(message, selected.id, this.invoke);
            // Copy based on config format
            let cpFormat = 'all';
            try {
                const cfg = await this.invoke('get_config');
                cpFormat = cfg.extensions?.['color-picker']?.copy_format || 'all';
            } catch {}
            const { r, g, b } = selected.data;
            const hex = '#' + [r,g,b].map(c => c.toString(16).padStart(2,'0')).join('').toUpperCase();
            const rgb = `rgb(${r}, ${g}, ${b})`;
            const text = cpFormat === 'hex' ? hex : cpFormat === 'rgb' ? rgb : cpFormat === 'hsl' ? selected.description : `${hex}\n${rgb}`;
            try { await navigator.clipboard.writeText(text); } catch {}
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
            this.clearSuggestions();
            return;
        }

        // Handle dev tool result
        if (this.currentMatches.length > 0 && this.selectedIndex >= 0 && this.currentMatches[this.selectedIndex].type === 'devtool') {
            const selected = this.currentMatches[this.selectedIndex];
            recordSelection(message, selected.id, this.invoke);
            try { await navigator.clipboard.writeText(selected.data?.value || selected.label); } catch {}
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
            this.clearSuggestions();
            return;
        }

        // Handle timer/stopwatch
        if (this.currentMatches.length > 0 && this.selectedIndex >= 0 && this.currentMatches[this.selectedIndex].type === 'timer_cmd') {
            const selected = this.currentMatches[this.selectedIndex];
            const timerData = selected.data;
            if (timerData.type === 'timer' && timerData.durationMs) {
                this._startTimerUI(timerData.durationMs);
            } else if (timerData.type === 'stopwatch') {
                const sw = getSlotState('stopwatch');
                if (sw.active && sw.running) { pauseResumeSlot('stopwatch'); }
                else if (sw.active && !sw.running) { stopSlot('stopwatch'); const bar = document.getElementById('timerBar_stopwatch'); if (bar) { bar.remove(); } this.windowManager.resizeWindow(); }
                else { this._startStopwatchUI(); }
            }
            // hint type — do nothing on Enter
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
            this.clearSuggestions();
            return;
        }

        // Handle extension results generically (for any type not handled above)
        const _coreTypes = new Set(['math', 'color', 'devtool', 'timer_cmd', 'command', 'slash', 'shortcut', 'system', 'url', 'path', 'app']);
        if (this.currentMatches.length > 0 && this.selectedIndex >= 0 && this.extensionManager) {
            const selected = this.currentMatches[this.selectedIndex];
            if (!_coreTypes.has(selected.type)) {
                const action = this.extensionManager.executeResult(selected);
                if (action) {
                    recordSelection(message, selected.id, this.invoke);
                    if (action.type === 'copy' && action.value) {
                        try { await navigator.clipboard.writeText(action.value); } catch {}
                    } else if (action.type === 'open_url' && action.value) {
                        try { await this.invoke('open_url', { url: action.value }); } catch {}
                    } else if (action.type === 'open_path' && action.value) {
                        try { await this.invoke('open_path', { path: action.value }); } catch {}
                    } else if (action.type === 'send_prompt' && action.value) {
                        this.elements.input.value = action.value;
                    }
                    this.elements.input.value = '';
                    this.elements.input.style.height = 'auto';
                    this.clearSuggestions();
                    return;
                }
            }
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

        // Handle selected suggestion from unified search
        if (this.currentMatches.length > 0 && this.selectedIndex >= 0) {
            const selected = this.currentMatches[this.selectedIndex];
            recordSelection(message, selected.id, this.invoke);

            if (selected.type === 'command' || selected.type === 'slash') {
                await this.executeCommandAction(selected.data || selected);
                return;
            } else if (selected.type === 'selection') {
                await this.executeSelection(selected.data?.command || selected.command, selected.data?.value || selected.value);
                return;
            } else if (selected.type === 'shortcut') {
                const sc = selected.data?.shortcut || selected.shortcut;
                const args = selected.data?.args || selected.args;
                const command = this.buildShortcutCommand(sc, args);
                await this.executeShortcut(command);
                return;
            } else if (selected.type === 'system') {
                const d = selected.data || selected;
                await this._executeSystemCommand(d.cmdId, d.needsConfirm, false);
                return;
            } else if (selected.type === 'system_confirm') {
                const d = selected.data || selected;
                try {
                    await this.invoke('execute_system_command', { commandId: d.cmdId, elevated: d.elevated || false });
                } catch (e) { console.error('System command failed:', e); }
                this.elements.input.value = '';
                this.elements.input.style.height = 'auto';
                this.clearSuggestions();
                return;
            } else if (selected.type === 'url') {
                await this.openUrl(selected.data?.value || selected.value);
                return;
            } else if (selected.type === 'path') {
                await this.openPath(selected.data?.value || selected.value);
                return;
            } else if (selected.type === 'app') {
                await this.launchApp(selected.data?.name || selected.label);
                return;
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
        const useSelection = !options.skipSelection && document.getElementById('useSelectionCheckbox')?.checked;
        if (useSelection && this.lastSelection && this.lastSelection.trim()) {
            message = `The following text is currently selected in my active window:\n\`\`\`\n${this.lastSelection.trim()}\n\`\`\`\n\n${message}`;
        }
        // Hide selection indicator after use
        const indicator = document.getElementById('selectionIndicator');
        if (indicator) indicator.style.display = 'none';
        const quickActionsContainer = document.getElementById('quickActionsContainer');
        if (quickActionsContainer) quickActionsContainer.style.display = 'none';
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
        this.updateDatetimeVisibility();
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
            let rustResults = [];
            if (options.forceChat || attachments) {
                rustResults = [];
            } else if (this._noMatchSinceLen > 0 && message.length >= this._noMatchSinceLen) {
                rustResults = [];
            } else {
                try {
                    const json = await this.invoke('handle_floating_input', { input: message });
                    rustResults = JSON.parse(json);
                } catch { rustResults = []; }
            }
            this._noMatchSinceLen = 0;

            // Check if the top result is a URL, path, or app launch
            const top = rustResults[0];
            if (top?.type === 'url') {
                await this.openUrl(top.value);
                this.stopThinking();
            } else if (top?.type === 'path') {
                await this.openPath(top.value);
                this.stopThinking();
            } else if (top?.type === 'app') {
                await this.launchApp(top.name);
                this.stopThinking();
            } else {
                // No actionable match — send to agent
                this.currentResponse = '';
                this.elements.responseText.textContent = this.currentResponse;
                this.elements.contentArea.classList.add('visible');
                this.elements.expandBtn.classList.add('visible');
                this.isWaitingForResponse = true;
                this._promptGeneration++;
                const gen = this._promptGeneration;
                await this.windowManager.resizeWindow();
                this.dismissBanner();
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
            // Ensure content area is visible (safety net if something else hid it)
            this.elements.contentArea.classList.add('visible');
            this.elements.expandBtn.classList.add('visible');
            
            // Transition compact sources to full (bottom) layout
            const compactEl = document.getElementById('toolSourcesCompact');
            if (compactEl) {
                compactEl.remove();
                if (this.toolSources.length > 0 || this.toolUsages.length > 0) {
                    this.renderSources();
                }
            }
        }
        
        renderMarkdown(this.currentResponse, this.elements.responseText, true);

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

            // Ignore stale completions (e.g., steering response arriving after user sent a message)
            if (!this.currentResponse || this.currentResponse.trim().length === 0) {
                return;
            }

            this.stopThinking();
            this.computerControlActive = false;
            this.elements.floatingStopBtn.style.display = 'none';
            // Restore datetime display
            this.updateDatetimeVisibility();
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
        this.computerControlActive = false;
        this.elements.floatingStopBtn.style.display = 'none';
        // Restore datetime display
        this.updateDatetimeVisibility();
    }

    handleSessionReset(event) {
            this.isWaitingForResponse = false;
            this.elements.floatingStopBtn.style.display = 'none';
            // Restore datetime display
            this.updateDatetimeVisibility();
            this.showError(getSessionResetMessage(event.payload));
        }

    handleToolCallUpdate(event) {
            if (!this.isWaitingForResponse) return;
            const { updated, update } = processToolCallUpdate(event, this);

            // Detect computer-control tool usage and keep window visible
            if (update?.title) {
                const ccTools = ['screenshot', 'click', 'double_click', 'right_click',
                    'move_mouse', 'drag', 'scroll', 'type_text', 'key_press',
                    'key_press_confirmed', 'launch_app', 'wait', 'get_screen_size',
                    'get_cursor_position'];
                if (ccTools.includes(update.title)) {
                    this.computerControlActive = true;
                }
            }

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

        // Don't hide if we just finished resizing or dragging — the mouseup
        // outside the window boundary fires a click event we should ignore.
        if (this.windowManager.isResizing || this.windowManager.isDragging) return;
        if (this.windowManager._resizeEndedAt && Date.now() - this.windowManager._resizeEndedAt < 300) return;
        
        const container = document.querySelector('.floating-container');
        if (container && !container.contains(event.target)) {
            await this.appWindow.hide();
        }
    }
}
