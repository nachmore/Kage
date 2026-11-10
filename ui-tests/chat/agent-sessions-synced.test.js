/**
 * Chats from the user's other computers (docs/design/SYNC.md §4.6): listed
 * under "On your other computers" with the computer's name, opened
 * read-only, and **Continue here** starts a new chat with the transcript
 * attached.
 */
import { describe, it, expect, beforeAll, afterEach, vi } from 'vitest';
import { marked } from 'marked';
import { initEnglish } from '../helpers/sync-i18n.js';

let AgentSessionViewer;

beforeAll(async () => {
    globalThis.marked = marked;
    await initEnglish(await import('../../ui/js/shared/i18n.js'));
    ({ AgentSessionViewer } = await import('../../ui/js/chat/agent-sessions.js'));
});

afterEach(() => {
    document.body.innerHTML = '';
});

const synced = {
    provider_id: 'synced',
    session_id: 'dev1:kiro-cli:s1',
    title: 'Plan the launch',
    updated_at: '2026-11-09T10:00:00Z',
    message_count: 4,
    locator: { path: '/data/kage/sync/chats/dev1/k1.jsonl', id: 'dev1:kiro-cli:s1' },
    extras: { read_only: true, origin_provider: 'kiro-cli', device: 'dev1' },
};
const local = {
    provider_id: 'kiro-cli',
    session_id: 'here1',
    title: 'Local chat',
    updated_at: '2026-11-09T09:00:00Z',
    message_count: 2,
    locator: {},
    extras: {},
};

function viewer({ devices = [{ id: 'dev1', name: 'Work <PC>' }] } = {}) {
    const invoke = vi.fn(async (cmd) => {
        if (cmd === 'agent_session_providers') {
            return [
                { id: 'kiro-cli', label: 'Kiro CLI', available: true },
                { id: 'synced', label: 'On your other computers', available: true },
            ];
        }
        if (cmd === 'sync_list_devices') {
            if (!devices) throw { kind: 'sync_unavailable', message: 'off' };
            return devices;
        }
        if (cmd === 'agent_load_session') return [{ role: 'user', content: 'hi' }];
        return null;
    });
    document.body.innerHTML = `<div id="sessionSourceToggle"><button data-source="kage"></button></div>
        <div id="sessionList"></div><div id="messages"></div><div class="chat-input-container"></div>`;
    const chatApp = {
        elements: { messagesArea: document.getElementById('messages') },
        createNewSession: vi.fn(async () => {}),
        attachmentManager: { addFile: vi.fn() },
        createMessageElement(role) {
            const el = document.createElement('div');
            el.className = `message ${role}-message`;
            el.innerHTML = '<div class="message-content"></div>';
            return el;
        },
    };
    const v = new AgentSessionViewer(
        invoke,
        { sessionList: document.getElementById('sessionList') },
        chatApp
    );
    return { v, invoke, chatApp };
}

describe('chats from other computers', () => {
    it('are listed last, under their own heading, with the computer name', async () => {
        const { v } = viewer();
        await v.init();
        v.sessions = [synced, local];
        v.renderSessionList();
        const list = document.getElementById('sessionList');
        const heading = list.querySelector('.kd-synced-heading');
        expect(heading.textContent).toBe('On your other computers');
        const items = [...list.querySelectorAll('.kd-session-item')];
        expect(items.map((i) => i.dataset.sessionId)).toEqual(['here1', 'dev1:kiro-cli:s1']);
        expect(items[1].textContent).toContain('Work <PC>');
        expect(list.innerHTML).not.toContain('<PC>');
    });

    it('still list, without names, while sync is off', async () => {
        const { v } = viewer({ devices: null });
        await v.init();
        v.sessions = [synced];
        v.renderSessionList();
        expect(document.getElementById('sessionList').textContent).toContain(
            'On your other computers'
        );
    });

    it('open read-only; Continue here starts a new chat with the transcript attached', async () => {
        const { v, chatApp } = viewer();
        await v.init();
        v.sessions = [synced];
        const area = document.getElementById('messages');
        v.renderMessages(area, [{ role: 'user', content: 'hi' }], synced);
        const banner = area.querySelector('.kd-synced-banner');
        expect(banner.textContent).toContain('Read-only · a chat from');
        expect(banner.textContent).toContain('Work <PC>');
        banner.querySelector('.kd-continue-btn').click();
        await new Promise((r) => setTimeout(r, 0));
        expect(chatApp.createNewSession).toHaveBeenCalled();
        expect(chatApp.attachmentManager.addFile).toHaveBeenCalledWith(
            '/data/kage/sync/chats/dev1/k1.jsonl',
            'Plan the launch.jsonl',
            'text/plain'
        );
    });
});
