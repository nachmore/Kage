/**
 * Link Preview message formatter (sandboxed).
 *
 * Finds URL anchor tags in the rendered message HTML, fetches OG / Twitter
 * Card / favicon metadata via `fetch_link_metadata` (guarded by the
 * `shell` capability), and inserts a card after each. Cards render
 * three ways depending on what came back:
 *
 *   - **Hero card** — has an `og:image` we trust. Image goes top, with
 *     domain + title + description below. This is the shape users
 *     expect from Slack / Discord / Notion link unfurls.
 *   - **Compact card** — favicon-only fallback when no hero image is
 *     available. Single row, like the previous version.
 *   - **Fallback card** — metadata fetch failed or returned nothing.
 *     Renders with the parsed domain and a colour-derived initial so
 *     the user still gets a consistent visual; we never silently skip
 *     a card now.
 *
 * Sandbox boundary: we operate in a DOMParser-owned document, return
 * the resulting HTML, and let the host sanitiser approve every tag and
 * attribute. The card structure uses tags + attrs the sanitiser
 * already permits (`A`, `IMG`, `SPAN`, `class`, `style: background`,
 * `src` validated against http(s)).
 */

const MAX_URL_LENGTH = 2048;
const DESC_TRUNCATE_AT = 140;

export default class LinkPreviewFormatter {
    initialize(context) {
        this.config = context.config || {};
        this.invoke = context.invoke;
        // In-memory cache for the duration of this sandbox lifetime. The
        // disk cache lives in the host (`get_link_metadata_cached`) and
        // is shared across windows / restarts.
        this._metaCache = new Map();
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    /**
     * Host contract: return a replacement HTML string, or null to leave
     * the existing HTML unchanged.
     */
    async format(html, context) {
        if (!this.config?.enabled) return null;
        // Streaming chunks contain URLs mid-write; resolving against
        // them is wasteful and produces flicker. Wait for the final
        // pass.
        if (context?.streaming) return null;

        const maxPreviews = Number(this.config?.max_previews) || 5;
        const showImages = this.config?.show_images !== false;

        const doc = new DOMParser().parseFromString(
            `<!doctype html><body>${html}</body>`,
            'text/html'
        );
        const body = doc.body;
        if (!body) return null;

        // Collect candidate links once. Skip anchors that are clearly
        // structural (tool sources, code blocks, or already-rendered
        // cards from a previous format pass).
        const hrefs = [];
        const seen = new Set();
        const anchors = body.querySelectorAll('a[href]');
        for (const a of anchors) {
            const href = a.getAttribute('href') || '';
            if (!/^https?:/i.test(href)) continue;
            if (href.length > MAX_URL_LENGTH) continue;
            if (seen.has(href)) continue;
            if (
                a.closest(
                    '.tool-sources, .source-chip, .source-bubble, .code-block-wrapper, .link-preview-card'
                )
            )
                continue;
            seen.add(href);
            hrefs.push({ href, anchor: a });
            if (hrefs.length >= maxPreviews) break;
        }
        if (hrefs.length === 0) return null;

        // allSettled, not all — one slow / failing URL shouldn't block
        // sibling cards. Failed fetches still get a fallback card.
        const settled = await Promise.allSettled(hrefs.map((h) => this._getMeta(h.href)));

        for (let i = 0; i < hrefs.length; i++) {
            const { href, anchor } = hrefs[i];
            const meta = settled[i].status === 'fulfilled' ? settled[i].value : null;
            const card = this._buildCardHtml(href, meta, doc, { showImages });
            const insertAfter = anchor.closest('p, li, div') || anchor;
            if (insertAfter.parentNode) {
                insertAfter.parentNode.insertBefore(card, insertAfter.nextSibling);
            }
        }

        return body.innerHTML;
    }

    destroy() {
        this._metaCache.clear();
    }

    // --- Internals ---

    async _getMeta(url) {
        if (this._metaCache.has(url)) return this._metaCache.get(url);
        try {
            const meta = await this.invoke('fetch_link_metadata', { url });
            this._metaCache.set(url, meta || null);
            return meta || null;
        } catch {
            // Network / sandbox / permission failure — cache the null
            // so we don't retry on every re-format pass within this
            // session. The disk cache (host-side) will pick the URL up
            // again on a fresh session.
            this._metaCache.set(url, null);
            return null;
        }
    }

    /**
     * Build a card. `meta` may be null (fallback shape) or partial
     * (compact shape if no hero image). All three shapes share the
     * same outer `<a class="link-preview-card">` so the host CSS can
     * style + size them consistently.
     */
    _buildCardHtml(href, meta, doc, opts = {}) {
        let domain = '';
        try {
            domain = new URL(href).hostname.replace(/^www\./, '');
        } catch {}
        const title = meta?.title || domain || href;
        const description = meta?.description || '';
        const favicon = meta?.favicon || '';
        const image = (opts.showImages !== false && meta?.image) || '';
        const hue = this._hashToHue(domain || href);
        const isHero = !!image;

        const card = doc.createElement('a');
        card.className = isHero
            ? 'link-preview-card link-preview-card-hero'
            : meta
              ? 'link-preview-card'
              : 'link-preview-card link-preview-card-fallback';
        card.setAttribute('href', href);
        card.setAttribute('title', href);
        // Mark the card up as a link to a preview so screen readers
        // describe what the user is about to follow. The default link
        // text would just be the URL.
        card.setAttribute('aria-label', `Link preview: ${title}`);

        if (isHero) {
            const heroWrap = doc.createElement('span');
            heroWrap.className = 'link-preview-hero';
            const heroImg = doc.createElement('img');
            heroImg.setAttribute('src', image);
            heroImg.setAttribute('class', 'link-preview-hero-img');
            heroImg.setAttribute('alt', '');
            heroImg.setAttribute('loading', 'lazy');
            heroWrap.appendChild(heroImg);
            card.appendChild(heroWrap);
        }

        // Header row: favicon (or coloured initial) + domain on the right.
        const header = doc.createElement('span');
        header.className = 'link-preview-header';

        const iconWrap = doc.createElement('span');
        iconWrap.className = 'link-preview-icon';
        if (favicon && /^https?:/i.test(favicon)) {
            const img = doc.createElement('img');
            img.setAttribute('src', favicon);
            img.setAttribute('class', 'link-preview-favicon');
            img.setAttribute('width', '20');
            img.setAttribute('height', '20');
            img.setAttribute('alt', '');
            img.setAttribute('loading', 'lazy');
            iconWrap.appendChild(img);
        } else {
            iconWrap.setAttribute('style', `background: hsl(${hue}, 55%, 45%)`);
            iconWrap.textContent = (domain.charAt(0) || '?').toUpperCase();
        }
        header.appendChild(iconWrap);

        const dom = doc.createElement('span');
        dom.className = 'link-preview-domain';
        dom.textContent = domain || href;
        header.appendChild(dom);
        card.appendChild(header);

        const t = doc.createElement('span');
        t.className = 'link-preview-title';
        t.textContent = title;
        card.appendChild(t);

        if (description) {
            const d = doc.createElement('span');
            d.className = 'link-preview-desc';
            d.textContent =
                description.length > DESC_TRUNCATE_AT
                    ? description.substring(0, DESC_TRUNCATE_AT - 1) + '…'
                    : description;
            card.appendChild(d);
        }

        return card;
    }

    _hashToHue(str) {
        let hash = 0;
        for (let i = 0; i < str.length; i++) {
            hash = str.charCodeAt(i) + ((hash << 5) - hash);
        }
        return Math.abs(hash) % 360;
    }
}
