/**
 * The Background AI panel on an extension's Settings page, and the consent
 * dialog (docs/design/EXTENSION_AGENT_CALLS.md §4.2, §4.3).
 *
 * Shown for an extension whose grant holds `agent_background` and whose
 * manifest declares `agentBackground`. It is plain host DOM in a trusted
 * window, so the extension can't reach or imitate it. Every command it
 * calls is host-only (`null` in COMMAND_CAPABILITIES), and Rust re-checks
 * the gates on each one.
 *
 * Header order on the page (SYNC.md §23.3): capability badges, the sync
 * chip, this panel, the Windows block, then the extension's own settings.
 *
 * The page is built as one HTML string, so {@link renderAgentAccessPanel}
 * returns a placeholder and fills it when `get_extension_agent_access`
 * answers. Panels refresh on `agent_calls_activity` and `config_updated`.
 *
 * The consent dialog opens when the switch goes on, from "Allow again", or
 * from a click on the extension's own settings button through the
 * `agent_access` host effect (settings-host-effects.js). Nothing else opens
 * it, and it never opens on its own.
 *
 * The panel acts at once through host commands, never through Save, so its
 * root carries `data-sync-ui`: the dirty tracker skips nodes marked that
 * way (dirty-tracker.js).
 *
 * Every string the extension supplies (its name, `reason`, labels,
 * purposes) goes into the DOM as text, never as HTML. Agent names and
 * hosts are isolated left to right (`<bdi dir="ltr">`).
 */

// Its stylesheet styles the dialogs below (kage-confirm-*).
import '../shared/confirm-dialog.js';
import { onConfigChange } from '../shared/config-cache.js';
import { errMessage } from '../shared/error-message.js';
import { EVT } from '../shared/events.js';
import {
    fetchExtensionLocaleViaInvoke,
    resolveExtensionMessage,
} from '../shared/extension-manager/i18n.js';
import { activeLanguage, t } from '../shared/i18n.js';
import { setAgentAccessOpener } from '../shared/settings-host-effects.js';
import { escapeAttr } from '../shared/tool-utils.js';
import { getSettingsManager } from './module-registry.js';

/** The capability the panel is for. */
export const AGENT_CAPABILITY = 'agent_background';

/** Rows in "Recent requests" (the backend sends at most 20). */
const LOG_ROWS = 20;
/** Longest `reason` shown, in characters (§3.2). */
const MAX_REASON_CHARS = 300;
/** Longest label shown, in characters (§5.6). */
const MAX_LABEL_CHARS = 80;
/** Failures in a row that open the failure breaker (§5.6). */
const FAILURE_BREAKER = 3;
/** Allowance levels, in the order the select lists them. */
const LEVELS = Object.freeze(['low', 'normal', 'high']);
/** Capabilities the dialog lists as "can also read" (Rust sends the held ones). */
const DATA_CAPABILITIES = Object.freeze([
    'clipboard',
    'calendar',
    'session',
    'activity',
    'windows',
    'filesystem',
]);
const MSG_TOKEN = /^__MSG_(.+)__$/;
/**
 * Control, bidi override and private-use characters: never shown from
 * extension text (the private-use ones are the slots below).
 */
function isUnsafeChar(code) {
    return (
        code < 0x20 ||
        (code >= 0x7f && code <= 0x9f) ||
        code === 0x061c ||
        code === 0x200e ||
        code === 0x200f ||
        (code >= 0x202a && code <= 0x202e) ||
        (code >= 0x2066 && code <= 0x2069) ||
        (code >= 0xe000 && code <= 0xf8ff)
    );
}

/**
 * Placeholders a translated string carries for values that become their
 * own text nodes: `t()` fills them in, {@link appendText} swaps each for
 * its value. Private-use characters, so they never occur in a catalog.
 */
const SLOT = Object.freeze({ ext: '\ue020', agent: '\ue021', host: '\ue022', reason: '\ue023' });
const SLOT_SPLIT = /([\ue020-\ue02f])/;

// --- Pure helpers ---------------------------------------------------------------

function language() {
    try {
        return activeLanguage() || undefined;
    } catch {
        return undefined;
    }
}

/** A count in the user's number format. */
export function formatCount(n) {
    const value = Number.isFinite(Number(n)) ? Number(n) : 0;
    try {
        return new Intl.NumberFormat(language()).format(value);
    } catch {
        return String(value);
    }
}

/**
 * A local time from an RFC 3339 string: the time alone when it is today,
 * else with the weekday. '' when it doesn't parse.
 */
export function formatClock(iso, now = new Date()) {
    if (typeof iso !== 'string' || !iso) return '';
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return '';
    const opts =
        d.toDateString() === now.toDateString()
            ? { hour: '2-digit', minute: '2-digit' }
            : { weekday: 'short', hour: '2-digit', minute: '2-digit' };
    try {
        return d.toLocaleString(language(), opts);
    } catch {
        return d.toLocaleString(undefined, opts);
    }
}

/** A log row's time: date and time. */
function formatWhen(iso) {
    const d = new Date(iso);
    if (typeof iso !== 'string' || Number.isNaN(d.getTime())) return '';
    const opts = { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' };
    try {
        return d.toLocaleString(language(), opts);
    } catch {
        return d.toLocaleString(undefined, opts);
    }
}

/** Extension text, safe to show: no control or bidi characters, capped. */
export function cleanText(value, max) {
    if (typeof value !== 'string') return '';
    const chars = Array.from(value)
        .filter((c) => !isUnsafeChar(c.codePointAt(0)))
        .join('')
        .trim();
    const all = Array.from(chars);
    return all.length > max ? `${all.slice(0, max - 1).join('')}…` : chars;
}

function holdsCapability(capabilities, cap) {
    if (Array.isArray(capabilities)) return capabilities.includes(cap);
    return typeof capabilities?.has === 'function' && capabilities.has(cap);
}

/**
 * Whether an extension page shows the panel: the grant holds
 * `agent_background` and the manifest has an `agentBackground` block.
 * Rust decides whether the block is valid; the panel says so if not.
 *
 * @param {object} module - the page's settings module: `_extensionId`,
 *   `_capabilities` (the grant), `_manifest`
 */
export function showsAgentPanel(module) {
    const id = module?._extensionId;
    if (typeof id !== 'string' || !id) return false;
    if (!holdsCapability(module._capabilities, AGENT_CAPABILITY)) return false;
    const decl = module._manifest?.agentBackground;
    return !!decl && typeof decl === 'object' && !Array.isArray(decl);
}

/**
 * The name the audit viewer shows for an extension id: its Settings
 * page's title, else the id.
 */
export function extensionDisplayName(extensionId) {
    const id = typeof extensionId === 'string' ? extensionId : '';
    const modules = getSettingsManager()?.modules;
    const module = Array.isArray(modules) ? modules.find((m) => m?._extensionId === id) : null;
    const title = typeof module?.title === 'string' ? module.title.trim() : '';
    return title && !MSG_TOKEN.test(title) ? title : id || '?';
}

/** The words for a call's outcome (the panel log and the audit viewer). */
export function agentOutcomeLabel(outcome) {
    switch (outcome) {
        case 'ok':
            return t('settings.manager.ext.agent.outcome.ok');
        case 'invalid_response':
            return t('settings.manager.ext.agent.outcome.invalid_response');
        case 'not_json':
            return t('settings.manager.ext.agent.outcome.not_json');
        case 'response_too_large':
            return t('settings.manager.ext.agent.outcome.response_too_large');
        case 'timeout':
            return t('settings.manager.ext.agent.outcome.timeout');
        case 'cancelled':
            return t('settings.manager.ext.agent.outcome.cancelled');
        case 'tool_attempted':
            return t('settings.manager.ext.agent.outcome.tool_attempted');
        case 'agent_error':
            return t('settings.manager.ext.agent.outcome.agent_error');
        case 'connection_lost':
            return t('settings.manager.ext.agent.outcome.connection_lost');
        default:
            return typeof outcome === 'string' ? cleanText(outcome, 40) : '?';
    }
}

/**
 * What the status line says, from the panel state.
 *
 * @returns {{ kind: string, agent?: string, time?: string, allowAgain?: boolean }}
 *   `kind`: off, on, needs_consent, stale, paused_failures, paused_tools,
 *   paused_stopped or unavailable
 */
export function panelStatus(panel) {
    if (!panel?.available) return { kind: 'unavailable' };
    const agent = panel.agent?.label || '';
    switch (panel.access) {
        case 'consent_stale':
            return { kind: 'stale', agent, allowAgain: true };
        case 'needs_consent':
            return { kind: 'needs_consent', allowAgain: true };
        case 'on': {
            const until = panel.pause?.until;
            if (until) {
                const reason = panel.pause.reason;
                const kind =
                    reason === 'tool_attempt'
                        ? 'paused_tools'
                        : reason === 'failures'
                          ? 'paused_failures'
                          : 'paused_stopped';
                return { kind, time: until };
            }
            return { kind: 'on', agent: panel.consent_agent || agent };
        }
        default:
            return { kind: 'off' };
    }
}

/** The allowance the dialog and the select show for a level. */
function allowanceFor(panel, level) {
    const a = panel?.allowances?.[level || panel?.allowance || 'normal'];
    return { calls: Number(a?.calls) || 0, chars: Number(a?.chars) || 0 };
}

/**
 * The call rows of "Recent requests", newest first: the panel's log
 * without the switch's on/off entries.
 */
export function callRows(panel) {
    const viewable = new Set(Array.isArray(panel?.viewable) ? panel.viewable : []);
    const log = Array.isArray(panel?.log) ? panel.log : [];
    return log
        .filter((e) => e && e.event === 'extension_agent_call')
        .slice(0, LOG_ROWS)
        .map((e) => ({
            callId: typeof e.call_id === 'string' ? e.call_id : '',
            at: typeof e.at === 'string' ? e.at : '',
            purpose: cleanText(e.purpose, 24) || '?',
            label: cleanText(e.label, MAX_LABEL_CHARS),
            outcome: typeof e.outcome === 'string' ? e.outcome : '',
            chars: Number(e.prompt_chars) || 0,
            agent: cleanText(e.agent, 64),
            viewable: typeof e.call_id === 'string' && viewable.has(e.call_id),
        }));
}

/**
 * Where the active agent sends text, for the dialog: the catalog key
 * suffix and the other computer, if any (§4.3).
 */
export function agentWhere(agent) {
    const kind = agent?.location?.kind;
    const host = typeof agent?.location?.host === 'string' ? agent.location.host : '';
    if (kind === 'cloud' || kind === 'local') return { kind };
    if (kind === 'remote' && host) return { kind, host };
    return { kind: 'unknown' };
}

/**
 * Everything the consent dialog shows. Pure.
 *
 * @param {object} panel - `get_extension_agent_access`'s answer
 * @param {{ name?: string, reason?: string }} [opts] - the extension's
 *   display name, and its `reason` with any `__MSG_` token resolved
 */
export function consentModel(panel, { name, reason } = {}) {
    const reads = Array.isArray(panel?.reads)
        ? panel.reads.filter((c) => DATA_CAPABILITIES.includes(c))
        : [];
    return {
        extension: cleanText(name || panel?.name || panel?.extension_id || '', 80),
        agent: cleanText(panel?.agent?.label || '', 120),
        where: agentWhere(panel?.agent),
        reason: cleanText(reason, MAX_REASON_CHARS),
        calls: allowanceFor(panel).calls,
        reads,
    };
}

/**
 * The manifest's `reason`, with a `__MSG_key__` token resolved from the
 * extension's catalog (fetched when not given). '' when it can't be.
 */
export async function resolveReason(raw, { catalog, fallback, invoke, extensionId } = {}) {
    if (typeof raw !== 'string') return '';
    const token = MSG_TOKEN.exec(raw.trim());
    if (!token) return cleanText(raw, MAX_REASON_CHARS);
    let text = resolveExtensionMessage(token[1], catalog, fallback, '');
    if (!text && typeof invoke === 'function' && extensionId) {
        const loc = await fetchExtensionLocaleViaInvoke(invoke, {
            id: extensionId,
            type: 'extension',
        });
        text = resolveExtensionMessage(token[1], loc.catalog, loc.fallback, '');
    }
    return cleanText(text, MAX_REASON_CHARS);
}

function listOf(items) {
    try {
        return new Intl.ListFormat(language(), { style: 'long', type: 'conjunction' }).format(
            items
        );
    } catch {
        return items.join(', ');
    }
}

// --- DOM helpers -------------------------------------------------------------------

function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = text;
    return node;
}

/**
 * Append a translated string to `target`, putting each slot's value in
 * its own `<bdi>` (left to right when `ltr`). Values are text nodes, so
 * they are never parsed as HTML.
 */
function appendText(target, text, values = {}) {
    for (const part of String(text ?? '').split(SLOT_SPLIT)) {
        if (!part) continue;
        const value = values[part];
        if (value === undefined) {
            target.append(part);
            continue;
        }
        const bdi = document.createElement('bdi');
        if (value.ltr) bdi.dir = 'ltr';
        bdi.textContent = String(value.text ?? '');
        target.append(bdi);
    }
    return target;
}

let _uid = 0;
function uniqueId(prefix) {
    _uid += 1;
    return `${prefix}-${_uid}`;
}

const STYLE_ID = 'ext-agent-access-style';

function ensureStyles() {
    if (typeof document === 'undefined' || document.getElementById(STYLE_ID)) return;
    const style = document.createElement('style');
    style.id = STYLE_ID;
    style.textContent = `
.ext-agent-panel { border-block-start: 1px solid var(--kage-border, #3a3640); margin-block: 8px; }
.ext-agent-switch-row { align-items: flex-start; }
.ext-agent-switch-label { font-size: 13px; color: var(--kage-text); cursor: pointer; }
.ext-agent-status { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin-block-start: 8px; }
.ext-agent-usage, .ext-agent-allowance { margin-block-start: 4px; }
.ext-agent-allowance { display: flex; align-items: center; gap: 8px; font-size: 13px; }
.ext-agent-allowance .setting-select { inline-size: auto; }
.ext-agent-actions { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin-block-start: 8px; }
.ext-agent-feedback { margin-block-start: 6px; }
.ext-agent-feedback.is-error { color: var(--kage-error); }
.ext-agent-log-title { font-size: 12px; font-weight: 600; margin-block: 12px 4px; color: var(--kage-text-bright); }
.ext-agent-log { list-style: none; margin: 0; padding: 0; max-block-size: 240px; overflow-y: auto; }
.ext-agent-log li { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; padding-block: 4px; font-size: 12px; color: var(--kage-text); border-block-end: 1px solid var(--kage-border, #3a3640); }
.ext-agent-log li:last-child { border-block-end: none; }
.ext-agent-log-label { color: var(--kage-text-muted); }
.ext-agent-link { background: none; border: none; padding: 0; font: inherit; color: var(--kage-accent); cursor: pointer; text-decoration: underline; }
.ext-agent-note { font-size: 12px; color: var(--kage-text-muted); margin-block-start: 4px; }
.ext-agent-consent-modal { inline-size: min(92vw, 520px); }
.ext-agent-consent-body { padding-block: 0 14px; padding-inline: 18px; font-size: 13px; line-height: 1.5; }
.ext-agent-consent-reason { margin-block: 0 10px; margin-inline: 0; padding-inline-start: 10px; border-inline-start: 3px solid var(--kage-accent); white-space: pre-wrap; word-break: break-word; }
.ext-agent-consent-list { margin: 0; padding-inline-start: 18px; }
.ext-agent-consent-list li { margin-block-end: 4px; }
.ext-agent-detail-modal { inline-size: min(92vw, 760px); }
.ext-agent-detail-body { padding-block: 0 14px; padding-inline: 18px; max-block-size: 65vh; overflow-y: auto; }
.ext-agent-detail-heading { font-size: 12px; font-weight: 600; margin-block: 8px 4px; }
.ext-agent-detail-pre { margin: 0; padding: 8px; border-radius: 6px; background: rgba(0, 0, 0, 0.15); font-family: var(--kage-font-mono, ui-monospace, monospace); font-size: 12px; white-space: pre-wrap; word-break: break-word; max-block-size: 30vh; overflow: auto; }
`;
    document.head.appendChild(style);
}

/**
 * A modal on the shared dialog chrome. Escape and a click on the backdrop
 * close it with `false`; Tab stays inside it.
 *
 * @returns {{ backdrop: HTMLElement, modal: HTMLElement, close: Function, done: Promise }}
 */
function openModal(modalClass, { labelledBy, describedBy } = {}) {
    ensureStyles();
    const backdrop = el('div', 'kage-confirm-backdrop');
    const modal = el('div', `kage-confirm-modal ${modalClass}`);
    modal.setAttribute('role', 'dialog');
    modal.setAttribute('aria-modal', 'true');
    if (labelledBy) modal.setAttribute('aria-labelledby', labelledBy);
    if (describedBy) modal.setAttribute('aria-describedby', describedBy);
    backdrop.append(modal);
    const previous = document.activeElement;
    let settle;
    const done = new Promise((resolve) => {
        settle = resolve;
    });
    let closed = false;
    const onKey = (e) => {
        if (e.key === 'Escape') {
            e.stopPropagation();
            e.preventDefault();
            close(false);
        } else if (e.key === 'Tab') {
            const focusable = [...modal.querySelectorAll('button:not([disabled])')];
            if (focusable.length === 0) return;
            const first = focusable[0];
            const last = focusable[focusable.length - 1];
            if (e.shiftKey && document.activeElement === first) {
                e.preventDefault();
                last.focus();
            } else if (!e.shiftKey && document.activeElement === last) {
                e.preventDefault();
                first.focus();
            }
        }
    };
    function close(result) {
        if (closed) return;
        closed = true;
        document.removeEventListener('keydown', onKey, true);
        backdrop.remove();
        try {
            previous?.focus?.();
        } catch {}
        settle(result);
    }
    document.addEventListener('keydown', onKey, true);
    backdrop.addEventListener('click', (e) => {
        if (e.target === backdrop) close(false);
    });
    document.body.append(backdrop);
    return { backdrop, modal, close, done };
}

// --- The consent dialog ------------------------------------------------------------

/** True while a consent dialog is open: only one at a time. */
let consentOpen = false;

/**
 * Show the consent dialog (§4.3). Resolves `true` on Allow, `false` on
 * Not now, Escape or a click outside it.
 *
 * @param {ReturnType<typeof consentModel>} model
 * @returns {Promise<boolean>}
 */
export function showConsentDialog(model) {
    if (typeof document === 'undefined' || !document.body) return Promise.resolve(false);
    const titleId = uniqueId('ext-agent-consent-title');
    const whereId = uniqueId('ext-agent-consent-where');
    const { modal, close, done } = openModal('ext-agent-consent-modal', {
        labelledBy: titleId,
        describedBy: whereId,
    });
    const ext = { text: model.extension };
    const agent = { text: model.agent, ltr: true };

    const header = el('div', 'kage-confirm-header');
    header.append(el('div', 'kage-confirm-icon', '🌙'));
    const heading = el('div', 'kage-confirm-heading');
    const title = el('div', 'kage-confirm-title');
    title.id = titleId;
    appendText(
        title,
        t('settings.manager.ext.agent.consent.title', { extension: SLOT.ext, agent: SLOT.agent }),
        { [SLOT.ext]: ext, [SLOT.agent]: agent }
    );
    const where = el('div', 'kage-confirm-message ext-agent-consent-where');
    where.id = whereId;
    where.dataset.where = model.where.kind;
    const whereValues = { [SLOT.agent]: agent, [SLOT.host]: { text: model.where.host, ltr: true } };
    switch (model.where.kind) {
        case 'cloud':
            appendText(
                where,
                t('settings.manager.ext.agent.consent.where.cloud', { agent: SLOT.agent }),
                whereValues
            );
            break;
        case 'local':
            appendText(
                where,
                t('settings.manager.ext.agent.consent.where.local', { agent: SLOT.agent }),
                whereValues
            );
            break;
        case 'remote':
            appendText(
                where,
                t('settings.manager.ext.agent.consent.where.remote', {
                    agent: SLOT.agent,
                    host: SLOT.host,
                }),
                whereValues
            );
            break;
        default:
            appendText(
                where,
                t('settings.manager.ext.agent.consent.where.unknown', { agent: SLOT.agent }),
                whereValues
            );
    }
    heading.append(title, where);
    header.append(heading);

    const body = el('div', 'ext-agent-consent-body');
    if (model.reason) {
        // A quote from the extension: its own words, shown as text.
        const quote = el('blockquote', 'ext-agent-consent-reason');
        appendText(
            quote,
            t('settings.manager.ext.agent.consent.reason', {
                extension: SLOT.ext,
                reason: SLOT.reason,
            }),
            { [SLOT.ext]: ext, [SLOT.reason]: { text: model.reason } }
        );
        body.append(quote);
    }
    const list = el('ul', 'ext-agent-consent-list');
    const item = (text, values = {}) => list.append(appendText(el('li'), text, values));
    item(
        t('settings.manager.ext.agent.consent.what', {
            extension: SLOT.ext,
            calls: formatCount(model.calls),
        }),
        { [SLOT.ext]: ext }
    );
    if (model.reads.length > 0) {
        const labels = model.reads.map((cap) => t(`settings.manager.cap.${cap}.label`));
        const li = appendText(
            el('li'),
            t('settings.manager.ext.agent.consent.reads', {
                extension: SLOT.ext,
                capabilities: listOf(labels),
            }),
            { [SLOT.ext]: ext }
        );
        li.dataset.reads = model.reads.join(' ');
        list.append(li);
    }
    item(t('settings.manager.ext.agent.consent.when'));
    item(t('settings.manager.ext.agent.consent.tools'));
    item(t('settings.manager.ext.agent.consent.history'));
    item(t('settings.manager.ext.agent.consent.log'));
    body.append(list);

    const footer = el('div', 'kage-confirm-footer');
    const cancel = el('button', 'kage-confirm-btn', t('settings.manager.ext.agent.consent.cancel'));
    cancel.type = 'button';
    cancel.dataset.action = 'cancel';
    const allow = el(
        'button',
        'kage-confirm-btn kage-confirm-btn-primary',
        t('settings.manager.ext.agent.consent.allow')
    );
    allow.type = 'button';
    allow.dataset.action = 'allow';
    cancel.addEventListener('click', () => close(false));
    allow.addEventListener('click', () => close(true));
    footer.append(cancel, allow);

    modal.append(header, body, footer);
    modal.classList.add('ext-agent-consent');
    // Not now has the focus: Enter alone never sends anything.
    cancel.focus();
    return done;
}

/**
 * Turn background AI on for an extension: read the panel state, show the
 * consent dialog, and pass the consent token back on Allow. Used by the
 * switch, "Allow again", and the `agent_access` host effect.
 *
 * @param {string} extensionId - the page's own extension, from the host
 * @param {{ invoke?: Function, catalog?: object, fallback?: object,
 *   name?: string }} [deps] - the extension's catalog resolves its `reason`
 * @returns {Promise<{ ok: boolean, already?: boolean, cancelled?: boolean,
 *   busy?: boolean, message?: string }>}
 */
export async function requestAgentAccess(extensionId, deps = {}) {
    const invoke = deps.invoke || panels.get(extensionId)?.invoke || defaultInvoke();
    if (typeof invoke !== 'function' || typeof extensionId !== 'string' || !extensionId) {
        return { ok: false };
    }
    if (consentOpen) return { ok: false, busy: true };
    consentOpen = true;
    const entry = panels.get(extensionId);
    if (entry) {
        entry.pending = true;
        entry.feedback = null;
        repaint(entry);
    }
    try {
        const panel = await invoke('get_extension_agent_access', { extensionId });
        const name = cleanText(panel?.name, 80) || cleanText(deps.name, 80) || extensionId;
        if (!panel?.available) {
            return {
                ok: false,
                message: t('errors.agent_call.not_available', { extension: name }),
            };
        }
        if (panel.access === 'on') return { ok: true, already: true };
        if (!panel.agent || !panel.consent_token) {
            return { ok: false, message: t('errors.agent_call.agent_unavailable') };
        }
        const reason = await resolveReason(panel.reason, {
            catalog: deps.catalog ?? entry?.catalog,
            fallback: deps.fallback ?? entry?.fallback,
            invoke,
            extensionId,
        });
        const allowed = await showConsentDialog(consentModel(panel, { name, reason }));
        if (!allowed) return { ok: false, cancelled: true };
        // The token names the agent the dialog named. If the agent changed
        // while it was open, Rust refuses with `agent_changed`.
        await invoke('set_extension_agent_access', {
            extensionId,
            enabled: true,
            consentToken: panel.consent_token,
        });
        return { ok: true };
    } catch (e) {
        console.warn(`[ext ${extensionId}] turning on background AI failed:`, e);
        return { ok: false, message: errMessage(e) };
    } finally {
        consentOpen = false;
        if (entry) entry.pending = false;
        await refreshAgentAccessPanel(extensionId);
    }
}

// The `agent_access` host effect reaches the dialog through this.
setAgentAccessOpener((args) =>
    requestAgentAccess(args?.extensionId, {
        catalog: args?.catalog,
        fallback: args?.fallback,
        name: args?.name,
    })
);

// --- "Show what was sent" ------------------------------------------------------------

/**
 * The exact prompt and reply of one call, read-only (§4.2). Resolves when
 * closed.
 *
 * @param {{ purpose?: string, label?: string, at?: string, prompt?: string,
 *   reply?: string }} detail
 */
export function showRequestDetail(detail) {
    if (typeof document === 'undefined' || !document.body) return Promise.resolve(false);
    const titleId = uniqueId('ext-agent-detail-title');
    const { modal, close, done } = openModal('ext-agent-detail-modal', { labelledBy: titleId });
    const header = el('div', 'kage-confirm-header');
    const heading = el('div', 'kage-confirm-heading');
    const title = el('div', 'kage-confirm-title', t('settings.manager.ext.agent.log.sent_title'));
    title.id = titleId;
    const parts = [
        formatWhen(detail?.at),
        cleanText(detail?.purpose, 24),
        cleanText(detail?.label, MAX_LABEL_CHARS),
    ].filter(Boolean);
    heading.append(title, el('div', 'kage-confirm-message', parts.join(' · ')));
    header.append(heading);

    const body = el('div', 'ext-agent-detail-body');
    const block = (heading, text, kind) => {
        body.append(el('div', 'ext-agent-detail-heading', heading));
        const pre = el('pre', 'ext-agent-detail-pre', typeof text === 'string' ? text : '');
        pre.dir = 'auto';
        pre.dataset.part = kind;
        body.append(pre);
    };
    block(t('settings.manager.ext.agent.log.sent_prompt'), detail?.prompt, 'prompt');
    block(t('settings.manager.ext.agent.log.sent_reply'), detail?.reply, 'reply');

    const footer = el('div', 'kage-confirm-footer');
    const ok = el(
        'button',
        'kage-confirm-btn kage-confirm-btn-primary',
        t('settings.manager.ext.agent.log.close')
    );
    ok.type = 'button';
    ok.dataset.action = 'close';
    ok.addEventListener('click', () => close(true));
    footer.append(ok);
    modal.append(header, body, footer);
    ok.focus();
    return done;
}

// --- The panel ----------------------------------------------------------------------

/**
 * extension id → { id, invoke, catalog, fallback, name, last, seq,
 * busy, pending, feedback }
 */
const panels = new Map();
let listening = false;
let refreshTimer = null;

function defaultInvoke() {
    return globalThis.window?.__TAURI__?.core?.invoke;
}

function rootsFor(extensionId) {
    if (typeof document === 'undefined') return [];
    return [...document.querySelectorAll('[data-ext-agent]')].filter(
        (node) => node.dataset.extAgent === extensionId
    );
}

function scheduleRefreshAll() {
    if (refreshTimer) clearTimeout(refreshTimer);
    refreshTimer = setTimeout(() => {
        refreshTimer = null;
        for (const id of panels.keys()) {
            if (rootsFor(id).length > 0) refreshAgentAccessPanel(id);
        }
    }, 150);
}

function ensureListeners(listen) {
    if (listening) return;
    listening = true;
    // An agent switch, a restored backup or another window's change.
    onConfigChange(() => scheduleRefreshAll());
    // A call started or ended, or the queue changed.
    const fn = typeof listen === 'function' ? listen : globalThis.window?.__TAURI__?.event?.listen;
    if (typeof fn === 'function') {
        Promise.resolve()
            .then(() => fn(EVT.AGENT_CALLS_ACTIVITY, () => scheduleRefreshAll()))
            .catch((e) => console.warn('[settings] agent_calls_activity unavailable:', e));
    }
}

/**
 * HTML for the panel: a placeholder filled once the state arrives, or ''
 * when the extension doesn't show it (see {@link showsAgentPanel}).
 *
 * @param {object} module - the page's settings module
 * @param {{ invoke?: Function, listen?: Function }} [deps]
 * @returns {string}
 */
export function renderAgentAccessPanel(module, { invoke, listen } = {}) {
    if (!showsAgentPanel(module)) return '';
    const call = invoke || defaultInvoke();
    if (typeof call !== 'function') return '';
    ensureStyles();
    const id = module._extensionId;
    const prev = panels.get(id);
    panels.set(id, {
        id,
        invoke: call,
        catalog: module._sandbox?.i18nCatalog,
        fallback: module._sandbox?.i18nFallback,
        name: typeof module.title === 'string' ? module.title : id,
        last: null,
        seq: prev?.seq || 0,
        busy: false,
        pending: false,
        feedback: null,
    });
    ensureListeners(listen);
    refreshAgentAccessPanel(id);
    return `<div class="setting-row ext-agent-panel" data-ext-agent="${escapeAttr(id)}" data-sync-ui role="group" aria-label="${escapeAttr(t('settings.manager.ext.agent.section_title'))}"></div>`;
}

/**
 * Re-read one extension's state and redraw its panel.
 *
 * @returns {Promise<object|null>} the panel state, or null
 */
export async function refreshAgentAccessPanel(extensionId) {
    const entry = panels.get(extensionId);
    if (!entry) return null;
    entry.seq += 1;
    const seq = entry.seq;
    let panel;
    try {
        panel = await entry.invoke('get_extension_agent_access', { extensionId });
    } catch (e) {
        if (seq !== entry.seq) return null;
        console.warn(`[ext ${extensionId}] background AI state unavailable:`, e);
        entry.last = null;
        for (const root of rootsFor(extensionId)) {
            root.textContent = '';
            root.append(el('div', 'setting-label', t('settings.manager.ext.agent.section_title')));
            root.append(
                el('div', 'setting-description ext-agent-feedback is-error', errMessage(e))
            );
        }
        return null;
    }
    // A newer refresh started meanwhile; it draws.
    if (seq !== entry.seq) return null;
    if (!panel || typeof panel !== 'object') return null;
    entry.last = panel;
    repaint(entry);
    return panel;
}

function repaint(entry) {
    if (!entry.last) return;
    for (const root of rootsFor(entry.id)) fillPanel(root, entry.last, entry);
}

/** The status line, with "Allow again" where it applies. */
function fillStatus(target, panel, entry, name) {
    const status = panelStatus(panel);
    const agent = { text: cleanText(status.agent, 120), ltr: true };
    const time = formatClock(status.time);
    const text = el('span', 'ext-agent-status-text');
    text.dataset.status = status.kind;
    switch (status.kind) {
        case 'on':
            appendText(text, t('settings.manager.ext.agent.status.on', { agent: SLOT.agent }), {
                [SLOT.agent]: agent,
            });
            break;
        case 'needs_consent':
            text.textContent = t('settings.manager.ext.agent.status.needs_consent');
            break;
        case 'stale':
            appendText(text, t('settings.manager.ext.agent.status.stale', { agent: SLOT.agent }), {
                [SLOT.agent]: agent,
            });
            break;
        case 'paused_failures':
            text.textContent = t('settings.manager.ext.agent.status.paused_failures', {
                time,
                n: FAILURE_BREAKER,
            });
            break;
        case 'paused_tools':
            text.textContent = t('settings.manager.ext.agent.status.paused_tools', { time });
            break;
        case 'paused_stopped':
            text.textContent = t('settings.manager.ext.agent.status.paused_stopped', { time });
            break;
        case 'unavailable':
            appendText(
                text,
                t('settings.manager.ext.agent.status.unavailable', { extension: SLOT.ext }),
                { [SLOT.ext]: { text: name } }
            );
            break;
        default:
            text.textContent = t('settings.manager.ext.agent.status.off');
    }
    target.append(text);
    if (status.allowAgain) {
        const again = el('button', 'setting-button', t('settings.manager.ext.agent.allow_again'));
        again.type = 'button';
        again.dataset.agentAccess = 'allow';
        again.disabled = entry.busy || entry.pending;
        again.addEventListener('click', () => allowFromPanel(entry));
        target.append(again);
    }
}

function fillLog(root, panel, entry) {
    const rows = callRows(panel);
    root.append(el('div', 'ext-agent-log-title', t('settings.manager.ext.agent.log.title')));
    if (rows.length === 0) {
        root.append(
            el(
                'div',
                'setting-description ext-agent-log-empty',
                t('settings.manager.ext.agent.log.empty')
            )
        );
        return;
    }
    const list = el('ul', 'ext-agent-log');
    for (const row of rows) {
        const li = el('li');
        li.dataset.callId = row.callId;
        li.dataset.outcome = row.outcome;
        li.append(
            appendText(
                el('span', 'ext-agent-log-text'),
                t('settings.manager.ext.agent.log.row', {
                    time: formatWhen(row.at),
                    purpose: row.purpose,
                    outcome: agentOutcomeLabel(row.outcome),
                    chars: formatCount(row.chars),
                    agent: SLOT.agent,
                }),
                { [SLOT.agent]: { text: row.agent, ltr: true } }
            )
        );
        if (row.label) li.append(el('span', 'ext-agent-log-label', row.label));
        if (row.viewable) {
            const view = el(
                'button',
                'ext-agent-link',
                t('settings.manager.ext.agent.log.view_sent')
            );
            view.type = 'button';
            view.dataset.agentAccess = `view:${row.callId}`;
            view.addEventListener('click', () => viewSent(entry, row.callId));
            li.append(view);
        }
        list.append(li);
    }
    root.append(list);
    if (rows.some((r) => !r.viewable)) {
        root.append(
            el('div', 'ext-agent-note', t('settings.manager.ext.agent.log.sent_unavailable'))
        );
    }
}

function fillPanel(root, panel, entry) {
    // Keep the focus on the same control across a redraw.
    const active = document.activeElement;
    const focusKey = root.contains(active) ? active?.dataset?.agentAccess : undefined;
    const id = entry.id;
    const name = cleanText(panel.name, 80) || cleanText(entry.name, 80) || id;
    const locked = entry.busy || entry.pending;
    root.textContent = '';
    root.dataset.access = panel.access || 'off';
    root.append(el('div', 'setting-label', t('settings.manager.ext.agent.section_title')));

    // 1. The switch.
    const switchId = `ext-agent-switch-${id}`;
    const descId = `${switchId}-desc`;
    const row = el('div', 'setting-checkbox-row ext-agent-switch-row');
    const box = el('label', 'kage-checkbox');
    const input = document.createElement('input');
    input.type = 'checkbox';
    input.id = switchId;
    input.setAttribute('role', 'switch');
    input.dataset.agentAccess = 'switch';
    input.checked = entry.pending || !!panel.enabled;
    input.setAttribute('aria-checked', String(input.checked));
    input.setAttribute('aria-describedby', descId);
    // Off always works; on needs the grant and a valid manifest block.
    input.disabled = locked || (!panel.available && !panel.enabled);
    input.addEventListener('change', () => onSwitch(entry, input.checked));
    box.append(input);
    const texts = el('div', 'ext-agent-switch-text');
    const label = el('label', 'ext-agent-switch-label');
    label.htmlFor = switchId;
    appendText(label, t('settings.manager.ext.agent.toggle.label', { extension: SLOT.ext }), {
        [SLOT.ext]: { text: name },
    });
    const desc = el('div', 'setting-description', t('settings.manager.ext.agent.toggle.desc'));
    desc.id = descId;
    texts.append(label, desc);
    row.append(box, texts);
    root.append(row);

    // 2. The status line.
    const status = el('div', 'setting-description ext-agent-status');
    status.setAttribute('role', 'status');
    fillStatus(status, panel, entry, name);
    root.append(status);

    if (panel.available) {
        // 3. Usage on this computer.
        const usage = panel.usage || {};
        let text = t('settings.manager.ext.agent.usage', {
            calls: formatCount(usage.calls_today),
            limit: formatCount(usage.calls_limit),
            chars: formatCount(usage.chars_today),
        });
        const resets = formatClock(usage.resets_at);
        if (resets) {
            text += ` · ${t('settings.manager.ext.agent.usage.resets', { time: resets })}`;
        }
        root.append(el('div', 'setting-description ext-agent-usage', text));

        // 4. The daily allowance. A change while the consent is stale
        // would need a fresh OK, so it waits for "Allow again".
        const allowance = el('div', 'ext-agent-allowance');
        const selectId = `ext-agent-allowance-${id}`;
        const selectLabel = el('label', '', t('settings.manager.ext.agent.allowance.label'));
        selectLabel.htmlFor = selectId;
        const select = document.createElement('select');
        select.className = 'setting-select';
        select.id = selectId;
        select.dataset.agentAccess = 'allowance';
        for (const level of LEVELS) {
            const option = document.createElement('option');
            option.value = level;
            option.textContent = t('settings.manager.ext.agent.allowance.option', {
                level: levelName(level),
                calls: formatCount(allowanceFor(panel, level).calls),
            });
            select.append(option);
        }
        select.value = LEVELS.includes(panel.allowance) ? panel.allowance : 'normal';
        select.disabled = locked || !(panel.access === 'on' || panel.access === 'off');
        select.addEventListener('change', () => onAllowance(entry, select.value));
        allowance.append(selectLabel, select);
        root.append(allowance);
    }

    // 5. Stop now, while a call runs or waits.
    if (panel.running || Number(panel.waiting) > 0) {
        const actions = el('div', 'ext-agent-actions');
        const stop = el(
            'button',
            'setting-button icon-button-danger',
            t('settings.manager.ext.agent.stop.btn')
        );
        stop.type = 'button';
        stop.dataset.agentAccess = 'stop';
        stop.disabled = entry.busy;
        stop.addEventListener('click', () => onStop(entry));
        actions.append(stop);
        root.append(actions);
    }

    if (entry.feedback?.text) {
        const note = el(
            'div',
            `setting-description ext-agent-feedback${entry.feedback.error ? ' is-error' : ''}`,
            entry.feedback.text
        );
        note.setAttribute('role', entry.feedback.error ? 'alert' : 'status');
        root.append(note);
    }

    // 6. Recent requests.
    fillLog(root, panel, entry);

    if (focusKey) {
        [...root.querySelectorAll('[data-agent-access]')]
            .find((node) => node.dataset.agentAccess === focusKey)
            ?.focus();
    }
}

function levelName(level) {
    switch (level) {
        case 'low':
            return t('settings.manager.ext.agent.allowance.low');
        case 'high':
            return t('settings.manager.ext.agent.allowance.high');
        default:
            return t('settings.manager.ext.agent.allowance.normal');
    }
}

async function allowFromPanel(entry) {
    const result = await requestAgentAccess(entry.id, { invoke: entry.invoke });
    if (result.message) {
        entry.feedback = { text: result.message, error: true };
        repaint(entry);
    }
}

async function setAccess(entry, args) {
    entry.busy = true;
    entry.feedback = null;
    repaint(entry);
    try {
        await entry.invoke('set_extension_agent_access', { extensionId: entry.id, ...args });
    } catch (e) {
        console.warn(`[ext ${entry.id}] changing background AI failed:`, e);
        entry.feedback = { text: errMessage(e), error: true };
    } finally {
        entry.busy = false;
        await refreshAgentAccessPanel(entry.id);
    }
}

function onSwitch(entry, on) {
    // Turning on always asks first; turning off stops calls at once (Rust).
    if (on) return allowFromPanel(entry);
    return setAccess(entry, { enabled: false });
}

function onAllowance(entry, level) {
    if (!LEVELS.includes(level)) return Promise.resolve();
    // Same switch state, new level. `enabled: true` without a token is
    // accepted only while the consent is current.
    return setAccess(entry, { enabled: entry.last?.access === 'on', allowance: level });
}

async function onStop(entry) {
    entry.busy = true;
    repaint(entry);
    try {
        await entry.invoke('stop_extension_agent_calls', { extensionId: entry.id });
        entry.feedback = { text: t('settings.manager.ext.agent.stop.done') };
    } catch (e) {
        console.warn(`[ext ${entry.id}] stopping background AI failed:`, e);
        entry.feedback = { text: errMessage(e), error: true };
    } finally {
        entry.busy = false;
        await refreshAgentAccessPanel(entry.id);
    }
}

async function viewSent(entry, callId) {
    try {
        const detail = await entry.invoke('get_extension_agent_request_detail', {
            extensionId: entry.id,
            callId,
        });
        await showRequestDetail(detail);
    } catch (e) {
        console.warn(`[ext ${entry.id}] request detail unavailable:`, e);
        entry.feedback = { text: errMessage(e), error: true };
        repaint(entry);
    }
}

/** Forget every mounted panel and listener (tests). */
export function resetAgentAccessPanels() {
    panels.clear();
    consentOpen = false;
    if (refreshTimer) clearTimeout(refreshTimer);
    refreshTimer = null;
    listening = false;
}
