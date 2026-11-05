#!/usr/bin/env node
// Check a built site (default: _site/). Exits non-zero on any error.
//
//   node site/scripts/check.mjs [DIR] [--base /Kage/]
//
// - HTML well-formedness: balanced tags, quoted/duplicate attributes,
//   duplicate ids, one <h1>, <html lang>, <title>, meta description and
//   viewport, alt + width/height on every <img>, no leftover {{TOKENS}}.
// - Internal links and assets: every relative or base-rooted href/src/
//   srcset and CSS url() must exist in the build; #fragments must match an
//   id on the target page.
// - No runtime third-party loads: scripts, stylesheets, images, fonts and
//   CSS imports must be local. (Plain links to other sites are fine.)
// - A size budget, as warnings.
//
// Zero dependencies; Node 20+.

import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve, sep } from 'node:path';

const args = process.argv.slice(2);
const baseIdx = args.indexOf('--base');
const BASE = (() => {
    let b = baseIdx === -1 ? process.env.SITE_BASE || '/Kage/' : args[baseIdx + 1];
    if (!b.startsWith('/')) b = `/${b}`;
    return b.endsWith('/') ? b : `${b}/`;
})();
const dirArg = args.find((a, i) => !a.startsWith('--') && (baseIdx === -1 || i !== baseIdx + 1));
const ROOT = resolve(dirArg || '_site');

if (!existsSync(ROOT)) {
    console.error(`no build at ${ROOT}; run node site/scripts/build.mjs first`);
    process.exit(2);
}

const errors = [];
const warnings = [];
const err = (file, line, msg) => errors.push(`${file}${line ? `:${line}` : ''}  ${msg}`);
const warn = (file, msg) => warnings.push(`${file}  ${msg}`);

function walk(dir) {
    const out = [];
    for (const e of readdirSync(dir, { withFileTypes: true })) {
        const p = join(dir, e.name);
        if (e.isDirectory()) out.push(...walk(p));
        else out.push(p);
    }
    return out;
}

const files = walk(ROOT);
const rel = (f) => relative(ROOT, f).split(sep).join('/');

// ─── a tiny HTML tokenizer ─────────────────────────────────────────────

const VOID = new Set([
    'area',
    'base',
    'br',
    'col',
    'embed',
    'hr',
    'img',
    'input',
    'link',
    'meta',
    'source',
    'track',
    'wbr',
]);
const RAW = new Set(['script', 'style']);

function lineOf(text, idx) {
    let n = 1;
    for (let i = 0; i < idx; i++) if (text.charCodeAt(i) === 10) n++;
    return n;
}

function parseAttrs(src, file, line) {
    const attrs = new Map();
    const re = /\s*([^\s"'>/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/gy;
    let i = 0;
    while (i < src.length) {
        if (/^\s*\/?\s*$/.test(src.slice(i))) break;
        re.lastIndex = i;
        const m = re.exec(src);
        if (!m || m[0].length === 0) {
            err(file, line, `malformed attributes near "${src.slice(i, i + 30)}"`);
            break;
        }
        const name = m[1].toLowerCase();
        if (attrs.has(name)) err(file, line, `duplicate attribute "${name}"`);
        attrs.set(name, m[2] ?? m[3] ?? m[4] ?? '');
        i = re.lastIndex;
    }
    return attrs;
}

function parseHtml(text, file) {
    const elements = [];
    const stack = [];
    let i = 0;
    while (i < text.length) {
        const lt = text.indexOf('<', i);
        if (lt === -1) break;
        if (text.startsWith('<!--', lt)) {
            const end = text.indexOf('-->', lt + 4);
            if (end === -1) {
                err(file, lineOf(text, lt), 'unterminated comment');
                break;
            }
            i = end + 3;
            continue;
        }
        if (/^<!doctype/i.test(text.slice(lt, lt + 9))) {
            i = text.indexOf('>', lt) + 1;
            continue;
        }
        const close = text[lt + 1] === '/';
        const nameMatch = /^[a-zA-Z][a-zA-Z0-9-]*/.exec(text.slice(lt + (close ? 2 : 1)));
        if (!nameMatch) {
            i = lt + 1; // a bare "<" in text
            continue;
        }
        const name = nameMatch[0].toLowerCase();
        // find the end of the tag, respecting quotes
        let j = lt + 1 + (close ? 1 : 0) + nameMatch[0].length;
        let quote = null;
        for (; j < text.length; j++) {
            const c = text[j];
            if (quote) {
                if (c === quote) quote = null;
            } else if (c === '"' || c === "'") quote = c;
            else if (c === '>') break;
            else if (c === '<') {
                err(file, lineOf(text, lt), `"<" inside <${name}> tag`);
                break;
            }
        }
        const line = lineOf(text, lt);
        const inner = text.slice(lt + 1 + (close ? 1 : 0) + nameMatch[0].length, j);
        i = j + 1;
        if (close) {
            const top = stack.pop();
            if (!top) err(file, line, `stray </${name}>`);
            else if (top.name !== name) {
                err(file, line, `</${name}> closes <${top.name}> opened on line ${top.line}`);
                const k = stack.findLastIndex((s) => s.name === name);
                if (k !== -1) stack.length = k;
                else stack.push(top);
            }
            continue;
        }
        const selfClose = /\/\s*$/.test(inner);
        const attrs = parseAttrs(inner.replace(/\/\s*$/, ''), file, line);
        const el = { name, attrs, line, parents: stack.map((s) => s.name) };
        elements.push(el);
        if (RAW.has(name)) {
            const end = text.toLowerCase().indexOf(`</${name}`, i);
            if (end === -1) {
                err(file, line, `unterminated <${name}>`);
                break;
            }
            el.text = text.slice(i, end);
            i = text.indexOf('>', end) + 1;
            continue;
        }
        if (VOID.has(name) || selfClose) continue;
        stack.push({ name, line });
    }
    for (const s of stack) err(file, s.line, `<${s.name}> is never closed`);
    return elements;
}

// ─── checks ────────────────────────────────────────────────────────────

const pages = new Map(); // rel path -> { ids }
const refs = []; // { file, line, url, kind }

const isExternal = (u) => /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(u);

for (const f of files.filter((p) => p.endsWith('.html'))) {
    const file = rel(f);
    const text = readFileSync(f, 'utf8');
    const els = parseHtml(text, file);
    const ids = new Set();
    let h1 = 0;
    const has = (n) => els.some((e) => e.name === n);

    for (const left of text.match(/\{\{[A-Z_]+\}\}/g) || [])
        err(file, 0, `unreplaced token ${left}`);

    for (const el of els) {
        const id = el.attrs.get('id');
        if (id !== undefined) {
            if (ids.has(id)) err(file, el.line, `duplicate id "${id}"`);
            ids.add(id);
        }
        if (el.name === 'h1') h1++;
        if (el.name === 'img') {
            if (!el.attrs.has('alt'))
                err(file, el.line, `<img src="${el.attrs.get('src')}"> has no alt`);
            if (!el.attrs.has('width') || !el.attrs.has('height')) {
                err(
                    file,
                    el.line,
                    `<img src="${el.attrs.get('src')}"> needs width and height (layout shift)`
                );
            }
        }
        if (el.attrs.has('sizes') && el.name === 'img' && !el.attrs.has('srcset')) {
            err(file, el.line, '<img sizes> without srcset');
        }

        const url = (attr, kind) => {
            const v = el.attrs.get(attr);
            if (v !== undefined) refs.push({ file, line: el.line, url: v, kind });
        };
        if (el.name === 'a' || el.name === 'area') url('href', 'link');
        if (el.name === 'link')
            url(
                'href',
                /stylesheet|icon|preload|manifest/.test(el.attrs.get('rel') || '') ? 'load' : 'link'
            );
        if (['img', 'script', 'source', 'iframe', 'video', 'audio', 'embed'].includes(el.name))
            url('src', 'load');
        if (el.name === 'use') {
            const h = el.attrs.get('href') ?? el.attrs.get('xlink:href');
            if (h !== undefined) refs.push({ file, line: el.line, url: h, kind: 'use' });
        }
        for (const part of (el.attrs.get('srcset') || '').split(',').filter(Boolean)) {
            refs.push({ file, line: el.line, url: part.trim().split(/\s+/)[0], kind: 'load' });
        }
        const style = el.attrs.get('style') || (el.name === 'style' ? el.text : '');
        for (const m of (style || '').matchAll(/url\(\s*["']?([^"')]+)/g)) {
            refs.push({ file, line: el.line, url: m[1], kind: 'load' });
        }
    }

    const htmlEl = els.find((e) => e.name === 'html');
    if (!htmlEl?.attrs.get('lang')) err(file, 0, '<html> needs a lang');
    if (!has('title')) err(file, 0, 'missing <title>');
    if (!els.some((e) => e.name === 'meta' && e.attrs.get('name') === 'viewport'))
        err(file, 0, 'missing viewport meta');
    if (!els.some((e) => e.name === 'meta' && e.attrs.get('name') === 'description')) {
        err(file, 0, 'missing meta description');
    }
    if (h1 !== 1) err(file, 0, `expected exactly one <h1>, found ${h1}`);

    pages.set(file, { ids });
}

for (const f of files.filter((p) => p.endsWith('.css'))) {
    const file = rel(f);
    const text = readFileSync(f, 'utf8');
    for (const m of text.matchAll(/@import\s+(?:url\()?\s*["']?([^"')\s;]+)/g)) {
        refs.push({ file, line: lineOf(text, m.index), url: m[1], kind: 'load' });
    }
    for (const m of text.matchAll(/url\(\s*["']?([^"')]+)/g)) {
        refs.push({ file, line: lineOf(text, m.index), url: m[1], kind: 'load' });
    }
}

for (const r of refs) {
    const u = r.url.trim();
    if (!u || u.startsWith('data:') || u.startsWith('mailto:') || u.startsWith('tel:')) continue;
    if (/^javascript:/i.test(u)) {
        err(r.file, r.line, `javascript: URL "${u}"`);
        continue;
    }
    if (isExternal(u)) {
        if (r.kind === 'load' || r.kind === 'use')
            err(r.file, r.line, `loads a third-party resource at runtime: ${u}`);
        else if (u.startsWith('http://')) err(r.file, r.line, `insecure link ${u}`);
        continue;
    }
    const [pathPart, frag] = u.split('#');
    const cleanPath = pathPart.split('?')[0];
    let target;
    if (!cleanPath) target = r.file;
    else if (cleanPath.startsWith('/')) {
        if (!cleanPath.startsWith(BASE)) {
            err(r.file, r.line, `"${u}" is outside the site base ${BASE}`);
            continue;
        }
        target = cleanPath.slice(BASE.length);
    } else
        target = relative(ROOT, resolve(ROOT, dirname(r.file), cleanPath))
            .split(sep)
            .join('/');
    if (target.startsWith('..')) {
        err(r.file, r.line, `"${u}" points outside the site`);
        continue;
    }
    let abs = join(ROOT, target);
    if (target === '' || (existsSync(abs) && statSync(abs).isDirectory())) {
        target = target ? `${target.replace(/\/$/, '')}/index.html` : 'index.html';
        abs = join(ROOT, target);
    }
    if (!existsSync(abs)) {
        err(r.file, r.line, `broken ${r.kind === 'link' ? 'link' : 'reference'} "${u}"`);
        continue;
    }
    if (frag && target.endsWith('.html')) {
        const page = pages.get(target);
        if (page && !page.ids.has(decodeURIComponent(frag)))
            err(r.file, r.line, `"${u}": no id "${frag}" on ${target}`);
    }
}

// ─── size budget (warnings) ────────────────────────────────────────────

let total = 0;
for (const f of files) {
    const size = statSync(f).size;
    total += size;
    if (size > 500 * 1024) warn(rel(f), `is ${(size / 1024).toFixed(0)} KiB`);
}
if (total > 4 * 1024 * 1024) warn('site', `total ${(total / 1024 / 1024).toFixed(1)} MiB`);

for (const w of warnings) console.warn(`warning  ${w}`);
if (errors.length) {
    console.error(`site check failed (${errors.length}):`);
    for (const e of errors) console.error(`  ${e}`);
    process.exit(1);
}
console.log(
    `site check passed: ${pages.size} pages, ${refs.length} references, ${files.length} files, ${(total / 1024).toFixed(0)} KiB`
);
