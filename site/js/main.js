/* Kage site: page-level behaviour.
 *
 * - scroll reveals (IntersectionObserver). This runs first, and the CSS has
 *   a failsafe that shows everything if this script never gets this far
 *   (see .reveal-ready in site.css). The hero copy doesn't depend on it.
 * - platform-aware labels ([data-mac] on a Mac) and the recommended
 *   download card
 * - scroll progress, nav state, multi-layer hero parallax (scroll +
 *   pointer), section parallax and 3D tilt on screenshots, pointer
 *   spotlights on cards
 * - the mobile menu, the marquee pause buttons, the live theme/language
 *   switcher, the copy button, and the latest Nightly's date from the
 *   GitHub API (silently skipped if it fails)
 *
 * One rAF loop does all scroll/pointer work; listeners are passive and only
 * schedule it. Everything written per frame is a transform.
 */
(() => {
    const doc = document.documentElement;
    const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    const fine = window.matchMedia('(pointer: fine)').matches;
    const os = doc.dataset.os;
    const isMac = os === 'mac';
    const $$ = (sel, el = document) => [...el.querySelectorAll(sel)];

    // ─── reveals ──────────────────────────────────────────────────────

    const reveals = $$('.reveal');
    const counters = $$('[data-count]');

    // Until <html> has .reveal-ready, a CSS animation shows every .reveal
    // 2.5 s after it's first styled. If this script got here after that
    // (slow network, busy CPU), keep everything shown instead of hiding it
    // again only to fade it back in.
    function failsafeFired() {
        if (performance.now() < 2400) return false;
        const probe = reveals.find((el) => !el.classList.contains('reveal--tilt'));
        return !!probe && getComputedStyle(probe).opacity === '1';
    }

    if (reduce || !('IntersectionObserver' in window) || failsafeFired()) {
        reveals.forEach((el) => el.classList.add('is-in'));
    } else {
        const io = new IntersectionObserver(
            (entries) => {
                for (const e of entries) {
                    if (!e.isIntersecting) continue;
                    e.target.classList.add('is-in');
                    io.unobserve(e.target);
                    if (e.target.matches('.stats')) countUp();
                }
            },
            { rootMargin: '0px 0px -10% 0px', threshold: 0 }
        );
        reveals.forEach((el) => io.observe(el));
    }
    // Cancels the failsafe: from here on the observer owns the reveals.
    doc.classList.add('reveal-ready');

    function countUp() {
        for (const el of counters) {
            const end = Number(el.dataset.count);
            if (!(end > 5)) continue;
            const t0 = performance.now();
            const tick = (t) => {
                const k = Math.min(1, (t - t0) / 1100);
                el.textContent = String(Math.round(end * (1 - (1 - k) ** 3)));
                if (k < 1) requestAnimationFrame(tick);
            };
            el.textContent = '0';
            requestAnimationFrame(tick);
        }
    }

    // ─── platform labels ──────────────────────────────────────────────

    if (isMac) {
        for (const el of $$('[data-mac]')) el.textContent = el.dataset.mac;
    }
    const card = document.querySelector(`[data-platform="${isMac ? 'mac' : 'windows'}"]`);
    if (isMac || os === 'windows') {
        card?.classList.add('is-recommended');
        const label = document.querySelector('[data-dl-label] span');
        if (label) label.textContent = isMac ? 'Download for macOS' : 'Download for Windows';
    }

    // ─── scroll + pointer loop ────────────────────────────────────────

    const nav = document.querySelector('[data-nav]');
    const bar = document.querySelector('.progress span');
    const hero = document.querySelector('.hero');
    // data-depth is how far a layer lags the page on scroll (bigger =
    // farther away); data-px is how far it follows the pointer (bigger =
    // nearer). Walking from the nearest layer outwards, no layer may follow
    // the pointer more than a nearer one, so the two cues always agree on
    // the order even if the markup's numbers drift.
    const layers = reduce
        ? []
        : $$('.sky [data-depth]')
              .map((el) => ({
                  el,
                  depth: Number(el.dataset.depth) * (fine ? 1 : 0.6),
                  px: fine ? Number(el.dataset.px || 0) : 0,
              }))
              .sort((a, b) => a.depth - b.depth);
    let pxCap = Number.POSITIVE_INFINITY;
    for (const l of layers) {
        l.px = Math.min(l.px, pxCap);
        pxCap = l.px;
    }
    const pars =
        reduce || !fine
            ? []
            : $$('[data-parallax]').map((el) => ({
                  el,
                  f: Number(el.dataset.parallax),
                  top: 0,
                  h: 0,
              }));

    let vh = window.innerHeight;
    let docH = doc.scrollHeight;
    let heroH = hero ? hero.offsetHeight : 0;
    const pointer = { x: 0, y: 0, cx: 0, cy: 0 };
    let raf = 0;
    let solid = null;

    function docTop(el) {
        let y = 0;
        for (let n = el; n; n = n.offsetParent) y += n.offsetTop;
        return y;
    }

    function measure() {
        vh = window.innerHeight;
        docH = doc.scrollHeight;
        heroH = hero ? hero.offsetHeight : 0;
        for (const p of pars) {
            p.top = docTop(p.el);
            p.h = p.el.offsetHeight;
        }
    }

    function frame() {
        raf = 0;
        const sy = window.scrollY;

        const isSolid = sy > 16;
        if (isSolid !== solid) {
            solid = isSolid;
            nav?.classList.toggle('is-solid', isSolid);
        }
        if (bar)
            bar.style.transform = `scaleX(${Math.min(1, sy / Math.max(1, docH - vh)).toFixed(4)})`;

        let again = false;
        if (layers.length) {
            pointer.cx += (pointer.x - pointer.cx) * 0.08;
            pointer.cy += (pointer.y - pointer.cy) * 0.08;
            again =
                Math.abs(pointer.x - pointer.cx) > 0.002 ||
                Math.abs(pointer.y - pointer.cy) > 0.002;
            if (sy < heroH + 50) {
                for (const l of layers) {
                    const x = -pointer.cx * l.px;
                    const y = sy * l.depth - pointer.cy * l.px;
                    l.el.style.transform = `translate3d(${x.toFixed(2)}px, ${y.toFixed(2)}px, 0)`;
                }
            }
        }
        for (const p of pars) {
            const offset = p.top + p.h / 2 - sy - vh / 2;
            if (Math.abs(offset) > vh * 1.2) continue;
            p.el.style.transform = `translate3d(0, ${(-offset * p.f).toFixed(2)}px, 0)`;
        }
        if (again) schedule();
    }

    function schedule() {
        if (!raf) raf = requestAnimationFrame(frame);
    }

    window.addEventListener('scroll', schedule, { passive: true });
    window.addEventListener(
        'resize',
        () => {
            measure();
            schedule();
        },
        { passive: true }
    );
    if (fine && layers.length) {
        window.addEventListener(
            'pointermove',
            (e) => {
                if (window.scrollY > heroH) return;
                pointer.x = (e.clientX / window.innerWidth) * 2 - 1;
                pointer.y = (e.clientY / vh) * 2 - 1;
                schedule();
            },
            { passive: true }
        );
    }
    if ('ResizeObserver' in window) {
        new ResizeObserver(() => {
            measure();
            schedule();
        }).observe(document.body);
    }
    measure();
    frame();

    // ─── 3D tilt on screenshots ───────────────────────────────────────

    if (fine && !reduce) {
        for (const fig of $$('[data-tilt]')) {
            const frameEl = fig.querySelector('.frame');
            if (!frameEl) continue;
            let pending = null;
            let rafId = 0;
            const apply = () => {
                rafId = 0;
                if (!pending) return;
                const { x, y } = pending;
                frameEl.style.transform = `rotateX(${(-y * 5).toFixed(2)}deg) rotateY(${(x * 7).toFixed(2)}deg)`;
                frameEl.style.setProperty('--sx', `${(x * 30 - 10).toFixed(1)}%`);
            };
            fig.addEventListener(
                'pointermove',
                (e) => {
                    const r = frameEl.getBoundingClientRect();
                    pending = {
                        x: (e.clientX - r.left) / r.width - 0.5,
                        y: (e.clientY - r.top) / r.height - 0.5,
                    };
                    frameEl.classList.add('is-tilting');
                    if (!rafId) rafId = requestAnimationFrame(apply);
                },
                { passive: true }
            );
            fig.addEventListener('pointerleave', () => {
                pending = null;
                frameEl.classList.remove('is-tilting');
                frameEl.style.transform = '';
                frameEl.style.removeProperty('--sx');
            });
        }
    }

    // ─── card spotlights ──────────────────────────────────────────────

    if (fine) {
        for (const el of $$('[data-spot]')) {
            let rafId = 0;
            let pt = null;
            el.addEventListener(
                'pointermove',
                (e) => {
                    pt = e;
                    if (rafId) return;
                    rafId = requestAnimationFrame(() => {
                        rafId = 0;
                        const r = el.getBoundingClientRect();
                        el.style.setProperty('--mx', `${(pt.clientX - r.left).toFixed(0)}px`);
                        el.style.setProperty('--my', `${(pt.clientY - r.top).toFixed(0)}px`);
                    });
                },
                { passive: true }
            );
        }
    }

    // ─── mobile menu ──────────────────────────────────────────────────

    // A <details> sheet: close it when a link in it is chosen, on Escape
    // (focus goes back to its button) and on a press anywhere outside it.
    const menu = document.querySelector('.nav__menu');
    if (menu) {
        const summary = menu.querySelector('summary');
        menu.addEventListener('click', (e) => {
            if (e.target instanceof Element && e.target.closest('.nav__sheet a')) menu.open = false;
        });
        document.addEventListener('keydown', (e) => {
            if (e.key !== 'Escape' || !menu.open) return;
            const hadFocus = menu.contains(document.activeElement);
            menu.open = false;
            if (hadFocus) summary?.focus();
        });
        document.addEventListener(
            'pointerdown',
            (e) => {
                if (menu.open && !menu.contains(e.target)) menu.open = false;
            },
            { passive: true }
        );
    }

    // ─── marquee pause buttons ────────────────────────────────────────

    // Every auto-scrolling strip has a button (WCAG 2.2.2). Any of them
    // pauses or plays all the strips; the label stays "Pause …" and
    // aria-pressed says whether they're paused.
    const marqueeGroups = $$('[data-marquee-group]');
    const marqueeToggles = $$('[data-marquee-toggle]');
    function pauseMarquees(paused) {
        for (const g of marqueeGroups) g.classList.toggle('is-paused', paused);
        for (const b of marqueeToggles) b.setAttribute('aria-pressed', String(paused));
    }
    for (const b of marqueeToggles) {
        b.addEventListener('click', () => pauseMarquees(b.getAttribute('aria-pressed') !== 'true'));
    }

    // ─── theme + language switcher ────────────────────────────────────

    const demo = document.querySelector('[data-lm="demo"]');
    const COPY = {
        en: {
            title: 'Three easy ideas for a rainy Sunday:',
            a: 'Bake a loaf of banana bread',
            b: 'Start a 1,000-piece puzzle',
            c: 'Visit a museum nearby',
            ph: 'Ask me anything…',
        },
        ja: {
            title: '雨の日曜日におすすめの過ごし方を3つ：',
            a: 'バナナブレッドを焼く',
            b: '1000ピースのパズルに挑戦する',
            c: '近くの美術館に行く',
            ph: '何でも聞いてください…',
        },
        ar: {
            title: 'ثلاث أفكار سهلة ليوم أحد ممطر:',
            a: 'اخبز رغيفًا من خبز الموز',
            b: 'ابدأ أحجية من ١٠٠٠ قطعة',
            c: 'زر متحفًا قريبًا',
            ph: 'اسألني أي شيء…',
        },
        de: {
            title: 'Drei einfache Ideen für einen verregneten Sonntag:',
            a: 'Ein Bananenbrot backen',
            b: 'Ein 1000-Teile-Puzzle anfangen',
            c: 'Ein Museum in der Nähe besuchen',
            ph: 'Fragen Sie mich etwas …',
        },
    };

    function press(group, btn) {
        $$('button', group).forEach((b) => b.setAttribute('aria-pressed', String(b === btn)));
    }

    // Where the browser has view transitions, a change morphs the window
    // (::view-transition-group(theme-demo) in launcher.css). Elsewhere the
    // CSS fades the colours, and a language change fades the text out and
    // back in. Reduced motion: an instant swap.
    const morphs = !reduce && typeof document.startViewTransition === 'function';
    let swapTimer = 0;

    function change(update, fadeText) {
        if (morphs) {
            document.startViewTransition(update);
        } else if (fadeText && !reduce && demo) {
            demo.classList.add('is-swapping');
            clearTimeout(swapTimer);
            swapTimer = setTimeout(() => {
                update();
                demo.classList.remove('is-swapping');
            }, 180);
        } else {
            update();
        }
    }

    for (const btn of $$('[data-theme-pick]')) {
        btn.addEventListener('click', () => {
            const t = btn.dataset.themePick;
            press(btn.parentElement, btn);
            // Like the app: one change, every window.
            change(() => {
                for (const lm of $$('.lm')) {
                    if (t === 'dark') lm.removeAttribute('data-theme');
                    else lm.dataset.theme = t;
                }
            }, false);
        });
    }

    for (const btn of $$('[data-lang-pick]')) {
        btn.addEventListener('click', () => {
            const lang = btn.dataset.langPick;
            const strings = COPY[lang];
            if (!demo || !strings) return;
            press(btn.parentElement, btn);
            change(() => {
                for (const el of $$('[data-i18n]', demo)) el.textContent = strings[el.dataset.i18n];
                demo.setAttribute('lang', lang);
                if (lang === 'ar') demo.setAttribute('dir', 'rtl');
                else demo.removeAttribute('dir');
            }, true);
        });
    }

    // ─── copy the quarantine command ──────────────────────────────────

    // The button's name is its visible text ("Copy" plus a hidden "the
    // command"); the result is announced through a polite status region.
    const copyKey = isMac ? '⌘C' : 'Ctrl+C';
    for (const btn of $$('[data-copy]')) {
        const box = btn.parentElement;
        const src = box.querySelector('[data-copy-src]');
        const label = btn.querySelector('[data-copy-label]');
        const status = box.querySelector('[data-copy-status]');
        let resetTimer = 0;
        let sayTimer = 0;
        btn.addEventListener('click', async () => {
            if (!src) return;
            const text = src.textContent.trim();
            let ok = false;
            try {
                await navigator.clipboard.writeText(text);
                ok = true;
            } catch {
                const range = document.createRange();
                range.selectNodeContents(src);
                const sel = window.getSelection();
                sel.removeAllRanges();
                sel.addRange(range);
                try {
                    ok = document.execCommand('copy');
                } catch {
                    ok = false;
                }
            }
            if (label) label.textContent = ok ? 'Copied' : `Press ${copyKey}`;
            btn.classList.toggle('is-copied', ok);
            if (status) {
                // Empty it first so a second click is announced again.
                status.textContent = '';
                clearTimeout(sayTimer);
                sayTimer = setTimeout(() => {
                    status.textContent = ok
                        ? 'Copied the command.'
                        : `Couldn't copy. The command is selected; press ${copyKey} to copy it.`;
                }, 60);
            }
            clearTimeout(resetTimer);
            resetTimer = setTimeout(() => {
                if (label) label.textContent = 'Copy';
                btn.classList.remove('is-copied');
                if (status) status.textContent = '';
            }, 2200);
        });
    }

    // ─── latest Nightly (optional, degrades silently) ─────────────────

    const meta = document.querySelector('[data-release-meta]');
    const dl = document.querySelector('#download');
    let fetched = false;

    async function loadRelease() {
        if (fetched || !meta || !window.fetch) return;
        fetched = true;
        const ctl = new AbortController();
        const timer = setTimeout(() => ctl.abort(), 6000);
        try {
            const res = await fetch(
                'https://api.github.com/repos/nachmore/Kage/releases/tags/dev-latest',
                {
                    headers: { Accept: 'application/vnd.github+json' },
                    credentials: 'omit',
                    referrerPolicy: 'no-referrer',
                    signal: ctl.signal,
                }
            );
            if (!res.ok) return;
            const rel = await res.json();
            const when = rel.published_at ? new Date(rel.published_at) : null;
            if (!when || Number.isNaN(when.getTime())) return;
            const date = new Intl.DateTimeFormat('en', { dateStyle: 'medium' }).format(when);
            const version = /\(([\d.]+)/.exec(rel.name || '')?.[1];
            meta.textContent = `Latest Nightly: ${date}${version ? ` (${version})` : ''}.`;
            const mb = (n) => `· ${(n / 1e6).toFixed(1)} MB`;
            for (const a of rel.assets || []) {
                const kind = /_x64-setup\.exe$/.test(a.name)
                    ? 'exe'
                    : /_aarch64\.dmg$/.test(a.name)
                      ? 'dmg'
                      : '';
                const slot = kind && document.querySelector(`[data-asset-size="${kind}"]`);
                if (slot && a.size) slot.textContent = mb(a.size);
            }
        } catch {
            // Offline, rate-limited or blocked: the static copy already says enough.
        } finally {
            clearTimeout(timer);
        }
    }

    if (dl && 'IntersectionObserver' in window) {
        const io = new IntersectionObserver(
            (entries) => {
                if (entries.some((e) => e.isIntersecting)) {
                    io.disconnect();
                    loadRelease();
                }
            },
            { rootMargin: '900px 0px' }
        );
        io.observe(dl);
    }
})();
