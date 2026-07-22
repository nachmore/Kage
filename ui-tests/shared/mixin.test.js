/**
 * Tests for the descriptor-preserving mixin applier (ui/js/shared/mixin.js).
 *
 * The bug this pins: `Object.assign(Target.prototype, mixin)` evaluates
 * mixin getters ONCE at copy time (with `this` = the bare mixin object)
 * and freezes the result onto the target as a data property; setters are
 * silently dropped. That broke FloatingApp's speech-state accessors
 * (`isSpeechListening` stuck at false, `_usedSpeechForLastMessage`
 * setter discarded) after the app.js split.
 */

import { describe, it, expect } from 'vitest';
import { applyMixin } from '../../ui/js/shared/mixin.js';
import { FloatingApp } from '../../ui/js/floating/app.js';

describe('applyMixin', () => {
    it('preserves getters as live accessors', () => {
        const mixin = {
            get value() {
                return this.backing;
            },
        };
        class Target {}
        applyMixin(Target.prototype, mixin);

        const t = new Target();
        t.backing = 42;
        expect(t.value).toBe(42);
        t.backing = 43;
        expect(t.value).toBe(43);
    });

    it('preserves setters instead of dropping them', () => {
        const mixin = {
            get value() {
                return this.backing;
            },
            set value(v) {
                this.backing = v;
            },
        };
        class Target {}
        applyMixin(Target.prototype, mixin);

        const t = new Target();
        t.value = 'hello';
        expect(t.backing).toBe('hello');
    });

    it('copies plain methods like Object.assign does', () => {
        const mixin = {
            greet() {
                return `hi ${this.name}`;
            },
        };
        class Target {}
        applyMixin(Target.prototype, mixin);

        const t = new Target();
        t.name = 'kage';
        expect(t.greet()).toBe('hi kage');
    });

    it('accepts a class and uses its prototype, skipping constructor', () => {
        class Mixin {
            get doubled() {
                return this.n * 2;
            }
            bump() {
                this.n += 1;
            }
        }
        class Target {
            constructor() {
                this.marker = 'target';
            }
        }
        const originalCtor = Target.prototype.constructor;
        applyMixin(Target.prototype, Mixin);

        expect(Target.prototype.constructor).toBe(originalCtor);
        const t = new Target();
        t.n = 3;
        expect(t.doubled).toBe(6);
        t.bump();
        expect(t.n).toBe(4);
    });
});

describe('FloatingApp speech accessors (regression)', () => {
    it('isSpeechListening is a live accessor reading this.speech', () => {
        const desc = Object.getOwnPropertyDescriptor(
            FloatingApp.prototype,
            'isSpeechListening'
        );
        expect(typeof desc.get).toBe('function');

        const app = Object.create(FloatingApp.prototype);
        expect(app.isSpeechListening).toBe(false);
        app.speech = { isListening: true };
        expect(app.isSpeechListening).toBe(true);
    });

    it('_usedSpeechForLastMessage keeps both getter and setter', () => {
        const desc = Object.getOwnPropertyDescriptor(
            FloatingApp.prototype,
            '_usedSpeechForLastMessage'
        );
        expect(typeof desc.get).toBe('function');
        expect(typeof desc.set).toBe('function');

        const app = Object.create(FloatingApp.prototype);
        app.speech = { usedSpeechForLastMessage: false };
        app._usedSpeechForLastMessage = true;
        expect(app.speech.usedSpeechForLastMessage).toBe(true);
        expect(app._usedSpeechForLastMessage).toBe(true);
    });
});
