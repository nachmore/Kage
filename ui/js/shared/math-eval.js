/**
 * Math expression evaluator using mathjs library.
 * Supports arithmetic expressions and unit conversions (e.g., "5 km to miles").
 */

// mathjs is loaded via a deferred script tag — available by the time user types
// Access it via window.math

/**
 * Cache the last input that failed evaluation so we can skip re-evaluating
 * when the user is just appending characters to something already invalid.
 * e.g. "1 2" fails → "1 2 3", "1 2 3 4" all skip instantly.
 */
let _lastFailedPrefix = '';

/**
 * Reset the failed-prefix cache. Exported for testing.
 */
export function resetMathCache() {
    _lastFailedPrefix = '';
}

/**
 * Cheap regex pre-filter: reject input that clearly isn't math.
 * Intentionally generous — false positives are fine since evaluate() is
 * the real validator. The goal is to avoid calling evaluate() on obvious
 * non-math like "hello world" or "open settings".
 */
function couldBeMath(input) {
    const trimmed = input.trim();
    if (!trimmed || trimmed.length === 0) return false;
    // Must contain at least one digit
    if (!/\d/.test(trimmed)) return false;
    // Reject obvious natural language (only 3+ letter words separated by spaces)
    if (/^[a-z]{3,}(\s+[a-z]{3,})+$/i.test(trimmed)) return false;
    // Unit conversion: "5 km to miles", "100 lb in kg", "72 fahrenheit to celsius"
    if (/^\d[\d.,]*\s*[a-z]+\s+(to|in)\s+[a-z]+$/i.test(trimmed)) return true;
    // Contains an operator, parenthesis, factorial, or function call — likely math
    if (/[+\-*\/\^%()!]|[a-z]+\s*\(/i.test(trimmed)) return true;
    // Bare number (e.g. "42") — let it through, evaluateMath filters it later
    return true;
}

/**
 * Evaluate a math expression or unit conversion string.
 * @param {string} input - The expression to evaluate
 * @param {number} precision - Decimal places (0 = auto)
 * @returns {{ result: number|string, display: string } | null}
 */
export function evaluateMath(input, precision = 0) {
    const trimmed = input.trim();
    if (!couldBeMath(trimmed)) return null;
    if (!window.math) return null;
    // If the user is just appending to a previously failed input, skip
    if (_lastFailedPrefix && trimmed.startsWith(_lastFailedPrefix)) return null;
    try {
        const result = window.math.evaluate(trimmed);

        // Handle Unit results (from conversions like "5 km to miles")
        if (result && typeof result === 'object' && result.units) {
            const num = result.toNumber();
            if (!isFinite(num)) return null;
            const unitName = result.toString().replace(/^[\d.\-]+\s*/, '');
            const display = `${parseFloat(num.toFixed(2))} ${unitName}`;
            _lastFailedPrefix = '';
            return { result: num, display };
        }

        // Handle numeric results
        if (typeof result !== 'number' && !window.math.isBigNumber(result)) return null;
        const numResult = typeof result === 'number' ? result : result.toNumber();
        if (!isFinite(numResult)) return null;

        // Skip if the input is just a plain number (no actual calculation).
        const inputAsNum = Number(trimmed);
        if (!isNaN(inputAsNum) && numResult === inputAsNum) return null;

        _lastFailedPrefix = '';
        let display;
        if (precision > 0) {
            display = numResult.toFixed(precision);
        } else {
            // Auto: show up to 12 significant digits, trim trailing zeros
            display = parseFloat(numResult.toPrecision(12)).toString();
        }
        return { result: numResult, display };
    } catch {
        _lastFailedPrefix = trimmed;
        return null;
    }
}
