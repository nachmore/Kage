// Main application logic
import { updateSelection, appendSendHint } from '../suggestions.js';
import { WindowManager } from '../window.js';
import {
    renderMarkdown,
    createTaskPlanElement,
    setAppIconInvoke,
    _resetDiagramFailures,
} from '../../shared/markdown.js';
import { loadSlashCommands } from '../../shared/commands.js';
import { submitSelection } from '../../shared/slash-selection.js';
import {
    AttachmentManager,
    handlePasteEvent,
    renderAttachmentPreviews,
} from '../../shared/attachments.js';
import {
    renderToolChipsHtml,
    renderSourceChipsHtml,
    renderSourceBubblesHtml,
    attachSourceClickHandler,
    extractSuggestedActions,
} from '../../shared/streaming-utils.js';
import { sendAppNotification } from '../../shared/notify.js';
import { EVT } from '../../shared/events.js';
import { WINDOW } from '../../shared/window-labels.js';
import { getWindowSessionOrNull } from '../../shared/session-resolve.js';
import { errLabel, errMessage } from '../../shared/error-message.js';
import { getActionsForText, renderQuickActionChips } from '../../shared/quick-actions.js';
import {
    startTimer,
    startStopwatch,
    pauseResumeSlot,
    stopSlot,
    getSlotState,
    updateTimerBar,
    setupTimerBarControls,
} from '../timer.js';
import { playTimerSound } from '../../shared/timer-sounds.js';
import {
    unifiedSearch,
    renderUnifiedResults,
    loadFrecency,
    setExtensionManager,
    searchDebounceMs,
} from '../search-unified.js';
import { buildExecCtx } from '../../shared/exec-context.js';
import { ExtensionManager } from '../../shared/extension-manager.js';
import { SpeechController } from '../../shared/speech.js';
import {
    matchShortcut as matchShortcutFn,
    buildShortcutCommand as buildShortcutCommandFn,
    cmdOrCtrlPressed,
    platformKeyLabel,
} from '../../shared/shortcuts.js';
import {
    isClipboardTrigger,
    getClipboardFilter,
    fetchClipboardHistory,
    filterClipboardHistory,
    renderClipboardHistory,
} from '../clipboard-history.js';
import { mountPromptForm } from '../../shared/prompt-form.js';
import { executeShortcutCommand, handleEnterAction } from '../../shared/result-executor.js';
import { setupRtlDetection } from '../../shared/rtl.js';
import { escapeHtml, formatBytes } from '../../shared/tool-utils.js';
import { checkOnline, markOnline, onNetworkChange, offlineMessage } from '../../shared/network.js';
import { getConfig, onConfigChange } from '../../shared/config-cache.js';
import { parseContextPercent, drawContextRing } from '../../shared/context-usage.js';
import { ExtensionToolController } from '../../shared/extension-tool-controller.js';
import { AutomationPlanController } from '../../shared/automation-plan-controller.js';
import { MessageStreamController } from '../../shared/message-stream-controller.js';
import { trackEvent, messageLengthBucket } from '../../shared/telemetry.js';
import {
    hideExtensionBar,
    showExtensionBar,
    updateExtensionBar,
} from '../../shared/extension-bar.js';
import { sanitizeExtensionHtml } from '../../shared/extension-html-sanitizer.js';
import { renderToolbarButtons } from '../../shared/extension-toolbar.js';
import { runToolbarHostEffect } from '../../shared/toolbar-host-effects.js';
import { BannerController } from '../banner.js';
import { t } from '../../shared/i18n.js';

export {
    _resetDiagramFailures,
    appendSendHint,
    AttachmentManager,
    attachSourceClickHandler,
    AutomationPlanController,
    BannerController,
    buildExecCtx,
    buildShortcutCommandFn,
    checkOnline,
    cmdOrCtrlPressed,
    createTaskPlanElement,
    drawContextRing,
    errLabel,
    errMessage,
    escapeHtml,
    EVT,
    executeShortcutCommand,
    ExtensionManager,
    ExtensionToolController,
    extractSuggestedActions,
    fetchClipboardHistory,
    filterClipboardHistory,
    formatBytes,
    getActionsForText,
    getClipboardFilter,
    getConfig,
    getSlotState,
    getWindowSessionOrNull,
    handleEnterAction,
    handlePasteEvent,
    hideExtensionBar,
    isClipboardTrigger,
    loadFrecency,
    loadSlashCommands,
    markOnline,
    matchShortcutFn,
    messageLengthBucket,
    MessageStreamController,
    mountPromptForm,
    offlineMessage,
    onConfigChange,
    onNetworkChange,
    parseContextPercent,
    pauseResumeSlot,
    platformKeyLabel,
    playTimerSound,
    renderAttachmentPreviews,
    renderClipboardHistory,
    renderMarkdown,
    renderQuickActionChips,
    renderSourceBubblesHtml,
    renderSourceChipsHtml,
    renderToolbarButtons,
    renderToolChipsHtml,
    renderUnifiedResults,
    runToolbarHostEffect,
    sanitizeExtensionHtml,
    searchDebounceMs,
    sendAppNotification,
    setAppIconInvoke,
    setExtensionManager,
    setupRtlDetection,
    setupTimerBarControls,
    showExtensionBar,
    SpeechController,
    startStopwatch,
    startTimer,
    stopSlot,
    submitSelection,
    t,
    trackEvent,
    unifiedSearch,
    updateExtensionBar,
    updateSelection,
    updateTimerBar,
    WINDOW,
    WindowManager,
};
