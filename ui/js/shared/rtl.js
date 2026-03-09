/**
 * RTL detection and input direction management.
 *
 * Detects whether the first character of input is an RTL script (Arabic, Hebrew, etc.)
 * and applies a CSS class to flip the input container layout accordingly.
 * Only triggers on the first character — switching mid-input does nothing.
 */

// Unicode ranges for RTL scripts
const RTL_REGEX = /[\u0590-\u05FF\u0600-\u06FF\u0700-\u074F\u0750-\u077F\u08A0-\u08FF\uFB50-\uFDFF\uFE70-\uFEFF]/;

/**
 * Check if a character is from an RTL script.
 */
function isRtlChar(ch) {
    return RTL_REGEX.test(ch);
}

/**
 * Get the first meaningful (non-whitespace, non-punctuation) character from text.
 */
function getFirstMeaningfulChar(text) {
    for (const ch of text) {
        if (ch.trim() && !/[\d\s\p{P}\p{S}]/u.test(ch)) return ch;
    }
    return null;
}

/**
 * Set up RTL detection on a textarea/input element.
 * Adds/removes 'rtl' class on the container based on the first character typed.
 *
 * @param {HTMLTextAreaElement|HTMLInputElement} inputEl - The input element
 * @param {HTMLElement} container - The container element to toggle 'rtl' class on
 * @param {HTMLElement} [responseEl] - Optional response/content element to also toggle 'rtl' on
 */
export function setupRtlDetection(inputEl, container, responseEl) {
    if (!inputEl || !container) return;

    let lastDirection = 'ltr';

    function update() {
        const text = inputEl.value;
        if (!text || text.trim().length === 0) {
            // Reset when input is cleared
            lastDirection = 'ltr';
            container.classList.remove('rtl');
            if (responseEl) responseEl.classList.remove('rtl');
            return;
        }

        const firstChar = getFirstMeaningfulChar(text);
        if (!firstChar) return;

        const dir = isRtlChar(firstChar) ? 'rtl' : 'ltr';
        if (dir !== lastDirection) {
            lastDirection = dir;
            container.classList.toggle('rtl', dir === 'rtl');
            if (responseEl) responseEl.classList.toggle('rtl', dir === 'rtl');
        }
    }

    inputEl.addEventListener('input', update);
    // Also check on paste
    inputEl.addEventListener('paste', () => setTimeout(update, 0));
}
