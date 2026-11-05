import { describe, expect, it } from 'vitest';
import { HOST_FEATURES, invokeError } from '../../ui/js/extension-sandbox/runtime/host-api.js';

describe('sandbox runtime host API', () => {
    it('advertises the settings copy action, structured invoke errors, and manifest-default config', () => {
        expect(HOST_FEATURES).toContain('settings.copy_action');
        expect(HOST_FEATURES).toContain('invoke.error_kind');
        expect(HOST_FEATURES).toContain('config.manifest_defaults');
        expect(Object.isFrozen(HOST_FEATURES)).toBe(true);
    });

    it('builds an Error carrying the AppError kind and key', () => {
        const err = invokeError({
            error: 'OAuth listener was cancelled before a callback arrived',
            errorKind: 'cancelled',
            errorKey: 'errors.passthrough',
        });
        expect(err).toBeInstanceOf(Error);
        expect(err.message).toBe('OAuth listener was cancelled before a callback arrived');
        expect(err.kind).toBe('cancelled');
        expect(err.key).toBe('errors.passthrough');
    });

    it('leaves kind / key unset for plain host errors', () => {
        const err = invokeError({ error: "Extension 'x': missing capability 'oauth'" });
        expect(err.message).toMatch(/missing capability/);
        expect(err).not.toHaveProperty('kind');
        expect(err).not.toHaveProperty('key');
    });
});
