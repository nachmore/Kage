/**
 * Link Preview Message Formatter
 *
 * Finds URLs in rendered messages and appends rich preview cards
 * with title, description, and favicon fetched via the backend.
 */
export default class LinkPreviewFormatter {
    initialize(context) {
        this.config = context.config;
        this.invoke = context.invoke;
    }

    onConfigUpdate(config) {
        this.config = config;
    }

    format(container, context) {
        if (!this.config?.enabled) return;
        if (context.streaming) return;

        const maxPreviews = this.config?.max_previews || 5;
        const seen = new Set();
        const links = container.querySelectorAll('a[href]');
        let count = container.querySelectorAll('.link-preview-card').length;

        for (const link of links) {
            if (count >= maxPreviews) break;

            const href = link.getAttribute('href');
            if (!href || !href.startsWith('http')) continue;
            if (link.closest('.tool-sources, .source-chip, .source-bubble, .code-block-wrapper, .link-preview-card')) continue;
            if (seen.has(href)) continue;
            seen.add(href);
            if (container.querySelector(`.link-preview-card[data-url="${CSS.escape(href)}"]`)) continue;

            try {
                const url = new URL(href);
                const card = this._createPlaceholderCard(url, href);
                const insertAfter = link.closest('p, li, div');
                if (insertAfter && insertAfter.parentElement) {
                    insertAfter.parentElement.insertBefore(card, insertAfter.nextSibling);
                    count++;
                    this._fetchAndEnrich(card, href);
                }
            } catch { /* invalid URL */ }
        }
    }

    _createPlaceholderCard(url, href) {
        const card = document.createElement('a');
        card.className = 'link-preview-card';
        card.href = href;
        card.dataset.url = href;
        card.title = href;

        const domain = url.hostname.replace(/^www\./, '');
        const hue = this._hashToHue(domain);

        card.innerHTML = `
            <span class="link-preview-icon" style="background:hsl(${hue},55%,45%)">${this._esc(domain.charAt(0).toUpperCase())}</span>
            <span class="link-preview-info">
                <span class="link-preview-domain">${this._esc(domain)}</span>
                <span class="link-preview-title link-preview-loading">Loading...</span>
            </span>
            <svg class="link-preview-arrow" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/></svg>
        `;
        return card;
    }

    async _fetchAndEnrich(card, href) {
        try {
            const meta = await this.invoke('fetch_link_metadata', { url: href });
            if (!card.isConnected) return; // card was removed from DOM

            const domain = card.querySelector('.link-preview-domain')?.textContent || '';
            const titleEl = card.querySelector('.link-preview-title');
            const infoEl = card.querySelector('.link-preview-info');
            const iconEl = card.querySelector('.link-preview-icon');

            // Update title
            if (titleEl) {
                titleEl.classList.remove('link-preview-loading');
                titleEl.textContent = meta.title || domain;
            }

            // Add description
            if (meta.description && infoEl) {
                const descEl = document.createElement('span');
                descEl.className = 'link-preview-desc';
                descEl.textContent = meta.description.length > 120
                    ? meta.description.substring(0, 117) + '...'
                    : meta.description;
                infoEl.appendChild(descEl);
            }

            // Update favicon
            if (meta.favicon && iconEl) {
                const img = document.createElement('img');
                img.className = 'link-preview-favicon';
                img.src = meta.favicon;
                img.width = 24;
                img.height = 24;
                img.alt = '';
                img.onerror = () => { img.remove(); }; // keep the letter icon on failure
                iconEl.parentElement.insertBefore(img, iconEl);
                img.onload = () => { iconEl.style.display = 'none'; };
            }
        } catch {
            // Fetch failed — just remove the loading state
            const titleEl = card.querySelector('.link-preview-title');
            if (titleEl) {
                titleEl.classList.remove('link-preview-loading');
                titleEl.textContent = '';
            }
        }
    }

    _hashToHue(str) {
        let hash = 0;
        for (let i = 0; i < str.length; i++) {
            hash = str.charCodeAt(i) + ((hash << 5) - hash);
        }
        return Math.abs(hash) % 360;
    }

    _esc(s) {
        const d = document.createElement('div');
        d.textContent = s;
        return d.innerHTML;
    }

    destroy() {}
}
