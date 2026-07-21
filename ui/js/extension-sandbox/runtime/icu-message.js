export function formatIcu(template, vars, locale) {
    if (typeof template !== 'string' || !template.includes('{')) return template || '';
    let out = '';
    let index = 0;
    while (index < template.length) {
        if (template[index] !== '{') {
            out += template[index++];
            continue;
        }
        const close = findMatchingBrace(template, index);
        if (close < 0) return out + template.slice(index);
        out += expandPlaceholder(template.slice(index + 1, close), vars, locale);
        index = close + 1;
    }
    return out;
}

function findMatchingBrace(value, start) {
    let depth = 0;
    for (let index = start; index < value.length; index++) {
        if (value[index] === '{') depth++;
        else if (value[index] === '}' && --depth === 0) return index;
    }
    return -1;
}

function expandPlaceholder(inner, vars, locale) {
    const parts = splitTopLevel(inner, ',');
    if (parts.length === 1) {
        const name = inner.trim();
        const value = vars[name];
        return value === undefined || value === null ? `{${name}}` : String(value);
    }
    const [variable, kind, ...rest] = parts.map((part) => part.trim());
    const body = rest.join(', ').trim();
    if (kind === 'plural') return expandPlural(variable, body, vars, locale);
    if (kind === 'select') return expandSelect(variable, body, vars, locale);
    return `{${inner}}`;
}

function splitTopLevel(value, separator) {
    const parts = [];
    let depth = 0;
    let buffer = '';
    for (const character of value) {
        if (character === '{') depth++;
        else if (character === '}') depth--;
        if (depth === 0 && character === separator) {
            parts.push(buffer);
            buffer = '';
        } else {
            buffer += character;
        }
    }
    parts.push(buffer);
    return parts;
}

function parseArms(body) {
    const arms = new Map();
    let index = 0;
    while (index < body.length) {
        while (index < body.length && /\s/.test(body[index])) index++;
        const keyEnd = body.indexOf('{', index);
        if (keyEnd < 0) break;
        const close = findMatchingBrace(body, keyEnd);
        if (close < 0) break;
        arms.set(body.slice(index, keyEnd).trim(), body.slice(keyEnd + 1, close));
        index = close + 1;
    }
    return arms;
}

function expandPlural(variable, body, vars, locale) {
    const count = vars[variable];
    const arms = parseArms(body);
    let arm = arms.get(`=${count}`);
    if (arm === undefined) {
        let category = 'other';
        try {
            category = new Intl.PluralRules(locale).select(Number(count));
        } catch {}
        arm = arms.get(category) || arms.get('other') || '';
    }
    return formatIcu(arm.replaceAll('#', String(count)), vars, locale);
}

function expandSelect(variable, body, vars, locale) {
    const arms = parseArms(body);
    return formatIcu(
        arms.get(String(vars[variable] ?? '')) || arms.get('other') || '',
        vars,
        locale
    );
}
