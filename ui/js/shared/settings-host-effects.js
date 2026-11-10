import { formatBytes } from './tool-utils.js';
import { openViewDenial, runOpenViewEffect } from './extension-host-effects.js';
import { applyManifestI18n } from './extension-manager/i18n.js';

/**
 * `open_view` from a settings action (docs/design/EXTENSION_VIEWS.md §6.1):
 * open one of the extension's views from the Settings window. The checks
 * use the settings sandbox's grant and host features; the extension is the
 * one the renderer was built for. A refusal or failure shows on the
 * action row.
 */
async function openViewFromSettingsAction(renderer, host, setStatus) {
    const sandbox = renderer.sandbox;
    const manifest = renderer.manifest || {};
    const extensionId = renderer.extensionId;
    const name =
        applyManifestI18n(manifest, sandbox?.i18nCatalog, sandbox?.i18nFallback).name ||
        extensionId;
    const capabilities = new Set(sandbox?.capabilities || []);
    // The sandbox's host features are already cut down to its grant, so a
    // missing grant would read as "this window can't open views". Count the
    // window as offering views unless the grant is there and the feature
    // isn't; the grant check then says what's missing.
    const windowOffersViews =
        (Array.isArray(sandbox?.hostFeatures) && sandbox.hostFeatures.includes('views')) ||
        !capabilities.has('views');
    const result = await runOpenViewEffect(host, {
        extensionId,
        source: 'settings',
        invoke: window?.__TAURI__?.core?.invoke,
        check: (effect) =>
            openViewDenial(effect, {
                hostFeatures: windowOffersViews ? ['views'] : [],
                capabilities,
                views: manifest.contributes?.views,
                extensionId,
                extensionName: /^__MSG_.+__$/.test(name) ? extensionId : name,
            }),
    });
    if (!result.ok) setStatus(`❌ ${result.message}`);
}

/**
 * The Background AI consent flow (docs/design/EXTENSION_AGENT_CALLS.md
 * §4.3). Settings registers it (settings/extension-agent-access.js); in any
 * other window it stays null and the effect does nothing.
 */
let agentAccessOpener = null;

/**
 * Register the function the `agent_access` effect calls. It gets
 * `{ extensionId, catalog, fallback, name }` and resolves
 * `{ ok, message? }`.
 */
export function setAgentAccessOpener(fn) {
    agentAccessOpener = typeof fn === 'function' ? fn : null;
}

/**
 * `agent_access` from a settings action: ask the user, in the host's own
 * dialog, to let this page's extension use the agent in the background.
 * Only ever for the extension the renderer was built for: the effect
 * carries no id the host would use. It runs only after a click on the
 * extension's button (`_runAction`), never on its own.
 */
async function agentAccessFromSettingsAction(renderer, ctx, setStatus) {
    const extensionId = renderer.extensionId;
    if (!extensionId || !agentAccessOpener) return;
    const sandbox = renderer.sandbox;
    const name = applyManifestI18n(
        renderer.manifest || {},
        sandbox?.i18nCatalog,
        sandbox?.i18nFallback
    ).name;
    const result = await agentAccessOpener({
        extensionId,
        catalog: sandbox?.i18nCatalog,
        fallback: sandbox?.i18nFallback,
        name: typeof name === 'string' && !/^__MSG_.+__$/.test(name) ? name : '',
    });
    if (result?.ok) {
        // The extension's page shows the new state on its next render.
        if (!result.already) {
            await renderer.refresh?.({ preserveStatusFor: ctx.sourceAction, status: ctx.status });
        }
    } else if (result?.message) {
        setStatus(`❌ ${result.message}`);
    }
}

export async function runSettingsHostSideEffect(renderer, host, ctx, pickFileContents) {
    if (!host || typeof host !== 'object') return;
    const setStatus = ctx.setStatus || (() => {});
    switch (host.type) {
        case 'refresh': {
            // Preserve the status the action just set — the re-render
            // rebuilds the DOM (including the status span), so we
            // re-apply it against the fresh action row afterwards.
            await renderer.refresh({ preserveStatusFor: ctx.sourceAction, status: ctx.status });
            break;
        }
        case 'download': {
            const filename = String(host.filename || 'export.txt');
            const content = String(host.content || '');
            const mime = String(host.mime || 'application/octet-stream');
            const blob = new Blob([content], { type: mime });
            const url = URL.createObjectURL(blob);
            const a = document.createElement('a');
            a.href = url;
            a.download = filename;
            document.body.appendChild(a);
            a.click();
            a.remove();
            setTimeout(() => URL.revokeObjectURL(url), 1000);
            break;
        }
        case 'pick_file': {
            const accept = typeof host.accept === 'string' ? host.accept : '';
            const content = await pickFileContents(accept);
            if (content === null) return; // user cancelled
            try {
                // The user just picked a file: a click lease (EXTENSION_FILE_TEXT §5.2).
                const result = await renderer.sandbox.call(
                    'onFileSelected',
                    {
                        action: host.action || null,
                        filename: content.filename,
                        content: content.text,
                        values: renderer.currentValues(),
                    },
                    { userGesture: true }
                );
                if (result && typeof result === 'object' && result.status) {
                    setStatus(String(result.status));
                }
            } catch (e) {
                renderer.log.warn?.('onFileSelected RPC failed:', e);
                setStatus(`❌ ${e?.message || e}`);
            }
            break;
        }
        case 'link_metadata': {
            // Host-side bridge for the Link Preview extension's
            // cache management. Extensions can't call settings-only
            // Tauri commands directly (and we don't want to widen
            // the extension capability surface for what is, in the
            // end, a tiny shared cache). Instead the extension's
            // `runSettingsAction` returns a `host` effect with
            // op = 'clear' or 'stats' and we run it here. Status
            // text shown on the action row is whatever the host
            // command returns.
            const op = String(host.op || '');
            const invoke = window?.__TAURI__?.core?.invoke;
            if (!invoke) {
                setStatus('❌ host invoke unavailable');
                return;
            }
            try {
                if (op === 'clear') {
                    await invoke('link_metadata_clear_cache');
                    setStatus('✓ Cache cleared.');
                } else if (op === 'stats') {
                    const stats = await invoke('link_metadata_cache_stats');
                    const entries = stats?.entries ?? 0;
                    const bytes = stats?.bytes ?? 0;
                    setStatus(`${entries} URLs · ${formatBytes(bytes) || '0 B'}`);
                } else {
                    setStatus(`❌ unknown link_metadata op: ${op}`);
                }
            } catch (e) {
                setStatus(`❌ ${e?.message || e}`);
            }
            break;
        }
        case 'play_timer_sound': {
            // Preview a built-in or custom timer sound. Audio playback
            // is a host capability because the extension sandbox
            // doesn't share an AudioContext with the parent, and our
            // timer-sounds module lives in the main-window bundle.
            try {
                const { playTimerSound, stopTimerSound, isSoundPlaying } = await import(
                    './timer-sounds.js'
                );
                if (isSoundPlaying()) {
                    stopTimerSound();
                    return;
                }
                const soundId = String(host.soundId || 'two-tone');
                const customPath = host.customPath ? String(host.customPath) : '';
                const repeats = Number(host.repeats) > 0 ? Number(host.repeats) : 1;
                playTimerSound(soundId, customPath, repeats, () => {});
            } catch (e) {
                renderer.log.warn?.('play_timer_sound failed:', e);
            }
            break;
        }
        case 'open_view': {
            await openViewFromSettingsAction(renderer, host, setStatus);
            break;
        }
        case 'agent_access': {
            await agentAccessFromSettingsAction(renderer, ctx, setStatus);
            break;
        }
        default:
            // unknown effect — ignore (forwards-compatible)
            break;
    }
}
