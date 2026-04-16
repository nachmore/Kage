/**
 * Math Calculator search provider — extracted from math-eval.js.
 * Uses window.math (mathjs) for evaluation.
 */

export default class MathSearchProvider {
    initialize(context) {
        this.config = context.config || {};
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    match(query) {
        const mathResult = evaluateMath(query, this.config.precision ?? 2);
        if (!mathResult) return [];

        let display = mathResult.display;
        if (this.config.thousands_separator) {
            const parts = display.split('.');
            parts[0] = parts[0].replace(/\B(?=(\d{3})+(?!\d))/g, ',');
            display = parts.join('.');
        }

        return [{
            id: 'math',
            type: 'math',
            label: '= ' + display,
            description: 'Press Enter to copy result',
            icon: '🧮',
            score: 93,
            data: { value: display, raw: mathResult.result },
        }];
    }

    execute(result) {
        return { type: 'copy', value: result.data.value };
    }

    destroy() {}
}

// --- Core evaluation logic (moved from math-eval.js) ---

function looksLikeMath(input) {
    const trimmed = input.trim();
    if (!trimmed || trimmed.length === 0) return false;
    if (!/\d/.test(trimmed)) return false;
    if (/^\d[\d.,]*\s*[a-z]+\s+(to|in)\s+[a-z]+$/i.test(trimmed)) return true;
    if (!/[+\-*\/\^%()!]|[a-z]+\s*\(/i.test(trimmed)) return false;
    if (/[a-z]{3,}\s+[a-z]{3,}/i.test(trimmed)) return false;
    return true;
}

function evaluateMath(input, precision = 0) {
    if (!looksLikeMath(input)) return null;
    if (!window.math) return null;
    try {
        const result = window.math.evaluate(input.trim());

        if (result && typeof result === 'object' && result.units) {
            const num = result.toNumber();
            if (!isFinite(num)) return null;
            const unitName = result.toString().replace(/^[\d.\-]+\s*/, '');
            const display = `${parseFloat(num.toFixed(2))} ${unitName}`;
            return { result: num, display };
        }

        if (typeof result !== 'number' && !window.math.isBigNumber(result)) return null;
        const num = typeof result === 'number' ? result : result.toNumber();
        if (!isFinite(num)) return null;
        // Skip if the input is just a plain number (no actual calculation).
        // Use strict Number() instead of parseFloat() — parseFloat("1/1") returns 1
        // which would incorrectly suppress "1/1 = 1".
        const inputAsNum = Number(input.trim());
        if (!isNaN(inputAsNum) && num === inputAsNum) return null;

        let display;
        if (precision >= 0) {
            display = num.toFixed(precision);
        } else {
            display = String(parseFloat(num.toPrecision(15)));
        }
        return { result: num, display };
    } catch {
        return null;
    }
}
