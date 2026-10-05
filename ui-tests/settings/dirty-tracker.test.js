/**
 * Tests for ui/js/settings/dirty-tracker.js — the pure bookkeeping behind
 * the settings window's unsaved-edit tracking (which sections may be
 * refreshed from config, and which ones Save writes).
 */

import { describe, it, expect, beforeEach } from 'vitest';
import {
    DirtyTracker,
    SETTINGS_DIRTY_EVENT,
    dirtySectionForEvent,
} from '../../ui/js/settings/dirty-tracker.js';

describe('DirtyTracker', () => {
    it('marks and reports ids', () => {
        const d = new DirtyTracker();
        expect(d.isDirty('a')).toBe(false);
        d.mark('a');
        expect(d.isDirty('a')).toBe(true);
        expect(d.isDirty('b')).toBe(false);
    });

    it('ignores empty ids', () => {
        const d = new DirtyTracker();
        d.mark('');
        d.mark(null);
        expect(d.snapshot().size).toBe(0);
    });

    it('clearSaved clears only ids not edited since the snapshot', () => {
        const d = new DirtyTracker();
        d.mark('a');
        d.mark('b');
        const snap = d.snapshot();
        // Edited again while the save was in flight.
        d.mark('b');
        // First edited while the save was in flight.
        d.mark('c');
        d.clearSaved(snap);
        expect(d.isDirty('a')).toBe(false);
        expect(d.isDirty('b')).toBe(true);
        expect(d.isDirty('c')).toBe(true);
    });

    it('snapshot is a copy', () => {
        const d = new DirtyTracker();
        const snap = d.snapshot();
        d.mark('a');
        expect(snap.has('a')).toBe(false);
    });

    it('clearAll drops everything', () => {
        const d = new DirtyTracker();
        d.mark('a');
        d.mark('b');
        d.clearAll();
        expect(d.isDirty('a')).toBe(false);
        expect(d.isDirty('b')).toBe(false);
    });
});

describe('dirtySectionForEvent', () => {
    let captured;
    const capture = (e) => {
        captured.push(dirtySectionForEvent(e));
    };

    beforeEach(() => {
        captured = [];
        document.body.innerHTML = `
            <div data-section-content="alpha">
                <input id="txt">
                <div id="plain"><span id="plainInner">x</span></div>
                <button id="btn"><span id="btnInner">go</span></button>
                <div id="act" data-action="something">act</div>
                <ul id="list"><li id="row"><button id="del">x</button></li></ul>
            </div>
            <input id="outside">
            <button id="outsideBtn">out</button>
        `;
    });

    function fire(el, type) {
        document.addEventListener(type, capture, true);
        try {
            el.dispatchEvent(new Event(type, { bubbles: true }));
        } finally {
            document.removeEventListener(type, capture, true);
        }
        return captured.pop();
    }

    it('input/change inside a section mark that section', () => {
        const txt = document.getElementById('txt');
        expect(fire(txt, 'input')).toBe('alpha');
        expect(fire(txt, 'change')).toBe('alpha');
    });

    it('the explicit dirty event marks the section', () => {
        expect(fire(document.getElementById('plain'), SETTINGS_DIRTY_EVENT)).toBe('alpha');
    });

    it('clicks on button-like controls count, including inner nodes', () => {
        expect(fire(document.getElementById('btn'), 'click')).toBe('alpha');
        expect(fire(document.getElementById('btnInner'), 'click')).toBe('alpha');
        expect(fire(document.getElementById('act'), 'click')).toBe('alpha');
    });

    it('clicks on plain content do not count', () => {
        expect(fire(document.getElementById('plainInner'), 'click')).toBeNull();
    });

    it('events outside every section are ignored', () => {
        expect(fire(document.getElementById('outside'), 'input')).toBeNull();
        expect(fire(document.getElementById('outsideBtn'), 'click')).toBeNull();
    });

    it('still resolves the section when an earlier handler removed the target', () => {
        // Mirrors the delegated action dispatcher: an earlier capture
        // listener deletes the clicked row before ours runs.
        const remover = () => document.getElementById('row')?.remove();
        document.addEventListener('click', remover, true);
        try {
            expect(fire(document.getElementById('del'), 'click')).toBe('alpha');
        } finally {
            document.removeEventListener('click', remover, true);
        }
    });
});
