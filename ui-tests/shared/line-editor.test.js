import { beforeEach, describe, expect, it } from 'vitest';
import { createLineEditor } from '../../ui/js/shared/line-editor.js';

function rows(container) {
    return [...container.querySelectorAll('.line-editor-row')];
}

function rowValues(container) {
    return rows(container).map((r) => r.querySelector('.line-editor-input').value);
}

function clickButton(row, classToken) {
    const btn = row.querySelector(`.line-editor-${classToken}`);
    btn.click();
}

describe('createLineEditor', () => {
    let container;

    beforeEach(() => {
        container = document.createElement('div');
        document.body.appendChild(container);
    });

    it('renders one row per initial line', () => {
        createLineEditor(container, { lines: ['alpha', 'beta', 'gamma'] });
        expect(rowValues(container)).toEqual(['alpha', 'beta', 'gamma']);
    });

    it('returns the current values via getLines', () => {
        const editor = createLineEditor(container, { lines: ['one', 'two'] });
        rows(container)[1].querySelector('.line-editor-input').value = 'changed';
        expect(editor.getLines()).toEqual(['one', 'changed']);
    });

    it('reorders rows when up button is clicked', () => {
        createLineEditor(container, { lines: ['a', 'b', 'c'] });
        clickButton(rows(container)[2], 'up');
        expect(rowValues(container)).toEqual(['a', 'c', 'b']);
    });

    it('reorders rows when down button is clicked', () => {
        createLineEditor(container, { lines: ['a', 'b', 'c'] });
        clickButton(rows(container)[0], 'down');
        expect(rowValues(container)).toEqual(['b', 'a', 'c']);
    });

    it('does nothing when up is clicked on the first row', () => {
        createLineEditor(container, { lines: ['a', 'b'] });
        clickButton(rows(container)[0], 'up');
        expect(rowValues(container)).toEqual(['a', 'b']);
    });

    it('does nothing when down is clicked on the last row', () => {
        createLineEditor(container, { lines: ['a', 'b'] });
        clickButton(rows(container)[1], 'down');
        expect(rowValues(container)).toEqual(['a', 'b']);
    });

    it('removes a row when ✕ is clicked', () => {
        createLineEditor(container, { lines: ['a', 'b', 'c'] });
        clickButton(rows(container)[1], 'remove');
        expect(rowValues(container)).toEqual(['a', 'c']);
    });

    it('appends an empty row via the add button', () => {
        const editor = createLineEditor(container, { lines: ['a'] });
        container.querySelector('.line-editor-add').click();
        expect(editor.getLines()).toEqual(['a', '']);
    });

    it('replaces all rows via setLines', () => {
        const editor = createLineEditor(container, { lines: ['a'] });
        editor.setLines(['x', 'y', 'z']);
        expect(rowValues(container)).toEqual(['x', 'y', 'z']);
    });

    it('splits multiline input across multiple rows', () => {
        const editor = createLineEditor(container, { lines: [] });
        editor.setLines(['line one\nline two', 'standalone']);
        expect(rowValues(container)).toEqual(['line one', 'line two', 'standalone']);
    });

    it('preserves interior empty rows (paragraph breaks)', () => {
        const editor = createLineEditor(container, { lines: ['## A', '', '- one'] });
        expect(editor.getLines()).toEqual(['## A', '', '- one']);
    });

    it('hides the empty hint when at least one row is present', () => {
        createLineEditor(container, { lines: ['a'], emptyHint: 'No lines yet.' });
        const empty = container.querySelector('.line-editor-empty');
        expect(empty.style.display).toBe('none');
    });

    it('shows the empty hint when no rows are present', () => {
        createLineEditor(container, { lines: [], emptyHint: 'No lines yet.' });
        const empty = container.querySelector('.line-editor-empty');
        expect(empty.style.display).toBe('');
    });

    it('does not show an empty hint when none was configured', () => {
        createLineEditor(container, { lines: [] });
        const empty = container.querySelector('.line-editor-empty');
        expect(empty.style.display).toBe('none');
    });

    it('Enter on a row inserts a new row directly below it', () => {
        createLineEditor(container, { lines: ['first', 'second'] });
        const input = rows(container)[0].querySelector('.line-editor-input');
        input.dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })
        );
        expect(rowValues(container)).toEqual(['first', '', 'second']);
    });

    it('Backspace on an empty row deletes it (when other rows remain)', () => {
        createLineEditor(container, { lines: ['kept', ''] });
        const input = rows(container)[1].querySelector('.line-editor-input');
        input.dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Backspace', bubbles: true, cancelable: true })
        );
        expect(rowValues(container)).toEqual(['kept']);
    });

    it('Backspace on the last remaining empty row leaves it alone', () => {
        createLineEditor(container, { lines: [''] });
        const input = rows(container)[0].querySelector('.line-editor-input');
        input.dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Backspace', bubbles: true, cancelable: true })
        );
        expect(rowValues(container)).toEqual(['']);
    });

    it('escapes HTML special characters when seeding row values', () => {
        // The factory builds rows via innerHTML for the buttons + input;
        // make sure user-supplied content can't smuggle markup.
        createLineEditor(container, { lines: ['<script>alert(1)</script>'] });
        // The row has 3 buttons + 1 input — no extra script element should appear.
        expect(container.querySelectorAll('script').length).toBe(0);
        expect(rowValues(container)).toEqual(['<script>alert(1)</script>']);
    });

    it('destroy clears the container', () => {
        const editor = createLineEditor(container, { lines: ['a'] });
        editor.destroy();
        expect(container.innerHTML).toBe('');
    });
});
