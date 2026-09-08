import { randomBytes } from "node:crypto";
import {
    chmodSync,
    mkdirSync,
    realpathSync,
    renameSync,
    rmSync,
    statSync,
    writeFileSync,
} from "node:fs";
import { dirname } from "node:path";

/**
 * An existing symlink resolves to its target, so `renameSync` replaces the target, not the link.
 * The staged file is created no more permissively than the existing regular file.
 * A failed write or rename removes the staged sibling before the error propagates.
 * Callers need not create the parent directory.
 */
export function writeFileAtomic(targetPath: string, data: string): void {
    mkdirSync(dirname(targetPath), { recursive: true });
    const finalPath = resolveLinkTarget(targetPath);
    const mode = existingFileMode(finalPath);
    const tmpPath = `${finalPath}.${process.pid}.${randomBytes(6).toString("hex")}.tmp`;
    try {
        writeFileSync(tmpPath, data, { encoding: "utf-8", mode: mode ?? 0o666 });
        // open(2) masks the creation mode with the umask; chmod restores the exact bits.
        if (mode !== undefined) chmodSync(tmpPath, mode);
        renameSync(tmpPath, finalPath);
    } catch (error) {
        rmSync(tmpPath, { force: true });
        throw error;
    }
}

function resolveLinkTarget(path: string): string {
    try {
        return realpathSync.native(path);
    } catch {
        // A missing target or a dangling link keeps the given path; the rename creates it.
        return path;
    }
}

function existingFileMode(path: string): number | undefined {
    try {
        const stat = statSync(path, { throwIfNoEntry: false });
        return stat?.isFile() ? stat.mode & 0o777 : undefined;
    } catch {
        return undefined;
    }
}
