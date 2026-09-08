import {
    chmodSync,
    lstatSync,
    mkdirSync,
    readlinkSync,
    renameSync,
    statSync,
    writeFileSync,
} from "node:fs";
import { dirname, resolve } from "node:path";

const MAX_SYMLINK_HOPS = 40;

/**
 * Resolve link text instead of calling `realpathSync`: dangling links' missing
 * targets remain writable without replacing the link.
 */
function resolveWriteTarget(targetPath: string): string {
    let current = targetPath;
    for (let hops = 0; hops < MAX_SYMLINK_HOPS; hops++) {
        if (!lstatSync(current, { throwIfNoEntry: false })?.isSymbolicLink()) return current;
        current = resolve(dirname(current), readlinkSync(current));
    }
    throw new Error(`Too many levels of symbolic links: ${targetPath}`);
}

/**
 * When targetPath names a file and chmodSync succeeds, writeFileAtomic copies its 0o777 permission bits to tmpPath.
 *
 * Callers need not create the parent directory.
 */
export function writeFileAtomic(targetPath: string, data: string): void {
    mkdirSync(dirname(targetPath), { recursive: true });
    const resolvedTarget = resolveWriteTarget(targetPath);
    mkdirSync(dirname(resolvedTarget), { recursive: true });
    const tmpPath = `${resolvedTarget}.tmp`;
    writeFileSync(tmpPath, data, { encoding: "utf-8" });
    try {
        if (statSync(resolvedTarget, { throwIfNoEntry: false })?.isFile()) {
            const mode = statSync(resolvedTarget).mode & 0o777;
            chmodSync(tmpPath, mode);
        }
    } catch {
        // If statSync or chmodSync throws, writeFileAtomic still attempts renameSync(tmpPath, resolvedTarget).
    }
    renameSync(tmpPath, resolvedTarget);
}
