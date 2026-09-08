import {
    chmodSync,
    lstatSync,
    mkdirSync,
    realpathSync,
    renameSync,
    statSync,
    writeFileSync,
} from "node:fs";
import { dirname } from "node:path";

/**
 * When targetPath names a file and chmodSync succeeds, writeFileAtomic copies its 0o777 permission bits to tmpPath.
 *
 * Callers need not create the parent directory.
 *
 * A non-dangling symlink resolves before renameSync, preserving the symlink.
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

/** A dangling symlink resolves to itself; the rename then replaces the link like any other missing target. */
function resolveSymlinkTarget(path: string): string {
    try {
        if (!lstatSync(path).isSymbolicLink()) return path;
        return realpathSync(path);
    } catch {
        return path;
    }
}
