import { SettingsModule } from './base.js';
import { t } from '../shared/i18n.js';
import { errLabel } from '../shared/error-message.js';
import { escapeAttr, escapeHtml } from '../shared/tool-utils.js';
import { isWindows } from '../shared/shortcuts.js';
import { DraftDialog } from './internal-sites-draft.js';

/**
 * Internal Sites — link previews for pages that need the user's own
 * sign-in. Each profile pairs declarative detection rules with a proxy
 * script that runs locally (see src/internal_sites.rs for the contract).
 *
 * State lives in `this._profiles` and is written to
 * `config.internal_sites` by the global Save. The list and the editor are
 * rendered from that state; edits update it through delegated listeners
 * keyed on `data-is-*` attributes, so a re-render never loses input.
 */

const URL_RULE_KINDS = ['domain_suffix', 'regex'];
const CONTENT_TARGETS = ['body', 'final_url'];
const MATCH_KINDS = ['contains', 'regex'];
const DEFAULT_TIMEOUT_SECS = 15;

/** The one script language this platform runs. */
function platformLang() {
    return isWindows() ? 'powershell' : 'bash';
}

/**
 * Sample URL as typed → absolute http(s) URL, assuming https:// when no
 * scheme was given. `null` when it can't be a web URL.
 */
export function normalizeSampleUrl(raw) {
    const v = String(raw || '').trim();
    if (!v) return null;
    const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(v) ? v : `https://${v}`;
    try {
        const u = new URL(withScheme);
        return u.protocol === 'http:' || u.protocol === 'https:' ? u.href : null;
    } catch {
        return null;
    }
}

function newProfile() {
    return {
        id: globalThis.crypto?.randomUUID?.() || `site-${Date.now()}`,
        name: '',
        enabled: true,
        url_rules: [],
        content_rules: [],
        proxy: { lang: platformLang(), script: '', timeout_ms: 0 },
        sample_url: '',
    };
}

/** Fill any fields an older or hand-edited config left out. */
function normalizeProfile(p) {
    const base = newProfile();
    return {
        ...base,
        ...p,
        url_rules: Array.isArray(p?.url_rules) ? p.url_rules.map((r) => ({ ...r })) : [],
        content_rules: Array.isArray(p?.content_rules)
            ? p.content_rules.map((r) => ({ ...r }))
            : [],
        proxy: { ...base.proxy, ...(p?.proxy || {}) },
    };
}

/** Persisted shape: abandoned (blank) rule rows are dropped. */
export function cleanProfile(p) {
    return {
        ...p,
        url_rules: p.url_rules.filter((r) => r.pattern?.trim()),
        content_rules: p.content_rules.filter((r) => r.pattern?.trim()),
    };
}

function options(values, selected, labelKey) {
    return values
        .map(
            (v) =>
                `<option value="${v}"${v === selected ? ' selected' : ''}>${escapeHtml(t(`${labelKey}.${v}`))}</option>`
        )
        .join('');
}

const SECONDARY_BTN =
    'background:transparent;color:var(--kage-text-secondary);border:1px solid var(--kage-border);';

export class InternalSitesSettingsModule extends SettingsModule {
    constructor() {
        super('internal-sites', t('settings.internal_sites.title'), '🏢');
        this._profiles = [];
        this._editing = null; // index into _profiles, or null for the list
        this._busy = null; // 'rules' | 'script' | 'test' while a request runs
        this._status = null; // { slot, text, kind } — shown beside the button that set it
        this._testResult = null;
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                <div class="setting-description" style="margin-bottom:8px;">${escapeHtml(t('settings.internal_sites.intro'))}</div>
                <div class="setting-description" style="margin-bottom:12px;font-size:11px;opacity:0.8;">${escapeHtml(t('settings.internal_sites.script_warning'))}</div>
                <div id="internalSitesMount"></div>
            </div>
        `;
    }

    load(config) {
        this._profiles = Array.isArray(config.internal_sites)
            ? config.internal_sites.map(normalizeProfile)
            : [];
        if (this._editing !== null && this._editing >= this._profiles.length) {
            this._editing = null;
        }
        this._renderView();
    }

    save(config) {
        config.internal_sites = this._profiles.map(cleanProfile);
    }

    validate() {
        const unnamed = this._profiles.find((p) => !p.name.trim());
        if (unnamed) {
            return { valid: false, error: t('settings.internal_sites.validate.name_required') };
        }
        return { valid: true };
    }

    initialize() {
        const mount = document.getElementById('internalSitesMount');
        if (!mount) return;
        mount.addEventListener('input', (e) => this._onFieldEdit(e));
        mount.addEventListener('change', (e) => this._onFieldEdit(e));
        mount.addEventListener('click', (e) => {
            const btn = e.target.closest?.('[data-is-action]');
            if (!btn || !mount.contains(btn)) return;
            e.preventDefault();
            this._onAction(btn.dataset.isAction, btn.dataset.isIndex);
        });
    }

    // --- state edits --------------------------------------------------------

    _current() {
        return this._editing === null ? null : this._profiles[this._editing];
    }

    _onFieldEdit(e) {
        const el = e.target;
        const field = el?.dataset?.isField;
        if (!field) return;
        const value = el.type === 'checkbox' ? el.checked : el.value;

        if (field === 'list-enabled') {
            const p = this._profiles[Number(el.dataset.isIndex)];
            if (p) p.enabled = value;
            return;
        }
        const p = this._current();
        if (!p) return;
        const i = Number(el.dataset.isIndex);
        switch (field) {
            case 'name':
            case 'sample_url':
                p[field] = value;
                break;
            case 'enabled':
                p.enabled = value;
                break;
            case 'url-kind':
                p.url_rules[i].kind = value;
                break;
            case 'url-pattern':
                p.url_rules[i].pattern = value;
                break;
            case 'content-target':
                p.content_rules[i].target = value;
                break;
            case 'content-kind':
                p.content_rules[i].kind = value;
                break;
            case 'content-pattern':
                p.content_rules[i].pattern = value;
                break;
            case 'lang':
                p.proxy.lang = value;
                break;
            case 'script':
                p.proxy.script = value;
                break;
            case 'timeout': {
                const secs = Number(value);
                p.proxy.timeout_ms =
                    Number.isFinite(secs) && secs > 0 ? Math.round(secs * 1000) : 0;
                break;
            }
        }
    }

    _onAction(action, indexArg) {
        const i = Number(indexArg);
        const p = this._current();
        switch (action) {
            case 'add':
                this._profiles.push(newProfile());
                this._openEditor(this._profiles.length - 1);
                this.markDirty();
                return;
            case 'edit':
                this._openEditor(i);
                return;
            case 'delete': {
                const target = this._profiles[i];
                if (!target) return;
                const name = target.name || t('settings.internal_sites.list.unnamed');
                if (!confirm(t('settings.internal_sites.delete_confirm', { name }))) return;
                this._profiles.splice(i, 1);
                this.markDirty();
                this._renderView();
                return;
            }
            case 'back':
                this._editing = null;
                this._renderView();
                return;
            case 'add-url-rule':
                p.url_rules.push({ kind: 'domain_suffix', pattern: '' });
                break;
            case 'add-content-rule':
                p.content_rules.push({ target: 'body', kind: 'contains', pattern: '' });
                break;
            case 'remove-url-rule':
                p.url_rules.splice(i, 1);
                break;
            case 'remove-content-rule':
                p.content_rules.splice(i, 1);
                break;
            case 'draft-rules':
                this._draftRules();
                return;
            case 'draft-script':
                this._draftScript();
                return;
            case 'test':
                this._runTest();
                return;
            default:
                return;
        }
        this.markDirty();
        this._renderView();
    }

    _openEditor(index) {
        this._editing = index;
        this._status = null;
        this._testResult = null;
        this._renderView();
    }

    // --- agent drafting + test ---------------------------------------------

    _requireSampleUrl(p, slot) {
        const url = normalizeSampleUrl(p.sample_url);
        if (!url) {
            this._setStatus(
                slot,
                t('settings.internal_sites.validate.sample_url_required'),
                'error'
            );
            return null;
        }
        if (url !== p.sample_url) {
            p.sample_url = url;
            this.markDirty();
        }
        return url;
    }

    async _withBusy(kind, fn) {
        if (this._busy) return;
        this._busy = kind;
        this._renderView();
        try {
            await fn();
        } finally {
            this._busy = null;
            this._renderView();
        }
    }

    /**
     * Run an agent draft through the modal (guidance → live progress →
     * result). Returns the command's result, or null if the user
     * cancelled or it failed (the slot status says which).
     */
    async _runDraft(slot, title, call) {
        let outcome;
        await this._withBusy(slot, async () => {
            this._setStatus(slot, null, null, false);
            outcome = await new DraftDialog({ title, run: call }).open();
        });
        if (outcome?.status === 'done') return outcome.result;
        if (outcome?.status === 'cancelled') {
            this._setStatus(slot, t('settings.internal_sites.draft.cancelled'), null);
        } else if (outcome?.status === 'failed') {
            this._setStatus(
                slot,
                errLabel(t('settings.internal_sites.error.draft_failed'), outcome.error),
                'error'
            );
        }
        return null;
    }

    async _draftRules() {
        const p = this._current();
        const url = p && this._requireSampleUrl(p, 'rules');
        if (!url) return;
        const invoke = window.__TAURI__.core.invoke;
        const draft = await this._runDraft(
            'rules',
            t('settings.internal_sites.draft.title_rules'),
            ({ guidance, requestId }) =>
                invoke('internal_sites_draft_rules', { url, guidance, requestId })
        );
        if (!draft) return;
        if (!p.name.trim() && draft.name) p.name = draft.name;
        if (draft.url_rules?.length) p.url_rules = draft.url_rules;
        if (draft.content_rules?.length) p.content_rules = draft.content_rules;
        this.markDirty();
        this._setStatus('rules', t('settings.internal_sites.draft_rules.done'), 'success');
    }

    async _draftScript() {
        const p = this._current();
        const url = p && this._requireSampleUrl(p, 'script');
        if (!url) return;
        if (
            p.proxy.script.trim() &&
            !confirm(t('settings.internal_sites.draft_script.replace_confirm'))
        ) {
            return;
        }
        const invoke = window.__TAURI__.core.invoke;
        const script = await this._runDraft(
            'script',
            t('settings.internal_sites.draft.title_script'),
            ({ guidance, requestId }) =>
                invoke('internal_sites_draft_script', {
                    url,
                    lang: p.proxy.lang,
                    guidance,
                    requestId,
                })
        );
        if (!script) return;
        p.proxy.script = script;
        this.markDirty();
        this._setStatus('script', t('settings.internal_sites.draft_script.done'), 'success');
    }

    async _runTest() {
        const p = this._current();
        const url = p && this._requireSampleUrl(p, 'test');
        if (!url) return;
        const invoke = window.__TAURI__.core.invoke;
        this._testResult = null;
        this._setStatus('test', t('settings.internal_sites.testing'), null, false);
        await this._withBusy('test', async () => {
            try {
                const profile = cleanProfile(p);
                this._testResult = await invoke('internal_sites_test', { profile, url });
                this._status = null;
            } catch (e) {
                this._setStatus(
                    'test',
                    errLabel(t('settings.internal_sites.error.test_failed'), e),
                    'error',
                    false
                );
            }
        });
    }

    _setStatus(slot, text, kind, render = true) {
        this._status = text ? { slot, text, kind } : null;
        if (render) this._renderView();
    }

    /** Status line for `slot`, rendered right under the button that owns it. */
    _statusHtml(slot) {
        const s = this._status;
        if (!s || s.slot !== slot) return '';
        return `<div class="setting-description internal-site-status ${s.kind || ''}" role="status">${escapeHtml(s.text)}</div>`;
    }

    // --- rendering ----------------------------------------------------------

    _renderView() {
        const mount = document.getElementById('internalSitesMount');
        if (!mount) return;
        mount.innerHTML = this._editing === null ? this._renderList() : this._renderEditor();
    }

    _renderList() {
        const rows = this._profiles
            .map((p, i) => {
                const name = p.name.trim() || t('settings.internal_sites.list.unnamed');
                const summary = p.proxy.script.trim()
                    ? t('settings.internal_sites.list.summary', {
                          url: p.url_rules.length,
                          content: p.content_rules.length,
                      })
                    : t('settings.internal_sites.list.no_script');
                return `
                <div class="internal-site-row">
                    <label class="kage-checkbox" title="${escapeAttr(t('settings.internal_sites.enabled'))}">
                        <input type="checkbox" data-is-field="list-enabled" data-is-index="${i}"${p.enabled ? ' checked' : ''}>
                    </label>
                    <div class="internal-site-row-text">
                        <div class="internal-site-row-name">${escapeHtml(name)}</div>
                        <div class="setting-description" style="margin:0;">${escapeHtml(summary)}</div>
                    </div>
                    <button class="setting-button" data-is-action="edit" data-is-index="${i}">${escapeHtml(t('settings.internal_sites.list.edit'))}</button>
                    <button class="setting-button" data-is-action="delete" data-is-index="${i}" style="${SECONDARY_BTN}">${escapeHtml(t('settings.internal_sites.list.delete'))}</button>
                </div>`;
            })
            .join('');
        return `
            ${rows || `<div class="setting-description">${escapeHtml(t('settings.internal_sites.list.empty'))}</div>`}
            <button class="setting-button" data-is-action="add" style="margin-top:10px;">${escapeHtml(t('settings.internal_sites.list.add'))}</button>
        `;
    }

    _renderEditor() {
        const p = this._current();
        const busy = this._busy;
        const dis = busy ? ' disabled' : '';
        const label = (key) => escapeHtml(t(`settings.internal_sites.editor.${key}`));
        const btnLabel = (kind, key) =>
            busy === kind ? escapeHtml(t('settings.internal_sites.working')) : label(key);

        const urlRules = p.url_rules
            .map(
                (r, i) => `
            <div class="internal-site-rule">
                <select class="setting-input" data-is-field="url-kind" data-is-index="${i}">${options(URL_RULE_KINDS, r.kind, 'settings.internal_sites.rule.url_kind')}</select>
                <input type="text" class="setting-input internal-site-pattern" data-is-field="url-pattern" data-is-index="${i}" value="${escapeAttr(r.pattern || '')}" placeholder="${escapeAttr(t('settings.internal_sites.rule.pattern_placeholder'))}" dir="ltr">
                <button class="setting-button" data-is-action="remove-url-rule" data-is-index="${i}" style="${SECONDARY_BTN}" title="${escapeAttr(t('settings.internal_sites.rule.remove'))}" aria-label="${escapeAttr(t('settings.internal_sites.rule.remove'))}">✕</button>
            </div>`
            )
            .join('');

        const contentRules = p.content_rules
            .map(
                (r, i) => `
            <div class="internal-site-rule">
                <select class="setting-input" data-is-field="content-target" data-is-index="${i}">${options(CONTENT_TARGETS, r.target, 'settings.internal_sites.rule.target')}</select>
                <select class="setting-input" data-is-field="content-kind" data-is-index="${i}">${options(MATCH_KINDS, r.kind, 'settings.internal_sites.rule.match_kind')}</select>
                <input type="text" class="setting-input internal-site-pattern" data-is-field="content-pattern" data-is-index="${i}" value="${escapeAttr(r.pattern || '')}" placeholder="${escapeAttr(t('settings.internal_sites.rule.pattern_placeholder'))}" dir="ltr">
                <button class="setting-button" data-is-action="remove-content-rule" data-is-index="${i}" style="${SECONDARY_BTN}" title="${escapeAttr(t('settings.internal_sites.rule.remove'))}" aria-label="${escapeAttr(t('settings.internal_sites.rule.remove'))}">✕</button>
            </div>`
            )
            .join('');

        const timeoutSecs = p.proxy.timeout_ms ? p.proxy.timeout_ms / 1000 : DEFAULT_TIMEOUT_SECS;
        // One language per platform; a picker only appears for a profile
        // written on another OS (e.g. a config copied across machines).
        const langPicker =
            p.proxy.lang === platformLang()
                ? ''
                : `<select class="setting-input" data-is-field="lang">${options(['powershell', 'bash'], p.proxy.lang, 'settings.internal_sites.editor.lang')}</select>`;

        return `
            <div style="display:flex;align-items:center;gap:8px;margin-bottom:12px;">
                <button class="setting-button" data-is-action="back" style="${SECONDARY_BTN}">${label('back')}</button>
                <label class="kage-checkbox" style="margin-left:auto;">
                    <input type="checkbox" data-is-field="enabled"${p.enabled ? ' checked' : ''}>
                    <span style="font-size:12px;">${escapeHtml(t('settings.internal_sites.enabled'))}</span>
                </label>
            </div>

            ${this.createControlRow(
                label('name'),
                '',
                `<input type="text" class="setting-input" data-is-field="name" value="${escapeAttr(p.name)}" placeholder="${escapeAttr(t('settings.internal_sites.editor.name_placeholder'))}">`
            )}

            ${this.createControlWithActionRow(
                label('sample_url'),
                escapeHtml(t('settings.internal_sites.editor.sample_url_description')),
                `<input type="text" class="setting-input" data-is-field="sample_url" value="${escapeAttr(p.sample_url)}" placeholder="wiki.example.com/page" dir="ltr" inputmode="url" spellcheck="false">`,
                `<button class="setting-button" data-is-action="test"${dis}>${btnLabel('test', 'test_btn')}</button>`
            )}
            ${this._statusHtml('test')}
            ${this._renderTestResult()}

            <div class="setting-row">
                <div class="setting-label">${label('rules')}</div>
                <div class="setting-description">${label('rules_description')}</div>
                <div class="setting-control" style="margin-top:6px;">
                    <button class="setting-button" data-is-action="draft-rules"${dis}>${btnLabel('rules', 'draft_rules_btn')}</button>
                </div>
                ${this._statusHtml('rules')}
            </div>

            <div class="setting-row">
                <div class="setting-label">${label('url_rules')}</div>
                <div class="setting-description">${label('url_rules_description')}</div>
                <div class="internal-site-rules">${urlRules}</div>
                <button class="setting-button" data-is-action="add-url-rule" style="${SECONDARY_BTN}margin-top:6px;">${label('add_url_rule')}</button>
            </div>

            <div class="setting-row">
                <div class="setting-label">${label('content_rules')}</div>
                <div class="setting-description">${label('content_rules_description')}</div>
                <div class="internal-site-rules">${contentRules}</div>
                <button class="setting-button" data-is-action="add-content-rule" style="${SECONDARY_BTN}margin-top:6px;">${label('add_content_rule')}</button>
            </div>

            <div class="setting-row">
                <div class="setting-label">${label('script')}</div>
                <div class="setting-description">${t('settings.internal_sites.editor.script_description_html')}</div>
                <div class="internal-site-script-bar">
                    ${langPicker}
                    <label class="setting-description" style="margin:0;display:flex;align-items:center;gap:6px;">
                        ${label('timeout')}
                        <input type="number" class="setting-input" data-is-field="timeout" min="1" max="60" value="${timeoutSecs}" style="width:70px;">
                    </label>
                    <button class="setting-button" data-is-action="draft-script" style="margin-left:auto;"${dis}>${btnLabel('script', 'draft_script_btn')}</button>
                </div>
                ${this._statusHtml('script')}
                <textarea class="setting-input internal-site-script" data-is-field="script" rows="12" spellcheck="false" dir="ltr">${escapeHtml(p.proxy.script)}</textarea>
            </div>
        `;
    }

    _renderTestResult() {
        const r = this._testResult;
        if (!r) return '';
        const yesNo = (v) =>
            v === null || v === undefined
                ? t('settings.internal_sites.test.not_checked')
                : v
                  ? t('settings.internal_sites.test.matched')
                  : t('settings.internal_sites.test.not_matched');
        const direct = r.probe.ok
            ? t('settings.internal_sites.test.direct_status', {
                  status: r.probe.status ?? '',
                  url: r.probe.final_url || '',
              })
            : t('settings.internal_sites.test.direct_failed', { error: r.probe.error || '' });
        const proxy = r.proxy.timed_out
            ? t('settings.internal_sites.test.proxy_timed_out')
            : r.proxy.ok
              ? t('settings.internal_sites.test.proxy_ok', { bytes: r.proxy.stdout_bytes })
              : t('settings.internal_sites.test.proxy_failed', { code: r.proxy.exit_code ?? '?' });

        const md = r.metadata;
        const favicon = md?.favicon?.startsWith('data:image/')
            ? `<img src="${escapeAttr(md.favicon)}" alt="" width="16" height="16">`
            : '';
        const preview =
            md && (md.title || md.description)
                ? `<div class="internal-site-preview">
                    <div class="internal-site-preview-title">${favicon}<span>${escapeHtml(md.title || '')}</span></div>
                    ${md.description ? `<div class="setting-description" style="margin:4px 0 0;">${escapeHtml(md.description)}</div>` : ''}
                </div>`
                : `<div class="setting-description">${escapeHtml(t('settings.internal_sites.test.no_preview'))}</div>`;
        const stderr = r.proxy.stderr_tail?.trim()
            ? `<pre class="internal-site-stderr" dir="ltr">${escapeHtml(r.proxy.stderr_tail)}</pre>`
            : '';
        const iconReason = (a) =>
            a.ok
                ? t('settings.internal_sites.test.icon.ok')
                : a.timed_out
                  ? t('settings.internal_sites.test.proxy_timed_out')
                  : a.rejection
                    ? t(`settings.internal_sites.test.icon.${a.rejection}`)
                    : t('settings.internal_sites.test.proxy_failed', { code: a.exit_code ?? '?' });
        const attempts = Array.isArray(r.favicon_attempts) ? r.favicon_attempts : [];
        const faviconSummary = md?.favicon
            ? t('settings.internal_sites.test.icon.found')
            : t('settings.internal_sites.test.icon.none');
        const faviconList = attempts.length
            ? `<ul class="internal-site-icon-attempts">${attempts
                  .map(
                      (a) =>
                          `<li class="${a.ok ? 'ok' : 'failed'}"><span dir="ltr">${escapeHtml(a.url)}</span> — ${escapeHtml(iconReason(a))}</li>`
                  )
                  .join('')}</ul>`
            : '';
        const line = (key, value) =>
            `<div><span class="internal-site-test-key">${escapeHtml(t(`settings.internal_sites.test.${key}`))}</span> ${escapeHtml(String(value))}</div>`;

        return `
            <div class="internal-site-test">
                ${line('url_rules', yesNo(r.url_match))}
                ${line('content_rules', yesNo(r.content_match))}
                ${line('direct', direct)}
                ${line('proxy', proxy)}
                ${stderr}
                ${md ? `${line('favicon', faviconSummary)}${faviconList}` : ''}
                ${preview}
            </div>
        `;
    }
}
