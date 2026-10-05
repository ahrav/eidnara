/**
 * Publication into a host-owned message array: the container checks a publication needs and the
 * all-or-none in-place write. Harness adapters that publish in place import this module; the
 * transform session client does not.
 */

import { types } from "node:util";

// Object.defineProperty bypasses inherited numeric setters on the host array.
function defineSlot<T>(array: T[], index: number, value: T): void {
    Object.defineProperty(array, index, {
        value,
        writable: true,
        enumerable: true,
        configurable: true,
    });
}

type HostArrayRejectionReason =
    | "proxy"
    | "not_array"
    | "not_extensible"
    | "length_not_writable"
    | "slot_not_configurable";

/**
 * Checks the container and the output slots `[0, slots)` a publication writes; the capture recheck
 * covers every captured window slot. Slots at `slots` and above are deleted by the shrink, which fails
 * explicitly instead of being checked here.
 */
export function publicationRejection(
    target: unknown,
    slots: number,
): HostArrayRejectionReason | null {
    if (types.isProxy(target)) return "proxy";
    if (!Array.isArray(target)) return "not_array";
    // D21 lists the sealed and frozen checks although non-extensibility already implies them.
    if (!Object.isExtensible(target) || Object.isSealed(target) || Object.isFrozen(target))
        return "not_extensible";
    if (!Object.getOwnPropertyDescriptor(target, "length")?.writable) return "length_not_writable";
    const end = Math.min(slots, target.length);
    for (let index = 0; index < end; index += 1) {
        if (Object.getOwnPropertyDescriptor(target, index)?.configurable === false)
            return "slot_not_configurable";
    }
    return null;
}

/** Why a shrink failed and the length `ArraySetLength` left before the window was restored. */
export interface PublicationFailure {
    error: unknown;
    shrunkLength: number;
    detail: string;
}

/**
 * Precondition: `publicationRejection(target, next.length)` returns `null`. Shrinks the length to
 * `next.length` first, then defines each slot, bypassing inherited setters. A non-configurable slot
 * k at or above `next.length` stops the shrink after every slot above k is deleted; no slot of
 * `next` is then written, and the captured `window` references that the shrink removed are put
 * back after k: the part above k when k is inside the window, the whole window after a covered k,
 * whose covered slots above it stay lost. A throw while restoring is reported as the same failure.
 */
export function publishInPlace(
    target: unknown[],
    next: readonly unknown[],
    window: readonly unknown[],
    boundaryIndex: number,
): PublicationFailure | undefined {
    try {
        if (target.length > next.length)
            Object.defineProperty(target, "length", { value: next.length });
    } catch (error) {
        const shrunkLength = target.length;
        // E.g. boundaryIndex 3 and k 4 leave length 5, so first = 2 and window[2..] lands at 5.
        const first = Math.max(0, shrunkLength - boundaryIndex);
        let restoreError: unknown;
        try {
            for (let index = first; index < window.length; index += 1)
                defineSlot(target, shrunkLength + index - first, window[index]);
        } catch (thrown) {
            restoreError = thrown;
        }
        const restored =
            restoreError === undefined
                ? `restored ${window.length - first} captured window references`
                : `restoring captured window references failed (${String(restoreError)})`;
        return {
            error,
            shrunkLength,
            detail: `shrink to ${next.length} stopped at length ${shrunkLength} (${String(error)}); ${restored}`,
        };
    }
    for (let index = 0; index < next.length; index += 1) defineSlot(target, index, next[index]);
    return undefined;
}
