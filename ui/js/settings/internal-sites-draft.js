import { t } from '../shared/i18n.js';
import { errMessage } from '../shared/error-message.js';
import { escapeAttr, escapeHtml } from '../shared/tool-utils.js';
import { EVT } from '../shared/events.js';

/**
 * Modal for an Internal Sites agent draft (rules or proxy script).
 *
 * Greys out the settings window and walks through:
 *   1. guidance — optional free text that steers the agent;
 *   2. running  — live view of the throwaway agent session: its tool calls
 *      and streamed text, any permission prompt it raises (answered here —
 *      the backend routes the session's prompts to the settings window),
 *      and a Cancel button;
 *   3. error    — the failure, with Close.
 *
 * The backend announces the session id via `internal_sites_draft_session`
 * (matched on our request id); everything else is filtered by that id.
 *
 * `open()` resolves to `{ status: 'done', result }`, `{ status: 'cancelled' }`
 * or `{ status: 'failed', error }`.
 */

const DRAFT_SESSION_EVENT = 'internal_sites_draft_session';
const MAX_TEXT_TAIL = 1500;
const MAX_LOG_ENTRIES = 50;

function newRequestId() {
    return globalThis.crypto?.randomUUID?.() || `draft-${Date.now()}-${Math.random()}`;
}

export class DraftDialog {
    /**
     * @param {object} opts
     * @param {string} opts.title
     * @param {(args: {guidance: string|null, requestId: string}) => Promise<any>} opts.run
     */
    constructor({ title, run }) {
        this._title = title;
        this._run = run;
        this._phase = 'guidance';
        this._guidance = '';
        this._requestId = null;
        this._sessionId = null;
        this._tools = new Map(); // toolCallId → { title, status }
        this._text = '';
        this._permission = null; // ACP request_permission notification
        this._error = null;
        this._startedAt = 0;
        this._unlisten = [];
        this._timer = null;
        this._root = null;
        this._resolve = null;
    }

    open() {
        this._root = document.createElement('div');
        this._root.className = 'isd-overlay';
        this._root.setAttribute('role', 'dialog');
        this._root.setAttribute('aria-modal', 'true');
        this._root.addEventListener('click', (e) => this._onClick(e));
        this._root.addEventListener('input', (e) => {
            if (e.target.matches?.('.isd-guidance')) this._guidance = e.target.value;
        });
        this._root.addEventListener('keydown', (e) => {
            if (e.key === 'Escape') {
                e.preventDefault();
                this._cancel();
            }
        });
        document.body.appendChild(this._root);
        this._render();
        this._root.querySelector('.isd-guidance')?.focus();
        return new Promise((resolve) => {
            this._resolve = resolve;
        });
    }

    // --- flow ---------------------------------------------------------------

    _onClick(e) {
        const action = e.target.closest?.('[data-isd-action]')?.dataset.isdAction;
        if (!action) return;
        e.preventDefault();
        if (action === 'start') this._start();
        else if (action === 'cancel') this._cancel();
        else if (action === 'close') this._finish({ status: 'failed', error: this._error });
        else if (action === 'permission') this._answerPermission(e.target.closest('[data-option]'));
    }

    async _start() {
        if (this._phase !== 'guidance') return;
        this._phase = 'running';
        this._requestId = newRequestId();
        this._startedAt = Date.now();
        await this._listen();
        this._render();
        this._timer = setInterval(() => this._renderLive(), 1000);

        const requestId = this._requestId;
        const guidance = this._guidance.trim() || null;
        try {
            const result = await this._run({ guidance, requestId });
            if (this._requestId !== requestId) return; // cancelled meanwhile
            this._finish({ status: 'done', result });
        } catch (e) {
            if (this._requestId !== requestId) return;
            this._stopLive();
            this._phase = 'error';
            this._error = e;
            this._render();
        }
    }

    _cancel() {
        const invoke = window.__TAURI__?.core?.invoke;
        const sessionId = this._sessionId;
        if (this._phase === 'running' && sessionId && invoke) {
            // Release a pending permission prompt first — an agent blocked
            // on it may not honour the cancel until it's answered.
            invoke('dismiss_pending_permission', { sessionId }).catch(() => {});
            invoke('cancel_generation', { sessionId }).catch(() => {});
        }
        // Drop the in-flight result: the backend may take a moment to
        // wind the session down, but the user is done with it.
        this._requestId = null;
        this._finish({ status: 'cancelled' });
    }

    _finish(outcome) {
        this._stopLive();
        for (const off of this._unlisten) {
            try {
                off();
            } catch {}
        }
        this._unlisten = [];
        this._root?.remove();
        this._root = null;
        const resolve = this._resolve;
        this._resolve = null;
        resolve?.(outcome);
    }

    _stopLive() {
        if (this._timer) clearInterval(this._timer);
        this._timer = null;
    }

    async _listen() {
        const listen = window.__TAURI__?.event?.listen;
        if (!listen) return;
        const mine = (sid) => !!sid && sid === this._sessionId;
        const subs = [
            listen(DRAFT_SESSION_EVENT, (e) => {
                if (e.payload?.requestId === this._requestId) {
                    this._sessionId = e.payload.sessionId;
                }
            }),
            listen('message_chunk', (e) => {
                if (!mine(e.payload?.sessionId)) return;
                this._text = (this._text + (e.payload.text || '')).slice(-MAX_TEXT_TAIL * 2);
                this._renderLive();
            }),
            listen('tool_call_update', (e) => {
                const params = e.payload?.params;
                if (!mine(params?.sessionId)) return;
                const u = params.update || {};
                if (!u.toolCallId) return;
                const prev = this._tools.get(u.toolCallId) || {};
                this._tools.set(u.toolCallId, {
                    title:
                        u.title || prev.title || u.kind || t('settings.internal_sites.draft.tool'),
                    status: u.status || prev.status || 'pending',
                });
                while (this._tools.size > MAX_LOG_ENTRIES) {
                    this._tools.delete(this._tools.keys().next().value);
                }
                this._renderLive();
            }),
            listen('permission_request', (e) => {
                const n = e.payload?.notification;
                if (!mine(n?.params?.sessionId)) return;
                this._permission = { notification: n, toolName: e.payload.toolName };
                this._renderLive();
            }),
            listen(EVT.PERMISSION_DISMISSED, (e) => {
                const id = e.payload?.requestId;
                if (!this._permission) return;
                if (
                    id == null ||
                    JSON.stringify(id) === JSON.stringify(this._permission.notification.id)
                ) {
                    this._permission = null;
                    this._renderLive();
                }
            }),
        ];
        this._unlisten = await Promise.all(subs);
    }

    _answerPermission(btn) {
        const p = this._permission;
        const optionId = btn?.dataset.option;
        if (!p || !optionId) return;
        const n = p.notification;
        this._permission = null;
        this._renderLive();
        window.__TAURI__.core
            .invoke('send_permission_response', {
                sessionId: n.params?.sessionId || null,
                requestId: n.id,
                optionId,
                toolTitle: n.params?.toolCall?.title || p.toolName || '',
            })
            .catch((e) => console.warn('[internal-sites] permission response failed:', e));
    }

    // --- rendering ----------------------------------------------------------

    _render() {
        if (!this._root) return;
        const btn = (action, key, primary) =>
            `<button class="setting-button" data-isd-action="${action}"${primary ? '' : ' style="background:transparent;color:var(--kage-text-secondary);border:1px solid var(--kage-border);"'}>${escapeHtml(t(`settings.internal_sites.draft.${key}`))}</button>`;
        let body;
        if (this._phase === 'guidance') {
            body = `
                <label class="setting-label" for="isdGuidance">${escapeHtml(t('settings.internal_sites.draft.guidance_label'))}</label>
                <div class="setting-description">${escapeHtml(t('settings.internal_sites.draft.guidance_description'))}</div>
                <textarea id="isdGuidance" class="setting-input isd-guidance" rows="4" placeholder="${escapeAttr(t('settings.internal_sites.draft.guidance_placeholder'))}">${escapeHtml(this._guidance)}</textarea>
                <div class="isd-actions">${btn('cancel', 'cancel')}${btn('start', 'start', true)}</div>`;
        } else if (this._phase === 'running') {
            body = `
                <div class="setting-description">${escapeHtml(t('settings.internal_sites.drafting'))}</div>
                <div class="isd-live"></div>
                <div class="isd-actions">${btn('cancel', 'cancel')}</div>`;
        } else {
            body = `
                <div class="setting-description internal-site-status error" role="alert">${escapeHtml(
                    `${t('settings.internal_sites.error.draft_failed')}: ${errMessage(this._error)}`
                )}</div>
                <div class="isd-live"></div>
                <div class="isd-actions">${btn('close', 'close', true)}</div>`;
        }
        this._root.innerHTML = `
            <div class="isd-panel">
                <h3 class="isd-title">${escapeHtml(this._title)}</h3>
                ${body}
            </div>`;
        this._renderLive();
    }

    /** Refresh only the live area so typing / focus elsewhere is untouched. */
    _renderLive() {
        const live = this._root?.querySelector('.isd-live');
        if (!live) return;
        const secs = this._startedAt ? Math.round((Date.now() - this._startedAt) / 1000) : 0;
        const tools = [...this._tools.values()]
            .map((tc) => {
                const icon = tc.status === 'completed' ? '✓' : tc.status === 'failed' ? '✗' : '…';
                return `<li class="isd-tool isd-tool-${escapeAttr(tc.status)}"><span class="isd-tool-icon">${icon}</span> ${escapeHtml(tc.title)}</li>`;
            })
            .join('');
        const text = this._text.trim()
            ? `<pre class="isd-text" dir="auto">${escapeHtml(this._text.slice(-MAX_TEXT_TAIL))}</pre>`
            : '';
        const permission = this._permissionHtml();
        const waiting =
            !tools && !text && !permission && this._phase === 'running'
                ? `<div class="setting-description">${escapeHtml(t('settings.internal_sites.draft.waiting'))}</div>`
                : '';
        live.innerHTML = `
            ${this._phase === 'running' ? `<div class="isd-elapsed">${escapeHtml(t('settings.internal_sites.draft.elapsed', { seconds: secs }))}</div>` : ''}
            ${permission}
            ${tools ? `<ul class="isd-tools">${tools}</ul>` : ''}
            ${text}
            ${waiting}`;
        const pre = live.querySelector('.isd-text');
        if (pre) pre.scrollTop = pre.scrollHeight;
    }

    _permissionHtml() {
        const p = this._permission;
        if (!p) return '';
        const n = p.notification;
        const tool = p.toolName || n.params?.toolCall?.title || '';
        const options = Array.isArray(n.params?.options) ? n.params.options : [];
        const buttons = options
            .map((o) => {
                const allow = String(o.kind || o.optionId || '').startsWith('allow');
                return `<button class="setting-button" data-isd-action="permission" data-option="${escapeAttr(o.optionId)}"${allow ? '' : ' style="background:transparent;color:var(--kage-text-secondary);border:1px solid var(--kage-border);"'}>${escapeHtml(o.name || o.optionId)}</button>`;
            })
            .join('');
        return `
            <div class="isd-permission" role="alert">
                <div class="setting-label">${escapeHtml(t('settings.internal_sites.draft.permission_title', { tool }))}</div>
                <div class="isd-actions" style="justify-content:flex-start;">${buttons}</div>
            </div>`;
    }
}
