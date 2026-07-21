import {
    EVT,
    loadFrecency,
    loadSlashCommands,
    onConfigChange,
    setAppIconInvoke,
    setupRtlDetection,
    t,
    trackEvent,
} from './dependencies.js';

export const LifecycleInitMethods = {
    async init() {
        const _t0 = performance.now();
        const _ts = (label) =>
            console.log(`⏱ [${(performance.now() - _t0).toFixed(0)}ms] init: ${label}`);

        this._setupSyncWiring();
        _ts('Synchronous setup done');

        await this._bootstrapState();
        _ts('Parallel IPC done (shortcuts + commands + frecency)');

        this.setupSpeech();
        this._registerAppEventListeners();

        setTimeout(() => this.elements.input.focus(), 100);

        this._runPostInitChecks();

        // Tell the backend the window is usable for input. Extension
        // loading below happens in the background and doesn't block.
        this.invoke('notify_frontend_ready').catch(() => {});
        _ts('notify_frontend_ready sent');
        setAppIconInvoke(this.invoke);

        this._initializeExtensionsInBackground(_ts);
    },

    _setupSyncWiring() {
        this.cacheElements();
        this.setupEventListeners();
        this.setupStreamingListeners();
        this.setupVisibilityTracking();
        this.setupNetworkMonitor();
        this.windowManager.setupDragging(this.elements.mascotContainer);
        this.windowManager.setupResizeHandle(document.getElementById('resizeHandle'));

        // Double-click ghost to open full chat window
        this.windowManager._onDoubleClick = async () => {
            try {
                await this.invoke('open_chat_window');
                await this.appWindow.hide();
            } catch (err) {
                console.error('Failed to open chat window:', err);
            }
        };
        this.windowManager.setupScaleChangeListener();
        this.windowManager.setupObserver();

        const inputContainer = this.elements.input?.closest('.input-container');
        setupRtlDetection(this.elements.input, inputContainer, this.elements.responseText);
    },

    async _bootstrapState() {
        await Promise.all([
            this.loadShortcuts(),
            loadSlashCommands(this.invoke),
            loadFrecency(this.invoke),
            this._adoptFloatingSession(),
        ]);
    },

    /**
     * Tauri-event listeners only. `tauri://focus` / `tauri://blur` and
     * the keyboard handlers live in `setupEventListeners` instead —
     * those need to be in place before any IPC has a chance to steal
     * focus.
     */
    _registerAppEventListeners() {
        // onConfigChange (not a raw config_updated listener) guarantees the
        // config cache is already invalidated when this runs, so
        // loadShortcuts() below re-fetches fresh data. See config-cache.js.
        onConfigChange(async () => {
            console.log('Config updated, reloading...');
            await this.loadShortcuts();
            await this.extensionManager.onConfigUpdate();
            await this.extensionManager.reload();
            this.updateSpeechButtonVisibility();
            this._updateToolbarVisibility();
            this._checkTerminatorMode();
            this._refreshOllamaStatusWidget();
            this.updateDatetimeVisibility();
        });

        this.listen(EVT.EXTENSIONS_CHANGED, async () => {
            console.log('Extensions changed, reloading...');
            await this.extensionManager.reload();
        });

        this.listen('slash_commands_available', async () => {
            console.log('Slash commands updated, reloading...');
            await loadSlashCommands(this.invoke);
        });

        this.listen(EVT.CLIPBOARD_HISTORY_MODE, async () => {
            console.log('Clipboard history mode activated via hotkey');
            // Clear any stale content
            this.elements.responseText.textContent = '';
            this.elements.contentArea.classList.remove('visible');
            this.elements.expandBtn.classList.remove('visible');
            this.elements.floatingStopBtn.style.display = 'none';
            this.currentResponse = '';
            this.banner.dismiss();
            // Enter clipboard mode
            this.elements.input.value = '>cb ';
            this._enterClipboardMode();
        });

        this.listen(EVT.VOICE_MODE, () => {
            console.log('Voice mode activated via hotkey');
            trackEvent('voice_input_used', { trigger: 'hotkey' });
            this.elements.responseText.textContent = '';
            this.elements.contentArea.classList.remove('visible');
            this.elements.expandBtn.classList.remove('visible');
            this.elements.floatingStopBtn.style.display = 'none';
            this.currentResponse = '';
            this.banner.dismiss();
            this.elements.input.value = '';
            this.clearSuggestions();
            if (this.speech) {
                this.speech.voiceMode = true;
                if (!this.speech.isListening) {
                    this.speech.start();
                }
            }
        });

        this.listen(EVT.SHOW_FLOATING_BANNER, (event) => {
            const { icon, text, action_label, action_type, action_data } = event.payload;
            this.banner.show(icon, text, action_label, action_type, action_data);
        });

        this.listen(EVT.UPDATE_AVAILABLE, (event) => {
            const version = event.payload;
            this.banner.show(
                '⬆️',
                t('floating.banner.update_available', { version }),
                t('floating.banner.action.install_now'),
                'update_install',
                ''
            );
        });
    },

    /**
     * The post-update "Kage has been updated!" banner is wired into
     * the first `tauri://focus` (in `setupEventListeners`), not fired
     * from here — the idle-install path leaves the floating window
     * hidden until the user summons it, and we only want the banner
     * to show up when the user is actually looking at the window.
     */
    _runPostInitChecks() {
        this.banner.checkForCrashBanner();
        this._checkTerminatorMode();
        this._refreshOllamaStatusWidget();

        const bottomSlot = document.getElementById('extWidgetSlotBottom');
        const statusSlot = document.getElementById('extWidgetSlotStatus');
        if (bottomSlot) this.extensionManager.setWidgetSlot('floating-bottom', bottomSlot);
        if (statusSlot) this.extensionManager.setWidgetSlot('floating-status', statusSlot);
    },

    /**
     * Deferred so cold-start time-to-paint isn't blocked on extension
     * loading; basic input/response works without extensions ready.
     * `_ts` is the optional timing logger threaded through from init().
     */
};
