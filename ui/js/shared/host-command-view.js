/**
 * Commands an agent asks Kage to run outside its sandbox — the
 * `run_outside_sandbox` tool of Kage's MCP sidecar, decided in
 * src/host_commands/. The backend sends a prompt to the window that owns
 * the asking session (else the floating window); this module shows it
 * inline, reusing "Run it here"'s confirm pieces (shell-run-view.js): the
 * whole command with hidden characters marked, where it runs, the agent's
 * reason, the warnings, and three answers — Run once, Always allow
 * commands starting with [editable words], Deny. A command with hidden
 * characters gets Copy and Deny only.
 *
 * Every prompt answers only its own request (`answer_host_command` with
 * its id; the backend also checks the window), each answer once. Nothing
 * here is focused when a prompt appears, so keys typed elsewhere can't
 * answer it, and Run takes a single deliberate click. Unanswered prompts
 * are denied by the backend after five minutes.
 *
 * Once approved — or when a rule ran it without asking — the card shows
 * the run (live output, time, Stop) with the same view as "Run it here";
 * the result goes back to the agent as the tool's result, not as a chat
 * message. A card that leaves the page (a re-render, the floating
 * window's reset) while it still matters moves to the window's tray.
 */

import { errMessage } from './error-message.js';
import { EVT } from './events.js';
import { t } from './i18n.js';
import { isPlainShellCommand } from './shell-commands.js';
import { appendShellCommandText } from './shell-command-view.js';
import {
    attachAgentRun,
    buildConfirmCommand,
    buildRiskList,
    failAgentRun,
    finishAgentRun,
    onDeliberateClick,
    revealLongNote,
} from './shell-run-view.js';
import { copyButton, el, icon, layoutChanged, uid } from './tool-call-dom.js';

/** Guard ids (src/host_commands/guard.rs `CredentialHit`) in display order. */
export const GUARD_KINDS = Object.freeze([
    'aws',
    'sandbox_session',
    'ssh',
    'netrc',
    'git',
    'gcloud',
    'cloud',
    'keychain',
    'env_dump',
    'credential_env',
    'hidden_glob',
]);

const REASON_MAX = 2000;
/** How often cards that left the page are put back (and the expiry line refreshed). */
const REHOME_MS = 1000;

/** requestId → card state. */
const cards = new Map();
let ui = null;

function tauri() {
    const api = window.__TAURI__;
    if (typeof api?.core?.invoke !== 'function') throw new Error('Tauri API unavailable');
    return api;
}

function button(className, label, iconName) {
    const btn = el('button', `shell-cmd-copy ${className}`);
    btn.type = 'button';
    if (iconName) btn.append(icon(iconName, 'shell-cmd-icon', 12));
    const span = el('span', 'shell-cmd-copy-label');
    span.dir = 'auto';
    span.textContent = label;
    btn.append(span);
    return btn;
}

/** The guard's warning: what it found, in words. */
export function guardText(kinds) {
    const known = GUARD_KINDS.filter((k) => kinds?.includes(k));
    if (!known.length) return '';
    const names = known.map((k) => t(`shared.host_command.guard_kind.${k}`)).join(', ');
    return t('shared.host_command.guard', { kinds: names });
}

function titleText(p, auto) {
    if (auto) {
        return p.agent
            ? t('shared.host_command.auto_title', { agent: p.agent })
            : t('shared.host_command.auto_title_unknown');
    }
    return p.agent
        ? t('shared.host_command.title', { agent: p.agent })
        : t('shared.host_command.title_unknown');
}

function minutesLeft(expiresAt) {
    const ms = Number(expiresAt) - Date.now();
    return Math.max(1, Math.ceil(ms / 60000));
}

/** The card's top: title, origin notes, the agent's reason, the command and where it runs. */
function buildHead(card, p, auto) {
    const head = el('div', 'host-cmd-head');
    const title = el('div', 'shell-run-confirm-title host-cmd-title');
    title.id = uid('kage-host-cmd-title');
    const titleText_ = el('span');
    titleText_.dir = 'auto';
    titleText_.textContent = titleText(p, auto);
    title.append(icon('shield', 'shell-cmd-icon', 13), titleText_);
    card.el.setAttribute('aria-labelledby', title.id);
    head.append(title);

    if (!p.matched || p.background) {
        const origin = el('p', 'host-cmd-origin');
        origin.dir = 'auto';
        origin.textContent = p.background
            ? t('shared.host_command.background')
            : t('shared.host_command.unmatched');
        head.append(origin);
    }

    if (p.reason) {
        const reason = el('p', 'host-cmd-reason');
        const label = el('span', 'host-cmd-reason-label');
        label.textContent = t('shared.host_command.reason_label');
        const text = el('span', 'host-cmd-reason-text');
        // The agent's words: shown as text, in their own direction.
        text.dir = 'auto';
        const reasonText = String(p.reason);
        text.textContent =
            reasonText.length > REASON_MAX ? `${reasonText.slice(0, REASON_MAX)}…` : reasonText;
        reason.append(label, ' ', text);
        head.append(reason);
    }

    const box = buildConfirmCommand(p.command, (parent, command) =>
        appendShellCommandText(parent, command)
    );
    card.box = box;
    head.append(box.pre, box.longNote);

    const where = el('div', 'shell-run-confirm-where');
    const label = el('span');
    label.textContent = t('shared.shell.sandbox.confirm.cwd_label');
    const path = el('code', 'shell-run-confirm-path');
    path.dir = 'ltr';
    path.textContent = p.cwd || '';
    where.append(label, ' ', path);
    if (p.shell) {
        const shell = el('span', 'shell-run-confirm-shell');
        shell.textContent = t('shared.shell.sandbox.confirm.shell', { shell: p.shell });
        where.append(' ', shell);
    }
    head.append(where);
    return head;
}

function newCard(p) {
    const root = el('div', 'shell-run-confirm host-cmd-card');
    root.setAttribute('role', 'group');
    root.dir = 'auto';
    root.dataset.requestId = p.requestId;
    const card = {
        id: p.requestId,
        el: root,
        payload: p,
        phase: 'prompt',
        run: null,
        status: null,
        actions: null,
        runSlot: el('div', 'shell-run-slot host-cmd-run'),
        box: null,
    };
    cards.set(p.requestId, card);
    return card;
}

function setStatus(card, text, kind = '') {
    if (!card.status) return;
    card.status.textContent = text || '';
    card.status.hidden = !text;
    card.status.dataset.kind = kind;
}

function placeCard(card) {
    let placed = false;
    try {
        placed = !!ui?.place?.(card.el, card.payload);
    } catch (e) {
        console.warn('[host-command] place failed:', e);
    }
    if (!placed || !card.el.isConnected) ui?.tray?.append(card.el);
    revealLongNote(card.box);
    layoutChanged(card.el);
}

/** Show the prompt for one request (once, however often it's sent). */
export function showHostCommandPrompt(p) {
    if (!p || typeof p.requestId !== 'string' || typeof p.command !== 'string') return null;
    if (cards.has(p.requestId)) return cards.get(p.requestId);
    const card = newCard(p);
    const root = card.el;
    root.append(buildHead(card, p, false));

    const plain = p.plain !== false && isPlainShellCommand(p.command);
    const warn = el('p', 'shell-run-confirm-warn');
    warn.dir = 'auto';
    warn.textContent = plain
        ? t('shared.shell.sandbox.confirm.warning')
        : t('shared.shell.sandbox.warn_hidden');
    root.append(warn);

    const guard = guardText(p.guard);
    if (guard) {
        const g = el('p', 'host-cmd-guard');
        g.dir = 'auto';
        g.textContent = guard;
        root.append(g);
    }
    root.append(buildRiskList(p.command));

    const expiry = el('p', 'host-cmd-expiry');
    expiry.dir = 'auto';
    card.expiry = expiry;
    updateExpiry(card);
    root.append(expiry);

    const status = el('p', 'shell-run-confirm-error host-cmd-status');
    status.dir = 'auto';
    status.setAttribute('role', 'status');
    status.hidden = true;
    card.status = status;

    const actions = el('div', 'shell-run-confirm-actions host-cmd-actions');
    card.actions = actions;
    const deny = button('shell-run-cancel host-cmd-deny', t('shared.host_command.deny'));
    deny.addEventListener('click', (e) => {
        e.stopPropagation();
        answer(card, 'deny');
    });
    actions.append(deny);
    if (!plain) {
        // Hidden characters: Kage won't run it. Copy it to look closer.
        actions.prepend(
            copyButton(
                () => t('shared.shell.copy_command'),
                () => p.command
            )
        );
    } else {
        if (p.canAlways && typeof p.suggestedPrefix === 'string') {
            const always = el('div', 'host-cmd-always');
            const alwaysBtn = button('host-cmd-always-btn', t('shared.host_command.always'));
            const input = el('input', 'host-cmd-prefix');
            input.type = 'text';
            input.dir = 'ltr';
            input.spellcheck = false;
            input.autocomplete = 'off';
            input.value = p.suggestedPrefix;
            input.setAttribute('aria-label', t('shared.host_command.prefix_label'));
            input.addEventListener('keydown', (e) => {
                // Enter in the field must not approve anything.
                if (e.key === 'Enter') e.preventDefault();
                e.stopPropagation();
            });
            onDeliberateClick(alwaysBtn, () => answer(card, 'always', input.value));
            always.append(alwaysBtn, input);
            card.prefix = input;
            actions.append(always);
        }
        const run = button(
            'shell-run-go host-cmd-run-once',
            t('shared.host_command.run_once'),
            'play'
        );
        onDeliberateClick(run, () => answer(card, 'run_once'));
        actions.append(run);
    }
    root.append(status, actions, card.runSlot);
    placeCard(card);
    return card;
}

function updateExpiry(card) {
    if (!card.expiry) return;
    if (card.phase !== 'prompt' || !card.payload.expiresAt) {
        card.expiry.hidden = true;
        return;
    }
    card.expiry.hidden = false;
    card.expiry.textContent = t('shared.host_command.expires', {
        minutes: minutesLeft(card.payload.expiresAt),
    });
}

function setButtonsDisabled(card, disabled) {
    for (const b of card.actions?.querySelectorAll('button, input') || []) b.disabled = disabled;
}

/** Send this card's answer (its own request id only). */
async function answer(card, decision, prefix) {
    if (card.phase !== 'prompt' || card.answering) return;
    card.answering = true;
    setButtonsDisabled(card, true);
    setStatus(card, '');
    try {
        await tauri().core.invoke('answer_host_command', {
            requestId: card.id,
            decision,
            prefix: decision === 'always' ? String(prefix ?? '') : null,
        });
    } catch (e) {
        card.answering = false;
        // Timed out or withdrawn meanwhile: nothing to retry.
        if (e?.kind === 'cancelled') {
            close(card, t('shared.host_command.closed'));
            return;
        }
        setButtonsDisabled(card, false);
        setStatus(card, t('shared.host_command.answer_failed', { error: errMessage(e) }), 'error');
        return;
    }
    card.answering = false;
    if (decision === 'deny') {
        close(card, t('shared.host_command.denied'));
        return;
    }
    // The run may already have started (its update can beat this reply).
    if (card.phase !== 'prompt') return;
    card.phase = 'approved';
    card.actions.remove();
    updateExpiry(card);
    card.startingShown = decision !== 'always';
    setStatus(
        card,
        decision === 'always'
            ? t('shared.host_command.rule_added', {
                  rule: String(prefix ?? '')
                      .trim()
                      .replace(/\s+/g, ' '),
              })
            : t('shared.host_command.starting'),
        'info'
    );
    layoutChanged(card.el);
}

/** The request ended without running: say why, drop the buttons. */
function close(card, text) {
    card.phase = 'closed';
    card.actions?.remove();
    updateExpiry(card);
    setStatus(card, text, 'info');
    if (!card.el.isConnected) placeCard(card);
    layoutChanged(card.el);
}

const CLOSE_REASON = Object.freeze({
    expired: () => t('shared.host_command.expired'),
    withdrawn: () => t('shared.host_command.withdrawn'),
    busy: () => t('shared.host_command.busy'),
    shutdown: () => t('shared.host_command.shutdown'),
});

/** A card for a run that started without a prompt (a rule or allow-all). */
function autoCard(u) {
    const card = newCard({ ...u, matched: true });
    card.phase = 'approved';
    card.el.classList.add('host-cmd-auto');
    card.el.append(buildHead(card, u, true));
    const status = el('p', 'host-cmd-status');
    status.dir = 'auto';
    status.setAttribute('role', 'status');
    card.status = status;
    card.el.append(status, card.runSlot);
    setStatus(
        card,
        u.approvedBy === 'rule' && u.rule
            ? t('shared.host_command.by_rule', { rule: u.rule })
            : t('shared.host_command.by_allow_all'),
        'info'
    );
    placeCard(card);
    return card;
}

/** A `host_command_update` from the backend. */
export function onHostCommandUpdate(u) {
    if (!u || typeof u.requestId !== 'string') return;
    let card = cards.get(u.requestId);
    switch (u.phase) {
        case 'started': {
            if (!card) {
                if (typeof u.command !== 'string') return;
                card = autoCard(u);
            }
            if (card.run || card.phase === 'closed') return;
            card.actions?.remove();
            // "Starting…" gives way to the run itself; a rule just made,
            // or the rule that ran it, stays said.
            if (card.phase === 'prompt' || card.startingShown) setStatus(card, '');
            card.phase = 'running';
            updateExpiry(card);
            card.run = attachAgentRun({
                runId: u.runId,
                command: u.command ?? card.payload.command,
                cwd: u.cwd ?? card.payload.cwd,
                timeoutSeconds: u.timeoutSeconds ?? card.payload.timeoutSeconds,
                card: card.el,
                slot: card.runSlot,
                layout: (node) => layoutChanged(node),
            });
            layoutChanged(card.el);
            break;
        }
        case 'finished':
            if (card?.run) {
                card.phase = 'done';
                finishAgentRun(card.run, u.result);
            }
            break;
        case 'failed':
            if (card?.run) {
                card.phase = 'done';
                failAgentRun(card.run, u.error || '');
            }
            break;
        case 'closed':
            if (card && card.phase !== 'done' && card.phase !== 'closed') {
                close(card, (CLOSE_REASON[u.reason] || CLOSE_REASON.expired)());
            }
            break;
        default:
    }
}

/** Put cards that still matter back in the page; refresh expiry lines. */
export function rehomeHostCommandCards() {
    for (const card of cards.values()) {
        updateExpiry(card);
        const live =
            card.phase === 'prompt' || card.phase === 'approved' || card.phase === 'running';
        if (live && !card.el.isConnected && ui?.tray) {
            ui.tray.append(card.el);
            layoutChanged(card.el);
        }
    }
}

/**
 * Wire this window up. `tray`: where cards go when `place(cardEl,
 * payload)` doesn't put them somewhere better (return true when it did).
 * Re-shows the prompts still waiting for this window (after a reload).
 */
export async function installHostCommandUi({ tray, place } = {}) {
    disposeHostCommandUi();
    ui = { tray: tray || null, place: place || null, unlisten: [], timer: null };
    const api = window.__TAURI__;
    const listen = api?.event?.listen;
    if (typeof listen === 'function') {
        const offPrompt = await listen(EVT.HOST_COMMAND_PROMPT, (e) =>
            showHostCommandPrompt(e?.payload)
        );
        const offUpdate = await listen(EVT.HOST_COMMAND_UPDATE, (e) =>
            onHostCommandUpdate(e?.payload)
        );
        ui?.unlisten.push(offPrompt, offUpdate);
    }
    if (ui) ui.timer = setInterval(rehomeHostCommandCards, REHOME_MS);
    try {
        const waiting = await api?.core?.invoke?.('pending_host_commands');
        if (Array.isArray(waiting)) for (const p of waiting) showHostCommandPrompt(p);
    } catch (e) {
        console.warn('[host-command] could not fetch waiting prompts:', errMessage(e));
    }
}

/** Stop listening and forget every card (tests, window teardown). */
export function disposeHostCommandUi() {
    if (ui) {
        clearInterval(ui.timer);
        for (const off of ui.unlisten) {
            try {
                off?.();
            } catch {}
        }
    }
    ui = null;
    cards.clear();
}
