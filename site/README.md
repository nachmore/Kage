# Kage marketing site

The source for Kage's GitHub Pages site (`https://nachmore.github.io/Kage/`). Plain HTML, CSS and JavaScript: no framework, no bundler, no runtime CDN, no trackers, no web fonts.

```
site/
  index.html          the page
  404.html            GitHub Pages' not-found page
  css/site.css        layout, sections, motion
  css/launcher.css    the live launcher mock and the hero cat
  js/launcher.js      the hero demo (types queries, plays out results)
  js/mascot.js        the cat: gaze, blinks, reactions, the footer wave
  js/main.js          reveals, parallax, tilt, theme/language demo, downloads
  assets/             site-only art (star fields)
  scripts/build.mjs   assembles _site/
  scripts/check.mjs   checks _site/
```

## Preview locally

You need Node.js 20 or later. `cwebp` (from libwebp) is optional; with it the build also writes smaller WebP screenshots.

```bash
node site/scripts/build.mjs --base /
node site/scripts/check.mjs _site --base /
python3 -m http.server 8000 --bind 127.0.0.1 --directory _site
```

Then open <http://127.0.0.1:8000/>. Keep the `--bind 127.0.0.1` so the preview isn't reachable from other machines, and stop the server (Ctrl+C) when you're done. `--base /` makes the 404 page's links work at the server root; Python's server shows its own 404 page, so open `/404.html` directly to look at ours.

The same steps are wired up as npm scripts at the repo root: `npm run site:build`, `npm run site:check` and `npm run site:lint` (Biome, using the repo's `biome.json`).

## How the build works

`build.mjs` copies `site/` into `_site/` and then copies in every asset the pages reference from elsewhere in the repo, so the site never keeps its own copy of the README screenshots or the mascot:

| Path in the page | Comes from |
|---|---|
| `assets/shots/*` | `docs/media/readme/*` |
| `assets/media/*` | `docs/media/*` |
| `assets/mascot/*` | `ui/assets/*` |
| `assets/icons/*` | `icons/*` |
| anything else under `assets/` | `site/assets/` |

A reference to a file that doesn't exist fails the build. It also fills in `{{SITE_URL}}` (Open Graph and canonical URLs), `{{BASE}}` (absolute links in `404.html`), and the build date, commit and Kage version shown in the footer. Screenshots get 900px and 1800px WebP versions inside a `<picture>` when `cwebp` is available; in the source HTML use `data-sizes` (not `sizes`) on those `<img>` tags and the build moves it onto the `<source>`.

`check.mjs` fails on malformed HTML (unbalanced tags, duplicate attributes or ids), a missing `<h1>`, `lang`, title, description or viewport, `<img>` without `alt` or `width`/`height`, broken internal links, `#anchors` or asset references, and any script, stylesheet, image or font loaded from another site at runtime.

## Deploying

`.github/workflows/site.yml` runs on pushes to `main` and on pull requests that touch `site/`, `docs/media/`, `ui/assets/`, `icons/` or the workflow itself, and can be started by hand. Pull requests get Biome, the build and the checks; `main` also uploads `_site/` and deploys it with `actions/deploy-pages`.

**One-time setup for the repo owner.** Turn on Pages with GitHub Actions as the source: Settings → Pages → Build and deployment → Source → **GitHub Actions**. Or with the GitHub CLI:

```bash
# Pages not enabled yet
gh api -X POST repos/nachmore/Kage/pages -f build_type=workflow

# Pages already enabled from a branch: switch it to Actions
gh api -X PUT repos/nachmore/Kage/pages -f build_type=workflow
```

Until that's done the deploy job fails; build and check still run. The `github-pages` environment that GitHub creates only lets `main` deploy.

## Editing notes

- **Claims.** Every feature the page mentions has to be in the [README](../README.md) or the [user guide](../docs/FEATURES.md). Update those first.
- **Motion.** Animate `transform` and `opacity` only. Reveals use the individual `translate`/`rotate`/`scale` properties so JS can own `transform` on the same element for parallax and tilt. Listeners are passive and only schedule a `requestAnimationFrame`.
- **Reduced motion.** With `prefers-reduced-motion`, parallax, tilt, the auto-playing demo, marquees and the cat's gaze and blinks are off. The demo shows a finished scene and the example chips still switch scenes.
- **No JavaScript.** Content must stay visible without JS: `.reveal` only hides things under `html.js`, the demo's markup is a finished scene, and controls that need JS carry `needs-js`.
- **The demo.** Scenes live in `SCENES` in `js/launcher.js`. The window's height is fixed (`--lm-max`) and its visible panel slides to fit the content, so a scene's final state has to fit in `--lm-max` at every width; check at 360px, 390px and 1440px.
- **Screenshots.** Re-take them with the README pipeline, not here; the next build picks them up.
