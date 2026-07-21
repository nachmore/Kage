import { formatBytes } from './tool-utils.js';

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
                const result = await renderer.sandbox.call('onFileSelected', {
                    action: host.action || null,
                    filename: content.filename,
                    content: content.text,
                    values: renderer.save(),
                });
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
        default:
            // unknown effect — ignore (forwards-compatible)
            break;
    }
}
