import { localizeManifestForPrompt } from './extension-manager.js';
import { showPermissionPrompt } from './permission-prompt.js';

/**
 * Stages an extension and commits it only after capability approval.
 * A declined approval removes staged files before they can be loaded.
 *
 * For an upgrade the backend parks the new version beside the live one, so
 * the decline path's `uninstall_extension` (with `rollback: true`) only
 * discards the parked files — the working version, its settings, grant and
 * data stay put. The backend (not this caller) decides which case applies,
 * keyed on whether a parked upgrade exists. Without `rollback` the command
 * is always a real uninstall, so only this decline path may pass it.
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

    const rollback = async () => {
        try {
            await invoke('uninstall_extension', {
                id: manifest.id,
                kind: manifest.type || 'extension',
                rollback: true,
            });
        } catch (error) {
            console.warn('Rollback uninstall failed:', error);
        }
    };

    let decision;
    try {
        decision =
            existing && !expandsCaps
                ? { approved: true, granted: requested }
                : await showPermissionPrompt(
                      // A parked upgrade's new name/description live in its
                      // parked dir, not the old live one.
                      await localizeManifestForPrompt(invoke, manifest, { staged: true }),
                      {
                          isUpgrade: !!existing,
                          previouslyGranted,
                      }
                  );
    } catch (error) {
        // Never leave staged files behind a prompt that didn't resolve.
        await rollback();
        throw error;
    }

    if (!decision.approved) {
        await rollback();
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
