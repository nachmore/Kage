import { sanitizeExtensionHtml, findExtActions } from '../extension-html-sanitizer.js';
import { t, tHtml } from '../i18n.js';
import { applyMixin } from '../mixin.js';

const WIDGET_MIN_INTERVAL_MS = 1_000;
const WIDGET_MAX_INTERVAL_MS = 24 * 3_600 * 1_000;
const WIDGET_SLOW_RENDER_MS = 5_000;
const WIDGET_SLOW_RENDER_RATIO = 0.7;
const WIDGET_FAILURE_TRIP_THRESHOLD = 3;
const WIDGET_STALE_CONTENT_FLOOR_MS = 90_000;

export function installExtensionUiMethods(ExtensionManager) {
    applyMixin(ExtensionManager.prototype, {
        setWidgetSlot(slotName, element) {
            if (!this._widgetSlots) this._widgetSlots = new Map();
            this._widgetSlots.set(slotName, element);
            // If widgets were already mounted to a pending queue, flush them
            // now that the slot exists.
            if (this._pendingWidgetMounts?.has(slotName)) {
                const pending = this._pendingWidgetMounts.get(slotName);
                this._pendingWidgetMounts.delete(slotName);
                pending.forEach((fn) => {
                    try {
                        fn();
                    } catch {}
                });
            }
        },

        async _mountAllWidgets() {
            if (!this._widgetInstances) this._widgetInstances = new Map(); // key: extId + ':' + widgetId
            for (const [extId, ext] of this.extensions) {
                if (!this._isEnabled(extId)) continue;
                if (!Array.isArray(ext.manifest.contributes?.widgets)) continue;
                for (const w of ext.manifest.contributes.widgets) {
                    if (!w?.id || !w?.slot) continue;
                    await this._mountWidget(extId, ext, w);
                }
            }
        },

        async _mountWidget(extId, ext, widgetManifest) {
            const key = `${extId}:${widgetManifest.id}`;
            if (this._widgetInstances.has(key)) return;
            if (!ext.sandbox?.widgetIds?.includes(widgetManifest.id)) {
                // Extension declared the widget in the manifest but the
                // sandbox didn't actually load it (e.g. import error).
                console.warn(`Widget '${key}' declared but not loaded in sandbox — skipping mount`);
                return;
            }

            const host = document.createElement('div');
            host.className = `ext-widget ext-widget-${widgetManifest.slot}`;
            host.dataset.extWidgetKey = key;

            const controller = {
                extensionId: extId,
                widgetId: widgetManifest.id,
                slot: widgetManifest.slot,
                host,
                timer: null,
                destroyed: false,
                refreshIntervalMs: 0,
                // --- Refresh-budget enforcement state ----------------------
                // A widget can declare a healthy 60s interval but still
                // misbehave: throw on every tick, run >> than its declared
                // cadence, or stack overlapping renders if we don't gate.
                // The fields below implement a small circuit-breaker.

                /** True while a renderWidget RPC is in flight. We skip ticks
                 *  rather than letting them stack — prevents unbounded
                 *  pending-promise growth if the extension is slow. */
                renderInFlight: false,

                /** Consecutive failures (RPC throw, timeout, or render that
                 *  blew the slow-render budget). When this hits the trip
                 *  threshold we kill the timer and surface a paused state. */
                consecutiveFailures: 0,

                /** Set true after the breaker trips. The host shows a small
                 *  "Widget paused" message with a retry link; until then we
                 *  don't auto-recover. Manual retry resets the counter. */
                tripped: false,

                /** performance.now() of the last render that actually painted
                 *  (RPC returned, content written to the host). Drives the
                 *  stale-content clear on failure: a frozen "Now" bar from
                 *  hours ago misinforms worse than an empty slot, so once a
                 *  render fails and the painted content is well past its
                 *  refresh cadence we wipe it. 0 = never painted. */
                lastSuccessRenderAt: 0,
            };
            this._widgetInstances.set(key, controller);

            // Attach to slot (or queue if slot not yet registered).
            const slotEl = this._widgetSlots?.get(widgetManifest.slot);
            if (slotEl) {
                slotEl.appendChild(host);
            } else {
                if (!this._pendingWidgetMounts) this._pendingWidgetMounts = new Map();
                const queue = this._pendingWidgetMounts.get(widgetManifest.slot) || [];
                queue.push(() => {
                    const newSlot = this._widgetSlots.get(widgetManifest.slot);
                    if (newSlot) newSlot.appendChild(host);
                });
                this._pendingWidgetMounts.set(widgetManifest.slot, queue);
            }

            // Fetch the extension's refresh interval and do an initial render.
            try {
                const ms = await ext.sandbox.call('getWidgetRefreshInterval', {
                    widgetId: widgetManifest.id,
                });
                const n = Number(ms);
                if (Number.isFinite(n) && n > 0) {
                    // Clamp to a sane range. The floor stops a widget from
                    // re-rendering itself into a CPU spike (declared `1`
                    // interpreted as 1ms), the ceiling keeps the schedule
                    // bounded so the timer doesn't sit idle for years.
                    controller.refreshIntervalMs = Math.max(
                        WIDGET_MIN_INTERVAL_MS,
                        Math.min(n, WIDGET_MAX_INTERVAL_MS)
                    );
                } else {
                    controller.refreshIntervalMs = 0;
                }
            } catch {
                controller.refreshIntervalMs = 0;
            }

            // Between the awaits above and the setInterval below, the
            // extension may have been unloaded (_unmountWidgetsFor flipped
            // `destroyed` and tried to clearInterval on a still-null timer).
            // Bail out here so we don't schedule an orphan interval.
            if (controller.destroyed) return;

            await this._renderWidget(controller);

            if (controller.destroyed) return;

            if (controller.refreshIntervalMs > 0) {
                controller.timer = setInterval(
                    () => this._renderWidget(controller),
                    controller.refreshIntervalMs
                );
            }
        },

        async _renderWidget(controller) {
            if (controller.destroyed || controller.tripped) return;
            // Skip ticks while the floating window is hidden. The host
            // signals this via window._kageFloatingHidden in app.js; the
            // widget is repainting into an off-screen webview otherwise,
            // which is wasted work for both us and the extension.
            // This check is intentionally lighter than the breaker path:
            // a skipped-while-hidden tick is not a failure (no counter
            // increment, no breaker trip). The next tick after the window
            // is shown will catch up.
            if (typeof window !== 'undefined' && window._kageFloatingHidden === true) return;
            const ext = this.extensions.get(controller.extensionId);
            if (!ext?.sandbox) return;

            // Re-entrancy guard. setInterval keeps firing even if the
            // previous render hasn't finished; left unchecked, a slow
            // widget piles up overlapping `renderWidget` RPCs and starves
            // every other RPC on the same sandbox. Skipping is preferable
            // to queueing — by the time the in-flight render returns, its
            // output is already what the next tick would draw.
            if (controller.renderInFlight) {
                this._noteWidgetFailure(controller, 'overlap');
                return;
            }
            controller.renderInFlight = true;

            const start = performance.now();
            let failureReason = null;
            try {
                const out = await ext.sandbox.call('renderWidget', {
                    widgetId: controller.widgetId,
                });
                // Bail if we were unmounted while the RPC was in flight —
                // writing to a detached host is harmless but the listeners
                // we'd wire up would never fire anyway.
                if (controller.destroyed) return;

                const elapsed = performance.now() - start;
                // Slow-render checks. Both bounds are hard caps:
                //   - absolute: 5s blocks the UI noticeably regardless of
                //     declared cadence. We treat that as a failure even
                //     for hourly-cadence widgets.
                //   - relative: a render eating 70%+ of its own interval
                //     is one bad scheduling beat away from overlapping
                //     with itself. Treat as a failure before we hit the
                //     overlap path above.
                if (elapsed >= WIDGET_SLOW_RENDER_MS) {
                    failureReason = 'slow_absolute';
                } else if (
                    controller.refreshIntervalMs > 0 &&
                    elapsed >= controller.refreshIntervalMs * WIDGET_SLOW_RENDER_RATIO
                ) {
                    failureReason = 'slow_relative';
                }

                // The RPC returned and we're about to paint (either content or
                // the empty/hidden state). Either way the host now reflects a
                // fresh render, so reset the staleness clock.
                controller.lastSuccessRenderAt = performance.now();

                if (!out || typeof out.html !== 'string') {
                    // Nothing to render → hide the host so it takes up no layout.
                    controller.host.innerHTML = '';
                    controller.host.style.display = 'none';
                } else {
                    const frag = sanitizeExtensionHtml(out.html, 'rich');
                    controller.host.innerHTML = '';
                    if (out.className) {
                        controller.host.className = `ext-widget ext-widget-${controller.slot} ${out.className}`;
                    }
                    controller.host.style.display = '';
                    controller.host.appendChild(frag);

                    // Wire declared action buttons. We enumerate all
                    // data-ext-action elements in the widget and match by
                    // action id — avoids having to escape arbitrary ids inside
                    // a CSS attribute selector.
                    if (Array.isArray(out.actions)) {
                        const actionMap = new Map();
                        for (const a of out.actions) {
                            if (!a?.id) continue;
                            actionMap.set(a.id, a.rpc || a.id);
                        }
                        if (actionMap.size > 0) {
                            const nodes = controller.host.querySelectorAll('[data-ext-action]');
                            for (const btn of nodes) {
                                const aid = btn.getAttribute('data-ext-action');
                                if (!actionMap.has(aid)) continue;
                                if (btn.__kageExtAction) continue;
                                btn.__kageExtAction = true;
                                const rpc = actionMap.get(aid);
                                btn.addEventListener('click', (ev) => {
                                    ev.preventDefault();
                                    ev.stopPropagation();
                                    this._runWidgetAction(controller, rpc);
                                });
                            }
                        }
                    }
                }

                if (failureReason) {
                    this._noteWidgetFailure(controller, failureReason, elapsed);
                } else {
                    // Successful render resets the failure counter — a single
                    // good tick after two bad ones shouldn't leave us one
                    // tick away from tripping. Transient blips are forgiven.
                    controller.consecutiveFailures = 0;
                }
            } catch (e) {
                console.warn(
                    `widget render for '${controller.extensionId}:${controller.widgetId}' failed:`,
                    e
                );
                this._clearIfStale(controller);
                this._noteWidgetFailure(controller, 'throw');
            } finally {
                controller.renderInFlight = false;
            }
        },

        /** Clear painted widget content once it has gone stale after a failed
         *  render. A single transient blip shouldn't flicker the bar away —
         *  but a time-sensitive widget (calendar's "Now" bar, todos' due
         *  reminders) that keeps failing must not keep showing content that
         *  was true hours ago. We wipe once the painted content is older than
         *  a grace window: max(2× refresh interval, STALE_CONTENT_FLOOR_MS).
         *  The breaker's paused-state notice (after the trip threshold)
         *  supersedes this; this just stops misinformation in the gap before
         *  the breaker trips, and for widgets whose cadence never reaches it. */ _clearIfStale(
            controller
        ) {
            // Never painted, or already empty → nothing stale to clear.
            if (controller.lastSuccessRenderAt === 0) return;
            if (!controller.host || controller.host.style.display === 'none') return;

            const interval = controller.refreshIntervalMs || 0;
            const grace = Math.max(interval * 2, WIDGET_STALE_CONTENT_FLOOR_MS);
            const age = performance.now() - controller.lastSuccessRenderAt;
            if (age < grace) return;

            controller.host.innerHTML = '';
            controller.host.style.display = 'none';
            // Reset the clock so we don't re-evaluate against the same stamp;
            // the next successful render re-arms it.
            controller.lastSuccessRenderAt = 0;
            document.dispatchEvent(new CustomEvent('kage-resize-request'));
        },

        /** Increment the failure counter and trip the breaker if we've hit
         *  the threshold. `reason` is one of:
         *    - 'overlap'        — re-entrant tick skipped
         *    - 'slow_absolute'  — render exceeded WIDGET_SLOW_RENDER_MS
         *    - 'slow_relative'  — render exceeded interval * SLOW_RENDER_RATIO
         *    - 'throw'          — RPC threw or timed out
         *  Each failure increments the counter; a successful render resets
         *  it. Once tripped we stop the timer and surface a paused-state UI
         *  so the user sees what happened. */ _noteWidgetFailure(controller, reason, elapsedMs) {
            controller.consecutiveFailures++;
            const key = `${controller.extensionId}:${controller.widgetId}`;
            console.warn(
                `[widget] ${key} failure (${reason}` +
                    (typeof elapsedMs === 'number' ? `, ${Math.round(elapsedMs)}ms` : '') +
                    `): ${controller.consecutiveFailures}/${WIDGET_FAILURE_TRIP_THRESHOLD}`
            );
            if (controller.consecutiveFailures < WIDGET_FAILURE_TRIP_THRESHOLD) return;

            // Trip the breaker. Stop the timer, mark the controller, and
            // render a small paused notice with a retry link. We keep the
            // host element in the DOM so the user can choose to recover —
            // unmounting would reset the breaker silently on the next
            // refresh anyway.
            controller.tripped = true;
            if (controller.timer) {
                clearInterval(controller.timer);
                controller.timer = null;
            }
            try {
                controller.host.style.display = '';
                controller.host.innerHTML = '';
                const notice = document.createElement('div');
                notice.className = 'ext-widget-paused';
                notice.style.cssText =
                    'padding:8px 12px;font-size:12px;color:var(--kage-text-muted);background:var(--kage-bg-input);border-radius:4px;display:flex;align-items:center;gap:8px;';
                const extName =
                    this.extensions.get(controller.extensionId)?.manifest?.name ||
                    controller.extensionId;
                notice.innerHTML = tHtml('shared.extension.widget.paused_html', { name: extName });
                const retry = document.createElement('a');
                retry.href = '#';
                retry.textContent = t('shared.extension.widget.retry');
                retry.style.cssText = 'color:var(--kage-accent);text-decoration:underline;';
                retry.addEventListener('click', (ev) => {
                    ev.preventDefault();
                    this._retryWidget(controller);
                });
                notice.appendChild(retry);
                controller.host.appendChild(notice);
            } catch {
                // DOM may be in any state if we got here mid-render; the
                // breaker tripping is what matters, the UI hint is
                // best-effort.
            }

            // Telemetry — surface in aggregate so we can spot a problematic
            // extension across the install base. Anonymous: extension id
            // only, no widget content. Best-effort import so this file
            // doesn't hard-depend on the telemetry module being loadable.
            try {
                import('../telemetry.js')
                    .then(({ trackEvent }) =>
                        trackEvent('extension_widget_disabled', {
                            extension_id: controller.extensionId,
                            widget_id: controller.widgetId,
                            reason,
                        })
                    )
                    .catch(() => {});
            } catch {}
        },

        /** Reset and resume a tripped widget. Single retry: if it trips
         *  again, we leave it paused — repeated retries would let a broken
         *  widget burn CPU indefinitely. The user can disable the
         *  extension if it never recovers. */ _retryWidget(controller) {
            if (controller.destroyed) return;
            controller.consecutiveFailures = 0;
            controller.tripped = false;
            controller.renderInFlight = false;
            controller.host.innerHTML = '';
            this._renderWidget(controller).then(() => {
                if (controller.destroyed || controller.tripped) return;
                if (controller.refreshIntervalMs > 0 && !controller.timer) {
                    controller.timer = setInterval(
                        () => this._renderWidget(controller),
                        controller.refreshIntervalMs
                    );
                }
            });
        },

        async _runWidgetAction(controller, actionId) {
            const ext = this.extensions.get(controller.extensionId);
            if (!ext?.sandbox) return;
            try {
                const out = await ext.sandbox.call('onWidgetAction', {
                    widgetId: controller.widgetId,
                    actionId,
                    context: {},
                });
                // If the action returns an immediate re-render request, do it.
                if (out?.rerender) {
                    await this._renderWidget(controller);
                }
            } catch (e) {
                console.warn(
                    `widget action '${actionId}' in '${controller.extensionId}' failed:`,
                    e
                );
            }
        },

        /** Re-render every mounted widget now. Called when the floating
         *  window becomes visible: `_renderWidget` skips ticks while the
         *  window is hidden (window._kageFloatingHidden), so a widget mounted
         *  while hidden — or whose periodic ticks were all skipped — would
         *  otherwise show nothing until its next interval fires after the
         *  window is already open. This catches them up on show so the first
         *  visible paint has current content. Tripped/destroyed widgets are
         *  skipped by _renderWidget itself. Fire-and-forget per widget. */ renderAllWidgets() {
            if (!this._widgetInstances) return;
            for (const ctrl of this._widgetInstances.values()) {
                this._renderWidget(ctrl);
            }
        },

        _unmountWidgetsFor(extensionId) {
            if (!this._widgetInstances) return;
            for (const [key, ctrl] of this._widgetInstances) {
                if (ctrl.extensionId !== extensionId) continue;
                ctrl.destroyed = true;
                if (ctrl.timer) clearInterval(ctrl.timer);
                try {
                    ctrl.host.remove();
                } catch {}
                this._widgetInstances.delete(key);
            }
        },

        /**
         * Refresh the cached list of toolbar buttons from all enabled
         * extensions that expose a toolbar provider. Call after init and on
         * config updates; readers use the synchronous `getToolbarButtons()`.
         */ async _refreshToolbarButtons() {
            const out = [];
            for (const [id, ext] of this.extensions) {
                if (!ext.sandbox?.hasToolbar) continue;
                if (!this._isEnabled(id)) continue;
                try {
                    const defs = await ext.sandbox.call('getToolbarButtons', {});
                    if (Array.isArray(defs)) {
                        for (const d of defs) {
                            if (!d?.id) continue;
                            out.push({
                                extensionId: id,
                                id: String(d.id),
                                icon: String(d.icon || '🧩'),
                                tooltip: String(d.tooltip || ''),
                            });
                        }
                    }
                } catch (e) {
                    console.warn(`toolbar getButtons() in '${id}' failed:`, e);
                }
            }
            this._toolbarButtonsCache = out;
            return out;
        },

        /**
         * Synchronous snapshot of toolbar buttons. The cache is primed by
         * `initialize()` and refreshed on config change.
         */ getToolbarButtons() {
            if (!this._toolbarButtonsCache) return [];
            return this._toolbarButtonsCache.map((b) => ({
                ...b,
                // The onClick callback bridges to the sandbox RPC. The call
                // site provides the current chat context (input + messages).
                onClick: (ctx) => this.runToolbarClick(b.extensionId, b.id, ctx),
            }));
        },

        /**
         * Execute a toolbar button click. `ctx` carries the current chat
         * input and messages so the extension can make an informed decision
         * without DOM access. Returns a host effect the caller should
         * apply (set input, send message, etc.) or null.
         */ async runToolbarClick(extensionId, buttonId, ctx = {}) {
            const ext = this.extensions.get(extensionId);
            if (!ext?.sandbox?.hasToolbar) return null;
            try {
                // Marshal ctx so extensions can't reach back into live DOM
                // via functions accidentally passed in.
                const safeCtx = {
                    input: typeof ctx.input === 'string' ? ctx.input : '',
                    messages: Array.isArray(ctx.messages)
                        ? ctx.messages.map((m) => ({
                              role: String(m?.role || ''),
                              content: typeof m?.content === 'string' ? m.content : '',
                          }))
                        : [],
                };
                const out = await ext.sandbox.call('onToolbarClick', {
                    buttonId,
                    context: safeCtx,
                });
                return out && typeof out === 'object' ? out : null;
            } catch (e) {
                console.warn(`toolbar onClick in '${extensionId}' failed:`, e);
                return null;
            }
        },

        // --- Message formatter -------------------------------------------------

        /**
         * Run all enabled extension message formatters against the rendered
         * container. Each formatter receives the container's innerHTML and
         * returns either a replacement string (sanitized and applied) or
         * null to leave the content unchanged. During streaming we skip
         * formatters that haven't opted into live formatting.
         */ async formatMessage(container, context) {
            if (!container || !this.extensions?.size) return;
            const ctx = {
                streaming: !!context?.streaming,
                role: String(context?.role || ''),
            };
            for (const [id, ext] of this.extensions) {
                if (!ext.sandbox?.hasFormatter) continue;
                if (!this._isEnabled(id)) continue;
                // Skip streaming calls unless the extension explicitly
                // opted in. Most formatters return null during streaming,
                // so round-tripping per chunk is wasted work.
                if (ctx.streaming && !ext.sandbox.formatterOptsInStreaming) continue;
                try {
                    const out = await ext.sandbox.call('formatMessage', {
                        html: container.innerHTML,
                        context: ctx,
                    });
                    if (out && typeof out.html === 'string') {
                        const frag = sanitizeExtensionHtml(out.html, 'rich');
                        // Replace the container's children with the sanitized
                        // fragment. We use replaceChildren so existing
                        // listeners on the container itself are preserved.
                        container.replaceChildren();
                        container.appendChild(frag);
                        // Wire any declared extension actions in the new DOM.
                        this._wireExtActionsFor(id, container);
                    }
                } catch (e) {
                    console.warn(`message formatter in '${id}' failed:`, e);
                }
            }
        },

        // --- Shared: wire data-ext-action buttons in sanitized HTML -----------
        _wireExtActionsFor(extensionId, root) {
            const hits = findExtActions(root);
            for (const { element, actionId } of hits) {
                // Defensive: prevent double-wiring when the same container is
                // re-formatted multiple times during streaming.
                if (element.__kageExtAction) continue;
                element.__kageExtAction = true;
                element.addEventListener('click', (ev) => {
                    ev.preventDefault();
                    ev.stopPropagation();
                    const ext = this.extensions.get(extensionId);
                    if (!ext?.sandbox) return;
                    // Custom-render actions are for result-row buttons: they
                    // all flow through onWidgetAction-style RPC because we
                    // don't yet have a `onRenderAction` — in Commit C we route
                    // them through the search provider's execute() with a
                    // synthetic result carrying { action: actionId }.
                    ext.sandbox
                        .call('onResultAction', {
                            actionId,
                            resultId: root.dataset?.extResultId || null,
                        })
                        .catch((e) => console.warn(`onResultAction '${actionId}' failed:`, e));
                });
            }
        },
    });
}
