// Expanded chat application logic
import {
    renderMarkdown,
    initMarkdown,
    createTaskPlanElement,
    setAppIconInvoke,
    _resetDiagramFailures,
} from '../shared/markdown.js';
import {
    AttachmentManager,
    handlePasteEvent,
    setupDragDrop,
    renderAttachmentPreviews,
    attachmentPreviewHtml,
    sessionImageToDataUrl,
} from '../shared/attachments.js';
import {
    loadSlashCommands,
    executeCommand,
    getSlashCommandMeta,
    getSlashCommandDispatch,
} from '../shared/commands.js';
import { buildChatMarkdown, defaultExportFilename } from '../shared/chat-export.js';
import { escapeHtml, stripKageTags } from '../shared/tool-utils.js';
import { EVT } from '../shared/events.js';
import { WINDOW, isChatLabel } from '../shared/window-labels.js';
import { applyMixin } from '../shared/mixin.js';
import { getWindowSessionOrNull } from '../shared/session-resolve.js';
import { SessionStreamRegistry, STREAM } from '../shared/session-streams.js';
import { errLabel } from '../shared/error-message.js';
import { t } from '../shared/i18n.js';
import { submitSelection, loadSelection } from '../shared/slash-selection.js';
import { mascotHTML } from '../shared/mascot.js';
import {
    isOnline,
    checkOnline,
    markOnline,
    onNetworkChange,
    offlineMessage,
} from '../shared/network.js';
import {
    renderToolChipsHtml,
    renderSourceChipsHtml,
    attachSourceClickHandler,
    processToolCallUpdate,
} from '../shared/streaming-utils.js';
import { sendAppNotification } from '../shared/notify.js';
import { SpeechController } from '../shared/speech.js';
import { ExtensionManager } from '../shared/extension-manager.js';
import {
    unifiedSearch,
    loadFrecency,
    setExtensionManager,
    getExtensionManager,
} from '../shared/search-engine.js';
import { cmdOrCtrlPressed } from '../shared/shortcuts.js';
import {
    executeResult as executeResultShared,
    handleEnterAction,
} from '../shared/result-executor.js';
import { getActionsForText } from '../shared/quick-actions.js';
import { setupRtlDetection } from '../shared/rtl.js';
import { sanitizeExtensionHtml as sanitizeExtensionHtmlStatic } from '../shared/extension-html-sanitizer.js';
import { renderToolbarButtons } from '../shared/extension-toolbar.js';
import { getConfig } from '../shared/config-cache.js';
import { parseContextPercent, drawContextRing } from '../shared/context-usage.js';
import { ExtensionToolController } from '../shared/extension-tool-controller.js';
import { AutomationPlanController } from '../shared/automation-plan-controller.js';
import { MessageStreamController } from '../shared/message-stream-controller.js';
import { trackEvent, messageLengthBucket } from '../shared/telemetry.js';
import {
    buildRenderQueue,
    formatDuration,
    formatRelativeDate,
    formatError as formatErrorShared,
    orderSessionsForSidebar,
} from '../shared/session-render.js';

import { createLifecycleMixin } from './app/lifecycle.js';
import { createStreamListenersMixin } from './app/stream-listeners.js';
import { createSessionStateMixin } from './app/session-state.js';
import { createSessionSidebarMixin } from './app/session-sidebar.js';
import { createSessionHistoryMixin } from './app/session-history.js';
import { createSessionActionsMixin } from './app/session-actions.js';
import { createComposerMixin } from './app/composer.js';
import { createMessagesMixin } from './app/messages.js';
import { createChatActionsMixin } from './app/chat-actions.js';
import { createModelsToolbarMixin } from './app/models-toolbar.js';

const CHAT_APP_DEPENDENCIES = {
    attachmentPreviewHtml,
    attachSourceClickHandler,
    buildChatMarkdown,
    buildRenderQueue,
    checkOnline,
    cmdOrCtrlPressed,
    defaultExportFilename,
    drawContextRing,
    errLabel,
    escapeHtml,
    EVT,
    executeCommand,
    executeResultShared,
    ExtensionManager,
    ExtensionToolController,
    formatDuration,
    formatErrorShared,
    formatRelativeDate,
    getActionsForText,
    getConfig,
    getExtensionManager,
    getSlashCommandDispatch,
    getSlashCommandMeta,
    getWindowSessionOrNull,
    handleEnterAction,
    handlePasteEvent,
    initMarkdown,
    isChatLabel,
    isOnline,
    loadFrecency,
    loadSelection,
    loadSlashCommands,
    mascotHTML,
    messageLengthBucket,
    offlineMessage,
    onNetworkChange,
    orderSessionsForSidebar,
    parseContextPercent,
    processToolCallUpdate,
    renderAttachmentPreviews,
    renderMarkdown,
    renderSourceChipsHtml,
    renderToolbarButtons,
    renderToolChipsHtml,
    sanitizeExtensionHtmlStatic,
    sessionImageToDataUrl,
    setAppIconInvoke,
    setExtensionManager,
    setupDragDrop,
    setupRtlDetection,
    SpeechController,
    STREAM,
    stripKageTags,
    submitSelection,
    t,
    trackEvent,
    unifiedSearch,
    WINDOW,
};

export class ChatApp {
    constructor(invoke, appWindow, listen) {
        this.invoke = invoke;
        this.appWindow = appWindow;
        this.listen = listen;

        // Tauri webview label of the window we run in. `main` is the
        // privileged chat window from tauri.conf.json; `chat-<uuid>`
        // are peers spawned via open_new_chat_window. Used everywhere
        // we need to read/write per-window state (session bookkeeping,
        // permission routing).
        this.windowLabel = appWindow?.label || WINDOW.MAIN;

        this.messages = [];
        this.currentStreamingMessage = null;
        this.currentStreamingContent = '';
        this.isWaitingForResponse = false;
        this.isConnected = false;
        // Per-session stream tracking: which sessions have a turn in
        // flight / completed-unviewed. The window is a viewport; streams
        // belong to sessions, not to the window. See session-streams.js.
        this.streamRegistry = new SessionStreamRegistry();
        this.streamRegistry.onChange(() => this._refreshSessionBadges());
        this.sessions = [];
        this._sessionsFullyLoaded = false;
        this._loadingMore = false;
        this._seenSessionIds = new Set();
        this.activeSessionId = null;
        this.floatingSessionId = null;
        this.currentAcpSessionId = null;
        this.toolSources = [];
        this.toolUsages = [];
        this._toolCallIds = new Set();
        this._sourceDomains = new Set();
        this.userInfo = null;
        this.attachmentManager = new AttachmentManager();
        this.currentSuggestions = [];
        this.suggestionIndex = -1;
        this.availableModels = [];
        this.currentModelId = null;
        this._showSpeakBtn = false;
        this._showTranslateBtn = false;

        // Per-message DOM target the controller renders into; set when a chunk
        // hits the chat for the first time and consumed by renderIndicator.
        this._extensionToolContentDiv = null;

        const app = this;
        this.extensionToolController = new ExtensionToolController({
            invoke,
            getSessionId: () => app.activeSessionId,
            get extensionManager() {
                return app.extensionManager;
            },
            permissionModal: {
                showForExtensionTool: (...args) =>
                    window.ChatPermissions.showForExtensionTool(...args),
            },
            addToolUsage: (entry) => {
                if (!app._toolCallIds) app._toolCallIds = new Set();
                if (app._toolCallIds.has(entry.toolCallId)) return;
                app._toolCallIds.add(entry.toolCallId);
                app.toolUsages.push(entry);
                if (app._extensionToolContentDiv) {
                    app.renderSourcesInMessage(app._extensionToolContentDiv);
                }
            },
            renderIndicator: (info) =>
                app._renderExtensionToolIndicator(info, app._extensionToolContentDiv),
            onExecuteStart: () => {},
            onExecuteEnd: () => {},
            onWaitForFollowup: () => {
                app.isWaitingForResponse = true;
                app.showTypingIndicator();
            },
            resetAccumulator: () => {
                app.currentStreamingContent = '';
            },
        });

        // Per-message DOM target the automation-plan controller renders into.
        // Set by chunk/complete handlers before delegating to the controller.
        this._automationContentDiv = null;
        this.automationPlanController = new AutomationPlanController({
            invoke,
            listen,
            getSessionId: () => app.activeSessionId,
            renderTasks: (tasks) => {
                if (!app._automationContentDiv) return;
                const wrapper = createTaskPlanElement(tasks);
                app._automationContentDiv.innerHTML = '';
                app._automationContentDiv.appendChild(wrapper);
                app.scrollToBottom();
            },
            appendReviewActions: (bar) => {
                if (app._automationContentDiv) app._automationContentDiv.appendChild(bar);
            },
            onPlanReadyForReview: () => {
                app.hideTypingIndicator();
                app.isWaitingForResponse = false;
                app.updateInputState();
                app.elements.chatInput.focus();
                app.scrollToBottom();
            },
            onPlanExecutionStart: () => {
                app.isWaitingForResponse = true;
                app.updateInputState();
                app.scrollToBottom();
            },
            onPlanComplete: () => {
                app.messages.push({ role: 'assistant', content: '[Automation plan completed]' });
                app.currentStreamingMessage = null;
                app.currentStreamingContent = '';
                app.isWaitingForResponse = false;
                app.updateInputState();
                app.elements.chatInput.focus();
                app.scrollToBottom();
                app.loadSessions();
            },
            onPlanFailed: () => {
                app.isWaitingForResponse = false;
                app.updateInputState();
            },
        });

        this.messageStreamController = new MessageStreamController({
            // processToolCallUpdate() writes tool/source tracking straight
            // onto the host adapter — forward to the app-instance arrays the
            // render paths (renderSourcesInMessage) read.
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
            isWaiting: () => app.isWaitingForResponse && !!app.currentStreamingMessage,
            // Session filter: accept only chunks that belong to OUR session.
            // Reject when we have no pinned session (null activeSessionId) to
            // prevent vacuuming up chunks from other windows' sessions.
            acceptSessionId: (sid) => {
                if (!sid) return !!app.activeSessionId;
                if (!app.activeSessionId) return false;
                return sid === app.activeSessionId;
            },
            getAccumulator: () => app.currentStreamingContent,
            appendToAccumulator: (delta) => {
                app.currentStreamingContent = (app.currentStreamingContent || '') + delta;
            },
            resetAccumulator: () => {
                app.currentStreamingContent = '';
            },
            automationPlanController: this.automationPlanController,
            extensionToolController: this.extensionToolController,
            onChunkAppended: () => {
                // Stash per-message DOM target the controllers need; chat
                // re-resolves it every chunk because the streaming message
                // element is held on `currentStreamingMessage`.
                const cd = app.currentStreamingMessage?.querySelector('.message-content') || null;
                app._automationContentDiv = cd;
                app._extensionToolContentDiv = cd;
                app.hideTypingIndicator();
            },
            bumpLayout: () => app.scrollToBottom(),
            renderStreaming: (text) => {
                const contentDiv = app.currentStreamingMessage.querySelector('.message-content');
                renderMarkdown(text, contentDiv, true);
                const toolSpinner = contentDiv.querySelector('.tool-running-indicator');
                if (toolSpinner) toolSpinner.remove();
                let indicator = contentDiv.querySelector('.streaming-indicator');
                if (!indicator) {
                    indicator = document.createElement('span');
                    indicator.className = 'streaming-indicator';
                    indicator.textContent = '...';
                    contentDiv.appendChild(indicator);
                }
                app.scrollToBottom();
            },
            feedTTS: (text) => {
                if (app.speech) app.speech.feedStreamingText(text);
            },
            onCompleteHeader: () => {
                markOnline();
                app.isConnected = true;
                app.updateConnectionStatus();
            },
            // Chat doesn't drop empty completes — it still needs to clean up
            // the streaming message element. Always proceed.
            dropEmptyComplete: () => false,
            onBeforeFinalRender: () => {
                app.hideTypingIndicator();
                if (app.currentStreamingMessage) {
                    const contentDiv =
                        app.currentStreamingMessage.querySelector('.message-content');
                    const indicator = contentDiv?.querySelector('.streaming-indicator');
                    if (indicator) indicator.remove();
                    // Stash the contentDiv so the automation-plan fallback
                    // controller knows where to render if it fires next.
                    app._automationContentDiv = contentDiv;
                }
            },
            renderFinal: (text) => {
                if (!app.currentStreamingMessage) return;
                const contentDiv = app.currentStreamingMessage.querySelector('.message-content');
                renderMarkdown(text, contentDiv);
                if (app.toolSources.length > 0 || app.toolUsages.length > 0) {
                    app.renderSourcesInMessage(contentDiv);
                }
                app.messages.push({ role: 'assistant', content: text });
                app.currentStreamingMessage = null;
            },
            onAfterFinalRender: async (_text) => {
                const finalContent = app.messages[app.messages.length - 1]?.content || '';
                app.currentStreamingContent = '';
                app.isWaitingForResponse = false;
                app.updateInputState();
                app.elements.chatInput.focus();
                app.scrollToBottom();

                if (app.speech && finalContent) {
                    app.speech.finishStreamingText(finalContent);
                    app.speech.speakResponse(finalContent);
                }

                app.loadSessions();
                app.loadFloatingSessionId();

                // Set timestamp on the message actions bar
                if (app.elements.messagesArea.lastElementChild) {
                    const msgEl = app.elements.messagesArea.querySelector(
                        '.message.agent:last-of-type'
                    );
                    const ts = msgEl?.querySelector('.msg-timestamp');
                    if (ts) {
                        let label = new Date().toLocaleTimeString([], {
                            hour: '2-digit',
                            minute: '2-digit',
                        });
                        if (app._streamStartTime) {
                            const durSecs = (Date.now() - app._streamStartTime) / 1000;
                            label += ` (${app._formatDuration(durSecs)})`;
                        }
                        ts.textContent = label;
                    }
                }

                app.showSuggestionChips();

                try {
                    // Notify when the response lands while the chat window is
                    // unfocused. Must NOT gate on isWaitingForResponse — it was
                    // just set false at the top of onAfterFinalRender, so that
                    // condition was always false and the notification never
                    // fired. The floating window uses the same `!focused && text`
                    // test.
                    if (finalContent && !app._windowFocused) {
                        const preview = finalContent
                            .substring(0, 100)
                            .replace(/[#*`\n]/g, ' ')
                            .trim();
                        await sendAppNotification(
                            app.invoke,
                            t('shared.notify.kage_title'),
                            preview || t('chat.notification.response_ready'),
                            WINDOW.MAIN
                        );
                    }
                } catch {
                    /* ignore */
                }
            },
            onError: async (event, online) => {
                app.hideTypingIndicator();
                if (app.currentStreamingMessage) {
                    app.currentStreamingMessage.remove();
                    app.currentStreamingMessage = null;
                }
                app.isWaitingForResponse = false;
                app.updateInputState();
                app.elements.chatInput.focus();
                _resetDiagramFailures();
                if (!online) app.showError(offlineMessage());
                else app.showError(t('chat.error.error_with_payload', { payload: event.payload }));
                app.isConnected = online;
                app.updateConnectionStatus();
            },
            onSessionReset: (event, msg) => {
                app.hideTypingIndicator();
                _resetDiagramFailures();
                if (app.currentStreamingMessage) {
                    app.currentStreamingMessage.remove();
                    app.currentStreamingMessage = null;
                }
                const data = event.payload;
                if (data?.reason === 'image_unsupported' && data.reconnected) {
                    app.isConnected = true;
                    app.updateConnectionStatus();
                    app.showSessionResetMessage(msg);
                } else {
                    if (data?.reason === 'image_unsupported') {
                        app.isConnected = false;
                        app.updateConnectionStatus();
                    }
                    app.showError(msg);
                }
                app.isWaitingForResponse = false;
                app.updateInputState();
                app.elements.chatInput.focus();
                app.loadSessions();
            },
            flushPendingMarkdown: () => {
                if (app.currentStreamingMessage && app.currentStreamingContent) {
                    const contentDiv =
                        app.currentStreamingMessage.querySelector('.message-content');
                    if (contentDiv) renderMarkdown(app.currentStreamingContent, contentDiv);
                }
            },
            showToolRunningSpinner: (friendly) => {
                if (!app.currentStreamingMessage) return;
                const contentDiv = app.currentStreamingMessage.querySelector('.message-content');
                if (!contentDiv) return;
                let spinner = contentDiv.querySelector('.tool-running-indicator');
                if (!spinner) {
                    spinner = document.createElement('div');
                    spinner.className = 'folder-plan-spinner-row tool-running-indicator';
                    contentDiv.appendChild(spinner);
                }
                // `friendly` can be the raw agent-supplied tool title when it
                // isn't in the friendly-name map — must be escaped.
                spinner.innerHTML = `<span class="folder-plan-spinner"></span> ${escapeHtml(friendly)}...`;
            },
            // Chat doesn't render sources inline during streaming — they're
            // rendered once when the message completes.
            onToolCallTracked: () => {},
        });

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
}

applyMixin(ChatApp.prototype, createLifecycleMixin(CHAT_APP_DEPENDENCIES));
applyMixin(ChatApp.prototype, createStreamListenersMixin(CHAT_APP_DEPENDENCIES));
applyMixin(ChatApp.prototype, createSessionStateMixin(CHAT_APP_DEPENDENCIES));
applyMixin(ChatApp.prototype, createSessionSidebarMixin(CHAT_APP_DEPENDENCIES));
applyMixin(ChatApp.prototype, createSessionHistoryMixin(CHAT_APP_DEPENDENCIES));
applyMixin(ChatApp.prototype, createSessionActionsMixin(CHAT_APP_DEPENDENCIES));
applyMixin(ChatApp.prototype, createComposerMixin(CHAT_APP_DEPENDENCIES));
applyMixin(ChatApp.prototype, createMessagesMixin(CHAT_APP_DEPENDENCIES));
applyMixin(ChatApp.prototype, createChatActionsMixin(CHAT_APP_DEPENDENCIES));
applyMixin(ChatApp.prototype, createModelsToolbarMixin(CHAT_APP_DEPENDENCIES));
