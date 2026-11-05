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
 * All motion is transform/opacity. Pointer work is batched into one rAF.
 * prefers-reduced-motion keeps the faces but drops gaze, blinks and jumps.
 */
(() => {
    const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

    const peek = document.querySelector('[data-cat]');
    if (peek) initPeek(peek);

    const sleepy = document.querySelector('[data-sleepy]');
    if (sleepy) initSleepy(sleepy);

    function initPeek(el) {
        const pupils = el.querySelectorAll('.cat__pupil');
        const tilt = el.querySelector('.cat__tilt');
        const launcher = document.querySelector('[data-lm="hero"]');
        const MAX_X = 3.4; // svg units the pupils may travel
        const MAX_Y = 2.6;

        const aim = { x: 0, y: 0, active: false };
        const cur = { x: 0, y: 0 };
        const target = { x: 0, y: 0 };
        let lastPointer = 0;
        let frame = 0;
        let gazeUntil = 0;
        let happyTimer = 0;

        function setTargetToPoint(px, py) {
            const r = el.getBoundingClientRect();
            const cx = r.left + r.width / 2;
            const cy = r.top + r.height * 0.36;
            const dx = px - cx;
            const dy = py - cy;
            const dist = Math.hypot(dx, dy) || 1;
            const reach = Math.min(1, dist / 240);
            target.x = (dx / dist) * MAX_X * reach;
            target.y = (dy / dist) * MAX_Y * reach;
        }

        function step() {
            frame = 0;
            if (aim.active) setTargetToPoint(aim.x, aim.y);
            cur.x += (target.x - cur.x) * 0.18;
            cur.y += (target.y - cur.y) * 0.18;
            const t = `translate(${cur.x.toFixed(2)}px, ${cur.y.toFixed(2)}px)`;
            pupils.forEach((p) => {
                p.style.transform = t;
            });
            tilt.style.transform = `rotate(${(cur.x * 0.8).toFixed(2)}deg)`;
            if (Math.abs(target.x - cur.x) > 0.01 || Math.abs(target.y - cur.y) > 0.01) {
                frame = requestAnimationFrame(step);
            }
        }

        function kick() {
            if (!frame) frame = requestAnimationFrame(step);
        }

        function lookAtElement(node, dx = 0.5, dy = 0.5, hold = 1600) {
            if (!node || reduce) return;
            if (performance.now() - lastPointer < 2500) return;
            const r = node.getBoundingClientRect();
            aim.active = false;
            setTargetToPoint(r.left + r.width * dx, r.top + r.height * dy);
            gazeUntil = performance.now() + hold;
            kick();
        }

        function lookAhead() {
            target.x = 0;
            target.y = 0;
            kick();
        }

        // ── pointer gaze ──
        if (!reduce) {
            window.addEventListener(
                'pointermove',
                (e) => {
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
            // Pointer gone quiet: drift back, now and then glance around.
            setInterval(() => {
                const now = performance.now();
                if (now - lastPointer < 3000 || now < gazeUntil || document.hidden) return;
                aim.active = false;
                if (Math.random() < 0.55) lookAhead();
                else {
                    target.x = (Math.random() * 2 - 1) * MAX_X;
                    target.y = (Math.random() * 2 - 1) * MAX_Y * 0.6;
                    kick();
                }
            }, 2600);
        }

        // ── blinking ──
        function blink() {
            el.classList.add('is-blink');
            setTimeout(() => el.classList.remove('is-blink'), 130);
        }
        function scheduleBlink() {
            setTimeout(
                () => {
                    if (!document.hidden) {
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
