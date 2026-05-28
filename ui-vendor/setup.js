#!/usr/bin/env node
/**
 * Vendor setup script — copies browser-ready bundles from node_modules into
 * ui/vendor/lib/ so the app can load them via <script> tags and dynamic imports.
 *
 * Run: node ui-vendor/setup.js
 * Or:  cd ui-vendor && npm install  (postinstall hook runs this automatically)
 *
 * Lives outside ui/ so npm dev tooling (package.json, node_modules) doesn't
 * end up brotli-embedded in the shipped binary by tauri-codegen.
 */

const fs = require('fs');
const path = require('path');

const ROOT = __dirname; // ui-vendor/
const NM = path.join(ROOT, 'node_modules');
const LIB = path.resolve(ROOT, '..', 'ui', 'vendor', 'lib');

// Prism language packs to include
const PRISM_LANGUAGES = [
    'bash', 'clike', 'csharp', 'css', 'go', 'java', 'javascript',
    'json', 'markdown', 'markup', 'python', 'rust', 'sql', 'typescript', 'yaml'
];

/**
 * File copy manifest: [source (relative to node_modules), dest (relative to lib/)]
 */
const COPIES = [
    // marked — markdown parser
    ['marked/marked.min.js', 'marked.min.js'],

    // mathjs — math expression evaluator (UMD bundle)
    ['mathjs/lib/browser/math.js', 'math.js'],

    // mermaid — diagram renderer
    ['mermaid/dist/mermaid.min.js', 'mermaid.min.js'],

    // prismjs — syntax highlighter (core)
    ['prismjs/prism.js', 'prism.js'],

    // prismjs — okaidia theme
    ['prismjs/themes/prism-okaidia.min.css', 'prism-themes/prism-okaidia.min.css'],

    // @hpcc-js/wasm-graphviz — graphviz WASM renderer (ESM)
    ['@hpcc-js/wasm-graphviz/dist/index.js', 'graphviz.js'],

    // tinyld — language detection (ESM browser bundle)
    ['tinyld/dist/tinyld.normal.browser.js', 'tinyld.js'],

    // KaTeX — math rendering (used by the markdown `latex` / `math` fence
    // renderer). The fonts are referenced by relative URL inside the CSS,
    // so the dest layout under ui/vendor/lib/katex/ must mirror upstream's.
    ['katex/dist/katex.min.js', 'katex/katex.min.js'],
    ['katex/dist/katex.min.css', 'katex/katex.min.css'],

    // prism language components
    ...PRISM_LANGUAGES.map(lang => [
        `prismjs/components/prism-${lang}.min.js`,
        `prism-components/prism-${lang}.min.js`
    ]),
];

/**
 * Directory-copy manifest: [source dir (relative to node_modules),
 * dest dir (relative to lib/)]. Recursively copies every file in the
 * source directory.
 */
const DIR_COPIES = [
    // KaTeX fonts (60 files: woff/woff2/ttf for each font face).
    ['katex/dist/fonts', 'katex/fonts'],
];

function ensureDir(dir) {
    if (!fs.existsSync(dir)) {
        fs.mkdirSync(dir, { recursive: true });
    }
}

function copyFile(src, dest) {
    const srcPath = path.join(NM, src);
    const destPath = path.join(LIB, dest);

    if (!fs.existsSync(srcPath)) {
        console.error(`  ✗ Missing: ${src}`);
        return false;
    }

    ensureDir(path.dirname(destPath));
    fs.copyFileSync(srcPath, destPath);
    return true;
}

function copyDir(srcRel, destRel) {
    const srcPath = path.join(NM, srcRel);
    const destPath = path.join(LIB, destRel);

    if (!fs.existsSync(srcPath)) {
        console.error(`  ✗ Missing dir: ${srcRel}`);
        return 0;
    }
    ensureDir(destPath);

    let count = 0;
    for (const entry of fs.readdirSync(srcPath, { withFileTypes: true })) {
        const childSrc = path.join(srcPath, entry.name);
        const childDest = path.join(destPath, entry.name);
        if (entry.isDirectory()) {
            // Recurse into subdirs by re-using copyDir's contract via a
            // depth-first walk. The katex fonts dir is flat today, so this
            // mostly future-proofs the helper for nested layouts.
            count += copyDirRecursiveAbs(childSrc, childDest);
        } else if (entry.isFile()) {
            fs.copyFileSync(childSrc, childDest);
            count++;
        }
    }
    return count;
}

function copyDirRecursiveAbs(srcAbs, destAbs) {
    ensureDir(destAbs);
    let count = 0;
    for (const entry of fs.readdirSync(srcAbs, { withFileTypes: true })) {
        const childSrc = path.join(srcAbs, entry.name);
        const childDest = path.join(destAbs, entry.name);
        if (entry.isDirectory()) count += copyDirRecursiveAbs(childSrc, childDest);
        else if (entry.isFile()) {
            fs.copyFileSync(childSrc, childDest);
            count++;
        }
    }
    return count;
}

/**
 * Generate emoji-names.js from unicode-emoji-json package.
 * The package provides a JSON file; we transform it into an ES module export.
 */
function generateEmojiNames() {
    const jsonPath = path.join(NM, 'unicode-emoji-json/data-by-emoji.json');
    if (!fs.existsSync(jsonPath)) {
        console.error('  ✗ Missing: unicode-emoji-json/data-by-emoji.json');
        return false;
    }

    const raw = JSON.parse(fs.readFileSync(jsonPath, 'utf8'));
    // Transform { "😀": { "name": "grinning face", ... }, ... }
    // into     { "😀": "grinning face", ... }
    const nameMap = {};
    for (const [emoji, data] of Object.entries(raw)) {
        nameMap[emoji] = data.name;
    }

    const destPath = path.join(LIB, 'emoji-names.js');
    ensureDir(path.dirname(destPath));
    fs.writeFileSync(destPath, `export const emojiNames = ${JSON.stringify(nameMap)};\n`);
    return true;
}

// --- Main ---

console.log('vendor/setup: copying browser bundles to lib/...');

// Check node_modules exists
if (!fs.existsSync(NM)) {
    console.error('vendor/setup: node_modules not found. Run "npm install" in ui-vendor/ first.');
    process.exit(1);
}

ensureDir(LIB);

let ok = 0;
let fail = 0;

for (const [src, dest] of COPIES) {
    if (copyFile(src, dest)) {
        ok++;
    } else {
        fail++;
    }
}

for (const [srcDir, destDir] of DIR_COPIES) {
    const copied = copyDir(srcDir, destDir);
    if (copied > 0) {
        ok += copied;
    } else {
        fail++;
    }
}

// Special: emoji-names (generated from JSON)
if (generateEmojiNames()) {
    ok++;
} else {
    fail++;
}

console.log(`vendor/setup: done — ${ok} files copied${fail ? `, ${fail} failed` : ''}.`);

if (fail > 0) {
    process.exit(1);
}
