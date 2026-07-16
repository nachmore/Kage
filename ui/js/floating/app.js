// Main application logic
import { updateSelection, appendSendHint } from './suggestions.js';
import { WindowManager } from './window.js';
import { renderMarkdown, createTaskPlanElement, setAppIconInvoke } from '../shared/markdown.js';
import { loadSlashCommands } from '../shared/commands.js';
import { submitSelection } from '../shared/slash-selection.js';
import {
    AttachmentManager,
    handlePasteEvent,
    renderAttachmentPreviews,
} from '../shared/attachments.js';
import {
    renderToolChipsHtml,
    renderSourceChipsHtml,
    renderSourceBubblesHtml,
    attachSourceClickHandler,
    extractSuggestedActions,
} from '../shared/streaming-utils.js';
import { sendAppNotification } from '../shared/notify.js';
import { EVT } from '../shared/events.js';
import { WINDOW } from '../shared/window-labels.js';
import { getWindowSessionOrNull } from '../shared/session-resolve.js';
import { errLabel, errMessage } from '../shared/error-message.js';
import { getActionsForText, renderQuickActionChips } from '../shared/quick-actions.js';
import {
    startTimer,
    startStopwatch,
    pauseResumeSlot,
    stopSlot,
    getSlotState,
    updateTimerBar,
    setupTimerBarControls,
} from './timer.js';
import { playTimerSound } from '../shared/timer-sounds.js';
import {
    unifiedSearch,
    renderUnifiedResults,
    loadFrecency,
    setExtensionManager,
    searchDebounceMs,
} from './search-unified.js';
import { ExtensionManager } from '../shared/extension-manager.js';
import { SpeechController } from '../shared/speech.js';
import {
    matchShortcut as matchShortcutFn,
    buildShortcutCommand as buildShortcutCommandFn,
    cmdOrCtrlPressed,
    platformKeyLabel,
} from '../shared/shortcuts.js';
import {
    isClipboardTrigger,
    getClipboardFilter,
    fetchClipboardHistory,
    filterClipboardHistory,
    renderClipboardHistory,
} from './clipboard-history.js';
import { mountPromptForm } from '../shared/prompt-form.js';
import { executeShortcutCommand, handleEnterAction } from '../shared/result-executor.js';
import { setupRtlDetection } from '../shared/rtl.js';
import { escapeHtml, formatBytes } from '../shared/tool-utils.js';
import { checkOnline, markOnline, onNetworkChange, offlineMessage } from '../shared/network.js';
import { getConfig, onConfigChange } from '../shared/config-cache.js';
import { parseContextPercent, drawContextRing } from '../shared/context-usage.js';
import { ExtensionToolController } from '../shared/extension-tool-controller.js';
import { AutomationPlanController } from '../shared/automation-plan-controller.js';
import { MessageStreamController } from '../shared/message-stream-controller.js';
import { trackEvent, messageLengthBucket } from '../shared/telemetry.js';
import { hideExtensionBar, showExtensionBar, updateExtensionBar } from '../shared/extension-bar.js';
import { sanitizeExtensionHtml } from '../shared/extension-html-sanitizer.js';
import { renderToolbarButtons } from '../shared/extension-toolbar.js';
import { BannerController } from './banner.js';
import { t } from '../shared/i18n.js';

/**
 * Minimum gap between floating-session re-bootstrap attempts. A send while
 * bootstrap has failed triggers a retry, but the backend restart it drives
 * is expensive (spawns/reinitialises the agent), so we debounce: retries
 * closer together than this just re-show the existing error instead of
 * kicking off another attempt. Comfortably longer than the backend's own
 * restart retry ladder so a genuine cold-start failure fully plays out
 * before the user's next send can trigger a fresh attempt.
 */
const BOOTSTRAP_RETRY_DEBOUNCE_MS = 5000;

/**
 * Measure the natural (no-overflow) content height of a textarea without
 * disturbing its current rendered height. Setting `height='auto'` on the
 * live element collapses it to single-line for one paint, which the user
 * sees as a jerk — so we mirror the value+styles into a hidden clone.
 */
function measureTextareaContentHeight(textarea) {
    const clone = textarea.cloneNode(false);
    clone.value = textarea.value;
    clone.style.position = 'absolute';
    clone.style.visibility = 'hidden';
    clone.style.height = 'auto';
    clone.style.maxHeight = 'none';
    clone.style.width = textarea.clientWidth + 'px';
    clone.style.overflow = 'hidden';
    textarea.parentNode.insertBefore(clone, textarea);
    const h = clone.scrollHeight;
    clone.remove();
    return h;
}

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
        // Delay timer for the search "loading more…" hint. The hint only
        // shows if a search is still running after SEARCH_LOADING_DELAY_MS,
        // so fast searches never flash it in and out. See
        // _requestSearchLoading. `_searchLoadingShownGen` records the search
        // generation whose delay gate has already elapsed, so a render-wipe
        // mid-stream re-shows the hint immediately instead of re-delaying.
        this._searchLoadingTimer = null;
        this._searchLoadingShownGen = -1;
        this._pendingSearchLoadingLabel = null;
        this.currentResponse = '';
        // Floating window's pinned session id. Bootstrapped from
        // window_sessions["floating"] on init; updated on every message
        // send via the recovery-session-id passthrough on message_complete
        // (image-error recovery may swap us to a fresh session).
        this.floatingSessionId = null;
        // True while `_adoptFloatingSession` is awaiting setup's
        // `session_pinned` event (or creating its own session in the
        // start_session_on_launch=false path). The send/typing UX
        // checks this so the user sees a "Spinning up agent…"
        // placeholder rather than an error when they type before the
        // session arrives.
        this.bootstrappingSession = false;
        // Set when bootstrap *fails* (both setup and our recovery
        // session/new failed). Surfaces an error notice in the UI so
        // the user understands floating won't work this session.
        // NOT a permanent latch: a send while this is set triggers a
        // debounced re-bootstrap (see `_retryBootstrap`), so a session
        // that failed to connect (e.g. the agent backend was briefly
        // unavailable at launch) heals itself once the backend recovers,
        // without requiring an app restart.
        this.sessionBootstrapError = null;
        // Epoch-ms of the last re-bootstrap attempt, used to debounce
        // retries so a burst of sends can't cascade into a respawn storm
        // against the agent backend. See `_retryBootstrap`.
        this._lastBootstrapRetryAt = 0;
        this.isWaitingForResponse = false;
        this.shortcuts = [];
        // Track the length at which pattern matching last failed (returned "chat").
        // While the input only grows beyond this length, skip redundant backend calls.
        this._noMatchSinceLen = 0;
        this.toolUsages = [];
        this.toolSources = [];
        this._toolCallIds = new Set();
        this._sourceDomains = new Set();
        this.computerControlActive = false;
        this._noBlurTools = new Set(); // MCP tools that should prevent window hide on blur
        this._promptGeneration = 0; // incremented each time we send a user message
        this.attachmentManager = new AttachmentManager();
        this.extensionManager = new ExtensionManager(invoke);
        this.extensionToolController = new ExtensionToolController({
            invoke,
            getSessionId: () => this.floatingSessionId,
            extensionManager: this.extensionManager,
            permissionModal: {
                showForExtensionTool: (...args) =>
                    window.PermissionModal.showForExtensionTool(...args),
            },
            addToolUsage: (entry) => {
                if (!this._toolCallIds) this._toolCallIds = new Set();
                if (this._toolCallIds.has(entry.toolCallId)) return;
                this._toolCallIds.add(entry.toolCallId);
                this.toolUsages.push(entry);
                this.renderSources();
            },
            renderIndicator: (info) => this._renderExtensionToolIndicator(info),
            onExecuteStart: () => {
                // Hide stop button while tool is executing — the tool may show its own UI
                // (e.g. folder plan confirmation with Run/Cancel buttons)
                this.stopThinking();
                this.elements.floatingStopBtn.style.display = 'none';
                this.updateDatetimeVisibility();
                // Ensure content area is visible for tool UI, and resize after a tick
                // to accommodate any confirmation UI the tool renders
                this.elements.contentArea.classList.add('visible');
                setTimeout(() => this.windowManager.resizeWindow(), 100);
            },
            onExecuteEnd: () => {},
            onWaitForFollowup: () => {
                // Show thinking dots while waiting for the agent's follow-up response
                this.isWaitingForResponse = true;
                this.startThinking();
                this.updateDatetimeVisibility();
            },
            resetAccumulator: () => {
                this.currentResponse = '';
            },
        });
        this.automationPlanController = new AutomationPlanController({
            invoke,
            listen,
            getSessionId: () => this.floatingSessionId,
            renderTasks: (tasks) => {
                const wrapper = createTaskPlanElement(tasks);
                this.elements.responseText.innerHTML = '';
                this.elements.responseText.appendChild(wrapper);
                this.elements.contentArea.classList.add('visible');
                this.elements.expandBtn.classList.add('visible');
                this.windowManager.resizeWindow();
            },
            appendReviewActions: (bar) => {
                this.elements.responseText.appendChild(bar);
            },
            onPlanReadyForReview: () => {
                this.stopThinking();
                this.elements.floatingStopBtn.style.display = 'none';
                this.updateDatetimeVisibility();
                this.isWaitingForResponse = false;
                this.windowManager.resizeWindow();
                // Focus input so the user can type to revise the plan.
                this.elements.input.focus();
            },
            onPlanExecutionStart: async () => {
                this.isWaitingForResponse = true;
                this.elements.floatingStopBtn.style.display = '';
                this.updateDatetimeVisibility();
                await this.windowManager.resizeWindow();
            },
            onPlanComplete: async () => {
                this.isWaitingForResponse = false;
                this.elements.floatingStopBtn.style.display = 'none';
                this.stopThinking();
                this.computerControlActive = false;
                this.updateDatetimeVisibility();
                this._showFloatingResponseActions();
                await this.windowManager.resizeWindow();
            },
            onPlanFailed: (e) => {
                this.showError(errLabel(t('floating.error.automation_failed'), e));
                this.isWaitingForResponse = false;
            },
        });
        const app = this;
        this.messageStreamController = new MessageStreamController({
            // processToolCallUpdate() writes tool/source tracking straight
            // onto the host adapter — forward to the app-instance arrays the
            // render paths (renderSources / onToolCallTracked) read.
            get toolUsages() {
                return app.toolUsages;
            },
            get toolSources() {
                return app.toolSources;
            },
            get _toolCallIds() {
                return app._toolCallIds;
            },
            set _toolCallIds(v) {
                app._toolCallIds = v;
            },
            get _sourceDomains() {
                return app._sourceDomains;
            },
            set _sourceDomains(v) {
                app._sourceDomains = v;
            },
            isWaiting: () => this.isWaitingForResponse,
            // Filter chunks by our pinned session id. Pre-multi-session
            // this returned `true` because there was only one session
            // global to the app; under the multi-session backend that
            // accepts ANY session's chunks — including the launch
            // steering reply, the auto-titler's hidden response, and
            // any chat-* peer's stream. The session_id-on-the-event
            // check rejects all of those.
            acceptSessionId: (sid) => !sid || sid === this.floatingSessionId,
            getAccumulator: () => this.currentResponse,
            appendToAccumulator: (delta) => {
                this.currentResponse = (this.currentResponse || '') + delta;
            },
            resetAccumulator: () => {
                this.currentResponse = '';
            },
            automationPlanController: this.automationPlanController,
            extensionToolController: this.extensionToolController,
            onChunkAppended: (text) => {
                if (text && text.trim().length > 0) {
                    this.elements.loadingDots.classList.remove('visible');
                    this.elements.mascotContainer.classList.remove('thinking');
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
            },
            bumpLayout: () => this.windowManager.resizeWindow(),
            renderStreaming: (text) => {
                renderMarkdown(text, this.elements.responseText, true);
                const toolSpinner =
                    this.elements.responseText.querySelector('.tool-running-indicator');
                if (toolSpinner) toolSpinner.remove();
                if (this.elements.responseText.lastChild) {
                    let streamingIndicator =
                        this.elements.responseText.querySelector('.streaming-indicator');
                    if (!streamingIndicator) {
                        streamingIndicator = document.createElement('span');
                        streamingIndicator.className = 'streaming-indicator';
                        streamingIndicator.textContent = '...';
                        this.elements.responseText.appendChild(streamingIndicator);
                    }
                }
                this.windowManager.resizeWindow();
            },
            feedTTS: (text) => {
                if (this.speech) this.speech.feedStreamingText(text);
            },
            onCompleteHeader: () => {
                markOnline();
                this._noBlurTools.clear();
                this.elements.floatingStopBtn.style.display = 'none';
                this.updateDatetimeVisibility();
            },
            dropEmptyComplete: () => {
                return !this.currentResponse || this.currentResponse.trim().length === 0;
            },
            onBeforeFinalRender: () => {
                this.stopThinking();
                this.computerControlActive = false;
                this.elements.floatingStopBtn.style.display = 'none';
                this.updateDatetimeVisibility();
                const streamingIndicator =
                    this.elements.responseText.querySelector('.streaming-indicator');
                if (streamingIndicator) streamingIndicator.remove();
            },
            // The floating window historically waited 50ms after message_complete
            // to let trailing chunks flush before the final render — without this,
            // the last few tokens were sometimes missing from the markdown.
            waitForPendingChunks: () => new Promise((r) => setTimeout(r, 50)),
            renderFinal: (text) => {
                renderMarkdown(text, this.elements.responseText);
            },
            onAfterFinalRender: async (text) => {
                await this.windowManager.resizeWindow();
                this.isWaitingForResponse = false;
                this._showFloatingResponseActions();

                const suggested = extractSuggestedActions(text);
                if (suggested && suggested.actions.length > 0) {
                    renderMarkdown(suggested.cleanText, this.elements.responseText);
                    this._renderSuggestedActions(suggested.actions);
                }
                if (!suggested || suggested.actions.length === 0) {
                    this._showResponseActions(text);
                }

                if (this.speech) {
                    this.speech.finishStreamingText(text);
                    this.speech.speakResponse(text);
                }

                this._refreshContextUsage();

                try {
                    if (!this._windowFocused && text) {
                        const preview = text
                            .substring(0, 100)
                            .replace(/[#*`\n]/g, ' ')
                            .trim();
                        await sendAppNotification(
                            this.invoke,
                            t('shared.notify.kage_title'),
                            preview || t('floating.notification.response_ready'),
                            WINDOW.FLOATING
                        );
                    }
                } catch {
                    /* ignore */
                }
            },
            onError: async (event, online) => {
                if (!this.isWaitingForResponse) return;
                this.isWaitingForResponse = false;
                this.computerControlActive = false;
                this._noBlurTools.clear();
                this.elements.floatingStopBtn.style.display = 'none';
                this.updateDatetimeVisibility();
                if (!online) this.showError(offlineMessage());
                else
                    this.showError(
                        t('floating.error.error_with_payload', { payload: event.payload }),
                        { reconnect: true }
                    );
            },
            onSessionReset: (_event, msg) => {
                this.isWaitingForResponse = false;
                this.elements.floatingStopBtn.style.display = 'none';
                this.updateDatetimeVisibility();
                this.showError(msg);
            },
            flushPendingMarkdown: () => {
                if (this.currentResponse && this.currentResponse.trim().length > 0) {
                    renderMarkdown(this.currentResponse, this.elements.responseText);
                }
            },
            showToolRunningSpinner: (friendly) => {
                let spinner = this.elements.responseText.querySelector('.tool-running-indicator');
                if (!spinner) {
                    spinner = document.createElement('div');
                    spinner.className = 'folder-plan-spinner-row tool-running-indicator';
                    this.elements.responseText.appendChild(spinner);
                }
                spinner.innerHTML = `<span class="folder-plan-spinner"></span> ${friendly}...`;
                this.elements.contentArea.classList.add('visible');
                this.windowManager.resizeWindow();
            },
            onToolCallTracked: (update, updated) => {
                // Detect computer-control tool usage and keep window visible
                if (update?.title) {
                    const ccTools = [
                        'screenshot',
                        'click',
                        'double_click',
                        'right_click',
                        'move_mouse',
                        'drag',
                        'scroll',
                        'type_text',
                        'key_press',
                        'key_press_confirmed',
                        'launch_app',
                        'wait',
                        'get_screen_size',
                        'get_cursor_position',
                    ];
                    if (ccTools.includes(update.title)) {
                        this.computerControlActive = true;
                    }
                    // Tools that steal focus (show dialogs) — prevent blur-hide while running
                    const noBlurToolNames = ['pick_folder'];
                    if (noBlurToolNames.includes(update.title)) {
                        this._noBlurTools.add(update.toolCallId);
                    }
                }
                if (updated && (this.toolSources.length > 0 || this.toolUsages.length > 0)) {
                    if (!this.currentResponse || this.currentResponse.trim().length === 0) {
                        this.renderSourcesCompact();
                    } else {
                        this.renderSources();
                    }
                }
            },
        });
        this.banner = new BannerController({
            invoke: this.invoke,
            resizeWindow: () => this.windowManager.resizeWindow(),
            resetUI: () => this.resetUI(),
            isWaitingForResponse: () => this.isWaitingForResponse,
            windowManager: this.windowManager,
        });
        this.lastSelection = null;
        this._compacting = false;
        this._messageHistory = []; // shell-style input history
        this._historyIndex = -1; // -1 = not browsing history
        this._historySaved = ''; // stash current input when entering history

        this.elements = {};
    }

    get _extensionToolCallHandled() {
        return this.extensionToolController.handled;
    }
    set _extensionToolCallHandled(v) {
        this.extensionToolController.handled = v;
    }
    get _extensionToolExecuting() {
        return this.extensionToolController.executing;
    }
    set _extensionToolExecuting(v) {
        this.extensionToolController.executing = v;
    }

    get _automationPlanStarted() {
        return this.automationPlanController.started;
    }
    set _automationPlanStarted(v) {
        this.automationPlanController.started = v;
    }
    get _automationPlan() {
        return this.automationPlanController.plan;
    }
    get _automationStatuses() {
        return this.automationPlanController.statuses;
    }
    get _automationCleanup() {
        return this.automationPlanController.cleanup;
    }
    get _pendingPlanRevision() {
        return this.automationPlanController.pendingRevision;
    }
    set _pendingPlanRevision(v) {
        this.automationPlanController.pendingRevision = v;
    }

    async init() {
        const _t0 = performance.now();
        const _ts = (label) =>
            console.log(`⏱ [${(performance.now() - _t0).toFixed(0)}ms] init: ${label}`);

        this._setupSyncWiring();
        _ts('Synchronous setup done');

        await this._bootstrapState();
        _ts('Parallel IPC done (shortcuts + commands + frecency)');

        this.setupSpeech();
        this._registerAppEventListeners();

        setTimeout(() => this.elements.input.focus(), 100);

        this._runPostInitChecks();

        // Tell the backend the window is usable for input. Extension
        // loading below happens in the background and doesn't block.
        this.invoke('notify_frontend_ready').catch(() => {});
        _ts('notify_frontend_ready sent');
        setAppIconInvoke(this.invoke);

        this._initializeExtensionsInBackground(_ts);
    }

    _setupSyncWiring() {
        this.cacheElements();
        this.setupEventListeners();
        this.setupStreamingListeners();
        this.setupVisibilityTracking();
        this.setupNetworkMonitor();
        this.windowManager.setupDragging(this.elements.mascotContainer);
        this.windowManager.setupResizeHandle(document.getElementById('resizeHandle'));

        // Double-click ghost to open full chat window
        this.windowManager._onDoubleClick = async () => {
            try {
                await this.invoke('open_chat_window');
                await this.appWindow.hide();
            } catch (err) {
                console.error('Failed to open chat window:', err);
            }
        };
        this.windowManager.setupScaleChangeListener();
        this.windowManager.setupObserver();

        const inputContainer = this.elements.input?.closest('.input-container');
        setupRtlDetection(this.elements.input, inputContainer, this.elements.responseText);
    }

    async _bootstrapState() {
        await Promise.all([
            this.loadShortcuts(),
            loadSlashCommands(this.invoke),
            loadFrecency(this.invoke),
            this._adoptFloatingSession(),
        ]);
    }

    /**
     * Tauri-event listeners only. `tauri://focus` / `tauri://blur` and
     * the keyboard handlers live in `setupEventListeners` instead —
     * those need to be in place before any IPC has a chance to steal
     * focus.
     */
    _registerAppEventListeners() {
        // onConfigChange (not a raw config_updated listener) guarantees the
        // config cache is already invalidated when this runs, so
        // loadShortcuts() below re-fetches fresh data. See config-cache.js.
        onConfigChange(async () => {
            console.log('Config updated, reloading...');
            await this.loadShortcuts();
            await this.extensionManager.onConfigUpdate();
            await this.extensionManager.reload();
            this.updateSpeechButtonVisibility();
            this._updateToolbarVisibility();
            this._checkTerminatorMode();
            this._refreshOllamaStatusWidget();
            this.updateDatetimeVisibility();
        });

        this.listen(EVT.EXTENSIONS_CHANGED, async () => {
            console.log('Extensions changed, reloading...');
            await this.extensionManager.reload();
        });

        this.listen('slash_commands_available', async () => {
            console.log('Slash commands updated, reloading...');
            await loadSlashCommands(this.invoke);
        });

        this.listen(EVT.CLIPBOARD_HISTORY_MODE, async () => {
            console.log('Clipboard history mode activated via hotkey');
            // Clear any stale content
            this.elements.responseText.textContent = '';
            this.elements.contentArea.classList.remove('visible');
            this.elements.expandBtn.classList.remove('visible');
            this.elements.floatingStopBtn.style.display = 'none';
            this.currentResponse = '';
            this.banner.dismiss();
            // Enter clipboard mode
            this.elements.input.value = '>cb ';
            this._enterClipboardMode();
        });

        this.listen(EVT.VOICE_MODE, () => {
            console.log('Voice mode activated via hotkey');
            trackEvent('voice_input_used', { trigger: 'hotkey' });
            this.elements.responseText.textContent = '';
            this.elements.contentArea.classList.remove('visible');
            this.elements.expandBtn.classList.remove('visible');
            this.elements.floatingStopBtn.style.display = 'none';
            this.currentResponse = '';
            this.banner.dismiss();
            this.elements.input.value = '';
            this.clearSuggestions();
            if (this.speech) {
                this.speech.voiceMode = true;
                if (!this.speech.isListening) {
                    this.speech.start();
                }
            }
        });

        this.listen(EVT.SHOW_FLOATING_BANNER, (event) => {
            const { icon, text, action_label, action_type, action_data } = event.payload;
            this.banner.show(icon, text, action_label, action_type, action_data);
        });

        this.listen(EVT.UPDATE_AVAILABLE, (event) => {
            const version = event.payload;
            this.banner.show(
                '⬆️',
                t('floating.banner.update_available', { version }),
                t('floating.banner.action.install_now'),
                'update_install',
                ''
            );
        });
    }

    /**
     * The post-update "Kage has been updated!" banner is wired into
     * the first `tauri://focus` (in `setupEventListeners`), not fired
     * from here — the idle-install path leaves the floating window
     * hidden until the user summons it, and we only want the banner
     * to show up when the user is actually looking at the window.
     */
    _runPostInitChecks() {
        this.banner.checkForCrashBanner();
        this._checkTerminatorMode();
        this._refreshOllamaStatusWidget();

        const bottomSlot = document.getElementById('extWidgetSlotBottom');
        const statusSlot = document.getElementById('extWidgetSlotStatus');
        if (bottomSlot) this.extensionManager.setWidgetSlot('floating-bottom', bottomSlot);
        if (statusSlot) this.extensionManager.setWidgetSlot('floating-status', statusSlot);
    }

    /**
     * Deferred so cold-start time-to-paint isn't blocked on extension
     * loading; basic input/response works without extensions ready.
     * `_ts` is the optional timing logger threaded through from init().
     */
    _initializeExtensionsInBackground(_ts) {
        this.extensionManager
            .initialize()
            .then(() => {
                _ts?.('Extensions initialized (background)');
                setExtensionManager(this.extensionManager);
                this.extensionToolController.sendSteering();
                if (this._onExtensionsReady) this._onExtensionsReady();
                _ts?.('Extension steering sent (background)');
            })
            .catch((e) => {
                console.warn('Background extension init failed:', e);
            });
    }

    cacheElements() {
        this.elements = {
            input: document.getElementById('promptInput'),
            appSuggestions: document.getElementById('appSuggestions'),
            contentArea: document.getElementById('contentArea'),
            responseText: document.getElementById('responseText'),
            loadingDots: document.getElementById('loadingDots'),
            loadingElapsed: document.getElementById('loadingElapsed'),
            expandBtn: document.getElementById('expandBtn'),
            floatingStopBtn: document.getElementById('floatingStopBtn'),
            mascotContainer: document.querySelector('.mascot-container'),
            attachmentPreviews: document.getElementById('attachmentPreviews'),
            datetimeDisplay: document.getElementById('datetimeDisplay'),
            speechBtn: document.getElementById('speechBtn'),
            speechWave: document.getElementById('speechWave'),
            floatingToolbar: document.getElementById('floatingToolbar'),
            floatingAttachFileBtn: document.getElementById('floatingAttachFileBtn'),
            floatingAttachImageBtn: document.getElementById('floatingAttachImageBtn'),
            floatingFileInput: document.getElementById('floatingFileInput'),
            floatingImageInput: document.getElementById('floatingImageInput'),
            floatingToolbarExt: document.getElementById('floatingToolbarExt'),
            floatingContextIndicator: document.getElementById('floatingContextIndicator'),
            floatingContextRing: document.getElementById('floatingContextRing'),
            floatingContextPercent: document.getElementById('floatingContextPercent'),
            floatingModelSelector: document.getElementById('floatingModelSelector'),
            floatingModelName: document.getElementById('floatingModelName'),
        };
    }

    /**
     * Single source of truth for datetime visibility.
     * Call this instead of directly manipulating the datetime element.
     */
    updateDatetimeVisibility() {
        const dt = this.elements.datetimeDisplay;
        if (!dt) return;
        // Don't show if datetime is disabled in config (managed by applyDateTime in theme.js)
        const configEnabled = dt.dataset.enabled === '1';
        // Hide if: not configured, streaming, stop button visible, input has text, quick actions visible, or speech listening
        const stopVisible = this.elements.floatingStopBtn.style.display !== 'none';
        const hasInput = this.elements.input.value.length > 0;
        const qaVisible =
            document.getElementById('quickActionsContainer')?.style.display === 'flex' ||
            document.getElementById('responseActionsContainer')?.style.display === 'flex';
        const dtHidden =
            !configEnabled ||
            this.isWaitingForResponse ||
            stopVisible ||
            hasInput ||
            qaVisible ||
            this.speech?.isListening;
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
                        this.elements.speechBtn.style.right = dtWidth + 18 + 'px';
                    } else {
                        this.elements.speechBtn.style.right = '10px';
                    }
                }
            }
        }
    }

    /**
     * Set (or clear) the native hover tooltip previewing the captured
     * selection. Uses the browser's built-in `title` so multi-line text
     * doesn't reflow the input layout — the OS renders the tooltip in its
     * own layer. The value is plain text, never HTML.
     */
    _setSelectionPreview(text) {
        const el = document.getElementById('selectionCheckboxLabel');
        if (!el) return;
        const trimmed = (text || '').trim();
        if (trimmed) {
            el.title = trimmed;
        } else {
            el.removeAttribute('title');
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
                if (this.speech?.isActive || this.speech?.isListening) {
                    e.preventDefault();
                    this.speech.stopVoiceMode();
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
            // Ctrl/⌘+, — open settings
            if (cmdOrCtrlPressed(e) && e.key === ',') {
                e.preventDefault();
                this.invoke('open_settings_window');
                return;
            }
            // Ctrl/⌘+E — expand to full chat
            if (cmdOrCtrlPressed(e) && e.key === 'e') {
                e.preventDefault();
                this.handleExpandClick();
                return;
            }
            // Ctrl/⌘+L — clear/reset
            if (cmdOrCtrlPressed(e) && e.key === 'l') {
                e.preventDefault();
                this.resetUI();
                this.windowManager.userSetHeight = null;
                this.windowManager.resizeWindow();
                return;
            }
            // Ctrl/⌘+Shift+C — copy last response
            if (cmdOrCtrlPressed(e) && e.shiftKey && e.key === 'C') {
                e.preventDefault();
                if (this.currentResponse) {
                    navigator.clipboard.writeText(this.currentResponse).catch(() => {});
                }
                return;
            }
            // Ctrl/⌘+W — hide window
            if (cmdOrCtrlPressed(e) && e.key === 'w') {
                e.preventDefault();
                this.appWindow.hide();
                return;
            }
        });

        // Paste handler for images
        this.elements.input.addEventListener('paste', (e) =>
            handlePasteEvent(e, this.attachmentManager)
        );

        // Re-render previews when attachments change and resize window
        this.attachmentManager.onChange((attachments) => {
            renderAttachmentPreviews(
                this.elements.attachmentPreviews,
                attachments,
                this.attachmentManager
            );
            this.windowManager.resizeWindow();
        });

        // Floating toolbar: attach file/image buttons
        this.elements.floatingAttachFileBtn?.addEventListener('click', () => {
            this._filePickerOpen = true;
            this.elements.floatingFileInput?.click();
        });
        this.elements.floatingAttachImageBtn?.addEventListener('click', () => {
            this._filePickerOpen = true;
            this.elements.floatingImageInput?.click();
        });
        this.elements.floatingFileInput?.addEventListener('change', (e) => {
            this._filePickerOpen = false;
            for (const file of e.target.files) {
                const path = file.path || file.name;
                this.attachmentManager.addFile(path, file.name, file.type || 'text/plain');
            }
            e.target.value = '';
            this.appWindow.show();
            this.appWindow.setFocus();
        });
        this.elements.floatingImageInput?.addEventListener('change', async (e) => {
            this._filePickerOpen = false;
            for (const file of e.target.files) {
                if (!file.type.startsWith('image/')) continue;
                try {
                    const base64 = await new Promise((resolve, reject) => {
                        const reader = new FileReader();
                        reader.onload = () => resolve(reader.result.split(',')[1]);
                        reader.onerror = reject;
                        reader.readAsDataURL(file);
                    });
                    this.attachmentManager.addImage(base64, file.type);
                } catch (err) {
                    console.error('Failed to read image:', file.name, err);
                }
            }
            e.target.value = '';
            this.appWindow.show();
            this.appWindow.setFocus();
        });
        // Handle file picker cancel (no change event fires)
        window.addEventListener('focus', () => {
            if (this._filePickerOpen) {
                this._filePickerOpen = false;
                this.appWindow.show();
                this.appWindow.setFocus();
            }
        });

        // Show/hide toolbar based on config
        this._updateToolbarVisibility();

        // Model selector in toolbar — opens model settings
        this.elements.floatingModelSelector?.addEventListener('click', () => {
            this.invoke('open_settings_window', { section: 'model' });
        });
    }

    setupStreamingListeners() {
        this.listen(EVT.MESSAGE_CHUNK, (event) => this.handleMessageChunk(event));
        this.listen(EVT.MESSAGE_COMPLETE, (event) => {
            // Broadcast event — every streaming-audience window hears
            // every session's completes. Only treat it as ours when the
            // active session (post any in-flight recovery) or the
            // pre-recovery session matches our pin; a payload with no
            // session id at all (automation-plan completion) also counts.
            // Without this filter, a turn finishing in a chat window
            // re-pinned floating to that window's session.
            const newId = event?.payload?.sessionId;
            const oldId = event?.payload?.oldSessionId;
            const ours =
                (!newId && !oldId) ||
                newId === this.floatingSessionId ||
                (oldId && oldId === this.floatingSessionId);
            if (!ours) return;

            // Recovery may have moved us to a fresh session; pick up
            // the new id so subsequent sends/cancels target it.
            if (newId && newId !== this.floatingSessionId) {
                console.log('[floating] adopting recovery session id:', newId);
                this.floatingSessionId = newId;
                this.invoke('set_window_session', {
                    label: WINDOW.FLOATING,
                    sessionId: newId,
                }).catch(() => {});
            }
            this.handleMessageComplete();
        });
        this.listen(EVT.MESSAGE_ERROR, (event) => this.handleMessageError(event));
        this.listen(EVT.TOOL_CALL_UPDATE, (event) => this.handleToolCallUpdate(event));
        this.listen('session_migrated', (event) => {
            // The backend died mid-turn and recovery swapped us to a fresh
            // session; the recovered response is about to stream under the
            // new id. Adopt it *without* tearing down the waiting UI (unlike
            // session_reset) and drop the accumulated steering-reply text so
            // the resend renders clean rather than after a stray greeting.
            const oldId = event?.payload?.oldSessionId;
            const newId = event?.payload?.newSessionId;
            const ours = oldId && oldId === this.floatingSessionId;
            if (!ours || !newId) return;
            console.log('[floating] session migrated mid-turn:', oldId, '→', newId);
            this.floatingSessionId = newId;
            this.currentResponse = '';
            this.invoke('set_window_session', {
                label: WINDOW.FLOATING,
                sessionId: newId,
            }).catch(() => {});
        });
        this.listen('session_reset', (event) => {
            // session_reset is broadcast to all windows; only adopt the
            // new id if our pinned session was the one that died.
            const oldId = event?.payload?.oldSessionId;
            const newId = event?.payload?.newSessionId;
            const ours = oldId && oldId === this.floatingSessionId;
            if (!ours) return;
            if (newId) {
                this.floatingSessionId = newId;
                this.invoke('set_window_session', {
                    label: WINDOW.FLOATING,
                    sessionId: newId,
                }).catch(() => {});
            }
            this.handleSessionReset(event);
        });
        this.toolSources = [];

        // Track compaction state — queue outgoing messages while compacting
        this.listen(EVT.COMPACTION_STATUS, (event) => {
            const status = event.payload?.params?.status?.type;
            if (status === 'started') {
                this._compacting = true;
                this._showCompactionIndicator();
            } else if (status === 'completed') {
                this._compacting = false;
                this._hideCompactionIndicator();
                // Ensure stop button is hidden after compaction — it may have been
                // left visible if handleMessageComplete was skipped during tool execution.
                this.elements.floatingStopBtn.style.display = 'none';
                this.updateDatetimeVisibility();
            }
        });

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
                } catch {
                    this.lastSelection = null;
                }
                if (this.lastSelection) {
                    if (indicator) indicator.style.display = '';
                    if (checkbox) checkbox.checked = true;
                    this._setSelectionPreview(this.lastSelection);
                    // Hide datetime to make room for quick actions
                    this.updateDatetimeVisibility();

                    // Show quick action chips based on text content
                    if (quickActionsContainer) {
                        try {
                            const config = await getConfig(this.invoke);
                            const qaConfig = config.quick_actions || {
                                enabled: true,
                                custom_actions: [],
                            };
                            const actions = await getActionsForText(this.lastSelection, qaConfig);
                            renderQuickActionChips(
                                actions,
                                quickActionsContainer,
                                (promptTemplate) => {
                                    const prompt = promptTemplate.replace(
                                        /\{text\}/g,
                                        this.lastSelection
                                    );
                                    this.sendChatMessage(prompt, { skipSelection: true });
                                }
                            );
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
            this._setSelectionPreview(null);
            if (quickActionsContainer) quickActionsContainer.style.display = 'none';
            // Restore datetime and resize back to normal
            this.updateDatetimeVisibility();
            this.windowManager.resizeWindow();
        });

        document.addEventListener('kage-clear', () => {
            this.resetUI();
            this.windowManager.userSetHeight = null;
            this.windowManager.resizeWindow();
        });

        document.addEventListener('kage-resize-request', () => {
            this.windowManager.resizeWindow();
        });

        document.addEventListener('kage-show-response', (e) => {
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

        // Prompt-dispatch slash commands (standard ACP, e.g. Claude): send the
        // slash text as a normal message so the agent interprets it and streams
        // the answer back through the usual pipeline.
        document.addEventListener('kage-send-prompt', (e) => {
            const text = e.detail?.text;
            if (!text) return;
            this.clearSuggestions();
            this.sendChatMessage(text, { forceChat: true });
        });

        document.addEventListener('kage-show-selection', (e) => {
            const { command, options } = e.detail;
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
            this.elements.contentArea.classList.remove('visible');

            // Show options as selectable items in the suggestions dropdown
            this.currentMatches = options.map((opt) => ({
                type: 'selection',
                name: opt.label,
                value: opt.value,
                current: opt.current,
                command: command,
            }));
            this.selectedIndex = options.findIndex((o) => o.current);
            if (this.selectedIndex < 0) this.selectedIndex = 0;

            const container = this.elements.appSuggestions;
            container.innerHTML = '';
            container.scrollTop = 0;

            options.forEach((opt, index) => {
                const item = document.createElement('div');
                item.className =
                    'app-suggestion-item' + (index === this.selectedIndex ? ' selected' : '');
                // Prefer the human description (e.g. an agent's blurb); fall
                // back to the raw value when the agent gave no description.
                // label/description/value come from the agent's commands/execute
                // response — untrusted content in a privileged webview, so build
                // the structure statically and set the text via textContent
                // (mirrors the chat window's slash-selection renderer).
                const subtitle = opt.description || opt.value || '';
                item.innerHTML = `
                    <div class="app-icon">${opt.current ? '✓' : '○'}</div>
                    <div class="app-info">
                        <div class="app-name"><span class="app-name-label"></span>${opt.current ? '<span class="selection-current">●</span>' : ''}</div>
                        <div class="app-description"></div>
                    </div>
                `;
                item.querySelector('.app-name-label').textContent = opt.label || '';
                item.querySelector('.app-description').textContent = subtitle;
                item.addEventListener('click', () => this.executeSelection(command, opt.value));
                container.appendChild(item);
            });

            container.classList.add('visible');
            // Defer scroll-to-selected until after layout is complete
            this.windowManager.resizeWindow();
            setTimeout(() => updateSelection(container, this.selectedIndex), 20);
        });
    }

    setupNetworkMonitor() {
        const bar = document.getElementById('offlineBar');
        const update = (online) => {
            if (bar) bar.style.display = online ? 'none' : 'flex';
            this.windowManager.resizeWindow();
        };
        // Do a real connectivity check on startup
        checkOnline().then((online) => update(online));
        onNetworkChange(update);
    }

    setupVisibilityTracking() {
        this._windowFocused = true; // assume focused at startup

        // --- Pause CSS animations when hidden to stop GPU compositing ---
        // WebView2 keeps processing infinite CSS animations even on hidden windows,
        // which causes the shared GPU process to burn CPU continuously.
        if (!document.getElementById('kage-anim-pause-style')) {
            const s = document.createElement('style');
            s.id = 'kage-anim-pause-style';
            s.textContent =
                '.animations-paused, .animations-paused * { animation-play-state: paused !important; }';
            document.head.appendChild(s);
        }
        // Window starts hidden — pause immediately
        document.documentElement.classList.add('animations-paused');
        // Also catch visibility changes (belt-and-suspenders with focus/blur)
        document.addEventListener('visibilitychange', () => {
            if (document.hidden) document.documentElement.classList.add('animations-paused');
            else document.documentElement.classList.remove('animations-paused');
        });

        this.appWindow.listen('tauri://focus', async () => {
            this._windowFocused = true;
            document.documentElement.classList.remove('animations-paused');
            // First focus this process — show the post-update banner
            // if last_updated_version is still set. Running it here
            // (instead of from init()) means the banner waits until
            // the user can actually see the window: interactive
            // installs trigger this immediately via setup's auto-show,
            // idle installs trigger it the first time the user summons
            // the floating window manually. Either way the user
            // actually sees it.
            if (!this._updateBannerChecked) {
                this._updateBannerChecked = true;
                // The post-install auto-show races against other windows'
                // webviews painting for the first time (notably the
                // preloaded chat window's main.js init). Whichever paints
                // later steals focus and the blur handler would hide us —
                // taking the banner with it. Suppress the next ~2s of
                // blur-hides so the user actually sees the celebration.
                this.banner.checkForUpdateBanner().then((shown) => {
                    if (shown) this._suppressBlurHideUntil = Date.now() + 2000;
                });
            }
            // Resume work that was paused on hide. Mascot animation
            // intervals were ticking against an invisible window — a
            // small but constant CPU drag on every hidden minute.
            // Mirrors the existing permission-modal pause/resume.
            window._kageFloatingHidden = false;
            if (window._kageMascot) {
                try {
                    window._kageMascot.resume();
                } catch (e) {
                    console.warn('mascot.resume failed:', e);
                }
            }
            // Catch up widget renders that were skipped while hidden.
            // _renderWidget no-ops when _kageFloatingHidden is true, so a
            // widget mounted while the window was hidden (e.g. after an
            // extension hot-update) — or whose periodic ticks were all
            // skipped — would show nothing until its next interval fires.
            // Force a render now so the first visible paint is current.
            try {
                this.extensionManager?.renderAllWidgets();
            } catch (e) {
                console.warn('renderAllWidgets on show failed:', e);
            }
            // Notify updater of activity
            this.invoke('touch_floating_activity').catch(() => {});

            // Refresh App Mode chip — the foreground app at summon
            // time is what we'll inject steering for, and that's
            // captured here just before the user starts typing.
            this._refreshAppModeChip().catch((e) => console.log('App Mode lookup failed:', e));

            // Check network status when launcher is invoked (debounced)
            checkOnline().then((online) => {
                const bar = document.getElementById('offlineBar');
                if (bar) bar.style.display = online ? 'none' : 'flex';
                this.windowManager.resizeWindow();
            });

            // Ensure toolbar is visible if configured
            this._updateToolbarVisibility();

            // Restore any overlays hidden by clipboard mode
            if (!this._clipboardMode) {
                this._restoreOverlaysAfterClipboard();
            }

            // Clear any pending system command confirmations and re-trigger search
            if (this.currentMatches.some((m) => m.type === 'system_confirm')) {
                const query = this.elements.input.value.trim();
                if (query) {
                    this.clearSuggestions();
                    const results = await unifiedSearch(query, this.invoke, this.shortcuts);
                    if (results.length > 0) {
                        const { selectedIndex, matches } = await renderUnifiedResults(
                            results,
                            this.elements.appSuggestions,
                            () => this.windowManager.resizeWindow(),
                            (r) => this._onResultClick(r)
                        );
                        this.currentMatches = matches;
                        this.selectedIndex = selectedIndex;
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
                        const config = await getConfig(this.invoke);
                        if (config.ui?.preserve_last_response === false) {
                            setTimeout(() => this.resetUI(), 50);
                        } else {
                            setTimeout(() => {
                                this.elements.input.focus();
                                if (!this._clipboardMode) this.elements.input.select();
                            }, 50);
                        }
                    } catch (_e) {
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
            this._windowFocused = false;
            // Suppress blur-hide briefly after a post-update show. We
            // honour two signals:
            //
            //   `this._suppressBlurHideUntil` — set by checkForUpdateBanner
            //     once the banner is rendered. Catches blurs that happen
            //     after focus stabilises.
            //
            //   `window._kagePostUpdateSuppressUntil` — pre-armed by the
            //     Rust setup code (`maybe_show_floating_after_interactive_install`)
            //     via WebviewWindow::eval BEFORE the floating window is
            //     shown. Catches the early focus-thrashing storm that
            //     fires while the chat / inline-assist preloaded webviews
            //     paint for the first time — the JS bootstrap hasn't yet
            //     run, so the instance flag above is undefined and the
            //     blur would otherwise hide the window before the user
            //     sees the celebration banner.
            const winSuppress = window._kagePostUpdateSuppressUntil;
            if (winSuppress && Date.now() < winSuppress) {
                return;
            }
            if (this._suppressBlurHideUntil && Date.now() < this._suppressBlurHideUntil) {
                return;
            }
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
            // Don't hide if a focus-stealing MCP tool is running (e.g. folder picker dialog)
            if (this._noBlurTools.size > 0) {
                return;
            }
            // Don't hide if an automation plan is running
            if (this._automationPlanStarted) {
                return;
            }
            // Don't hide if file picker is open
            if (this._filePickerOpen) {
                return;
            }
            // Don't hide while an extension tool is being processed
            if (this._extensionToolExecuting || this._extensionToolCallHandled) {
                return;
            }
            await this.appWindow.hide();
            this.banner.dismiss();
            // Pause work that doesn't need to run while hidden.
            // - Mascot animation: ticks every ~120-150ms; over a long
            //   idle session that's real CPU we can give back to the
            //   foreground app the user is actually using.
            // - Hidden flag: read by ExtensionManager._renderWidget so
            //   long-cadence widgets (calendar, todos) skip ticks
            //   that would otherwise repaint into an invisible host.
            // Listeners and observers stay attached — reattaching on
            // every show would directly inflate time-to-paint, which
            // is the headline metric for this window.
            window._kageFloatingHidden = true;
            if (window._kageMascot) {
                try {
                    window._kageMascot.pause();
                } catch (e) {
                    console.warn('mascot.pause failed:', e);
                }
            }
            // Shut down mic and voice mode on hide
            if (this.speech) {
                this.speech.stopVoiceMode();
                this.speech.cancelSpeech();
            }
            // Clean up clipboard mode state on hide
            if (this._clipboardMode) {
                this._restoreOverlaysAfterClipboard();
                this._clipboardMode = false;
                this._clipboardEntries = null;
            }
            // Clear >cb prefix if it's still in the input
            if (this.elements.input.value.startsWith('>cb')) {
                this.elements.input.value = '';
                this.clearSuggestions();
            }
            // Hide response quick actions — if user didn't use them, they're stale
            const responseActions = document.getElementById('responseActionsContainer');
            if (responseActions) responseActions.style.display = 'none';

            // Pause animations to stop GPU compositing while hidden
            document.documentElement.classList.add('animations-paused');
        });

        // Close-time cleanup. The floating window is normally hidden,
        // not closed — this listener is defensive insurance for two
        // cases: (1) a future refactor that flips floating to
        // close-on-dismiss, and (2) explicit teardown via the tray
        // "quit" path. Without it, a closed-but-not-destroyed webview
        // would sit there with mascot timers still ticking until the
        // process exits. The cleanup is intentionally narrow:
        //   - Mascot intervals (the only timer we own that runs while
        //     hidden today).
        //   - Extension manager teardown via destroy(), which cascades
        //     to widget timers and sandbox iframes.
        // We deliberately don't strip event listeners — webview
        // teardown frees the JS heap wholesale, and the cost of
        // walking every DOM listener individually exceeds the wholesale
        // GC the runtime is about to do anyway.
        this.appWindow.listen('tauri://close-requested', () => {
            try {
                if (window._kageMascot) {
                    window._kageMascot.destroy();
                    window._kageMascot = null;
                }
                this.extensionManager?.destroy?.();
            } catch (e) {
                console.warn('floating close-requested cleanup failed:', e);
            }
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
        this._toolCallIds = new Set();
        this._sourceDomains = new Set();
        this.attachmentManager.clear();
        this.elements.contentArea.classList.remove('visible');
        this.elements.contentArea.classList.remove('banner-only');
        const responseActions = document.getElementById('responseActionsContainer');
        if (responseActions) {
            responseActions.innerHTML = '';
            responseActions.style.display = 'none';
        }
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
        // Exit voice conversation mode on reset
        if (this.speech?.voiceMode) {
            this.speech.stopVoiceMode();
        }
        this.elements.input.focus();
        // Re-show datetime when input is cleared
        this.updateDatetimeVisibility();
    }

    startThinking() {
        this.elements.mascotContainer.classList.add('thinking');
        this.elements.loadingDots.classList.add('visible');
        this._startElapsedTimer();
        // A response is about to arrive — drop banner-only mode so the
        // content-area's overflow:auto comes back for scrollable replies.
        // Cheap no-op if the class wasn't set.
        this.elements.contentArea?.classList.remove('banner-only');
        // Switch mascot to jumping animation at larger size
        if (window._kageMascot) {
            import('../shared/mascot-animations.js').then((m) =>
                window._kageMascot.setActive(m.ANIMATIONS.jumping, 60)
            );
        }
        // Show inline stop button in input area, hide datetime
        this.updateDatetimeVisibility();
        this.elements.floatingStopBtn.style.display = '';
    }

    stopThinking() {
        this.elements.mascotContainer.classList.remove('thinking');
        this.elements.loadingDots.classList.remove('visible');
        this._stopElapsedTimer();
        // Return mascot to idle with a wave transition
        if (window._kageMascot) window._kageMascot.setIdle(true);
    }

    /**
     * Show a running elapsed timer beside the loading dots while the agent
     * works. There is no wall-clock cap on a request any more (the backend
     * waits as long as the agent keeps streaming), so the timer is the user's
     * signal that a long request is still alive — and the Stop button remains
     * the escape hatch. Ticks once a second; the label stays hidden for the
     * first few seconds so quick replies don't flash a "0s".
     */
    _startElapsedTimer() {
        this._stopElapsedTimer();
        const el = this.elements.loadingElapsed;
        if (!el) return;
        const started = performance.now();
        el.textContent = '';
        const tick = () => {
            const secs = Math.floor((performance.now() - started) / 1000);
            // Don't distract on fast turns; only surface once it's notably slow.
            if (secs < 3) {
                el.textContent = '';
                return;
            }
            const time =
                secs < 60
                    ? `${secs}s`
                    : `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`;
            el.textContent = t('floating.elapsed', { time });
        };
        tick();
        this._elapsedTimer = setInterval(tick, 1000);
    }

    _stopElapsedTimer() {
        if (this._elapsedTimer) {
            clearInterval(this._elapsedTimer);
            this._elapsedTimer = null;
        }
        if (this.elements.loadingElapsed) this.elements.loadingElapsed.textContent = '';
    }

    stopGenerating() {
        if (!this.isWaitingForResponse) return;

        // If an automation plan is running, stop it gracefully
        if (this._automationPlanStarted) {
            this.automationPlanController.stopGracefully();
            this.computerControlActive = false;
        }

        this.isWaitingForResponse = false;
        this._justStoppedGenerating = true;
        setTimeout(() => {
            this._justStoppedGenerating = false;
        }, 300);
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
        this.invoke('cancel_generation', { sessionId: this.floatingSessionId }).catch((e) =>
            console.log('Cancel:', e)
        );
    }

    // --- Speech ---

    async updateSpeechButtonVisibility() {
        await this.speech.updateVisibility();
    }

    /**
     * App Modes (P1.4 from product_suggestions.md). Looks up the
     * foreground app's process name and asks the backend whether any
     * configured rule matches. On a hit, paints a small chip above the
     * input ("🎯 VS Code mode") and stashes the steering payload on
     * `this._appModeMatch` so `sendChatMessage` can splice it into the
     * outgoing prompt next to `<_kage_ctx>`.
     *
     * The chip is also a one-shot dismiss control — clicking it nulls
     * `_appModeMatch` and hides the chip without touching config, so
     * the rule still fires next summon. The state lives outside the
     * config update flow so the click is instantaneous (no IPC).
     */
    async _refreshAppModeChip() {
        const chip = document.getElementById('appModeChip');
        const labelEl = document.getElementById('appModeChipLabel');
        if (!chip || !labelEl) return;

        // Always start the chip hidden — only re-show on a confirmed
        // match. The previous summon's match should never linger.
        chip.style.display = 'none';
        this._appModeMatch = null;

        let exe = '';
        try {
            const sw = await this.invoke('get_source_window');
            exe = sw?.processName || '';
        } catch {
            return;
        }
        if (!exe) return;

        try {
            const matched = await this.invoke('match_context_rule', { executable: exe });
            if (!matched) return;
            this._appModeMatch = matched;
            labelEl.textContent = matched.friendly_name;
            chip.style.display = 'inline-flex';
            // Wire dismiss once — first event we see clears state.
            // We swap the listener each refresh because the matched
            // rule may have changed between summons.
            chip.onclick = () => {
                this._appModeMatch = null;
                chip.style.display = 'none';
            };
        } catch (e) {
            console.log('match_context_rule failed:', e);
        }
    }

    /**
     * Optional Ollama status widget. Off by default, configured per
     * Ollama connection in the Agents wizard. When the active
     * connection is Ollama-shaped and `show_status_widget=true`,
     * render a small "🦙 <model> · ready" chip via the existing
     * extension-bar slot and refresh it every 30s while the floating
     * window is being used. Anything else hides the widget.
     *
     * Reuses the extension-bar API rather than rolling its own DOM
     * because that path already handles window-resize accounting,
     * blur-without-focus-steal, and theme inversion. The "extension"
     * label is a misnomer — it's just a styled bar slot — but
     * adopting an existing pattern beats inventing a new chip system
     * for a six-row feature.
     */
    async _refreshOllamaStatusWidget() {
        let cfg;
        try {
            cfg = await getConfig(this.invoke);
        } catch {
            return;
        }
        const active = cfg.acp?.connections?.find((c) => c.id === cfg.acp?.active_connection_id);
        const settings = active?.preset_id === 'ollama' ? active.ollama_settings : null;
        const enabled = !!settings?.show_status_widget && !!settings?.base_url;

        if (!enabled) {
            this._stopOllamaStatusPoll();
            hideExtensionBar('ollama-status');
            return;
        }

        // Show a placeholder immediately so the user sees the widget
        // appear even before the first probe completes. The poll
        // below replaces "checking..." with the resolved state.
        this._ollamaStatus = {
            baseUrl: settings.base_url,
            model: settings.model || '',
            // Tracking the same params the poll reads from. If the
            // user changes model in Settings while the floating
            // window is open, the config_updated listener calls us
            // again and we re-seed.
        };
        showExtensionBar({
            id: 'ollama-status',
            icon: '🦙',
            text: settings.model ? `${settings.model} · checking…` : 'Ollama · checking…',
            className: 'ollama-status-bar',
        });
        this._startOllamaStatusPoll();
        // Kick one immediate refresh too — saves the user a 30s wait
        // when the widget first appears.
        this._pollOllamaStatusOnce().catch(() => {});
    }

    _startOllamaStatusPoll() {
        this._stopOllamaStatusPoll();
        // 30s is the same cadence the existing extension status widgets
        // use; balances "is it still up" reassurance with avoiding LAN
        // chatter for users on metered or finicky networks.
        this._ollamaStatusInterval = setInterval(
            () => this._pollOllamaStatusOnce().catch(() => {}),
            30 * 1000
        );
    }

    _stopOllamaStatusPoll() {
        if (this._ollamaStatusInterval) {
            clearInterval(this._ollamaStatusInterval);
            this._ollamaStatusInterval = null;
        }
    }

    async _pollOllamaStatusOnce() {
        const s = this._ollamaStatus;
        if (!s) return;
        let probe = null;
        try {
            probe = await this.invoke('ollama_probe', { baseUrl: s.baseUrl });
        } catch (e) {
            updateExtensionBar('ollama-status', {
                text: `${s.model || 'Ollama'} · offline`,
            });
            return;
        }
        if (probe?.status !== 'Reachable') {
            updateExtensionBar('ollama-status', {
                text: `${s.model || 'Ollama'} · offline`,
            });
            return;
        }
        // Reachable — hit /api/tags too, since it's the only way to
        // surface the resident size. allSettled-style: if list_models
        // fails for a transient reason, leave the size off.
        let sizeStr = '';
        try {
            const models = await this.invoke('ollama_list_models', {
                baseUrl: s.baseUrl,
            });
            const match = (Array.isArray(models) ? models : []).find((m) => m?.name === s.model);
            if (match?.size) sizeStr = ` · ${formatBytes(match.size)}`;
        } catch {}
        const versionStr = probe.version ? ` (Ollama ${probe.version})` : '';
        updateExtensionBar('ollama-status', {
            text: `${s.model || 'Ollama'}${sizeStr} · ready${versionStr}`,
        });
    }

    async _updateToolbarVisibility() {
        try {
            const config = await getConfig(this.invoke);
            const show = config.ui?.show_floating_toolbar === true;
            if (this.elements.floatingToolbar) {
                this.elements.floatingToolbar.style.display = show ? 'flex' : 'none';
                if (show) {
                    this._renderExtensionToolbarButtons();
                    // Throttle expensive IPC calls — models load once per session, context refreshes every 5min
                    const now = Date.now();
                    if (!this._modelsLoaded) {
                        this._modelsLoaded = true;
                        this._loadModels();
                    }
                    if (!this._lastContextRefresh || now - this._lastContextRefresh > 300000) {
                        this._lastContextRefresh = now;
                        this._refreshContextUsage();
                    }
                }
            }
            this.windowManager.resizeWindow();
        } catch (e) {
            console.warn('[Floating] Failed to update toolbar visibility:', e);
        }
    }

    _renderExtensionToolbarButtons() {
        const container = this.elements.floatingToolbarExt;
        if (!container || !this.extensionManager) return;
        const buttons = this.extensionManager.getToolbarButtons();
        console.log('[Floating] Rendering extension toolbar buttons:', buttons.length);
        renderToolbarButtons({
            container,
            buttons,
            buttonClass: 'floating-toolbar-btn',
            sanitizeIcon: (iconStr) => sanitizeExtensionHtml(iconStr, 'icon'),
            buildContext: () => ({
                input: this.elements.input?.value || '',
                messages: [],
            }),
            onHostEffect: (host) => this._runToolbarHostEffect(host),
        });
    }

    /**
     * Apply a host effect returned from an extension toolbar click.
     * Mirrors the chat window's implementation with the floating input.
     */
    _runToolbarHostEffect(host) {
        if (!host || typeof host !== 'object') return;
        switch (host.type) {
            case 'set_chat_input': {
                const v = String(host.value ?? '');
                if (this.elements.input) {
                    this.elements.input.value = v;
                    this.elements.input.focus();
                    this.elements.input.dispatchEvent(new Event('input'));
                }
                break;
            }
            case 'append_chat_input': {
                const v = String(host.value ?? '');
                if (this.elements.input) {
                    const cur = this.elements.input.value || '';
                    const sep = cur && !cur.endsWith(' ') ? ' ' : '';
                    this.elements.input.value = cur + sep + v;
                    this.elements.input.focus();
                    this.elements.input.dispatchEvent(new Event('input'));
                }
                break;
            }
            case 'show_ephemeral_message':
                // Floating window has no messages area; log and drop so
                // extensions can tell the difference between unsupported
                // contexts and silent failure.
                console.info(
                    '[Floating] Ignoring show_ephemeral_message host effect — only supported in chat window'
                );
                break;
            default:
                console.warn('[Floating] Unknown toolbar host effect:', host.type);
                break;
        }
    }

    // --- Context % and Model Selector ---

    async _refreshContextUsage() {
        try {
            const result = await this.invoke('execute_slash_command', {
                sessionId: this.floatingSessionId,
                command: 'context',
                args: {},
            });
            const pct = parseContextPercent(result);
            if (pct !== null) {
                if (this.elements.floatingContextPercent)
                    this.elements.floatingContextPercent.textContent = pct + '%';
                if (this.elements.floatingContextIndicator)
                    this.elements.floatingContextIndicator.title = pct + '% context used';
                drawContextRing(this.elements.floatingContextRing, pct);
            }
        } catch {}
    }

    async _loadModels() {
        try {
            const models = await this.invoke('get_available_models');
            this._availableModels = models || [];
            if (this._availableModels.length > 0) {
                const current = this._availableModels[0];
                if (this.elements.floatingModelName)
                    this.elements.floatingModelName.textContent =
                        current.name || current.modelId || '?';
            }
        } catch {}
    }

    setupSpeech() {
        this.speech = new SpeechController({
            invoke: this.invoke,
            elements: {
                input: this.elements.input,
                speechBtn: this.elements.speechBtn,
                speechWave: this.elements.speechWave,
            },
            onSend: (text) => this.sendChatMessage(text),
            onVisibilityUpdate: () => this.updateDatetimeVisibility(),
            barContainer: document.querySelector('.input-container'),
        });
        this.speech.setup();
    }

    // Convenience accessors used by Escape handler and sendChatMessage
    get isSpeechListening() {
        return this.speech?.isListening ?? false;
    }
    get _usedSpeechForLastMessage() {
        return this.speech?.usedSpeechForLastMessage ?? false;
    }
    set _usedSpeechForLastMessage(v) {
        if (this.speech) this.speech.usedSpeechForLastMessage = v;
    }

    /**
     * Resolve floating's pinned session id.
     *
     * Behaviour depends on the user's `start_session_on_launch` setting:
     *
     * - **Enabled (default).** Setup is preloading a session in the
     *   background; wait for the `session_pinned` event with NO
     *   timeout (kiro-cli cold-start can take 10s+). `session_pin_failed`
     *   covers the deadlock: setup emits it on connect/load/create
     *   failures, we react by creating our own session. Either way,
     *   we don't hang the user forever.
     *
     * - **Disabled.** Don't wait — create our own session immediately.
     *
     * In both cases the listener is registered BEFORE the synchronous
     * `get_window_session` check, so we can't miss a `session_pinned`
     * that fires between the two (which would otherwise leave us
     * permanently stalled).
     *
     * The frontend "spinning up agent…" UX is driven by
     * `this.bootstrappingSession` — true while we're in the wait phase
     * so onChunkAppended / typing handlers can show the placeholder.
     */
    async _adoptFloatingSession() {
        this.bootstrappingSession = true;
        // Register both listeners first so a fast event can't fire
        // between our synchronous check and the await below.
        let resolveAdopted;
        let resolveFailed;
        const adopted = new Promise((resolve) => {
            resolveAdopted = resolve;
        });
        const failed = new Promise((resolve) => {
            resolveFailed = resolve;
        });
        const unlistenPinned = await this.listen('session_pinned', (event) => {
            const { label, sessionId } = event?.payload || {};
            if (label === WINDOW.FLOATING && sessionId) {
                resolveAdopted(sessionId);
            }
        });
        const unlistenFailed = await this.listen('session_pin_failed', (event) => {
            const { label, reason } = event?.payload || {};
            if (label === WINDOW.FLOATING) {
                resolveFailed(reason || 'unknown');
            }
        });

        try {
            // Setup may have already pinned us before our init ran.
            const existing = await getWindowSessionOrNull(this.invoke, WINDOW.FLOATING);
            if (existing) {
                this.floatingSessionId = existing;
                this.bootstrappingSession = false;
                console.log(`[floating] adopted pre-pinned session: ${existing}`);
                return;
            }

            // Read the config so we know whether to wait or create now.
            // Default to true on read failure — the wait + failure path
            // below recovers gracefully if setup never emits anything.
            let willPreload = true;
            try {
                const config = await getConfig(this.invoke);
                willPreload = config?.acp?.agent?.start_session_on_launch !== false;
            } catch (e) {
                console.warn('[floating] config read failed, assuming preload:', e);
            }

            if (!willPreload) {
                console.log('[floating] start_session_on_launch=false, creating session now');
                const id = await this.invoke('switch_acp_session', { sessionId: null });
                this.floatingSessionId = id;
                this.bootstrappingSession = false;
                return;
            }

            // Race the two outcomes. No timeout — failure events
            // bound the wait, kiro-cli cold-start can be slow.
            const winner = await Promise.race([
                adopted.then((sid) => ({ kind: 'pinned', sid })),
                failed.then((reason) => ({ kind: 'failed', reason })),
            ]);

            if (winner.kind === 'pinned') {
                this.floatingSessionId = winner.sid;
                this.bootstrappingSession = false;
                console.log(`[floating] adopted pinned session via event: ${winner.sid}`);
                return;
            }

            // Setup told us it failed. Try to create our own as a
            // last resort; if THAT fails the user gets an explicit
            // error rather than a silent hang.
            console.warn(`[floating] setup reported pin failure: ${winner.reason}`);
            try {
                const id = await this.invoke('switch_acp_session', { sessionId: null });
                this.floatingSessionId = id;
                console.log(`[floating] recovered with own session: ${id}`);
            } catch (e) {
                console.error('[floating] recovery session/new also failed:', e);
                this.floatingSessionId = null;
                this.sessionBootstrapError = errMessage(e);
            } finally {
                this.bootstrappingSession = false;
            }
        } catch (e) {
            console.error('[floating] failed to adopt session:', e);
            this.floatingSessionId = null;
            this.sessionBootstrapError = String(e);
            this.bootstrappingSession = false;
        } finally {
            if (typeof unlistenPinned === 'function') unlistenPinned();
            if (typeof unlistenFailed === 'function') unlistenFailed();
        }
    }

    /**
     * Re-attempt session bootstrap after a prior failure, then flush the
     * queued send. Triggered from `sendChatMessage` when the user sends
     * while `sessionBootstrapError` is set — a transient backend outage at
     * launch shouldn't strand the floating window until the app restarts.
     *
     * Debounced via `_lastBootstrapRetryAt`: retries closer together than
     * `BOOTSTRAP_RETRY_DEBOUNCE_MS` skip the (expensive) backend reconnect
     * and just re-show the existing error, so a burst of sends can't cascade
     * into a respawn storm against the agent backend. The backend
     * `restart_connection` has its own coalesce+retry guard too; this is the
     * front line of the same defence.
     *
     * Reuses the existing queue/poll/flush machinery: we set
     * `bootstrappingSession` so `_waitForBootstrapAndSend` waits for the
     * retry to settle, then `_flushPendingSend` either replays the send (on
     * success) or re-shows the error (on repeat failure).
     */
    _retryBootstrapAndSend(message, options) {
        const now = Date.now();
        if (now - this._lastBootstrapRetryAt < BOOTSTRAP_RETRY_DEBOUNCE_MS) {
            // Too soon since the last attempt — a retry is likely still in
            // flight or only just failed. Surface the error rather than
            // kicking off another reconnect.
            this.showError(
                t('floating.error.agent_unavailable', { reason: this.sessionBootstrapError })
            );
            return;
        }
        this._lastBootstrapRetryAt = now;
        this._pendingSend = { message, options };
        this._showBootstrapSpinner();
        // Gate BEFORE starting the async retry so the poller waits on it.
        this.bootstrappingSession = true;
        this._retryBootstrap();
        this._waitForBootstrapAndSend();
    }

    /**
     * Single re-bootstrap attempt: ask the backend for a session (which
     * lazily reconnects/respawns the agent if the connection died), and
     * clear or refresh `sessionBootstrapError` based on the outcome. Always
     * clears `bootstrappingSession` on the way out so the poller unblocks.
     */
    async _retryBootstrap() {
        console.log('[floating] retrying session bootstrap after prior failure');
        try {
            const id = await this.invoke('switch_acp_session', { sessionId: null });
            this.floatingSessionId = id;
            this.sessionBootstrapError = null;
            console.log(`[floating] re-bootstrap succeeded: ${id}`);
        } catch (e) {
            console.error('[floating] re-bootstrap failed:', e);
            this.floatingSessionId = null;
            this.sessionBootstrapError = errMessage(e);
        } finally {
            this.bootstrappingSession = false;
        }
    }

    /**
     * Show a "Spinning up agent…" placeholder in floating's response
     * area while we're waiting for the launch session to be pinned.
     * Removed once `_flushPendingSend()` runs OR the bootstrap fails
     * (showError replaces it).
     */
    _showBootstrapSpinner() {
        if (!this.elements.responseText) return;
        this.elements.contentArea.classList.add('visible');
        this.elements.responseText.innerHTML = `
            <div class="bootstrap-spinner">
                ${t('floating.bootstrap.spinner')}
                <span class="bootstrap-dot">.</span><span class="bootstrap-dot">.</span><span class="bootstrap-dot">.</span>
            </div>`;
        this.windowManager.resizeWindow();
    }

    /**
     * After bootstrap completes (or fails), flush a queued send. Called
     * from `_waitForBootstrapAndSend`. On success, replays the original
     * `sendChatMessage`; on failure, surfaces the error.
     */
    _flushPendingSend() {
        const pending = this._pendingSend;
        this._pendingSend = null;
        if (!pending) return;
        // Clear the spinner — sendChatMessage's normal path will set
        // up its own thinking indicator.
        if (this.elements.responseText) {
            this.elements.responseText.innerHTML = '';
        }
        if (this.sessionBootstrapError) {
            this.showError(
                t('floating.error.agent_unavailable', { reason: this.sessionBootstrapError })
            );
            return;
        }
        if (!this.floatingSessionId) {
            this.showError(t('floating.error.no_session'));
            return;
        }
        // Re-enter sendChatMessage; the bootstrap-guard at the top
        // will pass through now.
        this.sendChatMessage(pending.message, pending.options);
    }

    /**
     * Poll once-per-100ms for bootstrap completion (success or failure)
     * and flush. Cheap because we're only running while the user has a
     * pending send queued — usually <1s on hot launches, ~10s on cold.
     */
    async _waitForBootstrapAndSend() {
        // If multiple calls race, only one polling loop should run.
        if (this._waitingForBootstrap) return;
        this._waitingForBootstrap = true;
        try {
            while (this.bootstrappingSession) {
                await new Promise((r) => setTimeout(r, 100));
            }
        } finally {
            this._waitingForBootstrap = false;
        }
        this._flushPendingSend();
    }

    async loadShortcuts() {
        try {
            const config = await getConfig(this.invoke);
            this.shortcuts = config.shortcuts || [];
            console.log('Loaded shortcuts:', this.shortcuts);
        } catch (error) {
            console.error('Failed to load shortcuts:', error);
            this.shortcuts = [];
        }
    }

    _startTimerUI(durationMs) {
        startTimer(
            durationMs,
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
            const fullConfig = await getConfig(this.invoke);
            config = fullConfig.extensions?.timer || {};
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
                await sendAppNotification(
                    this.invoke,
                    t('floating.timer.notification_title'),
                    '⏱️ ' + t('floating.timer.notification_body'),
                    WINDOW.FLOATING
                );
            } catch {}
        }

        if (config.sound_on_complete !== false) {
            try {
                playTimerSound(
                    config.sound_id || 'two-tone',
                    config.custom_sound_path || '',
                    config.sound_repeats || 3
                );
            } catch {}
        }

        // Auto-hide the timer bar after 5 seconds
        setTimeout(() => {
            const s = getSlotState('timer');
            if (!s.active) {
                const bar = document.getElementById('timerBar_timer');
                if (bar) {
                    bar.style.display = 'none';
                    bar.remove();
                }
                this.windowManager.resizeWindow();
            }
        }, 5000);
    }

    async _checkTerminatorMode() {
        try {
            const isTerminator = await this.invoke('is_terminator_mode');
            if (isTerminator) {
                // Clear dismissed flag when mode is (re-)enabled so the bar
                // reappears if the user toggled it off and back on.
                if (this._terminatorWasOff) {
                    sessionStorage.removeItem('terminator_bar_dismissed');
                }
                this._terminatorWasOff = false;
                if (!sessionStorage.getItem('terminator_bar_dismissed')) {
                    showExtensionBar({
                        id: 'terminator',
                        icon: '🤖',
                        text: t('floating.terminator.text'),
                        className: 'terminator-bar',
                        buttons: [
                            {
                                id: 'dismiss',
                                label: '✕',
                                title: t('floating.terminator.dismiss_title'),
                                onClick: () => {
                                    sessionStorage.setItem('terminator_bar_dismissed', '1');
                                    hideExtensionBar('terminator');
                                },
                            },
                        ],
                    });
                }
            } else {
                this._terminatorWasOff = true;
                hideExtensionBar('terminator');
            }
        } catch {
            /* ignore */
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
            selectionText: document.getElementById('useSelectionCheckbox')?.checked
                ? this.lastSelection || ''
                : '',
            onPrompt: (text) => this.sendChatMessage(text),
            onDisplay: (text) => {
                this.currentResponse = text;
                renderMarkdown(text, this.elements.responseText);
                this.elements.contentArea.classList.add('visible');
                this.windowManager.resizeWindow();
            },
            onCopy: async (text) => {
                try {
                    await navigator.clipboard.writeText(text);
                } catch {}
            },
            onReplaceInput: (text) => {
                this.elements.input.value = text;
                this.elements.input.dispatchEvent(new Event('input', { bubbles: true }));
            },
            // An extension result signalled that it mutated state its widget
            // renders (e.g. Spotify `sp like`). Repaint mounted widgets so the
            // floating bar reflects the change immediately rather than after
            // the widget's next poll. Delayed one beat because APIs like
            // Spotify's are eventually consistent — an immediate re-render can
            // still read the pre-change state (the widget's own onAction path
            // uses the same 250ms settle).
            onRefreshWidgets: () => {
                setTimeout(() => {
                    try {
                        this.extensionManager?.renderAllWidgets();
                    } catch (e) {
                        console.warn('renderAllWidgets on refresh_widgets failed:', e);
                    }
                }, 300);
            },
            onTimerStart: (ms) => this._startTimerUI(ms),
            onStopwatch: () => {
                const sw = getSlotState('stopwatch');
                if (sw.active && sw.running) {
                    pauseResumeSlot('stopwatch');
                } else if (sw.active && !sw.running) {
                    stopSlot('stopwatch');
                    const bar = document.getElementById('timerBar_stopwatch');
                    if (bar) {
                        bar.remove();
                    }
                    this.windowManager.resizeWindow();
                } else {
                    this._startStopwatchUI();
                }
            },
            onPromptForm: (formCmd) => this._showPromptForm(formCmd),
        };
    }

    /**
     * Render the missing-placeholders form in the response area. On
     * submit, re-build the shortcut command with the collected params
     * and re-enter the executor — single round trip back into the
     * normal `prompt` flow.
     */
    _showPromptForm(formCmd) {
        const responseEl = this.elements.responseText;
        if (!responseEl) return;
        // Hide the markdown response slot — we're using the same area
        // for the form. The contentArea visibility flag ensures the
        // window expands to fit the form.
        this.elements.contentArea.classList.add('visible');
        this.elements.contentArea.classList.remove('banner-only');

        mountPromptForm(responseEl, formCmd, {
            onSubmit: async (paramsByName) => {
                const useSelection = document.getElementById('useSelectionCheckbox')?.checked;
                const sel = useSelection && this.lastSelection ? this.lastSelection : '';
                const rebuilt = buildShortcutCommandFn(
                    formCmd.shortcut,
                    formCmd.args,
                    sel,
                    paramsByName
                );
                // Clear the form before executing — rebuilt is a regular
                // `prompt` command at this point so `onPrompt` will
                // populate the response area normally.
                responseEl.textContent = '';
                this.elements.contentArea.classList.remove('visible');
                this.windowManager.resizeWindow();
                await this.executeShortcut(rebuilt);
            },
            onCancel: () => {
                responseEl.textContent = '';
                this.elements.contentArea.classList.remove('visible');
                this.windowManager.resizeWindow();
                this.elements.input.focus();
            },
        });
        this.windowManager.resizeWindow();
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
            this.showError(errLabel(t('floating.error.failed_to_execute_shortcut'), error));
        }
    }

    /**
     * Delay before the search "loading more…" hint appears. Searches that
     * finish faster than this never show the hint at all, so the common
     * fast case doesn't flash a spinner in and out. ~half a second is long
     * enough that a human reads the absence as "instant".
     */
    static get SEARCH_LOADING_DELAY_MS() {
        return 500;
    }

    /**
     * Request the search loading hint for generation `gen`, showing it only
     * if the search is still running after SEARCH_LOADING_DELAY_MS. Calling
     * again with new text just updates the label (or re-arms the timer if
     * the hint isn't visible yet). The `gen` guard means a hint armed by an
     * older search can never appear over a newer one.
     */
    _requestSearchLoading(gen, label) {
        if (gen !== this._searchGeneration) return;
        const existing = this.elements.appSuggestions.querySelector('.suggestions-loading');
        if (existing) {
            // Already visible — just keep the label current.
            existing.classList.remove('suggestions-loading-out');
            existing.textContent = label;
            return;
        }
        this._pendingSearchLoadingLabel = label;
        // The delay gate has already elapsed for this generation (the hint
        // was shown, then a later partial-render wiped the container via
        // innerHTML=''). Re-append immediately rather than re-delaying —
        // otherwise an actively-streaming search keeps resetting its own
        // 500ms gate and never shows progress.
        if (this._searchLoadingShownGen === gen) {
            // Re-append after a render-wipe — no entry animation, so an
            // actively-streaming search doesn't re-flash the fade-in on
            // every partial batch.
            this._appendSearchLoadingHint(label, false);
            this.windowManager.resizeWindow();
            return;
        }
        // First time for this generation: arm the delay timer so a search
        // that finishes within the gate never shows the hint at all.
        if (this._searchLoadingTimer) return; // timer already counting down
        this._searchLoadingTimer = setTimeout(() => {
            this._searchLoadingTimer = null;
            if (gen !== this._searchGeneration) return; // search moved on
            this._searchLoadingShownGen = gen;
            this._appendSearchLoadingHint(this._pendingSearchLoadingLabel || label, true);
            this.windowManager.resizeWindow();
        }, FloatingApp.SEARCH_LOADING_DELAY_MS);
    }

    /**
     * Create + append the loading hint element. `animate` plays the
     * fade-in (true on first show; false when re-appending after a
     * render-wipe so streaming partials don't re-trigger the animation).
     */
    _appendSearchLoadingHint(label, animate) {
        const hint = document.createElement('div');
        hint.className = 'suggestions-hint suggestions-loading';
        if (!animate) hint.classList.add('suggestions-loading-no-in');
        hint.textContent = label;
        this.elements.appSuggestions.appendChild(hint);
    }

    /**
     * Hide the search loading hint. Cancels a pending (not-yet-shown) hint
     * outright — the fast-search path, which is why nothing flashes. If the
     * hint is already on screen, fade it out before removing so it animates
     * away instead of vanishing.
     */
    _hideSearchLoading() {
        if (this._searchLoadingTimer) {
            clearTimeout(this._searchLoadingTimer);
            this._searchLoadingTimer = null;
        }
        this._pendingSearchLoadingLabel = null;
        this._searchLoadingShownGen = -1;
        const existing = this.elements.appSuggestions.querySelector('.suggestions-loading');
        if (!existing) return;
        existing.classList.add('suggestions-loading-out');
        const el = existing;
        const done = () => el.remove();
        el.addEventListener('animationend', done, { once: true });
        // Fallback in case the animation is interrupted (element detached,
        // reduced-motion, etc.) so we never leak a stuck hint.
        setTimeout(done, 250);
    }

    async handleInputChange(_event) {
        const rawQuery = this.elements.input.value;
        const query = rawQuery.trim();

        // Resize the textarea and OS window in lockstep — see animateInputResize.
        // We measure scrollHeight via a clone so the live textarea never has
        // a 1-frame "single line with overflow" state.
        const input = this.elements.input;
        const oldH = input.offsetHeight;
        const newH = Math.min(measureTextareaContentHeight(input), 100);
        if (newH !== oldH) {
            this.windowManager.animateInputResize(input, oldH, newH);
        }

        // Reset tab cycle state when user types
        this._tabCycleActive = false;

        // Reset history browsing when user types new content
        if (this._historyIndex >= 0) {
            this._historyIndex = -1;
            this._historySaved = '';
        }

        // Dismiss banner as soon as user starts typing — it's served its purpose
        if (query.length > 0) this.banner.dismiss();

        // Update datetime visibility based on input state
        this.updateDatetimeVisibility();

        if (this.searchTimeout) {
            clearTimeout(this.searchTimeout);
        }

        if (query.length === 0) {
            this._hideSearchLoading();
            this.elements.appSuggestions.classList.remove('visible');
            this.currentMatches = [];
            this.selectedIndex = -1;
            this._noMatchSinceLen = 0;
            await this.windowManager.resizeWindow();
            return;
        }

        // Resize window to fit the growing input
        await this.windowManager.resizeWindow();

        // Debounced unified search — queries all sources in parallel.
        // File-shaped queries hit the disk, so they debounce harder; the
        // heuristic + timing live in the shared search engine.
        const debounceMs = searchDebounceMs(query);
        this._searchGeneration++;
        const gen = this._searchGeneration;
        this.searchTimeout = setTimeout(async () => {
            this.searchTimeout = null; // Mark debounce as fired
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

            const results = await unifiedSearch(
                rawQuery,
                this.invoke,
                this.shortcuts,
                async (partial, { done, pending }) => {
                    // Progressive rendering: show results as they arrive
                    if (gen !== this._searchGeneration) return; // stale
                    if (partial.length > 0) {
                        const { selectedIndex, matches } = await renderUnifiedResults(
                            partial,
                            this.elements.appSuggestions,
                            () => this.windowManager.resizeWindow(),
                            (r) => this._onResultClick(r)
                        );
                        // renderUnifiedResults awaits a sandbox round-trip; a
                        // newer flush may have superseded us while we were in
                        // it. Commit matches + selection together only if we're
                        // still current, so the two never disagree (which would
                        // make Enter fire the wrong row — see the function's
                        // doc comment).
                        if (gen !== this._searchGeneration) return;
                        this.currentMatches = matches;
                        this.selectedIndex = selectedIndex;
                    }
                    // Show/hide loading indicator with provider names.
                    // _requestSearchLoading delay-gates the hint so fast
                    // searches (which reach done before the gate fires)
                    // never flash it; _hideSearchLoading fades it out.
                    if (done) {
                        this._hideSearchLoading();
                    } else {
                        let label = t('floating.suggestions.loading_more');
                        if (pending && pending.length > 0) {
                            const shown = pending.slice(0, 2).join(', ');
                            label += ' (' + shown + (pending.length > 2 ? ', \u2026' : '') + ')';
                        }
                        label += '\u2026';
                        this._requestSearchLoading(gen, label);
                    }
                    this.windowManager.resizeWindow();
                }
            );
            // Discard stale results — a newer search was started while this one was in-flight
            if (gen !== this._searchGeneration) return;
            // All providers resolved — hide the loading hint (cancels it
            // outright if the delay gate never fired, fades it out if it did).
            this._hideSearchLoading();
            if (results.length > 0) {
                const { selectedIndex, matches } = await renderUnifiedResults(
                    results,
                    this.elements.appSuggestions,
                    () => this.windowManager.resizeWindow(),
                    (r) => this._onResultClick(r)
                );
                if (gen !== this._searchGeneration) return;
                this.currentMatches = matches;
                this.selectedIndex = selectedIndex;
                // Show send hint for non-instant results
                if (!['color', 'math', 'devtool'].includes(results[0].type)) {
                    appendSendHint(this.elements.appSuggestions);
                }
            } else {
                this.clearSuggestions();
            }
        }, debounceMs);
    }

    async clearSuggestions() {
        this._searchGeneration++; // discard in-flight searches
        if (this.searchTimeout) {
            clearTimeout(this.searchTimeout);
            this.searchTimeout = null;
        }
        this._hideSearchLoading();
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
        // After dropdown renders, ensure the window is on-screen
        await this.windowManager.resizeWindow();
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
                    <div class="app-description">${t('floating.suggestions.system.confirm_select')}</div>
                </div>
            `;
        } else {
            item.innerHTML = `
                <div class="app-icon">${cmdLabel.split(' ')[0]}</div>
                <div class="app-info">
                    <div class="app-name">${cmdLabel.substring(cmdLabel.indexOf(' ') + 1)}</div>
                    <div class="app-description">${canElevate ? t('floating.suggestions.system.enter_admin_hint', { keys: platformKeyLabel('Ctrl+Shift+Enter') }) : t('floating.suggestions.system.enter_to_execute')}</div>
                </div>
            `;
        }

        item.addEventListener('click', () =>
            this._executeSystemCommand(cmdId, needsConfirm, false)
        );
        container.appendChild(item);
        container.classList.add('visible');
        this.windowManager.resizeWindow();
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
                    <div class="app-name">${elevated ? t('floating.suggestions.system.are_you_sure_admin') : t('floating.suggestions.system.are_you_sure')}</div>
                    <div class="app-description">${t('floating.suggestions.system.confirm_hint')}</div>
                </div>
            `;
            confirmItem.addEventListener('click', async () => {
                try {
                    await this.invoke('execute_system_command', { commandId: cmdId, elevated });
                } catch (e) {
                    console.error('System command failed:', e);
                }
                this._clearInput();
            });
            container.appendChild(confirmItem);

            this.currentMatches = [{ type: 'system_confirm', cmdId, elevated }];
            this.selectedIndex = 0;
            this.windowManager.resizeWindow();
            return;
        }

        try {
            await this.invoke('execute_system_command', { commandId: cmdId, elevated });
        } catch (e) {
            console.error('System command failed:', e);
        }
        this._clearInput();
    }

    async executeCommandAction(cmd) {
        this._clearInput();
        await cmd.execute(this.invoke, this.appWindow);
    }

    async executeSelection(command, value) {
        this.clearSuggestions();
        try {
            // Shared submit — sends { <command>Name: value }, the arg-shape the
            // agent actually accepts (verified via scripts/probe_slash.py).
            const msg = await submitSelection(this.invoke, this.floatingSessionId, command, value);
            document.dispatchEvent(
                new CustomEvent('kage-show-response', {
                    detail: msg || t('floating.suggestions.selection_fallback', { value }),
                })
            );
        } catch (e) {
            document.dispatchEvent(
                new CustomEvent('kage-show-response', {
                    detail: errLabel(t('floating.error.error_label'), e),
                })
            );
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
            // History navigation: if browsing history, go forward
            if (this._historyIndex >= 0 && this.currentMatches.length === 0) {
                event.preventDefault();
                this._historyIndex--;
                if (this._historyIndex < 0) {
                    // Back to the original input
                    this.elements.input.value = this._historySaved;
                    this._historySaved = '';
                } else {
                    this.elements.input.value = this._messageHistory[this._historyIndex];
                }
                return;
            }
            const itemCount =
                this.elements.appSuggestions.querySelectorAll('.app-suggestion-item').length;
            if (itemCount > 0) {
                // Only navigate suggestions if cursor is on the last line of the textarea
                const ta = this.elements.input;
                const _textBeforeCursor = ta.value.substring(0, ta.selectionStart);
                const textAfterCursor = ta.value.substring(ta.selectionEnd);
                const isLastLine = !textAfterCursor.includes('\n');
                if (isLastLine) {
                    event.preventDefault();
                    this.selectedIndex = (this.selectedIndex + 1) % itemCount;
                    updateSelection(this.elements.appSuggestions, this.selectedIndex);
                }
            }
            // When no suggestions or not on last line, let default behavior handle cursor movement
        } else if (event.key === 'ArrowUp') {
            // History navigation: if input is empty (or already browsing) and no suggestions
            if (this._messageHistory.length > 0 && this.currentMatches.length === 0) {
                const inputVal = this.elements.input.value;
                const isEmpty = inputVal.trim() === '' || this._historyIndex >= 0;
                if (isEmpty && this._historyIndex < this._messageHistory.length - 1) {
                    event.preventDefault();
                    if (this._historyIndex < 0) {
                        this._historySaved = inputVal; // stash whatever was typed
                    }
                    this._historyIndex++;
                    this.elements.input.value = this._messageHistory[this._historyIndex];
                    return;
                }
            }
            const itemCount =
                this.elements.appSuggestions.querySelectorAll('.app-suggestion-item').length;
            if (itemCount > 0) {
                // Only navigate suggestions if cursor is on the first line of the textarea
                const ta = this.elements.input;
                const textBeforeCursor = ta.value.substring(0, ta.selectionStart);
                const isFirstLine = !textBeforeCursor.includes('\n');
                if (isFirstLine) {
                    event.preventDefault();
                    this.selectedIndex =
                        this.selectedIndex <= 0 ? itemCount - 1 : this.selectedIndex - 1;
                    updateSelection(this.elements.appSuggestions, this.selectedIndex);
                }
            }
            // When no suggestions or not on first line, let default behavior handle cursor movement
        } else if (event.key === 'Backspace') {
            // Empty-input backspace dismisses the App Mode chip (same
            // effect as clicking it). Lets users back out of a matched
            // mode without reaching for the mouse. Only fires when the
            // input is genuinely empty so normal text editing isn't
            // affected; the click handler in `_refreshAppModeChip` is
            // still the canonical clear path.
            if (this.elements.input.value === '' && this._appModeMatch && !event.repeat) {
                const chip = document.getElementById('appModeChip');
                if (chip && chip.style.display !== 'none') {
                    event.preventDefault();
                    this._appModeMatch = null;
                    chip.style.display = 'none';
                    return;
                }
            }
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
        } else if (event.key === 'Enter' && cmdOrCtrlPressed(event) && event.shiftKey) {
            // Ctrl/⌘+Shift+Enter: execute as elevated (admin) if it's a system command
            event.preventDefault();
            if (this.currentMatches.length > 0 && this.selectedIndex >= 0) {
                const selected = this.currentMatches[this.selectedIndex];
                if (selected.type === 'system') {
                    await this._executeSystemCommand(selected.cmdId, selected.needsConfirm, true);
                    return;
                }
            }
        } else if (event.key === 'Enter' && cmdOrCtrlPressed(event)) {
            // Ctrl/⌘+Enter: send directly to agent, bypassing suggestions and input classification
            event.preventDefault();
            const message = this.elements.input.value.trim();
            if (message) {
                await this.clearSuggestions();
                await this.sendChatMessage(message, { forceChat: true });
            }
        } else if (event.key === 'Enter' && !event.shiftKey && !cmdOrCtrlPressed(event)) {
            event.preventDefault();
            await this.handleEnterKey();
        }
    }

    async handleEnterKey() {
        // Cancel any pending debounced search so we don't use stale suggestions.
        // When typing fast, the last input event's debounce may not have fired yet,
        // meaning currentMatches reflects an older, partial query.
        if (this.searchTimeout) {
            clearTimeout(this.searchTimeout);
            this.searchTimeout = null;
            // Stale suggestions — clear them so handleEnterAction falls through
            // to direct shortcut/command matching on the actual input value.
            this.currentMatches = [];
            this.selectedIndex = -1;
        }

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
                await new Promise((r) => setTimeout(r, 150));
                try {
                    await this.invoke('paste_clipboard_item', { text: selected.data.text });
                    console.log(
                        '[Clipboard] Pasted to active app:',
                        selected.data.text.slice(0, 50)
                    );
                } catch (e) {
                    console.warn('[Clipboard] Failed to paste:', e);
                }
                return;
            }
        }

        if (!message && !hasAttachments && !hasSelection) return;

        const result = await handleEnterAction({
            message,
            suggestions: this.currentMatches,
            selectedIndex: this.selectedIndex,
            shortcuts: this.shortcuts,
            ctx: this._getExecCtx(),
            onSend: (msg) => this.sendChatMessage(msg),
            onSystemCommand: (cmdId, needsConfirm, elevated) =>
                this._executeSystemCommand(cmdId, needsConfirm, elevated),
            onSelection: (command, value) => this.executeSelection(command, value),
        });

        await this._applyEnterActionResult(result);
    }

    /**
     * Apply the {handled, action} verdict from handleEnterAction. Shared by
     * the Enter key (handleEnterKey) and click-to-execute (_onResultClick) so
     * both paths treat hide / replace_input / keep_suggestions identically.
     */
    async _applyEnterActionResult(result) {
        if (!result?.handled) return;
        if (result.action === 'replace_input') {
            /* input already replaced by onReplaceInput callback */
        } else if (result.action === 'hide') {
            this.resetUI();
            await this.appWindow.hide();
        } else if (result.action === 'keep_suggestions') {
            // A selection picker was just rendered into the suggestions
            // dropdown (e.g. /agent, /model). Clear the input text but
            // DON'T touch the suggestions — clearSuggestions() would wipe
            // the picker we just painted, which was the silent-failure bug.
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
        } else {
            this._clearInput();
        }
    }

    /**
     * Click-to-execute for a unified-search result row. Mirrors pressing
     * Enter on that row: runs it through the same handleEnterAction machinery
     * (so slash commands, system commands, shortcuts, apps, URLs all behave
     * identically whether clicked or keyed) and applies the same verdict.
     */
    async _onResultClick(result) {
        if (!result) return;
        // Point the selection at the clicked row so handleEnterAction executes
        // exactly it, regardless of what was keyboard-highlighted.
        const idx = this.currentMatches.indexOf(result);
        if (idx < 0) return;
        this.selectedIndex = idx;

        // Clipboard mode is paste-on-select — handle it the same way Enter does.
        if (this._clipboardMode && result.type === 'clipboard' && result.data?.text) {
            this._clearInput();
            await this.appWindow.hide();
            await new Promise((r) => setTimeout(r, 150));
            try {
                await this.invoke('paste_clipboard_item', { text: result.data.text });
            } catch (e) {
                console.warn('[Clipboard] Failed to paste:', e);
            }
            return;
        }

        const actionResult = await handleEnterAction({
            message: this.elements.input.value.trim(),
            suggestions: this.currentMatches,
            selectedIndex: this.selectedIndex,
            shortcuts: this.shortcuts,
            ctx: this._getExecCtx(),
            onSend: (msg) => this.sendChatMessage(msg),
            onSystemCommand: (cmdId, needsConfirm, elevated) =>
                this._executeSystemCommand(cmdId, needsConfirm, elevated),
            onSelection: (command, value) => this.executeSelection(command, value),
        });
        await this._applyEnterActionResult(actionResult);
    }

    _clearInput() {
        this.elements.input.value = '';
        this.elements.input.style.height = 'auto';
        this.clearSuggestions();
    }

    async sendChatMessage(message, options = {}) {
        // If bootstrap is still running, show a "Spinning up agent…"
        // placeholder and queue the send. The bootstrap path will
        // call _flushPendingSend() once the session arrives.
        if (this.bootstrappingSession) {
            this._showBootstrapSpinner();
            this._pendingSend = { message, options };
            this._waitForBootstrapAndSend();
            return;
        }
        // Bootstrap previously failed. Rather than latching that error until
        // the app restarts (the old behaviour — a transient backend outage at
        // launch left floating permanently dead), retry the bootstrap. It's
        // debounced so rapid sends can't cascade into repeated reconnects.
        if (this.sessionBootstrapError) {
            this._retryBootstrapAndSend(message, options);
            return;
        }

        // Track message in shell-style history (skip duplicates of the last entry)
        if (
            message.trim() &&
            (this._messageHistory.length === 0 || this._messageHistory[0] !== message.trim())
        ) {
            this._messageHistory.unshift(message.trim());
            if (this._messageHistory.length > 50) this._messageHistory.pop();
        }
        this._historyIndex = -1;
        this._historySaved = '';

        // Stop any ongoing TTS; in voice mode, don't kill the mic — it will restart after response
        if (this.speech) {
            this.speech.cancelSpeech();
            if (this.speech.isListening && !this.speech.voiceMode) {
                this.speech.stop();
            }
        }

        // If a plan is pending review, send the message as a revision request
        if (this._pendingPlanRevision) {
            this.automationPlanController.reset();
            this.extensionToolController.reset();
            // Reset UI for the new response
            this.elements.input.value = '';
            this.elements.input.style.height = 'auto';
            this.currentResponse = '';
            this.elements.responseText.textContent = '';
            this.elements.contentArea.classList.add('visible');
            this.isWaitingForResponse = true;
            this._promptGeneration++;
            this.startThinking();
            this.updateDatetimeVisibility();
            await this.windowManager.resizeWindow();
            try {
                // Notify the chat window so it can show the user bubble
                window.__TAURI__.event.emit('floating_message_sent', { message });
                trackEvent('message_sent', {
                    source: 'floating',
                    length: messageLengthBucket(message),
                });
                await this.invoke('send_message_streaming', {
                    sessionId: this.floatingSessionId,
                    message,
                    attachments: null,
                });
            } catch (e) {
                this.showError(errLabel(t('floating.error.error_label'), e));
            }
            return;
        }

        const attachments = this.attachmentManager.toContentBlocks();
        this.attachmentManager.clear();

        // Include selected text as context if checkbox is checked
        const useSelection =
            !options.skipSelection && document.getElementById('useSelectionCheckbox')?.checked;
        if (useSelection && this.lastSelection?.trim()) {
            message = `The following text is currently selected in my active window:\n\`\`\`\n${this.lastSelection.trim()}\n\`\`\`\n\n${message}`;
        }
        // Hide selection indicator after use
        const indicator = document.getElementById('selectionIndicator');
        if (indicator) indicator.style.display = 'none';
        this._setSelectionPreview(null);
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
                } catch {
                    rustResults = [];
                }
            }
            this._noMatchSinceLen = 0;

            // Check if the top result is a URL, path, or app launch
            const top = rustResults[0];
            if (top?.type === 'url') {
                await this.openUrl(top.value);
            } else if (top?.type === 'path') {
                await this.openPath(top.value);
            } else if (top?.type === 'app') {
                await this.launchApp(top.name);
            } else {
                // No actionable match — send to agent. Reset UI now.
                // If a response is in progress, cancel it first
                if (this.isWaitingForResponse) {
                    this.invoke('cancel_generation', {
                        sessionId: this.floatingSessionId,
                    }).catch((e) => console.log('Cancel:', e));
                    this.isWaitingForResponse = false;
                    this.stopThinking();
                    this.elements.floatingStopBtn.style.display = 'none';
                    const indicator =
                        this.elements.responseText.querySelector('.streaming-indicator');
                    if (indicator) indicator.remove();
                }

                this.elements.contentArea.classList.remove('visible');
                this.toolSources = [];
                this.toolUsages = [];
                this._toolCallIds = new Set();
                this._sourceDomains = new Set();
                const sourcesEl2 = document.getElementById('toolSources');
                if (sourcesEl2) sourcesEl2.remove();
                const compactEl2 = document.getElementById('toolSourcesCompact');
                if (compactEl2) compactEl2.remove();
                await this.windowManager.resetHeightForNewMessage();
                this.startThinking();
                this.updateDatetimeVisibility();
                this.elements.expandBtn.classList.remove('visible');

                // No actionable match — send to agent
                this.currentResponse = '';
                this.elements.responseText.textContent = this.currentResponse;
                this.elements.contentArea.classList.add('visible');
                this.elements.expandBtn.classList.add('visible');
                this.isWaitingForResponse = true;
                this.extensionToolController.reset();
                this.automationPlanController.reset();
                this._promptGeneration++;
                const _gen = this._promptGeneration;
                await this.windowManager.resizeWindow();
                this.banner.dismiss();

                // Prepend screen context (source window info) and any
                // App Mode steering. Both ride at the head of the
                // outgoing prompt so the agent sees them before the
                // actual user message. App-mode steering travels with
                // every prompt where it applies — consciously kept
                // light (per-rule cap of 500 chars) so token cost
                // stays small even on long conversations.
                try {
                    const config = await getConfig(this.invoke);
                    if (config?.system?.screen_context) {
                        const sw = await this.invoke('get_source_window');
                        if (sw) {
                            message = `<_kage_ctx app="${sw.processName}" title="${sw.title}"/>\n${message}`;
                        }
                    }
                } catch (e) {
                    console.log('Screen context unavailable:', e);
                }

                // App Mode steering — _appModeMatch was set by
                // _refreshAppModeChip when the user summoned. Click-
                // dismiss clears it without touching config; we just
                // skip the splice in that case.
                if (this._appModeMatch?.steering_payload) {
                    message = `${this._appModeMatch.steering_payload}\n${message}`;
                }

                // Notify the chat window so it can show the user bubble
                window.__TAURI__.event.emit('floating_message_sent', { message });
                trackEvent('message_sent', {
                    source: 'floating',
                    length: messageLengthBucket(message),
                    attachments: attachments?.length || 0,
                });
                await this.invoke('send_message_streaming', {
                    sessionId: this.floatingSessionId,
                    message,
                    attachments,
                });
            }
        } catch (error) {
            console.error('Error handling input:', error);
            this.showError(errLabel(t('floating.error.error_label'), error));
        }
    }

    /** Force the streaming renderer to paint the full accumulated text now.
     *  Called by the permission modal handler before showing the dialog so
     *  the user sees the complete streamed text behind it. */
    flushStreamingRender() {
        this.messageStreamController.flushStreamingRender();
    }

    handleMessageChunk(event) {
        return this.messageStreamController.handleChunk(event);
    }

    async handleMessageComplete() {
        return this.messageStreamController.handleComplete();
    }

    /**
     * Render a loading indicator while an extension tool call is being streamed.
     * Window-specific DOM — wired into the ExtensionToolController via host adapter.
     * Tool-usage tracking is handled by the controller before this fires.
     */
    _renderExtensionToolIndicator(info) {
        const beforeFence = this.currentResponse.split('```extension_tool_call')[0].trim();
        if (beforeFence) {
            renderMarkdown(beforeFence, this.elements.responseText, true);
        } else {
            const friendlyName = this.extensionToolController.getExtensionToolFriendlyName(
                info.extension,
                info.tool
            );
            this.elements.responseText.innerHTML = `<div class="folder-plan-spinner-row"><span class="folder-plan-spinner"></span> ${escapeHtml(friendlyName)}...</div>`;
        }
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
                    copyBtn.innerHTML =
                        '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>';
                    setTimeout(() => {
                        copyBtn.innerHTML =
                            '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>';
                    }, 1500);
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
            const config = await getConfig(this.invoke);
            if (!config.ui?.show_response_actions) return;
            const qaConfig = config.quick_actions || { enabled: true, custom_actions: [] };
            const actions = await getActionsForText(responseText, qaConfig);
            console.log('[QA] Actions found:', actions.length);
            if (actions.length === 0) return;
            const container = document.getElementById('responseActionsContainer');
            if (container) {
                container.innerHTML = '';
                for (const action of actions) {
                    const chip = document.createElement('button');
                    chip.className = 'quick-action-chip';
                    chip.title = action.label;
                    const iconSpan = document.createElement('span');
                    iconSpan.className = 'quick-action-icon';
                    iconSpan.textContent = action.icon || '⚡';
                    const labelSpan = document.createElement('span');
                    labelSpan.className = 'quick-action-label';
                    labelSpan.textContent = action.label;
                    chip.appendChild(iconSpan);
                    chip.appendChild(labelSpan);
                    chip.addEventListener('click', () => {
                        const prompt = action.prompt.replace(/\{text\}/g, responseText);
                        container.style.display = 'none';
                        this.sendChatMessage(prompt, { skipSelection: true });
                    });
                    container.appendChild(chip);
                }
                container.style.display = 'flex';
                await this.windowManager.resizeWindow();
            }
        } catch (e) {
            console.warn('[QA] Response actions error:', e);
        }
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

    _showCompactionIndicator() {
        // Show a subtle notice that context is being compacted
        let notice = document.getElementById('compactionNotice');
        if (!notice) {
            notice = document.createElement('div');
            notice.id = 'compactionNotice';
            notice.className = 'compaction-notice';
            notice.innerHTML = `<span class="folder-plan-spinner"></span> ${t('floating.compaction.in_progress')}`;
            this.elements.responseText?.appendChild(notice);
        }
    }

    _hideCompactionIndicator() {
        const notice = document.getElementById('compactionNotice');
        if (notice) notice.remove();
    }

    async handleMessageError(event) {
        return this.messageStreamController.handleError(event);
    }

    handleSessionReset(event) {
        return this.messageStreamController.handleSessionReset(event);
    }

    handleToolCallUpdate(event) {
        return this.messageStreamController.handleToolCallUpdate(event);
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
        sourcesEl.innerHTML =
            renderToolChipsHtml(this.toolUsages) + renderSourceChipsHtml(this.toolSources);
        attachSourceClickHandler(sourcesEl, this.invoke);
        this.windowManager.resizeWindow();
    }

    renderSourcesCompact() {
        this.elements.loadingDots.classList.remove('visible');
        this.elements.mascotContainer.classList.remove('thinking');

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
        attachSourceClickHandler(compactEl, this.invoke);
        this.windowManager.resizeWindow();
    }

    showError(message, opts = {}) {
        this.stopThinking();
        this.currentResponse = message;
        this.elements.contentArea.classList.add('visible');
        this.elements.expandBtn.classList.add('visible');

        // For connection errors, offer a Reconnect affordance (parity with the
        // chat window). Other errors (shortcut failed, no session, etc.) stay
        // plain text. Build via DOM API so `message` never touches innerHTML.
        this.elements.responseText.textContent = '';
        const msgSpan = document.createElement('span');
        msgSpan.textContent = message;
        this.elements.responseText.appendChild(msgSpan);

        if (opts.reconnect) {
            const btn = document.createElement('button');
            btn.className = 'floating-error-reconnect';
            btn.textContent = t('chat.error.btn.reconnect');
            btn.addEventListener('click', async () => {
                btn.disabled = true;
                try {
                    const success = await this.invoke('reconnect_acp');
                    if (success) {
                        this.elements.responseText.textContent = '';
                        this.elements.contentArea.classList.remove('visible');
                        this.elements.expandBtn.classList.remove('visible');
                        this.windowManager.resizeWindow();
                        return;
                    }
                } catch (e) {
                    console.log('Reconnect failed:', e);
                }
                btn.disabled = false;
            });
            this.elements.responseText.appendChild(document.createElement('br'));
            this.elements.responseText.appendChild(btn);
        }

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
        // Don't hide if an extension tool is being processed
        if (this._extensionToolExecuting || this._extensionToolCallHandled) {
            return;
        }

        // Don't hide if we just finished resizing or dragging — the mouseup
        // outside the window boundary fires a click event we should ignore.
        if (this.windowManager.isResizing || this.windowManager.isDragging) return;
        if (
            this.windowManager._resizeEndedAt &&
            Date.now() - this.windowManager._resizeEndedAt < 300
        )
            return;

        const container = document.querySelector('.floating-container');
        if (container && !container.contains(event.target)) {
            // Don't hide if a sandbox iframe is running (Try button)
            if (window._kageSandboxActive) return;
            await this.appWindow.hide();
        }
    }
}
