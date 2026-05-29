/**
 * Automation Plan Controller — shared between FloatingApp and ChatApp.
 *
 * Owns the automation-plan FSM: incremental detection while the agent
 * streams the plan fence, the "ready-for-review" interstitial with its Run
 * button, the per-step listener wiring, and the cleanup. The two windows
 * differ only in where the task list renders (shared response area vs. a
 * per-message content div) and what UI fires around lifecycle transitions
 * (resize/scroll, thinking dots vs typing indicator, stop button) — those
 * flow through the host adapter.
 *
 * Lifecycle the host drives:
 *   - `controller.reset()` when starting a new prompt
 *   - `controller.tryHandleChunk(text)` from each chunk handler — `{ handled }`
 *     is true if the controller has taken over rendering this chunk
 *   - `controller.tryHandleCompleteFallback(text)` from each completion
 *     handler — `{ handled }` is true if the controller surfaced a plan that
 *     wasn't caught mid-stream
 *   - `controller.stopGracefully()` from the host's stop/cancel path
 *
 * Reads exposed on the controller:
 *   - `controller.started`     — true while a plan is in review or running
 *   - `controller.plan`        — current plan or null
 *   - `controller.pendingRevision` — set while waiting for user to either
 *     hit Run or type a revision; consumed by host's send-message path
 *
 * Host adapter shape:
 *   - invoke(cmd, args)               REQUIRED
 *   - listen(event, handler)          REQUIRED — Tauri-style event listener;
 *                                     must return an unlisten thunk
 *   - renderTasks(tasks)              REQUIRED — paint the task list and bump
 *                                     layout (resize / scroll)
 *   - appendReviewActions(actionsBar) REQUIRED — attach the Run button bar
 *                                     to the right DOM target
 *   - onPlanReadyForReview()          stop thinking/typing UI; flip
 *                                     isWaitingForResponse to false
 *   - onPlanExecutionStart()          show the stop button etc.
 *   - onPlanComplete()                post-completion UI (response actions,
 *                                     focus, load sessions, …)
 *   - onPlanFailed(err)               error UI
 *   - onRunClicked()                  fired when the user hits Run, before
 *                                     execution begins (used by floating to
 *                                     stop window-hide on outside click)
 */

import {
    detectAutomationPlan,
    detectAutomationPlanIncremental,
    automationPlanToTasks,
} from './streaming-utils.js';

import { EVT } from './events.js';
import { t } from './i18n.js';

export class AutomationPlanController {
    constructor(host) {
        this.host = host;
        this.plan = null;
        this.statuses = null;
        this.results = null;
        this.started = false;
        this.cleanup = null;
        this.pendingRevision = null;
    }

    reset() {
        this.plan = null;
        this.statuses = null;
        this.results = null;
        this.started = false;
        this.pendingRevision = null;
        if (this.cleanup) {
            this.cleanup();
            this.cleanup = null;
        }
    }

    tryHandleChunk(text) {
        if (this.started) return { handled: true };

        const completePlan = detectAutomationPlan(text);
        if (completePlan) {
            this.started = true;
            this._showForReview(completePlan);
            return { handled: true };
        }

        const partialPlan = detectAutomationPlanIncremental(text);
        if (partialPlan) {
            this.plan = partialPlan;
            this.statuses = {};
            this.results = {};
            for (const s of partialPlan) this.statuses[s.step] = 'pending';
            this._renderTasks();
            return { handled: true };
        }

        return { handled: false };
    }

    tryHandleCompleteFallback(text) {
        if (this.started) return { handled: true };
        const plan = detectAutomationPlan(text);
        if (plan) {
            this.started = true;
            this._showForReview(plan);
            return { handled: true };
        }
        return { handled: false };
    }

    stopGracefully() {
        if (!this.started || !this.statuses) return;
        for (const [step, status] of Object.entries(this.statuses)) {
            if (status === 'running') this.statuses[step] = 'stopped';
        }
        this._renderTasks();
        if (this.cleanup) this.cleanup();
        this.cleanup = null;
        this.started = false;
    }

    /** Caller hit Run or accepted the plan inline — kick off execution. */
    runPendingPlan() {
        const plan = this.pendingRevision || this.plan;
        if (!plan) return;
        this.pendingRevision = null;
        this._executePlan(plan);
    }

    _showForReview(plan) {
        this.plan = plan;
        this.statuses = {};
        this.results = {};
        for (const s of plan) this.statuses[s.step] = 'pending';
        this._renderTasks();

        const actionsBar = document.createElement('div');
        actionsBar.className = 'taskplan-review-actions';
        actionsBar.innerHTML = `
            <button class="taskplan-review-btn taskplan-run-btn" id="planRunBtn">${t('shared.automation.run_btn')}</button>
            <span class="taskplan-review-hint">${t('shared.automation.run_hint')}</span>
        `;
        this.host.appendReviewActions(actionsBar);

        this.host.onPlanReadyForReview?.();

        const runBtn = actionsBar.querySelector('#planRunBtn');
        if (runBtn) {
            // Prevent any window-hide-on-mousedown logic from firing.
            runBtn.addEventListener('mousedown', (e) => e.preventDefault());
            runBtn.addEventListener('click', (e) => {
                e.stopPropagation();
                actionsBar.remove();
                this.pendingRevision = null;
                this.host.onRunClicked?.();
                this._executePlan(plan);
            });
        }

        // Pending-revision flag lets the host's send-message path detect that
        // a follow-up message should be treated as a plan revision rather
        // than a brand-new prompt.
        this.pendingRevision = plan;
    }

    async _executePlan(plan) {
        this.plan = plan;
        this.statuses = {};
        this.results = {};
        for (const s of plan) this.statuses[s.step] = 'pending';
        this._renderTasks();

        this.host.onPlanExecutionStart?.();

        this.cleanup = null;

        const stepStartUnlisten = await this.host.listen('automation_step_start', (event) => {
            const { step } = event.payload;
            this.statuses[step] = 'running';
            this._renderTasks();
        });

        const stepCompleteUnlisten = await this.host.listen(
            EVT.AUTOMATION_STEP_COMPLETE,
            (event) => {
                const { step, success, result, stopped } = event.payload;
                this.statuses[step] = stopped ? 'stopped' : success ? 'done' : 'failed';
                if (result) this.results[step] = result;
                this._renderTasks();
            }
        );

        const cleanup = () => {
            stepStartUnlisten();
            stepCompleteUnlisten();
            planCompleteUnlisten();
            this.cleanup = null;
        };
        this.cleanup = cleanup;

        const planCompleteUnlisten = await this.host.listen(
            'automation_plan_complete',
            async () => {
                cleanup();
                this.started = false;
                await this.host.onPlanComplete?.();
            }
        );

        try {
            await this.host.invoke('execute_automation_plan', {
                sessionId: this.host.getSessionId?.() || null,
                planJson: JSON.stringify(plan),
            });
        } catch (e) {
            console.error('Automation plan execution failed:', e);
            cleanup();
            this.started = false;
            this.host.onPlanFailed?.(e);
        }
    }

    _renderTasks() {
        if (!this.plan) return;
        const tasks = automationPlanToTasks(this.plan, this.statuses || {}, this.results || {});
        this.host.renderTasks(tasks);
    }
}
