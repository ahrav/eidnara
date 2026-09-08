import { chmodSync, mkdirSync, realpathSync, renameSync, statSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

/**
 * A symlinked target is resolved first so the rename replaces the file the
 * link points at and the link itself survives.
 */
function resolveWriteTarget(targetPath: string): string {
    try {
        return realpathSync(targetPath);
    } catch {
        return targetPath;
    }
}

/**
 * When targetPath names a file and chmodSync succeeds, writeFileAtomic copies its 0o777 permission bits to tmpPath.
 *
 * Callers need not create the parent directory.
 */
export function writeFileAtomic(targetPath: string, data: string): void {
    mkdirSync(dirname(targetPath), { recursive: true });
    const resolvedTarget = resolveWriteTarget(targetPath);
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
