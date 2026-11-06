/**
 * Launcher result menu — what happens after the user picks an action from
 * a result's right-click menu (floating/context-menu.js opens the menu).
 *
 * Uninstall always asks first, inline in the suggestions area (sized with
 * the window like any other row, so it can't be clipped), naming the app.
 * While that confirmation — or the action it starts — is pending, the
 * floating window doesn't hide on blur (lifecycle-visibility.js).
 */

import { errMessage, t, updateSelection } from './dependencies.js';

/** How long a "Copied" style confirmation stays up. */
const FLASH_MS = 2500;

export const ResultMenuMethods = {
    /** Point the selection at `result`'s row (right-clicking selects it). */
    selectResultRow(result) {
        const idx = this.currentMatches.indexOf(result);
        if (idx < 0) return;
        this.selectedIndex = idx;
        updateSelection(this.elements.appSuggestions, idx);
    },

    /**
     * Run `action` (an id from `launcher_result_actions`) on `target`
     * (`{ kind, target }`). `info` is what the menu was built from.
     */
    async runResultAction(target, info, action) {
        if (action === 'uninstall') {
            const confirmed = await this._confirmUninstall(info);
            if (!confirmed) return;
        }
        this._resultActionBusy = true;
        try {
            let res;
            try {
                res = await this.invoke('launcher_result_action', {
                    kind: target.kind,
                    target: target.target,
                    action,
                    confirmed: action === 'uninstall',
                });
            } catch (e) {
                this._showResultNotice(errMessage(e), { tone: 'error' });
                return;
            }
            await this._applyResultOutcome(info, action, res);
        } finally {
            this._resultActionBusy = false;
        }
    },

    async _applyResultOutcome(info, action, res) {
        const name = res?.name || info?.name || '';
        const windows = info?.platform === 'windows';
        switch (res?.outcome) {
            case 'done':
            case 'uninstaller_launched':
            case 'settings_opened':
                // Opened, revealed or handed off to the OS — get out of the
                // way, exactly like launching an app does.
                this._clearInput();
                await this.appWindow.hide();
                break;
            case 'copied': {
                const key =
                    action === 'copy_path'
                        ? 'result_menu.feedback.copied_path'
                        : action === 'cut_file'
                          ? 'result_menu.feedback.cut'
                          : 'result_menu.feedback.copied';
                this._flashResultNotice(t(key, { name }));
                break;
            }
            case 'moved_to_trash':
                this._flashResultNotice(
                    t(windows ? 'result_menu.feedback.recycled' : 'result_menu.feedback.trashed', {
                        name,
                    })
                );
                // The item is gone: search again so its row disappears.
                this._refreshResultsSoon();
                break;
            case 'revealed_for_manual_removal':
                // Finder just came forward; the explanation has to stay
                // readable instead of the launcher hiding on that blur.
                this._suppressBlurHideUntil = Date.now() + 1500;
                this._showResultNotice(t('result_menu.feedback.manual_removal', { name }));
                break;
            default:
                // 'cancelled' (UAC declined): nothing happened, say nothing.
                break;
        }
    },

    _refreshResultsSoon() {
        setTimeout(() => {
            if (this.elements.input.value.trim()) this.handleInputChange();
        }, 250);
    },

    /** Remove any notice / confirmation this module put on screen. */
    _clearResultNotice() {
        clearTimeout(this._resultNoticeTimer);
        this._resultNoticeTimer = null;
        for (const el of this.elements.appSuggestions.querySelectorAll('.result-action-notice')) {
            el.remove();
        }
    },

    /** A one-line status under the results; stays until the next search. */
    _showResultNotice(text, { tone = 'info' } = {}) {
        this._clearResultNotice();
        const el = document.createElement('div');
        el.className = `result-action-notice ${tone === 'error' ? 'error' : ''}`.trim();
        el.setAttribute('role', tone === 'error' ? 'alert' : 'status');
        el.textContent = text;
        const container = this.elements.appSuggestions;
        container.appendChild(el);
        container.classList.add('visible');
        this.windowManager.resizeWindow();
        return el;
    },

    /** A notice that goes away by itself. */
    _flashResultNotice(text) {
        const el = this._showResultNotice(text);
        this._resultNoticeTimer = setTimeout(() => {
            el.remove();
            this._resultNoticeTimer = null;
            if (!this.elements.appSuggestions.children.length) {
                this.elements.appSuggestions.classList.remove('visible');
            }
            this.windowManager.resizeWindow();
        }, FLASH_MS);
    },

    /**
     * Ask, inline, whether to uninstall `info.name`. Resolves true only on
     * an explicit click / Enter on the Uninstall button; Cancel, Esc and
     * typing a new query all resolve false. Cancel has the initial focus.
     */
    _confirmUninstall(info) {
        this._cancelResultConfirm();
        const name = info?.name || '';
        const container = this.elements.appSuggestions;
        this._clearResultNotice();
        container.textContent = '';
        container.scrollTop = 0;
        this.currentMatches = [];
        this.selectedIndex = -1;

        const panel = document.createElement('div');
        panel.className = 'result-confirm';
        panel.setAttribute('role', 'alertdialog');
        panel.setAttribute('aria-modal', 'true');
        const title = document.createElement('div');
        title.className = 'result-confirm-title';
        title.id = 'resultConfirmTitle';
        title.textContent = t('result_menu.confirm.title', { name });
        const body = document.createElement('div');
        body.className = 'result-confirm-body';
        body.id = 'resultConfirmBody';
        body.textContent = t(
            info?.platform === 'windows'
                ? 'result_menu.confirm.body_windows'
                : 'result_menu.confirm.body_macos',
            { name }
        );
        panel.setAttribute('aria-labelledby', title.id);
        panel.setAttribute('aria-describedby', body.id);
        const actions = document.createElement('div');
        actions.className = 'result-confirm-actions';
        const cancel = document.createElement('button');
        cancel.type = 'button';
        cancel.className = 'result-confirm-btn';
        cancel.textContent = t('result_menu.confirm.cancel');
        const confirm = document.createElement('button');
        confirm.type = 'button';
        confirm.className = 'result-confirm-btn danger';
        confirm.textContent = t('result_menu.confirm.uninstall');
        actions.append(cancel, confirm);
        panel.append(title, body, actions);
        container.appendChild(panel);
        container.classList.add('visible');

        return new Promise((resolve) => {
            const finish = (ok) => {
                if (this._resultConfirm !== state) return;
                this._resultConfirm = null;
                panel.remove();
                resolve(ok);
                if (!ok) this._refreshResultsSoon();
            };
            const state = {
                cancelButton: cancel,
                cancel: () => finish(false),
            };
            this._resultConfirm = state;
            cancel.addEventListener('click', () => finish(false));
            confirm.addEventListener('click', () => finish(true));
            panel.addEventListener('keydown', (e) => {
                if (e.key === 'Escape') {
                    e.preventDefault();
                    e.stopPropagation();
                    finish(false);
                } else if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
                    e.preventDefault();
                    (document.activeElement === cancel ? confirm : cancel).focus();
                }
            });
            this.windowManager.resizeWindow();
            // The menu window just closed; make sure the launcher has the
            // keyboard so Esc / Tab / Enter reach the buttons.
            this.invoke('present_overlay_window')
                .catch(() => {})
                .finally(() => setTimeout(() => cancel.focus(), 0));
        });
    },

    /** Dismiss a pending uninstall confirmation (resolves it as "no"). */
    _cancelResultConfirm() {
        this._resultConfirm?.cancel();
    },
};
