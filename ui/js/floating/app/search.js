import {
    appendSendHint,
    escapeHtml,
    fetchClipboardHistory,
    filterClipboardHistory,
    getClipboardFilter,
    isClipboardTrigger,
    platformKeyLabel,
    renderClipboardHistory,
    renderUnifiedResults,
    searchDebounceMs,
    t,
    unifiedSearch,
} from './dependencies.js';
import { measureTextareaContentHeight } from './helpers.js';

export const SearchMethods = {
    /**
     * Request the search loading hint for generation `gen`, showing it only
     * if the search is still running after SEARCH_LOADING_DELAY_MS. Calling
     * again with new text just updates the label (or re-arms the timer if
     * the hint isn't visible yet). The `gen` guard means a hint armed by an
     * older search can never appear over a newer one.
     */
    _requestSearchLoading(gen, label) {
        if (gen !== this._searchGeneration) return;
        const existing = this.elements.appSuggestions.querySelector('.suggestions-loading');
        if (existing) {
            // Already visible — just keep the label current.
            existing.classList.remove('suggestions-loading-out');
            existing.textContent = label;
            return;
        }
        this._pendingSearchLoadingLabel = label;
        // The delay gate has already elapsed for this generation (the hint
        // was shown, then a later partial-render wiped the container via
        // innerHTML=''). Re-append immediately rather than re-delaying —
        // otherwise an actively-streaming search keeps resetting its own
        // 500ms gate and never shows progress.
        if (this._searchLoadingShownGen === gen) {
            // Re-append after a render-wipe — no entry animation, so an
            // actively-streaming search doesn't re-flash the fade-in on
            // every partial batch.
            this._appendSearchLoadingHint(label, false);
            this.windowManager.resizeWindow();
            return;
        }
        // First time for this generation: arm the delay timer so a search
        // that finishes within the gate never shows the hint at all.
        if (this._searchLoadingTimer) return; // timer already counting down
        this._searchLoadingTimer = setTimeout(() => {
            this._searchLoadingTimer = null;
            if (gen !== this._searchGeneration) return; // search moved on
            this._searchLoadingShownGen = gen;
            this._appendSearchLoadingHint(this._pendingSearchLoadingLabel || label, true);
            this.windowManager.resizeWindow();
        }, this.constructor.SEARCH_LOADING_DELAY_MS);
    },

    /**
     * Create + append the loading hint element. `animate` plays the
     * fade-in (true on first show; false when re-appending after a
     * render-wipe so streaming partials don't re-trigger the animation).
     */
    _appendSearchLoadingHint(label, animate) {
        const hint = document.createElement('div');
        hint.className = 'suggestions-hint suggestions-loading';
        if (!animate) hint.classList.add('suggestions-loading-no-in');
        hint.textContent = label;
        this.elements.appSuggestions.appendChild(hint);
    },

    /**
     * Hide the search loading hint. Cancels a pending (not-yet-shown) hint
     * outright — the fast-search path, which is why nothing flashes. If the
     * hint is already on screen, fade it out before removing so it animates
     * away instead of vanishing.
     */
    _hideSearchLoading() {
        if (this._searchLoadingTimer) {
            clearTimeout(this._searchLoadingTimer);
            this._searchLoadingTimer = null;
        }
        this._pendingSearchLoadingLabel = null;
        this._searchLoadingShownGen = -1;
        const existing = this.elements.appSuggestions.querySelector('.suggestions-loading');
        if (!existing) return;
        existing.classList.add('suggestions-loading-out');
        const el = existing;
        const done = () => el.remove();
        el.addEventListener('animationend', done, { once: true });
        // Fallback in case the animation is interrupted (element detached,
        // reduced-motion, etc.) so we never leak a stuck hint.
        setTimeout(done, 250);
    },

    async handleInputChange(_event) {
        const rawQuery = this.elements.input.value;
        const query = rawQuery.trim();

        // Resize the textarea and OS window in lockstep — see animateInputResize.
        // We measure scrollHeight via a clone so the live textarea never has
        // a 1-frame "single line with overflow" state.
        const input = this.elements.input;
        const oldH = input.offsetHeight;
        const newH = Math.min(measureTextareaContentHeight(input), 100);
        if (newH !== oldH) {
            this.windowManager.animateInputResize(input, oldH, newH);
        }

        // Reset tab cycle state when user types
        this._tabCycleActive = false;

        // Reset history browsing when user types new content
        if (this._historyIndex >= 0) {
            this._historyIndex = -1;
            this._historySaved = '';
        }

        // Dismiss banner as soon as user starts typing — it's served its purpose
        if (query.length > 0) this.banner.dismiss();

        // Update datetime visibility based on input state
        this.updateDatetimeVisibility();

        if (this.searchTimeout) {
            clearTimeout(this.searchTimeout);
        }

        if (query.length === 0) {
            this._hideSearchLoading();
            this.elements.appSuggestions.classList.remove('visible');
            this.currentMatches = [];
            this.selectedIndex = -1;
            this._noMatchSinceLen = 0;
            await this.windowManager.resizeWindow();
            return;
        }

        // Resize window to fit the growing input
        await this.windowManager.resizeWindow();

        // Debounced unified search — queries all sources in parallel.
        // File-shaped queries hit the disk, so they debounce harder; the
        // heuristic + timing live in the shared search engine.
        const debounceMs = searchDebounceMs(query);
        this._searchGeneration++;
        const gen = this._searchGeneration;
        this.searchTimeout = setTimeout(async () => {
            this.searchTimeout = null; // Mark debounce as fired
            // Check for clipboard history trigger
            if (isClipboardTrigger(query)) {
                const filter = getClipboardFilter(query);
                if (!this._clipboardMode) {
                    await this._enterClipboardMode(filter);
                } else {
                    await this._updateClipboardFilter(filter);
                }
                return;
            }
            if (this._clipboardMode) this._restoreOverlaysAfterClipboard();
            this._clipboardMode = false;

            const results = await unifiedSearch(
                rawQuery,
                this.invoke,
                this.shortcuts,
                async (partial, { done, pending }) => {
                    // Progressive rendering: show results as they arrive
                    if (gen !== this._searchGeneration) return; // stale
                    if (partial.length > 0) {
                        const { selectedIndex, matches } = await renderUnifiedResults(
                            partial,
                            this.elements.appSuggestions,
                            () => this.windowManager.resizeWindow(),
                            (r) => this._onResultClick(r)
                        );
                        // renderUnifiedResults awaits a sandbox round-trip; a
                        // newer flush may have superseded us while we were in
                        // it. Commit matches + selection together only if we're
                        // still current, so the two never disagree (which would
                        // make Enter fire the wrong row — see the function's
                        // doc comment).
                        if (gen !== this._searchGeneration) return;
                        this.currentMatches = matches;
                        this.selectedIndex = selectedIndex;
                    }
                    // Show/hide loading indicator with provider names.
                    // _requestSearchLoading delay-gates the hint so fast
                    // searches (which reach done before the gate fires)
                    // never flash it; _hideSearchLoading fades it out.
                    if (done) {
                        this._hideSearchLoading();
                    } else {
                        let label = t('floating.suggestions.loading_more');
                        if (pending && pending.length > 0) {
                            const shown = pending.slice(0, 2).join(', ');
                            label += ' (' + shown + (pending.length > 2 ? ', \u2026' : '') + ')';
                        }
                        label += '\u2026';
                        this._requestSearchLoading(gen, label);
                    }
                    this.windowManager.resizeWindow();
                }
            );
            // Discard stale results — a newer search was started while this one was in-flight
            if (gen !== this._searchGeneration) return;
            // All providers resolved — hide the loading hint (cancels it
            // outright if the delay gate never fired, fades it out if it did).
            this._hideSearchLoading();
            if (results.length > 0) {
                const { selectedIndex, matches } = await renderUnifiedResults(
                    results,
                    this.elements.appSuggestions,
                    () => this.windowManager.resizeWindow(),
                    (r) => this._onResultClick(r)
                );
                if (gen !== this._searchGeneration) return;
                this.currentMatches = matches;
                this.selectedIndex = selectedIndex;
                // Show send hint for non-instant results
                if (!['color', 'math', 'devtool'].includes(results[0].type)) {
                    appendSendHint(this.elements.appSuggestions);
                }
            } else {
                this.clearSuggestions();
            }
        }, debounceMs);
    },

    async clearSuggestions() {
        this._searchGeneration++; // discard in-flight searches
        if (this.searchTimeout) {
            clearTimeout(this.searchTimeout);
            this.searchTimeout = null;
        }
        this._hideSearchLoading();
        this.elements.appSuggestions.classList.remove('visible');
        this.currentMatches = [];
        this.selectedIndex = -1;
        if (this._clipboardMode) this._restoreOverlaysAfterClipboard();
        this._clipboardMode = false;
        await this.windowManager.resizeWindow();
    },

    /** Enter clipboard history mode — fetch and display history */
    async _enterClipboardMode(filter = '') {
        this._clipboardMode = true;
        this._hideOverlaysForClipboard();
        const entries = await fetchClipboardHistory(this.invoke);
        const filtered = filterClipboardHistory(entries, filter);
        this._clipboardEntries = entries; // Cache for filtering
        this.selectedIndex = renderClipboardHistory(
            filtered,
            this.elements.appSuggestions,
            this.currentMatches,
            () => this.windowManager.resizeWindow()
        );
        // After dropdown renders, ensure the window is on-screen
        await this.windowManager.resizeWindow();
    },

    /**
     * Hide banners, calendar overlay, and timer bars while clipboard mode is active.
     */
    _hideOverlaysForClipboard() {
        document.body.classList.add('clipboard-mode');
    },

    /**
     * Restore overlays that were hidden for clipboard mode.
     */
    _restoreOverlaysAfterClipboard() {
        document.body.classList.remove('clipboard-mode');
    },

    /** Update clipboard history filter (called on input change in clipboard mode) */
    async _updateClipboardFilter(filter) {
        if (!this._clipboardEntries) return;
        const filtered = filterClipboardHistory(this._clipboardEntries, filter);
        this.selectedIndex = renderClipboardHistory(
            filtered,
            this.elements.appSuggestions,
            this.currentMatches,
            () => this.windowManager.resizeWindow()
        );
    },

    _renderSystemCommandSuggestion(cmdId, cmdLabel, needsConfirm) {
        const container = this.elements.appSuggestions;
        container.innerHTML = '';
        container.scrollTop = 0;

        const item = document.createElement('div');
        item.className = 'app-suggestion-item selected';

        const canElevate = ['terminal', 'taskmanager', 'filemanager'].includes(cmdId);

        // cmdLabel is a fixed Rust-side string today, but escape anyway so
        // this render path stays safe if the label ever becomes dynamic.
        if (needsConfirm) {
            item.innerHTML = `
            <div class="app-icon">⚠️</div>
            <div class="app-info">
                <div class="app-name">${escapeHtml(cmdLabel)}</div>
                <div class="app-description">${t('floating.suggestions.system.confirm_select')}</div>
            </div>
        `;
        } else {
            item.innerHTML = `
            <div class="app-icon">${escapeHtml(cmdLabel.split(' ')[0])}</div>
            <div class="app-info">
                <div class="app-name">${escapeHtml(cmdLabel.substring(cmdLabel.indexOf(' ') + 1))}</div>
                <div class="app-description">${canElevate ? t('floating.suggestions.system.enter_admin_hint', { keys: platformKeyLabel('Ctrl+Shift+Enter') }) : t('floating.suggestions.system.enter_to_execute')}</div>
            </div>
        `;
        }

        item.addEventListener('click', () =>
            this._executeSystemCommand(cmdId, needsConfirm, false)
        );
        container.appendChild(item);
        container.classList.add('visible');
        this.windowManager.resizeWindow();
    },

    async _executeSystemCommand(cmdId, needsConfirm, elevated = false) {
        if (needsConfirm) {
            const container = this.elements.appSuggestions;
            container.innerHTML = '';
            const confirmItem = document.createElement('div');
            confirmItem.className = 'app-suggestion-item selected';
            confirmItem.innerHTML = `
            <div class="app-icon">⚠️</div>
            <div class="app-info">
                <div class="app-name">${elevated ? t('floating.suggestions.system.are_you_sure_admin') : t('floating.suggestions.system.are_you_sure')}</div>
                <div class="app-description">${t('floating.suggestions.system.confirm_hint')}</div>
            </div>
        `;
            confirmItem.addEventListener('click', async () => {
                try {
                    await this.invoke('execute_system_command', { commandId: cmdId, elevated });
                } catch (e) {
                    console.error('System command failed:', e);
                }
                this._clearInput();
            });
            container.appendChild(confirmItem);

            this.currentMatches = [{ type: 'system_confirm', cmdId, elevated }];
            this.selectedIndex = 0;
            this.windowManager.resizeWindow();
            return;
        }

        try {
            await this.invoke('execute_system_command', { commandId: cmdId, elevated });
        } catch (e) {
            console.error('System command failed:', e);
        }
        this._clearInput();
    },
};
