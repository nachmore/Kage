/**
 * Extension Tool Controller — shared between FloatingApp and ChatApp.
 *
 * Owns the streaming-fence detection FSM (handled / executing flags), the
 * permission check, the tool execution, and the result-emit. Window-specific
 * surface (rendering, "show progress" UI, post-execution thinking-state) is
 * delegated through the `host` adapter so this module touches no DOM directly
 * and has no hidden coupling to either window.
 *
 * Lifecycle the host drives:
 *   - `controller.startSteering()` once extensions are loaded and the window
 *     has a session — sends the tool steering and re-sends it whenever the
 *     extension tools change (`stopSteering()` on teardown)
 *   - `controller.reset()` when starting a new prompt
 *   - `controller.maybeHandleChunk(text)` from each chunk handler — returns
 *     `{ handled: true }` if the controller has taken over rendering/execution
 *   - `controller.maybeHandleComplete(text)` from each completion handler —
 *     same signal
 *   - `controller.shouldSkipChunkRender()` — true while a tool is mid-execution
 *
 * Host adapter shape (everything optional unless noted):
 *   - invoke(cmd, args)  REQUIRED
 *   - extensionManager   REQUIRED
 *   - permissionModal: { showForExtensionTool(extension, tool, icon) -> Promise<bool> }  REQUIRED
 *   - addToolUsage({ toolCallId, title, kind })   tracks chip in window's tool list
 *   - renderIndicator(info)   show "running …" placeholder (window-specific DOM)
 *   - onExecuteStart()        called once permission granted, before tool runs
 *   - onExecuteEnd()          called after response sent, before waiting flag flips
 *   - onWaitForFollowup()     called after onExecuteEnd to show typing/thinking UI
 *   - resetAccumulator()      window resets its current-response buffer (the old
 *                             fence must not be re-detected by next chunk)
 */

import { getExtensionToolFriendlyName } from './tool-utils.js';
import { detectExtensionToolCall, detectExtensionToolCallIncremental } from './streaming-utils.js';

/**
 * Quiet period after a tool-definitions change before the steering is
 * re-sent. One config save can surface as two changes (config update, then
 * extension reload), and a setting flicked on and off again settles back to
 * what the agent already has — neither should cost an agent turn.
 */
export const STEERING_REFRESH_DEBOUNCE_MS = 500;

export class ExtensionToolController {
    constructor(host) {
        this.host = host;
        this.handled = false;
        this.executing = false;
        /**
         * Block this window last published: `null` before its first one, ''
         * for "no tools". The first publication goes out even when empty, so
         * the backend steers this window's sessions with "no tools" rather
         * than with another window's list.
         */
        this._steeringSent = null;
        this._steeringRun = null;
        this._steeringPending = false;
        /** A queued rerun must fetch fresh definitions (not just re-render). */
        this._steeringPendingFetch = false;
        this._steeringTimer = null;
        this._unwatchTools = null;
    }

    reset() {
        this.handled = false;
        this.executing = false;
    }

    shouldSkipChunkRender() {
        return this.executing;
    }

    /** Returns true when the controller has taken over the chunk (host should bail). */
    maybeHandleChunk(text) {
        if (!this.handled) {
            const toolCall = detectExtensionToolCall(text);
            if (toolCall) {
                this.handled = true;
                this._handleToolCall(toolCall);
                return { handled: true };
            }
            const partial = detectExtensionToolCallIncremental(text);
            if (partial) {
                this._renderIndicator(partial);
                return { handled: true };
            }
        } else if (!this.executing) {
            // Tool call handled and execution finished. If the new accumulated
            // text no longer contains the fence, the host has reset the
            // accumulator for the follow-up response — clear handled so future
            // fences are detected normally.
            if (!text.includes('```extension_tool_call')) {
                this.handled = false;
            }
        }
        return { handled: false };
    }

    /** Returns true when the controller has taken over completion. */
    maybeHandleComplete(text) {
        if (this.executing || this.handled) return { handled: true };
        const toolCall = detectExtensionToolCall(text);
        if (toolCall) {
            this.handled = true;
            this._handleToolCall(toolCall);
            return { handled: true };
        }
        return { handled: false };
    }

    /**
     * Send the extension tool steering for the host's session, then keep the
     * agent in sync: whenever the extension manager reports different tool
     * definitions, re-send (debounced). Safe to call more than once.
     */
    startSteering() {
        if (!this._unwatchTools) {
            const em = this.host.extensionManager;
            if (typeof em?.onToolDefinitionsChanged === 'function') {
                this._unwatchTools = em.onToolDefinitionsChanged(() =>
                    this.scheduleSteeringRefresh()
                );
            }
        }
        return this.sendSteering();
    }

    /** Stop following tool-definition changes (window teardown). */
    stopSteering() {
        clearTimeout(this._steeringTimer);
        this._steeringTimer = null;
        this._unwatchTools?.();
        this._unwatchTools = null;
    }

    /**
     * Re-send the steering once the tool definitions settle. Renders the
     * definitions the manager just reported (its cache) instead of fetching
     * again: a fetch notifies listeners itself, so an extension whose
     * getTools() answers differently on every call would otherwise keep
     * this loop, and an agent turn per lap, going forever.
     */
    scheduleSteeringRefresh() {
        clearTimeout(this._steeringTimer);
        this._steeringTimer = setTimeout(() => {
            this._steeringTimer = null;
            this.sendSteering({ fetch: false });
        }, STEERING_REFRESH_DEBOUNCE_MS);
    }

    /**
     * Build the steering block from the extension manager and ship it to the
     * agent if it differs from what this window last sent. An empty block is
     * sent too (the first one and any after tools): the backend records that
     * this window has no tools, and turns it into a "tools withdrawn" notice
     * for sessions that were sent tools.
     *
     * Single-flight with one trailing rerun, so this window's claims reach
     * the backend in order (the backend then delivers per session in claim
     * order and drops superseded blocks).
     *
     * @param {{ fetch?: boolean }} [opts] - `fetch: false` renders the
     *   manager's cached definitions rather than asking every extension.
     */
    sendSteering({ fetch = true } = {}) {
        if (this._steeringRun) {
            this._steeringPending = true;
            this._steeringPendingFetch ||= fetch;
            return this._steeringRun;
        }
        this._steeringRun = (async () => {
            try {
                let fetchNow = fetch;
                do {
                    this._steeringPending = false;
                    await this._sendSteeringOnce(fetchNow);
                    fetchNow = this._steeringPendingFetch;
                    this._steeringPendingFetch = false;
                } while (this._steeringPending);
            } finally {
                this._steeringRun = null;
            }
        })();
        return this._steeringRun;
    }

    async _sendSteeringOnce(fetch) {
        const em = this.host.extensionManager;
        let block;
        try {
            const cached =
                !fetch && typeof em.getToolDefinitionsCached === 'function'
                    ? em.getToolDefinitionsCached()
                    : undefined;
            block = (await em.buildToolSteeringBlock(cached)) || '';
        } catch (e) {
            console.warn('Failed to build extension tool steering:', e);
            return;
        }
        // Unchanged since this window's last send. The backend replays the
        // latest block to the window's other sessions before their next
        // prompt, so there's nothing to do on a session switch either.
        if (block === this._steeringSent) return;
        // No session right now (none adopted yet, the active chat deleted):
        // still publish, so the backend replays this window's current block
        // to whichever session it prompts next.
        const sessionId = this.host.getSessionId?.() || null;
        try {
            await this.host.invoke('send_extension_tool_steering', {
                sessionId,
                toolSteering: block,
            });
            this._steeringSent = block;
        } catch (e) {
            console.warn('Failed to send extension tool steering:', e);
        }
    }

    getExtensionIcon(extensionId) {
        const em = this.host.extensionManager;
        if (!extensionId || !em) return '🧩';
        const defs = em.getToolDefinitionsCached?.() || [];
        const def = defs.find((d) => d.extensionId === extensionId);
        return def?.extensionIcon || '🧩';
    }

    getExtensionToolFriendlyName(extension, tool) {
        return getExtensionToolFriendlyName(extension, tool, this.host.extensionManager);
    }

    _renderIndicator(info) {
        if (info.extension && info.tool) {
            const toolTitle = `ext:${info.extension}/${info.tool}`;
            const toolCallId = `ext-${info.extension}-${info.tool}`;
            this.host.addToolUsage?.({ toolCallId, title: toolTitle, kind: 'extension' });
        }
        this.host.renderIndicator?.(info);
    }

    async _handleToolCall(toolCall) {
        const { extension, tool, params } = toolCall;
        const icon = this.getExtensionIcon(extension);
        const toolTitle = `ext:${extension}/${tool}`;

        console.log(`Extension tool call: ${extension}/${tool}`, params);

        const extToolCallId = `ext-${extension}-${tool}`;
        this.host.addToolUsage?.({
            toolCallId: extToolCallId,
            title: toolTitle,
            kind: 'extension',
        });

        const policy = await this._resolvePolicy(extension, tool);

        if (policy === 'deny') {
            this.executing = false;
            this.handled = false;
            await this._sendResponse(extension, tool, 'Permission denied by user policy', false);
            return;
        }

        if (policy === 'ask') {
            const allowed = await this.host.permissionModal.showForExtensionTool(
                extension,
                tool,
                icon
            );
            if (!allowed) {
                this.executing = false;
                this.handled = false;
                await this._sendResponse(extension, tool, 'Permission denied by user', false);
                return;
            }
        }

        this.executing = true;
        this.host.onExecuteStart?.();

        const result = await this.host.extensionManager.executeExtensionTool(
            extension,
            tool,
            params
        );
        const success = !result.error;
        const resultJson = JSON.stringify(success ? result.result : result.error);

        try {
            await this.host.invoke('extension_tool_response', {
                sessionId: this.host.getSessionId?.() || null,
                extensionId: extension,
                toolName: tool,
                resultJson,
                success,
            });
        } catch (e) {
            console.error('Failed to send extension tool response:', e);
        }

        this.executing = false;
        this.host.onExecuteEnd?.();

        // Reset accumulator so the (now-stale) fence isn't re-detected by the
        // first chunk of the agent's follow-up response.
        this.host.resetAccumulator?.();
        this.handled = false;

        this.host.onWaitForFollowup?.();
    }

    async _resolvePolicy(extension, tool) {
        let policy;
        try {
            policy = await this.host.invoke('check_extension_tool_permission', {
                extensionId: extension,
                toolName: tool,
            });
        } catch (e) {
            console.error('Failed to check extension tool permission:', e);
            return 'ask';
        }
        // The user's stored policy is authoritative. `hasBuiltInConfirmation`
        // comes from untrusted sandbox code, so it may only skip the host
        // prompt (the extension confirms itself) — never override a deny.
        if (policy !== 'ask') return policy;
        let toolDefs;
        try {
            toolDefs = await this.host.extensionManager.getToolDefinitions();
        } catch (e) {
            console.warn('Failed to refresh extension tool definitions:', e);
            toolDefs = this.host.extensionManager.getToolDefinitionsCached?.() || [];
        }
        const extDef = toolDefs.find((d) => d.extensionId === extension);
        const toolDef = extDef?.tools?.find((t) => t.name === tool);
        return toolDef?.hasBuiltInConfirmation === true ? 'allow' : policy;
    }

    async _sendResponse(extension, tool, message, success) {
        try {
            await this.host.invoke('extension_tool_response', {
                sessionId: this.host.getSessionId?.() || null,
                extensionId: extension,
                toolName: tool,
                resultJson: JSON.stringify(message),
                success,
            });
        } catch (e) {
            console.error('Failed to send extension tool response:', e);
        }
    }
}
