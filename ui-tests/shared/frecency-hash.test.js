import { describe, it, expect } from 'vitest';
import {
    sha256,
    hmacSha256,
    createFrecencyHasher,
    HASHED_KEY_PREFIX,
} from '../../ui/js/shared/frecency-hash.js';

const hex = (bytes) => Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
const utf8 = (s) => new TextEncoder().encode(s);
const fromHex = (h) => Uint8Array.from(h.match(/../g), (b) => parseInt(b, 16));

describe('sha256', () => {
    it('matches the FIPS 180-2 test vectors', () => {
        expect(hex(sha256(utf8('')))).toBe(
            'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
        );
        expect(hex(sha256(utf8('abc')))).toBe(
            'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
        );
        expect(hex(sha256(utf8('abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq')))).toBe(
            '248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1'
        );
    });

    it('handles inputs around the block-padding boundaries', () => {
        // 55, 56 and 64 bytes straddle the length-field boundary.
        expect(hex(sha256(utf8('a'.repeat(55))))).toBe(
            '9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318'
        );
        expect(hex(sha256(utf8('a'.repeat(56))))).toBe(
            'b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a'
        );
        expect(hex(sha256(utf8('a'.repeat(64))))).toBe(
            'ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb'
        );
    });
});

describe('hmacSha256', () => {
    it('matches RFC 4231 test cases 1, 2 and 6', () => {
        expect(hex(hmacSha256(new Uint8Array(20).fill(0x0b), utf8('Hi There')))).toBe(
            'b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7'
        );
        expect(hex(hmacSha256(utf8('Jefe'), utf8('what do ya want for nothing?')))).toBe(
            '5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843'
        );
        // Key longer than the block size is hashed first.
        expect(
            hex(
                hmacSha256(
                    new Uint8Array(131).fill(0xaa),
                    utf8('Test Using Larger Than Block-Size Key - Hash Key First')
                )
            )
        ).toBe('60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54');
    });
});

describe('createFrecencyHasher', () => {
    const KEY = btoa(String.fromCharCode(...fromHex('00112233445566778899aabbccddeeff'.repeat(2))));

    it('rejects missing or short keys so callers never fall back to raw values', () => {
        expect(createFrecencyHasher(null)).toBeNull();
        expect(createFrecencyHasher('')).toBeNull();
        expect(createFrecencyHasher('not base64!')).toBeNull();
        expect(createFrecencyHasher(btoa('short'))).toBeNull();
    });

    it('is deterministic per key, separates ids from prefixes, and never echoes the input', () => {
        const a = createFrecencyHasher(KEY);
        const b = createFrecencyHasher(KEY);
        const secret = 'pw:Tr0ub4dor&3-correct-horse';
        expect(a.id(secret)).toBe(b.id(secret));
        expect(a.id(secret).startsWith(HASHED_KEY_PREFIX)).toBe(true);
        expect(a.id(secret)).toMatch(/^h1:[0-9a-f]{32}$/);
        expect(a.id(secret)).not.toContain('Tr0ub4dor');
        expect(a.id('calc')).not.toBe(a.prefix('calc'));
    });

    it('gives different hashes under a different key', () => {
        const other = btoa(String.fromCharCode(...new Uint8Array(32).fill(7)));
        expect(createFrecencyHasher(KEY).id('app:Safari')).not.toBe(
            createFrecencyHasher(other).id('app:Safari')
        );
    });
});
