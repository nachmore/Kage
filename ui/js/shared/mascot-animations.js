/**
 * Kage Mascot Animation Library
 *
 * Central registry of all mascot animations. Import an animation by name
 * and pass it directly to createAnimatedMascot().
 *
 * Usage:
 *   import { ANIMATIONS } from './mascot-animations.js';
 *   import { createAnimatedMascot } from './mascot.js';
 *   const anim = createAnimatedMascot({ ...ANIMATIONS.waving, size: 40 });
 *
 * Each entry contains the frames array, default playback settings,
 * and aspect ratio so the controller can size them correctly.
 */

const BASE = 'assets/animations';

export const ANIMATIONS = {
    waving: {
        frames: [
            `${BASE}/waving/kage-waving-f1.svg`,
            `${BASE}/waving/kage-waving-f2.svg`,
            `${BASE}/waving/kage-waving-f3.svg`,
            `${BASE}/waving/kage-waving-f4.svg`,
            `${BASE}/waving/kage-waving-f5.svg`,
        ],
        fps: 4,
        loop: true,
        aspect: 43 / 36,  // wider than tall
    },
    jumping: {
        frames: [
            `${BASE}/jumping/kage-jumping-f1.svg`,
            `${BASE}/jumping/kage-jumping-f2.svg`,
            `${BASE}/jumping/kage-jumping-f3.svg`,
            `${BASE}/jumping/kage-jumping-f4.svg`,
            `${BASE}/jumping/kage-jumping-f5.svg`,
            `${BASE}/jumping/kage-jumping-f6.svg`,
            `${BASE}/jumping/kage-jumping-f7.svg`,
            `${BASE}/jumping/kage-jumping-f8.svg`,
        ],
        fps: 8,
        loop: true,
        aspect: 45 / 73,  // taller than wide
    },
};
