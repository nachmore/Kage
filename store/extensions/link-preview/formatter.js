/**
 * Link Preview message formatter (sandboxed).
 *
 * Receives the rendered message HTML from the host, finds URL anchor
 * tags, fetches their metadata via fetch_link_metadata (guarded by the
 * `shell` capability), and inserts a preview card next to each.
 *
 * We stay inside the DOMParser-owned document — no access to the host
 * DOM — and return the resulting HTML string for the host to sanitize
 * and inject.
 */

const MAX_URL_LENGTH = 2048;

export default class LinkPreviewFormatter {
    initialize(context) {
        this.config = context.config || {};
        this.invoke = context.invoke;
        // Keep an in-memory metadata cache so repeated re-formats during
        // streaming don't re-fetch the same URL.
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
        // Skip streaming passes — URLs in the middle of being written
        // aren't meaningful yet, and the async fetches would pile up.
        if (context?.streaming) return null;

        const maxPreviews = Number(this.config?.max_previews) || 5;

        // Use DOMParser — creates an inert document, no scripts run.
        const doc = new DOMParser().parseFromString(
            `<!doctype html><body>${html}</body>`,
            'text/html',
        );
        const body = doc.body;
        if (!body) return null;

        // Don't touch links that already have a preview card immediately
        // after them (e.g. previously-formatted content).
        let hrefs = [];
        const seen = new Set();
        const anchors = body.querySelectorAll('a[href]');
        for (const a of anchors) {
            const href = a.getAttribute('href') || '';
            if (!/^https?:/i.test(href)) continue;
            if (href.length > MAX_URL_LENGTH) continue;
            if (seen.has(href)) continue;
            // Skip links inside tool sources, code blocks, or existing
            // preview cards.
            if (a.closest('.tool-sources, .source-chip, .source-bubble, .code-block-wrapper, .link-preview-card')) continue;
            seen.add(href);
            hrefs.push({ href, anchor: a });
        }
        hrefs = hrefs.slice(0, maxPreviews);
        if (hrefs.length === 0) return null;

        // Fetch all metadata in parallel (with the per-URL cache).
        const metas = await Promise.all(hrefs.map(h => this._getMeta(h.href)));

        for (let i = 0; i < hrefs.length; i++) {
            const { href, anchor } = hrefs[i];
            const meta = metas[i];
            const card = this._buildCardHtml(href, meta, doc);
            // Insert after the anchor's closest block-ish parent.
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
            this._metaCache.set(url, null);
            return null;
        }
    }

    _buildCardHtml(href, meta, doc) {
        let domain = '';
        try { domain = new URL(href).hostname.replace(/^www\./, ''); } catch {}
        const title = meta?.title || domain || href;
        const description = meta?.description || '';
        const favicon = meta?.favicon || '';
        const hue = this._hashToHue(domain || href);

        // Build as an <a> element — the sanitizer validates href and will
        // drop anything unsafe.
        const card = doc.createElement('a');
        card.className = 'link-preview-card';
        card.setAttribute('href', href);
        card.setAttribute('title', href);

        const iconWrap = doc.createElement('span');
        iconWrap.className = 'link-preview-icon';
        iconWrap.setAttribute('style', `background: hsl(${hue}, 55%, 45%)`);
        // We prefer the favicon image when we have one; otherwise show
        // the domain initial.
        if (favicon && /^https?:/i.test(favicon)) {
            const img = doc.createElement('img');
            img.setAttribute('src', favicon);
            img.setAttribute('class', 'link-preview-favicon');
            img.setAttribute('width', '24');
            img.setAttribute('height', '24');
            img.setAttribute('alt', '');
            iconWrap.appendChild(img);
        } else {
            iconWrap.textContent = (domain.charAt(0) || '?').toUpperCase();
        }
        card.appendChild(iconWrap);

        const info = doc.createElement('span');
        info.className = 'link-preview-info';
        const dom = doc.createElement('span');
        dom.className = 'link-preview-domain';
        dom.textContent = domain || href;
        info.appendChild(dom);
        const t = doc.createElement('span');
        t.className = 'link-preview-title';
        t.textContent = title;
        info.appendChild(t);
        if (description) {
            const d = doc.createElement('span');
            d.className = 'link-preview-desc';
            d.textContent = description.length > 120
                ? description.substring(0, 117) + '…'
                : description;
            info.appendChild(d);
        }
        card.appendChild(info);
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
