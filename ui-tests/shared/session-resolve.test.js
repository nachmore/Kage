/**
 * Tests for ui/js/shared/session-resolve.js — the helper that fetches a
 * window's pinned ACP session id for handing to agent commands, swallowing
 * "no session / lookup failed" to null.
 *
 * The contract:
 *   - returns the session id when get_window_session resolves with one
 *   - returns null when get_window_session resolves null (window unpinned)
 *   - returns null (never throws) when get_window_session rejects
 *
 * The backend agent commands take Option<String> and create a real
 * session on null, so the swallow-to-null is intentional — but it must
 * stay in one place. If this drifts back to a raw call, a null sessionId
 * crashes the command at arg deserialization ("expected a string").
 */

import { describe, it, expect, vi } from 'vitest';
import { getWindowSessionOrNull } from '../../ui/js/shared/session-resolve.js';

describe('getWindowSessionOrNull', () => {
    it('passes the label through to get_window_session', async () => {
        const invoke = vi.fn().mockResolvedValue('sess-123');
        const id = await getWindowSessionOrNull(invoke, 'floating');
        expect(invoke).toHaveBeenCalledWith('get_window_session', { label: 'floating' });
        expect(id).toBe('sess-123');
    });

    it('returns null when the window has no pinned session', async () => {
        const invoke = vi.fn().mockResolvedValue(null);
        expect(await getWindowSessionOrNull(invoke, 'main')).toBeNull();
    });

    it('swallows a rejected lookup to null instead of throwing', async () => {
        const invoke = vi.fn().mockRejectedValue(new Error('state not managed'));
        await expect(getWindowSessionOrNull(invoke, 'main')).resolves.toBeNull();
    });
});
