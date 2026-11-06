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
 *   - `controller.startSteering()` once extensions are loaded — publishes the
 *     tool steering to the backend and re-publishes it whenever the extension
 *     tools change (`stopSteering()` on teardown). The backend attaches it to
 *     the window's next user prompt on a session that doesn't have it yet.
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
 * re-published. One config save can surface as two changes (config update,
 * then extension reload), and a setting flicked on and off again settles back
 * to what was published — neither should publish twice.
 */
export const STEERING_REFRESH_DEBOUNCE_MS = 500;

export class ExtensionToolController {
    constructor(host) {
        this.host = host;
        this.handled = false;
        this.executing = false;
        /**
         * The current steering lifetime, from startSteering() to
         * stopSteering(); `null` outside one. Work started in a lifetime
         * checks it's still current after every await, so nothing is
         * published after stopSteering().
         * @type {null | {
         *   sent: string | null,
         *   run: Promise<void> | null,
         *   pending: boolean,
         *   pendingFetch: boolean,
         *   timer: ReturnType<typeof setTimeout> | null,
         *   unwatch: (() => void) | null,
         * }}
         */
        this._steering = null;
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
     * Publish this window's extension tool steering, then keep it current:
     * whenever the extension manager reports different tool definitions,
     * publish again (debounced). Safe to call more than once; after
     * stopSteering() it starts a new lifetime, which publishes afresh.
     */
    startSteering() {
        if (!this._steering) {
            const life = {
                sent: null,
                run: null,
                pending: false,
                pendingFetch: false,
                timer: null,
                unwatch: null,
            };
            this._steering = life;
            const em = this.host.extensionManager;
            if (typeof em?.onToolDefinitionsChanged === 'function') {
                life.unwatch = em.onToolDefinitionsChanged(() => this.scheduleSteeringRefresh());
            }
        }
        return this.sendSteering();
    }

    /**
     * Stop following tool-definition changes (window teardown). Work still in
     * flight stops at its next await; nothing is published after this.
     */
    stopSteering() {
        const life = this._steering;
        if (!life) return;
        this._steering = null;
        clearTimeout(life.timer);
        life.unwatch?.();
    }

    /**
     * Publish again once the tool definitions settle. Renders the definitions
     * the manager just reported (its cache) instead of fetching again: a
     * fetch notifies listeners itself, so an extension whose getTools()
     * answers differently on every call would otherwise keep this loop going
     * forever.
     */
    scheduleSteeringRefresh() {
        const life = this._steering;
        if (!life) return;
        clearTimeout(life.timer);
        life.timer = setTimeout(() => {
            life.timer = null;
            if (life === this._steering) this.sendSteering({ fetch: false });
        }, STEERING_REFRESH_DEBOUNCE_MS);
    }

    /**
     * Build the steering block from the extension manager and publish it to
     * the backend if it differs from what this window last published. An
     * empty block is published too (the first one, and any after tools): the
     * backend records that this window has no tools, and turns it into a
     * "tools withdrawn" notice for sessions that were sent tools. Does nothing
     * outside a startSteering() lifetime.
     *
     * Single-flight with one trailing rerun, so this window's publications
     * reach the backend in order and the last one wins.
     *
     * @param {{ fetch?: boolean }} [opts] - `fetch: false` renders the
     *   manager's cached definitions rather than asking every extension.
     */
    sendSteering({ fetch = true } = {}) {
        const life = this._steering;
        if (!life) return Promise.resolve();
        if (life.run) {
            life.pending = true;
            life.pendingFetch ||= fetch;
            return life.run;
        }
        life.run = (async () => {
            try {
                let fetchNow = fetch;
                do {
                    life.pending = false;
                    await this._sendSteeringOnce(life, fetchNow);
                    fetchNow = life.pendingFetch;
                    life.pendingFetch = false;
                } while (life.pending && life === this._steering);
            } finally {
                life.run = null;
            }
        })();
        return life.run;
    }

    async _sendSteeringOnce(life, fetch) {
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
        if (life !== this._steering) return; // stopped meanwhile
        // Unchanged since this window's last publication. The backend sends
        // it with the next prompt on any session that doesn't have it, so
        // there's nothing to do on a session switch either.
        if (block === life.sent) return;
        try {
            await this.host.invoke('send_extension_tool_steering', { toolSteering: block });
            if (life !== this._steering) return;
            life.sent = block;
        } catch (e) {
            console.warn('Failed to publish extension tool steering:', e);
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
