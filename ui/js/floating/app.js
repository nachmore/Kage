// Main application logic
import { renderShortcutSuggestion, renderShortcutSuggestions, renderUrlSuggestion, renderPathSuggestion, renderSuggestions, updateSelection, appendSendHint } from './suggestions.js';
import { WindowManager } from './window.js';
import { renderMarkdown, createTaskPlanElement } from '../shared/markdown.js';
import { matchCommands, matchSlashCommands, matchCommandsByName, loadSlashCommands, renderCommandSuggestions, executeCommand } from '../shared/commands.js';
import { AttachmentManager, handlePasteEvent, renderAttachmentPreviews } from '../shared/attachments.js';
import { processToolCallUpdate, renderToolChipsHtml, renderSourceChipsHtml, renderSourceBubblesHtml, getSessionResetMessage, detectAutomationPlan, detectAutomationPlanIncremental, automationPlanToTasks, detectExtensionToolCall, detectExtensionToolCallIncremental, extractSuggestedActions } from '../shared/streaming-utils.js';
import { sendAppNotification } from '../shared/notify.js';
import { getActionsForText, renderQuickActionChips } from '../shared/quick-actions.js';
import { startTimer, startStopwatch, pauseResumeSlot, stopSlot, getSlotState, updateTimerBar, setupTimerBarControls } from './timer.js';
import { playTimerSound } from '../shared/timer-sounds.js';
import { unifiedSearch, renderUnifiedResults, recordSelection, loadFrecency, setExtensionManager } from './search-unified.js';
import { ExtensionManager } from '../shared/extension-manager.js';
import { SpeechController } from '../shared/speech.js';
import { matchShortcut as matchShortcutFn, buildShortcutCommand as buildShortcutCommandFn } from '../shared/shortcuts.js';
import { isClipboardTrigger, getClipboardFilter, fetchClipboardHistory, filterClipboardHistory, renderClipboardHistory } from './clipboard-history.js';
import { executeResult as executeResultShared, executeShortcutCommand, handleEnterAction } from '../shared/result-executor.js';
import { setupRtlDetection } from '../shared/rtl.js';

export class FloatingApp {
    constructor(invoke, appWindow, listen) {
        this.invoke = invoke;
        this.appWindow = appWindow;
        this.listen = listen;
        this.windowManager = new WindowManager(invoke);
        
        this.currentMatches = [];
        this.selectedIndex = -1;
        this.searchTimeout = null;
        this._searchGeneration = 0;
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

        // RTL detection — flip input container layout when first char is RTL
        const inputContainer = this.elements.input?.closest('.input-container');
        setupRtlDetection(this.elements.input, inputContainer, this.elements.responseText);
        
        await this.loadShortcuts();
        await loadSlashCommands(this.invoke);
        await this.extensionManager.initialize();
        setExtensionManager(this.extensionManager);
        await loadFrecency(this.invoke);
        this.setupSpeech();

        // Send extension tool definitions to the agent as steering
        this._sendExtensionToolSteering();
        
        // Listen for config updates
        this.listen('config_updated', async () => {
            console.log('Config updated, reloading...');
            await this.loadShortcuts();
            await this.extensionManager.onConfigUpdate();
            await this.extensionManager.reload();
            this.updateSpeechButtonVisibility();
        });

        // Listen for extension install/uninstall
        this.listen('extensions_changed', async () => {
            console.log('Extensions changed, reloading...');
            await this.extensionManager.reload();
        });

        // Listen for slash commands from ACP
        this.listen('slash_commands_available', async () => {
            console.log('Slash commands updated, reloading...');
            await loadSlashCommands(this.invoke);
        });

        // Listen for clipboard history hotkey
        this.listen('clipboard_history_mode', async () => {
            console.log('Clipboard history mode activated via hotkey');
            this.elements.input.value = '>cb ';
            this._enterClipboardMode();
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
            datetimeDisplay: document.getElementById('datetimeDisplay'),
            speechBtn: document.getElementById('speechBtn'),
            speechWave: document.getElementById('speechWave')
        };
    }

    /**
     * Single source of truth for datetime visibility.
     * Call this instead of directly manipulating the datetime element.
     */
    updateDatetimeVisibility() {
        const dt = this.elements.datetimeDisplay;
        if (!dt) return;
        // Hide if: streaming, stop button visible, input has text, quick actions visible, or speech listening
        const stopVisible = this.elements.floatingStopBtn.style.display !== 'none';
        const hasInput = this.elements.input.value.length > 0;
        const qaVisible = document.getElementById('quickActionsContainer')?.style.display === 'flex'
            || document.getElementById('responseActionsContainer')?.style.display === 'flex';
        const dtHidden = this.isWaitingForResponse || stopVisible || hasInput || qaVisible || this.speech?.isListening;
        if (dtHidden) {
            dt.style.display = 'none';
        } else {
            dt.style.display = '';
            dt.style.opacity = '1';
        }
        // Position speech button: to the left of datetime when visible, or at right edge
        // Hide speech button when stop button is showing (generating response)
        if (this.elements.speechBtn) {
            if (stopVisible) {
                this.elements.speechBtn.style.display = 'none';
            } else {
                // Re-show if config says so (updateVisibility sets the base display)
                // Only restore if it was hidden by us, not by config
                if (this.elements.speechBtn.dataset.configVisible === 'true') {
                    this.elements.speechBtn.style.display = '';
                }
                if (this.elements.speechBtn.style.display !== 'none') {
                    if (!dtHidden && dt.style.display !== 'none') {
                        const dtWidth = dt.offsetWidth || 60;
                        this.elements.speechBtn.style.right = (dtWidth + 18) + 'px';
                    } else {
                        this.elements.speechBtn.style.right = '10px';
                    }
                }
            }
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
                // Escape — stop speech/TTS first, then stop generating, then hide
                if (e.key === 'Escape') {
                    // Stop speech recognition or TTS first
                    if (this.speech?.isActive) {
                        e.preventDefault();
                        this.speech.stop();
                        this.speech.cancelSpeech();
                        return;
                    }
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
        this.appWindow.listen('tauri://focus', async () => {
            // Notify updater of activity
            this.invoke('touch_floating_activity').catch(() => {});

            // Restore any overlays hidden by clipboard mode
            if (!this._clipboardMode) {
                this._restoreOverlaysAfterClipboard();
            }

            // Clear any pending system command confirmations and re-trigger search
            if (this.currentMatches.some(m => m.type === 'system_confirm')) {
                const query = this.elements.input.value.trim();
                if (query) {
                    this.clearSuggestions();
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
                if (this.isWaitingForResponse) {
                    setTimeout(() => this.elements.input.focus(), 50);
                } else {
                    try {
                        const config = await this.invoke('get_config');
                        if (config.ui?.preserve_last_response === false) {
                            setTimeout(() => this.resetUI(), 50);
                        } else {
                            setTimeout(() => {
                                this.elements.input.focus();
                                if (!this._clipboardMode) this.elements.input.select();
                            }, 50);
                        }
                    } catch (e) {
                        setTimeout(() => {
                            this.elements.input.focus();
                            if (!this._clipboardMode) this.elements.input.select();
                        }, 50);
                    }
                }
            }

            this.updateDatetimeVisibility();
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
            // Hide response quick actions — if user didn't use them, they're stale
            const responseActions = document.getElementById('responseActionsContainer');
            if (responseActions) responseActions.style.display = 'none';
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
        const responseActions = document.getElementById('responseActionsContainer');
        if (responseActions) responseActions.style.display = 'none';
        const floatingActions = document.getElementById('floatingResponseActions');
        if (floatingActions) floatingActions.style.display = 'none';
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

        // If an automation plan is running, stop it gracefully
        if (this._automationPlanStarted && this._automationStatuses) {
            for (const [step, status] of Object.entries(this._automationStatuses)) {
                if (status === 'running') {
                    this._automationStatuses[step] = 'stopped';
                }
            }
            this._renderAutomationPlan();
            if (this._automationCleanup) this._automationCleanup();
            this._automationPlanStarted = false;
            this.computerControlActive = false;
        }

        this.isWaitingForResponse = false;
        this._justStoppedGenerating = true;
        setTimeout(() => { this._justStoppedGenerating = false; }, 300);
        this.stopThinking();
        this.elements.floatingStopBtn.style.display = 'none';
        this.updateDatetimeVisibility();
        const indicator = this.elements.responseText.querySelector('.streaming-indicator');
        if (indicator) indicator.remove();

        // Don't overwrite the plan UI with markdown
        if (!this._automationPlan) {
            if (this.currentResponse) {
                renderMarkdown(this.currentResponse, this.elements.responseText);
            } else {
                this.elements.contentArea.classList.remove('visible');
                this.elements.expandBtn.classList.remove('visible');
            }
        }

        this.windowManager.resizeWindow();
        this.invoke('cancel_generation').catch(e => console.log('Cancel:', e));
    }

    // --- Speech ---

    async updateSpeechButtonVisibility() {
        await this.speech.updateVisibility();
    }

    setupSpeech() {
        this.speech = new SpeechController({
            invoke: this.invoke,
            elements: {
                input: this.elements.input,
                speechBtn: this.elements.speechBtn,
                speechWave: this.elements.speechWave
            },
            onSend: (text) => this.sendChatMessage(text),
            onVisibilityUpdate: () => this.updateDatetimeVisibility(),
            barContainer: document.querySelector('.input-container'),
        });
        this.speech.setup();
    }

    // Convenience accessors used by Escape handler and sendChatMessage
    get isSpeechListening() { return this.speech?.isListening ?? false; }
    get _usedSpeechForLastMessage() { return this.speech?.usedSpeechForLastMessage ?? false; }
    set _usedSpeechForLastMessage(v) { if (this.speech) this.speech.usedSpeechForLastMessage = v; }

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
        return matchShortcutFn(input, this.shortcuts);
    }

    buildShortcutCommand(shortcut, args) {
        const useSelection = document.getElementById('useSelectionCheckbox')?.checked;
        const sel = useSelection && this.lastSelection ? this.lastSelection : '';
        return buildShortcutCommandFn(shortcut, args, sel);
    }

    /** Build execution context for the shared result executor. */
    _getExecCtx() {
        return {
            invoke: this.invoke,
            appWindow: this.appWindow,
            extensionManager: this.extensionManager,
            selectionText: document.getElementById('useSelectionCheckbox')?.checked ? this.lastSelection || '' : '',
            onPrompt: (text) => this.sendChatMessage(text),
            onDisplay: (text) => {
                this.currentResponse = text;
                renderMarkdown(text, this.elements.responseText);
                this.elements.contentArea.classList.add('visible');
                this.windowManager.resizeWindow();
            },
            onCopy: async (text) => { try { await navigator.clipboard.writeText(text); } catch {} },
            onTimerStart: (ms) => this._startTimerUI(ms),
            onStopwatch: () => {
                const sw = getSlotState('stopwatch');
                if (sw.active && sw.running) { pauseResumeSlot('stopwatch'); }
                else if (sw.active && !sw.running) { stopSlot('stopwatch'); const bar = document.getElementById('timerBar_stopwatch'); if (bar) { bar.remove(); } this.windowManager.resizeWindow(); }
                else { this._startStopwatchUI(); }
            }
        };
    }

    async executeShortcut(command) {
        try {
            const result = await executeShortcutCommand(command, this._getExecCtx());
            if (result.action === 'hide') {
                this.resetUI();
                await this.appWindow.hide();
            }
            this._clearInput();
        } catch (error) {
            console.error('Failed to execute shortcut:', error);
            this.showError('Failed to execute shortcut: ' + error);
        }
    }

    async handleInputChange(event) {
        const rawQuery = this.elements.input.value;
        const query = rawQuery.trim();
        
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
        // Use a longer debounce for file search patterns to avoid unnecessary disk queries
        const looksLikeFileSearch = /\.\w{0,6}$/.test(query) || query.includes('*') || query.includes('?') || query.toLowerCase().startsWith('>find ');
        const debounceMs = looksLikeFileSearch ? 250 : 100;
        this._searchGeneration++;
        const gen = this._searchGeneration;
        this.searchTimeout = setTimeout(async () => {
            // Check for clipboard history trigger
            if (isClipboardTrigger(query)) {
                const filter = getClipboardFilter(query);
                if (!this._clipboardMode) {
                    await this._enterClipboardMode(filter);
                } else {
                    await this._updateClipboardFilter(filter);
                }
                return;
            }
            if (this._clipboardMode) this._restoreOverlaysAfterClipboard();
            this._clipboardMode = false;

            const results = await unifiedSearch(rawQuery, this.invoke, this.shortcuts);
            // Discard stale results — a newer search was started while this one was in-flight
            if (gen !== this._searchGeneration) return;
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
        }, debounceMs);
    }

    async clearSuggestions() {
        this.elements.appSuggestions.classList.remove('visible');
        this.currentMatches = [];
        this.selectedIndex = -1;
        if (this._clipboardMode) this._restoreOverlaysAfterClipboard();
        this._clipboardMode = false;
        await this.windowManager.resizeWindow();
    }

    /** Enter clipboard history mode — fetch and display history */
    async _enterClipboardMode(filter = '') {
        this._clipboardMode = true;
        this._hideOverlaysForClipboard();
        const entries = await fetchClipboardHistory(this.invoke);
        const filtered = filterClipboardHistory(entries, filter);
        this._clipboardEntries = entries; // Cache for filtering
        this.selectedIndex = renderClipboardHistory(
            filtered,
            this.elements.appSuggestions,
            this.currentMatches,
            () => this.windowManager.resizeWindow()
        );
    }

    /**
     * Hide banners, calendar overlay, and timer bars while clipboard mode is active.
     */
    _hideOverlaysForClipboard() {
        document.body.classList.add('clipboard-mode');
    }

    /**
     * Restore overlays that were hidden for clipboard mode.
     */
    _restoreOverlaysAfterClipboard() {
        document.body.classList.remove('clipboard-mode');
    }

    /** Update clipboard history filter (called on input change in clipboard mode) */
    async _updateClipboardFilter(filter) {
        if (!this._clipboardEntries) return;
        const filtered = filterClipboardHistory(this._clipboardEntries, filter);
        this.selectedIndex = renderClipboardHistory(
            filtered,
            this.elements.appSuggestions,
            this.currentMatches,
            () => this.windowManager.resizeWindow()
        );
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
                this._clearInput();
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
        this._clearInput();
    }

    async executeCommandAction(cmd) {
        this._clearInput();
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
            const itemCount = this.elements.appSuggestions.querySelectorAll('.app-suggestion-item').length;
            if (itemCount > 0) {
                event.preventDefault();
                this.selectedIndex = (this.selectedIndex + 1) % itemCount;
                updateSelection(this.elements.appSuggestions, this.selectedIndex);
            }
            // When no suggestions, let the default behavior handle cursor movement in textarea
        } else if (event.key === 'ArrowUp') {
            const itemCount = this.elements.appSuggestions.querySelectorAll('.app-suggestion-item').length;
            if (itemCount > 0) {
                event.preventDefault();
                this.selectedIndex = this.selectedIndex <= 0 ? itemCount - 1 : this.selectedIndex - 1;
                updateSelection(this.elements.appSuggestions, this.selectedIndex);
            }
            // When no suggestions, let the default behavior handle cursor movement in textarea
        } else if (event.key === 'Escape') {
            if (this._clipboardMode) {
                event.preventDefault();
                this._restoreOverlaysAfterClipboard();
                this._clipboardMode = false;
                this._clipboardEntries = null;
                this._clearInput();
                return;
            }
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
        const hasSelection = this.currentMatches.length > 0 && this.selectedIndex >= 0;

        // Clipboard history mode — paste selected item into the previously focused app
        if (this._clipboardMode && hasSelection) {
            const selected = this.currentMatches[this.selectedIndex];
            if (selected.type === 'clipboard' && selected.data?.text) {
                this._clearInput();
                await this.appWindow.hide();
                // Small delay to let the previous window regain focus
                await new Promise(r => setTimeout(r, 150));
                try {
                    await this.invoke('paste_clipboard_item', { text: selected.data.text });
                    console.log('[Clipboard] Pasted to active app:', selected.data.text.slice(0, 50));
                } catch (e) {
                    console.warn('[Clipboard] Failed to paste:', e);
                }
                return;
            }
        }

        if (!message && !hasAttachments && !hasSelection) return;
        
        if (this.isWaitingForResponse) {
            this.stopThinking();
            this.isWaitingForResponse = false;
        }

        const result = await handleEnterAction({
            message,
            suggestions: this.currentMatches,
            selectedIndex: this.selectedIndex,
            shortcuts: this.shortcuts,
            ctx: this._getExecCtx(),
            onSend: (msg) => this.sendChatMessage(msg),
            onSystemCommand: (cmdId, needsConfirm, elevated) => this._executeSystemCommand(cmdId, needsConfirm, elevated),
            onSelection: (command, value) => this.executeSelection(command, value),
        });

        if (result.handled) {
            if (result.action === 'hide') { this.resetUI(); await this.appWindow.hide(); }
            else { this._clearInput(); }
        }
    }

    _clearInput() {
        this.elements.input.value = '';
        this.elements.input.style.height = 'auto';
        this.clearSuggestions();
    }

    async sendChatMessage(message, options = {}) {
        // Stop any ongoing TTS and speech recognition
        if (this.speech) {
            this.speech.cancelSpeech();
            if (this.speech.isListening) this.speech.stop();
        }

        // If a plan is pending review, send the message as a revision request
        if (this._pendingPlanRevision) {
            this._pendingPlanRevision = null;
            this._automationPlanStarted = false;
            // Reset UI for the new response
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
            this.currentResponse = '';
            this.elements.responseText.textContent = '';
            this.elements.contentArea.classList.add('visible');
            this.isWaitingForResponse = true;
            this._extensionToolCallHandled = false;
            this._extensionToolExecuting = false;
            this._promptGeneration++;
            this.startThinking();
            this.updateDatetimeVisibility();
            await this.windowManager.resizeWindow();
            try {
                await this.invoke('send_message_streaming', { message, attachments: null });
            } catch (e) {
                this.showError('Error: ' + e);
            }
            return;
        }

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
        const responseActionsContainer = document.getElementById('responseActionsContainer');
        if (responseActionsContainer) responseActionsContainer.style.display = 'none';
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
                this._extensionToolCallHandled = false;
                this._extensionToolExecuting = false;
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

        // Detect complete automation plan during streaming — show for review
        if (!this._automationPlanStarted) {
            const completePlan = detectAutomationPlan(this.currentResponse);
            if (completePlan) {
                this._automationPlanStarted = true;
                this._showPlanForReview(completePlan);
                return;
            }

            // Show tasks incrementally as they stream in
            const partialPlan = detectAutomationPlanIncremental(this.currentResponse);
            if (partialPlan) {
                this._automationPlan = partialPlan;
                this._automationStatuses = {};
                this._automationResults = {};
                for (const s of partialPlan) this._automationStatuses[s.step] = 'pending';
                this._renderAutomationPlan();
                this.windowManager.resizeWindow();
                return;
            }
        }

        // If automation plan is running, don't overwrite the plan UI
        if (this._automationPlanStarted) return;

        // Detect extension tool calls in streaming text
        if (!this._extensionToolCallHandled) {
            const toolCall = detectExtensionToolCall(this.currentResponse);
            if (toolCall) {
                this._extensionToolCallHandled = true;
                this._handleExtensionToolCall(toolCall);
                return;
            }

            // Show loading indicator while fence is being streamed
            const partial = detectExtensionToolCallIncremental(this.currentResponse);
            if (partial) {
                this._renderExtensionToolIndicator(partial);
                this.windowManager.resizeWindow();
                return;
            }
        } else if (!this._extensionToolExecuting) {
            // Tool call was handled and execution finished — if the new accumulated
            // text no longer contains the fence, the accumulator was reset for the
            // follow-up response. Clear the flag so rendering proceeds normally.
            if (!this.currentResponse.includes('```extension_tool_call')) {
                this._extensionToolCallHandled = false;
            }
        }

        // If extension tool is executing, don't overwrite the indicator
        if (this._extensionToolExecuting) return;
        
        renderMarkdown(this.currentResponse, this.elements.responseText, true);

        // Feed streaming text to TTS for sentence-chunked playback
        if (this.speech) this.speech.feedStreamingText(this.currentResponse);

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

            // If automation plan is running, don't overwrite the plan UI
            if (this._automationPlanStarted) return;

            // If extension tool is executing, the response will come as a follow-up
            if (this._extensionToolExecuting || this._extensionToolCallHandled) return;

            // Check for extension tool call in the completed response (fallback if not caught during streaming)
            if (!this._extensionToolCallHandled) {
                const toolCall = detectExtensionToolCall(this.currentResponse);
                if (toolCall) {
                    this._extensionToolCallHandled = true;
                    this._handleExtensionToolCall(toolCall);
                    return;
                }
            }

            this.stopThinking();
            this.computerControlActive = false;
            this.elements.floatingStopBtn.style.display = 'none';
            // Restore datetime display
            this.updateDatetimeVisibility();
            const streamingIndicator = this.elements.responseText.querySelector('.streaming-indicator');
            if (streamingIndicator) streamingIndicator.remove();

            // Check for automation plan in the response (fallback if not caught during streaming)
            const plan = detectAutomationPlan(this.currentResponse);
            if (plan && !this._automationPlanStarted) {
                this._automationPlanStarted = true;
                this._showPlanForReview(plan);
                return;
            }

            // Wait a tick to ensure all pending message_chunk events have been processed
            // before doing the final render. The message_complete event can arrive
            // before the last few chunk events are dispatched.
            await new Promise(r => setTimeout(r, 50));

            renderMarkdown(this.currentResponse, this.elements.responseText);
            await this.windowManager.resizeWindow();
            this.isWaitingForResponse = false;

            // Show response action buttons (copy, speak)
            this._showFloatingResponseActions();

            // Check for agent-suggested actions (hidden fence at end of response)
            const suggested = extractSuggestedActions(this.currentResponse);
            if (suggested && suggested.actions.length > 0) {
                this._renderSuggestedActions(suggested.actions);
            }

            // Show quick action chips on the response
            this._showResponseActions(this.currentResponse);

            // Read back response if speech was used
            if (this.speech) {
                this.speech.finishStreamingText(this.currentResponse);
                this.speech.speakResponse(this.currentResponse);
            }

            // Notify if window is hidden
            try {
                const isVisible = await this.appWindow.isVisible();
                if (!isVisible && this.currentResponse) {
                    const preview = this.currentResponse.substring(0, 100).replace(/[#*`\n]/g, ' ').trim();
                    await sendAppNotification(this.invoke, 'Kiro Assistant', preview || 'Response ready', 'floating');
                }
            } catch { /* ignore */ }
        }

    /**
     * Send extension tool definitions to the agent as a steering message.
     * Called after extensions are loaded and after config updates.
     */
    async _sendExtensionToolSteering() {
        const block = this.extensionManager.buildToolSteeringBlock();
        if (!block) return;
        try {
            await this.invoke('send_extension_tool_steering', { toolSteering: block });
        } catch (e) {
            console.warn('Failed to send extension tool steering:', e);
        }
    }

    /**
     * Render a loading indicator while an extension tool call is being streamed or executed.
     * Clears the response area so the incomplete fence isn't shown as a code block,
     * and keeps the thinking dots visible.
     */
    _renderExtensionToolIndicator(info) {
        // Only add to tool usages if we have the full extension/tool name
        if (info.extension && info.tool) {
            const toolTitle = `ext:${info.extension}/${info.tool}`;
            if (!this.toolUsages.find(t => t.title === toolTitle)) {
                this.toolUsages.push({ toolCallId: `ext-${info.extension}-${info.tool}`, title: toolTitle, kind: 'extension' });
            }
            this.renderSources();
        }

        // Show any text before the fence, but hide the fence itself
        const beforeFence = this.currentResponse.split('```extension_tool_call')[0].trim();
        if (beforeFence) {
            renderMarkdown(beforeFence, this.elements.responseText, true);
        } else {
            // Nothing before the fence — keep the response area clean so thinking dots show
            this.elements.responseText.innerHTML = '';
        }
    }

    /**
     * Get the icon for an extension by ID.
     */
    _getExtensionIcon(extensionId) {
        if (!extensionId || !this.extensionManager) return '🧩';
        const defs = this.extensionManager.getToolDefinitions();
        const def = defs.find(d => d.extensionId === extensionId);
        return def?.extensionIcon || '🧩';
    }

    /**
     * Handle a detected extension tool call: check permissions, execute, send result back.
     */
    async _handleExtensionToolCall(toolCall) {
        const { extension, tool, params } = toolCall;
        const icon = this._getExtensionIcon(extension);
        const toolTitle = `ext:${extension}/${tool}`;

        console.log(`Extension tool call: ${extension}/${tool}`, params);

        // Track as a standard tool usage
        if (!this.toolUsages.find(t => t.title === toolTitle)) {
            this.toolUsages.push({ toolCallId: `ext-${extension}-${tool}`, title: toolTitle, kind: 'extension' });
        }
        this.renderSources();

        // Check permission policy
        let policy;
        try {
            policy = await this.invoke('check_extension_tool_permission', {
                extensionId: extension,
                toolName: tool,
            });
        } catch (e) {
            console.error('Failed to check extension tool permission:', e);
            policy = 'ask';
        }

        if (policy === 'deny') {
            this._extensionToolExecuting = false;
            this._extensionToolCallHandled = false;
            try {
                await this.invoke('extension_tool_response', {
                    extensionId: extension,
                    toolName: tool,
                    resultJson: JSON.stringify('Permission denied by user policy'),
                    success: false,
                });
            } catch (e) {
                console.error('Failed to send denial:', e);
            }
            return;
        }

        if (policy === 'ask') {
            const allowed = await window.PermissionModal.showForExtensionTool(extension, tool, icon);
            if (!allowed) {
                this._extensionToolExecuting = false;
                this._extensionToolCallHandled = false;
                try {
                    await this.invoke('extension_tool_response', {
                        extensionId: extension,
                        toolName: tool,
                        resultJson: JSON.stringify('Permission denied by user'),
                        success: false,
                    });
                } catch (e) {
                    console.error('Failed to send denial:', e);
                }
                return;
            }
        }

        // Execute the tool
        this._extensionToolExecuting = true;

        // Hide stop button while tool is executing — the tool may show its own UI
        // (e.g. folder plan confirmation with Run/Cancel buttons)
        this.stopThinking();
        this.elements.floatingStopBtn.style.display = 'none';
        this.updateDatetimeVisibility();

        const result = await this.extensionManager.executeExtensionTool(extension, tool, params);
        const success = !result.error;
        const resultJson = JSON.stringify(success ? result.result : result.error);

        try {
            await this.invoke('extension_tool_response', {
                extensionId: extension,
                toolName: tool,
                resultJson,
                success,
            });
        } catch (e) {
            console.error('Failed to send extension tool response:', e);
        }

        this._extensionToolExecuting = false;
        // Reset the handled flag so the next message_complete is processed normally.
        // Also hide the stop button — the follow-up response's handleMessageComplete
        // may have already fired while the tool was executing and been skipped.
        this._extensionToolCallHandled = false;

        // Show thinking dots while waiting for the agent's follow-up response
        this.startThinking();
        this.updateDatetimeVisibility();
    }

    /**
     * Show an automation plan for user review before execution.
     * Renders the task list with Run/Edit action buttons.
     */
    _showPlanForReview(plan) {
        this._automationPlan = plan;
        this._automationStatuses = {};
        this._automationResults = {};
        for (const s of plan) this._automationStatuses[s.step] = 'pending';
        this._renderAutomationPlan();

        // Add review action bar below the plan
        const actionsBar = document.createElement('div');
        actionsBar.className = 'taskplan-review-actions';
        actionsBar.innerHTML = `
            <button class="taskplan-review-btn taskplan-run-btn" id="planRunBtn">▶ Run</button>
            <span class="taskplan-review-hint">or type to revise the plan</span>
        `;
        this.elements.responseText.appendChild(actionsBar);

        // Stop thinking state — plan is ready for review
        this.stopThinking();
        this.elements.floatingStopBtn.style.display = 'none';
        this.updateDatetimeVisibility();
        this.isWaitingForResponse = false;
        this.windowManager.resizeWindow();

        // Run button handler
        const runBtn = document.getElementById('planRunBtn');
        if (runBtn) {
            runBtn.addEventListener('mousedown', (e) => e.preventDefault());
            runBtn.addEventListener('click', (e) => {
                e.stopPropagation(); // Prevent handleOutsideClick from hiding the window
                actionsBar.remove();
                this._pendingPlanRevision = null;
                this.isWaitingForResponse = true;
                this._executeAutomationPlan(plan);
            });
        }

        // Focus input so user can type to revise
        this.elements.input.focus();

        // Override the next send to handle plan revision
        this._pendingPlanRevision = plan;
    }

    async _executeAutomationPlan(plan) {
        // Render the plan as a task list immediately
        this._automationPlan = plan;
        this._automationStatuses = {};
        this._automationResults = {};
        for (const s of plan) this._automationStatuses[s.step] = 'pending';
        this._renderAutomationPlan();
        await this.windowManager.resizeWindow();

        // Show stop button
        this.elements.floatingStopBtn.style.display = '';
        this.updateDatetimeVisibility();

        // Store cleanup so stopGenerating can tear down the plan
        this._automationCleanup = null;

        // Listen for step progress events
        const stepStartUnlisten = await this.listen('automation_step_start', (event) => {
            const { step } = event.payload;
            this._automationStatuses[step] = 'running';
            this._renderAutomationPlan();
            this.windowManager.resizeWindow();
        });

        const stepCompleteUnlisten = await this.listen('automation_step_complete', (event) => {
            const { step, success, result } = event.payload;
            this._automationStatuses[step] = success ? 'done' : 'failed';
            if (result) this._automationResults[step] = result.substring(0, 200);
            this._renderAutomationPlan();
            this.windowManager.resizeWindow();
        });

        const cleanup = () => {
            stepStartUnlisten();
            stepCompleteUnlisten();
            planCompleteUnlisten();
            this._automationCleanup = null;
        };
        this._automationCleanup = cleanup;

        const planCompleteUnlisten = await this.listen('automation_plan_complete', async () => {
            cleanup();
            this._automationPlanStarted = false;
            this.isWaitingForResponse = false;
            this.elements.floatingStopBtn.style.display = 'none';
            this.stopThinking();
            this.computerControlActive = false;
            this.updateDatetimeVisibility();
            this._showFloatingResponseActions();
            await this.windowManager.resizeWindow();
        });

        // Execute the plan
        try {
            await this.invoke('execute_automation_plan', {
                planJson: JSON.stringify(plan)
            });
        } catch (e) {
            console.error('Automation plan execution failed:', e);
            this.showError('Automation failed: ' + e);
            cleanup();
            this._automationPlanStarted = false;
            this.isWaitingForResponse = false;
        }
    }

    _renderAutomationPlan() {
        if (!this._automationPlan) return;
        const tasks = automationPlanToTasks(
            this._automationPlan,
            this._automationStatuses || {},
            this._automationResults || {}
        );
        const wrapper = createTaskPlanElement(tasks);
        this.elements.responseText.innerHTML = '';
        this.elements.responseText.appendChild(wrapper);
        this.elements.contentArea.classList.add('visible');
        this.elements.expandBtn.classList.add('visible');
    }

    _showFloatingResponseActions() {
        const bar = document.getElementById('floatingResponseActions');
        if (!bar) return;
        bar.style.display = 'flex';

        const copyBtn = document.getElementById('floatingCopyBtn');
        const speakBtn = document.getElementById('floatingSpeakBtn');

        // Show speak button if TTS is available
        if (speakBtn) {
            const hasTts = this.speech?.pocketTtsEnabled || this.speech?.readBack;
            speakBtn.style.display = hasTts ? '' : 'none';
        }

        // Wire copy
        if (copyBtn) {
            copyBtn.onclick = () => {
                const text = this.currentResponse || '';
                navigator.clipboard.writeText(text).then(() => {
                    copyBtn.innerHTML = '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>';
                    setTimeout(() => { copyBtn.innerHTML = '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>'; }, 1500);
                });
            };
        }

        // Wire speak
        if (speakBtn) {
            speakBtn.onclick = () => {
                if (this.speech && this.currentResponse) {
                    // Stop any existing speech before starting new one
                    this.speech.cancelSpeech();
                    this.speech.usedSpeechForLastMessage = true;
                    this.speech.speakResponse(this.currentResponse);
                }
            };
        }
    }

    async _showResponseActions(responseText) {
        console.log('[QA] _showResponseActions called, text length:', responseText?.length);
        if (!responseText?.trim()) return;
        try {
            const config = await this.invoke('get_config');
            if (!config.ui?.show_response_actions) return;
            const qaConfig = config.quick_actions || { enabled: true, custom_actions: [] };
            const actions = getActionsForText(responseText, qaConfig);
            console.log('[QA] Actions found:', actions.length);
            if (actions.length === 0) return;
            const container = document.getElementById('responseActionsContainer');
            if (container) {
                renderQuickActionChips(actions, container, (promptTemplate) => {
                    const prompt = promptTemplate.replace(/\{text\}/g, responseText);
                    container.style.display = 'none';
                    this.sendChatMessage(prompt, { skipSelection: true });
                });
                await this.windowManager.resizeWindow();
            }
        } catch (e) { console.warn('[QA] Response actions error:', e); }
    }

    /**
     * Render agent-suggested action chips below the response.
     * These come from a hidden ```suggested_actions``` fence in the agent's response.
     */
    _renderSuggestedActions(actions) {
        const container = document.getElementById('responseActionsContainer');
        if (!container) return;
        container.innerHTML = '';
        container.style.display = 'flex';
        for (const action of actions) {
            const chip = document.createElement('button');
            chip.className = 'quick-action-chip';
            chip.title = action.prompt;
            chip.innerHTML = `<span class="quick-action-label">${action.label}</span>`;
            chip.addEventListener('click', () => {
                container.style.display = 'none';
                this.sendChatMessage(action.prompt, { skipSelection: true });
            });
            container.appendChild(chip);
        }
        this.windowManager.resizeWindow();
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
            // Don't hide if a sandbox iframe is running (Try button)
            if (window._kiroSandboxActive) return;
            await this.appWindow.hide();
        }
    }
}
