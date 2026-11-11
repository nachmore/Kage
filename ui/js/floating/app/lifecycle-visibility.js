import {
    checkOnline,
    getConfig,
    onNetworkChange,
    renderUnifiedResults,
    signalMascot,
    signalMascotUpdated,
    unifiedSearch,
} from './dependencies.js';
import { EVT } from '../../shared/events.js';
import { installFileTextBroker } from '../../shared/extension-file-text.js';
import { presentOverlayWindow } from '../../shared/overlay-window.js';

/**
 * How long the launcher ignores blurs after it asked Rust to open a view
 * (docs/design/EXTENSION_VIEWS.md §15.2, Rule A): building a WebView
 * window can take a few hundred milliseconds.
 */
export const VIEW_OPEN_BLUR_GRACE_MS = 1500;

/**
 * How long the launcher ignores blurs, and keeps its content on a show,
 * after an extension's file dialog closed (EXTENSION_FILE_TEXT.md §5.3).
 */
export const FILE_DIALOG_BLUR_GRACE_MS = 1500;

/**
 * A `floating_compose` within this long keeps the show-time reset
 * (`preserve_last_response: false`) from wiping the composed text.
 */
const COMPOSE_KEEP_MS = 1000;

/** How long after a focus that found the launcher hidden to look again. */
const FOCUS_VISIBILITY_RECHECK_MS = 300;

export const LifecycleVisibilityMethods = {
    /** Is the floating window actually on screen? Errors count as visible. */
    async _isWindowVisible() {
        try {
            return await this.appWindow.isVisible();
        } catch {
            return true;
        }
    },

    setupNetworkMonitor() {
        const bar = document.getElementById('offlineBar');
        const update = (online) => {
            if (bar) bar.style.display = online ? 'none' : 'flex';
            this.windowManager.resizeWindow();
            // Kage's Wi-Fi bubble. `online` while not offline is a no-op in
            // the engine, so the startup check can send it unconditionally.
            signalMascot(online ? 'online' : 'offline');
        };
        // Do a real connectivity check on startup
        checkOnline().then((online) => update(online));
        onNetworkChange(update);
    },

    setupVisibilityTracking() {
        this._windowFocused = true; // assume focused at startup
        this._visibilitySequence = 0;
        // Pause extension widgets independently. The global hidden flag also
        // gates Ollama's initial status probe, which must run during startup.
        this.extensionManager?.setWidgetVisibility(false);

        // --- Pause CSS animations when hidden to stop GPU compositing ---
        // WebView2 keeps processing infinite CSS animations even on hidden windows,
        // which causes the shared GPU process to burn CPU continuously.
        if (!document.getElementById('kage-anim-pause-style')) {
            const s = document.createElement('style');
            s.id = 'kage-anim-pause-style';
            s.textContent =
                '.animations-paused, .animations-paused * { animation-play-state: paused !important; }';
            document.head.appendChild(s);
        }
        // Window starts hidden — pause immediately
        document.documentElement.classList.add('animations-paused');
        // Also catch visibility changes (belt-and-suspenders with focus/blur)
        document.addEventListener('visibilitychange', () => {
            if (document.hidden) document.documentElement.classList.add('animations-paused');
            else document.documentElement.classList.remove('animations-paused');
        });

        // Through the methods object, so a host that mixed in only part of
        // this object (tests) still gets both handlers.
        this.appWindow.listen('tauri://focus', () =>
            LifecycleVisibilityMethods._onFocus.call(this)
        );
        LifecycleVisibilityMethods._installBlurListener.call(this);
    },

    /**
     * The launcher got focus. If it's actually showing, undo what hiding
     * paused (CSS animations, the mascot, widgets) and refresh what a show
     * refreshes. Startup focuses the still-hidden window, so a focus alone
     * isn't a show; but a real show can also report focus a beat before the
     * window reports itself visible, so a focus that finds it hidden looks
     * again once, shortly after. Missing that left the window on screen with
     * its animations paused: the loading dots sat still while the elapsed
     * counter kept counting.
     *
     * @param {{retry?: boolean}} [opts]
     */
    async _onFocus({ retry = true } = {}) {
        {
            const sequence = ++this._visibilitySequence;
            this._windowFocused = true;
            const visible = await this._isWindowVisible();
            if (sequence !== this._visibilitySequence) return;
            // Was this a hide→show cycle, or just regaining focus on an
            // already-visible window? The flag is only ever set to false by
            // this handler, so `undefined` is the genuinely-hidden startup
            // state and counts as a reopen too.
            const wasHidden = window._kageFloatingHidden !== false;
            window._kageFloatingHidden = !visible;
            if (!visible) {
                this.extensionManager?.setWidgetVisibility(false);
                if (retry) {
                    setTimeout(() => {
                        if (sequence === this._visibilitySequence && this._windowFocused) {
                            LifecycleVisibilityMethods._onFocus.call(this, { retry: false });
                        }
                    }, FOCUS_VISIBILITY_RECHECK_MS);
                }
                return;
            }
            // An uninstall confirmation doesn't survive the launcher being
            // hidden (e.g. by its hotkey): coming back starts fresh.
            if (wasHidden) this._cancelResultConfirm?.();
            // A show starts without a view companion (§15.2).
            if (wasHidden) this._viewCompanion = false;
            document.documentElement.classList.remove('animations-paused');
            // Resume before signaling: reopen resets the previous party hat,
            // summon pops in, and the banner check may then celebrate the update.
            if (window._kageMascot) {
                try {
                    window._kageMascot.resume();
                } catch (e) {
                    console.warn('mascot.resume failed:', e);
                }
            }
            if (wasHidden) {
                signalMascot('reopen');
                signalMascot('summon');
            }
            // First focus this process — show the post-update banner
            // if last_updated_version is still set. Running it here
            // (instead of from init()) means the banner waits until
            // the user can actually see the window: interactive
            // installs trigger this immediately via setup's auto-show,
            // idle installs trigger it the first time the user summons
            // the floating window manually. Either way the user
            // actually sees it.
            // Startup also fires focus on the still-hidden window while the
            // other webviews paint (seconds before the post-update show).
            // Spending the one-shot check there plays the celebration into
            // an invisible window, and the real show's `reopen` then clears
            // the party hat — so only count a focus the user can see.
            if (!this._updateBannerChecked) {
                this._updateBannerChecked = true;
                // The post-install auto-show races against other windows'
                // webviews painting for the first time (notably the
                // preloaded chat window's main.js init). Whichever paints
                // later steals focus and the blur handler would hide us —
                // taking the banner with it. Suppress the next ~2s of
                // blur-hides so the user actually sees the celebration.
                this.banner.checkForUpdateBanner().then((shown) => {
                    if (!shown) return;
                    this._suppressBlurHideUntil = Date.now() + 2000;
                    // First launch after an update: balloons, then a party hat
                    // that stays on until the user types or reopens. Fires
                    // after the reopen/summon pair above, so the hat isn't
                    // immediately cleared.
                    signalMascotUpdated();
                });
            }
            // Startup may focus the still-hidden webview before post-init
            // checks. Catch up its skipped Ollama probe on the actual show.
            const status = this._ollamaStatus;
            if (
                wasHidden &&
                status &&
                (status.probeSkipped ||
                    status.lastProbeAt == null ||
                    Date.now() - status.lastProbeAt >= 30_000)
            ) {
                this._pollOllamaStatusOnce().catch(() => {});
            }
            // Resume widget timers and catch up missing or stale content.
            // Startup can send focus while the window is still hidden.
            try {
                this.extensionManager?.setWidgetVisibility(visible)?.catch((e) => {
                    console.warn('Widget resume on show failed:', e);
                });
            } catch (e) {
                console.warn('Widget resume on show failed:', e);
            }
            // Notify updater of activity
            this.invoke('touch_floating_activity').catch(() => {});

            // Refresh App Mode chip — the foreground app at summon
            // time is what we'll inject steering for, and that's
            // captured here just before the user starts typing.
            this._refreshAppModeChip().catch((e) => console.log('App Mode lookup failed:', e));

            // Check network status when launcher is invoked (debounced)
            checkOnline().then((online) => {
                const bar = document.getElementById('offlineBar');
                if (bar) bar.style.display = online ? 'none' : 'flex';
                this.windowManager.resizeWindow();
            });

            // Ensure toolbar is visible if configured
            this._updateToolbarVisibility();

            // Restore any overlays hidden by clipboard mode
            if (!this._clipboardMode) {
                this._restoreOverlaysAfterClipboard();
            }

            // Clear any pending system command confirmations and re-trigger search
            if (this.currentMatches.some((m) => m.type === 'system_confirm')) {
                const query = this.elements.input.value.trim();
                if (query) {
                    this.clearSuggestions();
                    const results = await unifiedSearch(query, this.invoke, this.shortcuts);
                    if (results.length > 0) {
                        const { selectedIndex, matches } = await renderUnifiedResults(
                            results,
                            this.elements.appSuggestions,
                            () => this.windowManager.resizeWindow(),
                            (r) => this._onResultClick(r)
                        );
                        this.currentMatches = matches;
                        this.selectedIndex = selectedIndex;
                    }
                } else {
                    this.clearSuggestions();
                }
            }

            // Don't reset UI if permission modal is open
            const permissionModal = document.getElementById('permissionModal');
            if (this._resultConfirm) {
                // An uninstall confirmation keeps the keyboard, not the input.
                setTimeout(() => this._resultConfirm?.cancelButton?.focus(), 50);
            } else if (!permissionModal || permissionModal.style.display === 'none') {
                if (this.isWaitingForResponse) {
                    setTimeout(() => this.elements.input.focus(), 50);
                } else {
                    try {
                        const config = await getConfig(this.invoke);
                        if (config.ui?.preserve_last_response === false) {
                            // Text a view just composed is what this show
                            // is for: keep it (and select it) instead.
                            setTimeout(() => {
                                if (this._composedRecently()) {
                                    this.elements.input.focus();
                                    this.elements.input.select();
                                } else if (this._fileDialogClosedRecently()) {
                                    // Back from an extension's file dialog:
                                    // its answer is still coming.
                                    this.elements.input.focus();
                                } else {
                                    this.resetUI();
                                }
                            }, 50);
                        } else {
                            setTimeout(() => {
                                this.elements.input.focus();
                                if (!this._clipboardMode) this.elements.input.select();
                            }, 50);
                        }
                    } catch (_e) {
                        setTimeout(() => {
                            this.elements.input.focus();
                            if (!this._clipboardMode) this.elements.input.select();
                        }, 50);
                    }
                }
            }

            this.updateDatetimeVisibility();
        }
    },

    /** Hide (and pause) when the launcher loses focus, unless it should stay. */
    _installBlurListener() {
        this.appWindow.listen('tauri://blur', async () => {
            const sequence = ++this._visibilitySequence;
            this._windowFocused = false;
            // Suppress blur-hide briefly after a post-update show. We
            // honour two signals:
            //
            //   `this._suppressBlurHideUntil` — set by checkForUpdateBanner
            //     once the banner is rendered. Catches blurs that happen
            //     after focus stabilises. Opening an extension view arms
            //     it too (`_armViewCompanion`).
            //
            //   `window._kagePostUpdateSuppressUntil` — pre-armed by the
            //     Rust setup code (`maybe_show_floating_after_interactive_install`)
            //     via WebviewWindow::eval BEFORE the floating window is
            //     shown. Catches the early focus-thrashing storm that
            //     fires while the chat / inline-assist preloaded webviews
            //     paint for the first time — the JS bootstrap hasn't yet
            //     run, so the instance flag above is undefined and the
            //     blur would otherwise hide the window before the user
            //     sees the celebration banner.
            const winSuppress = window._kagePostUpdateSuppressUntil;
            if (winSuppress && Date.now() < winSuppress) {
                return;
            }
            if (this._suppressBlurHideUntil && Date.now() < this._suppressBlurHideUntil) {
                return;
            }
            if (this._keepOpenOnBlur()) return;
            // Rule B (docs/design/EXTENSION_VIEWS.md §15.2): focus went to an
            // extension view, so stay beside it. Asks Rust only when a view
            // can exist at all.
            if (await this._staysForView(sequence)) return;
            await this._hideOnBlur();
        });

        // Rule B's other half: the user left Kage from a view. Rust sends
        // this when neither the launcher nor a view has focus a moment
        // after a view lost it.
        this.appWindow.listen(EVT.EXT_VIEW_FOCUS_LOST, () => this._onViewFocusLost());

        // A view's `context.view.compose(text)`: Rust has shown the
        // launcher; put the text in the input. Never sent.
        this.appWindow.listen(EVT.FLOATING_COMPOSE, (event) => this._applyCompose(event?.payload));

        // Close-time cleanup. The floating window is normally hidden,
        // not closed — this listener is defensive insurance for two
        // cases: (1) a future refactor that flips floating to
        // close-on-dismiss, and (2) explicit teardown via the tray
        // "quit" path. Without it, a closed-but-not-destroyed webview
        // would sit there with mascot timers still ticking until the
        // process exits. The cleanup is intentionally narrow:
        //   - Mascot intervals (the only timer we own that runs while
        //     hidden today).
        //   - Extension manager teardown via destroy(), which cascades
        //     to widget timers and sandbox iframes.
        // We deliberately don't strip event listeners — webview
        // teardown frees the JS heap wholesale, and the cost of
        // walking every DOM listener individually exceeds the wholesale
        // GC the runtime is about to do anyway.
        this.appWindow.listen('tauri://close-requested', () => {
            try {
                if (window._kageMascot) {
                    window._kageMascot.destroy();
                    window._kageMascot = null;
                }
                this._stopOllamaStatusPoll();
                // Not stopSteering(): the window is only hidden (close is
                // prevented), and the next reload re-populates the manager,
                // whose tool changes must keep reaching the backend. A real
                // teardown takes the JS with it, and the backend drops the
                // window's publication on Destroyed.
                this.extensionManager?.destroy?.();
            } catch (e) {
                console.warn('floating close-requested cleanup failed:', e);
            }
        });
    },

    /**
     * Whether the launcher must stay open when it loses focus: a
     * permission prompt, a drag, a menu, a pending confirmation, running
     * tools or plans, a file picker. Shared by the blur handler and
     * `ext_view_focus_lost`.
     */
    _keepOpenOnBlur() {
        // Don't hide if permission modal is open
        const permissionModal = document.getElementById('permissionModal');
        if (permissionModal && permissionModal.style.display !== 'none') {
            return true;
        }
        // Don't hide if dragging the window
        if (this.windowManager.isDragging) {
            return true;
        }
        // Don't hide if context menu is open
        const contextMenu = document.querySelector('.context-menu');
        if (contextMenu && contextMenu.style.display !== 'none') {
            return true;
        }
        // Don't hide if context menu popup window is open
        if (window._contextMenuOpen) {
            return true;
        }
        // Don't hide while an uninstall confirmation, or the result
        // action it started, is pending (app/result-menu.js)
        if (this._resultConfirm || this._resultActionBusy) {
            return true;
        }
        // Don't hide if we just stopped generating (prevents accidental hide on Esc)
        if (this._justStoppedGenerating) {
            return true;
        }
        // Don't hide if computer control is active — user needs to track progress
        if (this.computerControlActive) {
            return true;
        }
        // Don't hide if a focus-stealing MCP tool is running (e.g. folder picker dialog)
        if (this._noBlurTools.size > 0) {
            return true;
        }
        // Don't hide if an automation plan is running
        if (this._automationPlanStarted) {
            return true;
        }
        // Don't hide if file picker is open
        if (this._filePickerOpen) {
            return true;
        }
        // Don't hide while an extension's file dialog hid the launcher (§5.3)
        if (this._nativeDialogOpen) {
            return true;
        }
        // Don't hide while an extension tool is being processed
        if (this._extensionToolExecuting || this._extensionToolCallHandled) {
            return true;
        }
        return false;
    },

    /**
     * Rule A (docs/design/EXTENSION_VIEWS.md §15.2): the launcher is about
     * to open an extension view, which takes focus. Ignore blurs while the
     * window is built, and stay as the view's companion afterwards.
     */
    _armViewCompanion() {
        this._suppressBlurHideUntil = Math.max(
            this._suppressBlurHideUntil || 0,
            Date.now() + VIEW_OPEN_BLUR_GRACE_MS
        );
        this._viewCompanion = true;
    },

    /**
     * Rule B: whether this blur moved focus to an extension view window,
     * so the launcher stays (and becomes the view's companion). Also true
     * when the launcher got focus back while Rust was asked. Asks only
     * when an extension here can have a view, so the common blur costs no
     * IPC.
     *
     * @param {number} sequence - the blur's `_visibilitySequence`
     */
    async _staysForView(sequence) {
        if (!this.extensionManager?.hasViewCapableExtensions?.()) return false;
        let viewFocused = false;
        try {
            const state = await this.invoke('ext_view_focus_state');
            viewFocused = state?.view_focused === true;
        } catch {}
        if (sequence !== this._visibilitySequence) return true;
        if (!viewFocused) return false;
        // Already hidden (Esc, a hide action): nothing to keep beside the view.
        if (!(await this._isWindowVisible())) return false;
        if (sequence !== this._visibilitySequence) return true;
        this._viewCompanion = true;
        return true;
    },

    /**
     * `ext_view_focus_lost`: the user left Kage from a view. Hide the way
     * a blur does, if the launcher stayed as the view's companion and
     * doesn't have focus itself.
     */
    async _onViewFocusLost() {
        if (!this._viewCompanion || this._windowFocused) return;
        if (this._keepOpenOnBlur()) return;
        await this._hideOnBlur();
    },

    /**
     * `floating_compose { text }`: a view put text in the launcher's input
     * (§16.1). Replace the input and select it. Nothing is sent: the user
     * decides.
     */
    _applyCompose(payload) {
        const text = payload && typeof payload.text === 'string' ? payload.text : null;
        const input = this.elements?.input;
        if (text === null || !input) return;
        this._composedAt = Date.now();
        input.value = text;
        // Autogrow and suggestions follow the new text, as for typing.
        input.dispatchEvent(new Event('input', { bubbles: true }));
        input.focus();
        input.select();
    },

    /**
     * The launcher's file broker (EXTENSION_FILE_TEXT.md §5.3): the ask bar
     * and chip go at the top of the bubble, and the window hides while an
     * extension's file dialog is open, so the dialog gets focus and isn't
     * hidden under an always-on-top bar.
     */
    _installFileTextBroker() {
        installFileTextBroker({
            listen: this.listen,
            isVisible: () => this._isWindowVisible(),
            guard: { begin: () => this._beginFileDialog(), end: () => this._endFileDialog() },
            mount: () => document.querySelector('.speech-bubble'),
            onLayoutChange: () => this.windowManager?.resizeWindow(),
        });
    },

    async _beginFileDialog() {
        this._nativeDialogOpen = true;
        await this.appWindow.hide();
    },

    /** Show the launcher again the usual way, ignoring blurs for a moment. */
    async _endFileDialog() {
        const now = Date.now();
        this._suppressBlurHideUntil = Math.max(
            this._suppressBlurHideUntil || 0,
            now + FILE_DIALOG_BLUR_GRACE_MS
        );
        this._fileDialogClosedAt = now;
        this._nativeDialogOpen = false;
        await presentOverlayWindow(this.appWindow);
    },

    _fileDialogClosedRecently() {
        return (
            !!this._fileDialogClosedAt &&
            Date.now() - this._fileDialogClosedAt < FILE_DIALOG_BLUR_GRACE_MS
        );
    },

    /** Whether a view composed text into the input a moment ago. */
    _composedRecently() {
        return !!this._composedAt && Date.now() - this._composedAt < COMPOSE_KEEP_MS;
    },

    /** Hide after a blur, and pause what doesn't need to run while hidden. */
    async _hideOnBlur() {
        this._viewCompanion = false;
        await this.appWindow.hide();
        this.banner.dismiss();
        // Forget a manual resize made on the empty launcher. Resizing with
        // no response stretches the input panel; that's fine while open,
        // but it shouldn't survive a hide/show — the launcher should come
        // back at its natural (input + bars) height. When a response IS
        // showing, the manual size is intentional (e.g. dragged bigger to
        // read a long answer) and is preserved. resizeWindow() snaps the
        // now-hidden window back so the next show paints at natural height.
        if (!this.windowManager._hasResponseContent()) {
            this.windowManager.userSetHeight = null;
            this.windowManager.resizeWindow();
        }
        // Pause work that doesn't need to run while hidden.
        // - Mascot animation: ticks every ~120-150ms; over a long
        //   idle session that's real CPU we can give back to the
        //   foreground app the user is actually using.
        // - Hidden flag: read by ExtensionManager._renderWidget so
        //   long-cadence widgets (calendar, todos) skip ticks
        //   that would otherwise repaint into an invisible host.
        // Listeners and observers stay attached — reattaching on
        // every show would directly inflate time-to-paint, which
        // is the headline metric for this window.
        window._kageFloatingHidden = true;
        this.extensionManager?.setWidgetVisibility(false);
        // A calendar popup left open would greet the next show.
        this.calendarPicker?.close({ restoreFocus: false, immediate: true });
        if (window._kageMascot) {
            try {
                window._kageMascot.pause();
            } catch (e) {
                console.warn('mascot.pause failed:', e);
            }
        }
        // Shut down mic and voice mode on hide
        if (this.speech) {
            this.speech.stopVoiceMode();
            this.speech.cancelSpeech();
        }
        // Clean up clipboard mode state on hide
        if (this._clipboardMode) {
            this._restoreOverlaysAfterClipboard();
            this._clipboardMode = false;
            this._clipboardEntries = null;
        }
        // Clear >cb prefix if it's still in the input
        if (this.elements.input.value.startsWith('>cb')) {
            this.elements.input.value = '';
            this.clearSuggestions();
        }
        // Hide response quick actions — if user didn't use them, they're stale
        const responseActions = document.getElementById('responseActionsContainer');
        if (responseActions) responseActions.style.display = 'none';

        // Pause animations to stop GPU compositing while hidden
        document.documentElement.classList.add('animations-paused');
    },
};
