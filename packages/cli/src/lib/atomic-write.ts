import {
    chmodSync,
    lstatSync,
    mkdirSync,
    readlinkSync,
    realpathSync,
    renameSync,
    statSync,
    writeFileSync,
} from "node:fs";
import { dirname, resolve } from "node:path";

/**
 * When targetPath names a file and chmodSync succeeds, writeFileAtomic copies its 0o777 permission bits to tmpPath.
 *
 * Callers need not create the parent directory.
 *
 * A symlink resolves to its target before renameSync, preserving the symlink.
 */
export function writeFileAtomic(targetPath: string, data: string): void {
    const destination = resolveSymlinkTarget(targetPath);
    mkdirSync(dirname(destination), { recursive: true });
    const tmpPath = `${destination}.tmp`;
    writeFileSync(tmpPath, data, { encoding: "utf-8" });
    try {
        if (statSync(destination, { throwIfNoEntry: false })?.isFile()) {
            const mode = statSync(destination).mode & 0o777;
            chmodSync(tmpPath, mode);
        }
    } catch {
        // If statSync or chmodSync throws, writeFileAtomic still attempts renameSync(tmpPath, destination).
    }
    renameSync(tmpPath, destination);
}

/** Mirrors the kernel's symlink-following limit so a link cycle terminates instead of looping. */
const MAX_SYMLINK_HOPS = 40;

/** `realpathSync` cannot resolve a dangling symlink, so the fallback follows link text hop by hop and the write creates the final target. commentlint: allow(JUDGE) */
function resolveSymlinkTarget(path: string): string {
    try {
        if (!lstatSync(path).isSymbolicLink()) return path;
    } catch {
        return path;
    }
    try {
        return realpathSync(path);
    } catch {
        let current = path;
        for (let hop = 0; hop < MAX_SYMLINK_HOPS; hop++) {
            let isLink: boolean;
            try {
                isLink = lstatSync(current).isSymbolicLink();
            } catch {
                return current;
            }
            if (!isLink) return current;
            current = resolve(dirname(current), readlinkSync(current));
        }
        throw new Error(`Too many levels of symbolic links: ${path}`);
    }
}
