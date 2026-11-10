/**
 * The launcher's Background AI chip (docs/design/EXTENSION_AGENT_CALLS.md
 * §4.4): while an extension's background call runs, a small chip says
 * "🌙 {extension} is using your agent", plus "+{n} waiting". Clicking it
 * shows Stop and Settings.
 *
 * Driven by `agent_calls_activity`, which only first-party windows get.
 * Calls start only while the user is away, so the chip is rarely seen;
 * the panel log is the real record. No mascot pose, no notification, no
 * sound.
 *
 * The chip has its own row at the bottom of the bubble, at the inline
 * end, right above the toolbar (and its context indicator) when the
 * toolbar is shown. The toolbar is off by default, so the chip can't live
 * inside it.
 *
 * The extension's name is the host's (Rust resolves it from the manifest)
 * and goes in as text only.
 */

import { EVT } from '../shared/events.js';
import { t } from '../shared/i18n.js';
import { waitForTauri } from '../shared/tauri-init.js';

/** Longest name shown, in characters. */
const MAX_NAME_CHARS = 60;

/** Control and bidi override characters: never shown from a name. */
function isUnsafe(code) {
    return (
        code < 0x20 ||
        (code >= 0x7f && code <= 0x9f) ||
        code === 0x061c ||
        code === 0x200e ||
        code === 0x200f ||
        (code >= 0x202a && code <= 0x202e) ||
        (code >= 0x2066 && code <= 0x2069)
    );
}

function cleanName(value) {
    if (typeof value !== 'string') return '';
    const chars = Array.from(value.trim()).filter((c) => !isUnsafe(c.codePointAt(0)));
    return chars.length > MAX_NAME_CHARS
        ? `${chars.slice(0, MAX_NAME_CHARS - 1).join('')}…`
        : chars.join('');
}

/**
 * What the chip shows for an `agent_calls_activity` payload, or null when
 * no call runs. Pure.
 *
 * @param {{ running?: Array<{ extension_id: string, name?: string }>,
 *   waiting?: number }} payload
 * @returns {{ extensionId: string, name: string, waiting: number } | null}
 */
export function chipState(payload) {
    const running = Array.isArray(payload?.running) ? payload.running : [];
    const call = running.find((r) => r && typeof r.extension_id === 'string' && r.extension_id);
    if (!call) return null;
    const waiting = Number(payload?.waiting);
    return {
        extensionId: call.extension_id,
        name: cleanName(call.name) || call.extension_id,
        waiting: Number.isInteger(waiting) && waiting > 0 ? waiting : 0,
    };
}

export class AgentCallIndicator {
    /**
     * @param {HTMLElement} root - the chip's row (`#agentCallIndicator`)
     * @param {{ invoke: Function }} deps
     */
    constructor(root, { invoke }) {
        this.root = root;
        this.invoke = invoke;
        this.state = null;
        this.open = false;
        this._build();
        this.render();
    }

    _build() {
        const root = this.root;
        root.textContent = '';
        root.hidden = true;
        const chip = document.createElement('button');
        chip.type = 'button';
        chip.className = 'agent-call-chip';
        chip.setAttribute('aria-expanded', 'false');
        const icon = document.createElement('span');
        icon.className = 'agent-call-chip-icon';
        icon.setAttribute('aria-hidden', 'true');
        icon.textContent = '🌙';
        const text = document.createElement('span');
        text.className = 'agent-call-chip-text';
        const waiting = document.createElement('span');
        waiting.className = 'agent-call-chip-waiting';
        chip.append(icon, text, waiting);
        chip.addEventListener('click', () => this.toggle());

        const actions = document.createElement('span');
        actions.className = 'agent-call-actions';
        actions.id = `${root.id || 'agentCallIndicator'}Actions`;
        actions.hidden = true;
        chip.setAttribute('aria-controls', actions.id);
        const stop = document.createElement('button');
        stop.type = 'button';
        stop.className = 'agent-call-action';
        stop.dataset.agentCall = 'stop';
        stop.addEventListener('click', () => this.stop());
        const settings = document.createElement('button');
        settings.type = 'button';
        settings.className = 'agent-call-action';
        settings.dataset.agentCall = 'settings';
        settings.addEventListener('click', () => this.openSettings());
        actions.append(stop, settings);

        root.append(chip, actions);
        this.els = { chip, text, waiting, actions, stop, settings };
    }

    /** New activity from the backend. */
    update(payload) {
        const next = chipState(payload);
        // A different call (or none): fold the menu away.
        if (!next || next.extensionId !== this.state?.extensionId) this.open = false;
        this.state = next;
        this.render();
    }

    toggle() {
        if (!this.state) return;
        this.open = !this.open;
        this.render();
    }

    render() {
        const { chip, text, waiting, actions, stop, settings } = this.els;
        const s = this.state;
        this.root.hidden = !s;
        if (!s) {
            this.open = false;
            actions.hidden = true;
            chip.setAttribute('aria-expanded', 'false');
            return;
        }
        text.textContent = t('floating.agent_call.chip', { extension: s.name });
        waiting.textContent =
            s.waiting > 0 ? t('floating.agent_call.waiting', { n: s.waiting }) : '';
        waiting.hidden = s.waiting === 0;
        stop.textContent = t('floating.agent_call.stop');
        settings.textContent = t('floating.agent_call.settings');
        actions.hidden = !this.open;
        chip.setAttribute('aria-expanded', String(this.open));
    }

    /** **Stop**: cancel this extension's calls and pause it for an hour. */
    async stop() {
        const id = this.state?.extensionId;
        if (!id) return;
        this.open = false;
        this.render();
        try {
            await this.invoke('stop_extension_agent_calls', { extensionId: id });
        } catch (e) {
            console.warn('[floating] stopping background AI failed:', e);
        }
    }

    /** **Settings**: the extension's page, where the Background AI panel is. */
    async openSettings() {
        const id = this.state?.extensionId;
        if (!id) return;
        this.open = false;
        this.render();
        try {
            await this.invoke('open_settings_window', { section: `ext-${id}` });
        } catch (e) {
            console.warn('[floating] opening Settings failed:', e);
        }
    }
}

/**
 * Show the chip in `root` and keep it current.
 *
 * @param {{ root: HTMLElement, invoke: Function, listen: Function }} deps
 * @returns {Promise<AgentCallIndicator>}
 */
export async function installAgentCallIndicator({ root, invoke, listen }) {
    const indicator = new AgentCallIndicator(root, { invoke });
    await listen(EVT.AGENT_CALLS_ACTIVITY, (event) => indicator.update(event?.payload));
    // A language change re-renders the text.
    document.addEventListener('kage:i18n-changed', () => indicator.render());
    return indicator;
}

// The floating window: install once its row and Tauri are there.
if (typeof document !== 'undefined') {
    const root = document.getElementById('agentCallIndicator');
    if (root) {
        waitForTauri(({ invoke, listen }) => {
            installAgentCallIndicator({ root, invoke, listen }).catch((e) =>
                console.warn('[floating] background AI chip unavailable:', e)
            );
        });
    }
}
