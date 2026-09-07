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

/** Own enumerable string keys; array index keys come first in ascending numeric order, holes omitted. */
export function ownKeys(target: object): string[] {
    try {
        return Object.keys(target);
    } catch {
        return [];
    }
}
