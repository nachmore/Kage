import { describe, it, expect, vi, beforeEach } from 'vitest';
import { MessageStreamController, extractChunkDelta } from '../../ui/js/shared/message-stream-controller.js';

// network.js wires into addEventListener('online'/'offline') at import time;
// jsdom provides a no-op window so this just works.

function makeStubController(overrides = {}) {
    return {
        tryHandleChunk: vi.fn().mockReturnValue({ handled: false }),
        tryHandleCompleteFallback: vi.fn().mockReturnValue({ handled: false }),
        maybeHandleChunk: vi.fn().mockReturnValue({ handled: false }),
        maybeHandleComplete: vi.fn().mockReturnValue({ handled: false }),
        shouldSkipChunkRender: vi.fn().mockReturnValue(false),
        started: false,
        ...overrides,
    };
}

function makeHost(overrides = {}) {
    let acc = '';
    const automation = makeStubController({ tryHandleChunk: vi.fn().mockReturnValue({ handled: false }) });
    const ext = makeStubController();
    const host = {
        isWaiting: vi.fn().mockReturnValue(true),
        acceptSessionId: vi.fn().mockReturnValue(true),
        getAccumulator: () => acc,
        appendToAccumulator: (delta) => { acc += delta; },
        resetAccumulator: () => { acc = ''; },
        automationPlanController: automation,
        extensionToolController: ext,
        onChunkAppended: vi.fn(),
        bumpLayout: vi.fn(),
        renderStreaming: vi.fn(),
        feedTTS: vi.fn(),
        onCompleteHeader: vi.fn(),
        dropEmptyComplete: vi.fn().mockReturnValue(false),
        onBeforeFinalRender: vi.fn(),
        waitForPendingChunks: vi.fn(),
        renderFinal: vi.fn(),
        onAfterFinalRender: vi.fn(),
        onError: vi.fn(),
        onSessionReset: vi.fn(),
        flushPendingMarkdown: vi.fn(),
        showToolRunningSpinner: vi.fn(),
        onToolCallTracked: vi.fn(),
        toolUsages: [],
        toolSources: [],
        ...overrides,
    };
    return host;
}

describe('extractChunkDelta', () => {
    it('reads {text, sessionId} from object payload', () => {
        const r = extractChunkDelta({ payload: { text: 'hi', sessionId: 'abc' } });
        expect(r).toEqual({ text: 'hi', sessionId: 'abc' });
    });

    it('falls back to bare string payload', () => {
        const r = extractChunkDelta({ payload: 'hi' });
        expect(r.text).toBe('hi');
        expect(r.sessionId).toBe(null);
    });

    it('handles null payload', () => {
        const r = extractChunkDelta({ payload: null });
        expect(r.text).toBe('');
    });
});

describe('MessageStreamController.handleChunk', () => {
    it('bails when host is not waiting', () => {
        const host = makeHost({ isWaiting: vi.fn().mockReturnValue(false) });
        const c = new MessageStreamController(host);
        c.handleChunk({ payload: { text: 'hi' } });
        expect(host.renderStreaming).not.toHaveBeenCalled();
    });

    it('drops chunks the host rejects on session id', () => {
        const host = makeHost({ acceptSessionId: vi.fn().mockReturnValue(false) });
        const c = new MessageStreamController(host);
        c.handleChunk({ payload: { text: 'hi', sessionId: 'wrong' } });
        expect(host.renderStreaming).not.toHaveBeenCalled();
        expect(host.getAccumulator()).toBe('');
    });

    it('appends delta and renders the streaming text', () => {
        const host = makeHost();
        const c = new MessageStreamController(host);
        c.handleChunk({ payload: { text: 'hi ' } });
        c.handleChunk({ payload: { text: 'there' } });
        expect(host.getAccumulator()).toBe('hi there');
        expect(host.renderStreaming).toHaveBeenLastCalledWith('hi there');
        expect(host.feedTTS).toHaveBeenLastCalledWith('hi there');
    });

    it('delegates to automation controller and bails when handled', () => {
        const host = makeHost();
        host.automationPlanController.tryHandleChunk.mockReturnValue({ handled: true });
        const c = new MessageStreamController(host);
        c.handleChunk({ payload: { text: 'plan stuff' } });
        expect(host.renderStreaming).not.toHaveBeenCalled();
        expect(host.feedTTS).not.toHaveBeenCalled();
    });

    it('delegates to extension controller and bails (with bumpLayout) when handled', () => {
        const host = makeHost();
        host.extensionToolController.maybeHandleChunk.mockReturnValue({ handled: true });
        const c = new MessageStreamController(host);
        c.handleChunk({ payload: { text: 'ext stuff' } });
        expect(host.renderStreaming).not.toHaveBeenCalled();
        expect(host.bumpLayout).toHaveBeenCalledTimes(1);
    });

    it('skips render while extension tool is executing', () => {
        const host = makeHost();
        host.extensionToolController.shouldSkipChunkRender.mockReturnValue(true);
        const c = new MessageStreamController(host);
        c.handleChunk({ payload: { text: 'mid-tool delta' } });
        expect(host.getAccumulator()).toBe('mid-tool delta');
        expect(host.renderStreaming).not.toHaveBeenCalled();
    });
});

describe('MessageStreamController.handleComplete', () => {
    it('bails when not waiting', async () => {
        const host = makeHost({ isWaiting: vi.fn().mockReturnValue(false) });
        const c = new MessageStreamController(host);
        await c.handleComplete();
        expect(host.onCompleteHeader).not.toHaveBeenCalled();
    });

    it('calls header → drop → started-skip → ext-handle in order', async () => {
        const host = makeHost();
        host.automationPlanController.started = true;
        const c = new MessageStreamController(host);
        await c.handleComplete();
        expect(host.onCompleteHeader).toHaveBeenCalled();
        expect(host.dropEmptyComplete).toHaveBeenCalled();
        // Started → bail before ext check
        expect(host.extensionToolController.maybeHandleComplete).not.toHaveBeenCalled();
        expect(host.renderFinal).not.toHaveBeenCalled();
    });

    it('drops empty completes when host says to', async () => {
        const host = makeHost({ dropEmptyComplete: vi.fn().mockReturnValue(true) });
        const c = new MessageStreamController(host);
        await c.handleComplete();
        expect(host.onCompleteHeader).toHaveBeenCalled();
        expect(host.renderFinal).not.toHaveBeenCalled();
    });

    it('extension fence handled at completion bails before final render', async () => {
        const host = makeHost();
        host.extensionToolController.maybeHandleComplete.mockReturnValue({ handled: true });
        const c = new MessageStreamController(host);
        await c.handleComplete();
        expect(host.renderFinal).not.toHaveBeenCalled();
        expect(host.onBeforeFinalRender).not.toHaveBeenCalled();
    });

    it('automation fallback handled bails after onBeforeFinalRender', async () => {
        const host = makeHost();
        host.automationPlanController.tryHandleCompleteFallback.mockReturnValue({ handled: true });
        const c = new MessageStreamController(host);
        await c.handleComplete();
        expect(host.onBeforeFinalRender).toHaveBeenCalled();
        expect(host.renderFinal).not.toHaveBeenCalled();
    });

    it('happy path runs the full pipeline in order', async () => {
        const host = makeHost();
        host.appendToAccumulator('the answer is 42');
        const calls = [];
        host.onCompleteHeader.mockImplementation(() => calls.push('header'));
        host.onBeforeFinalRender.mockImplementation(() => calls.push('beforeFinal'));
        host.waitForPendingChunks.mockImplementation(() => calls.push('wait'));
        host.renderFinal.mockImplementation(() => calls.push('final'));
        host.onAfterFinalRender.mockImplementation(() => calls.push('after'));

        const c = new MessageStreamController(host);
        await c.handleComplete();
        expect(calls).toEqual(['header', 'wait', 'beforeFinal', 'final', 'after']);
        expect(host.renderFinal).toHaveBeenCalledWith('the answer is 42');
        expect(host.onAfterFinalRender).toHaveBeenCalledWith('the answer is 42');
    });
});

describe('MessageStreamController.handleSessionReset', () => {
    it('extracts message and forwards to host', () => {
        const host = makeHost();
        const c = new MessageStreamController(host);
        c.handleSessionReset({ payload: { reason: 'image_unsupported', reconnected: true } });
        expect(host.onSessionReset).toHaveBeenCalled();
        const [event, msg] = host.onSessionReset.mock.calls[0];
        expect(msg).toMatch(/image/);
        expect(event.payload.reason).toBe('image_unsupported');
    });
});

describe('MessageStreamController.handleToolCallUpdate', () => {
    it('drops when not waiting', () => {
        const host = makeHost({ isWaiting: vi.fn().mockReturnValue(false) });
        const c = new MessageStreamController(host);
        c.handleToolCallUpdate({ payload: { params: { update: { title: 'read', toolCallId: 'tc1' } } } });
        expect(host.flushPendingMarkdown).not.toHaveBeenCalled();
        expect(host.showToolRunningSpinner).not.toHaveBeenCalled();
    });

    it('drops when session id mismatches', () => {
        const host = makeHost({ acceptSessionId: vi.fn().mockReturnValue(false) });
        const c = new MessageStreamController(host);
        c.handleToolCallUpdate({
            payload: { params: { sessionId: 'other', update: { title: 'read', toolCallId: 'tc1' } } }
        });
        expect(host.flushPendingMarkdown).not.toHaveBeenCalled();
    });

    it('shows the running spinner without forcing a full markdown re-render', () => {
        // Tool-call updates fire on every kind/title change. Paying for a
        // full re-render here was wasteful — the throttled streaming path
        // already paints at ~60 fps. The flush is only triggered now from
        // permission_request via flushStreamingRender().
        const host = makeHost();
        const c = new MessageStreamController(host);
        c.handleToolCallUpdate({
            payload: { params: { update: { title: 'read_file', toolCallId: 'tc1', kind: 'read' } } }
        });
        expect(host.flushPendingMarkdown).not.toHaveBeenCalled();
        expect(host.showToolRunningSpinner).toHaveBeenCalled();
        expect(host.onToolCallTracked).toHaveBeenCalled();
    });

    it('skips the spinner when there is no title (and still no flush)', () => {
        const host = makeHost();
        const c = new MessageStreamController(host);
        c.handleToolCallUpdate({ payload: { params: { update: {} } } });
        expect(host.flushPendingMarkdown).not.toHaveBeenCalled();
        expect(host.showToolRunningSpinner).not.toHaveBeenCalled();
    });
});

describe('MessageStreamController.flushStreamingRender', () => {
    it('delegates to the host adapter so the window can paint immediately', () => {
        const host = makeHost();
        const c = new MessageStreamController(host);
        c.flushStreamingRender();
        expect(host.flushPendingMarkdown).toHaveBeenCalledTimes(1);
    });
});
