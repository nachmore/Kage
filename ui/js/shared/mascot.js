/**
 * Kage Mascot — shared theme-aware SVG icon library.
 *
 * Usage:
 *   import { createMascot } from './shared/mascot.js';
 *   container.appendChild(createMascot({ size: 120, variant: 'default' }));
 *
 * The SVG uses CSS custom properties so it automatically adapts to
 * light/dark themes and custom theme overrides.
 *
 * CSS variables consumed (with fallbacks):
 *   --kage-mascot-body:  body fill color   (default: --kage-accent-light / #C09CFF in dark, #1a1a1a in light)
 *   --kage-mascot-eyes:  eye area fill     (default: --kage-bg / #1e1e1e in dark, #ffffff in light)
 *   --kage-mascot-outline: outline color for SVG filter (default: #38B2AC)
 *   --kage-mascot-invert: set to "1" to render mascot white (default: "0")
 */

// ─── SVG source ─────────────────────────────────────────────────────────────
const MASCOT_SVG_PATH = 'assets/kage-icon.svg';
const TERMINATOR_SVG_PATH = 'assets/kage-terminator.svg';
const _svgCache = new Map(); // path → parsed SVG document
let _terminatorMode = false;

/** Set terminator mode globally. Call once at startup. */
export function setTerminatorMode(enabled) { _terminatorMode = enabled; }

/** Check if terminator mode is active. */
export function isTerminatorMode() { return _terminatorMode; }

/**
 * Read mascot theme settings from CSS custom properties.
 * Returns { outlineColor, invert } resolved from the current theme.
 */
export function getMascotThemeSettings() {
    const style = getComputedStyle(document.documentElement);
    const outlineColor = style.getPropertyValue('--kage-mascot-outline').trim()
        || style.getPropertyValue('--kage-accent').trim()
        || '#319795';
    const invertVal = style.getPropertyValue('--kage-mascot-invert').trim();
    const invert = invertVal === '1' || invertVal === 'true';
    return { outlineColor, invert };
}

/** Fetch and parse an SVG file, caching the result. */
async function loadSVG(path) {
    if (_svgCache.has(path)) return _svgCache.get(path);
    const resp = await fetch(path);
    const text = await resp.text();
    const parser = new DOMParser();
    const doc = parser.parseFromString(text, 'image/svg+xml');
    _svgCache.set(path, doc);
    return doc;
}

/**
 * Build a themed mascot SVG element from the source file.
 * Applies kage-mascot-body/eyes classes to black/white fills.
 */
function buildMascotFromSource(doc, size) {
    const srcSvg = doc.querySelector('svg');
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    svg.setAttribute('viewBox', srcSvg.getAttribute('viewBox'));
    svg.setAttribute('width', size);
    svg.setAttribute('height', size);
    svg.setAttribute('aria-hidden', 'true');

    // Clone all children from the source
    for (const child of srcSvg.childNodes) {
        svg.appendChild(child.cloneNode(true));
    }

    // Apply theme classes: black fills → body, white fills → eyes
    svg.querySelectorAll('*').forEach(el => {
        const style = el.getAttribute('style') || '';
        const fill = el.getAttribute('fill') || '';
        if (style.match(/fill\s*:\s*#000000/i) || fill === '#000000') {
            el.removeAttribute('style');
            el.classList.add('kage-mascot-body');
        }
        if (style.match(/fill\s*:\s*#ffffff/i) || fill === '#ffffff') {
            el.removeAttribute('style');
            el.classList.add('kage-mascot-eyes');
        }
    });

    return svg;
}

// ─── CSS (injected once) ────────────────────────────────────────────────────
let _cssInjected = false;
let _filterCounter = 0;

function ensureCSS() {
    if (_cssInjected) return;
    _cssInjected = true;
    const style = document.createElement('style');
    style.textContent = `
        .kage-mascot-body {
            fill: var(--kage-mascot-body, var(--kage-accent-light, #C09CFF));
        }
        .kage-mascot-eyes {
            fill: var(--kage-mascot-eyes, var(--kage-bg, #1e1e1e));
        }
        body.light-theme .kage-mascot-body {
            fill: var(--kage-mascot-body, #1a1a1a);
        }
        body.light-theme .kage-mascot-eyes {
            fill: var(--kage-mascot-eyes, #ffffff);
        }
        .kage-mascot-inverted { filter: invert(1); }
    `;
    document.head.appendChild(style);
}

/**
 * Create an SVG filter element for an outline/stroke effect on <img> elements.
 * Appended to the document once and referenced by CSS filter: url(#id).
 * @param {string} color  Outline color (CSS color string)
 * @param {number} [radius=2]  Outline thickness in px
 * @returns {string} The filter ID to use in `filter: url(#id)`
 */
export function ensureOutlineFilter(color, radius = 2) {
    const id = `kage-outline-${++_filterCounter}`;
    let svgHost = document.getElementById('kage-svg-filters');
    if (!svgHost) {
        svgHost = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
        svgHost.id = 'kage-svg-filters';
        svgHost.setAttribute('width', '0');
        svgHost.setAttribute('height', '0');
        svgHost.style.cssText = 'position:absolute;pointer-events:none;';
        document.body.appendChild(svgHost);
    }
    const filter = document.createElementNS('http://www.w3.org/2000/svg', 'filter');
    filter.id = id;
    filter.setAttribute('x', '-15%');
    filter.setAttribute('y', '-15%');
    filter.setAttribute('width', '130%');
    filter.setAttribute('height', '130%');
    filter.innerHTML = `
        <feMorphology in="SourceAlpha" operator="dilate" radius="${radius}" result="expanded"/>
        <feFlood flood-color="${color}" result="color"/>
        <feComposite in="color" in2="expanded" operator="in" result="outline"/>
        <feMerge>
            <feMergeNode in="outline"/>
            <feMergeNode in="SourceGraphic"/>
        </feMerge>
    `;
    svgHost.appendChild(filter);
    return id;
}

// ─── Public API ─────────────────────────────────────────────────────────────

/**
 * Create a theme-aware mascot SVG element from the kage-icon.svg source.
 * Returns a Promise that resolves to the SVG element.
 *
 * @param {object} [opts]
 * @param {number}  [opts.size=48]        Width & height in px
 * @param {string}  [opts.className='']   Extra CSS class(es) to add
 * @param {boolean} [opts.invert=false]   Invert colors (light mascot on dark bg)
 * @param {string|object} [opts.outline]  Outline color string or {color, radius}
 * @returns {Promise<SVGSVGElement>}
 */
export async function createMascot(opts = {}) {
    ensureCSS();
    const { size = 48, className = '', invert = false, outline = null, src = null } = opts;

    const doc = await loadSVG(src || MASCOT_SVG_PATH);
    const svg = buildMascotFromSource(doc, size);
    svg.classList.add('kage-mascot');
    if (invert) svg.classList.add('kage-mascot-inverted');
    if (className) className.split(' ').forEach(c => c && svg.classList.add(c));

    if (outline) {
        const color = typeof outline === 'string' ? outline : (outline.color || '#38B2AC');
        const radius = (typeof outline === 'object' && outline.radius) || 2;
        const filterId = ensureOutlineFilter(color, radius);
        svg.style.filter = `url(#${filterId})`;
    }

    return svg;
}

/**
 * Return the mascot as an HTML string (for innerHTML / template literals).
 * Uses a simple placeholder that gets hydrated when the SVG loads.
 * For synchronous contexts — prefer createMascot() when possible.
 */
export function mascotHTML(opts = {}) {
    ensureCSS();
    const { size = 48, className = '', invert = false } = opts;
    const cls = ['kage-mascot', invert ? 'kage-mascot-inverted' : '', className].filter(Boolean).join(' ');
    const id = `kage-mascot-${++_filterCounter}`;
    const svgPath = _terminatorMode ? TERMINATOR_SVG_PATH : MASCOT_SVG_PATH;
    // Kick off async load to hydrate the placeholder
    loadSVG(svgPath).then(doc => {
        const placeholder = document.getElementById(id);
        if (!placeholder) return;
        const svg = buildMascotFromSource(doc, size);
        svg.classList.add(...cls.split(' ').filter(Boolean));
        if (_terminatorMode) {
            const filterId = ensureOutlineFilter('#ef4444', 1);
            svg.style.filter = `url(#${filterId})`;
        }
        placeholder.replaceWith(svg);
    });
    return `<span id="${id}" class="${cls}" style="display:inline-block;width:${size}px;height:${size}px;"></span>`;
}



// ─── Animation support ──────────────────────────────────────────────────────

/** Preload an image and return a promise that resolves when loaded. */
const _preloadCache = new Map();
function preloadImg(src) {
    if (_preloadCache.has(src)) return _preloadCache.get(src);
    const p = new Promise((resolve) => {
        const img = new Image();
        img.onload = () => resolve(src);
        img.onerror = () => resolve(src); // resolve anyway, don't block
        img.src = src;
    });
    _preloadCache.set(src, p);
    return p;
}

/**
 * Create an animated mascot that cycles through SVG frame files.
 * All frames are pre-created as stacked <img> elements.
 */
export function createAnimatedMascot(opts = {}) {
    ensureCSS();
    const { frames: framePaths = [], size = 40, fps = 6, loop = true, autoplay = true, className = '', aspect, invert = false, outline = null } = opts;

    // Compute width/height from size + aspect ratio.
    let w, h;
    if (aspect && aspect < 1) {
        h = size; w = Math.round(size * aspect);
    } else if (aspect && aspect > 1) {
        w = size; h = Math.round(size / aspect);
    } else {
        w = size; h = size;
    }

    const container = document.createElement('div');
    container.style.cssText = `position:relative;width:${w}px;height:${h}px;`;
    container.classList.add('kage-mascot', 'kage-mascot-animated');
    if (invert) container.classList.add('kage-mascot-inverted');
    if (className) className.split(' ').forEach(c => c && container.classList.add(c));

    // Set up outline filter if requested
    let filterStyle = '';
    if (outline) {
        const color = typeof outline === 'string' ? outline : (outline.color || '#38B2AC');
        const radius = outline.radius || 2;
        const filterId = ensureOutlineFilter(color, radius);
        filterStyle = `filter:url(#${filterId});`;
    }

    const imgs = framePaths.map((src, i) => {
        const img = document.createElement('img');
        img.src = src;
        preloadImg(src); // register in cache so later preloadImg calls share the same promise
        img.width = w;
        img.height = h;
        img.style.cssText = `position:absolute;top:0;left:0;display:${i === 0 ? 'block' : 'none'};${filterStyle}`;
        img.setAttribute('aria-hidden', 'true');
        img.draggable = false;
        container.appendChild(img);
        return img;
    });

    let currentFrame = 0;
    let intervalId = null;
    let _onComplete = null;

    function showFrame(idx) {
        imgs[currentFrame].style.display = 'none';
        currentFrame = idx % imgs.length;
        imgs[currentFrame].style.display = 'block';
    }

    function play() {
        if (intervalId || imgs.length < 2) return;
        showFrame(0);
        intervalId = setInterval(() => {
            const next = currentFrame + 1;
            if (next >= imgs.length && !loop) {
                stop();
                if (_onComplete) { const cb = _onComplete; _onComplete = null; cb(); }
                return;
            }
            showFrame(next);
        }, 1000 / fps);
    }

    function stop() {
        if (intervalId) { clearInterval(intervalId); intervalId = null; }
    }

    function destroy() { stop(); container.remove(); }

    function playOnce(cb) {
        stop();
        _onComplete = cb || null;
        showFrame(0);
        intervalId = setInterval(() => {
            const next = currentFrame + 1;
            if (next >= imgs.length) {
                stop();
                if (_onComplete) { const fn = _onComplete; _onComplete = null; fn(); }
                return;
            }
            showFrame(next);
        }, 1000 / fps);
    }

    /** Hide this animation (all frames hidden). */
    function hide() { stop(); imgs.forEach(img => img.style.display = 'none'); }

    /** Show frame 0 without playing. */
    function showIdle() { stop(); showFrame(0); }

    if (autoplay && imgs.length > 1) play();

    return { element: container, play, playOnce, stop, destroy, showFrame, hide, showIdle };
}


// ─── Mascot Controller ──────────────────────────────────────────────────────

/**
 * High-level controller that manages a mascot container with multiple
 * pre-built animations. All frames are preloaded at init time so
 * transitions are instant with no flash.
 */
export function createMascotController(container, opts = {}) {
    const {
        size = 40,
        idle = null,
        periodic = null,
        periodicInterval = 10000,
        periodicJitter = 2000,
        invert = false,
        outline = null,
    } = opts;

    // Pre-build all animations and add them to the container.
    // They stay in the DOM permanently — we just show/hide them.
    const anims = new Map(); // key → { anim, def }
    let currentKey = null;
    let periodicTimer = null;
    let state = 'idle'; // 'idle' | 'periodic' | 'active'

    function getOrCreate(animDef) {
        const key = animDef.frames.join('|');
        if (anims.has(key)) return { key, ...anims.get(key) };
        const anim = createAnimatedMascot({ ...animDef, size, autoplay: false, loop: false, invert, outline });
        anim.element.style.display = 'none'; // hidden until switched to
        container.appendChild(anim.element);
        anims.set(key, { anim, def: animDef });
        return { key, anim, def: animDef };
    }

    function switchTo(key) {
        for (const [k, { anim }] of anims) {
            if (k === key) {
                anim.element.style.display = '';
            } else {
                anim.stop();
                if (anim._loopInterval) { clearInterval(anim._loopInterval); anim._loopInterval = null; }
                anim.element.style.display = 'none';
            }
        }
        currentKey = key;
    }

    function showStatic() {
        if (!idle) {
            container.appendChild(createMascot({ size }));
            return;
        }
        const { key, anim } = getOrCreate(idle);
        switchTo(key);
        anim.showIdle();
    }

    function jitteredDelay() {
        return periodicInterval + (Math.random() * 2 - 1) * periodicJitter;
    }

    function schedulePeriodicPlay() {
        if (periodicTimer) clearTimeout(periodicTimer);
        if (!periodic) return;
        periodicTimer = setTimeout(() => {
            if (state !== 'idle') { schedulePeriodicPlay(); return; }
            playPeriodic();
        }, jitteredDelay());
    }

    function playPeriodic() {
        state = 'periodic';
        const { key, anim } = getOrCreate(periodic);
        switchTo(key);
        anim.playOnce(() => {
            if (state === 'periodic') {
                state = 'idle';
                showStatic();
                schedulePeriodicPlay();
            }
        });
    }

    function setActive(animDef, activeSize) {
        if (periodicTimer) { clearTimeout(periodicTimer); periodicTimer = null; }
        const useSize = activeSize || size;
        const sizeKey = animDef.frames.join('|') + '@' + useSize;

        // Already playing this exact animation — nothing to do
        if (state === 'active' && currentKey === sizeKey) return;

        // Stop any existing loop intervals before starting a new one
        for (const { anim: a } of anims.values()) {
            if (a._loopInterval) { clearInterval(a._loopInterval); a._loopInterval = null; }
        }

        state = 'active';
        let entry;
        if (anims.has(sizeKey)) {
            entry = { key: sizeKey, ...anims.get(sizeKey) };
        } else {
            const anim = createAnimatedMascot({ ...animDef, size: useSize, autoplay: false, loop: false, invert, outline });
            anim.element.style.display = 'none';
            container.appendChild(anim.element);
            anims.set(sizeKey, { anim, def: animDef });
            entry = { key: sizeKey, anim, def: animDef };
        }
        const { key, anim } = entry;
        switchTo(key);
        // Start looping playback
        const frameFps = animDef.fps || 6;
        const frameCount = animDef.frames.length;
        let frame = 0;
        anim.showFrame(0);
        const ivl = setInterval(() => {
            frame = (frame + 1) % frameCount;
            anim.showFrame(frame);
        }, 1000 / frameFps);
        anim._loopInterval = ivl;
    }

    function setIdle(playTransition = true) {
        // Already idle (or periodic which is transitioning to idle) — skip
        if (state === 'idle' || state === 'periodic') return;

        // Stop any active loop
        for (const { anim } of anims.values()) {
            if (anim._loopInterval) { clearInterval(anim._loopInterval); anim._loopInterval = null; }
        }
        if (playTransition && periodic) {
            playPeriodic();
        } else {
            state = 'idle';
            showStatic();
            schedulePeriodicPlay();
        }
    }

    let _destroyed = false;
    let _preloadTimer = null;

    function destroy() {
        _destroyed = true;
        if (periodicTimer) clearTimeout(periodicTimer);
        if (_preloadTimer) { clearTimeout(_preloadTimer); _preloadTimer = null; }
        for (const { anim } of anims.values()) {
            anim.stop();
            if (anim._loopInterval) clearInterval(anim._loopInterval);
        }
        container.innerHTML = '';
    }

    let _pausedState = null;

    /** Freeze the mascot on its current idle frame and stop all timers. */
    function pause() {
        if (_pausedState) return; // already paused
        _pausedState = state;
        if (periodicTimer) { clearTimeout(periodicTimer); periodicTimer = null; }
        for (const { anim } of anims.values()) {
            if (anim._loopInterval) { clearInterval(anim._loopInterval); anim._loopInterval = null; }
        }
        // Return to idle — show placeholder or static frame
        showStatic();
        state = 'paused';
    }

    /** Resume from where we left off. */
    function resume() {
        if (!_pausedState) return;
        const prev = _pausedState;
        _pausedState = null;
        if (prev === 'active') {
            // Can't resume active without knowing which animation — just go idle
            state = 'idle';
            showStatic();
            schedulePeriodicPlay();
        } else {
            state = 'idle';
            showStatic();
            schedulePeriodicPlay();
        }
    }

    // Show the idle frame immediately (frame 0 is in the DOM as an <img>).
    showStatic();

    // Promise that resolves when the first visible frame has loaded.
    // Callers can await this before showing the window.
    const firstFrameSrc = idle?.frames?.[0];
    const readyPromise = firstFrameSrc
        ? new Promise(resolve => {
            // Find the frame 0 <img> that createAnimatedMascot just created
            const img = container.querySelector('img[src$="' + firstFrameSrc.split('/').pop() + '"]');
            if (img && img.complete) { resolve(); }
            else if (img) { img.onload = resolve; img.onerror = resolve; }
            else { resolve(); }
        })
        : Promise.resolve();

    // Preload remaining frames in the background for smooth animation later
    const allFrames = new Set();
    if (idle) idle.frames.forEach(f => allFrames.add(f));
    if (periodic) periodic.frames.forEach(f => allFrames.add(f));
    if (opts.preload) {
        for (const animDef of opts.preload) {
            animDef.frames.forEach(f => allFrames.add(f));
        }
    }

    Promise.all([...allFrames].map(preloadImg)).then(() => {
        if (_destroyed) return;
        if (periodic) {
            _preloadTimer = setTimeout(() => {
                _preloadTimer = null;
                if (!_destroyed && state === 'idle') playPeriodic();
            }, 500);
        }
    });

    return { setActive, setIdle, pause, resume, destroy, ready: readyPromise, get state() { return state; } };
}
