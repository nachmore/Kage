/**
 * Settings → Tool permissions → Audit log, for the agent sign-in
 * (shared/agent-sign-in.js): a `user_ran_command` with `via: sign_in` reads
 * as signing in to the agent, its `started` entry says so, and the other
 * `via` values keep their own summaries.
 */
import { beforeAll, describe, expect, it } from 'vitest';
import * as i18n from '../../ui/js/shared/i18n.js';
import { enCatalog, initEnglish } from '../helpers/sync-i18n.js';

let ToolPermissionsSettingsModule;
let EN;
beforeAll(async () => {
    await initEnglish(i18n);
    EN = enCatalog();
    ({ ToolPermissionsSettingsModule } = await import('../../ui/js/settings/tool-permissions.js'));
});

function row(overrides) {
    const mod = new ToolPermissionsSettingsModule();
    const div = document.createElement('div');
    div.innerHTML = mod._renderAuditRow({
        at: '2026-11-15T10:00:00.000Z',
        event: 'user_ran_command',
        command: '/opt/homebrew/bin/kiro-cli login',
        cwd: '/Users/me',
        via: 'sign_in',
        outcome: 'started',
        session_id: 'sess-1234567890',
        ...overrides,
    });
    return div.textContent;
}

describe('the agent sign-in in the audit log', () => {
    it('reads as signing in, and says when it started', () => {
        const text = row({});
        expect(text).toContain(EN['settings.tool_permissions.audit.summary.user_ran_sign_in'].message);
        expect(text).toContain(EN['settings.tool_permissions.audit.meta.started'].message);
    });

    it('its end entry carries the exit code', () => {
        const text = row({ outcome: 'exited', exit_code: 0 });
        expect(text).toContain(EN['settings.tool_permissions.audit.summary.user_ran_sign_in'].message);
        expect(text).toContain('exit code 0');
    });

    it('other runs keep their summaries', () => {
        expect(row({ via: 'kage', outcome: 'exited', exit_code: 1 })).toContain(
            EN['settings.tool_permissions.audit.summary.user_ran'].message
        );
        expect(row({ via: 'terminal', outcome: 'opened' })).toContain(
            EN['settings.tool_permissions.audit.summary.user_ran_terminal'].message
        );
    });
});
