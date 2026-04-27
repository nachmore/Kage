/**
 * Bridges the ES module world (sandbox host + settings renderer) to
 * the plain-script `manager.js` used by settings.html.
 *
 * Exposes a single `window.__kageSettingsSandbox` facade with the
 * functions the manager needs:
 *   - createPool(invoke): create a fresh ExtensionSandboxPool
 *   - normalize(permissions, id): normalize a manifest's permissions array
 *   - renderSchema(args): render a settings schema into a host container
 */

import { ExtensionSandboxPool } from '../shared/extension-sandbox-host.js';
import { renderSchema } from '../shared/settings-renderer.js';
import { normalizePermissions } from '../shared/extension-permissions.js';

window.__kageSettingsSandbox = {
    createPool(invoke) {
        return new ExtensionSandboxPool(invoke);
    },
    normalize(permissions, id) {
        return normalizePermissions(permissions, id);
    },
    renderSchema(args) {
        return renderSchema(args);
    },
};
