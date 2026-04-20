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

// --- Core evaluation logic ---

/**
 * Cache the last input that failed evaluation so we can skip re-evaluating
 * when the user is just appending characters to something already invalid.
 */
let _lastFailedPrefix = '';

/**
 * Cheap regex pre-filter: reject input that clearly isn't math.
 * Intentionally generous — false positives are fine since evaluate() is
 * the real validator.
 */
function couldBeMath(input) {
    const trimmed = input.trim();
    if (!trimmed || trimmed.length === 0) return false;
    if (!/\d/.test(trimmed)) return false;
    if (/^[a-z]{3,}(\s+[a-z]{3,})+$/i.test(trimmed)) return false;
    if (/^\d[\d.,]*\s*[a-z]+\s+(to|in)\s+[a-z]+$/i.test(trimmed)) return true;
    if (/[+\-*\/\^%()!]|[a-z]+\s*\(/i.test(trimmed)) return true;
    return true;
}

function evaluateMath(input, precision = 0) {
    const trimmed = input.trim();
    if (!couldBeMath(trimmed)) return null;
    if (!window.math) return null;
    if (_lastFailedPrefix && trimmed.startsWith(_lastFailedPrefix)) return null;
    try {
        const result = window.math.evaluate(trimmed);

        if (result && typeof result === 'object' && result.units) {
            const num = result.toNumber();
            if (!isFinite(num)) return null;
            const unitName = result.toString().replace(/^[\d.\-]+\s*/, '');
            const display = `${parseFloat(num.toFixed(2))} ${unitName}`;
            _lastFailedPrefix = '';
            return { result: num, display };
        }

        if (typeof result !== 'number' && !window.math.isBigNumber(result)) return null;
        const num = typeof result === 'number' ? result : result.toNumber();
        if (!isFinite(num)) return null;
        const inputAsNum = Number(trimmed);
        if (!isNaN(inputAsNum) && num === inputAsNum) return null;

        _lastFailedPrefix = '';
        let display;
        if (precision >= 0) {
            display = num.toFixed(precision);
        } else {
            display = String(parseFloat(num.toPrecision(15)));
        }
        return { result: num, display };
    } catch {
        _lastFailedPrefix = trimmed;
        return null;
    }
}
