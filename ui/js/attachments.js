/**
 * Shared attachment handling for both floating and main chat UIs.
 * Supports image paste, file drag-drop, and attachment preview rendering.
 */

const SUPPORTED_IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
const MAX_IMAGE_SIZE = 10 * 1024 * 1024; // 10MB
const MAX_ATTACHMENTS = 4;

/**
 * Manages a list of pending attachments for a chat input.
 */
export class AttachmentManager {
    constructor() {
        /** @type {Array<{type: string, data?: string, mimeType?: string, uri?: string, name?: string, previewUrl?: string}>} */
        this.attachments = [];
        this.onChangeCallbacks = [];
    }

    onChange(cb) {
        this.onChangeCallbacks.push(cb);
    }

    _notify() {
        for (const cb of this.onChangeCallbacks) cb(this.attachments);
    }

    /** Add an image from a base64 data string */
    addImage(base64Data, mimeType, previewUrl) {
        if (this.attachments.length >= MAX_ATTACHMENTS) return false;
        this.attachments.push({
            type: 'image',
            data: base64Data,
            mimeType,
            previewUrl: previewUrl || `data:${mimeType};base64,${base64Data}`
        });
        this._notify();
        return true;
    }

    /** Add a file reference (resource_link) */
    addFile(filePath, fileName, mimeType) {
        if (this.attachments.length >= MAX_ATTACHMENTS) return false;
        const uri = 'file:///' + filePath.replace(/\\/g, '/');
        this.attachments.push({
            type: 'resource_link',
            uri,
            name: fileName,
            mimeType: mimeType || guessMimeType(fileName)
        });
        this._notify();
        return true;
    }

    removeAt(index) {
        const removed = this.attachments.splice(index, 1);
        if (removed[0]?.previewUrl?.startsWith('blob:')) {
            URL.revokeObjectURL(removed[0].previewUrl);
        }
        this._notify();
    }

    clear() {
        for (const att of this.attachments) {
            if (att.previewUrl?.startsWith('blob:')) URL.revokeObjectURL(att.previewUrl);
        }
        this.attachments = [];
        this._notify();
    }

    hasAttachments() {
        return this.attachments.length > 0;
    }

    /** Build the attachments array for the Tauri invoke call */
    toContentBlocks() {
        if (!this.hasAttachments()) return null;
        return this.attachments.map(att => {
            if (att.type === 'image') {
                return { type: 'image', data: att.data, mimeType: att.mimeType };
            }
            return { type: 'resource_link', uri: att.uri, name: att.name, mimeType: att.mimeType };
        });
    }
}

/**
 * Handle paste events on an input element, extracting images from clipboard.
 * @param {ClipboardEvent} event
 * @param {AttachmentManager} manager
 */
export function handlePasteEvent(event, manager) {
    const items = event.clipboardData?.items;
    if (!items) return;

    for (const item of items) {
        if (SUPPORTED_IMAGE_TYPES.includes(item.type)) {
            event.preventDefault();
            if (manager.attachments.length >= MAX_ATTACHMENTS) {
                showLimitToast(event.target);
                return;
            }
            const file = item.getAsFile();
            if (file && file.size <= MAX_IMAGE_SIZE) {
                fileToBase64(file).then(({ base64, mimeType }) => {
                    manager.addImage(base64, mimeType);
                });
            }
            return; // only handle first image
        }
    }
}

/**
 * Set up drag-and-drop on a target element.
 * @param {HTMLElement} dropTarget - element to listen for drops
 * @param {HTMLElement} overlayTarget - element to show drag overlay on
 * @param {AttachmentManager} manager
 */
export function setupDragDrop(dropTarget, overlayTarget, manager) {
    let dragCounter = 0;

    dropTarget.addEventListener('dragenter', (e) => {
        e.preventDefault();
        dragCounter++;
        overlayTarget.classList.add('drag-over');
    });

    dropTarget.addEventListener('dragleave', (e) => {
        e.preventDefault();
        dragCounter--;
        if (dragCounter <= 0) {
            dragCounter = 0;
            overlayTarget.classList.remove('drag-over');
        }
    });

    dropTarget.addEventListener('dragover', (e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = 'copy';
    });

    dropTarget.addEventListener('drop', (e) => {
        e.preventDefault();
        dragCounter = 0;
        overlayTarget.classList.remove('drag-over');

        const files = e.dataTransfer?.files;
        if (!files || files.length === 0) return;

        for (const file of files) {
            if (manager.attachments.length >= MAX_ATTACHMENTS) {
                showLimitToast(dropTarget);
                break;
            }
            if (SUPPORTED_IMAGE_TYPES.includes(file.type) && file.size <= MAX_IMAGE_SIZE) {
                fileToBase64(file).then(({ base64, mimeType }) => {
                    manager.addImage(base64, mimeType);
                });
            } else if (file.name) {
                const path = file.path || file.name;
                manager.addFile(path, file.name, file.type || guessMimeType(file.name));
            }
        }
    });
}

/**
 * Render the attachment preview strip into a container element.
 * @param {HTMLElement} container
 * @param {Array} attachments
 * @param {AttachmentManager} manager
 */
export function renderAttachmentPreviews(container, attachments, manager) {
    container.innerHTML = '';
    if (!attachments || attachments.length === 0) {
        container.style.display = 'none';
        return;
    }
    container.style.display = 'flex';

    attachments.forEach((att, index) => {
        const item = document.createElement('div');
        item.className = 'attachment-preview-item';

        if (att.type === 'image' && att.previewUrl) {
            const img = document.createElement('img');
            img.src = att.previewUrl;
            img.alt = 'Attached image';
            img.className = 'attachment-preview-img';
            item.appendChild(img);
        } else {
            const icon = document.createElement('span');
            icon.className = 'attachment-preview-file-icon';
            icon.textContent = '📄';
            item.appendChild(icon);
            const name = document.createElement('span');
            name.className = 'attachment-preview-file-name';
            name.textContent = att.name || 'file';
            item.appendChild(name);
        }

        const removeBtn = document.createElement('button');
        removeBtn.className = 'attachment-remove-btn';
        removeBtn.innerHTML = '×';
        removeBtn.setAttribute('aria-label', 'Remove attachment');
        removeBtn.addEventListener('click', (e) => {
            e.stopPropagation();
            manager.removeAt(index);
        });
        item.appendChild(removeBtn);

        container.appendChild(item);
    });

    // Always show counter
    const counter = document.createElement('span');
    counter.className = 'attachment-counter';
    counter.textContent = `${attachments.length}/${MAX_ATTACHMENTS}`;
    container.appendChild(counter);
}

/**
 * Create attachment preview HTML for displaying in sent user messages.
 * @param {Array} attachments
 * @returns {string} HTML string
 */
export function attachmentPreviewHtml(attachments) {
    if (!attachments || attachments.length === 0) return '';
    let html = '<div class="message-attachments">';
    for (const att of attachments) {
        if (att.type === 'image' && att.previewUrl) {
            html += `<img src="${att.previewUrl}" alt="Attached image" class="message-attachment-img">`;
        } else if (att.type === 'resource_link') {
            html += `<span class="message-attachment-file">📄 ${escapeHtml(att.name || 'file')}</span>`;
        }
    }
    html += '</div>';
    return html;
}

// --- Utilities ---

/** Show a brief disappearing toast near the target element */
function showLimitToast(nearElement) {
    // Remove any existing toast
    const existing = document.querySelector('.attachment-limit-toast');
    if (existing) existing.remove();

    const toast = document.createElement('div');
    toast.className = 'attachment-limit-toast';
    toast.textContent = `Limit of ${MAX_ATTACHMENTS} attachments reached`;
    document.body.appendChild(toast);

    // Auto-remove after animation
    setTimeout(() => toast.remove(), 2500);
}

/**
 * Convert a JSONL session image content block to a data URL.
 * JSONL format: {kind:"image", data:{format:"png", source:{kind:"bytes", data:[...bytes]}}}
 * @param {object} imageItem - the content item from JSONL
 * @returns {string|null} data URL or null if not convertible
 */
export function sessionImageToDataUrl(imageItem) {
    try {
        const imgData = imageItem.data;
        if (!imgData || !imgData.source || !imgData.source.data) return null;
        const bytes = imgData.source.data;
        const format = imgData.format || 'png';
        const mimeType = `image/${format}`;
        // Convert byte array to base64
        let binary = '';
        for (let i = 0; i < bytes.length; i++) {
            binary += String.fromCharCode(bytes[i]);
        }
        const base64 = btoa(binary);
        return `data:${mimeType};base64,${base64}`;
    } catch (e) {
        console.error('Failed to convert session image:', e);
        return null;
    }
}

function fileToBase64(file) {
    return new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => {
            // result is "data:<mime>;base64,<data>"
            const dataUrl = reader.result;
            const base64 = dataUrl.split(',')[1];
            resolve({ base64, mimeType: file.type });
        };
        reader.onerror = reject;
        reader.readAsDataURL(file);
    });
}

function guessMimeType(filename) {
    const ext = (filename || '').split('.').pop()?.toLowerCase();
    const map = {
        'rs': 'text/x-rust', 'js': 'text/javascript', 'ts': 'text/typescript',
        'py': 'text/x-python', 'java': 'text/x-java', 'go': 'text/x-go',
        'c': 'text/x-c', 'cpp': 'text/x-c++', 'h': 'text/x-c',
        'css': 'text/css', 'html': 'text/html', 'json': 'application/json',
        'xml': 'text/xml', 'yaml': 'text/yaml', 'yml': 'text/yaml',
        'md': 'text/markdown', 'txt': 'text/plain', 'sh': 'text/x-shellscript',
        'toml': 'text/x-toml', 'sql': 'text/x-sql',
        'png': 'image/png', 'jpg': 'image/jpeg', 'jpeg': 'image/jpeg',
        'gif': 'image/gif', 'webp': 'image/webp', 'svg': 'image/svg+xml',
    };
    return map[ext] || 'application/octet-stream';
}

function escapeHtml(str) {
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
}
