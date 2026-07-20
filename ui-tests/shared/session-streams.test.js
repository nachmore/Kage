/**
 * Tests for ui/js/shared/session-streams.js — the per-session stream
 * registry that lets the chat window track N concurrent agent turns.
 *
 * Contract highlights:
 *   - begin/noteChunk/complete lifecycle per session, independent
 *     across sessions
 *   - NO content mirroring: the registry is a state machine + tool
 *     tracker; in-flight text lives in the backend accumulator
 *     (get_session_stream_snapshot) and completed text on disk
 *   - complete(viewing:true) consumes the entry; complete(viewing:false)
 *     flips to UNREAD for sidebar badging
 *   - chunks auto-begin (they can outrun session_activity)
 *   - markRead consumes UNREAD, returns-but-keeps STREAMING
 *   - trackTool routes tool updates onto the entry via the shared
 *     processToolCallUpdate
 */

import { describe, it, expect, vi } from 'vitest';
import { SessionStreamRegistry, STREAM } from '../../ui/js/shared/session-streams.js';
import { processToolCallUpdate } from '../../ui/js/shared/streaming-utils.js';

describe('SessionStreamRegistry', () => {
    it('begin creates a streaming entry', () => {
        const reg = new SessionStreamRegistry();
        const entry = reg.begin('s1');
        expect(entry.state).toBe(STREAM.STREAMING);
        expect(reg.isStreaming('s1')).toBe(true);
        expect(reg.isUnread('s1')).toBe(false);
    });

    it('begin is idempotent while streaming — keeps accumulated tool chips', () => {
        const reg = new SessionStreamRegistry();
        const entry = reg.begin('s1');
        entry.toolUsages.push({ toolCallId: 't1', title: 'Search' });
        const again = reg.begin('s1');
        expect(again.toolUsages).toHaveLength(1);
    });

    it('noteChunk auto-begins when chunks outrun session_activity', () => {
        const reg = new SessionStreamRegistry();
        const entry = reg.noteChunk('s1');
        expect(entry.state).toBe(STREAM.STREAMING);
        expect(reg.isStreaming('s1')).toBe(true);
    });

    it('tracks sessions independently', () => {
        const reg = new SessionStreamRegistry();
        reg.noteChunk('a');
        reg.noteChunk('b');
        reg.complete('a', { viewing: false });
        expect(reg.isUnread('a')).toBe(true);
        expect(reg.isStreaming('b')).toBe(true);
    });

    it('ignores null/empty session ids', () => {
        const reg = new SessionStreamRegistry();
        expect(reg.begin(null)).toBe(null);
        expect(reg.noteChunk(null)).toBe(null);
        expect(reg.anyStreaming()).toBe(false);
    });

    it('complete(viewing:true) consumes the entry', () => {
        const reg = new SessionStreamRegistry();
        reg.noteChunk('s1');
        const entry = reg.complete('s1', { viewing: true });
        expect(entry).not.toBe(null);
        expect(reg.get('s1')).toBe(null);
    });

    it('complete(viewing:false) flips to UNREAD and keeps tool chips', () => {
        const reg = new SessionStreamRegistry();
        const live = reg.noteChunk('s1');
        live.toolUsages.push({ toolCallId: 't1', title: 'Search' });
        const entry = reg.complete('s1', { viewing: false });
        expect(entry.state).toBe(STREAM.UNREAD);
        expect(reg.isUnread('s1')).toBe(true);
        expect(reg.get('s1').toolUsages).toHaveLength(1);
    });

    it('markRead consumes an UNREAD entry', () => {
        const reg = new SessionStreamRegistry();
        reg.noteChunk('s1');
        reg.complete('s1', { viewing: false });
        const entry = reg.markRead('s1');
        expect(entry).not.toBe(null);
        expect(reg.get('s1')).toBe(null);
    });

    it('markRead returns but keeps a STREAMING entry (live switch-in)', () => {
        const reg = new SessionStreamRegistry();
        reg.noteChunk('s1');
        const entry = reg.markRead('s1');
        expect(entry.state).toBe(STREAM.STREAMING);
        expect(reg.isStreaming('s1')).toBe(true);
    });

    it('fail drops the entry', () => {
        const reg = new SessionStreamRegistry();
        reg.noteChunk('s1');
        reg.fail('s1');
        expect(reg.get('s1')).toBe(null);
    });

    it('a chunk after an UNREAD complete starts a fresh turn', () => {
        const reg = new SessionStreamRegistry();
        reg.noteChunk('s1');
        reg.complete('s1', { viewing: false });
        const entry = reg.noteChunk('s1');
        expect(entry.state).toBe(STREAM.STREAMING);
        expect(entry.toolUsages).toHaveLength(0); // fresh turn, fresh chips
    });

    it('states() snapshots badge states', () => {
        const reg = new SessionStreamRegistry();
        reg.noteChunk('live');
        reg.noteChunk('done');
        reg.complete('done', { viewing: false });
        const states = reg.states();
        expect(states.get('live')).toBe(STREAM.STREAMING);
        expect(states.get('done')).toBe(STREAM.UNREAD);
    });

    it('anyStreaming reflects only live turns', () => {
        const reg = new SessionStreamRegistry();
        expect(reg.anyStreaming()).toBe(false);
        reg.noteChunk('s1');
        expect(reg.anyStreaming()).toBe(true);
        reg.complete('s1', { viewing: false });
        expect(reg.anyStreaming()).toBe(false); // unread is not streaming
    });

    it('notifies onChange subscribers and supports unsubscribe', () => {
        const reg = new SessionStreamRegistry();
        const seen = [];
        const off = reg.onChange((sid, kind) => seen.push(`${kind}:${sid}`));
        reg.begin('s1');
        reg.complete('s1', { viewing: false });
        reg.markRead('s1');
        off();
        reg.begin('s2');
        expect(seen).toEqual(['begin:s1', 'complete:s1', 'read:s1']);
    });

    it('a throwing onChange listener does not break the registry', () => {
        const reg = new SessionStreamRegistry();
        reg.onChange(() => {
            throw new Error('boom');
        });
        const spy = vi.spyOn(console, 'warn').mockImplementation(() => {});
        expect(() => reg.begin('s1')).not.toThrow();
        expect(reg.isStreaming('s1')).toBe(true);
        spy.mockRestore();
    });

    it('trackTool routes tool updates onto the session entry', () => {
        const reg = new SessionStreamRegistry();
        const event = {
            payload: {
                params: {
                    update: { toolCallId: 't1', title: 'Search web', kind: 'search' },
                },
            },
        };
        const { entry, updated, update } = reg.trackTool('s1', event, processToolCallUpdate);
        expect(updated).toBe(true);
        expect(update.title).toBe('Search web');
        expect(entry.toolUsages).toHaveLength(1);
        expect(entry.toolUsages[0].toolCallId).toBe('t1');
        // Second update with the same toolCallId is deduped.
        const second = reg.trackTool('s1', event, processToolCallUpdate);
        expect(second.updated).toBe(false);
        expect(second.entry.toolUsages).toHaveLength(1);
    });
});
