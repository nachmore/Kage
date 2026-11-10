/**
 * Settings → Tool permissions → Audit log: the rows for files extensions
 * read (docs/design/EXTENSION_FILE_TEXT.md §4.5, §5.13). One row per read
 * file, for both outcomes, with a "File reads" filter. The extension is
 * named as the user knows it, and nothing in an entry can become markup.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';
import { initI18n } from '../../ui/js/shared/i18n.js';
import {
    FILE_READ_EVENT,
    fileReadFilterOption,
    fileReadRow,
    formatFileSize,
    loadFileReadNames,
} from '../../ui/js/settings/audit-file-reads.js';

const EN_CATALOG = JSON.parse(
    readFileSync(
        join(dirname(fileURLToPath(import.meta.url)), '..', '..', 'locales', 'en', 'messages.json'),
        'utf-8'
    )
);

const flush = (ms = 0) => new Promise((r) => setTimeout(r, ms));

/** An `extension_file_read` entry as the backend serialises it. */
function readEntry(overrides = {}) {
    return {
        at: '2026-11-01T10:00:00Z',
        event: 'extension_file_read',
        extension_id: 'mindmap',
        file_name: 'Q3 plan.pdf',
        format: 'pdf',
        bytes: 50_541_363,
        chars: 1234,
        truncated: false,
        outcome: 'ok',
        duration_ms: 812,
        ...overrides,
    };
}

/** Render a row's parts into a node, the way the audit list does. */
function rowNode(parts) {
    const div = document.createElement('div');
    div.innerHTML = `<div class="audit-log-summary">${parts.summary}</div><div class="audit-log-meta">${parts.meta}</div>`;
    return div;
}

const MANIFESTS = [
    { manifest: { id: 'mindmap', name: 'Mind Map', icon: '🧠', version: '1.0.0' } },
    { manifest: { id: 'notes', name: '__MSG_manifest_name__', version: '1.0.0' } },
    { manifest: { id: 'other', name: 'Other', version: '1.0.0' } },
];

let ToolPermissionsSettingsModule;
beforeAll(async () => {
    await initI18n(async () => ({
        language: 'en',
        rtl: false,
        catalog: EN_CATALOG,
        fallback: EN_CATALOG,
    }));
    ({ ToolPermissionsSettingsModule } = await import('../../ui/js/settings/tool-permissions.js'));
});

afterEach(() => {
    teardownTauriMock();
    vi.restoreAllMocks();
    document.body.innerHTML = '';
});

describe('a file-read row', () => {
    const names = new Map([['mindmap', 'Mind Map']]);

    it('a read file: the extension, the file, its format, size and characters', () => {
        const parts = fileReadRow(readEntry(), names);
        expect(parts.icon).toBe('📄');
        expect(parts.meta).toBe('');
        const node = rowNode(parts);
        expect(node.querySelector('.audit-log-summary').textContent).toBe(
            'Mind Map read Q3 plan.pdf (pdf, 48.2 MB, 1,234 characters)'
        );
        expect(node.querySelector('strong').textContent).toBe('Mind Map');
    });

    it('a failed read: "couldn\'t read", with the outcome code as a muted tag', () => {
        const parts = fileReadRow(
            readEntry({ outcome: 'too_large', chars: 0, file_name: 'scan.pdf' }),
            names
        );
        expect(parts.icon).toBe('📄');
        const node = rowNode(parts);
        expect(node.querySelector('.audit-log-summary').textContent).toBe(
            "Mind Map couldn't read scan.pdf"
        );
        const tag = node.querySelector('.audit-log-meta code');
        expect(tag.textContent).toBe('too_large');
        expect(tag.getAttribute('dir')).toBe('ltr');
    });

    it('a cancelled read and an entry with no outcome are failures too', () => {
        const cancelled = rowNode(fileReadRow(readEntry({ outcome: 'cancelled' }), names));
        expect(cancelled.textContent).toContain("couldn't read");
        expect(cancelled.querySelector('code').textContent).toBe('cancelled');
        const bare = rowNode(fileReadRow({ event: 'extension_file_read' }));
        expect(bare.textContent).toContain("? couldn't read a file (name not shown)");
        expect(bare.querySelector('code').textContent).toBe('?');
    });

    it('names sit in <bdi>, so a right-to-left name keeps the row in order', () => {
        const node = rowNode(fileReadRow(readEntry({ file_name: 'خطة الربع.pdf' }), names));
        const isolates = [...node.querySelectorAll('bdi')];
        const file = isolates.find((b) => b.textContent === 'خطة الربع.pdf');
        expect(file.getAttribute('dir')).toBe('auto');
        expect(isolates.find((b) => b.textContent === 'Mind Map').getAttribute('dir')).toBe(
            'auto'
        );
        expect(isolates.find((b) => b.textContent === 'pdf').getAttribute('dir')).toBe('ltr');
        expect(isolates.find((b) => b.textContent === '48.2 MB').getAttribute('dir')).toBe('ltr');
    });

    it('a hostile file name, extension name, format or outcome stays text', () => {
        const evil = '<img src=x onerror="window.__pwned=1">';
        const hostileNames = new Map([['mindmap', evil]]);
        for (const entry of [
            readEntry({ file_name: evil }),
            readEntry({ format: evil }),
            readEntry({ outcome: evil }),
            readEntry({ extension_id: evil }),
        ]) {
            const node = rowNode(fileReadRow(entry, hostileNames));
            expect(node.querySelector('img')).toBeNull();
        }
        const node = rowNode(fileReadRow(readEntry({ file_name: evil }), names));
        expect([...node.querySelectorAll('bdi')].map((b) => b.textContent)).toContain(evil);
    });

    it('a name Kage didn\'t keep (it looked like a key) says so', () => {
        const ok = rowNode(fileReadRow(readEntry({ file_name: undefined }), names));
        expect(ok.textContent).toContain('Mind Map read a file (name not shown) (pdf,');
        const failed = rowNode(
            fileReadRow(readEntry({ file_name: undefined, outcome: 'malformed' }), names)
        );
        expect(failed.textContent).toContain("Mind Map couldn't read a file (name not shown)");
    });

    it('an extension that is no longer installed is named by its id', () => {
        const node = rowNode(fileReadRow(readEntry({ extension_id: 'gone' }), names));
        expect(node.querySelector('strong').textContent).toBe('gone');
    });

    it('missing or odd numbers show as zero, never NaN', () => {
        const node = rowNode(
            fileReadRow(readEntry({ bytes: 'lots', chars: -3, format: null }), names)
        );
        expect(node.textContent).toContain('(?, 0 B, 0 characters)');
        expect(node.textContent).not.toContain('NaN');
    });
});

describe('formatFileSize', () => {
    it('writes sizes as the host\'s error messages do', () => {
        // The same cases as `format_size` in src/file_text/tests.rs.
        expect(formatFileSize(0)).toBe('0 B');
        expect(formatFileSize(1023)).toBe('1023 B');
        expect(formatFileSize(1024)).toBe('1 KB');
        expect(formatFileSize(1536)).toBe('1.5 KB');
        expect(formatFileSize(32 * 1024 * 1024)).toBe('32 MB');
        expect(formatFileSize(50_541_363)).toBe('48.2 MB');
        expect(formatFileSize(undefined)).toBe('0 B');
    });
});

describe('loadFileReadNames', () => {
    it('asks the backend nothing when no entry is a file read', async () => {
        const invoke = vi.fn();
        const names = await loadFileReadNames(
            [{ event: 'denied', tool: 'shell' }, { event: 'granted' }],
            invoke
        );
        expect(names.size).toBe(0);
        expect(invoke).not.toHaveBeenCalled();
    });

    it('names the installed extensions that read files, resolving __MSG_ names', async () => {
        const invoke = vi.fn(async (cmd, args) => {
            if (cmd === 'list_extensions') return MANIFESTS;
            if (cmd === 'read_extension_locale' && args.extensionId === 'notes') {
                return { manifest_name: { message: 'Notes' } };
            }
            return null;
        });
        const names = await loadFileReadNames(
            [
                readEntry(),
                readEntry({ extension_id: 'notes' }),
                readEntry({ extension_id: 'gone' }),
                { event: 'denied', tool: 'shell' },
            ],
            invoke
        );
        expect([...names]).toEqual([
            ['mindmap', 'Mind Map'],
            ['notes', 'Notes'],
        ]);
        // Only the token name needed its catalog; `other` read nothing.
        const localeCalls = invoke.mock.calls.filter(([c]) => c === 'read_extension_locale');
        expect(localeCalls.every(([, a]) => a.extensionId === 'notes')).toBe(true);
    });

    it('an unresolved __MSG_ name falls back to the id, never the token', async () => {
        const invoke = vi.fn(async (cmd) => (cmd === 'list_extensions' ? MANIFESTS : null));
        const names = await loadFileReadNames([readEntry({ extension_id: 'notes' })], invoke);
        expect(names.get('notes')).toBe('notes');
    });

    it('cleans a manifest name: no bidi controls, at most 64 characters', async () => {
        const long = `\u202eevil\u202c ${'x'.repeat(100)}`;
        const invoke = vi.fn(async () => [{ manifest: { id: 'mindmap', name: long } }]);
        const names = await loadFileReadNames([readEntry()], invoke);
        const name = names.get('mindmap');
        expect(name.startsWith('evil x')).toBe(true);
        expect(name).not.toMatch(/[\u202a-\u202e]/);
        expect([...name].length).toBeLessThanOrEqual(64);
    });

    it('a failing list_extensions leaves the ids', async () => {
        const invoke = vi.fn(async () => {
            throw new Error('boom');
        });
        const names = await loadFileReadNames([readEntry()], invoke);
        expect(names.size).toBe(0);
        expect(rowNode(fileReadRow(readEntry(), names)).querySelector('strong').textContent).toBe(
            'mindmap'
        );
    });
});

describe('the audit log in Settings → Tool permissions', () => {
    const AUDIT = [
        readEntry(),
        readEntry({
            at: '2026-11-01T09:30:00Z',
            file_name: 'old.doc',
            format: 'docx',
            chars: 0,
            outcome: 'legacy_format',
        }),
        { at: '2026-11-01T09:00:00Z', event: 'denied', tool: 'shell' },
    ];

    function mount(audit = AUDIT) {
        const mock = setupTauriMock({
            get_permission_audit_log: async () => audit,
            get_permission_audit_log_path: async () => '/tmp/audit.jsonl',
            list_extensions: async () => MANIFESTS,
        });
        const mod = new ToolPermissionsSettingsModule();
        document.body.innerHTML = `<div data-section-content="tool-permissions">${mod.render()}</div>`;
        document.getElementById('terminatorMascot')?.remove();
        mod.initialize();
        return { mod, mock };
    }

    const rows = () => [...document.querySelectorAll('#auditLogList .audit-log-row')];
    const filter = () => document.getElementById('auditLogFilter');

    beforeEach(() => {
        vi.spyOn(console, 'error').mockImplementation(() => {});
    });

    it('offers a "File reads" filter', () => {
        mount();
        const option = filter().querySelector(`option[value="${FILE_READ_EVENT}"]`);
        expect(option.textContent).toBe('File reads');
        expect(fileReadFilterOption()).toContain(`value="${FILE_READ_EVENT}"`);
    });

    it('renders a row for each read, for both outcomes, with the extension named', async () => {
        mount();
        await flush();
        const all = rows();
        expect(all).toHaveLength(3);
        expect(all[0].querySelector('.audit-log-icon').textContent).toBe('📄');
        expect(all[0].querySelector('.audit-log-summary').textContent).toBe(
            'Mind Map read Q3 plan.pdf (pdf, 48.2 MB, 1,234 characters)'
        );
        expect(all[1].querySelector('.audit-log-summary').textContent).toBe(
            "Mind Map couldn't read old.doc"
        );
        expect(all[1].querySelector('.audit-log-meta code').textContent).toBe('legacy_format');
        // Not the "unknown event" fallback.
        expect(document.getElementById('auditLogList').textContent).not.toContain(
            'Unknown event'
        );
    });

    it('"File reads" shows only the file reads', async () => {
        mount();
        await flush();
        filter().value = FILE_READ_EVENT;
        filter().dispatchEvent(new Event('change'));
        const shown = rows();
        expect(shown).toHaveLength(2);
        expect(shown.every((r) => r.textContent.includes('Mind Map'))).toBe(true);
    });

    it('a log without file reads never lists the extensions', async () => {
        const { mock } = mount([{ at: '2026-11-01T09:00:00Z', event: 'denied', tool: 'shell' }]);
        await flush();
        expect(rows()).toHaveLength(1);
        expect(mock.invoke).not.toHaveBeenCalledWith('list_extensions', expect.anything());
        expect(mock.invoke.mock.calls.map(([c]) => c)).not.toContain('list_extensions');
    });
});
