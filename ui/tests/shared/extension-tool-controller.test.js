import { describe, it, expect, vi, beforeEach } from 'vitest';
import { ExtensionToolController } from '../../js/shared/extension-tool-controller.js';

function makeHost(overrides = {}) {
    return {
        invoke: vi.fn().mockResolvedValue('allow'),
        extensionManager: {
            buildToolSteeringBlock: vi.fn().mockResolvedValue('STEER'),
            getToolDefinitionsCached: vi.fn().mockReturnValue([]),
            getToolDefinitions: vi.fn().mockResolvedValue([]),
            executeExtensionTool: vi.fn().mockResolvedValue({ result: 'ok' }),
        },
        permissionModal: {
            showForExtensionTool: vi.fn().mockResolvedValue(true),
        },
        addToolUsage: vi.fn(),
        renderIndicator: vi.fn(),
        onExecuteStart: vi.fn(),
        onExecuteEnd: vi.fn(),
        onWaitForFollowup: vi.fn(),
        resetAccumulator: vi.fn(),
        ...overrides,
    };
}

const FENCE_OPEN = '```extension_tool_call\n{"extension":"e","tool":"t","params":{"x":1}}\n```';

describe('ExtensionToolController.maybeHandleChunk', () => {
    it('detects an incremental fence and renders the indicator', () => {
        const host = makeHost();
        const c = new ExtensionToolController(host);
        const partial = '```extension_tool_call\n{"extension":"weather","tool":"get_temp"';
        const r = c.maybeHandleChunk(partial);
        expect(r.handled).toBe(true);
        expect(host.renderIndicator).toHaveBeenCalledTimes(1);
        expect(c.handled).toBe(false);  // partial fence — not committed yet
    });

    it('detects a complete fence, marks handled, and starts executing', async () => {
        const host = makeHost();
        const c = new ExtensionToolController(host);
        const r = c.maybeHandleChunk(FENCE_OPEN);
        expect(r.handled).toBe(true);
        expect(c.handled).toBe(true);
        // _handleToolCall was scheduled — flush microtasks
        await new Promise(r => setTimeout(r, 0));
        expect(host.invoke).toHaveBeenCalledWith('check_extension_tool_permission', expect.any(Object));
    });

    it('returns unhandled for chunks without a fence', () => {
        const host = makeHost();
        const c = new ExtensionToolController(host);
        const r = c.maybeHandleChunk('Just some streaming text');
        expect(r.handled).toBe(false);
        expect(host.renderIndicator).not.toHaveBeenCalled();
    });

    it('clears handled flag once the fence disappears post-execution', () => {
        const host = makeHost();
        const c = new ExtensionToolController(host);
        c.handled = true;
        c.executing = false;
        const r = c.maybeHandleChunk('Follow-up response, no fence');
        expect(r.handled).toBe(false);
        expect(c.handled).toBe(false);
    });

    it('keeps handled true while text still contains a fence and tool not running', () => {
        const host = makeHost();
        const c = new ExtensionToolController(host);
        c.handled = true;
        c.executing = false;
        const r = c.maybeHandleChunk('still has ```extension_tool_call leftover');
        expect(r.handled).toBe(false);
        expect(c.handled).toBe(true);
    });
});

describe('ExtensionToolController.maybeHandleComplete', () => {
    it('signals handled while executing, without firing again', () => {
        const host = makeHost();
        const c = new ExtensionToolController(host);
        c.executing = true;
        const r = c.maybeHandleComplete('any text');
        expect(r.handled).toBe(true);
        expect(host.invoke).not.toHaveBeenCalled();
    });

    it('detects fence as a fallback when streaming missed it', async () => {
        const host = makeHost();
        const c = new ExtensionToolController(host);
        const r = c.maybeHandleComplete(FENCE_OPEN);
        expect(r.handled).toBe(true);
        await new Promise(r => setTimeout(r, 0));
        expect(host.invoke).toHaveBeenCalledWith('check_extension_tool_permission', expect.any(Object));
    });

    it('returns unhandled when text has no fence and no execution in flight', () => {
        const host = makeHost();
        const c = new ExtensionToolController(host);
        const r = c.maybeHandleComplete('Just some text');
        expect(r.handled).toBe(false);
    });
});

describe('ExtensionToolController._handleToolCall', () => {
    it('happy path: allow policy → execute → send response → wait for followup', async () => {
        const host = makeHost({
            invoke: vi.fn(async (cmd) => {
                if (cmd === 'check_extension_tool_permission') return 'allow';
                return null;
            }),
        });
        const c = new ExtensionToolController(host);
        await c._handleToolCall({ extension: 'e', tool: 't', params: {} });
        expect(host.onExecuteStart).toHaveBeenCalled();
        expect(host.extensionManager.executeExtensionTool).toHaveBeenCalledWith('e', 't', {});
        expect(host.invoke).toHaveBeenCalledWith('extension_tool_response', expect.objectContaining({
            extensionId: 'e',
            toolName: 't',
            success: true,
        }));
        expect(host.resetAccumulator).toHaveBeenCalled();
        expect(host.onWaitForFollowup).toHaveBeenCalled();
        expect(c.executing).toBe(false);
        expect(c.handled).toBe(false);
    });

    it('deny policy: skips execute and sends a denial response', async () => {
        const host = makeHost({
            invoke: vi.fn(async (cmd) => cmd === 'check_extension_tool_permission' ? 'deny' : null),
        });
        const c = new ExtensionToolController(host);
        await c._handleToolCall({ extension: 'e', tool: 't', params: {} });
        expect(host.extensionManager.executeExtensionTool).not.toHaveBeenCalled();
        expect(host.invoke).toHaveBeenCalledWith('extension_tool_response', expect.objectContaining({
            success: false,
            resultJson: JSON.stringify('Permission denied by user policy'),
        }));
        expect(c.handled).toBe(false);
        expect(c.executing).toBe(false);
    });

    it('ask policy + user denies modal: sends a denial response without executing', async () => {
        const host = makeHost({
            invoke: vi.fn(async (cmd) => cmd === 'check_extension_tool_permission' ? 'ask' : null),
            permissionModal: { showForExtensionTool: vi.fn().mockResolvedValue(false) },
        });
        const c = new ExtensionToolController(host);
        await c._handleToolCall({ extension: 'e', tool: 't', params: {} });
        expect(host.extensionManager.executeExtensionTool).not.toHaveBeenCalled();
        expect(host.invoke).toHaveBeenCalledWith('extension_tool_response', expect.objectContaining({
            success: false,
            resultJson: JSON.stringify('Permission denied by user'),
        }));
    });

    it('hasBuiltInConfirmation: bypasses permission check', async () => {
        const host = makeHost({
            extensionManager: {
                buildToolSteeringBlock: vi.fn(),
                getToolDefinitionsCached: vi.fn().mockReturnValue([]),
                getToolDefinitions: vi.fn().mockResolvedValue([
                    { extensionId: 'e', tools: [{ name: 't', hasBuiltInConfirmation: true }] }
                ]),
                executeExtensionTool: vi.fn().mockResolvedValue({ result: 'ok' }),
            },
        });
        const c = new ExtensionToolController(host);
        await c._handleToolCall({ extension: 'e', tool: 't', params: {} });
        expect(host.invoke).not.toHaveBeenCalledWith('check_extension_tool_permission', expect.any(Object));
        expect(host.extensionManager.executeExtensionTool).toHaveBeenCalled();
    });

    it('relays tool error: success=false with the error payload', async () => {
        const host = makeHost({
            invoke: vi.fn(async (cmd) => cmd === 'check_extension_tool_permission' ? 'allow' : null),
            extensionManager: {
                buildToolSteeringBlock: vi.fn(),
                getToolDefinitionsCached: vi.fn().mockReturnValue([]),
                getToolDefinitions: vi.fn().mockResolvedValue([]),
                executeExtensionTool: vi.fn().mockResolvedValue({ error: 'boom' }),
            },
        });
        const c = new ExtensionToolController(host);
        await c._handleToolCall({ extension: 'e', tool: 't', params: {} });
        expect(host.invoke).toHaveBeenCalledWith('extension_tool_response', expect.objectContaining({
            success: false,
            resultJson: JSON.stringify('boom'),
        }));
    });
});

describe('ExtensionToolController.sendSteering', () => {
    it('ships the block to the agent', async () => {
        const host = makeHost();
        const c = new ExtensionToolController(host);
        await c.sendSteering();
        expect(host.invoke).toHaveBeenCalledWith('send_extension_tool_steering', { toolSteering: 'STEER' });
    });

    it('skips the IPC when buildToolSteeringBlock returns falsy', async () => {
        const host = makeHost();
        host.extensionManager.buildToolSteeringBlock = vi.fn().mockResolvedValue(null);
        const c = new ExtensionToolController(host);
        await c.sendSteering();
        expect(host.invoke).not.toHaveBeenCalled();
    });
});

describe('ExtensionToolController.getExtensionIcon', () => {
    it('returns the extension icon when known', () => {
        const host = makeHost();
        host.extensionManager.getToolDefinitionsCached = vi.fn().mockReturnValue([
            { extensionId: 'weather', extensionIcon: '🌦️' }
        ]);
        const c = new ExtensionToolController(host);
        expect(c.getExtensionIcon('weather')).toBe('🌦️');
    });

    it('falls back to default for unknown extension', () => {
        const host = makeHost();
        const c = new ExtensionToolController(host);
        expect(c.getExtensionIcon('unknown')).toBe('🧩');
    });
});
