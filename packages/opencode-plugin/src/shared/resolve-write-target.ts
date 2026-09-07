import { lstatSync, readlinkSync } from "node:fs";
import { dirname, resolve } from "node:path";

const MAX_SYMLINK_HOPS = 40;

/**
 * Resolving final-component symlinks preserves the link during atomic replacement.
 * `readlinkSync` returns a dangling symlink's target path, so a link whose target is absent is not replaced either.
 */
export function resolveWriteTarget(filePath: string): string {
    let current = filePath;
    for (let hops = 0; hops < MAX_SYMLINK_HOPS; hops += 1) {
        const stat = lstatSync(current, { throwIfNoEntry: false });
        if (stat === undefined || !stat.isSymbolicLink()) break;
        current = resolve(dirname(current), readlinkSync(current));
    }
    return current;
}
