import { localizeManifestForPrompt } from './extension-manager.js';
import { showPermissionPrompt } from './permission-prompt.js';

/**
 * Stages an extension and commits it only after capability approval.
 * A declined approval removes staged files before they can be loaded.
 */
export async function runStagedExtensionInstall(invoke, stager, { onSuccess } = {}) {
    let priorGrant = null;
    try {
        const cfg = await invoke('get_config');
        priorGrant = cfg?.extension_grants || {};
    } catch {
        priorGrant = {};
    }

    const item = await stager();
    const manifest = item?.manifest;
    if (!manifest?.id) throw new Error('install returned no manifest');

    const existing = priorGrant[manifest.id] || null;
    const previouslyGranted = Array.isArray(existing?.granted) ? existing.granted : [];
    const requested = Array.isArray(manifest.permissions) ? manifest.permissions : [];
    const grantedSet = new Set(previouslyGranted);
    const expandsCaps = requested.some((cap) => !grantedSet.has(cap));

    const decision =
        existing && !expandsCaps
            ? { approved: true, granted: requested }
            : await showPermissionPrompt(await localizeManifestForPrompt(invoke, manifest), {
                  isUpgrade: !!existing,
                  previouslyGranted,
              });

    if (!decision.approved) {
        try {
            await invoke('uninstall_extension', {
                id: manifest.id,
                kind: manifest.type || 'extension',
            });
        } catch (error) {
            console.warn('Rollback uninstall failed:', error);
        }
        return { cancelled: true };
    }

    await invoke('commit_extension_install', {
        extensionId: manifest.id,
        granted: decision.granted,
        approvedVersion: manifest.version || '',
    });
    if (onSuccess) await onSuccess();
    return { cancelled: false, item };
}
