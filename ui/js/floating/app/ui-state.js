import {
    drawContextRing,
    formatBytes,
    getConfig,
    hideExtensionBar,
    parseContextPercent,
    renderMarkdown,
    renderToolbarButtons,
    sanitizeExtensionHtml,
    showExtensionBar,
    SpeechController,
    t,
    updateExtensionBar,
} from './dependencies.js';

export const UiStateMethods = {
    resetUI() {
        this.elements.input.value = '';
        this.elements.input.style.height = 'auto';
        this.elements.appSuggestions.classList.remove('visible');
        this.currentMatches = [];
        this.selectedIndex = -1;
        this._noMatchSinceLen = 0;
        this.toolUsages = [];
        this._toolCallIds = new Set();
        this._sourceDomains = new Set();
        this.attachmentManager.clear();
        this.elements.contentArea.classList.remove('visible');
        this.elements.contentArea.classList.remove('banner-only');
        const responseActions = document.getElementById('responseActionsContainer');
        if (responseActions) {
            responseActions.innerHTML = '';
            responseActions.style.display = 'none';
        }
        const floatingActions = document.getElementById('floatingResponseActions');
        if (floatingActions) floatingActions.style.display = 'none';
        this.stopThinking();
        this.elements.expandBtn.classList.remove('visible');
        this.currentResponse = '';
        this.toolSources = [];
        this.elements.floatingStopBtn.style.display = 'none';
        const sourcesEl = document.getElementById('toolSources');
        if (sourcesEl) sourcesEl.remove();
        const compactEl = document.getElementById('toolSourcesCompact');
        if (compactEl) compactEl.remove();
        // Exit voice conversation mode on reset
        if (this.speech?.voiceMode) {
            this.speech.stopVoiceMode();
        }
        this.elements.input.focus();
        // Re-show datetime when input is cleared
        this.updateDatetimeVisibility();
    },

    startThinking() {
        this.elements.mascotContainer.classList.add('thinking');
        this.elements.loadingDots.classList.add('visible');
        this._startElapsedTimer();
        // A response is about to arrive — drop banner-only mode so the
        // content-area's overflow:auto comes back for scrollable replies.
        // Cheap no-op if the class wasn't set.
        this.elements.contentArea?.classList.remove('banner-only');
        // Switch mascot to jumping animation at larger size
        if (window._kageMascot) {
            import('../../shared/mascot-animations.js').then((m) =>
                window._kageMascot.setActive(m.ANIMATIONS.jumping, 60)
            );
        }
        // Show inline stop button in input area, hide datetime
        this.updateDatetimeVisibility();
        this.elements.floatingStopBtn.style.display = '';
    },

    stopThinking() {
        this.elements.mascotContainer.classList.remove('thinking');
        this.elements.loadingDots.classList.remove('visible');
        this._stopElapsedTimer();
        // Return mascot to idle with a wave transition
        if (window._kageMascot) window._kageMascot.setIdle(true);
    },

    /**
     * Show a running elapsed timer beside the loading dots while the agent
     * works. There is no wall-clock cap on a request any more (the backend
     * waits as long as the agent keeps streaming), so the timer is the user's
     * signal that a long request is still alive — and the Stop button remains
     * the escape hatch. Ticks once a second; the label stays hidden for the
     * first few seconds so quick replies don't flash a "0s".
     */
    _startElapsedTimer() {
        this._stopElapsedTimer();
        const el = this.elements.loadingElapsed;
        if (!el) return;
        const started = performance.now();
        el.textContent = '';
        const tick = () => {
            const secs = Math.floor((performance.now() - started) / 1000);
            // Don't distract on fast turns; only surface once it's notably slow.
            if (secs < 3) {
                el.textContent = '';
                return;
            }
            const time =
                secs < 60
                    ? `${secs}s`
                    : `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`;
            el.textContent = t('floating.elapsed', { time });
        };
        tick();
        this._elapsedTimer = setInterval(tick, 1000);
    },

    _stopElapsedTimer() {
        if (this._elapsedTimer) {
            clearInterval(this._elapsedTimer);
            this._elapsedTimer = null;
        }
        if (this.elements.loadingElapsed) this.elements.loadingElapsed.textContent = '';
    },

    stopGenerating() {
        if (!this.isWaitingForResponse) return;

        // If an automation plan is running, stop it gracefully
        if (this._automationPlanStarted) {
            this.automationPlanController.stopGracefully();
            this.computerControlActive = false;
        }

        this.isWaitingForResponse = false;
        this._justStoppedGenerating = true;
        setTimeout(() => {
            this._justStoppedGenerating = false;
        }, 300);
        this.stopThinking();
        this.elements.floatingStopBtn.style.display = 'none';
        this.updateDatetimeVisibility();
        const indicator = this.elements.responseText.querySelector('.streaming-indicator');
        if (indicator) indicator.remove();

        // Don't overwrite the plan UI with markdown
        if (!this._automationPlan) {
            if (this.currentResponse) {
                renderMarkdown(this.currentResponse, this.elements.responseText);
                // The text above is whatever streamed before the cancel —
                // it can read as a complete answer (the agent may even have
                // finished generating but not yet ended the turn). Flag it
                // so a cancelled response is never mistaken for the full one.
                const note = document.createElement('div');
                note.className = 'response-partial-note';
                note.textContent = t('floating.response.partial_note');
                this.elements.responseText.appendChild(note);
            } else {
                this.elements.contentArea.classList.remove('visible');
                this.elements.expandBtn.classList.remove('visible');
            }
        }

        this.windowManager.resizeWindow();
        this.invoke('cancel_generation', { sessionId: this.floatingSessionId }).catch((e) =>
            console.log('Cancel:', e)
        );
    },

    // --- Speech ---

    async updateSpeechButtonVisibility() {
        await this.speech.updateVisibility();
    },

    /**
     * App Modes (P1.4 from product_suggestions.md). Looks up the
     * foreground app's process name and asks the backend whether any
     * configured rule matches. On a hit, paints a small chip above the
     * input ("🎯 VS Code mode") and stashes the steering payload on
     * `this._appModeMatch` so `sendChatMessage` can splice it into the
     * outgoing prompt next to `<_kage_ctx>`.
     *
     * The chip is also a one-shot dismiss control — clicking it nulls
     * `_appModeMatch` and hides the chip without touching config, so
     * the rule still fires next summon. The state lives outside the
     * config update flow so the click is instantaneous (no IPC).
     */
    async _refreshAppModeChip() {
        const chip = document.getElementById('appModeChip');
        const labelEl = document.getElementById('appModeChipLabel');
        if (!chip || !labelEl) return;

        // Always start the chip hidden — only re-show on a confirmed
        // match. The previous summon's match should never linger.
        chip.style.display = 'none';
        this._appModeMatch = null;

        let exe = '';
        try {
            const sw = await this.invoke('get_source_window');
            exe = sw?.processName || '';
        } catch {
            return;
        }
        if (!exe) return;

        try {
            const matched = await this.invoke('match_context_rule', { executable: exe });
            if (!matched) return;
            this._appModeMatch = matched;
            labelEl.textContent = matched.friendly_name;
            chip.style.display = 'inline-flex';
            // Wire dismiss once — first event we see clears state.
            // We swap the listener each refresh because the matched
            // rule may have changed between summons.
            chip.onclick = () => {
                this._appModeMatch = null;
                chip.style.display = 'none';
            };
        } catch (e) {
            console.log('match_context_rule failed:', e);
        }
    },

    /**
     * Optional Ollama status widget. Off by default, configured per
     * Ollama connection in the Agents wizard. When the active
     * connection is Ollama-shaped and `show_status_widget=true`,
     * render a small "🦙 <model> · ready" chip via the existing
     * extension-bar slot and refresh it every 30s while the floating
     * window is being used. Anything else hides the widget.
     *
     * Reuses the extension-bar API rather than rolling its own DOM
     * because that path already handles window-resize accounting,
     * blur-without-focus-steal, and theme inversion. The "extension"
     * label is a misnomer — it's just a styled bar slot — but
     * adopting an existing pattern beats inventing a new chip system
     * for a six-row feature.
     */
    async _refreshOllamaStatusWidget() {
        let cfg;
        try {
            cfg = await getConfig(this.invoke);
        } catch {
            return;
        }
        const active = cfg.acp?.connections?.find((c) => c.id === cfg.acp?.active_connection_id);
        const settings = active?.preset_id === 'ollama' ? active.ollama_settings : null;
        const enabled = !!settings?.show_status_widget && !!settings?.base_url;

        if (!enabled) {
            this._stopOllamaStatusPoll();
            hideExtensionBar('ollama-status');
            return;
        }

        // Show a placeholder immediately so the user sees the widget
        // appear even before the first probe completes. The poll
        // below replaces "checking..." with the resolved state.
        this._ollamaStatus = {
            baseUrl: settings.base_url,
            model: settings.model || '',
            // Tracking the same params the poll reads from. If the
            // user changes model in Settings while the floating
            // window is open, the config_updated listener calls us
            // again and we re-seed.
        };
        showExtensionBar({
            id: 'ollama-status',
            icon: '🦙',
            text: settings.model ? `${settings.model} · checking…` : 'Ollama · checking…',
            className: 'ollama-status-bar',
        });
        this._startOllamaStatusPoll();
        // Kick one immediate refresh too — saves the user a 30s wait
        // when the widget first appears.
        this._pollOllamaStatusOnce().catch(() => {});
    },

    _startOllamaStatusPoll() {
        this._stopOllamaStatusPoll();
        // 30s is the same cadence the existing extension status widgets
        // use; balances "is it still up" reassurance with avoiding LAN
        // chatter for users on metered or finicky networks.
        this._ollamaStatusInterval = setInterval(
            () => this._pollOllamaStatusOnce().catch(() => {}),
            30 * 1000
        );
    },

    _stopOllamaStatusPoll() {
        if (this._ollamaStatusInterval) {
            clearInterval(this._ollamaStatusInterval);
            this._ollamaStatusInterval = null;
        }
    },

    async _pollOllamaStatusOnce() {
        const s = this._ollamaStatus;
        if (!s) return;
        // Don't hit the network (possibly LAN) while the launcher is
        // hidden — same gate the mascot animation uses. The interval
        // keeps ticking; the first poll after re-show refreshes.
        if (window._kageFloatingHidden) return;
        let probe = null;
        try {
            probe = await this.invoke('ollama_probe', { baseUrl: s.baseUrl });
        } catch (e) {
            updateExtensionBar('ollama-status', {
                text: `${s.model || 'Ollama'} · offline`,
            });
            return;
        }
        if (probe?.status !== 'Reachable') {
            updateExtensionBar('ollama-status', {
                text: `${s.model || 'Ollama'} · offline`,
            });
            return;
        }
        // Reachable — hit /api/tags too, since it's the only way to
        // surface the resident size. allSettled-style: if list_models
        // fails for a transient reason, leave the size off.
        let sizeStr = '';
        try {
            const models = await this.invoke('ollama_list_models', {
                baseUrl: s.baseUrl,
            });
            const match = (Array.isArray(models) ? models : []).find((m) => m?.name === s.model);
            if (match?.size) sizeStr = ` · ${formatBytes(match.size)}`;
        } catch {}
        const versionStr = probe.version ? ` (Ollama ${probe.version})` : '';
        updateExtensionBar('ollama-status', {
            text: `${s.model || 'Ollama'}${sizeStr} · ready${versionStr}`,
        });
    },

    async _updateToolbarVisibility() {
        try {
            const config = await getConfig(this.invoke);
            const show = config.ui?.show_floating_toolbar === true;
            if (this.elements.floatingToolbar) {
                this.elements.floatingToolbar.style.display = show ? 'flex' : 'none';
                if (show) {
                    this._renderExtensionToolbarButtons();
                    // Throttle expensive IPC calls — models load once per session, context refreshes every 5min
                    const now = Date.now();
                    if (!this._modelsLoaded) {
                        this._modelsLoaded = true;
                        this._loadModels();
                    }
                    if (!this._lastContextRefresh || now - this._lastContextRefresh > 300000) {
                        this._lastContextRefresh = now;
                        this._refreshContextUsage();
                    }
                }
            }
            this.windowManager.resizeWindow();
        } catch (e) {
            console.warn('[Floating] Failed to update toolbar visibility:', e);
        }
    },

    _renderExtensionToolbarButtons() {
        const container = this.elements.floatingToolbarExt;
        if (!container || !this.extensionManager) return;
        const buttons = this.extensionManager.getToolbarButtons();
        console.log('[Floating] Rendering extension toolbar buttons:', buttons.length);
        renderToolbarButtons({
            container,
            buttons,
            buttonClass: 'floating-toolbar-btn',
            sanitizeIcon: (iconStr) => sanitizeExtensionHtml(iconStr, 'icon'),
            buildContext: () => ({
                input: this.elements.input?.value || '',
                messages: [],
            }),
            onHostEffect: (host) => this._runToolbarHostEffect(host),
        });
    },

    /**
     * Apply a host effect returned from an extension toolbar click.
     * Mirrors the chat window's implementation with the floating input.
     */
    _runToolbarHostEffect(host) {
        if (!host || typeof host !== 'object') return;
        switch (host.type) {
            case 'set_chat_input': {
                const v = String(host.value ?? '');
                if (this.elements.input) {
                    this.elements.input.value = v;
                    this.elements.input.focus();
                    this.elements.input.dispatchEvent(new Event('input'));
                }
                break;
            }
            case 'append_chat_input': {
                const v = String(host.value ?? '');
                if (this.elements.input) {
                    const cur = this.elements.input.value || '';
                    const sep = cur && !cur.endsWith(' ') ? ' ' : '';
                    this.elements.input.value = cur + sep + v;
                    this.elements.input.focus();
                    this.elements.input.dispatchEvent(new Event('input'));
                }
                break;
            }
            case 'show_ephemeral_message':
                // Floating window has no messages area; log and drop so
                // extensions can tell the difference between unsupported
                // contexts and silent failure.
                console.info(
                    '[Floating] Ignoring show_ephemeral_message host effect — only supported in chat window'
                );
                break;
            default:
                console.warn('[Floating] Unknown toolbar host effect:', host.type);
                break;
        }
    },

    // --- Context % and Model Selector ---

    async _refreshContextUsage() {
        if (!this.floatingSessionId) return;
        try {
            const result = await this.invoke('execute_slash_command', {
                sessionId: this.floatingSessionId,
                command: 'context',
                args: {},
            });
            const pct = parseContextPercent(result);
            if (pct !== null) {
                if (this.elements.floatingContextPercent)
                    this.elements.floatingContextPercent.textContent = pct + '%';
                if (this.elements.floatingContextIndicator)
                    this.elements.floatingContextIndicator.title = pct + '% context used';
                drawContextRing(this.elements.floatingContextRing, pct);
            }
        } catch {}
    },

    async _loadModels() {
        try {
            const models = await this.invoke('get_available_models');
            this._availableModels = models || [];
            if (this._availableModels.length > 0) {
                const current = this._availableModels[0];
                if (this.elements.floatingModelName)
                    this.elements.floatingModelName.textContent =
                        current.name || current.modelId || '?';
            }
        } catch {}
    },

    setupSpeech() {
        this.speech = new SpeechController({
            invoke: this.invoke,
            elements: {
                input: this.elements.input,
                speechBtn: this.elements.speechBtn,
                speechWave: this.elements.speechWave,
            },
            onSend: (text) => this.sendChatMessage(text),
            onVisibilityUpdate: () => this.updateDatetimeVisibility(),
            barContainer: document.querySelector('.input-container'),
        });
        this.speech.setup();
    },

    // Convenience accessors used by Escape handler and sendChatMessage
    get isSpeechListening() {
        return this.speech?.isListening ?? false;
    },
    get _usedSpeechForLastMessage() {
        return this.speech?.usedSpeechForLastMessage ?? false;
    },
    set _usedSpeechForLastMessage(v) {
        if (this.speech) this.speech.usedSpeechForLastMessage = v;
    },

    /**
     * Resolve floating's pinned session id.
     *
     * Behaviour depends on the user's `start_session_on_launch` setting:
     *
     * - **Enabled (default).** Setup is preloading a session in the
     *   background; wait for the `session_pinned` event with NO
     *   timeout (kiro-cli cold-start can take 10s+). `session_pin_failed`
     *   covers the deadlock: setup emits it on connect/load/create
     *   failures, we react by creating our own session. Either way,
     *   we don't hang the user forever.
     *
     * - **Disabled.** Don't wait — create our own session immediately.
     *
     * In both cases the listener is registered BEFORE the synchronous
     * `get_window_session` check, so we can't miss a `session_pinned`
     * that fires between the two (which would otherwise leave us
     * permanently stalled).
     *
     * The frontend "spinning up agent…" UX is driven by
     * `this.bootstrappingSession` — true while we're in the wait phase
     * so onChunkAppended / typing handlers can show the placeholder.
     */
};
