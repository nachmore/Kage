import { describe, it, expect, vi, beforeEach } from 'vitest';
import { AutomationPlanController } from '../../ui/js/shared/automation-plan-controller.js';

function makeHost(overrides = {}) {
    const listeners = {};
    const unlistens = [];
    const host = {
        invoke: vi.fn().mockResolvedValue(null),
        listen: vi.fn(async (event, handler) => {
            listeners[event] = handler;
            const unlisten = vi.fn();
            unlistens.push(unlisten);
            return unlisten;
        }),
        renderTasks: vi.fn(),
        appendReviewActions: vi.fn(),
        onPlanReadyForReview: vi.fn(),
        onPlanExecutionStart: vi.fn(),
        onPlanComplete: vi.fn(),
        onPlanFailed: vi.fn(),
        onRunClicked: vi.fn(),
        ...overrides,
    };
    host._listeners = listeners;
    host._unlistens = unlistens;
    return host;
}

const COMPLETE_PLAN_FENCE = '```automation_plan\n[{"step":1,"task":"do thing","details":"d"}]\n```';
// Has one complete step object but no closing fence — what the agent ships mid-stream.
const INCOMPLETE_PLAN_FENCE_PARTIAL_STEP = '```automation_plan\n[{"step":1,"task":"do thing","details":"d"},';

describe('AutomationPlanController.tryHandleChunk', () => {
    it('renders a partial plan as tasks while it streams', () => {
        const host = makeHost();
        const c = new AutomationPlanController(host);
        const r = c.tryHandleChunk(INCOMPLETE_PLAN_FENCE_PARTIAL_STEP);
        expect(r.handled).toBe(true);
        expect(c.started).toBe(false); // still partial — review hasn't fired
        expect(host.renderTasks).toHaveBeenCalled();
    });

    it('shows a complete plan for review and marks started', () => {
        const host = makeHost();
        const c = new AutomationPlanController(host);
        const r = c.tryHandleChunk(COMPLETE_PLAN_FENCE);
        expect(r.handled).toBe(true);
        expect(c.started).toBe(true);
        expect(host.renderTasks).toHaveBeenCalled();
        expect(host.appendReviewActions).toHaveBeenCalledTimes(1);
        expect(host.onPlanReadyForReview).toHaveBeenCalled();
        expect(c.pendingRevision).toBeTruthy();
    });

    it('returns unhandled for chunks without a plan', () => {
        const host = makeHost();
        const c = new AutomationPlanController(host);
        const r = c.tryHandleChunk('Just some streaming text');
        expect(r.handled).toBe(false);
        expect(host.renderTasks).not.toHaveBeenCalled();
    });

    it('once started, swallows further chunks without re-rendering for review', () => {
        const host = makeHost();
        const c = new AutomationPlanController(host);
        c.tryHandleChunk(COMPLETE_PLAN_FENCE);
        host.appendReviewActions.mockClear();
        host.onPlanReadyForReview.mockClear();

        const r = c.tryHandleChunk('more text follows');
        expect(r.handled).toBe(true);
        expect(host.appendReviewActions).not.toHaveBeenCalled();
        expect(host.onPlanReadyForReview).not.toHaveBeenCalled();
    });
});

describe('AutomationPlanController.tryHandleCompleteFallback', () => {
    it('catches a plan that streaming missed', () => {
        const host = makeHost();
        const c = new AutomationPlanController(host);
        const r = c.tryHandleCompleteFallback(COMPLETE_PLAN_FENCE);
        expect(r.handled).toBe(true);
        expect(c.started).toBe(true);
        expect(host.appendReviewActions).toHaveBeenCalledTimes(1);
    });

    it('signals handled when already started, without firing review again', () => {
        const host = makeHost();
        const c = new AutomationPlanController(host);
        c.tryHandleChunk(COMPLETE_PLAN_FENCE);
        host.appendReviewActions.mockClear();
        const r = c.tryHandleCompleteFallback('does not matter');
        expect(r.handled).toBe(true);
        expect(host.appendReviewActions).not.toHaveBeenCalled();
    });

    it('returns unhandled for completion text with no plan and no in-flight plan', () => {
        const host = makeHost();
        const c = new AutomationPlanController(host);
        const r = c.tryHandleCompleteFallback('plain text');
        expect(r.handled).toBe(false);
    });
});

describe('AutomationPlanController review → run flow', () => {
    it('clicking Run dispatches onRunClicked, removes the actions bar, and kicks execution', async () => {
        const host = makeHost();
        let attachedBar = null;
        host.appendReviewActions = vi.fn((bar) => {
            attachedBar = bar;
            // Simulate the host attaching the bar to the DOM so click-event traversal
            // works the same way as in the real app.
            document.body.appendChild(bar);
        });
        const c = new AutomationPlanController(host);
        c.tryHandleChunk(COMPLETE_PLAN_FENCE);
        expect(attachedBar).toBeTruthy();

        const runBtn = attachedBar.querySelector('#planRunBtn');
        expect(runBtn).toBeTruthy();
        runBtn.click();

        // Wait for async _executePlan to wire its listeners
        await new Promise(r => setTimeout(r, 0));

        expect(host.onRunClicked).toHaveBeenCalled();
        expect(attachedBar.isConnected).toBe(false); // bar removed from DOM
        expect(host.onPlanExecutionStart).toHaveBeenCalled();
        expect(host.invoke).toHaveBeenCalledWith('execute_automation_plan', expect.objectContaining({ planJson: expect.any(String) }));
        expect(host.listen).toHaveBeenCalledWith('automation_step_start', expect.any(Function));
        expect(host.listen).toHaveBeenCalledWith('automation_step_complete', expect.any(Function));
        expect(host.listen).toHaveBeenCalledWith('automation_plan_complete', expect.any(Function));
    });

    it('per-step events update statuses and re-render', async () => {
        const host = makeHost();
        host.appendReviewActions = vi.fn((bar) => document.body.appendChild(bar));
        const c = new AutomationPlanController(host);
        c.tryHandleChunk(COMPLETE_PLAN_FENCE);
        const runBtn = document.body.querySelector('#planRunBtn');
        runBtn.click();
        await new Promise(r => setTimeout(r, 0));

        host.renderTasks.mockClear();
        host._listeners['automation_step_start']({ payload: { step: 1 } });
        expect(c.statuses[1]).toBe('running');
        expect(host.renderTasks).toHaveBeenCalledTimes(1);

        host._listeners['automation_step_complete']({ payload: { step: 1, success: true, result: { ok: true } } });
        expect(c.statuses[1]).toBe('done');
        expect(c.results[1]).toEqual({ ok: true });

        host._listeners['automation_step_complete']({ payload: { step: 1, success: false, stopped: true } });
        expect(c.statuses[1]).toBe('stopped');
    });

    it('plan complete event calls onPlanComplete and clears started + cleanup', async () => {
        const host = makeHost();
        host.appendReviewActions = vi.fn((bar) => document.body.appendChild(bar));
        const c = new AutomationPlanController(host);
        c.tryHandleChunk(COMPLETE_PLAN_FENCE);
        document.body.querySelector('#planRunBtn').click();
        await new Promise(r => setTimeout(r, 0));

        await host._listeners['automation_plan_complete']({ payload: {} });

        expect(c.started).toBe(false);
        expect(c.cleanup).toBe(null);
        expect(host.onPlanComplete).toHaveBeenCalled();
        // All three unlisten thunks fired
        for (const u of host._unlistens) expect(u).toHaveBeenCalled();
    });

    it('execute_automation_plan rejection invokes onPlanFailed and clears started', async () => {
        const host = makeHost({ invoke: vi.fn().mockRejectedValue(new Error('boom')) });
        host.appendReviewActions = vi.fn((bar) => document.body.appendChild(bar));
        const c = new AutomationPlanController(host);
        c.tryHandleChunk(COMPLETE_PLAN_FENCE);
        document.body.querySelector('#planRunBtn').click();
        // Run async chain to completion
        await new Promise(r => setTimeout(r, 10));

        expect(c.started).toBe(false);
        expect(host.onPlanFailed).toHaveBeenCalledWith(expect.any(Error));
    });
});

describe('AutomationPlanController.stopGracefully', () => {
    it('flips running steps to stopped, re-renders, and tears down', async () => {
        const host = makeHost();
        host.appendReviewActions = vi.fn((bar) => document.body.appendChild(bar));
        const c = new AutomationPlanController(host);
        c.tryHandleChunk(COMPLETE_PLAN_FENCE);
        document.body.querySelector('#planRunBtn').click();
        await new Promise(r => setTimeout(r, 0));

        host._listeners['automation_step_start']({ payload: { step: 1 } });
        expect(c.statuses[1]).toBe('running');

        host.renderTasks.mockClear();
        c.stopGracefully();
        expect(c.statuses[1]).toBe('stopped');
        expect(host.renderTasks).toHaveBeenCalledTimes(1);
        expect(c.started).toBe(false);
        expect(c.cleanup).toBe(null);
        for (const u of host._unlistens) expect(u).toHaveBeenCalled();
    });

    it('is a no-op when no plan is running', () => {
        const host = makeHost();
        const c = new AutomationPlanController(host);
        c.stopGracefully();
        expect(host.renderTasks).not.toHaveBeenCalled();
    });
});

describe('AutomationPlanController.reset', () => {
    it('clears all FSM state and any active cleanup', async () => {
        const host = makeHost();
        host.appendReviewActions = vi.fn((bar) => document.body.appendChild(bar));
        const c = new AutomationPlanController(host);
        c.tryHandleChunk(COMPLETE_PLAN_FENCE);
        document.body.querySelector('#planRunBtn').click();
        await new Promise(r => setTimeout(r, 0));

        c.reset();
        expect(c.plan).toBe(null);
        expect(c.statuses).toBe(null);
        expect(c.results).toBe(null);
        expect(c.started).toBe(false);
        expect(c.pendingRevision).toBe(null);
        expect(c.cleanup).toBe(null);
        for (const u of host._unlistens) expect(u).toHaveBeenCalled();
    });
});
