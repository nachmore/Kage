// Floating application composition root.
import {
    _resetDiagramFailures,
    AttachmentManager,
    AutomationPlanController,
    BannerController,
    createTaskPlanElement,
    errLabel,
    escapeHtml,
    ExtensionManager,
    ExtensionToolController,
    extractSuggestedActions,
    markOnline,
    MessageStreamController,
    offlineMessage,
    renderMarkdown,
    sendAppNotification,
    t,
    WINDOW,
    WindowManager,
} from './app/dependencies.js';
import { LifecycleInitMethods } from './app/lifecycle-init.js';
import { LifecycleEventsMethods } from './app/lifecycle-events.js';
import { LifecycleVisibilityMethods } from './app/lifecycle-visibility.js';
import { UiStateMethods } from './app/ui-state.js';
import { SessionMethods } from './app/session.js';
import { CommandsMethods } from './app/commands.js';
import { SearchMethods } from './app/search.js';
import { InputMethods } from './app/input.js';
import { ResponseUiMethods } from './app/response-ui.js';

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
                _resetDiagramFailures();
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
                _resetDiagramFailures();
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
                // `friendly` can be the raw agent-supplied tool title when it
                // isn't in the friendly-name map — must be escaped.
                spinner.innerHTML = `<span class="folder-plan-spinner"></span> ${escapeHtml(friendly)}...`;
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

    static get SEARCH_LOADING_DELAY_MS() {
        return 500;
    }
}

Object.assign(
    FloatingApp.prototype,
    LifecycleInitMethods,
    LifecycleEventsMethods,
    LifecycleVisibilityMethods,
    UiStateMethods,
    SessionMethods,
    CommandsMethods,
    SearchMethods,
    InputMethods,
    ResponseUiMethods
);
