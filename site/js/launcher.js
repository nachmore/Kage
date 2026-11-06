/* Kage site: the live launcher demo in the hero.
 *
 * A small timeline engine types queries into an HTML/CSS mock of the
 * floating launcher and plays out real behaviours, in the order of the
 * example chips: an app result, a streamed agent answer with a tool chip, a
 * unit conversion, a shell command that asks permission first, a folder and
 * a timer. The agent's answer starts streaming about 8 s after load.
 *
 * - The window is summoned once at the start (the hotkey beat) and is
 *   dismissed and summoned again only after the shell command, so there is
 *   one hide-and-summon beat per loop. Other scenes hand over by clearing
 *   the query, the way typing over a selection does.
 * - Every wait goes through `wait()`, which holds while the demo is blocked
 *   (paused by the button, the hero scrolled out of view, or the tab hidden)
 *   and throws when a newer run has started.
 * - The window resizes with transforms only: see fit().
 * - `.demo-playing` on the hero is set while the loop is running, so the
 *   hotkey's "press me" ring only pulses before the first summon and while
 *   paused.
 * - The toggle keeps its "Pause demo" label; aria-pressed says it's paused.
 * - prefers-reduced-motion: nothing auto-plays; the demo shows the agent
 *   scene's final state and the chips show each scene's final state
 *   instantly. Pressing the toggle opts in to the animation.
 * - Broadcasts `kage:demo` events so the hero cat can react.
 */
(() => {
    const root = document.querySelector('[data-lm="hero"]');
    if (!root) return;

    const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)');
    const isMac = document.documentElement.dataset.os === 'mac';
    const MOD = isMac ? '⌘' : 'Ctrl';
    const ENTER = '⏎';

    const $ = (sel) => root.querySelector(sel);
    const hero = root.closest('.hero');
    const content = $('[data-lm-content]');
    const answer = $('[data-lm-answer]');
    const results = $('[data-lm-results]');
    const query = $('[data-lm-query]');
    const keyToast = $('[data-lm-key]');
    const timeEl = $('[data-lm-time]');
    const ring = $('[data-lm-ring]');
    const pct = $('[data-lm-pct]');
    const panel = $('.lm__panel');
    const strip = $('.lm__strip');
    const shadow = $('.lm__shadow');
    const mascot = $('.lm__mascot');
    const poses = root.querySelectorAll('[data-pose]');
    const hotkeys = document.querySelectorAll('[data-hotkey] .key');
    const chips = document.querySelectorAll('[data-scene]');
    const toggle = document.querySelector('[data-demo-toggle]');

    // ─── window sizing (transform-only) ───────────────────────────────

    let maxH = 400;

    function readMax() {
        const v = parseFloat(getComputedStyle(root).getPropertyValue('--lm-max'));
        maxH = Number.isFinite(v) && v > 0 ? v : 400;
    }

    function fit() {
        const h = Math.min(Math.ceil(content.offsetHeight), maxH);
        const dy = h - maxH;
        panel.style.transform = `translate3d(0, ${dy}px, 0)`;
        strip.style.transform = `translate3d(0, ${-dy}px, 0)`;
        shadow.style.transform = `scaleY(${(h / maxH).toFixed(4)})`;
        mascot.style.transform = `translate3d(0, ${Math.round(h / 2 - 30)}px, 0)`;
    }

    function snapFit() {
        root.classList.add('is-instant');
        fit();
        // Force the instant styles to apply before transitions come back.
        void root.offsetWidth;
        root.classList.remove('is-instant');
    }

    // ─── timeline control ─────────────────────────────────────────────

    const CANCEL = Symbol('cancel');
    let runId = 0;
    let userPaused = false;
    // Two separate reasons to hold the timeline besides the button: the
    // observer owns heroVisible and visibilitychange owns tabHidden, so
    // neither can overwrite the other.
    let heroVisible = true;
    let tabHidden = document.hidden;
    let waiters = [];

    const blocked = () => userPaused || !heroVisible || tabHidden;

    function gate() {
        if (!blocked()) return Promise.resolve();
        return new Promise((resolve) => waiters.push(resolve));
    }

    // Wake every waiter. Without `force` this only happens once nothing is
    // blocking; makeCtx forces it so a superseded run wakes up and cancels.
    function release(force) {
        if (blocked() && !force) return;
        const list = waiters;
        waiters = [];
        list.forEach((fn) => fn());
    }

    function makeCtx(fast) {
        clearTimeout(startTimer);
        const id = ++runId;
        release(true);
        const check = () => {
            if (id !== runId) throw CANCEL;
        };
        return {
            fast,
            async wait(ms) {
                if (fast) return;
                await gate();
                check();
                await new Promise((resolve) => setTimeout(resolve, ms));
                await gate();
                check();
            },
            check,
        };
    }

    const rand = (a, b) => a + Math.random() * (b - a);

    function emit(state) {
        document.dispatchEvent(new CustomEvent('kage:demo', { detail: { state } }));
    }

    // ─── primitives ───────────────────────────────────────────────────

    function pose(name) {
        poses.forEach((img) => img.classList.toggle('is-on', img.dataset.pose === name));
    }

    function setModes(...modes) {
        for (const m of ['is-chat', 'is-results', 'is-timer', 'has-badge', 'is-streaming']) {
            root.classList.toggle(m, modes.includes(m));
        }
    }

    // Back to an empty window. Also clears anything a cancelled run left
    // half-done: a held hotkey, a key toast, a fade.
    function resetWindow() {
        setModes();
        answer.innerHTML = '';
        results.innerHTML = '';
        query.textContent = '';
        query.classList.remove('is-selected');
        root.classList.remove('has-query', 'is-typing', 'is-out');
        keyToast.classList.remove('is-on', 'is-down');
        hotkeys.forEach((k) => k.classList.remove('is-pressed'));
        pose('idle');
    }

    async function type(ctx, text) {
        root.classList.add('has-query');
        if (ctx.fast) {
            query.textContent = text;
            return;
        }
        root.classList.add('is-typing');
        pose('typing');
        emit('typing');
        for (const ch of text) {
            query.textContent += ch;
            let d = rand(30, 72);
            if (ch === ' ') d += rand(15, 45);
            await ctx.wait(d);
        }
        root.classList.remove('is-typing');
    }

    async function clearQuery(ctx) {
        if (!query.textContent) return;
        query.classList.add('is-selected');
        await ctx.wait(280);
        query.textContent = '';
        query.classList.remove('is-selected');
        root.classList.remove('has-query');
    }

    async function press(ctx, keys) {
        if (ctx.fast) return;
        keyToast.innerHTML = keys.map((k) => `<kbd>${k}</kbd>`).join('');
        keyToast.classList.add('is-on');
        await ctx.wait(220);
        keyToast.classList.add('is-down');
        await ctx.wait(140);
        keyToast.classList.remove('is-down');
        await ctx.wait(220);
        keyToast.classList.remove('is-on');
    }

    async function swap(ctx, fn) {
        if (ctx.fast) {
            fn();
            return;
        }
        root.classList.add('is-out');
        await ctx.wait(200);
        fn();
        root.classList.remove('is-out');
    }

    // Hand over to the next scene without dismissing the window: select the
    // query, clear it and fade the old results out.
    async function clearOut(ctx) {
        await clearQuery(ctx);
        await swap(ctx, () => {
            results.innerHTML = '';
            setModes();
            pose('idle');
        });
        await ctx.wait(160);
    }

    async function hideWindow(ctx) {
        if (ctx.fast) return;
        root.classList.add('is-hidden');
        emit('hide');
        await ctx.wait(260);
        resetWindow();
        await ctx.wait(480);
    }

    // The summon beat: the hotkey goes down and the window appears.
    async function ensureShown(ctx) {
        if (!root.classList.contains('is-hidden')) return;
        if (ctx.fast) {
            reveal();
            return;
        }
        resetWindow();
        hotkeys.forEach((k) => k.classList.add('is-pressed'));
        await ctx.wait(140);
        root.classList.remove('is-hidden');
        emit('summon');
        await ctx.wait(200);
        hotkeys.forEach((k) => k.classList.remove('is-pressed'));
        await ctx.wait(380);
    }

    // Un-hide outside the summon beat (a chip click, a static scene).
    function reveal() {
        if (!root.classList.contains('is-hidden')) return;
        root.classList.remove('is-hidden');
        emit('summon');
    }

    function setContext(p) {
        ring.style.setProperty('--p', p);
        pct.textContent = `${p}%`;
    }

    // ─── content builders ─────────────────────────────────────────────

    const ICONS = {
        globe: '<svg class="ico" aria-hidden="true"><use href="#i-globe"/></svg>',
        term: '<svg class="ico" aria-hidden="true"><use href="#i-terminal"/></svg>',
        folder: '<svg class="ico" aria-hidden="true"><use href="#i-folder"/></svg>',
        check: '<svg class="ico" aria-hidden="true"><use href="#i-check"/></svg>',
    };

    const sendHint = () => `<div class="lm-hint"><kbd>${MOD}${ENTER}</kbd> to send to agent</div>`;

    function result({ icon, title, sub, hint = true }) {
        return (
            `<div class="lm-result">${icon}<span class="lm-result__text">` +
            `<span class="lm-result__title">${title}</span>` +
            `<span class="lm-result__sub" data-sub>${sub}</span></span></div>` +
            (hint ? sendHint() : '')
        );
    }

    const calcIcon = '<span class="lm-result__icon lm-result__icon--calc"></span>';
    const mathIcon = '<span class="lm-result__icon lm-result__icon--tile">🧮</span>';
    const folderIcon = `<span class="lm-result__icon lm-result__icon--tile">${ICONS.folder}</span>`;

    function chip(icon, label, extra = '') {
        const el = document.createElement('span');
        el.className = 'lm-chip';
        el.innerHTML = `${icon}${label}${extra}<span class="lm-chip__chev">›</span>`;
        return el;
    }

    /* Stream rich text into `host`, a few words per tick. Blocks are
     * [tag, pieces] ('h' is a bold sub-heading line); a piece is a string or [wrapTag, text]. Consecutive
     * 'li' blocks share a <ul>. */
    async function stream(ctx, host, blocks) {
        let list = null;
        for (const [tag, pieces] of blocks) {
            let block;
            if (tag === 'li') {
                if (!list) {
                    list = document.createElement('ul');
                    host.appendChild(list);
                }
                block = document.createElement('li');
                list.appendChild(block);
            } else {
                list = null;
                block = document.createElement(tag === 'h' ? 'p' : tag);
                if (tag === 'h') block.className = 'lm-h';
                host.appendChild(block);
            }
            for (const piece of pieces) {
                const [wrap, text] = typeof piece === 'string' ? [null, piece] : piece;
                let target = block;
                if (wrap) {
                    target = document.createElement(wrap);
                    block.appendChild(target);
                }
                if (ctx.fast) {
                    target.append(text);
                    continue;
                }
                const words = text.match(/\S+\s*|\s+/g) || [];
                for (let i = 0; i < words.length; ) {
                    const n = 1 + Math.floor(Math.random() * 3);
                    const span = document.createElement('span');
                    span.className = 'w';
                    span.textContent = words.slice(i, i + n).join('');
                    target.appendChild(span);
                    i += n;
                    await ctx.wait(rand(34, 70));
                }
            }
            await ctx.wait(rand(60, 140));
        }
    }

    function startChat(echoText) {
        setModes('is-chat', 'is-streaming');
        query.textContent = '';
        root.classList.remove('has-query');
        answer.innerHTML = `<div class="lm-echo"></div><div class="lm-answer"></div>`;
        answer.firstChild.textContent = echoText;
        const dots = document.createElement('div');
        dots.className = 'lm-dots';
        dots.innerHTML = '<span></span><span></span><span></span>';
        answer.lastChild.appendChild(dots);
        return { body: answer.lastChild, dots };
    }

    function chipRow() {
        let row = answer.querySelector('.lm-chips');
        if (!row) {
            row = document.createElement('div');
            row.className = 'lm-chips';
            answer.appendChild(row);
        }
        return row;
    }

    // ─── scenes ───────────────────────────────────────────────────────

    const LISBON = [
        ['p', ['An easy-going ', ['strong', 'weekend in Lisbon'], ':']],
        ['h', ['Saturday · Alfama & Baixa']],
        ['li', [['strong', 'Tram 28'], ' up to São Jorge Castle']],
        ['li', ['Sunset at a ', ['em', 'miradouro'], ', then fado']],
        ['h', ['Sunday · Belém']],
        ['li', ['Pastéis de Belém (go early)']],
        ['li', ['A riverside walk to LX Factory']],
    ];

    const SHELL_INTRO = [['p', ['Let me check the sizes.']]];
    const SHELL_ANSWER = [['p', ['The talk recording takes ', ['strong', '4.2 GB'], '.']]];
    const DU_OUT = [
        ['4.2G', '~/Downloads/talk-recording.mov'],
        ['1.3G', '~/Downloads/photos-lisbon'],
        ['610M', '~/Downloads/old-installers'],
    ];

    // Index n plays for the chip with data-scene="n" (same order as the
    // chips in index.html).
    const SCENES = [
        // 0 · calc → Calculator
        async (ctx) => {
            await ensureShown(ctx);
            await type(ctx, 'calc');
            await ctx.wait(140);
            results.innerHTML = result({ icon: calcIcon, title: 'Calculator', sub: 'Application' });
            setModes('is-results');
            pose('happy');
            if (ctx.fast) return;
            await ctx.wait(1200);
            await clearOut(ctx);
        },
        // 1 · Ask the agent → streamed answer with a fetch chip
        async (ctx) => {
            const q = 'Plan an easy weekend in Lisbon';
            await ensureShown(ctx);
            await type(ctx, q);
            await ctx.wait(200);
            await press(ctx, [MOD, ENTER]);
            let chat;
            await swap(ctx, () => {
                chat = startChat(q);
            });
            setContext(12);
            pose('thinking');
            emit('thinking');
            await ctx.wait(600);
            const fetchChip = chip(ICONS.globe, 'fetch');
            chipRow().appendChild(fetchChip);
            await ctx.wait(650);
            chat.dots.remove();
            emit('streaming');
            await stream(ctx, chat.body, LISBON);
            root.classList.remove('is-streaming');
            pose('happy');
            emit('done');
            if (ctx.fast) return;
            await ctx.wait(3200);
            await press(ctx, [MOD, 'L']);
            await swap(ctx, resetWindow);
            await ctx.wait(300);
        },
        // 2 · 12 km to mi → = 7.46 mi
        async (ctx) => {
            await ensureShown(ctx);
            await type(ctx, '12 km to mi');
            await ctx.wait(140);
            results.innerHTML = result({
                icon: mathIcon,
                title: '= 7.46 mi',
                sub: 'Press Enter to copy result',
            });
            setModes('is-results');
            pose('happy');
            if (ctx.fast) return;
            await ctx.wait(1300);
            await press(ctx, [ENTER]);
            const sub = results.querySelector('[data-sub]');
            if (sub) sub.textContent = 'Copied to the clipboard';
            emit('done');
            await ctx.wait(1100);
            await clearOut(ctx);
        },
        // 3 · Run a command → permission, shell block, output; then Esc
        // dismisses the window (the loop's one hide-and-summon beat).
        async (ctx) => {
            const q = 'Which files are biggest in my Downloads folder?';
            await ensureShown(ctx);
            await type(ctx, q);
            await ctx.wait(200);
            await press(ctx, [MOD, ENTER]);
            let chat;
            await swap(ctx, () => {
                chat = startChat(q);
            });
            setContext(18);
            pose('thinking');
            emit('thinking');
            await ctx.wait(650);
            chat.dots.remove();
            await stream(ctx, chat.body, SHELL_INTRO);

            const block = document.createElement('div');
            block.className = 'lm-shell';
            block.innerHTML =
                '<div class="lm-shell__cmd"><b>$</b><span>du -sh ~/Downloads/* | sort -rh | head -3</span>' +
                '<span class="lm-shell__state"><span class="lm-spin"></span></span></div>';
            chat.body.appendChild(block);

            if (!ctx.fast) {
                await ctx.wait(450);
                const perm = document.createElement('div');
                perm.className = 'lm-perm';
                perm.innerHTML =
                    '<div class="lm-perm__title">Tool Permission Required</div>' +
                    '<div class="lm-perm__what">Shell · list the biggest items in Downloads</div>' +
                    '<div class="lm-perm__btns"><span class="lm-perm__deny">Deny ▾</span>' +
                    '<span class="lm-perm__allow">Allow ▾</span></div>';
                chat.body.appendChild(perm);
                pose('idle');
                emit('ask');
                await ctx.wait(1500);
                const allow = perm.querySelector('.lm-perm__allow');
                allow.classList.add('is-down');
                await ctx.wait(170);
                allow.classList.remove('is-down');
                await ctx.wait(140);
                perm.remove();
                pose('thinking');
                emit('thinking');
                await ctx.wait(700);
            }

            const out = document.createElement('div');
            out.className = 'lm-shell__out';
            block.appendChild(out);
            for (const [size, path] of DU_OUT) {
                const line = document.createElement('div');
                line.innerHTML = '<i></i><span></span>';
                line.firstChild.textContent = size;
                line.lastChild.textContent = path;
                out.appendChild(line);
                await ctx.wait(140);
            }
            block.querySelector('.lm-shell__state').innerHTML = ICONS.check;
            const exit = document.createElement('div');
            exit.className = 'lm-shell__exit';
            exit.textContent = 'Exit code 0';
            block.appendChild(exit);
            chipRow().appendChild(chip(ICONS.term, 'shell'));
            await ctx.wait(450);
            emit('streaming');
            const tail = document.createElement('div');
            chat.body.appendChild(tail);
            await stream(ctx, tail, SHELL_ANSWER);
            // keep the chip row last
            answer.appendChild(chipRow());
            root.classList.remove('is-streaming');
            pose('happy');
            emit('done');
            if (ctx.fast) return;
            await ctx.wait(3200);
            await press(ctx, ['Esc']);
            await hideWindow(ctx);
        },
        // 4 · down → Downloads (summoned again after the Esc above)
        async (ctx) => {
            await ensureShown(ctx);
            await type(ctx, 'down');
            await ctx.wait(140);
            results.innerHTML = result({
                icon: folderIcon,
                title: 'Open Folder',
                sub: isMac ? '~/Downloads' : 'C:\\Users\\you\\Downloads',
            });
            setModes('is-results');
            pose('happy');
            if (ctx.fast) return;
            await ctx.wait(1400);
            await clearOut(ctx);
        },
        // 5 · timer 25m → a countdown, two ticks, then on to calc
        async (ctx) => {
            await ensureShown(ctx);
            await type(ctx, 'timer 25m');
            await ctx.wait(180);
            await press(ctx, [ENTER]);
            await swap(ctx, () => {
                query.textContent = '';
                root.classList.remove('has-query');
                timeEl.textContent = ctx.fast ? '24:58' : '25:00';
                setModes('is-timer', 'has-badge');
                pose('idle');
            });
            if (ctx.fast) return;
            emit('done');
            for (const t of ['24:59', '24:58']) {
                await ctx.wait(1000);
                timeEl.textContent = t;
            }
            await ctx.wait(450);
            await swap(ctx, () => setModes());
            await ctx.wait(250);
        },
    ];

    // ─── driving the loop ─────────────────────────────────────────────

    // With reduced motion, the scene the page shows (and the no-JS markup).
    const STATIC_SCENE = 1;
    // Lets the hero's CSS entrance settle before the first summon.
    const START_DELAY = 700;

    let current = 0;
    let looping = false;
    let startTimer = 0;

    // The hotkey's pulse ring is for while the demo isn't pressing it.
    function syncPlaying() {
        hero?.classList.toggle('demo-playing', looping && !userPaused);
    }

    function markChip(i) {
        chips.forEach((c, n) => {
            const on = n === i;
            c.classList.toggle('is-active', on);
            if (on) c.setAttribute('aria-current', 'true');
            else c.removeAttribute('aria-current');
        });
    }

    async function loop(start) {
        const ctx = makeCtx(false);
        looping = true;
        syncPlaying();
        let i = start;
        try {
            for (;;) {
                current = i;
                markChip(i);
                await SCENES[i](ctx);
                i = (i + 1) % SCENES.length;
            }
        } catch (err) {
            if (err !== CANCEL) throw err;
        }
    }

    async function showFinal(i) {
        const ctx = makeCtx(true);
        looping = false;
        syncPlaying();
        current = i;
        markChip(i);
        reveal();
        resetWindow();
        try {
            await SCENES[i](ctx);
        } catch (err) {
            if (err !== CANCEL) throw err;
        }
        snapFit();
    }

    // The label stays "Pause demo"; aria-pressed says whether it's paused.
    function setPaused(p) {
        userPaused = p;
        toggle?.setAttribute('aria-pressed', String(p));
        syncPlaying();
        if (!p) release();
    }

    function start() {
        root.classList.add('lm--measured');
        readMax();
        new ResizeObserver(fit).observe(content);
        window.addEventListener('resize', () => {
            readMax();
            fit();
        });

        const io = new IntersectionObserver(
            (entries) => {
                heroVisible = entries[entries.length - 1].isIntersecting;
                release();
            },
            { threshold: 0.05 }
        );
        io.observe(root);
        document.addEventListener('visibilitychange', () => {
            tabHidden = document.hidden;
            release();
        });

        chips.forEach((c, n) =>
            c.addEventListener('click', () => {
                // A click before the first summon, or mid-scene, takes over:
                // makeCtx cancels the pending start and the running scene.
                if (userPaused) showFinal(n);
                else {
                    reveal();
                    resetWindow();
                    loop(n);
                }
            })
        );

        toggle?.addEventListener('click', () => {
            const willPause = !userPaused;
            setPaused(willPause);
            if (willPause) {
                // Paused while the window is away (before the first summon,
                // or during the dismiss beat): show the scene's final state
                // rather than an empty hero. Play restarts that scene.
                if (root.classList.contains('is-hidden')) showFinal(current);
            } else if (!looping) {
                resetWindow();
                loop(current);
            }
        });

        if (reduceMotion.matches) {
            setPaused(true);
            showFinal(STATIC_SCENE);
            return;
        }
        // Start hidden: the first beat is the hotkey summoning the window.
        root.classList.add('is-hidden');
        resetWindow();
        snapFit();
        looping = true;
        startTimer = setTimeout(() => loop(0), START_DELAY);
    }

    start();
})();
