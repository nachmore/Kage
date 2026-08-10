/** Minimum gap between floating-session re-bootstrap attempts. */
export const BOOTSTRAP_RETRY_DEBOUNCE_MS = 5000;

/**
 * Report whether the textarea caret sits on the first and/or last *visual*
 * row, accounting for soft-wrapped lines (not just literal "\n"). The floating
 * input uses this to decide when ArrowUp/ArrowDown should navigate prompt
 * history versus move the caret within a multi-line or wrapped prompt — same
 * rule a text editor applies: up on the top row leaves the field, up anywhere
 * else moves within it.
 *
 * Works by laying out a hidden mirror div styled identically to the textarea,
 * with a marker span at the caret, then comparing the marker's top offset to
 * the first/last row bands. Returns { isFirstRow, isLastRow }; a single-row
 * (or empty) input reports both true.
 */
export function caretVisualRowInfo(textarea) {
    const doc = textarea.ownerDocument;
    const win = doc.defaultView;
    const cs = win.getComputedStyle(textarea);
    const value = textarea.value;
    // Arrow keys move relative to the focus end of any selection.
    const caret = textarea.selectionEnd ?? value.length;

    const mirror = doc.createElement('div');
    const style = mirror.style;
    // Copy every property that influences text layout so the mirror wraps at
    // exactly the same points as the textarea.
    const copy = [
        'fontStyle',
        'fontVariant',
        'fontWeight',
        'fontStretch',
        'fontSize',
        'lineHeight',
        'fontFamily',
        'textAlign',
        'textTransform',
        'textIndent',
        'letterSpacing',
        'wordSpacing',
        'tabSize',
    ];
    for (const p of copy) style[p] = cs[p];

    const paddingTop = parseFloat(cs.paddingTop) || 0;
    const paddingRight = parseFloat(cs.paddingRight) || 0;
    const paddingBottom = parseFloat(cs.paddingBottom) || 0;
    const paddingLeft = parseFloat(cs.paddingLeft) || 0;

    // Pin the mirror's *content* width to the textarea's content width so wrap
    // points line up regardless of the textarea's box-sizing. clientWidth is
    // the inner width (content + padding, excluding border/scrollbar); subtract
    // the horizontal padding to get the content width, then run the mirror as
    // content-box with matching padding.
    const contentWidth = Math.max(0, textarea.clientWidth - paddingLeft - paddingRight);
    style.boxSizing = 'content-box';
    style.width = contentWidth + 'px';
    style.paddingTop = paddingTop + 'px';
    style.paddingRight = paddingRight + 'px';
    style.paddingBottom = paddingBottom + 'px';
    style.paddingLeft = paddingLeft + 'px';
    style.position = 'absolute';
    style.visibility = 'hidden';
    style.top = '0';
    style.left = '-9999px';
    style.whiteSpace = 'pre-wrap';
    style.overflowWrap = 'break-word';
    style.wordWrap = 'break-word';

    // A zero-size marker at the very start anchors the "first row" baseline, so
    // the caret's row is measured *relative* to it. This sidesteps any
    // ambiguity about whether offsetTop is measured from the offsetParent's
    // padding or border edge — both markers share the same origin either way.
    const startMarker = doc.createElement('span');
    const caretMarker = doc.createElement('span');
    startMarker.textContent = '​'; // zero-width space
    // Give the caret marker real content so it has a measurable box even at the
    // end of the text; a lone caret at EOF would otherwise collapse.
    caretMarker.textContent = value.substring(caret) || '.';

    mirror.appendChild(startMarker);
    mirror.appendChild(doc.createTextNode(value.substring(0, caret)));
    mirror.appendChild(caretMarker);

    textarea.parentNode.insertBefore(mirror, textarea);

    // Resolve line height; getComputedStyle can still return "normal".
    let lineHeight = parseFloat(cs.lineHeight);
    if (!Number.isFinite(lineHeight) || lineHeight <= 0) {
        lineHeight = parseFloat(cs.fontSize) * 1.2 || 16;
    }

    // Caret offset relative to the first row: 0 on row 0, ~lineHeight on row 1…
    const caretRel = caretMarker.offsetTop - startMarker.offsetTop;
    // Total height of all text rows (scrollHeight strips neither padding).
    const contentHeight = mirror.scrollHeight - paddingTop - paddingBottom;
    const lastRowRelTop = contentHeight - lineHeight;

    mirror.remove();

    // Half-line tolerance absorbs sub-pixel rounding.
    const isFirstRow = caretRel <= lineHeight * 0.5;
    const isLastRow = caretRel >= lastRowRelTop - lineHeight * 0.5;
    return { isFirstRow, isLastRow };
}

/** Measure a textarea's natural content height without changing its live height. */
export function measureTextareaContentHeight(textarea) {
    const clone = textarea.cloneNode(false);
    clone.value = textarea.value;
    clone.style.position = 'absolute';
    clone.style.visibility = 'hidden';
    clone.style.height = 'auto';
    clone.style.maxHeight = 'none';
    clone.style.width = textarea.clientWidth + 'px';
    clone.style.overflow = 'hidden';
    textarea.parentNode.insertBefore(clone, textarea);
    const h = clone.scrollHeight;
    clone.remove();
    return h;
}
