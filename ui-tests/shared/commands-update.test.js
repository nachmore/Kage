import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// Echo the key (plus vars) so assertions can check which message showed and
// what it was filled with, independent of the EN wording.
vi.mock('../../ui/js/shared/i18n.js', async (importOriginal) => ({
    ...(await importOriginal()),
    t: (key, vars) => (vars ? `${key}${JSON.stringify(vars)}` : key),
}));

const { matchCommands, isUpdateCommandRunning } = await import(
    '../../ui/js/shared/commands.js'
);

function updateCommand() {
    const cmd = matchCommands('>update').find((c) => c.name === 'update');
    expect(cmd).toBeTruthy();
    return cmd;
}

// Hand-rolled invoke, same style as commands.test.js: records every call and
// answers from a per-command handler.
function makeInvoke(handlers) {
    const calls = [];
    const invoke = async (name, args) => {
        calls.push(name);
        const handler = handlers[name];
        if (!handler) throw new Error(`unexpected invoke: ${name}`);
        return handler(args);
    };
    return { invoke, calls };
}

let responses;
const onResponse = (e) => responses.push(e.detail);

beforeEach(() => {
    responses = [];
    document.addEventListener('kage-show-response', onResponse);
});

afterEach(() => {
    document.removeEventListener('kage-show-response', onResponse);
});

describe('>update', () => {
    it('is offered for >up', () => {
        expect(matchCommands('>up').some((c) => c.name === 'update')).toBe(true);
    });

    it('says up to date and does not install when no update is available', async () => {
        const { invoke, calls } = makeInvoke({
            check_for_update: async () => ({ current_version: '1.0.0', available_version: null }),
        });
        await updateCommand().execute(invoke);
        expect(calls).toEqual(['check_for_update']);
        expect(responses).toEqual([
            'command.update.checking',
            'command.update.up_to_date{"version":"1.0.0"}',
        ]);
        expect(isUpdateCommandRunning()).toBe(false);
    });

    it('installs once, after the check, when an update is available', async () => {
        const { invoke, calls } = makeInvoke({
            check_for_update: async () => ({ current_version: '1.0.0', available_version: '1.2.0' }),
            download_and_install_update: async () => {
                // The guard is still held while the install runs.
                expect(isUpdateCommandRunning()).toBe(true);
            },
        });
        await updateCommand().execute(invoke);
        await vi.waitFor(() => expect(isUpdateCommandRunning()).toBe(false));
        expect(calls).toEqual(['check_for_update', 'download_and_install_update']);
        expect(responses).toEqual([
            'command.update.checking',
            'command.update.installing{"version":"1.2.0"}',
        ]);
    });

    it('returns once the install starts, holding the guard until it settles', async () => {
        // The launcher clears its input when a command returns; awaiting a
        // minutes-long download held `>update` in the input, then wiped
        // whatever the user typed meanwhile.
        let finishInstall;
        const { invoke, calls } = makeInvoke({
            check_for_update: async () => ({ current_version: '1.0.0', available_version: '1.2.0' }),
            download_and_install_update: () =>
                new Promise((_resolve, reject) => {
                    finishInstall = reject;
                }),
        });
        await updateCommand().execute(invoke);
        expect(calls).toEqual(['check_for_update', 'download_and_install_update']);
        expect(isUpdateCommandRunning()).toBe(true);

        // Still installing: a second >update in this window calls nothing.
        await updateCommand().execute(invoke);
        expect(responses.at(-1)).toBe('command.update.already_running');
        expect(calls).toHaveLength(2);

        finishInstall({ kind: 'internal', message: 'Update install failed. (boom)' });
        await vi.waitFor(() => expect(isUpdateCommandRunning()).toBe(false));
        expect(responses.at(-1)).toBe(
            'command.update.install_failed: Update install failed. (boom)'
        );
    });

    it('says already running when another window holds the install slot', async () => {
        const { invoke } = makeInvoke({
            check_for_update: async () => ({ current_version: '1.0.0', available_version: '1.2.0' }),
            download_and_install_update: async () => {
                throw {
                    kind: 'internal',
                    key: 'errors.update.install_in_progress',
                    message: 'An update is already downloading and installing.',
                };
            },
        });
        await updateCommand().execute(invoke);
        await vi.waitFor(() => expect(isUpdateCommandRunning()).toBe(false));
        expect(responses).toEqual([
            'command.update.checking',
            'command.update.installing{"version":"1.2.0"}',
            'command.update.already_running',
        ]);
    });

    it('shows the AppError message when the check fails, and does not install', async () => {
        const { invoke, calls } = makeInvoke({
            check_for_update: async () => {
                throw { kind: 'internal', message: 'Check failed: offline' };
            },
        });
        await updateCommand().execute(invoke);
        expect(calls).toEqual(['check_for_update']);
        expect(responses.at(-1)).toBe('command.update.check_failed: Check failed: offline');
        expect(responses.join('\n')).not.toContain('[object Object]');
        expect(isUpdateCommandRunning()).toBe(false);
    });

    it('shows the AppError message when the install fails, then allows a retry', async () => {
        let installAttempts = 0;
        const { invoke, calls } = makeInvoke({
            check_for_update: async () => ({ current_version: '1.0.0', available_version: '1.2.0' }),
            download_and_install_update: async () => {
                installAttempts += 1;
                throw { kind: 'internal', message: 'Network error while downloading the update.' };
            },
        });
        await updateCommand().execute(invoke);
        await vi.waitFor(() => expect(isUpdateCommandRunning()).toBe(false));
        expect(responses.at(-1)).toBe(
            'command.update.install_failed: Network error while downloading the update.'
        );
        expect(responses.join('\n')).not.toContain('[object Object]');

        // The guard was released, so a second >update runs the whole flow again.
        await updateCommand().execute(invoke);
        await vi.waitFor(() => expect(isUpdateCommandRunning()).toBe(false));
        expect(installAttempts).toBe(2);
        expect(calls).toEqual([
            'check_for_update',
            'download_and_install_update',
            'check_for_update',
            'download_and_install_update',
        ]);
    });

    it('a second >update while one is in flight says it is already updating', async () => {
        let resolveCheck;
        const { invoke, calls } = makeInvoke({
            check_for_update: () =>
                new Promise((resolve) => {
                    resolveCheck = resolve;
                }),
            download_and_install_update: async () => {},
        });
        const first = updateCommand().execute(invoke);
        expect(isUpdateCommandRunning()).toBe(true);

        await updateCommand().execute(invoke);
        expect(responses.at(-1)).toBe('command.update.already_running');
        expect(calls).toEqual(['check_for_update']);

        resolveCheck({ current_version: '1.0.0', available_version: '1.2.0' });
        await first;
        await vi.waitFor(() => expect(isUpdateCommandRunning()).toBe(false));
        expect(calls).toEqual(['check_for_update', 'download_and_install_update']);
    });
});
