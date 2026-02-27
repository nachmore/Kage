/**
 * Math expression evaluator using mathjs library.
 * Supports arithmetic expressions and unit conversions (e.g., "5 km to miles").
 */

// mathjs is loaded via a deferred script tag — available by the time user types
// Access it via window.math

/**
 * Quick check: does the string look like it could be a math expression
 * or a unit conversion?
 */
function looksLikeMath(input) {
    const trimmed = input.trim();
    if (!trimmed || trimmed.length === 0) return false;
    // Must contain at least one digit
    if (!/\d/.test(trimmed)) return false;
    // Unit conversion: "5 km to miles", "100 lb in kg", "72 fahrenheit to celsius"
    if (/^\d[\d.,]*\s*[a-z]+\s+(to|in)\s+[a-z]+$/i.test(trimmed)) return true;
    // Must contain at least one operator, parenthesis, or function-like pattern
    if (!/[+\-*\/\^%()!]|[a-z]+\s*\(/i.test(trimmed)) return false;
    // Reject if it looks like natural language (contains spaces between words without operators)
    if (/[a-z]{3,}\s+[a-z]{3,}/i.test(trimmed)) return false;
    return true;
}

/**
 * Evaluate a math expression or unit conversion string.
 * @param {string} input - The expression to evaluate
 * @param {number} precision - Decimal places (0 = auto)
 * @returns {{ result: number|string, display: string } | null}
 */
export function evaluateMath(input, precision = 0) {
    if (!looksLikeMath(input)) return null;
    if (!window.math) return null;
    try {
        const result = window.math.evaluate(input.trim());

        // Handle Unit results (from conversions like "5 km to miles")
        if (result && typeof result === 'object' && result.units) {
            const num = result.toNumber();
            if (!isFinite(num)) return null;
            // Format unit conversions to 2 decimal places
            const unitName = result.toString().replace(/^[\d.\-]+\s*/, '');
            const display = `${parseFloat(num.toFixed(2))} ${unitName}`;
            return { result: num, display };
        }

        // Handle numeric results
        if (typeof result !== 'number' && !window.math.isBigNumber(result)) return null;
        const numResult = typeof result === 'number' ? result : result.toNumber();
        if (!isFinite(numResult)) return null;

        let display;
        if (precision > 0) {
            display = numResult.toFixed(precision);
        } else {
            // Auto: show up to 12 significant digits, trim trailing zeros
            display = parseFloat(numResult.toPrecision(12)).toString();
        }
        return { result: numResult, display };
    } catch {
        return null;
    }
}
