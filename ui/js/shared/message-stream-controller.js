/**
 * Message Stream Controller — shared between FloatingApp and ChatApp.
 *
 * Owns the pipeline shape of the five streaming events:
 *   - message_chunk      — append delta, dispatch to automation/extension
 *                          controllers, render streaming markdown
 *   - message_complete   — final render, post-completion UI
 *   - message_error      — error UI
 *   - session_reset      — show reset notice, restore input
 *   - tool_call_update   — process tool tracking, flush pending render,
 *                          show running-tool spinner
 *
 * Window-specific bits (render target, lifecycle UI, session filter, the
 * completion-path side panel) flow through the host adapter. The two prior
 * extractions (ExtensionToolController, AutomationPlanController) plug in
 * here through the host.
 *
 * Host adapter shape:
 *   - isWaiting()                     bool — gate for all chunk/complete events
 *   - acceptSessionId(sid)            bool — chunk session filter; chat does
 *                                     a belt-and-suspenders check, floating
 *                                     always returns true
 *   - getAccumulator()                string — current accumulated text
 *   - appendToAccumulator(delta)      void  — append the delta
 *   - resetAccumulator()              void  — clear after final render
 *   - automationPlanController        the AutomationPlanController instance
 *   - extensionToolController         the ExtensionToolController instance
 *   - onChunkAppended(text)           floating: hide mascot/loading + expose
 *                                     content area; chat: hide typing indicator
 *   - bumpLayout()                    floating: resizeWindow; chat: scroll
 *   - renderStreaming(text)           paint streaming markdown + manage
 *                                     tool-spinner + streaming-indicator
 *   - feedTTS(text)                   forward to speech.feedStreamingText
 *
 *   - onCompleteHeader()              early bookkeeping (mark online,
 *                                     hide stop button, …)
 *   - dropEmptyComplete()             bool — true if we should bail on
 *                                     empty completion (floating)
 *   - onBeforeFinalRender()           remove streaming indicator etc.
 *   - waitForPendingChunks()          optional — sleep/yield so the last
 *                                     few chunk events flush
 *   - renderFinal(text)               final markdown render
 *   - onAfterFinalRender(text)        post-completion UI (response actions,
 *                                     suggestion chips, notifications, push
 *                                     to messages array, …)
 *
 *   - onError(event, online)          handle network/error UI
 *   - onSessionReset(event, msg)      session-reset UI
 *
 *   - flushPendingMarkdown()          called by the controller's public
 *                                     `flushStreamingRender()` method when
 *                                     the host needs the full streamed text
 *                                     painted right now (e.g. before a
 *                                     permission dialog opens over it)
 *   - showToolRunningSpinner(friendly) append the running-tool spinner
 *   - onToolCallTracked(update, updated)  after processToolCallUpdate
 */

import { processToolCallUpdate, getSessionResetMessage } from './streaming-utils.js';
import { getToolFriendlyName } from './tool-utils.js';
import { checkOnError } from './network.js';

/**
 * Extract the {text, sessionId} delta from a message_chunk event.
 * Handles both shapes: `{text, sessionId}` (current) and bare strings.
 */
export function extractChunkDelta(event) {
    const payload = event?.payload && typeof event.payload === 'object' ? event.payload : null;
    if (payload) return { text: payload.text || '', sessionId: payload.sessionId || null };
    return { text: String(event?.payload || ''), sessionId: null };
}

export class MessageStreamController {
    constructor(host) {
        this.host = host;
    }

    handleChunk(event) {
        const host = this.host;
        if (!host.isWaiting()) return;

        const delta = extractChunkDelta(event);
        if (!host.acceptSessionId(delta.sessionId)) return;

        if (delta.text) host.appendToAccumulator(delta.text);
        host.onChunkAppended?.(host.getAccumulator());

        if (host.automationPlanController.tryHandleChunk(host.getAccumulator()).handled) {
            return;
        }

        if (host.extensionToolController.maybeHandleChunk(host.getAccumulator()).handled) {
            host.bumpLayout?.();
            return;
        }

        if (host.extensionToolController.shouldSkipChunkRender()) return;

        host.renderStreaming(host.getAccumulator());
        host.feedTTS?.(host.getAccumulator());
    }

    async handleComplete() {
        const host = this.host;
        if (!host.isWaiting()) return;

        await host.onCompleteHeader?.();

        if (host.dropEmptyComplete?.()) return;

        // Wait for any trailing chunks to flush before checking controllers.
        // The last chunk (e.g. closing ``` fence) can arrive milliseconds
        // before message_complete and may not have been processed yet.
        await host.waitForPendingChunks?.();

        if (host.automationPlanController.started) return;

        if (host.extensionToolController.maybeHandleComplete(host.getAccumulator()).handled) {
            return;
        }

        host.onBeforeFinalRender?.();

        if (
            host.automationPlanController.tryHandleCompleteFallback(host.getAccumulator()).handled
        ) {
            return;
        }

        host.renderFinal(host.getAccumulator());
        await host.onAfterFinalRender?.(host.getAccumulator());
    }

    async handleError(event) {
        const online = await checkOnError();
        await this.host.onError(event, online);
    }

    handleSessionReset(event) {
        const msg = getSessionResetMessage(event.payload);
        this.host.onSessionReset(event, msg);
    }

    handleToolCallUpdate(event) {
        const host = this.host;
        if (!host.isWaiting()) return;

        const evtSessionId = event?.payload?.params?.sessionId;
        if (evtSessionId && !host.acceptSessionId(evtSessionId)) return;

        const { updated, update } = processToolCallUpdate(event, host);

        // Note: we deliberately do NOT call host.flushPendingMarkdown() here
        // any more. Tool-call updates fire on every kind/title change and
        // every progress event — paying for a full markdown re-render on
        // each one was wasteful (the throttled streaming render is already
        // painting at ~60 fps via the debounce path). The original
        // motivation was "flush before a permission dialog appears" — but
        // permission_request is a separate Tauri event, and each window's
        // permission handler now calls flushStreamingRender() directly
        // before showing the modal.

        if (update?.title) {
            host.showToolRunningSpinner?.(getToolFriendlyName(update.title));
        }

        host.onToolCallTracked?.(update, updated);
    }

    /**
     * Synchronously render the full accumulated text now, cancelling any
     * pending throttled streaming render. Called from the window's
     * permission_request handler so the user sees the complete streamed
     * text behind the permission dialog, not whatever partial state the
     * debounce timer happens to have produced.
     */
    flushStreamingRender() {
        this.host.flushPendingMarkdown?.();
    }
}
