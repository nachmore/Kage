// Shared context-usage helpers used by both the chat and floating windows.
//
// Both windows ask the agent for context usage via the `context` slash
// command, parse a "NN%" out of the reply, and paint a tiny 16px ring
// canvas. The parsing and drawing were copy-pasted in each window's god
// class; this module is the single source of truth. `parseContextPercent`
// is pure (unit-tested directly); `drawContextRing` takes the canvas and a
// track colour so each window keeps its own look (chat is theme-aware,
// floating always paints on a dark surface).

/**
 * Pull an integer percent (0–100) out of a `context` slash-command result.
 * Accepts the raw result object the Tauri command returns — uses its
 * `message` field when present, else the stringified object. Returns null
 * when no `NN%` token is found.
 *
 * @param {unknown} result
 * @returns {number|null}
 */
export function parseContextPercent(result) {
    const msg =
        result && typeof result === 'object' && 'message' in result && result.message
            ? String(result.message)
            : (JSON.stringify(result) ?? '');
    const match = msg.match(/(\d+)%/);
    if (!match) return null;
    const pct = parseInt(match[1], 10);
    return Number.isNaN(pct) ? null : pct;
}

/**
 * Map a context-usage percent to its ring colour: green under 75, yellow
 * 75–89, red 90+.
 *
 * @param {number} percent
 * @returns {string} hex colour
 */
export function contextRingColor(percent) {
    if (percent >= 90) return '#ef4444';
    if (percent >= 75) return '#eab308';
    return '#22c55e';
}

/**
 * The faint full-circle track colour behind the usage arc, picked to read
 * against the current theme: white-on-dark, black-on-light. Both windows
 * load the shared theme system (`theme.js`), which toggles
 * `body.dark-theme` / `body.light-theme`, so this is correct for chat and
 * floating alike.
 *
 * @returns {string}
 */
export function contextRingTrackColor() {
    const isDark = document.body.classList.contains('dark-theme');
    return isDark ? 'rgba(255,255,255,0.15)' : 'rgba(0,0,0,0.1)';
}

/**
 * Paint the 16px context-usage ring onto `canvas`: a faint full-circle
 * track plus a coloured arc spanning `percent` of the circle. No-op when
 * the canvas is missing or has no 2d context. The track colour defaults to
 * the theme-aware value; pass `opts.track` to override.
 *
 * @param {HTMLCanvasElement|null} canvas
 * @param {number} percent
 * @param {object} [opts]
 * @param {string} [opts.track] track (background ring) colour
 */
export function drawContextRing(canvas, percent, opts = {}) {
    if (!canvas || typeof canvas.getContext !== 'function') return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const size = 16;
    const cx = size / 2;
    const cy = size / 2;
    const r = 6;
    const lineWidth = 2;
    const track = opts.track || contextRingTrackColor();

    ctx.clearRect(0, 0, size, size);

    // Background ring (full circle track).
    ctx.beginPath();
    ctx.arc(cx, cy, r, 0, Math.PI * 2);
    ctx.strokeStyle = track;
    ctx.lineWidth = lineWidth;
    ctx.stroke();

    // Filled arc proportional to usage.
    if (percent > 0) {
        const start = -Math.PI / 2;
        const end = start + (Math.PI * 2 * Math.min(percent, 100)) / 100;
        ctx.beginPath();
        ctx.arc(cx, cy, r, start, end);
        ctx.strokeStyle = contextRingColor(percent);
        ctx.lineWidth = lineWidth;
        ctx.lineCap = 'round';
        ctx.stroke();
    }
}
