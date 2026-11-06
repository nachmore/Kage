/* Kage site: the cat's personality.
 *
 * Hero cat (peeking over the launcher): eyes follow the pointer, random
 * blinks, a happy face on hover/focus, a squash-and-stretch jump with hearts
 * on click/Enter, and reactions to the demo (ducks away when the launcher
 * hides, peeks up when the agent is thinking, glances at what's happening).
 *
 * Footer cat: sleeps until you hover, focus or tap it, then waves using the
 * app's own waving frames.
 *
 * All motion is transform/opacity. Pointer work is batched into one rAF, and
 * the hero cat does none of it (no gaze loop, glances or blinks) while it's
 * off screen. prefers-reduced-motion keeps the faces but drops gaze, blinks
 * and jumps.
 */
(() => {
    const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

    const peek = document.querySelector('[data-cat]');
    if (peek) initPeek(peek);

    const sleepy = document.querySelector('[data-sleepy]');
    if (sleepy) initSleepy(sleepy);

    function initPeek(el) {
        const tilt = el.querySelector('.cat__tilt');
        const launcher = document.querySelector('[data-lm="hero"]');
        // The irises are drawn looking inward: the cute cross-eyed rest pose.
        // While the cat looks at something, each pupil first centres on its
        // own eye (dx/dy: eye centre minus iris centre, in SVG units, read
        // from the art), then moves from there; the clip-path keeps it
        // inside the eye.
        const num = (node, attr) => Number(node?.getAttribute(attr)) || 0;
        const eyes = [...el.querySelectorAll('.cat__eye')]
            .map((eye) => {
                const white = eye.querySelector('.cat__white');
                const iris = eye.querySelector('.cat__iris');
                return {
                    pupil: eye.querySelector('.cat__pupil'),
                    dx: num(white, 'cx') - num(iris, 'cx'),
                    dy: num(white, 'cy') - num(iris, 'cy'),
                };
            })
            .filter((e) => e.pupil);
        const MAX_X = 6; // svg units a pupil may travel from its eye's centre
        const MAX_Y = 4;
        const TILT = 1.4; // degrees of head tilt per unit of gaze

        const aim = { x: 0, y: 0, active: false };
        // x/y: gaze offset; e: 0 = resting (cross-eyed), 1 = looking.
        const cur = { x: 0, y: 0, e: 0 };
        const target = { x: 0, y: 0, e: 0 };
        let rect = null; // measured at most once per scroll or resize
        // Without IntersectionObserver, assume the cat is always on screen.
        let onScreen = !('IntersectionObserver' in window);
        let lastPointer = 0;
        let frame = 0;
        let glanceTimer = 0;
        let gazeUntil = 0;
        let happyTimer = 0;

        function catBox() {
            if (!rect) rect = el.getBoundingClientRect();
            return rect;
        }

        function setTargetToPoint(px, py) {
            const r = catBox();
            const dx = px - (r.left + r.width / 2);
            const dy = py - (r.top + r.height * 0.36);
            const dist = Math.hypot(dx, dy) || 1;
            const reach = Math.min(1, dist / 240);
            target.x = (dx / dist) * MAX_X * reach;
            target.y = (dy / dist) * MAX_Y * reach;
            target.e = 1;
        }

        function step() {
            frame = 0;
            if (!onScreen) return;
            if (aim.active) setTargetToPoint(aim.x, aim.y);
            cur.x += (target.x - cur.x) * 0.18;
            cur.y += (target.y - cur.y) * 0.18;
            cur.e += (target.e - cur.e) * 0.18;
            for (const { pupil, dx, dy } of eyes) {
                const x = cur.e * dx + cur.x;
                const y = cur.e * dy + cur.y;
                pupil.style.transform = `translate(${x.toFixed(2)}px, ${y.toFixed(2)}px)`;
            }
            if (tilt) tilt.style.transform = `rotate(${(cur.x * TILT).toFixed(2)}deg)`;
            if (
                Math.abs(target.x - cur.x) > 0.01 ||
                Math.abs(target.y - cur.y) > 0.01 ||
                Math.abs(target.e - cur.e) > 0.005
            ) {
                frame = requestAnimationFrame(step);
            }
        }

        function kick() {
            if (!frame && onScreen && !reduce) frame = requestAnimationFrame(step);
        }

        function lookAtElement(node, dx = 0.5, dy = 0.5, hold = 1600) {
            if (!node || reduce || !onScreen) return;
            if (performance.now() - lastPointer < 2500) return;
            const r = node.getBoundingClientRect();
            aim.active = false;
            setTargetToPoint(r.left + r.width * dx, r.top + r.height * dy);
            gazeUntil = performance.now() + hold;
            kick();
        }

        // Back to the resting, cross-eyed look.
        function lookAhead() {
            target.x = 0;
            target.y = 0;
            target.e = 0;
            kick();
        }

        // Pointer gone quiet: drift back, now and then glance around.
        function glance() {
            const now = performance.now();
            if (now - lastPointer < 3000 || now < gazeUntil || document.hidden) return;
            aim.active = false;
            if (Math.random() < 0.55) lookAhead();
            else {
                target.x = (Math.random() * 2 - 1) * MAX_X;
                target.y = (Math.random() * 2 - 1) * MAX_Y * 0.6;
                target.e = 1;
                kick();
            }
        }

        // ── only while the cat is on screen ──
        // Off screen there's no gaze loop, no glancing and no pointer work.
        function setOnScreen(visible) {
            if (visible === onScreen) return;
            onScreen = visible;
            rect = null;
            if (visible) {
                if (!reduce && !glanceTimer) glanceTimer = setInterval(glance, 2600);
                kick();
            } else {
                cancelAnimationFrame(frame);
                frame = 0;
                clearInterval(glanceTimer);
                glanceTimer = 0;
            }
        }
        if ('IntersectionObserver' in window) {
            new IntersectionObserver(
                (entries) => setOnScreen(entries[entries.length - 1].isIntersecting),
                { rootMargin: '80px 0px' }
            ).observe(el);
        }

        // ── pointer gaze ──
        if (!reduce) {
            const invalidate = () => {
                rect = null;
            };
            window.addEventListener('scroll', invalidate, { passive: true });
            window.addEventListener('resize', invalidate, { passive: true });
            window.addEventListener(
                'pointermove',
                (e) => {
                    if (!onScreen) return;
                    aim.x = e.clientX;
                    aim.y = e.clientY;
                    aim.active = true;
                    lastPointer = performance.now();
                    kick();
                },
                { passive: true }
            );
            document.addEventListener(
                'pointerleave',
                () => {
                    aim.active = false;
                    lookAhead();
                },
                { passive: true }
            );
            if (onScreen) glanceTimer = setInterval(glance, 2600);
        }

        // ── blinking ──
        function blink() {
            el.classList.add('is-blink');
            setTimeout(() => el.classList.remove('is-blink'), 130);
        }
        function scheduleBlink() {
            setTimeout(
                () => {
                    if (!document.hidden && onScreen) {
                        blink();
                        if (Math.random() < 0.2) setTimeout(blink, 260);
                    }
                    scheduleBlink();
                },
                2200 + Math.random() * 4200
            );
        }
        if (!reduce) scheduleBlink();

        // ── hover, focus and petting ──
        const happy = (on) => el.classList.toggle('is-happy', on);
        el.addEventListener('pointerenter', (e) => {
            if (e.pointerType === 'mouse') happy(true);
        });
        el.addEventListener('pointerleave', () => happy(false));
        el.addEventListener('focus', () => happy(true));
        el.addEventListener('blur', () => happy(false));

        const HEART =
            '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 21s-7.5-4.6-9.6-9.2C.9 8.4 3 4.5 6.7 4.5c2.1 0 3.6 1.2 4.3 2.4h2c.7-1.2 2.2-2.4 4.3-2.4 3.7 0 5.8 3.9 4.3 7.3C19.5 16.4 12 21 12 21z"/></svg>';

        function hearts() {
            for (let i = 0; i < 4; i++) {
                const h = document.createElement('span');
                h.className = 'heart';
                h.innerHTML = HEART;
                h.style.left = `${30 + Math.random() * 40}%`;
                h.style.setProperty('--hx', `${(Math.random() * 2 - 1) * 40}px`);
                h.style.setProperty('--hr', `${(Math.random() * 2 - 1) * 30}deg`);
                h.style.animationDelay = `${i * 90}ms`;
                if (i % 2) h.style.color = '#f687b3';
                h.addEventListener('animationend', () => h.remove(), { once: true });
                el.appendChild(h);
            }
        }

        el.addEventListener('click', () => {
            happy(true);
            clearTimeout(happyTimer);
            happyTimer = setTimeout(() => {
                if (!el.matches(':hover')) happy(false);
            }, 1100);
            if (reduce) return;
            el.classList.remove('is-jump');
            void el.offsetWidth;
            el.classList.add('is-jump');
            hearts();
        });
        el.addEventListener('animationend', (e) => {
            if (e.animationName === 'cat-jump') el.classList.remove('is-jump');
        });

        // ── reacting to the demo ──
        // The demo starts with the window dismissed; stay out of sight too.
        if (launcher?.classList.contains('is-hidden')) el.classList.add('is-away');
        const answerArea = launcher?.querySelector('.lm__content');
        const input = launcher?.querySelector('.lm__input');
        document.addEventListener('kage:demo', (e) => {
            const s = e.detail.state;
            if (s === 'hide') {
                el.classList.add('is-away');
                el.classList.remove('is-curious');
            } else if (s === 'summon') {
                setTimeout(() => el.classList.remove('is-away'), 180);
            } else if (s === 'typing') {
                lookAtElement(input, 0.15, 0.5, 2200);
            } else if (s === 'thinking' || s === 'ask') {
                el.classList.add('is-curious');
                lookAtElement(answerArea, 0.4, 0.6, 2600);
            } else if (s === 'streaming') {
                lookAtElement(answerArea, 0.3, 0.7, 2600);
            } else if (s === 'done') {
                el.classList.remove('is-curious');
                if (!reduce && !el.matches(':hover')) {
                    happy(true);
                    setTimeout(() => {
                        if (!el.matches(':hover') && document.activeElement !== el) happy(false);
                    }, 900);
                }
            }
        });
    }

    function initSleepy(el) {
        const frames = [...el.querySelectorAll('.sleepy__wave img')];
        let timer = 0;
        let n = 0;
        let stopAt = 0;

        function show(i) {
            frames.forEach((f, k) => {
                if (k === i) f.setAttribute('data-on', '');
                else f.removeAttribute('data-on');
            });
        }

        function wake(ms) {
            stopAt = Math.max(stopAt, performance.now() + ms);
            if (timer) return;
            el.classList.add('is-awake');
            n = 0;
            show(n);
            if (reduce) return;
            timer = setInterval(() => {
                n = (n + 1) % frames.length;
                show(n);
                if (
                    performance.now() > stopAt &&
                    !el.matches(':hover') &&
                    document.activeElement !== el
                ) {
                    sleep();
                }
            }, 220);
        }

        function sleep() {
            clearInterval(timer);
            timer = 0;
            el.classList.remove('is-awake');
            show(-1);
        }

        el.addEventListener('pointerenter', () => wake(1600));
        el.addEventListener('focus', () => wake(1600));
        el.addEventListener('click', () => wake(2600));
        el.addEventListener('pointerleave', () => {
            stopAt = Math.min(stopAt, performance.now() + 500);
            if (reduce) sleep();
        });
        el.addEventListener('blur', () => {
            if (reduce) sleep();
        });
    }
})();
