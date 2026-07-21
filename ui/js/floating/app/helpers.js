/** Minimum gap between floating-session re-bootstrap attempts. */
export const BOOTSTRAP_RETRY_DEBOUNCE_MS = 5000;

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
