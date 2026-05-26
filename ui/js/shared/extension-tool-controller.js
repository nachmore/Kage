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

export class ExtensionToolController {
    constructor(host) {
        this.host = host;
        this.handled = false;
        this.executing = false;
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

    /** Build the steering block from the extension manager and ship it to the agent. */
    async sendSteering() {
        const block = await this.host.extensionManager.buildToolSteeringBlock();
        if (!block) return;
        const sessionId = this.host.getSessionId?.();
        if (!sessionId) {
            console.warn('Skipping extension tool steering — no session id from host');
            return;
        }
        try {
            await this.host.invoke('send_extension_tool_steering', {
                sessionId,
                toolSteering: block,
            });
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
        const toolDefs = await this.host.extensionManager.getToolDefinitions();
        const extDef = toolDefs.find((d) => d.extensionId === extension);
        const toolDef = extDef?.tools?.find((t) => t.name === tool);
        if (toolDef?.hasBuiltInConfirmation === true) return 'allow';
        try {
            return await this.host.invoke('check_extension_tool_permission', {
                extensionId: extension,
                toolName: tool,
            });
        } catch (e) {
            console.error('Failed to check extension tool permission:', e);
            return 'ask';
        }
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
