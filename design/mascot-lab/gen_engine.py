"""Generate ui/js/shared/mascot-engine.js from the verified lab engine.

The clip/scheduler/props logic is lifted verbatim out of
design/mascot-lab/lab.template.html so the shipping code is the code that was
reviewed frame by frame in the lab. Only the shell around it is app-specific:
loading the SVGs at runtime, injecting CSS, the controller API, and the
rAF/visibility plumbing.

Run from the repo root:  python <this script>
"""

from pathlib import Path

ROOT = Path('C:/oss/nachmore/Kage')
LAB = (ROOT / 'design/mascot-lab/lab.template.html').read_text(encoding='utf-8')
lines = LAB.split('\n')


def grab(start_marker, end_marker):
    """Lines from the line starting with start_marker up to (not incl.) end_marker."""
    a = next(i for i, l in enumerate(lines) if l.startswith(start_marker))
    b = next(i for i, l in enumerate(lines) if i > a and l.startswith(end_marker))
    return '\n'.join(lines[a:b]).rstrip() + '\n'


utils = grab('function mulberry32(', '// ─── art ───')
art = grab('function makeArt(', 'let POSE_REG')
registration = grab('let POSE_REG', '// ─── scheduling helpers')
frames_clip = grab('function framesClip(', '// ─── Current engine')
tod = grab('const todFromHour =', 'const FEATURES')
tool_activity = grab('const TOOL_ACTIVITY =', 'const PROPS_BACK_SVG')
props = grab('const PROPS_BACK_SVG', 'function ProposedEngine(')
engine = grab('function ProposedEngine(', '// ─── columns')

# The lab's registration helper appends to document.body and reads ASSETS; keep
# it, but rename so the module's own loader owns when it runs.
registration = registration.replace('function measureRegistration()', 'function measureRegistration()')

OUT = ROOT / 'ui/js/shared/mascot-engine.js'

header = '''/**
 * Kage mascot animation engine.
 *
 * The clip/scheduler/props logic below is lifted verbatim from the design lab
 * (design/mascot-lab/), where every beat was reviewed frame by frame. Edit the
 * lab first, re-check it there, then regenerate — don't diverge the two.
 *
 * What this adds on top of the lab:
 *   - loads the real ui/assets SVGs at runtime and inlines them (the old
 *     controller used <img>, which CSS can't reach, so eyelids, gaze and
 *     per-pose registration were impossible)
 *   - one shared measure/registration pass, cached for every instance
 *   - the `createMascotController` API the windows already use, so existing
 *     call sites keep working, plus `signal()` for the new states
 *   - rAF driven, paused whenever the window is hidden, and honours
 *     prefers-reduced-motion
 *
 * Appearance note: inlining means page CSS *can* now reach the art, so the
 * body/eye colours are pinned to the raw art's black-on-white here. Without
 * that the themed `.kage-mascot-*` rules would turn the cat teal in dark mode,
 * which is not how it has ever looked. `invert` swaps the two inks.
 */

import { ensureOutlineFilter } from './mascot.js';

const NS = 'http://www.w3.org/2000/svg';

// Debug hooks the lab sets from the URL; fixed here.
const DEBUG_POSE = null;
const DEBUG_LID = null;
const DEBUG_VARIANT = null;

// ─── art files ───────────────────────────────────────────────────────────
const BASE_DIR = 'assets';
const WAVING = [1, 2, 3, 4, 5].map((i) => `animations/waving/kage-waving-f${i}.svg`);
const JUMPING = [1, 2, 3, 4, 5, 6, 7, 8].map((i) => `animations/jumping/kage-jumping-f${i}.svg`);
const POSES = [
    'happy',
    'winking',
    'interested',
    'looking-to-the-right',
    'looking-down',
    'sleeping',
    'magnifying-glass',
    'love',
    'in-cute-box',
    'coffee',
    'moon',
    'balloon',
    'balloon-looking-up',
    'dancing-with-bow',
    'conductor',
    'harmonica',
];

const ASSETS = { waving: {}, jumping: {}, poses: {} };

const BLACK = new Set(['#000000', '#000', 'black']);
const WHITE = new Set(['#ffffff', '#fff', 'white']);
const SKIP_TAGS = new Set(['namedview', 'metadata', 'title', 'desc']);

/**
 * Reduce a source SVG to `{ vb, svg }`: its viewBox plus cleaned inner markup
 * with the art's two fills swapped for classes. Editor cruft and ids are
 * dropped — ids would collide across the many copies an instance renders.
 */
function prepareAsset(doc) {
    const root = doc.querySelector('svg');
    const vb = root
        .getAttribute('viewBox')
        .trim()
        .split(/[\\s,]+/)
        .map(Number);

    const serialize = (el) => {
        const tag = el.tagName.split(':').pop();
        if (SKIP_TAGS.has(tag)) return '';
        const attrs = {};
        const classes = [];
        for (const { name, value } of el.attributes) {
            if (name === 'id' || name.includes(':')) continue;
            attrs[name] = value;
        }
        let fill = attrs.fill;
        delete attrs.fill;
        const style = attrs.style;
        delete attrs.style;
        const kept = [];
        if (style) {
            for (const decl of style.split(';')) {
                const idx = decl.indexOf(':');
                if (idx < 0) continue;
                const prop = decl.slice(0, idx).trim();
                const val = decl.slice(idx + 1).trim();
                if (prop === 'fill') fill = val;
                else kept.push(`${prop}:${val}`);
            }
        }
        if (fill != null) {
            const f = fill.toLowerCase();
            if (BLACK.has(f)) classes.push('m-body');
            else if (WHITE.has(f)) classes.push('m-eyes');
            else kept.push(`fill:${fill}`);
        }
        if (kept.length) attrs.style = kept.join(';');
        if (classes.length) attrs.class = classes.join(' ');
        const attrStr = Object.entries(attrs)
            .map(([k, v]) => ` ${k}="${String(v).replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;')}"`)
            .join('');
        const inner = [...el.children].map(serialize).join('');
        return inner ? `<${tag}${attrStr}>${inner}</${tag}>` : `<${tag}${attrStr}/>`;
    };

    return { vb, svg: [...root.children].map(serialize).join('') };
}

const _docCache = new Map();
async function fetchAsset(rel) {
    if (_docCache.has(rel)) return _docCache.get(rel);
    const promise = fetch(`${BASE_DIR}/${rel}`)
        .then((r) => r.text())
        .then((text) => prepareAsset(new DOMParser().parseFromString(text, 'image/svg+xml')));
    _docCache.set(rel, promise);
    return promise;
}

let _coreReady = null;
let _restReady = null;

/**
 * Load the frames needed to draw anything (the waving set is the base pose and
 * the wave; the jump set is the thinking loop), then measure. Poses stream in
 * afterwards — `show()` falls back to the base pose until one arrives, so a
 * slow load degrades to a still cat rather than a blank strip.
 */
function loadCoreArt() {
    if (_coreReady) return _coreReady;
    _coreReady = (async () => {
        const [waving, jumping] = await Promise.all([
            Promise.all(WAVING.map(fetchAsset)),
            Promise.all(JUMPING.map(fetchAsset)),
        ]);
        waving.forEach((a, i) => (ASSETS.waving[`waving-f${i + 1}`] = a));
        jumping.forEach((a, i) => (ASSETS.jumping[`jumping-f${i + 1}`] = a));
    })();
    return _coreReady;
}

function loadRestArt() {
    if (_restReady) return _restReady;
    _restReady = (async () => {
        await _coreReady;
        const loaded = await Promise.all(POSES.map((n) => fetchAsset(`kage-${n}.svg`).catch(() => null)));
        POSES.forEach((n, i) => {
            if (loaded[i]) ASSETS.poses[n] = loaded[i];
        });
        measureRegistration();
    })();
    return _restReady;
}

// ─── injected CSS ────────────────────────────────────────────────────────
let _cssInjected = false;
function ensureEngineCSS() {
    if (_cssInjected) return;
    _cssInjected = true;
    const style = document.createElement('style');
    style.textContent = `
.kage-mascot-view {
    /* Pinned, not themed: see the appearance note at the top of this file. */
    --body: #000000;
    --eyes: #ffffff;
}
.kage-mascot-view.inverted { --body: #ffffff; --eyes: #000000; }
.kage-mascot-view .rig { position: absolute; transform-origin: 50% 100%; }
.kage-mascot-view .art { position: absolute; display: none; overflow: visible; }
.kage-mascot-view .art.show { display: block; }
.kage-mascot-view .shadow {
    position: absolute; height: 3px; border-radius: 50%;
    background: var(--body); opacity: 0; transform-origin: 50% 50%;
}
.kage-mascot-view .fx { position: absolute; inset: 0; pointer-events: none; overflow: visible; }
.kage-mascot-view .fx span {
    position: absolute; color: #fff; font-weight: 700;
    text-shadow: 0 0 2px rgba(0, 0, 0, 0.35);
}
.kage-mascot-view .m-body { fill: var(--body); }
.kage-mascot-view .m-eyes { fill: var(--eyes); }
.kage-mascot-view .m-gaze { transform: translateX(calc(var(--gaze, 0) * 1px)); }
.kage-mascot-view .m-lid {
    transform-box: fill-box; transform-origin: 50% 0; transform: scaleY(var(--lid, 0));
}
/* The stroke hides the anti-aliased ring a same-size cover leaves behind. */
.kage-mascot-view .m-lid-shape {
    fill: var(--body); stroke: var(--body); stroke-width: 1px;
    vector-effect: non-scaling-stroke;
}
/* Props use the cat's two inks only. */
.kage-mascot-view .p-paper { fill: var(--eyes); stroke: var(--body); stroke-width: .9; }
.kage-mascot-view .p-pen { fill: none; stroke: var(--body); stroke-width: .65; stroke-linecap: round; }
.kage-mascot-view .p-ink { fill: var(--ink, var(--body)); stroke: var(--inkEdge, var(--eyes)); stroke-width: .5; }
.kage-mascot-view .p-ink-line { fill: none; stroke: var(--ink, var(--body)); stroke-width: .55; stroke-linecap: round; }
.kage-mascot-view .p-cut { fill: none; stroke: var(--inkEdge, var(--eyes)); stroke-width: .45; }
.kage-mascot-view .p-cut-fill { fill: var(--inkEdge, var(--eyes)); }
`;
    document.head.appendChild(style);
}

'''

footer = '''
// ─── controller ──────────────────────────────────────────────────────────

/**
 * Behaviour sets. `full` is the floating window (the surface you watch a
 * response on). `light` is the chat sidebar at 28px, where props would be
 * unreadable and situation animations belong on the main surface.
 */
const PROFILES = {
    full: {
        breathe: true, blink: true, waveEase: true, hop: 'drawn-arc', hopHeight: 35, hopRest: 0,
        landings: true, activities: true, hammerLight: true, situations: true, longWait: true,
        inputs: true, extHints: true, timeOfDay: true, summon: true, glances: true, doze: true,
        ponder: true, celebrate: true, poke: true, tod: 'auto', idleScale: 1,
    },
    light: {
        breathe: true, blink: true, waveEase: true, hop: 'drawn-arc', hopHeight: 20, hopRest: 0,
        landings: false, activities: false, hammerLight: true, situations: false, longWait: false,
        inputs: false, extHints: false, timeOfDay: true, summon: false, glances: true, doze: true,
        ponder: false, celebrate: true, poke: true, tod: 'auto', idleScale: 1,
    },
};

const prefersReducedMotion = () =>
    window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;

/**
 * Create a mascot in `container`.
 *
 * Keeps the previous controller's surface — `setActive` / `setIdle` / `pause` /
 * `resume` / `destroy` / `ready` / `state` — so existing call sites work
 * unchanged. `setActive`/`setIdle` are now thin adapters onto the think/idle
 * states; the `idle`/`periodic`/`preload` options are accepted and ignored,
 * since the engine owns its own idle behaviour.
 *
 * @param {HTMLElement} container
 * @param {object} [opts]
 * @param {number}  [opts.size=40]            cat width in px
 * @param {'full'|'light'} [opts.profile]     behaviour set (default: full)
 * @param {boolean} [opts.invert=false]       swap the two inks
 * @param {string|{color,radius}} [opts.outline]
 * @param {boolean} [opts.animations=true]    master switch (false → still cat)
 * @param {boolean} [opts.extensionHints=true]
 * @param {boolean} [opts.manualClock=false] don't run rAF; caller drives
 *   `advance(ms)`. Used by the engine harness to step deterministically.
 */
export function createMascotController(container, opts = {}) {
    ensureEngineCSS();
    const {
        size = 40,
        profile = 'full',
        invert = false,
        outline = null,
        animations = true,
        extensionHints = true,
        manualClock = false,
    } = opts;

    const features = { ...PROFILES[profile] };
    features.extHints = features.extHints && extensionHints;
    if (prefersReducedMotion()) features.reduced = true;

    container.style.position ||= 'relative';

    // The engine lays out against a fixed box; the strip's height changes with
    // the window, so the view is sized to the container and re-measured.
    // A fixed box, centred in the strip: the engine's geometry is measured
    // against it, and the strip's height changes with the window.
    const BOX_W = Math.round(size * 1.5);
    const BOX_H = Math.round(size * 2.5);
    const view = document.createElement('div');
    view.className = 'kage-mascot-view';
    if (invert) view.classList.add('inverted');
    view.style.cssText =
        `position:absolute;left:50%;top:50%;width:${BOX_W}px;height:${BOX_H}px;` +
        'transform:translate(-50%,-50%);overflow:visible;pointer-events:none;';
    container.appendChild(view);

    // A full-height hop would fly out of a collapsed strip (the window clips
    // it), so cap it at the headroom above the cat.
    const CAT_H_EST = size * 0.85;
    const maxHop = PROFILES[profile].hopHeight;
    const fitHop = () => {
        const h = container.clientHeight || BOX_H;
        features.hopHeight = Math.max(8, Math.min(maxHop, h / 2 - CAT_H_EST / 2 - 4));
    };
    fitHop();
    const ro = new ResizeObserver(fitHop);
    ro.observe(container);

    let engine = null;
    let frame = null;
    let destroyed = false;
    let paused = false;
    let t = 0;
    let last = 0;
    const pending = [];
    let state = 'idle';

    const rng = mulberry32(((Math.random() * 1e9) | 0) >>> 0);

    if (outline) {
        const color = typeof outline === 'string' ? outline : outline.color || '#38B2AC';
        const radius = (typeof outline === 'object' && outline.radius) || 2;
        const id = ensureOutlineFilter(color, radius);
        view.style.filter = `url(#${id}) drop-shadow(0 2px 4px rgba(0,0,0,.2))`;
    }

    const step = (now) => {
        frame = null;
        if (destroyed || paused || !engine) return;
        const dt = Math.min(50, now - (last || now));
        last = now;
        t += dt;
        engine.update(t, features);
        schedule();
    };
    const schedule = () => {
        if (destroyed || paused || manualClock || frame !== null) return;
        frame = requestAnimationFrame(step);
    };

    const ready = (async () => {
        await loadCoreArt();
        if (destroyed) return;
        // Poses stream in behind the first paint; the engine falls back to the
        // base pose for anything not loaded yet.
        loadRestArt();
        await _restReady;
        if (destroyed) return;
        engine = ProposedEngine(view, rng, { size });
        for (const [name, data] of pending.splice(0)) engine.event(name, t, features, data);
        last = performance.now();
        schedule();
    })();

    /** Drive a state change. See the lab for the full event vocabulary. */
    function signal(name, data) {
        if (destroyed) return;
        if (!animations && name !== 'reopen') return;
        if (name === 'think') state = 'active';
        else if (name === 'done' || name === 'reopen') state = 'idle';
        if (!engine) {
            pending.push([name, data]);
            return;
        }
        engine.event(name, t, features, data);
        schedule();
    }

    return {
        signal,
        /**
         * Advance the engine clock by `ms` in fixed 16ms steps. Only meaningful
         * with `manualClock`; lets a harness reach an exact moment without
         * depending on how often rAF happened to fire.
         */
        advance(ms) {
            if (!engine) return;
            for (let i = 0; i < Math.round(ms / 16); i++) {
                t += 16;
                engine.update(t, features);
            }
        },
        /** @deprecated kept for existing call sites — maps to the think state. */
        setActive() {
            signal('think');
        },
        /** @deprecated kept for existing call sites — maps back to idle. */
        setIdle() {
            signal('done');
        },
        pause() {
            paused = true;
            if (frame !== null) {
                cancelAnimationFrame(frame);
                frame = null;
            }
        },
        resume() {
            if (destroyed || !paused) return;
            paused = false;
            last = performance.now();
            schedule();
        },
        destroy() {
            destroyed = true;
            ro.disconnect();
            if (frame !== null) cancelAnimationFrame(frame);
            frame = null;
            engine = null;
            container.innerHTML = '';
        },
        ready,
        get state() {
            return state;
        },
    };
}
'''

body = '\n'.join([
    '// ─── utils (from the lab) ────────────────────────────────────────────────',
    utils,
    '// ─── art (from the lab) ──────────────────────────────────────────────────',
    art,
    registration,
    '// ─── scheduling helpers (from the lab) ───────────────────────────────────',
    frames_clip,
    tod,
    tool_activity,
    props,
    '// ─── engine (from the lab) ───────────────────────────────────────────────',
    engine,
])

# The lab hardcodes a 60x100 box with a 40px cat. Scale it to whatever width
# the caller asks for so every measured offset keeps its proportions.
_NL = chr(10)
_old_sig = _NL.join([
    'function ProposedEngine(view, rng) {',
    '    const W = 60, H = 100;',
    '    const CAT_W = 40;',
])
_new_sig = _NL.join([
    'function ProposedEngine(view, rng, opts = {}) {',
    '    const CAT_W = opts.size || 40;',
    '    const W = Math.round(CAT_W * 1.5), H = Math.round(CAT_W * 2.5);',
])
assert body.count(_old_sig) == 1, 'engine signature not found'
body = body.replace(_old_sig, _new_sig)

# biome's formatter is pinned to LF line endings, so write LF regardless
# of what Python's text mode would do on this platform.
cr = chr(13)
lf = chr(10)
OUT.write_bytes((header + body + footer).replace(cr + lf, lf).encode('utf-8'))
print(f'wrote {OUT} ({len(header + body + footer) // 1024} KB)')

# Keep the generated file formatter-clean so biome ci stays green.
import subprocess, sys
subprocess.run(['npx','biome','format','--write',str(OUT)], check=True, shell=sys.platform=='win32')

