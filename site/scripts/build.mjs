#!/usr/bin/env node
// Assemble the marketing site into a deployable folder (default: _site/).
//
//   node site/scripts/build.mjs [--out DIR] [--base /Kage/] [--site-url URL] [--no-webp]
//
// - Wipes and refills the output folder, so it only builds into _site/ at
//   the repo root, a folder under the repo's build directory (target/), or
//   a folder under the system temp directory, and never into a folder that
//   holds anything but a previous build (see checkOut()).
// - Copies site/{index.html,404.html,css,js,assets}.
// - Pulls every asset the pages reference from the repo, so the site never
//   carries its own copy of the README screenshots or the mascot art:
//     assets/shots/*   <- docs/media/readme/*
//     assets/media/*   <- docs/media/*
//     assets/mascot/*  <- ui/assets/*
//     assets/icons/*   <- icons/*
//   A reference to a file that doesn't exist fails the build.
// - If `cwebp` is on PATH, screenshots get 900w and 1800w WebP versions and
//   their <img> tags are wrapped in <picture>; otherwise the PNGs are used.
// - Fills {{SITE_URL}}, {{BASE}}, {{BUILD_DATE}}, {{BUILD_SHA}} and
//   {{BUILD_SHA_SHORT}}.
//
// No dependencies beyond Node 20+.

import { execFileSync } from 'node:child_process';
import {
    copyFileSync,
    cpSync,
    existsSync,
    lstatSync,
    mkdirSync,
    readdirSync,
    readFileSync,
    realpathSync,
    rmSync,
    statSync,
    writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const SITE = resolve(here, '..');
const REPO = resolve(SITE, '..');

function arg(name, fallback) {
    const i = process.argv.indexOf(`--${name}`);
    if (i === -1) return fallback;
    const v = process.argv[i + 1];
    if (v === undefined || v.startsWith('--')) {
        console.error(`--${name} needs a value`);
        process.exit(2);
    }
    return v;
}

const OUT_ARG = arg('out', '_site');
const OUT = resolve(REPO, OUT_ARG);
const BASE = normaliseBase(arg('base', process.env.SITE_BASE || '/Kage/'));
const SITE_URL = withSlash(
    arg('site-url', process.env.SITE_URL || 'https://nachmore.github.io/Kage/')
);
const WANT_WEBP = !process.argv.includes('--no-webp');

function withSlash(s) {
    return s.endsWith('/') ? s : `${s}/`;
}

function normaliseBase(b) {
    let s = b.trim() || '/';
    if (!s.startsWith('/')) s = `/${s}`;
    return withSlash(s);
}

const SOURCES = [
    ['assets/shots/', 'docs/media/readme/'],
    ['assets/media/', 'docs/media/'],
    ['assets/mascot/', 'ui/assets/'],
    ['assets/icons/', 'icons/'],
];

const errors = [];
const fail = (msg) => errors.push(msg);

// ─── 1. fresh output with the site's own files ─────────────────────────

// The build deletes the output folder first, so be strict about where that
// can be. Allowed: <repo>/_site (the default; gitignored), a folder inside
// <repo>/target/ (the repo's build directory; gitignored), or a folder
// inside the system temp directory. A relative --out is relative to the
// repo and must stay inside it, so `--out ../x` never escapes (even from a
// clone that lives in the temp directory); give a temp folder as an
// absolute path. Paths are compared with path.relative after resolving
// symlinks, so a symlinked parent or a sibling such as <repo>-backup can't
// slip through a string-prefix check. On top of that, an existing folder
// must be empty or carry the marker a previous build wrote; only
// <repo>/_site may predate the marker, and never if git tracks files in it.
const MARKER = '.kage-site-build';
const REPO_REAL = realpathSync(REPO);
const TMP_REAL = realpathSync(tmpdir());

// realpath() of the nearest existing ancestor, plus the rest of the path.
function realish(p) {
    let head = p;
    const tail = [];
    while (!existsSync(head)) {
        const up = dirname(head);
        if (up === head) break;
        tail.unshift(basename(head));
        head = up;
    }
    return join(realpathSync(head), ...tail);
}

// Strictly inside `root` (not `root` itself)?
function inside(root, p) {
    const rel = relative(root, p);
    return rel !== '' && rel !== '..' && !rel.startsWith(`..${sep}`) && !isAbsolute(rel);
}

function trackedFiles(dir) {
    if (!inside(REPO_REAL, dir)) return false;
    try {
        const out = execFileSync('git', ['ls-files', '-z', '--', relative(REPO_REAL, dir)], {
            cwd: REPO_REAL,
            encoding: 'utf8',
            stdio: ['ignore', 'pipe', 'ignore'],
        });
        return out.length > 0;
    } catch {
        return false; // no git (e.g. a source tarball): the location rules still apply
    }
}

// '' if `out` is a safe place to wipe and rebuild, else the reason it isn't.
function checkOut(raw, out) {
    if (!isAbsolute(raw) && out !== REPO && !inside(REPO, out)) {
        return 'a relative --out must stay inside the repo (give a temp folder as an absolute path)';
    }
    let link = false;
    try {
        link = lstatSync(out).isSymbolicLink();
    } catch {
        // doesn't exist yet
    }
    if (link) return 'it is a symlink';
    const real = realish(out);
    const defaultOut = join(REPO_REAL, '_site');
    // Inside the repo (even a repo that lives in the temp directory) only the
    // repo's own rules apply; outside it, never a folder that holds the repo.
    const inRepo = real === REPO_REAL || inside(REPO_REAL, real);
    const allowed = inRepo
        ? real === defaultOut || inside(join(REPO_REAL, 'target'), real)
        : inside(TMP_REAL, real) && !inside(real, REPO_REAL);
    if (!allowed) {
        return 'the output must be _site/ at the repo root, a folder inside target/, or a folder inside the temp directory';
    }
    if (!existsSync(out)) return '';
    if (!statSync(out).isDirectory()) return 'it is not a folder';
    if (existsSync(join(out, MARKER))) return '';
    if (trackedFiles(real)) return 'git tracks files in it';
    if (real !== defaultOut && readdirSync(out).length > 0) {
        return `it is not empty and has no ${MARKER} marker from a previous build`;
    }
    return '';
}

const refusal = checkOut(OUT_ARG, OUT);
if (refusal) {
    console.error(`refusing to build into ${OUT}: ${refusal}`);
    process.exit(2);
}
rmSync(OUT, { recursive: true, force: true });
mkdirSync(OUT, { recursive: true });
writeFileSync(
    join(OUT, MARKER),
    'Written by site/scripts/build.mjs. This folder is a site build; the next build deletes and rebuilds it.\n'
);

for (const name of ['index.html', '404.html']) copyFileSync(join(SITE, name), join(OUT, name));
for (const dir of ['css', 'js', 'assets']) {
    if (existsSync(join(SITE, dir))) cpSync(join(SITE, dir), join(OUT, dir), { recursive: true });
}

// ─── 2. tokens ─────────────────────────────────────────────────────────

function git(...args) {
    try {
        return execFileSync('git', args, {
            cwd: REPO,
            encoding: 'utf8',
            stdio: ['ignore', 'pipe', 'ignore'],
        }).trim();
    } catch {
        return '';
    }
}

const sha = process.env.GITHUB_SHA || git('rev-parse', 'HEAD') || 'local';
const TOKENS = {
    SITE_URL,
    BASE,
    BUILD_DATE: new Date().toISOString().slice(0, 10),
    BUILD_SHA: sha,
    BUILD_SHA_SHORT: sha.slice(0, 7),
};

const fillTokens = (text) =>
    text.replace(/\{\{([A-Z_]+)\}\}/g, (m, k) => (k in TOKENS ? TOKENS[k] : m));

// ─── 3. collect and copy referenced repo assets ────────────────────────

function walk(dir) {
    const out = [];
    for (const e of readdirSync(dir, { withFileTypes: true })) {
        const p = join(dir, e.name);
        if (e.isDirectory()) out.push(...walk(p));
        else out.push(p);
    }
    return out;
}

const htmlFiles = walk(OUT).filter((f) => f.endsWith('.html'));
const cssFiles = walk(OUT).filter((f) => f.endsWith('.css'));

const wanted = new Set();
const refRe = /(?:\b(?:src|href|content)=["']|url\(\s*["']?)([^"')\s]+)/g;

function noteRef(raw, fromFile) {
    let ref = fillTokens(raw).split(/[?#]/)[0];
    if (!ref) return;
    if (ref.startsWith(SITE_URL)) ref = ref.slice(SITE_URL.length);
    else if (ref.startsWith(BASE)) ref = ref.slice(BASE.length);
    else if (/^[a-z][a-z0-9+.-]*:/i.test(ref) || ref.startsWith('//') || ref.startsWith('/'))
        return;
    else
        ref = relative(OUT, resolve(dirname(fromFile), ref))
            .split(sep)
            .join('/');
    if (ref.startsWith('assets/')) wanted.add(ref);
}

for (const f of [...htmlFiles, ...cssFiles]) {
    const text = readFileSync(f, 'utf8');
    for (const m of text.matchAll(refRe)) noteRef(m[1], f);
    // srcset lists (none in the source today, but cheap to support)
    for (const m of text.matchAll(/\bsrcset=["']([^"']+)["']/g)) {
        for (const part of m[1].split(',')) noteRef(part.trim().split(/\s+/)[0], f);
    }
}

let copied = 0;
for (const ref of [...wanted].sort()) {
    const dest = join(OUT, ref);
    if (existsSync(dest)) continue; // shipped in site/assets
    const map = SOURCES.find(([prefix]) => ref.startsWith(prefix));
    if (!map) {
        fail(`${ref}: not in site/assets and no repo source maps to it`);
        continue;
    }
    const src = join(REPO, map[1], ref.slice(map[0].length));
    if (!existsSync(src) || !statSync(src).isFile()) {
        fail(`${ref}: source file ${relative(REPO, src)} is missing`);
        continue;
    }
    mkdirSync(dirname(dest), { recursive: true });
    copyFileSync(src, dest);
    copied++;
}

// ─── 4. WebP for screenshots ───────────────────────────────────────────

function hasCwebp() {
    try {
        execFileSync('cwebp', ['-version'], { stdio: 'ignore' });
        return true;
    } catch {
        return false;
    }
}

const webp = new Set();
if (WANT_WEBP && hasCwebp()) {
    for (const ref of wanted) {
        if (!ref.startsWith('assets/shots/') || !ref.endsWith('.png')) continue;
        const png = join(OUT, ref);
        if (!existsSync(png)) continue;
        const base = png.slice(0, -4);
        const opts = ['-quiet', '-q', '84', '-m', '6', '-sharp_yuv', '-metadata', 'none'];
        execFileSync('cwebp', [...opts, png, '-o', `${base}.webp`]);
        execFileSync('cwebp', [...opts, '-resize', '900', '0', png, '-o', `${base}-900.webp`]);
        webp.add(ref);
    }
} else if (WANT_WEBP) {
    console.warn('cwebp not found: shipping PNG screenshots only');
}

function pictureFor(imgTag) {
    const src = /\bsrc="([^"]+)"/.exec(imgTag)?.[1];
    const sizes = /\sdata-sizes="([^"]+)"/.exec(imgTag)?.[1];
    const img = imgTag.replace(/\sdata-sizes="[^"]*"/, '');
    if (!src || !webp.has(src)) return img;
    const stem = src.slice(0, -4);
    const sizesAttr = sizes ? ` sizes="${sizes}"` : '';
    return `<picture><source type="image/webp" srcset="${stem}-900.webp 900w, ${stem}.webp 1800w"${sizesAttr}>${img}</picture>`;
}

// ─── 5. write the pages ────────────────────────────────────────────────

for (const f of htmlFiles) {
    let html = fillTokens(readFileSync(f, 'utf8'));
    html = html.replace(/<img\b[^>]*>/g, pictureFor);
    const left = html.match(/\{\{[A-Z_]+\}\}/g);
    if (left) fail(`${relative(OUT, f)}: unknown token(s) ${[...new Set(left)].join(', ')}`);
    writeFileSync(f, html);
}

if (errors.length) {
    console.error(`site build failed:\n  ${errors.join('\n  ')}`);
    process.exit(1);
}

let bytes = 0;
const all = walk(OUT);
for (const f of all) bytes += statSync(f).size;
console.log(
    `built ${inside(REPO, OUT) ? relative(REPO, OUT) : OUT}: ${all.length} files, ${(bytes / 1024).toFixed(0)} KiB ` +
        `(${copied} assets from the repo, ${webp.size} screenshots as WebP) · base ${BASE} · ${TOKENS.BUILD_SHA_SHORT}`
);
