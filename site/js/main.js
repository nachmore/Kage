/* Kage site: page-level behaviour.
 *
 * - platform-aware labels (⌥ Option on a Mac, Alt elsewhere) and the
 *   recommended download card
 * - scroll reveals (IntersectionObserver), scroll progress, nav state
 * - multi-layer hero parallax (scroll + pointer), section parallax and
 *   3D tilt on screenshots, pointer spotlights on cards
 * - the live theme/language switcher, the copy button, and the latest
 *   Nightly's date from the GitHub API (silently skipped if it fails)
 *
 * One rAF loop does all scroll/pointer work; listeners are passive and only
 * schedule it. Everything written per frame is a transform.
 */
(() => {
    const doc = document.documentElement;
    const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    const fine = window.matchMedia('(pointer: fine)').matches;
    const os = doc.dataset.os;
    const $$ = (sel, el = document) => [...el.querySelectorAll(sel)];

    // ─── platform labels ──────────────────────────────────────────────

    if (os === 'mac') {
        $$('[data-mod-label]').forEach((el) => {
            el.textContent = '⌥ Option';
        });
        $$('[data-mod-label-kbd]').forEach((el) => {
            el.textContent = 'Option';
        });
        $$('[data-send-label]').forEach((el) => {
            el.textContent = '⌘+Enter';
        });
        $$('[data-inline-label]').forEach((el) => {
            el.textContent = '⌃⇧Space';
        });
    }
    const card = document.querySelector(`[data-platform="${os === 'mac' ? 'mac' : 'windows'}"]`);
    if (os === 'mac' || os === 'windows') {
        card?.classList.add('is-recommended');
        const label = document.querySelector('[data-dl-label] span');
        if (label) label.textContent = os === 'mac' ? 'Download for macOS' : 'Download for Windows';
    }

    // ─── reveals ──────────────────────────────────────────────────────

    const reveals = $$('.reveal');
    const counters = $$('[data-count]');
    if (reduce || !('IntersectionObserver' in window)) {
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

    // ─── scroll + pointer loop ────────────────────────────────────────

    const nav = document.querySelector('[data-nav]');
    const bar = document.querySelector('.progress span');
    const hero = document.querySelector('.hero');
    const layers = reduce
        ? []
        : $$('.sky [data-depth]').map((el) => ({
              el,
              depth: Number(el.dataset.depth) * (fine ? 1 : 0.6),
              px: fine ? Number(el.dataset.px || 0) : 0,
          }));
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

    for (const btn of $$('[data-theme-pick]')) {
        btn.addEventListener('click', () => {
            const t = btn.dataset.themePick;
            // Like the app: one change, every window.
            for (const lm of $$('.lm')) {
                if (t === 'dark') lm.removeAttribute('data-theme');
                else lm.dataset.theme = t;
            }
            press(btn.parentElement, btn);
        });
    }

    for (const btn of $$('[data-lang-pick]')) {
        btn.addEventListener('click', () => {
            const lang = btn.dataset.langPick;
            const strings = COPY[lang];
            if (!demo || !strings) return;
            for (const el of $$('[data-i18n]', demo)) el.textContent = strings[el.dataset.i18n];
            demo.setAttribute('lang', lang);
            if (lang === 'ar') demo.setAttribute('dir', 'rtl');
            else demo.removeAttribute('dir');
            press(btn.parentElement, btn);
        });
    }

    // ─── copy the quarantine command ──────────────────────────────────

    for (const btn of $$('[data-copy]')) {
        const src = btn.parentElement.querySelector('[data-copy-src]');
        const label = btn.querySelector('[data-copy-label]');
        let t = 0;
        btn.addEventListener('click', async () => {
            const text = src?.textContent.trim() || '';
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
            if (!label) return;
            label.textContent = ok ? 'Copied' : 'Press Ctrl+C';
            btn.classList.toggle('is-copied', ok);
            clearTimeout(t);
            t = setTimeout(() => {
                label.textContent = 'Copy';
                btn.classList.remove('is-copied');
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
