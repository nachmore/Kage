/**
 * Descriptor-preserving mixin applier.
 *
 * `Object.assign(Target.prototype, mixin)` copies getter/setter VALUES,
 * not accessors: a `get foo()` on the mixin is evaluated once at module
 * load (with `this` = the bare mixin object) and frozen onto the target
 * as a plain data property, and any matching setter is silently dropped.
 * This helper copies the full property descriptors instead, so live
 * accessors stay live on the target prototype.
 *
 * Accepts either a plain methods object or a class (in which case its
 * prototype is used).
 */
export function applyMixin(target, mixin) {
    const source = typeof mixin === 'function' ? mixin.prototype : mixin;
    const descriptors = Object.getOwnPropertyDescriptors(source);
    delete descriptors.constructor;
    Object.defineProperties(target, descriptors);
}
