/**
 * Property access over untrusted objects returns `undefined` (or no keys) when a getter or proxy
 * trap throws, so classifiers and formatters can keep reading the remaining members.
 */

export function readField(target: object, key: PropertyKey): unknown {
    try {
        return (target as Record<PropertyKey, unknown>)[key];
    } catch {
        return undefined;
    }
}

export function ownKeys(target: object): string[] {
    try {
        return Object.keys(target);
    } catch {
        return [];
    }
}

/** A `length` that cannot be read, or is not a non-negative integer, counts as `0`. */
export function readLength(target: object): number {
    const length = readField(target, "length");
    return typeof length === "number" && Number.isInteger(length) && length >= 0 ? length : 0;
}
