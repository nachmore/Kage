import {
    buildExecCtx,
    buildShortcutCommandFn,
    errLabel,
    executeShortcutCommand,
    getConfig,
    getSlotState,
    hideExtensionBar,
    matchShortcutFn,
    mountPromptForm,
    pauseResumeSlot,
    playTimerSound,
    renderMarkdown,
    sendAppNotification,
    setupTimerBarControls,
    showExtensionBar,
    startStopwatch,
    startTimer,
    stopSlot,
    t,
    updateTimerBar,
    WINDOW,
} from './dependencies.js';

export const CommandsMethods = {
    async loadShortcuts() {
        try {
            const config = await getConfig(this.invoke);
            this.shortcuts = config.shortcuts || [];
            console.log('Loaded shortcuts:', this.shortcuts);
        } catch (error) {
            console.error('Failed to load shortcuts:', error);
            this.shortcuts = [];
        }
    },

    _startTimerUI(durationMs) {
        startTimer(
            durationMs,
            (display, progress) => {
                updateTimerBar('timer', display, progress, true);
            },
            () => this._onTimerComplete()
        );
        setupTimerBarControls('timer', null, () => this.windowManager.resizeWindow());
        // Force resize after bar is in DOM
        setTimeout(() => this.windowManager.resizeWindow(), 60);
    },

    _startStopwatchUI() {
        startStopwatch((display) => {
            updateTimerBar('stopwatch', display, 0, true);
        });
        setupTimerBarControls('stopwatch', null, () => this.windowManager.resizeWindow());
        setTimeout(() => this.windowManager.resizeWindow(), 60);
    },

    async _onTimerComplete() {
        updateTimerBar('timer', '0:00', 1, false);

        let config = {};
        try {
            const fullConfig = await getConfig(this.invoke);
            config = fullConfig.extensions?.timer || {};
        } catch {}

        if (config.show_window_on_complete !== false) {
            try {
                const isVisible = await this.appWindow.isVisible();
                if (!isVisible) {
                    await this.appWindow.show();
                    await this.appWindow.setFocus();
                }
            } catch {}
        }

        if (config.notify_on_complete !== false) {
            try {
                await sendAppNotification(
                    this.invoke,
                    t('floating.timer.notification_title'),
                    '⏱️ ' + t('floating.timer.notification_body'),
                    WINDOW.FLOATING
                );
            } catch {}
        }

        if (config.sound_on_complete !== false) {
            try {
                playTimerSound(
                    config.sound_id || 'two-tone',
                    config.custom_sound_path || '',
                    config.sound_repeats || 3
                );
            } catch {}
        }

        // Auto-hide the timer bar after 5 seconds
        setTimeout(() => {
            const s = getSlotState('timer');
            if (!s.active) {
                const bar = document.getElementById('timerBar_timer');
                if (bar) {
                    bar.style.display = 'none';
                    bar.remove();
                }
                this.windowManager.resizeWindow();
            }
        }, 5000);
    },

    async _checkTerminatorMode() {
        try {
            const isTerminator = await this.invoke('is_terminator_mode');
            if (isTerminator) {
                // Clear dismissed flag when mode is (re-)enabled so the bar
                // reappears if the user toggled it off and back on.
                if (this._terminatorWasOff) {
                    sessionStorage.removeItem('terminator_bar_dismissed');
                }
                this._terminatorWasOff = false;
                if (!sessionStorage.getItem('terminator_bar_dismissed')) {
                    showExtensionBar({
                        id: 'terminator',
                        icon: '🤖',
                        text: t('floating.terminator.text'),
                        className: 'terminator-bar',
                        buttons: [
                            {
                                id: 'dismiss',
                                label: '✕',
                                title: t('floating.terminator.dismiss_title'),
                                onClick: () => {
                                    sessionStorage.setItem('terminator_bar_dismissed', '1');
                                    hideExtensionBar('terminator');
                                },
                            },
                        ],
                    });
                }
            } else {
                this._terminatorWasOff = true;
                hideExtensionBar('terminator');
            }
        } catch {
            /* ignore */
        }
    },

    matchShortcut(input) {
        return matchShortcutFn(input, this.shortcuts);
    },

    buildShortcutCommand(shortcut, args) {
        const useSelection = document.getElementById('useSelectionCheckbox')?.checked;
        const sel = useSelection && this.lastSelection ? this.lastSelection : '';
        return buildShortcutCommandFn(shortcut, args, sel);
    },

    /** Build execution context for the shared result executor. */
    _getExecCtx() {
        return buildExecCtx({
            invoke: this.invoke,
            appWindow: this.appWindow,
            extensionManager: this.extensionManager,
            input: this.elements.input,
            extra: {
                selectionText: document.getElementById('useSelectionCheckbox')?.checked
                    ? this.lastSelection || ''
                    : '',
                onPrompt: (text) => this.sendChatMessage(text),
                onDisplay: (text) => {
                    this.currentResponse = text;
                    renderMarkdown(text, this.elements.responseText);
                    this.elements.contentArea.classList.add('visible');
                    this.windowManager.resizeWindow();
                },
                // An extension result signalled that it mutated state its widget
                // renders (e.g. Spotify `sp like`). Repaint mounted widgets so the
                // floating bar reflects the change immediately rather than after
                // the widget's next poll. Delayed one beat because APIs like
                // Spotify's are eventually consistent — an immediate re-render can
                // still read the pre-change state (the widget's own onAction path
                // uses the same 250ms settle).
                onRefreshWidgets: () => {
                    setTimeout(() => {
                        try {
                            this.extensionManager?.renderAllWidgets();
                        } catch (e) {
                            console.warn('renderAllWidgets on refresh_widgets failed:', e);
                        }
                    }, 300);
                },
                onTimerStart: (ms) => this._startTimerUI(ms),
                onStopwatch: () => {
                    const sw = getSlotState('stopwatch');
                    if (sw.active && sw.running) {
                        pauseResumeSlot('stopwatch');
                    } else if (sw.active && !sw.running) {
                        stopSlot('stopwatch');
                        const bar = document.getElementById('timerBar_stopwatch');
                        if (bar) {
                            bar.remove();
                        }
                        this.windowManager.resizeWindow();
                    } else {
                        this._startStopwatchUI();
                    }
                },
                onPromptForm: (formCmd) => this._showPromptForm(formCmd),
            },
        });
    },

    /**
     * Render the missing-placeholders form in the response area. On
     * submit, re-build the shortcut command with the collected params
     * and re-enter the executor — single round trip back into the
     * normal `prompt` flow.
     */
    _showPromptForm(formCmd) {
        const responseEl = this.elements.responseText;
        if (!responseEl) return;
        // Hide the markdown response slot — we're using the same area
        // for the form. The contentArea visibility flag ensures the
        // window expands to fit the form.
        this.elements.contentArea.classList.add('visible');
        this.elements.contentArea.classList.remove('banner-only');

        mountPromptForm(responseEl, formCmd, {
            onSubmit: async (paramsByName) => {
                const useSelection = document.getElementById('useSelectionCheckbox')?.checked;
                const sel = useSelection && this.lastSelection ? this.lastSelection : '';
                const rebuilt = buildShortcutCommandFn(
                    formCmd.shortcut,
                    formCmd.args,
                    sel,
                    paramsByName
                );
                // Clear the form before executing — rebuilt is a regular
                // `prompt` command at this point so `onPrompt` will
                // populate the response area normally.
                responseEl.textContent = '';
                this.elements.contentArea.classList.remove('visible');
                this.windowManager.resizeWindow();
                await this.executeShortcut(rebuilt);
            },
            onCancel: () => {
                responseEl.textContent = '';
                this.elements.contentArea.classList.remove('visible');
                this.windowManager.resizeWindow();
                this.elements.input.focus();
            },
        });
        this.windowManager.resizeWindow();
    },

    async executeShortcut(command) {
        try {
            const result = await executeShortcutCommand(command, this._getExecCtx());
            if (result.action === 'hide') {
                this.resetUI();
                await this.appWindow.hide();
            }
            this._clearInput();
        } catch (error) {
            console.error('Failed to execute shortcut:', error);
            this.showError(errLabel(t('floating.error.failed_to_execute_shortcut'), error));
        }
    },

    /**
     * Delay before the search "loading more…" hint appears. Searches that
     * finish faster than this never show the hint at all, so the common
     * fast case doesn't flash a spinner in and out. ~half a second is long
     * enough that a human reads the absence as "instant".
     */
};
