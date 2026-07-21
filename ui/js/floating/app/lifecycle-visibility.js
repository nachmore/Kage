import {
    checkOnline,
    getConfig,
    onNetworkChange,
    renderUnifiedResults,
    unifiedSearch,
} from './dependencies.js';

export const LifecycleVisibilityMethods = {
    setupNetworkMonitor() {
        const bar = document.getElementById('offlineBar');
        const update = (online) => {
            if (bar) bar.style.display = online ? 'none' : 'flex';
            this.windowManager.resizeWindow();
        };
        // Do a real connectivity check on startup
        checkOnline().then((online) => update(online));
        onNetworkChange(update);
    },

    setupVisibilityTracking() {
        this._windowFocused = true; // assume focused at startup

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

        this.appWindow.listen('tauri://focus', async () => {
            this._windowFocused = true;
            document.documentElement.classList.remove('animations-paused');
            // First focus this process — show the post-update banner
            // if last_updated_version is still set. Running it here
            // (instead of from init()) means the banner waits until
            // the user can actually see the window: interactive
            // installs trigger this immediately via setup's auto-show,
            // idle installs trigger it the first time the user summons
            // the floating window manually. Either way the user
            // actually sees it.
            if (!this._updateBannerChecked) {
                this._updateBannerChecked = true;
                // The post-install auto-show races against other windows'
                // webviews painting for the first time (notably the
                // preloaded chat window's main.js init). Whichever paints
                // later steals focus and the blur handler would hide us —
                // taking the banner with it. Suppress the next ~2s of
                // blur-hides so the user actually sees the celebration.
                this.banner.checkForUpdateBanner().then((shown) => {
                    if (shown) this._suppressBlurHideUntil = Date.now() + 2000;
                });
            }
            // Resume work that was paused on hide. Mascot animation
            // intervals were ticking against an invisible window — a
            // small but constant CPU drag on every hidden minute.
            // Mirrors the existing permission-modal pause/resume.
            window._kageFloatingHidden = false;
            if (window._kageMascot) {
                try {
                    window._kageMascot.resume();
                } catch (e) {
                    console.warn('mascot.resume failed:', e);
                }
            }
            // Catch up widget renders that were skipped while hidden.
            // _renderWidget no-ops when _kageFloatingHidden is true, so a
            // widget mounted while the window was hidden (e.g. after an
            // extension hot-update) — or whose periodic ticks were all
            // skipped — would show nothing until its next interval fires.
            // Force a render now so the first visible paint is current.
            try {
                this.extensionManager?.renderAllWidgets();
            } catch (e) {
                console.warn('renderAllWidgets on show failed:', e);
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
            if (!permissionModal || permissionModal.style.display === 'none') {
                if (this.isWaitingForResponse) {
                    setTimeout(() => this.elements.input.focus(), 50);
                } else {
                    try {
                        const config = await getConfig(this.invoke);
                        if (config.ui?.preserve_last_response === false) {
                            setTimeout(() => this.resetUI(), 50);
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
        });

        this.appWindow.listen('tauri://blur', async () => {
            this._windowFocused = false;
            // Suppress blur-hide briefly after a post-update show. We
            // honour two signals:
            //
            //   `this._suppressBlurHideUntil` — set by checkForUpdateBanner
            //     once the banner is rendered. Catches blurs that happen
            //     after focus stabilises.
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
            // Don't hide if permission modal is open
            const permissionModal = document.getElementById('permissionModal');
            if (permissionModal && permissionModal.style.display !== 'none') {
                return;
            }
            // Don't hide if dragging the window
            if (this.windowManager.isDragging) {
                return;
            }
            // Don't hide if context menu is open
            const contextMenu = document.querySelector('.context-menu');
            if (contextMenu && contextMenu.style.display !== 'none') {
                return;
            }
            // Don't hide if context menu popup window is open
            if (window._contextMenuOpen) {
                return;
            }
            // Don't hide if we just stopped generating (prevents accidental hide on Esc)
            if (this._justStoppedGenerating) {
                return;
            }
            // Don't hide if computer control is active — user needs to track progress
            if (this.computerControlActive) {
                return;
            }
            // Don't hide if a focus-stealing MCP tool is running (e.g. folder picker dialog)
            if (this._noBlurTools.size > 0) {
                return;
            }
            // Don't hide if an automation plan is running
            if (this._automationPlanStarted) {
                return;
            }
            // Don't hide if file picker is open
            if (this._filePickerOpen) {
                return;
            }
            // Don't hide while an extension tool is being processed
            if (this._extensionToolExecuting || this._extensionToolCallHandled) {
                return;
            }
            await this.appWindow.hide();
            this.banner.dismiss();
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
        });

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
                this.extensionManager?.destroy?.();
            } catch (e) {
                console.warn('floating close-requested cleanup failed:', e);
            }
        });
    },
};
