/**
 * Tests for session-render.js — the chat window's session-history → render
 * queue logic plus the small format helpers (duration, relative date,
 * error coercion).
 *
 * The render-queue logic was previously tangled inside chat/app.js (P3.6
 * audit gap: zero coverage on the chat monolith). It handles three
 * non-obvious cases that are easy to break: steering messages plus their
 * assistant ack, `[Current time: ...]` injection stripping, and image
 * attachment extraction. Pin them all here.
 */

import { describe, it, expect } from 'vitest';
import {
    STEERING_MSG_PREFIX,
    buildMsgMeta,
    buildRenderQueue,
    formatDuration,
    formatRelativeDate,
    formatError,
    orderSessionsForSidebar,
} from '../../ui/js/shared/session-render.js';

// Stub image converter — production code uses btoa+Uint8Array, the tests
// only need to verify the wiring (the real converter is covered elsewhere).
const stubImageToDataUrl = (item) =>
    item && item.data && item.data.source
        ? `data:image/${item.data.format || 'png'};base64,STUB`
        : null;

// ---- buildMsgMeta -----------------------------------------------------------

describe('buildMsgMeta', () => {
    it('returns null when messageId is falsy', () => {
        expect(buildMsgMeta(null, {}, {}, 'user')).toBeNull();
        expect(buildMsgMeta('', {}, {}, 'user')).toBeNull();
    });

    it('returns null when no end timestamp recorded', () => {
        expect(buildMsgMeta('m1', {}, {}, 'user')).toBeNull();
    });

    it('subtracts duration from end timestamp for user messages', () => {
        // Durations are agent-time; user "send" timestamp is end - duration.
        // 30s duration ending at T means the user actually sent at T-30s.
        const endTs = '2026-05-07T12:00:30.000Z';
        const meta = buildMsgMeta('m1', { m1: endTs }, { m1: 30 }, 'user');
        expect(meta.timestamp).toBe('2026-05-07T12:00:00.000Z');
        // User messages don't carry a durationSecs; only assistant messages do.
        expect(meta.durationSecs).toBeUndefined();
    });

    it('keeps end timestamp and records duration for assistant messages', () => {
        const endTs = '2026-05-07T12:00:30.000Z';
        const meta = buildMsgMeta('m1', { m1: endTs }, { m1: 30 }, 'assistant');
        expect(meta.timestamp).toBe(endTs);
        expect(meta.durationSecs).toBe(30);
    });

    it('handles user message with zero duration as no-shift', () => {
        // Duration unknown → don't fabricate; the recorded end ts IS the send ts.
        const endTs = '2026-05-07T12:00:00.000Z';
        const meta = buildMsgMeta('m1', { m1: endTs }, {}, 'user');
        expect(meta.timestamp).toBe(endTs);
    });
});

// ---- buildRenderQueue -------------------------------------------------------

describe('buildRenderQueue', () => {
    it('emits user/assistant items for normal turns', () => {
        const messages = [
            { kind: 'Prompt', message_id: 'u1', content: [{ kind: 'text', data: 'Hello' }] },
            { kind: 'AssistantMessage', message_id: 'a1', content: [{ kind: 'text', data: 'Hi!' }] },
        ];
        const timestamps = { u1: '2026-05-07T12:00:00Z', a1: '2026-05-07T12:00:05Z' };
        const queue = buildRenderQueue(messages, timestamps, {}, stubImageToDataUrl);
        expect(queue).toHaveLength(2);
        expect(queue[0].type).toBe('user');
        expect(queue[0].text).toBe('Hello');
        expect(queue[1].type).toBe('assistant');
        expect(queue[1].text).toBe('Hi!');
    });

    it('tags steering messages and consumes the next assistant as ack', () => {
        // The agent's "got it" reply to a steering message is noise; we
        // collapse it under steering_ack so the timeline isn't bloated.
        const messages = [
            {
                kind: 'Prompt',
                message_id: 's1',
                content: [{ kind: 'text', data: STEERING_MSG_PREFIX + ' inject this context' }],
            },
            { kind: 'AssistantMessage', message_id: 'a1', content: [{ kind: 'text', data: 'ack' }] },
            { kind: 'Prompt', message_id: 'u2', content: [{ kind: 'text', data: 'real question' }] },
            { kind: 'AssistantMessage', message_id: 'a2', content: [{ kind: 'text', data: 'real answer' }] },
        ];
        const queue = buildRenderQueue(messages, {}, {}, stubImageToDataUrl);
        expect(queue.map(q => q.type)).toEqual([
            'steering',
            'steering_ack',
            'user',
            'assistant',
        ]);
        expect(queue[0].text).toBe('inject this context');
        expect(queue[1].text).toBe('ack');
        expect(queue[2].text).toBe('real question');
        expect(queue[3].text).toBe('real answer');
    });

    it('drops the steering_ack when assistant reply is empty', () => {
        // Empty ack still triggers the "skip next assistant" suppression but
        // we don't emit a phantom steering_ack item with no text.
        const messages = [
            {
                kind: 'Prompt',
                message_id: 's1',
                content: [{ kind: 'text', data: STEERING_MSG_PREFIX + ' inject' }],
            },
            { kind: 'AssistantMessage', message_id: 'a1', content: [{ kind: 'text', data: '' }] },
            { kind: 'Prompt', message_id: 'u2', content: [{ kind: 'text', data: 'after' }] },
        ];
        const queue = buildRenderQueue(messages, {}, {}, stubImageToDataUrl);
        expect(queue.map(q => q.type)).toEqual(['steering', 'user']);
    });

    it('strips [Current time: ...] injection lines from user prompts', () => {
        const messages = [
            {
                kind: 'Prompt',
                message_id: 'u1',
                content: [
                    { kind: 'text', data: '[Current time: 2026-05-07 12:00 PM]' },
                    { kind: 'text', data: 'What time is it?' },
                ],
            },
        ];
        const queue = buildRenderQueue(messages, {}, {}, stubImageToDataUrl);
        expect(queue).toHaveLength(1);
        expect(queue[0].text).toBe('What time is it?');
    });

    it('extracts image content into snapshots, leaving text intact', () => {
        const messages = [
            {
                kind: 'Prompt',
                message_id: 'u1',
                content: [
                    { kind: 'text', data: 'See this:' },
                    { kind: 'image', data: { source: { data: [1, 2, 3] }, format: 'jpeg' } },
                ],
            },
        ];
        const queue = buildRenderQueue(messages, {}, {}, stubImageToDataUrl);
        expect(queue[0].text).toBe('See this:');
        expect(queue[0].snapshots).toEqual([
            { type: 'image', previewUrl: 'data:image/jpeg;base64,STUB' },
        ]);
    });

    it('skips prompts with no displayable content', () => {
        // A prompt that's only a [Current time: ...] line and no images
        // produces no render item at all.
        const messages = [
            {
                kind: 'Prompt',
                message_id: 'u1',
                content: [{ kind: 'text', data: '[Current time: noon]' }],
            },
        ];
        expect(buildRenderQueue(messages, {}, {}, stubImageToDataUrl)).toEqual([]);
    });

    it('skips assistant messages with only whitespace content', () => {
        const messages = [
            { kind: 'AssistantMessage', message_id: 'a1', content: [{ kind: 'text', data: '   ' }] },
        ];
        expect(buildRenderQueue(messages, {}, {}, stubImageToDataUrl)).toEqual([]);
    });

    it('joins multiple assistant text parts with double newline', () => {
        const messages = [
            {
                kind: 'AssistantMessage',
                message_id: 'a1',
                content: [
                    { kind: 'text', data: 'Para one.' },
                    { kind: 'text', data: 'Para two.' },
                ],
            },
        ];
        const queue = buildRenderQueue(messages, {}, {}, stubImageToDataUrl);
        expect(queue[0].text).toBe('Para one.\n\nPara two.');
    });
});

// ---- formatDuration ---------------------------------------------------------

describe('formatDuration', () => {
    it('formats sub-minute durations as plain seconds', () => {
        expect(formatDuration(0)).toBe('0s');
        expect(formatDuration(5)).toBe('5s');
        expect(formatDuration(59)).toBe('59s');
    });

    it('rounds fractional seconds before formatting', () => {
        expect(formatDuration(4.4)).toBe('4s');
        expect(formatDuration(4.6)).toBe('5s');
    });

    it('uses Xm when minutes are exact', () => {
        expect(formatDuration(60)).toBe('1m');
        expect(formatDuration(120)).toBe('2m');
    });

    it('uses XmYs when there is a remainder', () => {
        expect(formatDuration(65)).toBe('1m5s');
        expect(formatDuration(125)).toBe('2m5s');
    });
});

// ---- formatRelativeDate -----------------------------------------------------

describe('formatRelativeDate', () => {
    // Pin "now" so the labels are deterministic across timezones.
    const now = new Date('2026-05-07T18:00:00Z');

    it('returns "Yesterday" for exactly one day prior', () => {
        const date = new Date('2026-05-06T18:00:00Z');
        expect(formatRelativeDate(date, now)).toBe('Yesterday');
    });

    it('returns a weekday name within the past week', () => {
        const threeDaysAgo = new Date('2026-05-04T18:00:00Z');
        const label = formatRelativeDate(threeDaysAgo, now);
        // toLocaleDateString output varies by locale — assert it's
        // a short non-empty string and not "Yesterday".
        expect(label).toBeTruthy();
        expect(label).not.toBe('Yesterday');
        expect(label.length).toBeLessThan(10);
    });

    it('returns a "Mon DD" style label beyond a week', () => {
        const longAgo = new Date('2026-04-01T18:00:00Z');
        const label = formatRelativeDate(longAgo, now);
        // Format depends on locale; minimally it should not be "Yesterday"
        // and should mention some month-or-day token.
        expect(label).not.toBe('Yesterday');
        expect(label).toMatch(/\d/); // contains the day number
    });
});

// ---- formatError ------------------------------------------------------------

describe('formatError', () => {
    it('returns "Unknown error" for falsy input', () => {
        expect(formatError(null)).toBe('Unknown error');
        expect(formatError(undefined)).toBe('Unknown error');
        expect(formatError('')).toBe('Unknown error');
    });

    it('returns string input unchanged', () => {
        expect(formatError('boom')).toBe('boom');
    });

    it('uses the .message field when present', () => {
        expect(formatError(new Error('actual message'))).toBe('actual message');
        expect(formatError({ message: 'IPC failed' })).toBe('IPC failed');
    });

    it('falls back to JSON when toString returns the [object Object] sentinel', () => {
        // A plain object with no message — toString() returns the unhelpful
        // "[object Object]" string. We should JSON-stringify instead.
        const out = formatError({ code: 42, detail: 'oops' });
        expect(out).toContain('42');
        expect(out).toContain('oops');
        expect(out).not.toContain('[object');
    });

    it('uses a custom toString when it produces useful output', () => {
        const obj = { toString: () => 'custom string repr' };
        expect(formatError(obj)).toBe('custom string repr');
    });

    it('returns "Unknown error" when JSON.stringify throws', () => {
        // Circular reference makes JSON.stringify throw — must not propagate.
        const cyc = {};
        cyc.self = cyc;
        expect(formatError(cyc)).toBe('Unknown error');
    });
});

describe('orderSessionsForSidebar', () => {
    const mk = (id, title, updated) => ({
        session_id: id,
        title,
        updated_at: updated,
    });

    it('pins the default (floating) session to the top', () => {
        const sessions = [
            mk('a', 'Alpha', '2026-01-03'),
            mk('b', 'Bravo', '2026-01-02'),
            mk('def', 'Default thread', '2026-01-01'),
        ];
        const out = orderSessionsForSidebar(sessions, { defaultId: 'def' });
        expect(out[0].session_id).toBe('def');
    });

    it('sorts the rest newest-first by updated_at', () => {
        const sessions = [
            mk('a', 'Alpha', '2026-01-01'),
            mk('b', 'Bravo', '2026-01-03'),
            mk('c', 'Charlie', '2026-01-02'),
        ];
        const out = orderSessionsForSidebar(sessions, {});
        expect(out.map((s) => s.session_id)).toEqual(['b', 'c', 'a']);
    });

    it('hides steering-only "New Chat" peers without a query', () => {
        const sessions = [
            mk('a', 'Real chat', '2026-01-02'),
            mk('b', 'New Chat', '2026-01-01'),
            mk('c', undefined, '2026-01-03'), // absent title defaults to New Chat
        ];
        const out = orderSessionsForSidebar(sessions, {});
        expect(out.map((s) => s.session_id)).toEqual(['a']);
    });

    it('keeps a "New Chat" row when it is the default session', () => {
        const sessions = [mk('a', 'Real', '2026-01-02'), mk('def', 'New Chat', '2026-01-01')];
        const out = orderSessionsForSidebar(sessions, { defaultId: 'def' });
        expect(out.map((s) => s.session_id).sort()).toEqual(['a', 'def']);
    });

    it('keeps a "New Chat" row when it is in keepIds (active/selected)', () => {
        const sessions = [mk('a', 'Real', '2026-01-02'), mk('sel', 'New Chat', '2026-01-01')];
        const out = orderSessionsForSidebar(sessions, { keepIds: ['sel'] });
        expect(out.map((s) => s.session_id).sort()).toEqual(['a', 'sel']);
    });

    it('filters by case-insensitive title match when a query is present', () => {
        const sessions = [
            mk('a', 'Budget planning', '2026-01-02'),
            mk('b', 'Dinner ideas', '2026-01-01'),
        ];
        const out = orderSessionsForSidebar(sessions, { searchQuery: 'BUDGET' });
        expect(out.map((s) => s.session_id)).toEqual(['a']);
    });

    it('a query overrides the New-Chat hiding rule', () => {
        const sessions = [mk('a', 'New Chat', '2026-01-01')];
        const out = orderSessionsForSidebar(sessions, { searchQuery: 'new' });
        expect(out.map((s) => s.session_id)).toEqual(['a']);
    });

    it('does not mutate the input array', () => {
        const sessions = [mk('a', 'A', '2026-01-01'), mk('b', 'B', '2026-01-02')];
        const copy = [...sessions];
        orderSessionsForSidebar(sessions, {});
        expect(sessions).toEqual(copy);
    });
});
